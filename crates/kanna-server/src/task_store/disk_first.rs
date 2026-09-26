//! Disk-first writes (spec §16.11, T13d): in `disk` authority mode the task
//! directory is the first durable write of every mutation.
//!
//! A transaction that owes disk records publishes them from its own
//! uncommitted rows before SQLite commits it
//! ([`crate::db::disk_first::DbConnection::publish_and_commit`]), holding
//! SQLite's write lock, so there is never a second writer. The tasks it
//! touched are one mutation, written all or none:
//!
//! 1. **Refuse** if the disk of any of them holds anything this database
//!    did not write: the task is fenced (`disk_divergence`, or a failed
//!    commit in this process), a ledger file at one of its unpublished
//!    sequences holds other bytes, or `task.json` is at a revision the
//!    database never published. Every task is vetted before any is
//!    written, so a refusal writes nothing: the transaction rolls back, and
//!    a task whose disk is ahead is reconciled from it.
//! 2. **Write each `task.json`** — the commit point. It carries the rows as
//!    the transaction leaves them (`state`, whose `reflects_through` counts
//!    this transaction's entries) and, under `ledger.in_flight`, the exact
//!    bytes of every entry not yet published as a file. One atomic rename
//!    makes a task's part durable on disk, rows and entries together. If a
//!    later task's `task.json` cannot be written, the earlier ones are put
//!    back and every task is fenced and reconciled from its disk, so no task
//!    keeps a half the others lost (a child's result without its parent's
//!    join resolution). A process that dies between two tasks' renames is
//!    not covered: it leaves the earlier tasks' parts on disk.
//!
//! Then, for each task in turn:
//!
//! 3. **Publish the entry files**, in sequence order, each written and
//!    synced (file and directory) and acknowledged, whatever reservation
//!    another operation holds below them: durability is never held back. A
//!    failure here does not undo the mutation: the entry is durable in
//!    `task.json` and is published later.
//! 4. **Advance the watermark.** Ordering is the separate
//!    `ledger.readable_through`: every sequence at or below it is a synced
//!    entry file or a gap (a released reservation). Step 2 recorded it as
//!    it stood; once the files are synced `task.json` is rewritten with it
//!    over them. Consumers that read in order ([`super::resolve_trigger`])
//!    read it once and then only the files at or below it.
//! 5. SQLite commits. If that fails, the disk already holds the mutation:
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
    /// Every entry file synced; the watermark not yet advanced over them.
    BeforeWatermark,
    /// Not a crash: writing this entry's file fails, as an I/O error would.
    PublishFails(i64),
    /// Not a crash: replacing this task's `task.json` fails, as an I/O
    /// error would.
    TaskJsonFails,
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
///
/// A transaction may touch several tasks (a child's result that resolves
/// its parent's join member, a departure that supersedes a dependent's
/// input). Their records are one mutation, so they reach the disk all or
/// none: every task is vetted before any `task.json` is written, and a
/// `task.json` that cannot be written puts back the ones written before it.
pub(crate) fn publish_before_commit(db: &Db, touched: Touched) -> Result<Vec<String>, Refusal> {
    let root = root_for_db(db.db_path());
    let db_refusal = |written: &[String], error: rusqlite::Error| {
        Refusal::new(format!("db error: {error}"), written.to_vec())
    };
    let mut tasks = touched.forced_tasks;
    tasks.extend(
        db.task_ids_for_outbox_rows(&touched.snapshots, &touched.entries)
            .map_err(|error| db_refusal(&[], error))?,
    );
    // 1. Refuse before anything is written, whichever task refuses.
    let mut vetted = Vec::new();
    for task in &tasks {
        if let Some(task) = vet_task(db, &root, task)? {
            vetted.push(task);
        }
    }
    // 2. The commit point of every task.
    write_task_jsons(&root, &vetted)?;
    let written: Vec<String> = vetted.iter().map(|task| task.task_id.clone()).collect();
    // 3-4. Entry files and watermarks. The mutation is on disk: any refusal
    // from here fences every task it wrote.
    for task in &vetted {
        publish_entries(db, &root, task, &written)?;
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
            // Tests key a removal's crash as `<kind>:<id>`.
            let key = format!("{kind}:{id}");
            if crashes_here(&root, &key, CrashPoint::BeforeCommit) {
                return Err(crash(&key, CrashPoint::BeforeCommit));
            }
        }
    }
    Ok(written)
}

/// The `snapshot_revision` of the `task.json` in `dir`, if there is one.
fn revision_on_disk(dir: &Path) -> Option<i64> {
    let bytes = std::fs::read(dir.join("task.json")).ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .get("snapshot_revision")
        .and_then(Value::as_i64)
}

/// A task this commit writes, vetted and rendered before anything is.
struct Vetted {
    task_id: String,
    dir: PathBuf,
    revision: i64,
    /// Entries to publish as files, in sequence order.
    publishable: Vec<(i64, String, Vec<u8>)>,
    /// The `task.json` this commit writes.
    bytes: Vec<u8>,
    readable_before: i64,
}

/// Step 1 for one task, writing nothing: `None` when it owes nothing, a
/// refusal when the disk holds anything this database did not write.
fn vet_task(db: &Db, root: &Path, task_id: &str) -> Result<Option<Vetted>, Refusal> {
    let db_error = |error: rusqlite::Error| Refusal::new(format!("db error: {error}"), Vec::new());
    let Some(item) = db.get_pipeline_item(task_id).map_err(db_error)? else {
        // Removed in this transaction: its tombstone is the removal's.
        return Ok(None);
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
        return Ok(None);
    }
    let ahead = |why: String| {
        Refusal::new(
            format!("task {task_id}: the disk is ahead of the database ({why})"),
            vec![task_id.to_string()],
        )
    };
    if !is_repairing(task_id) && authority::is_diverged(db, task_id) {
        return Err(Refusal::new(
            format!("task {task_id}: the disk is ahead of the database; awaiting reconciliation from disk"),
            Vec::new(),
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

    // Every committed entry is written now, in sequence order, whatever
    // reservation another operation holds below it: an open reservation
    // never holds back another commit's durability. What consumers may read
    // in order is the separate watermark (`ledger.readable_through`), which
    // only ever covers files already synced: the commit point carries this
    // commit's entries in flight under the watermark as it stood, and the
    // watermark moves on only once their files are on disk.
    let publishable = filled
        .iter()
        .map(|(sequence, file_name, payload)| (*sequence, (*file_name).clone(), (*payload).clone()))
        .collect();
    let (bytes, readable_before) = render_task_json(db, task_id, &[])?;
    Ok(Some(Vetted {
        task_id: task_id.to_string(),
        dir,
        revision,
        publishable,
        bytes,
        readable_before,
    }))
}

/// `task.json` as the open transaction leaves `task_id`, with its
/// unpublished entries in flight; a refusal fences `fence`.
fn render_task_json(db: &Db, task_id: &str, fence: &[String]) -> Result<(Vec<u8>, i64), Refusal> {
    let db_error =
        |error: rusqlite::Error| Refusal::new(format!("db error: {error}"), fence.to_vec());
    let mut in_flight = Vec::new();
    for entry in db.pending_ledger_entries(task_id).map_err(db_error)? {
        let (Some(file_name), Some(payload)) = (entry.file_name, entry.payload) else {
            continue;
        };
        let text = String::from_utf8(payload).map_err(|error| {
            Refusal::new(
                format!(
                    "ledger entry {} of {task_id} is not UTF-8: {error}",
                    entry.sequence
                ),
                fence.to_vec(),
            )
        })?;
        in_flight
            .push(json!({ "sequence": entry.sequence, "file_name": file_name, "payload": text }));
    }
    let readable_through = db.ledger_readable_through(task_id).map_err(db_error)?;
    let mut facts = db
        .task_snapshot_facts(task_id)
        .map_err(db_error)?
        .unwrap_or(Value::Null);
    if let Some(ledger) = facts.get_mut("ledger").and_then(Value::as_object_mut) {
        ledger.insert("published_through".into(), json!(readable_through));
        ledger.insert(READABLE_THROUGH_KEY.into(), json!(readable_through));
        if !in_flight.is_empty() {
            ledger.insert(IN_FLIGHT_KEY.into(), Value::Array(in_flight));
        }
    }
    let mut bytes = serde_json::to_vec_pretty(&facts).map_err(|error| {
        Refusal::new(
            format!("render task.json of {task_id}: {error}"),
            fence.to_vec(),
        )
    })?;
    bytes.push(b'\n');
    Ok((bytes, readable_through))
}

/// Step 2: write every vetted task's `task.json`. If one cannot be written,
/// the ones written before it get their earlier bytes back, so the disk
/// holds all of the mutation or none of it, and every task is fenced to be
/// reconciled from what its disk then holds.
fn write_task_jsons(root: &Path, vetted: &[Vetted]) -> Result<(), Refusal> {
    let mut previous: Vec<(&Vetted, Option<Vec<u8>>)> = Vec::new();
    for task in vetted {
        if crashes_here(root, &task.task_id, CrashPoint::BeforeTaskJson) {
            return Err(crash(&task.task_id, CrashPoint::BeforeTaskJson));
        }
        let before = std::fs::read(task.dir.join("task.json")).ok();
        let replaced = if crashes_here(root, &task.task_id, CrashPoint::TaskJsonFails) {
            Err("injected task.json failure".to_string())
        } else {
            replace_atomically(&task.dir, "task.json", &task.bytes)
        };
        if let Err(error) = replaced {
            let mut message = format!("task {}: {error}", task.task_id);
            for (written, bytes) in previous.iter().rev() {
                let restored = match bytes {
                    Some(bytes) => replace_atomically(&written.dir, "task.json", bytes),
                    None => std::fs::remove_file(written.dir.join("task.json"))
                        .map_err(|error| error.to_string()),
                };
                if let Err(error) = restored {
                    message.push_str(&format!(
                        "; task {} keeps this commit's task.json ({error})",
                        written.task_id
                    ));
                }
            }
            return Err(Refusal::new(
                message,
                vetted.iter().map(|task| task.task_id.clone()).collect(),
            ));
        }
        previous.push((task, before));
    }
    Ok(())
}

/// Steps 3 and 4 for one task whose `task.json` is written.
fn publish_entries(db: &Db, root: &Path, task: &Vetted, written: &[String]) -> Result<(), Refusal> {
    let task_id = task.task_id.as_str();
    let db_error =
        |error: rusqlite::Error| Refusal::new(format!("db error: {error}"), written.to_vec());
    db.acknowledge_task_snapshot(task_id, task.revision)
        .map_err(db_error)?;
    if crashes_here(root, task_id, CrashPoint::AfterTaskJson) {
        return Err(crash(task_id, CrashPoint::AfterTaskJson));
    }
    let ledger = task.dir.join("ledger");
    for (sequence, file_name, payload) in &task.publishable {
        // Written and synced, file and directory, before it counts.
        let published = if crashes_here(root, task_id, CrashPoint::PublishFails(*sequence)) {
            Err("injected publication failure".to_string())
        } else {
            publish_durably(&ledger, file_name, payload)
        };
        if let Err(error) = published {
            // Durable in task.json already; published by a later flush.
            // The watermark stays below it until then.
            log::warn!("task {task_id}: ledger entry {sequence} stays in flight: {error}");
            db.record_ledger_publish_error(task_id, *sequence, &error)
                .map_err(db_error)?;
            break;
        }
        db.acknowledge_ledger_entry(task_id, *sequence)
            .map_err(db_error)?;
        if crashes_here(root, task_id, CrashPoint::AfterEntry(*sequence)) {
            return Err(crash(task_id, CrashPoint::AfterEntry(*sequence)));
        }
    }
    // The files are synced: the watermark may now cover them.
    if db.ledger_readable_through(task_id).map_err(db_error)? != task.readable_before {
        if crashes_here(root, task_id, CrashPoint::BeforeWatermark) {
            return Err(crash(task_id, CrashPoint::BeforeWatermark));
        }
        let (bytes, _) = render_task_json(db, task_id, written)?;
        replace_atomically(&task.dir, "task.json", &bytes)
            .map_err(|error| Refusal::new(format!("task {task_id}: {error}"), written.to_vec()))?;
    }
    if crashes_here(root, task_id, CrashPoint::BeforeCommit) {
        return Err(crash(task_id, CrashPoint::BeforeCommit));
    }
    Ok(())
}

/// `task.json` `ledger.readable_through` (T13d, `disk` mode only): the
/// contiguous watermark consumers read the ledger in order up to. Files past
/// it are durable, and readable once the reservations below them close.
pub const READABLE_THROUGH_KEY: &str = "readable_through";

/// `task.json` `ledger.in_flight`: entries a disk-first commit made durable
/// in `task.json` before (or without) publishing their files.
pub const IN_FLIGHT_KEY: &str = "in_flight";

#[cfg(test)]
static FAILING_WRITES: LazyLock<Mutex<BTreeSet<PathBuf>>> = LazyLock::new(Default::default);

/// Fail every entry-file write into `ledger` until [`restore_writes`], as a
/// full or read-only disk would.
#[cfg(test)]
pub(crate) fn fail_writes(ledger: &Path) {
    FAILING_WRITES.lock().unwrap().insert(ledger.to_path_buf());
}

#[cfg(test)]
pub(crate) fn restore_writes(ledger: &Path) {
    FAILING_WRITES.lock().unwrap().remove(ledger);
}

#[cfg(test)]
fn writes_fail(ledger: &Path) -> bool {
    FAILING_WRITES.lock().unwrap().contains(ledger)
}

#[cfg(not(test))]
fn writes_fail(_ledger: &Path) -> bool {
    false
}

/// Write an entry file so that it counts: written and synced with its
/// directory ([`publish_immutable`]), then read back and compared. Any
/// failure leaves the entry unpublished, held by `task.json`'s
/// `ledger.in_flight`, with the readable watermark below it (T13d).
pub(crate) fn publish_durably(ledger: &Path, file_name: &str, bytes: &[u8]) -> Result<(), String> {
    if writes_fail(ledger) {
        return Err(format!(
            "write {}: injected failure",
            ledger.join(file_name).display()
        ));
    }
    publish_immutable(ledger, file_name, bytes)?;
    let path = ledger.join(file_name);
    let read_back =
        std::fs::read(&path).map_err(|error| format!("read back {}: {error}", path.display()))?;
    if read_back != bytes {
        return Err(format!(
            "read back {}: other bytes than written",
            path.display()
        ));
    }
    Ok(())
}

/// Publish the files of in-flight entries a `task.json` carries and the
/// ledger does not hold yet (a crash between a disk-first commit point and
/// its files). Returns how many were written. It never publishes an entry
/// in the database: the scan read it as not durable, so it is projected
/// unpublished and the publisher acknowledges it only after writing (or
/// finding) its file, synced and read back.
pub(crate) fn materialize_in_flight(
    directory: &super::rebuild::TaskDirectory,
) -> Result<usize, String> {
    let ledger = directory.path.join("ledger");
    let mut written = 0;
    for (file_name, bytes) in &directory.snapshot.in_flight {
        if ledger.join(file_name).exists() {
            continue;
        }
        publish_durably(&ledger, file_name, bytes)?;
        written += 1;
    }
    Ok(written)
}
