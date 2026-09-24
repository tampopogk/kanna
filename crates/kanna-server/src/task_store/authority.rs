//! Storage authority: which record wins, SQLite or the task directories
//! (spec §11, §16.11 — component T13, third increment).
//!
//! Every installation has a persisted **authority mode**:
//!
//! - `sql` (the default): SQLite is authoritative and the task directories
//!   are its published projection, as since T0. On disagreement the
//!   database wins and the publisher rewrites the disk.
//! - `disk`: the task directories (`task.json`, the ledger, `repo.json`)
//!   are authoritative and SQLite is a projection of them. On disagreement
//!   the disk wins: at every startup, and whenever the publisher finds the
//!   disk ahead of the database, the database is reconciled from the disk.
//!   A missing database is rebuilt from the disk before the server opens
//!   it.
//!
//! There is only ever one writer. In `sql` mode a mutation commits its rows
//! and its ledger entry in one SQLite transaction (the outbox) and the
//! publisher writes them out within seconds. In `disk` mode (T13d) the same
//! transaction writes its task directory records first, from its own
//! uncommitted rows, and SQLite commits after them
//! ([`super::disk_first`]): the disk is authoritative for writes as well as
//! for disagreements.
//!
//! # Record
//!
//! `<root>/authority/<installation>.json`, replaced atomically:
//! `{schema_version, installation, database, mode, requested, switch,
//! checkpoints}`. `installation` is derived from the database path, so a
//! store root shared by several installations (production and staging both
//! use `~/.kanna`) holds one record each, and every `repo.json` names the
//! installation that wrote it: a rebuild or reconciliation takes in only its
//! own repositories and their tasks. A record that does not exist means
//! `sql`.
//!
//! # Switching
//!
//! A switch is requested (`kanna-server storage-authority disk|sql`, or
//! `KANNA_STORAGE_AUTHORITY=disk|sql` in the server's environment) and
//! performed at the next startup, after the database is migrated and before
//! any service can schedule, dispatch or accept a mutation: the quiescent
//! boundary for every task and repository at once, so the two directions
//! never run a writer concurrently. Each step records a checkpoint before
//! the next begins; a restart at any checkpoint resumes by re-running the
//! steps, all of which are idempotent.
//!
//! `sql` → `disk`:
//! 1. `to_disk.begin` — the switch is recorded; still `sql`.
//! 2. `to_disk.drained` — stale reservations released, pre-ledger history
//!    backfilled, a record owed for every task never published (closed
//!    tasks that predate the disk records) and every repository, and the
//!    outbox drained with nothing left owed.
//! 3. `to_disk.repo_verified` (one per repository) — every task of the
//!    repository has a current `task.json` whose `state` equals its rows,
//!    its ledger files equal its published rows, and `repo.json` equals the
//!    registration. Any difference refuses the switch (`to_disk.refused`).
//! 4. `to_disk.verified` — every repository verified, and no task directory
//!    or tombstone the database does not account for.
//! 5. `to_disk.commit` — the mode is `disk`.
//!
//! `disk` → `sql` (rollback), from any checkpoint of a switch or from
//! `disk` mode:
//! 1. `to_sql.begin`.
//! 2. `to_sql.reconciled` — from `disk` mode only: the database is
//!    reconciled from the disk and the outbox drained, so it holds
//!    everything the disk does.
//! 3. `to_sql.commit` — the mode is `sql`.
//!
//! Because `disk` mode writes the disk first, a rollback is only as safe as
//! what it checks (T13d): `to_sql.reconciled` is recorded only when the
//! reconciled database verifies equal to the disk (the same check that gates
//! `to_disk.verified`); otherwise `to_sql.refused` records the differences,
//! the installation stays `disk` and the next start tries again.
//!
//! # The rollback window for older builds
//!
//! A `disk` record is written as [`DISK_FIRST_RECORD_SCHEMA_VERSION`] from
//! this build's first `disk`-mode start, before any disk-first write, and
//! names when (`disk_first_since`). A build before T13d accepts only
//! [`RECORD_SCHEMA_VERSION`], so it refuses to start on the installation
//! instead of running SQL-first over records it may not hold. Rolling back
//! by starting an older build is therefore closed from that point; rolling
//! back through this build stays open, and a completed rollback writes
//! version 1 again, which reopens older builds. Builds older than T13c do
//! not read the record at all and must never run a `disk` installation.

use super::rebuild::{
    project_store_onto, rebuild_scan_into_new_database, scan_store_records, KnownRows,
    RebuildReport, StoreScan, TaskDirectory,
};
use super::{flush_all, replace_atomically, root_for_db};
use crate::db::task_state::CARRIED_TABLES;
use crate::db::Db;
use crate::db::ReconcileChanges;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// Requests a switch at startup: `disk` or `sql`. The packaged desktop
/// never passes an inherited value on to the server it launches (T13d).
pub const AUTHORITY_ENV: &str = kanna_runtime_defaults::STORAGE_AUTHORITY_ENV;

/// The authority record's own version while the installation is `sql`.
/// Anything this build does not understand is refused.
pub const RECORD_SCHEMA_VERSION: u64 = 1;

/// The record's version while the installation is `disk` (T13d): its
/// writes are disk-first, and a build older than T13d, which understands
/// only [`RECORD_SCHEMA_VERSION`], refuses to open it instead of running
/// SQL-first over records it cannot read.
pub const DISK_FIRST_RECORD_SCHEMA_VERSION: u64 = 2;

/// What a `disk` record tells whoever opens it.
const DISK_FIRST_NOTE: &str = "This installation writes its task directories first (disk authority, T13d). \
Builds older than T13d cannot run it. To leave disk authority, run `kanna-server storage-authority sql` \
with a T13d or newer build and restart it; the rollback is verified before it commits.";

/// Checkpoints kept in the record, newest last.
const CHECKPOINT_HISTORY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Sql,
    Disk,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Sql => "sql",
            Mode::Disk => "disk",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "sql" => Some(Mode::Sql),
            "disk" => Some(Mode::Disk),
            _ => None,
        }
    }
}

/// The installation a database path is: stable for as long as the database
/// lives at that path, and distinct for every installation sharing a root.
pub fn installation_id(db_path: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(db_path.as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ---------------------------------------------------------------------------
// This process's view: installation and mode per store root
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Registered {
    installation: String,
    mode: Mode,
    /// The mode was taken from the persisted record (or set by [`start`]),
    /// not defaulted.
    resolved: bool,
}

static REGISTRY: LazyLock<Mutex<HashMap<PathBuf, Registered>>> = LazyLock::new(Default::default);

/// Name the installation that publishes under `root`. Its mode stays what
/// it was (`sql` until [`start`] says otherwise).
pub fn register(root: &Path, db_path: &str) {
    let mut registry = REGISTRY.lock().unwrap_or_else(|poison| poison.into_inner());
    let installation = installation_id(db_path);
    registry
        .entry(root.to_path_buf())
        .and_modify(|registered| registered.installation = installation.clone())
        .or_insert(Registered {
            installation,
            mode: Mode::Sql,
            resolved: false,
        });
}

fn set_mode(root: &Path, db_path: &str, mode: Mode) {
    register(root, db_path);
    let mut registry = REGISTRY.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(registered) = registry.get_mut(root) {
        registered.mode = mode;
        registered.resolved = true;
    }
}

/// Take `db_path`'s persisted authority mode into this process before its
/// first connection writes (T13d), unless this process already resolved it.
/// Every process that opens the database runs this, so a `disk`
/// installation's writes are gated whichever process makes them (the
/// server, or a subcommand such as `worktree-cleanup` that holds only the
/// path). A process with no configured root finds it by the installation's
/// record under [`super::candidate_roots`]. A record that exists and cannot
/// be read may say `disk`, so it is taken as `disk`.
pub(crate) fn resolve_persisted_mode(db_path: &str) {
    let configured = super::configured_root(db_path);
    let candidates = match &configured {
        Some(root) => vec![root.clone()],
        None => super::candidate_roots(db_path),
    };
    let installation = installation_id(db_path);
    for root in candidates {
        let resolved = REGISTRY
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(&root)
            .is_some_and(|registered| {
                registered.resolved && registered.installation == installation
            });
        if resolved {
            return;
        }
        if !record_path(&root, &installation).exists() {
            continue;
        }
        if configured.is_none() {
            super::adopt_root(db_path, &root);
        }
        let mode = load_record(&root, db_path).map_or_else(
            |error| {
                log::error!("storage authority record unreadable ({error}); gating writes as disk");
                Mode::Disk
            },
            |record| record.mode,
        );
        set_mode(&root, db_path, mode);
        return;
    }
}

#[cfg(test)]
pub(crate) fn forget_root_for_tests(root: &Path) {
    REGISTRY
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .remove(root);
}

/// The installation that publishes under `root`, if one registered: the
/// publisher stamps it into every `repo.json`.
pub fn installation_for_root(root: &Path) -> Option<String> {
    REGISTRY
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(root)
        .map(|registered| registered.installation.clone())
}

/// The mode this process runs `root` in.
pub fn mode_for_root(root: &Path) -> Mode {
    REGISTRY
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(root)
        .map_or(Mode::Sql, |registered| registered.mode)
}

/// The publisher found the disk ahead of the database for this task (a
/// ledger file it would have written already holds other bytes, or
/// `task.json` is newer than any revision the database reached). In `disk`
/// mode nothing of the task is published until it is reconciled from the
/// disk. The fence is a row of the database (`disk_divergence`), so a
/// restart between the finding and the repair keeps it.
pub(crate) fn flag_divergence(db: &Db, task_id: &str, why: &str) {
    log::warn!("task {task_id}: the disk is ahead of the database ({why}); reconciling from disk");
    if let Err(error) = db.flag_disk_divergence(task_id, why) {
        log::error!("task {task_id}: could not record that its disk is ahead: {error}");
    }
    super::wake_publisher();
}

pub(crate) fn diverged_tasks(db: &Db) -> Vec<String> {
    let mut tasks = db.disk_divergent_task_ids().unwrap_or_else(|error| {
        log::error!("could not read the tasks whose disk is ahead: {error}");
        Vec::new()
    });
    // A failed disk-first commit this process could not record (T13d).
    for task in super::disk_first::fenced_tasks(&root_for_db(db.db_path())) {
        if !tasks.contains(&task) {
            tasks.push(task);
        }
    }
    tasks
}

/// Flagged, and not yet repaired: nothing of the task is published over
/// the disk, and its differing `task.json` is taken as the disk's. A task
/// whose flag cannot be read is treated as flagged.
pub(crate) fn is_diverged(db: &Db, task_id: &str) -> bool {
    super::disk_first::is_fenced(&root_for_db(db.db_path()), task_id)
        || db.is_disk_divergent(task_id).unwrap_or(true)
}

// ---------------------------------------------------------------------------
// Record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorityRecord {
    pub schema_version: u64,
    pub installation: String,
    /// The database path the installation id was derived from.
    pub database: String,
    pub mode: Mode,
    /// A switch asked for and not yet performed.
    pub requested: Option<Mode>,
    /// A switch in progress.
    pub switch: Option<SwitchProgress>,
    /// The most recent checkpoints, oldest first.
    pub checkpoints: Vec<Checkpoint>,
    /// When this installation first ran disk-first (T13d); cleared by a
    /// completed rollback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_first_since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SwitchProgress {
    pub target: Mode,
    /// The mode the switch started from.
    pub from: Mode,
    pub started_at: String,
    /// The last checkpoint the switch passed.
    pub phase: String,
    /// `to_disk`: each verified repository's tasks and the `task.json`
    /// revision they were verified at.
    #[serde(default)]
    pub verified_repos: BTreeMap<String, BTreeMap<String, i64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub checkpoint: String,
    pub at: String,
    /// The authoritative mode once the checkpoint was recorded.
    pub mode: Mode,
    #[serde(default)]
    pub detail: Value,
}

impl AuthorityRecord {
    fn new(db_path: &str) -> Self {
        Self {
            schema_version: RECORD_SCHEMA_VERSION,
            installation: installation_id(db_path),
            database: db_path.to_string(),
            mode: Mode::Sql,
            requested: None,
            switch: None,
            checkpoints: Vec::new(),
            disk_first_since: None,
            note: None,
        }
    }

    fn checkpoint(&mut self, at: String, name: &str, detail: Value) {
        if let Some(switch) = &mut self.switch {
            switch.phase = name.to_string();
        }
        self.checkpoints.push(Checkpoint {
            checkpoint: name.to_string(),
            at,
            mode: self.mode,
            detail,
        });
        let excess = self.checkpoints.len().saturating_sub(CHECKPOINT_HISTORY);
        self.checkpoints.drain(..excess);
    }
}

pub fn record_path(root: &Path, installation: &str) -> PathBuf {
    root.join("authority").join(format!("{installation}.json"))
}

/// The installation's record; `sql` with nothing requested when none was
/// ever written. An unreadable record is an error, never a guess: it may
/// say `disk`.
pub fn load_record(root: &Path, db_path: &str) -> Result<AuthorityRecord, String> {
    let path = record_path(root, &installation_id(db_path));
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AuthorityRecord::new(db_path))
        }
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let record: AuthorityRecord =
        serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    if ![RECORD_SCHEMA_VERSION, DISK_FIRST_RECORD_SCHEMA_VERSION].contains(&record.schema_version) {
        return Err(format!(
            "{} has schema_version {}; this build understands {RECORD_SCHEMA_VERSION} and {DISK_FIRST_RECORD_SCHEMA_VERSION}",
            path.display(),
            record.schema_version
        ));
    }
    if record.installation != installation_id(db_path) {
        return Err(format!(
            "{} names installation {}, not this database's",
            path.display(),
            record.installation
        ));
    }
    Ok(record)
}

/// Write the record. Its version follows its mode: a `disk` record is
/// [`DISK_FIRST_RECORD_SCHEMA_VERSION`], which older builds refuse.
fn save_record(root: &Path, record: &AuthorityRecord) -> Result<(), String> {
    let mut record = record.clone();
    if record.mode == Mode::Disk {
        record.schema_version = DISK_FIRST_RECORD_SCHEMA_VERSION;
        record.note = Some(DISK_FIRST_NOTE.to_string());
    } else {
        record.schema_version = RECORD_SCHEMA_VERSION;
        record.disk_first_since = None;
        record.note = None;
    }
    let mut bytes = serde_json::to_vec_pretty(&record)
        .map_err(|error| format!("render authority record: {error}"))?;
    bytes.push(b'\n');
    let path = record_path(root, &record.installation);
    let dir = path.parent().unwrap_or(root);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    replace_atomically(dir, &name, &bytes)
}

/// Ask for `mode` at the next startup. Writes only the record.
pub fn request(db_path: &str, mode: Mode) -> Result<AuthorityRecord, String> {
    let root = root_for_db(db_path);
    let mut record = load_record(&root, db_path)?;
    record.requested = (record.mode != mode || record.switch.is_some()).then_some(mode);
    save_record(&root, &record)?;
    Ok(record)
}

/// `kanna-server storage-authority [status|disk|sql]`.
pub fn run_cli(config: &crate::config::Config, args: &[String]) -> Result<String, String> {
    super::configure(config);
    let db_path = config.db_path.as_str();
    let root = root_for_db(db_path);
    let describe = |record: &AuthorityRecord| {
        let mut lines = vec![
            format!("installation: {}", record.installation),
            format!("database: {}", record.database),
            format!("task store: {}", root.display()),
            format!("mode: {}", record.mode.as_str()),
        ];
        if let Some(requested) = record.requested {
            lines.push(format!(
                "requested: {} (performed at the next server start)",
                requested.as_str()
            ));
        }
        if let Some(switch) = &record.switch {
            lines.push(format!(
                "switch in progress: {} -> {} at {}",
                switch.from.as_str(),
                switch.target.as_str(),
                switch.phase
            ));
        }
        if let Some(since) = &record.disk_first_since {
            lines.push(format!(
                "disk-first writes since {since}: builds older than T13d refuse this installation"
            ));
        }
        if let Some(last) = record.checkpoints.last() {
            lines.push(format!(
                "last checkpoint: {} at {}",
                last.checkpoint, last.at
            ));
        }
        lines.join("\n")
    };
    match args.first().map(String::as_str) {
        None | Some("status") => Ok(describe(&load_record(&root, db_path)?)),
        Some(value) => match Mode::parse(value) {
            Some(mode) => Ok(describe(&request(db_path, mode)?)),
            None => Err(format!(
                "usage: kanna-server storage-authority [status|disk|sql] (got {value:?})"
            )),
        },
    }
}

// ---------------------------------------------------------------------------
// Test crash points
// ---------------------------------------------------------------------------

#[cfg(test)]
static STOPS: LazyLock<Mutex<HashMap<PathBuf, String>>> = LazyLock::new(Default::default);

/// Stop the next [`start`] under `root` right after it records the first
/// checkpoint whose name starts with `checkpoint`, as if the process died
/// there. Fires once.
#[cfg(test)]
pub(crate) fn stop_after(root: &Path, checkpoint: &str) {
    STOPS
        .lock()
        .unwrap()
        .insert(root.to_path_buf(), checkpoint.to_string());
}

#[cfg(test)]
type Interleaved = Box<dyn FnOnce() + Send>;

#[cfg(test)]
static INTERLEAVED: LazyLock<Mutex<HashMap<PathBuf, Interleaved>>> =
    LazyLock::new(Default::default);

/// Run `write` once, in the next reconciliation under `root`, after its
/// comparison and before its transaction: a writer outside the caller's
/// lease committing in between.
#[cfg(test)]
pub(crate) fn interleave_before_reconcile(root: &Path, write: impl FnOnce() + Send + 'static) {
    INTERLEAVED
        .lock()
        .unwrap()
        .insert(root.to_path_buf(), Box::new(write));
}

#[cfg(test)]
fn before_reconcile_transaction(root: &Path) {
    let write = INTERLEAVED.lock().unwrap().remove(root);
    if let Some(write) = write {
        write();
    }
}

#[cfg(not(test))]
fn before_reconcile_transaction(_root: &Path) {}

/// What a stopped [`start`] returns.
#[cfg(test)]
pub const INTERRUPTED: &str = "interrupted after checkpoint";

#[cfg(test)]
fn stop_point(root: &Path, checkpoint: &str) -> Result<(), String> {
    let mut stops = STOPS.lock().unwrap();
    if stops
        .get(root)
        .is_some_and(|prefix| checkpoint.starts_with(prefix.as_str()))
    {
        stops.remove(root);
        return Err(format!("{INTERRUPTED} {checkpoint}"));
    }
    Ok(())
}

#[cfg(not(test))]
fn stop_point(_root: &Path, _checkpoint: &str) -> Result<(), String> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Comparing a task directory with the database
// ---------------------------------------------------------------------------

/// A task's directory against its rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Compared {
    /// `(revision, published_revision)`; `None` when the database does not
    /// hold the task.
    sql: Option<(i64, i64)>,
    disk_revision: i64,
    has_state: bool,
    /// `state` and `links.dependencies` equal the rows (quiet columns
    /// aside).
    state_equal: bool,
    /// On disk, not committed in the database.
    disk_only: Vec<i64>,
    /// Committed in both with other bytes.
    differing: Vec<i64>,
    /// Published by the database, absent from disk.
    published_missing: Vec<i64>,
    /// Committed and unpublished, absent from disk (the outbox).
    pending: Vec<i64>,
    /// Committed and unpublished, already on disk with the same bytes.
    pending_on_disk: Vec<i64>,
    reservations: Vec<i64>,
    /// The publisher flagged the task: its `task.json` was found ahead of
    /// the rows, and a live write may since have raised the database's
    /// revision to or past it, so revisions no longer tell which is newer.
    diverged: bool,
}

impl Compared {
    /// Disk and database agree exactly, and nothing is owed.
    fn exact(&self) -> bool {
        self.sql
            .is_some_and(|(revision, published)| revision == published)
            && self.has_state
            && self.state_equal
            && self.disk_only.is_empty()
            && self.differing.is_empty()
            && self.published_missing.is_empty()
            && self.pending.is_empty()
            && self.pending_on_disk.is_empty()
            && self.reservations.is_empty()
    }

    fn differences(&self) -> Vec<String> {
        let mut notes = Vec::new();
        match self.sql {
            None => notes.push("the database does not hold the task".to_string()),
            Some((revision, published)) if revision != published => notes.push(format!(
                "task.json is owed (revision {revision}, published {published})"
            )),
            Some(_) => {}
        }
        if !self.has_state {
            notes.push("task.json has no state".into());
        } else if !self.state_equal {
            notes.push("task.json state differs from the rows".into());
        }
        let mut list = |label: &str, sequences: &[i64]| {
            if !sequences.is_empty() {
                notes.push(format!("{label}: {sequences:?}"));
            }
        };
        list("ledger entries only on disk", &self.disk_only);
        list("ledger entries that differ", &self.differing);
        list(
            "published ledger entries missing on disk",
            &self.published_missing,
        );
        list("unpublished ledger entries", &self.pending);
        list("unacknowledged ledger entries", &self.pending_on_disk);
        list("ledger reservations", &self.reservations);
        notes
    }
}

/// Carried rows without their quiet columns, which move with unrelated
/// writes and owe no record.
fn comparable(tables: &Map<String, Value>) -> Map<String, Value> {
    tables
        .iter()
        .map(|(table, rows)| {
            let quiet = CARRIED_TABLES
                .iter()
                .find(|carried| carried.table == table)
                .map_or(&[][..], |carried| carried.quiet);
            let rows = rows
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .map(|row| {
                            let mut row = row.as_object().cloned().unwrap_or_default();
                            row.retain(|column, _| !quiet.contains(&column.as_str()));
                            Value::Object(row)
                        })
                        .collect()
                })
                .unwrap_or_default();
            (table.clone(), Value::Array(rows))
        })
        .collect()
}

fn compare_task(db: &Db, directory: &TaskDirectory) -> Result<Compared, rusqlite::Error> {
    let snapshot = &directory.snapshot;
    let task_id = snapshot.task_id.as_str();
    let mut compared = Compared {
        disk_revision: snapshot.snapshot_revision,
        has_state: snapshot.state.is_some(),
        ..Compared::default()
    };
    if db.get_pipeline_item(task_id)?.is_none() {
        compared.disk_only = directory
            .entries
            .iter()
            .map(|entry| entry.file.sequence)
            .collect();
        return Ok(compared);
    }
    compared.sql = Some(db.task_snapshot_revisions(task_id)?.unwrap_or((0, 0)));
    if let Some(disk_tables) = &snapshot.state {
        let sql_state = db.task_state_record(task_id)?;
        let sql_tables = sql_state
            .get("tables")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut disk_dependencies = snapshot.dependencies.clone();
        disk_dependencies.sort();
        disk_dependencies.dedup();
        compared.state_equal = comparable(disk_tables) == comparable(&sql_tables)
            && db.list_task_blocker_ids(task_id)? == disk_dependencies;
    }
    let disk: BTreeMap<i64, &[u8]> = directory
        .entries
        .iter()
        .map(|entry| (entry.file.sequence, entry.bytes.as_slice()))
        .collect();
    let mut committed = BTreeSet::new();
    for row in db.ledger_rows_for_authority(task_id)? {
        if row.kind.is_none() {
            compared.reservations.push(row.sequence);
            continue;
        }
        committed.insert(row.sequence);
        match disk.get(&row.sequence) {
            Some(bytes) if row.payload.as_deref() == Some(*bytes) => {
                if !row.published {
                    compared.pending_on_disk.push(row.sequence);
                }
            }
            Some(_) => compared.differing.push(row.sequence),
            None if row.published => compared.published_missing.push(row.sequence),
            None => compared.pending.push(row.sequence),
        }
    }
    compared.disk_only = disk
        .keys()
        .copied()
        .filter(|sequence| !committed.contains(sequence))
        .collect();
    Ok(compared)
}

/// What `disk` mode does about a task directory.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// Nothing for the disk to correct; the database may be ahead (its
    /// outbox), which the publisher writes out.
    InSync,
    /// Identical rows, but `task.json` carries a revision the database
    /// never reached (it was restored from an older copy): raise it.
    CountersBehind(i64),
    /// The disk holds what the database does not: reconcile from it.
    DiskAhead(Vec<String>),
    /// The database published what the disk no longer holds: publish again.
    DiskBehind(Vec<i64>),
    /// The disk is ahead, but its `task.json` predates `state` and cannot
    /// be projected exactly.
    Unreconcilable(String),
}

fn verdict(compared: &Compared) -> Verdict {
    let Some((revision, published)) = compared.sql else {
        return if compared.has_state {
            Verdict::DiskAhead(vec!["the database does not hold the task".into()])
        } else {
            Verdict::Unreconcilable("the database does not hold the task".into())
        };
    };
    let mut ahead = Vec::new();
    if !compared.disk_only.is_empty() {
        ahead.push(format!(
            "ledger entries {:?} are on disk, not in the database",
            compared.disk_only
        ));
    }
    if !compared.differing.is_empty() {
        ahead.push(format!(
            "ledger entries {:?} differ from the database's",
            compared.differing
        ));
    }
    // A revision the database never reached was written by a database
    // this one does not descend from; the same revision with other rows
    // was changed on disk. Anything else is the database being ahead.
    if compared.has_state && !compared.state_equal {
        if compared.diverged {
            ahead.push(
                "the publisher found this task.json ahead of the rows, and it still differs".into(),
            );
        } else if compared.disk_revision > revision {
            ahead.push(format!(
                "task.json revision {} is newer than the database's {revision}",
                compared.disk_revision
            ));
        } else if compared.disk_revision == revision && revision == published {
            ahead.push(format!(
                "task.json differs from the rows at the same revision {revision}"
            ));
        }
    }
    if !ahead.is_empty() {
        return if compared.has_state {
            Verdict::DiskAhead(ahead)
        } else {
            Verdict::Unreconcilable(format!("{}; task.json has no state", ahead.join("; ")))
        };
    }
    if !compared.published_missing.is_empty() || compared.disk_revision < published {
        return Verdict::DiskBehind(compared.published_missing.clone());
    }
    if compared.disk_revision > revision {
        return Verdict::CountersBehind(compared.disk_revision);
    }
    Verdict::InSync
}

// ---------------------------------------------------------------------------
// Reconciling the database from disk
// ---------------------------------------------------------------------------

/// What reconciling a database from its disk records did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Tasks whose rows were replaced from their directory, and why.
    pub reconciled: Vec<(String, Vec<String>)>,
    /// Tasks whose records the disk lost, owed again from the database.
    pub republished: Vec<String>,
    /// Tasks the disk records as removed, removed from the database.
    pub removed: Vec<String>,
    /// Repositories this installation's tombstones record as removed,
    /// removed from the database with their tasks (T13d).
    pub removed_repos: Vec<String>,
    /// Repository registrations taken from `repo.json`.
    pub repos: Vec<String>,
    /// Ledger entries the database committed that the disk contradicts or
    /// lacks, dropped: `(task, sequence)`.
    pub discarded_entries: Vec<(String, i64)>,
    /// Tasks that could not be reconciled, and why.
    pub failed: Vec<(String, String)>,
    /// Directories and records that could not be read.
    pub unreadable: Vec<(PathBuf, String)>,
    /// Repositories under the root that belong to another installation.
    pub foreign_repos: Vec<String>,
    /// Ledger files published from the in-flight entries of a `task.json`
    /// a disk-first commit wrote before its files (T13d).
    pub materialized: usize,
    pub diagnostics: Vec<String>,
}

impl ReconcileReport {
    fn log(&self) {
        for (task, reasons) in &self.reconciled {
            log::warn!("task {task} reconciled from disk: {}", reasons.join("; "));
        }
        for task in &self.republished {
            log::warn!("task {task}: disk lost published records; owed again from the database");
        }
        for task in &self.removed {
            log::warn!("task {task} is removed on disk; removed from the database");
        }
        for repo in &self.removed_repos {
            log::warn!("repo {repo} is removed on disk; removed from the database with its tasks");
        }
        for (task, sequence) in &self.discarded_entries {
            log::warn!("task {task}: ledger entry {sequence} the disk does not hold was dropped");
        }
        for (task, error) in &self.failed {
            log::error!("task {task} could not be reconciled from disk: {error}");
        }
        for (path, error) in &self.unreadable {
            log::error!("{} is unreadable: {error}", path.display());
        }
        for note in &self.diagnostics {
            log::info!("reconcile: {note}");
        }
    }
}

fn task_id_of_tombstone(path: &Path) -> Option<String> {
    let parent = path.parent()?.file_name()?.to_string_lossy().to_string();
    (parent == "tasks").then(|| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
    })?
}

/// Make the database agree with the disk (`disk` mode): every task the
/// disk is ahead for is reconciled from its directory, every record the
/// disk lost is owed again, and the outbox is drained. `only` limits it to
/// some tasks (the publisher's divergence); `None` is the whole
/// installation, repositories included.
pub fn reconcile_from_disk(
    db: &Db,
    db_path: &str,
    only: Option<&BTreeSet<String>>,
) -> Result<ReconcileReport, String> {
    let root = root_for_db(db_path);
    let installation = installation_id(db_path);
    let db_error = |error: rusqlite::Error| format!("db error: {error}");
    let (scan, foreign) = scan_store_records(&root)?.for_installation(&installation);
    let mut report = ReconcileReport {
        materialized: materialize_scan(&scan, only),
        unreadable: scan.unreadable.clone(),
        foreign_repos: foreign,
        ..ReconcileReport::default()
    };
    let wanted = |task: &str| only.is_none_or(|only| only.contains(task));
    // Removals the database committed and has not yet tombstoned on disk:
    // the outbox is the disk's journal, so they stand, and the publisher
    // writes their tombstones below.
    let removing: BTreeSet<(String, String)> = db
        .pending_disk_removals()
        .map_err(db_error)?
        .into_iter()
        .map(|(kind, id, _)| (kind, id))
        .collect();
    let removing_task = |task: &str| removing.contains(&("task".to_string(), task.to_string()));
    if only.is_none() {
        for record in &scan.repos {
            if removing.contains(&("repo".to_string(), record.repo_id.clone())) {
                continue;
            }
            if db.sync_repo_from_disk(record).map_err(db_error)? {
                report.repos.push(record.repo_id.clone());
            }
        }
        // A repo.json this installation did not stamp (an older build
        // rewrote it) is owed again, so the disk names its owner.
        for repo in db.sql_repo_ids().map_err(db_error)? {
            if scan.repos.iter().any(|record| record.repo_id == repo) {
                continue;
            }
            let on_disk = std::fs::read(super::repo_dir(&root, &repo).join("repo.json"));
            if let Some(bytes) = on_disk
                .ok()
                .filter(|bytes| super::rebuild::is_tombstone(bytes))
            {
                // This installation removed it and its database never
                // committed the removal (a disk-first commit that died
                // before SQLite's, T13d): apply the removal, as a rebuild
                // does, with the tasks it removed.
                let stamped = serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("installation")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    });
                if stamped.as_deref() == Some(installation.as_str()) {
                    db.delete_repo(&repo).map_err(db_error)?;
                    report.removed_repos.push(repo);
                    continue;
                }
                // An unstamped tombstone (written before T13d): never
                // resurrected by a rewrite, never removed with its tasks by
                // a reconciliation. An operator decides.
                report.diagnostics.push(format!(
                    "repo {repo} is registered in the database but tombstoned on disk; left as it is"
                ));
                continue;
            }
            db.owe_repo_disk_record(&repo).map_err(db_error)?;
        }
    }
    let sql_tasks = db.sql_tasks_for_authority().map_err(db_error)?;
    let mut targets = BTreeSet::new();
    let mut disk_revisions = BTreeMap::new();
    let mut compared_at = BTreeMap::new();
    let mut in_sync = BTreeSet::new();
    let mut reasons: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for directory in scan
        .tasks
        .iter()
        .filter(|dir| wanted(&dir.snapshot.task_id) && !removing_task(&dir.snapshot.task_id))
    {
        let task_id = directory.snapshot.task_id.clone();
        let mut compared = compare_task(db, directory).map_err(db_error)?;
        compared.diverged = is_diverged(db, &task_id);
        compared_at.insert(task_id.clone(), compared.sql.map(|(revision, _)| revision));
        match verdict(&compared) {
            Verdict::InSync => {
                in_sync.insert(task_id.clone());
            }
            Verdict::CountersBehind(disk_revision) => {
                // The rows are the disk's; only the revision was behind.
                db.raise_task_snapshot_revision(&task_id, disk_revision)
                    .map_err(db_error)?;
                in_sync.insert(task_id.clone());
            }
            Verdict::DiskAhead(why) => {
                targets.insert(task_id.clone());
                disk_revisions.insert(task_id.clone(), directory.snapshot.snapshot_revision);
                reasons.insert(task_id, why);
            }
            Verdict::DiskBehind(missing) => {
                db.reowe_disk_publication(&task_id, &missing)
                    .map_err(db_error)?;
                report.republished.push(task_id);
            }
            Verdict::Unreconcilable(why) => report.failed.push((task_id, why)),
        }
    }
    let on_disk: BTreeSet<&str> = scan
        .tasks
        .iter()
        .map(|dir| dir.snapshot.task_id.as_str())
        .collect();
    let mut tombstoned = BTreeSet::new();
    for path in &scan.removed {
        let Some(task_id) = task_id_of_tombstone(path) else {
            continue;
        };
        tombstoned.insert(task_id.clone());
        if wanted(&task_id) && sql_tasks.iter().any(|task| task.id == task_id) {
            db.delete_task_creation_artifacts(&task_id)
                .map_err(db_error)?;
            report.removed.push(task_id);
        }
    }
    if only.is_none() {
        // The database published a task.json the disk no longer has.
        let ours: BTreeSet<&str> = scan
            .repos
            .iter()
            .map(|repo| repo.repo_id.as_str())
            .collect();
        for task in &sql_tasks {
            if on_disk.contains(task.id.as_str())
                || tombstoned.contains(&task.id)
                || !ours.contains(task.repo_id.as_str())
            {
                continue;
            }
            let published = db
                .task_snapshot_revisions(&task.id)
                .map_err(db_error)?
                .is_some_and(|(_, published)| published > 0);
            if published {
                let missing: Vec<i64> = db
                    .ledger_rows_for_authority(&task.id)
                    .map_err(db_error)?
                    .into_iter()
                    .filter(|row| row.published)
                    .map(|row| row.sequence)
                    .collect();
                db.reowe_disk_publication(&task.id, &missing)
                    .map_err(db_error)?;
                report.republished.push(task.id.clone());
            }
        }
    }
    if !targets.is_empty() {
        let (runs, joins) = db.sql_run_and_join_ids().map_err(db_error)?;
        let known = KnownRows {
            tasks: sql_tasks.iter().map(|task| task.id.clone()).collect(),
            runs,
            joins,
        };
        let project = |tasks: &BTreeSet<String>| {
            let subset = StoreScan {
                tasks: scan
                    .tasks
                    .iter()
                    .filter(|dir| tasks.contains(&dir.snapshot.task_id))
                    .cloned()
                    .collect(),
                repos: scan.repos.clone(),
                ..StoreScan::default()
            };
            project_store_onto(&subset, &known)
        };
        let projection = project(&targets);
        report.diagnostics.extend(projection.diagnostics.clone());
        before_reconcile_transaction(&root);
        let note_changes = |report: &mut ReconcileReport, changes: ReconcileChanges| {
            report.discarded_entries.extend(changes.discarded_entries);
            for (task, table, rowid) in changes.relocated_rows {
                report.diagnostics.push(format!(
                    "{task}: its {table} row {rowid} is recovered under a new rowid: another task's row holds {rowid}"
                ));
            }
            for (task, from, to) in changes.renumbered_inputs {
                report.diagnostics.push(format!(
                    "{task}: input {from} is recovered as input {to}: another task's input holds {from}"
                ));
            }
        };
        match super::disk_first::repairing(&targets, || {
            db.reconcile_tasks_from_projection(&targets, &projection, &disk_revisions, &compared_at)
        }) {
            Ok(changes) => {
                note_changes(&mut report, changes);
                for task in &targets {
                    report
                        .reconciled
                        .push((task.clone(), reasons.remove(task).unwrap_or_default()));
                }
            }
            // One task's rows may be what refuses the rest: try each alone.
            Err(together) => {
                report.diagnostics.push(format!(
                    "reconciling {} tasks together failed ({together}); reconciling each alone",
                    targets.len()
                ));
                for task in &targets {
                    let alone = BTreeSet::from([task.clone()]);
                    match super::disk_first::repairing(&alone, || {
                        db.reconcile_tasks_from_projection(
                            &alone,
                            &project(&alone),
                            &disk_revisions,
                            &compared_at,
                        )
                    }) {
                        Ok(changes) => {
                            note_changes(&mut report, changes);
                            report
                                .reconciled
                                .push((task.clone(), reasons.remove(task).unwrap_or_default()));
                        }
                        Err(error) => report.failed.push((task.clone(), db_error(error))),
                    }
                }
            }
        }
    }
    // A reconciled task's fence came down with its repair. A flagged task
    // found in sync needs nothing more; one that failed stays flagged, and
    // is retried at the publisher's next pass (or the next startup).
    for task in &in_sync {
        db.clear_disk_divergence(task).map_err(db_error)?;
        super::disk_first::unfence(&root, task);
    }
    for (task, _) in &report.reconciled {
        super::disk_first::unfence(&root, task);
    }
    for (task, error) in flush_all(db, db_path) {
        report.diagnostics.push(format!(
            "{task} is pending publication after reconciling: {error}"
        ));
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// Switching
// ---------------------------------------------------------------------------

/// Whether everything the disk must hold equals the database.
#[derive(Debug, Default)]
struct Verification {
    /// Per repository: its tasks verified exact (with the `task.json`
    /// revision each was verified at), and its problems.
    repos: BTreeMap<String, (BTreeMap<String, i64>, Vec<String>)>,
    /// Problems no repository of the database accounts for.
    unaccounted: Vec<String>,
}

impl Verification {
    fn problems(&self) -> Vec<String> {
        self.repos
            .values()
            .flat_map(|(_, problems)| problems.iter().cloned())
            .chain(self.unaccounted.iter().cloned())
            .collect()
    }
}

fn verify_disk_equals_database(db: &Db, db_path: &str) -> Result<Verification, String> {
    let root = root_for_db(db_path);
    let db_error = |error: rusqlite::Error| format!("db error: {error}");
    let (scan, _) = scan_store_records(&root)?.for_installation(&installation_id(db_path));
    let mut verification = Verification::default();
    for repo in db.sql_repo_ids().map_err(db_error)? {
        let (_, problems) = verification.repos.entry(repo.clone()).or_default();
        let Some(record) = scan.repos.iter().find(|record| record.repo_id == repo) else {
            problems.push(format!(
                "repo {repo}: no readable repo.json naming this installation"
            ));
            continue;
        };
        let live = db
            .repo_disk_record(&repo)
            .map_err(db_error)?
            .unwrap_or(Value::Null);
        let quiet = |registration: &Map<String, Value>| {
            let mut registration = registration.clone();
            registration.remove("last_opened_at");
            registration
        };
        let live_registration = live
            .get("registration")
            .and_then(Value::as_object)
            .map(quiet)
            .unwrap_or_default();
        if quiet(&record.registration) != live_registration
            || live.get("sidebar_order").and_then(Value::as_i64) != record.sidebar_order
        {
            problems.push(format!(
                "repo {repo}: repo.json differs from the registration"
            ));
        }
    }
    let directories: BTreeMap<&str, &TaskDirectory> = scan
        .tasks
        .iter()
        .map(|dir| (dir.snapshot.task_id.as_str(), dir))
        .collect();
    let sql_tasks = db.sql_tasks_for_authority().map_err(db_error)?;
    let unreadable = |task: &str| {
        scan.unreadable
            .iter()
            .find(|(path, _)| path.file_name().is_some_and(|name| name == task))
            .map(|(_, error)| error.clone())
    };
    for task in &sql_tasks {
        let (verified, problems) = verification.repos.entry(task.repo_id.clone()).or_default();
        let Some(directory) = directories.get(task.id.as_str()) else {
            problems.push(match unreadable(&task.id) {
                Some(error) => format!("task {}: its directory is unreadable: {error}", task.id),
                None => format!("task {}: no task directory", task.id),
            });
            continue;
        };
        let compared = compare_task(db, directory).map_err(db_error)?;
        if compared.exact() {
            verified.insert(task.id.clone(), compared.disk_revision);
        } else {
            problems.push(format!(
                "task {}: {}",
                task.id,
                compared.differences().join("; ")
            ));
        }
    }
    // A directory the database does not hold would be rebuilt into a task
    // it never had. One with no readable task.json is never rebuilt, so it
    // is only reported.
    for (id, directory) in &directories {
        if !sql_tasks.iter().any(|task| task.id == *id) {
            verification.unaccounted.push(format!(
                "task directory {id} (repo {}) is not a task of this database",
                directory.snapshot.repo_id
            ));
        }
    }
    for (path, error) in &scan.unreadable {
        let known = path
            .file_name()
            .is_some_and(|name| sql_tasks.iter().any(|task| name == task.id.as_str()));
        if !known {
            log::warn!(
                "{} is unreadable and not a task of this database; a rebuild skips it: {error}",
                path.display()
            );
        }
    }
    for path in &scan.removed {
        if let Some(id) = task_id_of_tombstone(path) {
            if let Some(task) = sql_tasks.iter().find(|task| task.id == id) {
                verification
                    .repos
                    .entry(task.repo_id.clone())
                    .or_default()
                    .1
                    .push(format!(
                        "task {id}: held by the database but tombstoned on disk"
                    ));
            }
        }
    }
    Ok(verification)
}

/// Idempotent: releases what no operation can own at startup, imports
/// pre-ledger history, owes every record, and publishes.
fn drain(db: &Db, db_path: &str) -> Result<Value, String> {
    let db_error = |error: rusqlite::Error| format!("db error: {error}");
    let released = db.release_stale_ledger_reservations().map_err(db_error)?;
    for task_id in db.tasks_needing_ledger_backfill().map_err(db_error)? {
        db.backfill_task_ledger(&task_id).map_err(db_error)?;
    }
    let owed = db.owe_every_disk_record().map_err(db_error)?;
    let failures = flush_all(db, db_path);
    if !failures.is_empty() {
        return Err(failures
            .iter()
            .map(|(task, error)| format!("{task}: {error}"))
            .collect::<Vec<_>>()
            .join("; "));
    }
    let pending = db.ledger_tasks_with_pending_work().map_err(db_error)?;
    let repos = db.repos_with_pending_disk_record().map_err(db_error)?;
    let removals = db.pending_disk_removals().map_err(db_error)?;
    if !pending.is_empty() || !repos.is_empty() || !removals.is_empty() {
        return Err(format!(
            "still owed after publishing: tasks {pending:?}, repositories {repos:?}, removals {}",
            removals.len()
        ));
    }
    Ok(json!({ "released_reservations": released, "first_records_owed": owed }))
}

/// Publish every in-flight entry file the scanned `task.json` records hold
/// and their ledgers lack (T13d), for the tasks being reconciled (`only`, or
/// all). A task whose files cannot be written is logged; its entries are
/// still read from `task.json`.
fn materialize_scan(scan: &StoreScan, only: Option<&BTreeSet<String>>) -> usize {
    scan.tasks
        .iter()
        .filter(|directory| only.is_none_or(|only| only.contains(&directory.snapshot.task_id)))
        .map(|directory| {
            super::disk_first::materialize_in_flight(directory).unwrap_or_else(|error| {
                log::error!(
                    "{}: in-flight ledger entries could not be published: {error}",
                    directory.path.display()
                );
                0
            })
        })
        .sum()
}

/// What [`start`] left the installation in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartOutcome {
    pub mode: Mode,
    /// `disk` mode's reconciliation at this startup.
    pub reconcile: Option<ReconcileReport>,
    /// Why a requested switch (to `disk`, or a rollback to `sql`) was
    /// refused, if it was.
    pub refused: Vec<String>,
}

struct Switch<'a> {
    db: &'a Db,
    db_path: &'a str,
    root: PathBuf,
    record: AuthorityRecord,
    /// Why a rollback was refused at this start.
    refused: Vec<String>,
}

impl Switch<'_> {
    fn now(&self) -> Result<String, String> {
        self.db
            .current_utc_timestamp()
            .map_err(|error| format!("db error: {error}"))
    }

    /// Record a checkpoint durably, then (in tests) stop if asked to.
    fn checkpoint(&mut self, name: &str, detail: Value) -> Result<(), String> {
        let at = self.now()?;
        if self.record.mode == Mode::Disk && self.record.disk_first_since.is_none() {
            self.record.disk_first_since = Some(at.clone());
        }
        self.record.checkpoint(at, name, detail);
        save_record(&self.root, &self.record)?;
        log::info!(
            "storage authority checkpoint {name} (authoritative: {})",
            self.record.mode.as_str()
        );
        stop_point(&self.root, name)
    }

    fn begin(&mut self, target: Mode) -> Result<(), String> {
        let started_at = self.now()?;
        self.record.switch = Some(SwitchProgress {
            target,
            from: self.record.mode,
            started_at,
            phase: String::new(),
            verified_repos: BTreeMap::new(),
        });
        let name = match target {
            Mode::Disk => "to_disk.begin",
            Mode::Sql => "to_sql.begin",
        };
        self.checkpoint(name, json!({ "from": self.record.mode }))
    }

    fn switch_to_disk(&mut self) -> Result<Vec<String>, String> {
        if self.record.switch.is_none() {
            self.begin(Mode::Disk)?;
        }
        let drained = match drain(self.db, self.db_path) {
            Ok(drained) => drained,
            Err(error) => return self.refuse(vec![format!("the outbox did not drain: {error}")]),
        };
        self.checkpoint("to_disk.drained", drained)?;
        let verification = verify_disk_equals_database(self.db, self.db_path)?;
        let before = self
            .record
            .switch
            .as_ref()
            .map(|switch| switch.verified_repos.clone())
            .unwrap_or_default();
        for (repo, (tasks, problems)) in &verification.repos {
            if !problems.is_empty() {
                continue;
            }
            // A task verified at an earlier attempt and changed since (an
            // older build ran in between) was verified again just now.
            let changed: Vec<&String> = before
                .get(repo)
                .map(|earlier| {
                    tasks
                        .iter()
                        .filter(|(task, revision)| {
                            earlier
                                .get(*task)
                                .is_some_and(|earlier| earlier != *revision)
                        })
                        .map(|(task, _)| task)
                        .collect()
                })
                .unwrap_or_default();
            if let Some(switch) = &mut self.record.switch {
                switch.verified_repos.insert(repo.clone(), tasks.clone());
            }
            self.checkpoint(
                "to_disk.repo_verified",
                json!({ "repo": repo, "tasks": tasks.len(), "changed_since_last_attempt": changed }),
            )?;
        }
        let problems = verification.problems();
        if !problems.is_empty() {
            return self.refuse(problems);
        }
        let verified: BTreeMap<&String, usize> = verification
            .repos
            .iter()
            .map(|(repo, (tasks, _))| (repo, tasks.len()))
            .collect();
        let tasks: usize = verified.values().sum();
        self.checkpoint(
            "to_disk.verified",
            json!({ "repos": verified.len(), "tasks": tasks }),
        )?;
        self.record.mode = Mode::Disk;
        self.record.switch = None;
        if self.record.requested == Some(Mode::Disk) {
            self.record.requested = None;
        }
        self.checkpoint(
            "to_disk.commit",
            json!({ "repos": verified.len(), "tasks": tasks }),
        )?;
        Ok(Vec::new())
    }

    /// The switch to `disk` cannot be made safely: stay `sql`. The request
    /// stays, so the next startup tries again once the cause is fixed.
    fn refuse(&mut self, problems: Vec<String>) -> Result<Vec<String>, String> {
        for problem in &problems {
            log::error!("switch to disk authority refused: {problem}");
        }
        self.record.switch = None;
        self.checkpoint(
            "to_disk.refused",
            json!({ "problems": problems.iter().take(50).collect::<Vec<_>>(),
                    "count": problems.len() }),
        )?;
        Ok(problems)
    }

    /// The database cannot be shown to hold what the disk does: stay
    /// `disk`. The request stays, so the next startup verifies again.
    fn refuse_rollback(&mut self, problems: Vec<String>) -> Result<(), String> {
        for problem in &problems {
            log::error!("rollback to sql authority refused: {problem}");
        }
        self.record.switch = None;
        self.refused = problems.clone();
        self.checkpoint(
            "to_sql.refused",
            json!({ "problems": problems.iter().take(50).collect::<Vec<_>>(),
                    "count": problems.len() }),
        )
    }

    fn roll_back_to_sql(&mut self) -> Result<Option<ReconcileReport>, String> {
        if self
            .record
            .switch
            .as_ref()
            .is_none_or(|switch| switch.target != Mode::Sql)
        {
            self.begin(Mode::Sql)?;
        }
        let mut report = None;
        if self.record.mode == Mode::Disk {
            // Disk-first writes (T13d) mean SQLite is no longer written
            // first, so a rollback is made safe by what it checks, not by
            // the write order: the database, reconciled from disk, must
            // hold exactly what the disk does, or the rollback is refused.
            let reconciled = reconcile_from_disk(self.db, self.db_path, None)?;
            reconciled.log();
            let mut problems: Vec<String> = reconciled
                .failed
                .iter()
                .map(|(task, error)| {
                    format!("task {task} could not be reconciled from disk: {error}")
                })
                .collect();
            let pending = self
                .db
                .ledger_tasks_with_pending_work()
                .map_err(|error| format!("db error: {error}"))?;
            if !pending.is_empty() {
                problems.push(format!("tasks {pending:?} are still owed publication"));
            }
            if problems.is_empty() {
                problems = verify_disk_equals_database(self.db, self.db_path)?.problems();
            }
            if !problems.is_empty() {
                return self.refuse_rollback(problems).map(|()| Some(reconciled));
            }
            self.checkpoint(
                "to_sql.reconciled",
                json!({ "reconciled": reconciled.reconciled.len(),
                        "republished": reconciled.republished.len(),
                        "verified": true }),
            )?;
            report = Some(reconciled);
        }
        self.record.mode = Mode::Sql;
        self.record.switch = None;
        if self.record.requested == Some(Mode::Sql) {
            self.record.requested = None;
        }
        self.checkpoint("to_sql.commit", json!({}))?;
        Ok(report)
    }
}

/// Startup: perform or resume a requested switch, then, in `disk` mode,
/// reconcile the database from the disk. Runs after migrations and before
/// any service starts. `request` (from [`AUTHORITY_ENV`]) is persisted as
/// the record's request first.
pub fn start(db: &Db, db_path: &str, request: Option<Mode>) -> Result<StartOutcome, String> {
    let root = root_for_db(db_path);
    register(&root, db_path);
    let mut switch = Switch {
        db,
        db_path,
        root: root.clone(),
        record: load_record(&root, db_path)?,
        refused: Vec::new(),
    };
    // The persisted mode governs from here, before any step that can fail:
    // a `disk` record is never served SQL-first.
    set_mode(&root, db_path, switch.record.mode);
    // An installation switched to disk by a build before T13d: its record
    // closes to older builds before this one writes anything disk-first.
    // `open_database` refuses to serve it if that cannot be persisted.
    if switch.record.mode == Mode::Disk && switch.record.disk_first_since.is_none() {
        switch.record.disk_first_since = Some(switch.now()?);
        save_record(&root, &switch.record)
            .map_err(|error| format!("{FENCE_NOT_PERSISTED}: {error}"))?;
    }
    if let Some(request) = request {
        // The explicit request replaces whatever was pending: one that asks
        // for the mode already in force (and no switch under way) asks for
        // nothing, and withdraws a pending request the other way.
        let wanted =
            (switch.record.mode != request || switch.record.switch.is_some()).then_some(request);
        if switch.record.requested != wanted {
            switch.record.requested = wanted;
            save_record(&root, &switch.record)?;
        }
    }
    let mut outcome = StartOutcome {
        mode: switch.record.mode,
        reconcile: None,
        refused: Vec::new(),
    };
    let in_progress = switch
        .record
        .switch
        .as_ref()
        .map(|progress| progress.target);
    let requested = switch.record.requested;
    let result = match (in_progress, requested) {
        // Rolling back (or asked to, mid-switch): finish the rollback.
        (Some(Mode::Sql), _) | (Some(Mode::Disk), Some(Mode::Sql)) => switch
            .roll_back_to_sql()
            .map(|report| outcome.reconcile = report),
        (Some(Mode::Disk), _) => switch
            .switch_to_disk()
            .map(|refused| outcome.refused = refused),
        (None, Some(Mode::Disk)) if switch.record.mode == Mode::Sql => switch
            .switch_to_disk()
            .map(|refused| outcome.refused = refused),
        (None, Some(Mode::Sql)) if switch.record.mode == Mode::Disk => switch
            .roll_back_to_sql()
            .map(|report| outcome.reconcile = report),
        (None, Some(_)) => {
            // Already in the requested mode.
            switch.record.requested = None;
            save_record(&root, &switch.record)
        }
        (None, None) => Ok(()),
    };
    // What the record now says, not what the switch meant to write: a
    // checkpoint that failed to save leaves the persisted mode in force. A
    // record that cannot be read back is taken as `disk`.
    let persisted = load_record(&root, db_path).map_or(Mode::Disk, |record| record.mode);
    set_mode(&root, db_path, persisted);
    outcome.mode = persisted;
    if !switch.refused.is_empty() {
        outcome.refused = std::mem::take(&mut switch.refused);
    }
    result?;
    if outcome.mode == Mode::Disk {
        let report = reconcile_from_disk(db, db_path, None)?;
        report.log();
        outcome.reconcile = Some(report);
    }
    Ok(outcome)
}

/// A `disk` record whose schema-version fence could not be written: older
/// builds could still open the installation, so it is not served.
const FENCE_NOT_PERSISTED: &str =
    "the disk-first fence could not be written to the authority record";

/// A switch requested through [`AUTHORITY_ENV`], if any.
pub fn requested_from_env() -> Result<Option<Mode>, String> {
    match std::env::var(AUTHORITY_ENV) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => Mode::parse(&value)
            .map(Some)
            .ok_or_else(|| format!("{AUTHORITY_ENV} must be sql or disk, not {value:?}")),
        Err(_) => Ok(None),
    }
}

/// Rebuild a missing database from this installation's disk records.
pub fn rebuild_missing_database(db_path: &str) -> Result<RebuildReport, String> {
    let root = root_for_db(db_path);
    let (scan, foreign) = scan_store_records(&root)?.for_installation(&installation_id(db_path));
    let materialized = materialize_scan(&scan, None);
    if materialized > 0 {
        log::warn!("published {materialized} in-flight ledger entries before rebuilding");
    }
    // A WAL or shared-memory file left beside a deleted database belongs to
    // it, not to the one about to be created.
    for suffix in ["-wal", "-shm"] {
        let stale = PathBuf::from(format!("{db_path}{suffix}"));
        if stale.exists() {
            log::warn!("removing {} left by the missing database", stale.display());
            std::fs::remove_file(&stale)
                .map_err(|error| format!("remove {}: {error}", stale.display()))?;
        }
    }
    let mut report = rebuild_scan_into_new_database(scan, Path::new(db_path))?;
    if !foreign.is_empty() {
        report.diagnostics.push(format!(
            "{} repositories under {} belong to another installation and were left out",
            foreign.len(),
            root.display()
        ));
    }
    Ok(report)
}

/// Open the server's database under its storage authority: rebuild it from
/// disk first when it is missing in `disk` mode, migrate it, then [`start`].
pub fn open_database(config: &crate::config::Config) -> Result<Db, String> {
    super::configure(config);
    let db_path = config.db_path.as_str();
    let root = root_for_db(db_path);
    register(&root, db_path);
    let record = load_record(&root, db_path)?;
    let request = requested_from_env()?;
    if record.mode == Mode::Disk && !Path::new(db_path).exists() {
        log::warn!(
            "database {db_path} is missing and the disk is authoritative: rebuilding it from {}",
            root.display()
        );
        let report = rebuild_missing_database(db_path)?;
        log::warn!(
            "rebuilt {} tasks ({} ledger entries, {} carried rows) from disk; {} unreadable",
            report.tasks,
            report.ledger_entries,
            report.carried_rows,
            report.unreadable.len()
        );
        for (path, error) in &report.unreadable {
            log::error!("{} could not be rebuilt: {error}", path.display());
        }
        for note in &report.diagnostics {
            log::info!("rebuild: {note}");
        }
    }
    let db = Db::open_migrated(db_path).map_err(|error| format!("open {db_path}: {error}"))?;
    match start(&db, db_path, request) {
        Ok(outcome) => log::info!(
            "storage authority: {}{}",
            outcome.mode.as_str(),
            if outcome.refused.is_empty() {
                String::new()
            } else {
                format!(
                    " (requested switch refused: {} problems; see the log and `kanna-server storage-authority status`)",
                    outcome.refused.len()
                )
            }
        ),
        // A `disk` record whose fence against older builds is not on disk
        // is not served at all.
        Err(error) if error.starts_with(FENCE_NOT_PERSISTED) => {
            return Err(format!(
                "storage authority is disk but {error}; refusing to start"
            ))
        }
        // Otherwise the server runs in the mode the record holds, which
        // `start` set before anything could fail (a `disk` record is always
        // served disk-first); the next startup retries the rest.
        Err(error) => log::error!(
            "storage authority startup did not complete ({error}); running as {}",
            mode_for_root(&root).as_str()
        ),
    }
    Ok(db)
}

#[cfg(test)]
#[path = "authority_tests.rs"]
mod tests;
