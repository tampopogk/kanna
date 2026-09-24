//! Disk-first writes (spec §16.11, T13d): in `disk` authority mode the task
//! directory is the first durable write of every mutation.
//!
//! A transaction that owes disk records publishes them from its own
//! uncommitted rows before SQLite commits it
//! ([`crate::db::disk_first::DbConnection::publish_and_commit`]), holding
//! SQLite's write lock, so there is never a second writer. For each task it
//! touched, in order:
//!
//! 1. **Refuse** if the disk holds anything this database did not write: the
//!    task is fenced (`disk_divergence`, or a failed commit in this
//!    process), a ledger file at one of its unpublished sequences holds other
//!    bytes, or `task.json` is at a revision the database never published.
//!    Nothing is written, the transaction rolls back, and the task is
//!    reconciled from its disk.
//! 2. **Write `task.json`** — the commit point. It carries the rows as the
//!    transaction leaves them (`state`, whose `reflects_through` counts this
//!    transaction's entries) and, under `ledger.in_flight`, the exact bytes
//!    of every entry not yet published as a file. One atomic rename makes the
//!    whole mutation durable on disk, rows and entries together.
//! 3. **Publish the entry files**, in sequence order up to an open
//!    reservation, acknowledging each. A failure here does not undo the
//!    mutation: the entry is durable in `task.json` and is published later.
//! 4. SQLite commits. If that fails, the disk already holds the mutation:
//!    the task is fenced and reconciled from its directory, so the mutation
//!    stands even though its caller saw an error.
//!
//! Owed `repo.json` records and tombstones are written the same way, before
//! the commit. A crash before step 2 leaves no trace of the mutation
//! anywhere; a crash after it leaves the mutation on disk, and the next
//! start materializes the in-flight entry files
//! ([`materialize_in_flight`]) and reconciles the database from the
//! directory.

use super::authority;
use super::{publish_immutable, replace_atomically, root_for_db, task_dir};
use crate::db::disk_first::Touched;
use crate::db::Db;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// Why a disk-first commit was refused, and what the disk now holds.
#[derive(Debug)]
pub(crate) struct Refusal {
    pub message: String,
    /// Tasks whose disk now holds more than the database (a record was
    /// written and the transaction did not commit, or the disk was already
    /// ahead): fenced and reconciled from disk.
    pub fence: Vec<String>,
    /// A test's simulated crash: a dead process does nothing afterwards.
    pub crashed: bool,
}

impl Refusal {
    fn new(message: String, fence: Vec<String>) -> Self {
        Self {
            message: format!("disk-first write refused: {message}"),
            fence,
            crashed: false,
        }
    }

    pub(crate) fn commit_failed(written: Vec<String>, error: &rusqlite::Error) -> Self {
        Self {
            message: format!(
                "SQLite did not commit after the disk did ({error}); reconciling from disk"
            ),
            fence: written,
            crashed: false,
        }
    }
}

// ---------------------------------------------------------------------------
// In-process fence
// ---------------------------------------------------------------------------

/// Tasks this process knows the disk is ahead for. The durable fence is
/// `disk_divergence`; this one also holds when writing that row failed
/// (after a failed commit the database may be what failed).
static FENCED: LazyLock<Mutex<BTreeSet<(PathBuf, String)>>> = LazyLock::new(Default::default);

fn fenced() -> std::sync::MutexGuard<'static, BTreeSet<(PathBuf, String)>> {
    FENCED.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub(crate) fn is_fenced(root: &Path, task_id: &str) -> bool {
    fenced().contains(&(root.to_path_buf(), task_id.to_string()))
}

pub(crate) fn fenced_tasks(root: &Path) -> Vec<String> {
    fenced()
        .iter()
        .filter(|(fenced_root, _)| fenced_root == root)
        .map(|(_, task)| task.clone())
        .collect()
}

pub(crate) fn unfence(root: &Path, task_id: &str) {
    fenced().remove(&(root.to_path_buf(), task_id.to_string()));
}

thread_local! {
    /// Tasks the running reconciliation is repairing from their own disk:
    /// its transaction writes back the directory's projection, so the fence
    /// that keeps every other write out does not apply to it.
    static REPAIRING: std::cell::RefCell<BTreeSet<String>> = const { std::cell::RefCell::new(BTreeSet::new()) };
}

/// Run `repair` with `tasks` exempt from the fence.
pub(crate) fn repairing<T>(tasks: &BTreeSet<String>, repair: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            REPAIRING.with(|slot| slot.borrow_mut().clear());
        }
    }
    REPAIRING.with(|slot| *slot.borrow_mut() = tasks.clone());
    let _reset = Reset;
    repair()
}

fn is_repairing(task_id: &str) -> bool {
    REPAIRING.with(|slot| slot.borrow().contains(task_id))
}

/// After a refused or failed disk-first commit was rolled back: fence every
/// task whose disk is now ahead, durably when the database allows it.
pub(crate) fn after_rollback(db: &Db, refusal: &Refusal) {
    if refusal.crashed {
        return;
    }
    let root = root_for_db(db.db_path());
    for task in &refusal.fence {
        fenced().insert((root.clone(), task.clone()));
        authority::flag_divergence(db, task, &refusal.message);
    }
}

// ---------------------------------------------------------------------------
// Test crash points
// ---------------------------------------------------------------------------

/// Where a test stops a disk-first commit, as if the process died there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrashPoint {
    /// Nothing written yet.
    BeforeTaskJson,
    /// `task.json` written; no entry file.
    AfterTaskJson,
    /// The entry with this sequence published; later ones not.
    AfterEntry(i64),
    /// Every record written; SQLite not committed.
    BeforeCommit,
}

#[cfg(test)]
static CRASHES: LazyLock<Mutex<std::collections::HashMap<(PathBuf, String), CrashPoint>>> =
    LazyLock::new(Default::default);

/// Crash the next disk-first commit of `task_id` under `root` at `point`.
#[cfg(test)]
pub(crate) fn crash_at(root: &Path, task_id: &str, point: CrashPoint) {
    CRASHES
        .lock()
        .unwrap()
        .insert((root.to_path_buf(), task_id.to_string()), point);
}

#[cfg(test)]
fn crashes_here(root: &Path, task_id: &str, point: CrashPoint) -> bool {
    let mut crashes = CRASHES.lock().unwrap();
    let key = (root.to_path_buf(), task_id.to_string());
    if crashes.get(&key) == Some(&point) {
        crashes.remove(&key);
        return true;
    }
    false
}

#[cfg(not(test))]
fn crashes_here(_root: &Path, _task_id: &str, _point: CrashPoint) -> bool {
    false
}

fn crash(task_id: &str, point: CrashPoint) -> Refusal {
    Refusal {
        message: format!("simulated crash of task {task_id} at {point:?}"),
        fence: Vec::new(),
        crashed: true,
    }
}

// ---------------------------------------------------------------------------
// Publication
// ---------------------------------------------------------------------------

/// Write every record the open transaction owes, before it commits. Returns
/// the tasks whose `task.json` was written.
pub(crate) fn publish_before_commit(db: &Db, touched: Touched) -> Result<Vec<String>, Refusal> {
    let root = root_for_db(db.db_path());
    let db_refusal = |written: &[String], error: rusqlite::Error| {
        Refusal::new(format!("db error: {error}"), written.to_vec())
    };
    let mut written: Vec<String> = Vec::new();
    let mut tasks = touched.forced_tasks;
    tasks.extend(
        db.task_ids_for_outbox_rows(&touched.snapshots, &touched.entries)
            .map_err(|error| db_refusal(&written, error))?,
    );
    for task in &tasks {
        match publish_task(db, &root, task, &written)? {
            Published::Nothing => {}
            Published::Written => written.push(task.clone()),
        }
    }
    let mut repos = touched.forced_repos;
    repos.extend(
        db.repo_ids_for_outbox_rows(&touched.repos)
            .map_err(|error| db_refusal(&written, error))?,
    );
    for repo in &repos {
        super::flush_repo_at(db, &root, repo)
            .map_err(|error| Refusal::new(format!("repo {repo}: {error}"), written.clone()))?;
    }
    if touched.removals {
        for (kind, id, repo_id) in db
            .pending_disk_removals()
            .map_err(|error| db_refusal(&written, error))?
        {
            super::publish_removal_at(db, &root, &kind, &id, &repo_id).map_err(|error| {
                Refusal::new(
                    format!("tombstone of {kind} {id}: {error}"),
                    written.clone(),
                )
            })?;
        }
    }
    Ok(written)
}

enum Published {
    Nothing,
    Written,
}

/// The `snapshot_revision` of the `task.json` in `dir`, if there is one.
fn revision_on_disk(dir: &Path) -> Option<i64> {
    let bytes = std::fs::read(dir.join("task.json")).ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .get("snapshot_revision")
        .and_then(Value::as_i64)
}

fn publish_task(
    db: &Db,
    root: &Path,
    task_id: &str,
    written: &[String],
) -> Result<Published, Refusal> {
    let db_error =
        |error: rusqlite::Error| Refusal::new(format!("db error: {error}"), written.to_vec());
    let Some(item) = db.get_pipeline_item(task_id).map_err(db_error)? else {
        // Removed in this transaction: its tombstone is the removal's.
        return Ok(Published::Nothing);
    };
    let pending = db.pending_ledger_entries(task_id).map_err(db_error)?;
    let (revision, published) = db
        .task_snapshot_revisions(task_id)
        .map_err(db_error)?
        .unwrap_or((0, 0));
    let filled: Vec<_> = pending
        .iter()
        .filter_map(|entry| {
            Some((
                entry.sequence,
                entry.file_name.as_ref()?,
                entry.payload.as_ref()?,
            ))
        })
        .collect();
    if filled.is_empty() && revision <= published {
        return Ok(Published::Nothing);
    }
    let ahead = |why: String| {
        let mut fence = written.to_vec();
        fence.push(task_id.to_string());
        Refusal::new(
            format!("task {task_id}: the disk is ahead of the database ({why})"),
            fence,
        )
    };
    if !is_repairing(task_id) && authority::is_diverged(db, task_id) {
        return Err(Refusal::new(
            format!("task {task_id}: the disk is ahead of the database; awaiting reconciliation from disk"),
            written.to_vec(),
        ));
    }
    let dir = task_dir(root, &item.repo_id, task_id);
    for (sequence, file_name, payload) in &filled {
        if let Ok(existing) = std::fs::read(dir.join("ledger").join(file_name)) {
            if existing != **payload {
                return Err(ahead(format!(
                    "ledger file {file_name} holds other bytes than entry {sequence}"
                )));
            }
        }
    }
    if let Some(on_disk) = revision_on_disk(&dir).filter(|on_disk| *on_disk > published) {
        return Err(ahead(format!(
            "task.json on disk is at revision {on_disk}, beyond the database's published {published}"
        )));
    }

    // Entries publishable now: in order, up to an open reservation.
    let publishable: Vec<_> = pending
        .iter()
        .take_while(|entry| entry.payload.is_some())
        .filter_map(|entry| {
            Some((
                entry.sequence,
                entry.file_name.clone()?,
                entry.payload.clone()?,
            ))
        })
        .collect();
    let mut in_flight = Vec::with_capacity(filled.len());
    for (sequence, file_name, payload) in &filled {
        let text = std::str::from_utf8(payload).map_err(|error| {
            Refusal::new(
                format!("ledger entry {sequence} of {task_id} is not UTF-8: {error}"),
                written.to_vec(),
            )
        })?;
        in_flight.push(json!({ "sequence": sequence, "file_name": file_name, "payload": text }));
    }
    let mut facts = db
        .task_snapshot_facts(task_id)
        .map_err(db_error)?
        .unwrap_or(Value::Null);
    if let Some(ledger) = facts.get_mut("ledger").and_then(Value::as_object_mut) {
        if let Some((last, _, _)) = publishable.last() {
            let through = ledger
                .get("published_through")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            ledger.insert("published_through".into(), json!(through.max(*last)));
        }
        if !in_flight.is_empty() {
            ledger.insert(IN_FLIGHT_KEY.into(), Value::Array(in_flight));
        }
    }
    let mut bytes = serde_json::to_vec_pretty(&facts).map_err(|error| {
        Refusal::new(
            format!("render task.json of {task_id}: {error}"),
            written.to_vec(),
        )
    })?;
    bytes.push(b'\n');

    if crashes_here(root, task_id, CrashPoint::BeforeTaskJson) {
        return Err(crash(task_id, CrashPoint::BeforeTaskJson));
    }
    replace_atomically(&dir, "task.json", &bytes)
        .map_err(|error| Refusal::new(format!("task {task_id}: {error}"), written.to_vec()))?;
    // From here the mutation is on disk: any refusal fences this task too.
    let mut now_written = written.to_vec();
    now_written.push(task_id.to_string());
    let db_error_after =
        |error: rusqlite::Error| Refusal::new(format!("db error: {error}"), now_written.clone());
    db.acknowledge_task_snapshot(task_id, revision)
        .map_err(db_error_after)?;
    if crashes_here(root, task_id, CrashPoint::AfterTaskJson) {
        return Err(crash(task_id, CrashPoint::AfterTaskJson));
    }
    let ledger = dir.join("ledger");
    for (sequence, file_name, payload) in &publishable {
        if let Err(error) = publish_immutable(&ledger, file_name, payload) {
            // Durable in task.json already; published by a later flush.
            log::warn!("task {task_id}: ledger entry {sequence} stays in flight: {error}");
            db.record_ledger_publish_error(task_id, *sequence, &error)
                .map_err(db_error_after)?;
            break;
        }
        db.acknowledge_ledger_entry(task_id, *sequence)
            .map_err(db_error_after)?;
        if crashes_here(root, task_id, CrashPoint::AfterEntry(*sequence)) {
            return Err(crash(task_id, CrashPoint::AfterEntry(*sequence)));
        }
    }
    if crashes_here(root, task_id, CrashPoint::BeforeCommit) {
        return Err(crash(task_id, CrashPoint::BeforeCommit));
    }
    Ok(Published::Written)
}

/// `task.json` `ledger.in_flight`: entries a disk-first commit made durable
/// in `task.json` before (or without) publishing their files.
pub const IN_FLIGHT_KEY: &str = "in_flight";

/// Publish the files of in-flight entries a `task.json` carries and the
/// ledger does not hold yet (a crash between a disk-first commit point and
/// its files). Returns how many were written.
pub(crate) fn materialize_in_flight(
    directory: &super::rebuild::TaskDirectory,
) -> Result<usize, String> {
    let ledger = directory.path.join("ledger");
    let mut written = 0;
    for (file_name, bytes) in &directory.snapshot.in_flight {
        if ledger.join(file_name).exists() {
            continue;
        }
        if publish_immutable(&ledger, file_name, bytes)? == super::Published::Written {
            written += 1;
        }
    }
    Ok(written)
}
