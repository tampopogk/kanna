//! Task directory and ledger (spec §7, §16.1 — component T0).
//!
//! Each task has a directory `<root>/repos/<repo-id>/tasks/<task-id>/` holding
//! a replaceable `task.json` and an ordered, append-only `ledger/`. In the
//! default `sql` authority mode SQLite stays authoritative: mutations enqueue
//! immutable ledger payloads in their own SQL transaction (see
//! [`crate::db::task_store`]) and this module publishes them to disk, in
//! order, atomically, and acknowledges them. In `disk` mode ([`authority`])
//! the same records are written by the mutation's own transaction before it
//! commits ([`disk_first`]). The ledger is internal message passing between stages; the real
//! outputs of a task are still commits, PRs and artifacts.
//!
//! # Disk contract (schema_version 1)
//!
//! Later components extend this envelope; they do not redefine it. T1 adds
//! exit routing, T2 session types, T6 artifact descriptors, T8 channel
//! identity.
//!
//! **Root.** `~/.kanna` for production and staging installations,
//! `<db-path>.task-store` for development and test databases (so separate
//! databases never share ledger identities), or `KANNA_TASK_STORE_ROOT`.
//!
//! **`task.json`** — in `sql` mode a projection of the rows; in `disk` mode
//! the commit point of every mutation (T13d). Rewritten atomically
//! (temp file + rename) whenever what it shows changes: `schema_version`,
//! `task_id`, `repo_id`, `title`, `origin_prompt`, `workflow {name,
//! definition}` (the exact pinned definition), `links {parent, dependencies,
//! stage_dependencies, subtask_joins, pr {url, number, head_sha}}`
//! (`dependencies` lists legacy task-level blockers; `stage_dependencies`
//! the T4 stage edges into the task, in edge order, with the result each
//! consumed and any newer upstream result that superseded it;
//! `subtask_joins` the T5 joins the task created and each member's
//! outcome), `stage`, `branch`, `base_ref`,
//! `owning_machine` (null until recorded per task), `created_at`,
//! `updated_at`, `closed_at`, `snapshot_revision`,
//! `ledger.published_through` (the highest published sequence), since T13d
//! in `disk` mode `ledger.in_flight` (`[{sequence, file_name, payload}]`,
//! the exact bytes of every entry committed and not yet a file when the
//! record was written; readers treat them as ledger entries) and, since
//! T13, `state {version, reflects_through, unreflected_reservations,
//! tables}`: the task's rows of every table that holds durable task state,
//! verbatim with their rowids ([`crate::db::task_state`]), and the ledger
//! boundary those rows reflect (the highest committed sequence, pending
//! included, read with them; reservations below it not yet filled). A carried row's change owes a new
//! `task.json` in the changing statement (a trigger bumps the snapshot
//! revision). A task removed from the database gets a tombstone
//! `{schema_version, task_id, repo_id, removed: true}` in place of its
//! `task.json`; its ledger stays.
//!
//! **`repos/<repo-id>/repo.json`** (T13) — the repository's registration and
//! sidebar order, rewritten the same way; a tombstone once unregistered.
//! Since T13c it names the `installation` that wrote it, so installations
//! sharing a root (production and staging) never take in each other's
//! tasks. Which side is authoritative, SQLite or these records, is the
//! installation's storage authority mode ([`authority`]).
//!
//! **`ledger/NNNNNN-<kind>.<ext>`** — immutable. `NNNNNN` is the per-task
//! sequence, zero-padded to six digits (order by the number, not the text,
//! should a task ever pass 999999 entries). Sequences are a strict
//! high-water mark (T13, `task_ledger_sequence`): a number is never handed
//! out twice, so a released or abandoned reservation leaves a permanent gap,
//! which readers treat as nothing. `result` and `input` entries are
//! Markdown: a `---` line, the pretty-printed JSON envelope, a `---` line, an
//! empty line, then the message verbatim to end of file. `transition` and
//! `plan` entries are the JSON envelope alone. A published file is never
//! rewritten; a retry that would write different bytes is refused.
//!
//! Every envelope carries:
//!
//! | field | meaning |
//! |---|---|
//! | `schema_version` | `1` |
//! | `entry_id` | `<task-id>-<NNNNNN>`; stable across retries |
//! | `task_id`, `sequence`, `kind` | identity and order |
//! | `operation_id` | groups the entries of one accepted mutation (a plan result and the workflow it published share one) |
//! | `source {kind, id, origin}` | the original record this entry mirrors (`stage_run`/run id, `task_input`/row id, `task_event`/seq, ...); `origin` is provenance an import carried from another machine |
//! | `recorded_at` | ISO-8601 UTC |
//! | `historical` | `true` for backfilled history |
//! | `run_id`, `session_ref` | the stage run, and the session reference `{kind: "stage_run", id: <run id>}`; a run that recorded a session identity (T2) adds `workspace_id`, `branch`, `name` and `transcript {provider, session_id, path}` beside those keys |
//! | `declared_role` | a role the caller declared (`operator`, `manager`, `agent`), else null |
//! | `channel_identity` | T8's verified channel ([`crate::mutation_provenance::ChannelIdentity`]) the mutation arrived on; historical/backfilled entries always carry the explicit tagged `unknown`, never a value inferred from a source label or transport |
//! | `artifacts` | T6's named artifact references a result carried: name → tagged reference (`{"type": "stored", "repoId", "artifactId", "kind"}`, `{"type": "commit", "repoId", "sha"}` or `{"type": "pr", "url", "headSha"}`). Stored references resolved in the artifact repository when the result was accepted. `{}` for every other entry |
//!
//! and one kind-specific object named after the kind:
//!
//! - `result`: `result_id` (= `entry_id`), `status` (one of the six verdicts),
//!   `stage`, `run_kind`, `branch` and `committed_sha` observed by the engine
//!   in the run's workspace when the result was accepted (null when the run
//!   had no workspace, or for history), `provenance {branch, committed_sha}`
//!   saying how each was obtained (`workspace`, `unborn`, `no_workspace`,
//!   `not_a_checkout`, or `unknown` for history),
//!   `timestamp`, the caller's legacy `metadata` (kept, never trusted as
//!   engine evidence), `request {kind, ...}` naming the call that recorded it,
//!   and `legacy_format` for results that predate `{status, summary}`. The
//!   message is the Markdown body. A revision request's `request` also
//!   carries its `summary` and `findings` apart (T13). An **engine-observed
//!   ending** (T13, source kind `stage_run_ending`) records a run that closed
//!   without a verdict: `status` null, `observed_by: "engine"`, `ending
//!   {run_status, no_work_termination}`; it is never read as a verdict.
//! - `input`: `input_id`, `source` (the stored label), `stage`,
//!   `delivered_at`. The delivered text is the Markdown body.
//! - `transition`: `from_stage`, `to_stage`, `branch`, `trigger`,
//!   `operation`, `triggering_result_id` (the newest result recorded since
//!   the previous transition, or null), and `exit` / `exit_source`, reserved
//!   for T1 and null until the engine routes by exit. T4 adds
//!   `dependencies` (present only when non-empty): the stage-edge inputs the
//!   entered stage consumed, in edge order, each `{upstream_task_id,
//!   upstream_stage, dependent_stage, position, role, result_id,
//!   committed_sha}` with `role` `base` (fork point), `merge` (handed to the
//!   session, never merged by the engine) or `gate`. A dependent entering
//!   the stage it starts in once its base edges are satisfied records a
//!   transition with `operation: "dependency_start"` and a null
//!   `from_stage`.
//! - `plan`: `operation` (`select` | `replace`), `source`, `stage`,
//!   `from_workflow`, `to_workflow`, `before`, `after` (pinned definitions),
//!   `superseded_run_ids`, `changed_execution_stages`, and `result_id` of the
//!   result it was published with, if any.
//!
//! # Delivery to sessions
//!
//! Every spawned session gets `KANNA_TASK_LEDGER_PATH` (the task directory)
//! and, in its environment preamble, the result that caused it: the newest
//! result recorded since the task's latest transition, or, for a new session
//! of the stage that transition entered, that transition's
//! `triggering_result_id`. It is read from published files, so a session is
//! only ever told about an entry it can open.

use crate::db::task_store::{LedgerEntryKind, PendingLedgerEntry};
use crate::db::Db;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::Notify;

pub mod authority;
pub(crate) mod disk_first;
// Disk authority (T13c) rebuilds a missing database and reconciles from
// disk; the rest of the rebuild (the unscoped entry points, the list of what
// a rebuild lacks) is read by tests and people.
#[cfg_attr(not(test), allow(dead_code))]
pub mod rebuild;
#[cfg(test)]
mod rebuild_tests;
#[cfg(test)]
mod tests;

pub const SCHEMA_VERSION: u32 = 1;

/// Environment variable naming the task directory in every agent session.
pub const LEDGER_PATH_ENV: &str = "KANNA_TASK_LEDGER_PATH";

const ROOT_OVERRIDE_ENV: &str = "KANNA_TASK_STORE_ROOT";

static ROOTS: LazyLock<Mutex<HashMap<String, PathBuf>>> = LazyLock::new(Default::default);

/// Woken whenever an entry is enqueued, so the publisher service flushes
/// promptly. The outbox, not this signal, is what makes publication durable.
static PUBLISHER: LazyLock<Notify> = LazyLock::new(Notify::new);

pub(crate) fn wake_publisher() {
    PUBLISHER.notify_one();
}

pub(crate) async fn publisher_woken() {
    PUBLISHER.notified().await;
}

/// The store root a server configuration uses.
pub fn root_for_config(config: &crate::config::Config) -> PathBuf {
    if !cfg!(test) {
        if let Some(root) = std::env::var_os(ROOT_OVERRIDE_ENV).filter(|root| !root.is_empty()) {
            return PathBuf::from(root);
        }
        if matches!(config.environment.as_str(), "production" | "staging") {
            if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
                return PathBuf::from(home).join(".kanna");
            }
        }
    }
    default_root_for_db(&config.db_path)
}

fn default_root_for_db(db_path: &str) -> PathBuf {
    PathBuf::from(format!("{db_path}.task-store"))
}

/// Register the root for a configuration's database, so code that only
/// holds a database path publishes to the same place.
pub fn configure(config: &crate::config::Config) {
    let root = root_for_config(config);
    authority::register(&root, &config.db_path);
    if let Ok(mut roots) = ROOTS.lock() {
        roots.insert(config.db_path.clone(), root);
    }
}

pub fn root_for_db(db_path: &str) -> PathBuf {
    ROOTS
        .lock()
        .ok()
        .and_then(|roots| roots.get(db_path).cloned())
        .unwrap_or_else(|| default_root_for_db(db_path))
}

pub fn task_dir(root: &Path, repo_id: &str, task_id: &str) -> PathBuf {
    root.join("repos").join(repo_id).join("tasks").join(task_id)
}

/// The task directory for a task in `db`, if the task exists.
pub fn task_dir_for(db: &Db, db_path: &str, task_id: &str) -> Option<PathBuf> {
    let item = db.get_pipeline_item(task_id).ok()??;
    Some(task_dir(&root_for_db(db_path), &item.repo_id, &item.id))
}

/// The task directory for `task_id`, opening the database only if it exists.
pub fn task_dir_for_db_path(db_path: &str, task_id: &str) -> Option<PathBuf> {
    if !Path::new(db_path).exists() {
        return None;
    }
    let db = Db::open(db_path).ok()?;
    task_dir_for(&db, db_path, task_id)
}

// ---------------------------------------------------------------------------
// Atomic file publication
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Published {
    Written,
    /// The file already held exactly these bytes: a retried publication.
    AlreadyPresent,
}

fn temp_path(dir: &Path, name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    dir.join(format!(".{name}.tmp-{}-{n}", std::process::id()))
}

fn sync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Publish an immutable file: write a synced temporary file in the same
/// directory, hard-link it into place (which never replaces an existing
/// file), then sync the directory. An existing file with identical bytes is a
/// successful retry; one with different bytes is refused and left alone.
pub fn publish_immutable(dir: &Path, name: &str, bytes: &[u8]) -> Result<Published, String> {
    let path = dir.join(name);
    let compare = |path: &Path| -> Result<Published, String> {
        let existing = std::fs::read(path)
            .map_err(|error| format!("read existing {}: {error}", path.display()))?;
        if existing == bytes {
            Ok(Published::AlreadyPresent)
        } else {
            Err(format!(
                "ledger file {} already exists with different content; refusing to overwrite",
                path.display()
            ))
        }
    };
    if path.exists() {
        return compare(&path);
    }
    std::fs::create_dir_all(dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    let temp = temp_path(dir, name);
    write_synced(&temp, bytes).map_err(|error| format!("write {}: {error}", temp.display()))?;
    let linked = std::fs::hard_link(&temp, &path);
    let _ = std::fs::remove_file(&temp);
    match linked {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return compare(&path),
        Err(error) => return Err(format!("publish {}: {error}", path.display())),
    }
    sync_dir(dir).map_err(|error| format!("sync {}: {error}", dir.display()))?;
    Ok(Published::Written)
}

/// Atomically replace a file (temp file + rename + directory sync).
pub fn replace_atomically(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    let temp = temp_path(dir, name);
    write_synced(&temp, bytes).map_err(|error| format!("write {}: {error}", temp.display()))?;
    if let Err(error) = std::fs::rename(&temp, dir.join(name)) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("replace {}: {error}", dir.join(name).display()));
    }
    sync_dir(dir).map_err(|error| format!("sync {}: {error}", dir.display()))
}

// ---------------------------------------------------------------------------
// Flush
// ---------------------------------------------------------------------------

/// Deterministic crash points for tests: stop a flush at a boundary as if
/// the process died there. Never armed outside tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlushFault {
    /// Fail before writing the entry with this sequence.
    BeforePublish(i64),
    /// Write the entry's file, then fail before acknowledging it in SQL.
    AfterPublishBeforeAck(i64),
    /// Fail before writing `task.json`.
    BeforeSnapshot,
}

#[cfg(test)]
static FAULTS: LazyLock<Mutex<HashMap<PathBuf, FlushFault>>> = LazyLock::new(Default::default);

/// Arm one fault for flushes under `root`; it fires once.
#[cfg(test)]
pub(crate) fn inject_fault(root: &Path, fault: FlushFault) {
    FAULTS.lock().unwrap().insert(root.to_path_buf(), fault);
}

#[cfg(test)]
fn take_fault(root: &Path, matches: impl Fn(FlushFault) -> bool) -> bool {
    let mut faults = FAULTS.lock().unwrap();
    if faults.get(root).copied().is_some_and(matches) {
        faults.remove(root);
        return true;
    }
    false
}

#[cfg(not(test))]
fn take_fault(_root: &Path, _matches: impl Fn(FlushFault) -> bool) -> bool {
    false
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlushOutcome {
    pub published: usize,
    /// Publication stopped at a sequence reserved by an operation still in
    /// flight; later entries wait for it.
    pub waiting_on_reservation: bool,
}

type FlushLocks = HashMap<(PathBuf, String), Arc<Mutex<()>>>;

fn task_flush_lock(root: &Path, task_id: &str) -> Arc<Mutex<()>> {
    static LOCKS: LazyLock<Mutex<FlushLocks>> = LazyLock::new(Default::default);
    let mut locks = LOCKS.lock().unwrap_or_else(|poison| poison.into_inner());
    Arc::clone(
        locks
            .entry((root.to_path_buf(), task_id.to_string()))
            .or_default(),
    )
}

/// Publish a task's pending entries, in sequence order, then its `task.json`.
///
/// Each entry is written before it is acknowledged, and acknowledging it
/// releases the task events it held. A failure leaves the entry pending with
/// its error recorded: it is recoverable work, never a reason to re-record the
/// mutation that produced it.
pub fn flush_task(db: &Db, db_path: &str, task_id: &str) -> Result<FlushOutcome, String> {
    flush_task_at(db, &root_for_db(db_path), task_id)
}

pub fn flush_task_at(db: &Db, root: &Path, task_id: &str) -> Result<FlushOutcome, String> {
    let lock = task_flush_lock(root, task_id);
    let _guard = lock.lock().unwrap_or_else(|poison| poison.into_inner());
    let item = db
        .get_pipeline_item(task_id)
        .map_err(|error| format!("db error: {error}"))?
        .ok_or_else(|| format!("task not found: {task_id}"))?;
    let dir = task_dir(root, &item.repo_id, &item.id);
    // Disk authority (T13c): a task found with its disk ahead publishes
    // nothing until it is reconciled from that disk. A live write since the
    // finding may have raised the database's revision past the disk's, so
    // no revision check can tell a later flush it is safe to write over it.
    if authority::mode_for_root(root) == authority::Mode::Disk
        && authority::is_diverged(db, task_id)
    {
        let error =
            "the disk is ahead of the database; awaiting reconciliation from disk".to_string();
        let _ = db.record_task_snapshot_error(task_id, &error);
        return Err(error);
    }
    // Disk-first (T13d): in `disk` mode the records are written only by a
    // committing transaction, under SQLite's write lock, so this flush is an
    // empty transaction that owes them.
    if authority::mode_for_root(root) == authority::Mode::Disk {
        drop(_guard);
        return flush_task_disk_first(db, task_id);
    }
    let ledger = dir.join("ledger");
    let mut outcome = FlushOutcome::default();
    let pending = db
        .pending_ledger_entries(task_id)
        .map_err(|error| format!("db error: {error}"))?;
    for PendingLedgerEntry {
        sequence,
        entry_id,
        file_name,
        payload,
    } in pending
    {
        let (Some(file_name), Some(payload)) = (file_name, payload) else {
            outcome.waiting_on_reservation = true;
            break;
        };
        let fail = |error: String| {
            let _ = db.record_ledger_publish_error(task_id, sequence, &error);
            Err(format!("ledger entry {entry_id} is pending: {error}"))
        };
        if take_fault(root, |fault| fault == FlushFault::BeforePublish(sequence)) {
            return fail("injected failure before publication".into());
        }
        if let Err(error) = publish_immutable(&ledger, &file_name, &payload) {
            // Disk authority (T13c): the disk already holds another entry
            // under this sequence, so the database is behind it.
            if error.contains("already exists with different content")
                && authority::mode_for_root(root) == authority::Mode::Disk
            {
                authority::flag_divergence(db, task_id, &error);
            }
            return fail(error);
        }
        if take_fault(root, |fault| {
            fault == FlushFault::AfterPublishBeforeAck(sequence)
        }) {
            return Err(format!(
                "ledger entry {entry_id} is pending: injected failure after publication"
            ));
        }
        db.acknowledge_ledger_entry(task_id, sequence)
            .map_err(|error| format!("ledger entry {entry_id} is pending: db error: {error}"))?;
        outcome.published += 1;
    }
    if let Some((revision, published)) = db
        .task_snapshot_revisions(task_id)
        .map_err(|error| format!("db error: {error}"))?
    {
        if revision > published {
            if take_fault(root, |fault| fault == FlushFault::BeforeSnapshot) {
                let error = "injected failure before task.json".to_string();
                let _ = db.record_task_snapshot_error(task_id, &error);
                return Err(error);
            }
            // Disk authority (T13c): a task.json at a revision this
            // database never reached was written from rows it does not
            // hold; it is reconciled from, never written over.
            if authority::mode_for_root(root) == authority::Mode::Disk {
                let on_disk = std::fs::read(dir.join("task.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .and_then(|value| value.get("snapshot_revision").and_then(Value::as_i64));
                if let Some(on_disk) = on_disk.filter(|on_disk| *on_disk > revision) {
                    let error = format!(
                        "task.json on disk is at revision {on_disk}, beyond the database's {revision}"
                    );
                    authority::flag_divergence(db, task_id, &error);
                    let _ = db.record_task_snapshot_error(task_id, &error);
                    return Err(error);
                }
            }
            let facts = db
                .task_snapshot_facts(task_id)
                .map_err(|error| format!("db error: {error}"))?
                .unwrap_or(Value::Null);
            let mut bytes = serde_json::to_vec_pretty(&facts)
                .map_err(|error| format!("render task.json: {error}"))?;
            bytes.push(b'\n');
            if let Err(error) = replace_atomically(&dir, "task.json", &bytes) {
                let _ = db.record_task_snapshot_error(task_id, &error);
                return Err(error);
            }
            db.acknowledge_task_snapshot(task_id, revision)
                .map_err(|error| format!("db error: {error}"))?;
        }
    }
    Ok(outcome)
}

fn flush_task_disk_first(db: &Db, task_id: &str) -> Result<FlushOutcome, String> {
    let owed = |db: &Db| -> Result<(usize, bool), String> {
        let pending = db
            .pending_ledger_entries(task_id)
            .map_err(|error| format!("db error: {error}"))?;
        let filled = pending
            .iter()
            .filter(|entry| entry.payload.is_some())
            .count();
        let waiting = pending
            .iter()
            .position(|entry| entry.payload.is_none())
            .is_some_and(|reservation| reservation < pending.len() - 1);
        Ok((filled, waiting))
    };
    let (before, _) = owed(db)?;
    db.with_immediate_transaction(|db| {
        db.owe_task_publication(task_id);
        Ok::<_, rusqlite::Error>(())
    })
    .map_err(|error| error.to_string())?;
    let (after, waiting_on_reservation) = owed(db)?;
    Ok(FlushOutcome {
        published: before.saturating_sub(after),
        waiting_on_reservation,
    })
}

/// Publish a task's pending entries after a mutation whose success does not
/// depend on it. A failure stays pending (its announcements stay held) and
/// the publisher service retries it.
pub fn flush_task_best_effort(db: &Db, db_path: &str, task_id: &str) {
    if let Err(error) = flush_task(db, db_path, task_id) {
        log::warn!("task ledger for {task_id} is pending publication: {error}");
    }
}

/// Flush every task with pending work, then every owed `repo.json` and
/// tombstone. Returns the tasks (or `repo:<id>`, `removed:<kind>:<id>`) that
/// failed.
pub fn flush_all(db: &Db, db_path: &str) -> Vec<(String, String)> {
    let root = root_for_db(db_path);
    let tasks = match db.ledger_tasks_with_pending_work() {
        Ok(tasks) => tasks,
        Err(error) => return vec![(String::new(), format!("db error: {error}"))],
    };
    let mut failures: Vec<(String, String)> = tasks
        .into_iter()
        .filter_map(|task_id| {
            flush_task(db, db_path, &task_id)
                .err()
                .map(|error| (task_id, error))
        })
        .collect();
    failures.extend(flush_disk_records_at(db, &root));
    failures
}

/// Publish every owed `repo.json` and tombstone under `root` (T13).
pub fn flush_disk_records_at(db: &Db, root: &Path) -> Vec<(String, String)> {
    let mut failures = Vec::new();
    if authority::mode_for_root(root) == authority::Mode::Disk {
        let owed = db.repos_with_pending_disk_record().and_then(|repos| {
            db.pending_disk_removals()
                .map(|removals| (repos, !removals.is_empty()))
        });
        match owed {
            Ok((repos, removals)) if !repos.is_empty() || removals => {
                let written = db.with_immediate_transaction(|db| {
                    for repo in &repos {
                        db.owe_repo_publication(repo);
                    }
                    if removals {
                        db.owe_removal_publication();
                    }
                    Ok::<_, rusqlite::Error>(())
                });
                if let Err(error) = written {
                    failures.push(("disk records".to_string(), error.to_string()));
                }
            }
            Ok(_) => {}
            Err(error) => failures.push((String::new(), format!("db error: {error}"))),
        }
        return failures;
    }
    match db.repos_with_pending_disk_record() {
        Ok(repos) => {
            for repo_id in repos {
                if let Err(error) = flush_repo_at(db, root, &repo_id) {
                    failures.push((format!("repo:{repo_id}"), error));
                }
            }
        }
        Err(error) => failures.push((String::new(), format!("db error: {error}"))),
    }
    match db.pending_disk_removals() {
        Ok(removals) => {
            for (kind, id, repo_id) in removals {
                if let Err(error) = publish_removal_at(db, root, &kind, &id, &repo_id) {
                    let _ = db.record_disk_removal_error(&kind, &id, &error);
                    failures.push((format!("removed:{kind}:{id}"), error));
                }
            }
        }
        Err(error) => failures.push((String::new(), format!("db error: {error}"))),
    }
    failures
}

pub fn repo_dir(root: &Path, repo_id: &str) -> PathBuf {
    root.join("repos").join(repo_id)
}

/// Rewrite `repos/<repo-id>/repo.json` if it is owed.
pub fn flush_repo_at(db: &Db, root: &Path, repo_id: &str) -> Result<(), String> {
    let lock = task_flush_lock(root, &format!("repo:{repo_id}"));
    let _guard = lock.lock().unwrap_or_else(|poison| poison.into_inner());
    let Some((revision, published)) = db
        .repo_disk_revisions(repo_id)
        .map_err(|error| format!("db error: {error}"))?
    else {
        return Ok(());
    };
    if revision <= published {
        return Ok(());
    }
    // A removed repository has no row here any more; its tombstone is
    // written from the removal outbox.
    let Some(mut record) = db
        .repo_disk_record(repo_id)
        .map_err(|error| format!("db error: {error}"))?
    else {
        return Ok(());
    };
    // The installation that owns the repository (T13c), so one sharing
    // this root never rebuilds or reconciles it as its own.
    if let Some(installation) = authority::installation_for_root(root) {
        record["installation"] = Value::String(installation);
    }
    let mut bytes =
        serde_json::to_vec_pretty(&record).map_err(|error| format!("render repo.json: {error}"))?;
    bytes.push(b'\n');
    if let Err(error) = replace_atomically(&repo_dir(root, repo_id), "repo.json", &bytes) {
        let _ = db.record_repo_disk_record_error(repo_id, &error);
        return Err(error);
    }
    db.acknowledge_repo_disk_record(repo_id, revision)
        .map_err(|error| format!("db error: {error}"))
}

/// Replace a removed task's `task.json` (or a removed repository's
/// `repo.json`) with a tombstone, so a rebuild does not bring it back. The
/// ledger stays. Nothing to do when the record was never published.
pub(crate) fn publish_removal_at(
    db: &Db,
    root: &Path,
    kind: &str,
    id: &str,
    repo_id: &str,
) -> Result<(), String> {
    let (lock_key, dir, file, identity) = match kind {
        "task" => (
            id.to_string(),
            task_dir(root, repo_id, id),
            "task.json",
            serde_json::json!({ "task_id": id, "repo_id": repo_id }),
        ),
        _ => (
            format!("repo:{id}"),
            repo_dir(root, id),
            "repo.json",
            serde_json::json!({ "repo_id": id }),
        ),
    };
    let lock = task_flush_lock(root, &lock_key);
    let _guard = lock.lock().unwrap_or_else(|poison| poison.into_inner());
    // Re-created meanwhile: the live record is owed instead.
    let recreated = match kind {
        "task" => db.get_pipeline_item(id).map(|item| item.is_some()),
        _ => db.get_repo(id).map(|repo| repo.is_some()),
    }
    .map_err(|error| format!("db error: {error}"))?;
    if !recreated && dir.join(file).exists() {
        let mut tombstone = identity;
        tombstone["schema_version"] = serde_json::json!(SCHEMA_VERSION);
        tombstone[crate::db::task_state::REMOVED_KEY] = serde_json::json!(true);
        let mut bytes = serde_json::to_vec_pretty(&tombstone)
            .map_err(|error| format!("render tombstone: {error}"))?;
        bytes.push(b'\n');
        replace_atomically(&dir, file, &bytes)?;
    }
    db.acknowledge_disk_removal(kind, id)
        .map_err(|error| format!("db error: {error}"))
}

/// Startup recovery, run before any service can schedule or dispatch work:
/// drop reservations no live operation owns, import the history of open tasks
/// that predate the ledger, and publish everything pending.
pub fn recover_on_startup(db: &Db, db_path: &str) {
    match db.release_stale_ledger_reservations() {
        Ok(0) => {}
        Ok(released) => log::warn!("released {released} stale task-ledger reservation(s)"),
        Err(error) => log::error!("failed to release stale task-ledger reservations: {error}"),
    }
    match db.tasks_needing_ledger_backfill() {
        Ok(tasks) => {
            for task_id in tasks {
                if let Err(error) = db.backfill_task_ledger(&task_id) {
                    log::error!("failed to backfill task ledger for {task_id}: {error}");
                }
            }
        }
        Err(error) => log::error!("failed to list tasks needing ledger backfill: {error}"),
    }
    for (task_id, error) in flush_all(db, db_path) {
        log::error!("task ledger for {task_id} could not be published at startup: {error}");
    }
}

// ---------------------------------------------------------------------------
// Engine-observed provenance
// ---------------------------------------------------------------------------

/// Branch and committed SHA the engine observed in a run's workspace when it
/// accepted the run's result. Never derived later from whatever HEAD is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceObservation {
    pub branch: Option<String>,
    pub committed_sha: Option<String>,
    /// `workspace` when read from the run's worktree, `unborn` for a
    /// worktree with no commit yet, `no_workspace` when the run recorded no
    /// worktree or it no longer exists, `not_a_checkout` when the recorded
    /// directory exists but git cannot read it.
    pub provenance: &'static str,
}

impl WorkspaceObservation {
    pub fn none() -> Self {
        Self {
            branch: None,
            committed_sha: None,
            provenance: "no_workspace",
        }
    }
}

fn git_output(cwd: &str, args: &[&str]) -> Result<Option<String>, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!text.is_empty()).then_some(text))
}

/// Read-only observation of a workspace's current branch and HEAD commit.
///
/// Whatever cannot be observed is recorded as unknown with the reason in
/// `provenance`, never filled from another source: a result is not refused
/// over the state of its workspace, and no commit is invented for it. Only a
/// failure to run git at all is an error.
pub fn observe_workspace(cwd: Option<&str>) -> Result<WorkspaceObservation, String> {
    let Some(cwd) = cwd.filter(|cwd| !cwd.trim().is_empty() && Path::new(cwd).is_dir()) else {
        return Ok(WorkspaceObservation::none());
    };
    if git_output(cwd, &["rev-parse", "--git-dir"])?.is_none() {
        return Ok(WorkspaceObservation {
            provenance: "not_a_checkout",
            ..WorkspaceObservation::none()
        });
    }
    let branch = git_output(cwd, &["symbolic-ref", "--short", "-q", "HEAD"])?;
    let committed_sha = git_output(cwd, &["rev-parse", "--verify", "-q", "HEAD^{commit}"])?;
    let provenance = if committed_sha.is_some() {
        "workspace"
    } else {
        "unborn"
    };
    Ok(WorkspaceObservation {
        branch,
        committed_sha,
        provenance,
    })
}

/// The `result` object of a new result entry.
pub fn result_body(
    status: &str,
    run: &crate::db::StageRun,
    observed: &WorkspaceObservation,
    metadata: Option<&Value>,
    request: Value,
) -> Value {
    serde_json::json!({
        "status": status,
        "stage": run.stage,
        "run_kind": run.kind,
        "branch": observed.branch,
        "committed_sha": observed.committed_sha,
        "provenance": {
            "branch": observed.provenance,
            "committed_sha": observed.provenance,
        },
        "metadata": metadata,
        "legacy_format": false,
        "request": request,
    })
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// One published ledger entry, as a following session reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerFile {
    pub file_name: String,
    pub sequence: i64,
    pub kind: LedgerEntryKind,
    pub envelope: Value,
    /// The verbatim Markdown body of a result or input entry.
    pub message: Option<String>,
}

impl LedgerFile {
    pub fn entry_id(&self) -> Option<&str> {
        self.envelope.get("entry_id").and_then(Value::as_str)
    }

    /// The kind-specific object (`result`, `input`, ...).
    pub fn body(&self) -> &Value {
        self.envelope
            .get(self.kind.as_str())
            .unwrap_or(&Value::Null)
    }
}

pub fn parse_ledger_file(file_name: &str, bytes: &[u8]) -> Result<LedgerFile, String> {
    let (number, rest) = file_name
        .split_once('-')
        .ok_or_else(|| format!("unrecognized ledger file name {file_name}"))?;
    let sequence = number
        .parse::<i64>()
        .map_err(|_| format!("unrecognized ledger file name {file_name}"))?;
    let kind = rest
        .rsplit_once('.')
        .and_then(|(kind, _)| LedgerEntryKind::parse(kind))
        .ok_or_else(|| format!("unrecognized ledger file name {file_name}"))?;
    let text = std::str::from_utf8(bytes).map_err(|error| format!("{file_name}: {error}"))?;
    let (envelope, message) = if kind.has_message_body() {
        let front = text
            .strip_prefix("---\n")
            .ok_or_else(|| format!("{file_name}: missing front matter"))?;
        let (json, body) = front
            .split_once("\n---\n\n")
            .ok_or_else(|| format!("{file_name}: unterminated front matter"))?;
        (json, Some(body.to_string()))
    } else {
        (text, None)
    };
    let envelope: Value =
        serde_json::from_str(envelope).map_err(|error| format!("{file_name}: {error}"))?;
    Ok(LedgerFile {
        file_name: file_name.to_string(),
        sequence,
        kind,
        envelope,
        message,
    })
}

/// Every published entry of a task directory, in sequence order.
pub fn read_ledger(task_dir: &Path) -> Result<Vec<LedgerFile>, String> {
    let ledger = task_dir.join("ledger");
    let entries = match std::fs::read_dir(&ledger) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", ledger.display())),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {}: {error}", ledger.display()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let bytes = std::fs::read(entry.path())
            .map_err(|error| format!("read {}: {error}", entry.path().display()))?;
        files.push(parse_ledger_file(&name, &bytes)?);
    }
    files.sort_by_key(|file| file.sequence);
    Ok(files)
}

// ---------------------------------------------------------------------------
// Session delivery
// ---------------------------------------------------------------------------

/// The result that caused a session, as its preamble states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggeringResult {
    pub entry_id: String,
    /// Path relative to the task directory, e.g. `ledger/000003-result.md`.
    pub file: String,
    pub status: String,
    pub stage: Option<String>,
    pub run_id: Option<String>,
    pub branch: Option<String>,
    pub committed_sha: Option<String>,
    pub message: String,
}

impl TriggeringResult {
    pub fn from_file(file: &LedgerFile) -> Option<Self> {
        if file.kind != LedgerEntryKind::Result {
            return None;
        }
        let body = file.body();
        let text =
            |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
        Some(Self {
            entry_id: file.entry_id()?.to_string(),
            file: format!("ledger/{}", file.file_name),
            status: text(body, "status").unwrap_or_else(|| "unknown".into()),
            stage: text(body, "stage"),
            run_id: text(&file.envelope, "run_id"),
            branch: text(body, "branch"),
            committed_sha: text(body, "committed_sha"),
            message: file.message.clone().unwrap_or_default(),
        })
    }
}

/// An upstream result a stage's dependency edge was satisfied by (T4), as
/// its session is told about it. `role` is `base` (the workspace forked from
/// `committed_sha`), `merge` (not merged by the engine; the session merges it
/// if it needs it) or `gate` (it only held the stage until it existed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyInput {
    pub upstream_task_id: String,
    pub upstream_stage: String,
    pub role: String,
    pub result_id: Option<String>,
    pub committed_sha: Option<String>,
}

/// What the engine delivers to a new session: where the ledger is, the
/// result that caused this session, and the dependency inputs of its stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLedger {
    pub task_dir: String,
    pub trigger: Option<TriggeringResult>,
    pub dependencies: Vec<DependencyInput>,
}

thread_local! {
    static PENDING_TRIGGER: RefCell<Option<TriggeringResult>> = const { RefCell::new(None) };
    static PENDING_DEPENDENCIES: RefCell<Vec<DependencyInput>> = const { RefCell::new(Vec::new()) };
}

/// Run `prepare` with the dependency inputs of the stage whose session it
/// prepares, read by the caller from the task's edges in the same
/// preparation that chose the workspace's fork point.
pub fn with_dependency_inputs<T>(inputs: Vec<DependencyInput>, prepare: impl FnOnce() -> T) -> T {
    struct Reset(Vec<DependencyInput>);
    impl Drop for Reset {
        fn drop(&mut self) {
            let previous = std::mem::take(&mut self.0);
            PENDING_DEPENDENCIES.with(|slot| *slot.borrow_mut() = previous);
        }
    }
    let previous =
        PENDING_DEPENDENCIES.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), inputs));
    let _reset = Reset(previous);
    prepare()
}

/// Run `prepare` with an explicitly identified triggering result that is
/// being recorded by the same operation and is not yet on disk (a revision
/// prepares the reviser's session before it records the reviewer's result,
/// in a sequence reserved for it). The operation must publish that entry
/// before the session it prepares is started.
pub fn with_pending_trigger<T>(trigger: TriggeringResult, prepare: impl FnOnce() -> T) -> T {
    struct Reset(Option<TriggeringResult>);
    impl Drop for Reset {
        fn drop(&mut self) {
            let previous = self.0.take();
            PENDING_TRIGGER.with(|slot| *slot.borrow_mut() = previous);
        }
    }
    let previous = PENDING_TRIGGER.with(|slot| slot.borrow_mut().replace(trigger));
    let _reset = Reset(previous);
    prepare()
}

/// The triggering result for a new session of `stage`, from published files.
///
/// The newest result recorded since the task's latest transition caused this
/// session (a completion that advances, a reviewer's result that loops back,
/// a main result a post follows). With none, a new session of the stage that
/// transition entered (a rerun, a restart, a resumed setup) receives that
/// transition's trigger; a session of any other stage receives nothing
/// rather than a result that did not cause it.
pub fn resolve_trigger(task_dir: &Path, stage: &str) -> Option<TriggeringResult> {
    if let Some(pending) = PENDING_TRIGGER.with(|slot| slot.borrow().clone()) {
        return Some(pending);
    }
    let files = match read_ledger(task_dir) {
        Ok(files) => files,
        Err(error) => {
            log::warn!("cannot read task ledger {}: {error}", task_dir.display());
            return None;
        }
    };
    resolve_trigger_in(&files, stage)
}

/// [`resolve_trigger`] over entries already read, in sequence order. A
/// transfer (T9) resolves the source session's trigger from the files it
/// carries, before any of them is on this machine's disk.
pub fn resolve_trigger_in(files: &[LedgerFile], stage: &str) -> Option<TriggeringResult> {
    let last_transition = files
        .iter()
        .rev()
        .find(|file| file.kind == LedgerEntryKind::Transition);
    let floor = last_transition.map_or(0, |file| file.sequence);
    // An engine-observed ending (T13) is not a result a session recorded,
    // so it never causes one.
    if let Some(result) = files.iter().rev().find(|file| {
        file.kind == LedgerEntryKind::Result
            && file.sequence > floor
            && !crate::db::task_store::is_engine_observed_result(file.body())
    }) {
        return TriggeringResult::from_file(result);
    }
    let transition = last_transition?;
    if transition.body().get("to_stage").and_then(Value::as_str) != Some(stage) {
        return None;
    }
    let trigger_id = transition
        .body()
        .get("triggering_result_id")
        .and_then(Value::as_str)?;
    files
        .iter()
        .find(|file| file.entry_id() == Some(trigger_id))
        .and_then(TriggeringResult::from_file)
}

/// The ledger delivery for a session whose environment names its task
/// directory.
pub fn session_ledger(spawn_env: &HashMap<String, String>, stage: &str) -> Option<SessionLedger> {
    let task_dir = spawn_env.get(LEDGER_PATH_ENV)?;
    Some(SessionLedger {
        trigger: resolve_trigger(Path::new(task_dir), stage),
        task_dir: task_dir.clone(),
        dependencies: PENDING_DEPENDENCIES.with(|slot| slot.borrow().clone()),
    })
}

/// Preamble section describing the ledger and the triggering result.
///
/// Mirrors `buildKannaLedgerSection` in
/// packages/core/src/workflow/prompt-builder.ts — keep the texts in sync. The
/// caller substitutes this last, so a message that happens to contain a
/// template marker stays literal text.
pub fn render_ledger_section(ledger: &SessionLedger) -> String {
    let mut section = format!(
        "Task ledger: `{}` (also in `{LEDGER_PATH_ENV}`). It holds this task's `task.json` and, under `ledger/`, its recorded results, tool-delivered inputs, stage transitions and workflow replacements as ordered files. Read an earlier entry there when you need it.",
        ledger.task_dir
    );
    match &ledger.trigger {
        None => section.push_str("\n\nNo recorded result caused this session."),
        Some(trigger) => {
            let or_unknown =
                |value: &Option<String>| value.clone().unwrap_or_else(|| "unknown".into());
            section.push_str(&format!(
                "\n\nResult that caused this session: ledger entry `{}` (`{}`), status `{}`, recorded by stage `{}` (run `{}`, branch `{}`, commit `{}`). Its message follows verbatim.\n\n-----BEGIN RESULT MESSAGE-----\n{}\n-----END RESULT MESSAGE-----",
                trigger.entry_id,
                trigger.file,
                trigger.status,
                or_unknown(&trigger.stage),
                or_unknown(&trigger.run_id),
                or_unknown(&trigger.branch),
                or_unknown(&trigger.committed_sha),
                trigger.message,
            ));
        }
    }
    if !ledger.dependencies.is_empty() {
        section.push_str("\n\nDependency inputs of this stage, in edge order:");
        for dependency in &ledger.dependencies {
            let or_unknown =
                |value: &Option<String>| value.clone().unwrap_or_else(|| "unknown".into());
            let consequence = match (dependency.role.as_str(), &dependency.committed_sha) {
                ("base", Some(_)) => "This workspace was forked from that commit.",
                ("base", None) => {
                    "That result recorded no commit, so this workspace forked from the repository's default start point."
                }
                ("merge", _) => {
                    "Kanna did not merge it. Merge that commit into your branch yourself if this stage needs it."
                }
                _ => "It only held this stage until it existed; your workspace was not rebased onto it.",
            };
            section.push_str(&format!(
                "\n- `{}`: task `{}` left stage `{}` with result `{}` at commit `{}`. {consequence}",
                dependency.role,
                dependency.upstream_task_id,
                dependency.upstream_stage,
                or_unknown(&dependency.result_id),
                or_unknown(&dependency.committed_sha),
            ));
        }
    }
    section
}
