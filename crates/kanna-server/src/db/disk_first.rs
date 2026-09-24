//! The database connection's commit gate for disk-first writes (spec
//! §16.11, T13d).
//!
//! In `disk` authority mode ([`crate::task_store::authority`]) the task
//! directory is the first durable write of every mutation: a transaction
//! that owes a disk record (a trigger or an enqueue touched the outbox —
//! `task_ledger_snapshot`, `task_ledger_entry`, `repo_disk_snapshot`,
//! `disk_record_removal`) writes those records from its own uncommitted rows
//! before SQLite commits it ([`crate::task_store::disk_first`]). SQLite is
//! derived in the same operation; when its commit fails, the disk holds the
//! mutation and the database is reconciled from it. In `sql` mode nothing
//! here changes what a commit does.
//!
//! Every write path reaches a commit through this gate:
//!
//! - [`crate::db::Db::with_immediate_transaction`] and
//!   [`DbConnection::unchecked_transaction`] publish before their `COMMIT`;
//! - [`DbConnection::execute`] runs an autocommit statement inside such a
//!   transaction in `disk` mode;
//! - any other commit that touched the outbox in `disk` mode (a raw
//!   `COMMIT`, a prepared statement in autocommit) is refused by the commit
//!   hook, so no path can write SQLite first by accident.

use super::Db;
use rusqlite::hooks::Action;
use rusqlite::{Connection, OptionalExtension, Params, TransactionBehavior};
use std::collections::BTreeSet;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// What a transaction touched that may owe a disk record: rowids of the
/// outbox tables, and tasks or repositories a publisher asked to write out.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Touched {
    pub snapshots: BTreeSet<i64>,
    pub entries: BTreeSet<i64>,
    pub repos: BTreeSet<i64>,
    pub removals: bool,
    pub forced_tasks: BTreeSet<String>,
    pub forced_repos: BTreeSet<String>,
}

impl Touched {
    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
            && self.entries.is_empty()
            && self.repos.is_empty()
            && !self.removals
            && self.forced_tasks.is_empty()
            && self.forced_repos.is_empty()
    }
}

#[derive(Debug, Default)]
struct Gate {
    touched: Mutex<Touched>,
    /// Set only while this connection's own publishing `COMMIT` runs.
    guarded: AtomicBool,
}

impl Gate {
    fn touched(&self) -> std::sync::MutexGuard<'_, Touched> {
        self.touched
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

/// Commits the gate refused, per database path.
static REFUSED: std::sync::LazyLock<Mutex<std::collections::HashMap<String, usize>>> =
    std::sync::LazyLock::new(Default::default);

/// How many commits on `db_path` wrote the outbox outside the gate in
/// `disk` mode and were refused.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn refused_commits(db_path: &str) -> usize {
    REFUSED
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(db_path)
        .copied()
        .unwrap_or(0)
}

#[cfg(test)]
static FAIL_COMMIT: std::sync::LazyLock<Mutex<BTreeSet<String>>> =
    std::sync::LazyLock::new(Default::default);

/// Fail the next disk-first COMMIT on `db_path` after its records are on
/// disk, as SQLite would on an I/O error. Fires once.
#[cfg(test)]
pub(crate) fn fail_next_commit(db_path: &str) {
    FAIL_COMMIT.lock().unwrap().insert(db_path.to_string());
}

#[cfg(test)]
fn fail_commit_here(db_path: &str) -> bool {
    FAIL_COMMIT.lock().unwrap().remove(db_path)
}

#[cfg(not(test))]
fn fail_commit_here(_db_path: &str) -> bool {
    false
}

/// Does this database run disk-first right now?
fn disk_first(path: &str) -> bool {
    crate::task_store::authority::mode_for_root(&crate::task_store::root_for_db(path))
        == crate::task_store::authority::Mode::Disk
}

/// Statements SQLite refuses inside a transaction, or that control one;
/// none of them writes a row.
fn runs_outside_transactions(sql: &str) -> bool {
    let keyword: String = sql
        .trim_start()
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_ascii_uppercase();
    matches!(
        keyword.as_str(),
        "VACUUM"
            | "PRAGMA"
            | "ATTACH"
            | "DETACH"
            | "BEGIN"
            | "COMMIT"
            | "END"
            | "ROLLBACK"
            | "SAVEPOINT"
            | "RELEASE"
    )
}

/// The SQLite connection of a [`Db`], with the commit gate installed.
pub struct DbConnection {
    inner: Connection,
    path: String,
    gate: Arc<Gate>,
}

impl std::fmt::Debug for DbConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DbConnection")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Deref for DbConnection {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.inner
    }
}

impl DbConnection {
    pub(super) fn new(inner: Connection, path: &str) -> Self {
        let gate = Arc::new(Gate::default());
        let recorder = Arc::clone(&gate);
        inner.update_hook(Some(
            move |action: Action, _database: &str, table: &str, rowid: i64| {
                let mut touched = recorder.touched();
                match (table, action) {
                    ("task_ledger_snapshot", _) => {
                        touched.snapshots.insert(rowid);
                    }
                    ("task_ledger_entry", Action::SQLITE_DELETE) => {}
                    ("task_ledger_entry", _) => {
                        touched.entries.insert(rowid);
                    }
                    ("repo_disk_snapshot", _) => {
                        touched.repos.insert(rowid);
                    }
                    ("disk_record_removal", Action::SQLITE_INSERT) => touched.removals = true,
                    _ => {}
                }
            },
        ));
        let committer = Arc::clone(&gate);
        let committed_path = path.to_string();
        inner.commit_hook(Some(move || {
            let touched = std::mem::take(&mut *committer.touched());
            if touched.is_empty() || committer.guarded.load(Ordering::SeqCst) {
                return false;
            }
            if disk_first(&committed_path) {
                log::error!(
                    "refused a commit that wrote the task-store outbox outside the disk-first gate ({touched:?})"
                );
                *REFUSED
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .entry(committed_path.clone())
                    .or_default() += 1;
                return true;
            }
            false
        }));
        let rolled_back = Arc::clone(&gate);
        inner.rollback_hook(Some(move || {
            *rolled_back.touched() = Touched::default();
        }));
        Self {
            inner,
            path: path.to_string(),
            gate,
        }
    }

    /// The database file this connection opened, as the caller named it.
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    /// Whether commits on this connection write the disk first.
    pub(crate) fn is_disk_first(&self) -> bool {
        disk_first(&self.path)
    }

    fn as_db(&self) -> &Db {
        // SAFETY: `Db` is `#[repr(transparent)]` over its only field, a
        // `DbConnection`, so the two have the same layout.
        unsafe { &*(self as *const DbConnection as *const Db) }
    }

    /// `Connection::execute`, except that in `disk` mode an autocommit
    /// statement runs inside a publishing transaction.
    pub fn execute<P: Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        if self.inner.is_autocommit() && !runs_outside_transactions(sql) && self.is_disk_first() {
            return self
                .as_db()
                .with_immediate_transaction(|db| db.conn.inner.execute(sql, params));
        }
        self.inner.execute(sql, params)
    }

    /// A deferred transaction whose commit goes through the gate.
    pub fn unchecked_transaction(&self) -> rusqlite::Result<DbTransaction<'_>> {
        self.transaction_with(TransactionBehavior::Deferred)
    }

    pub fn transaction_with(
        &self,
        behavior: TransactionBehavior,
    ) -> rusqlite::Result<DbTransaction<'_>> {
        self.inner.execute_batch(match behavior {
            TransactionBehavior::Deferred => "BEGIN DEFERRED",
            TransactionBehavior::Immediate => "BEGIN IMMEDIATE",
            TransactionBehavior::Exclusive => "BEGIN EXCLUSIVE",
            _ => "BEGIN",
        })?;
        Ok(DbTransaction {
            conn: self,
            open: true,
        })
    }

    /// Take what the open transaction touched, leaving nothing recorded.
    pub(crate) fn take_touched(&self) -> Touched {
        std::mem::take(&mut *self.gate.touched())
    }

    pub(crate) fn clear_touched(&self) {
        *self.gate.touched() = Touched::default();
    }

    /// Owe a publication of `task` at the open transaction's commit.
    pub(crate) fn force_task(&self, task_id: &str) {
        self.gate.touched().forced_tasks.insert(task_id.to_string());
    }

    pub(crate) fn force_repo(&self, repo_id: &str) {
        self.gate.touched().forced_repos.insert(repo_id.to_string());
    }

    pub(crate) fn force_removals(&self) {
        self.gate.touched().removals = true;
    }

    /// Publish what the open transaction owes (in `disk` mode), then commit
    /// it. On any failure the transaction is rolled back; a task whose disk
    /// record was already written is then fenced, because the disk holds a
    /// mutation the database does not.
    pub(crate) fn publish_and_commit(&self) -> rusqlite::Result<()> {
        let touched = self.take_touched();
        let mut written = Vec::new();
        if !touched.is_empty() && self.is_disk_first() {
            match crate::task_store::disk_first::publish_before_commit(self.as_db(), touched) {
                Ok(tasks) => written = tasks,
                Err(refusal) => {
                    let _ = self.inner.execute_batch("ROLLBACK");
                    crate::task_store::disk_first::after_rollback(self.as_db(), &refusal);
                    return Err(rusqlite::Error::InvalidParameterName(refusal.message));
                }
            }
        }
        // The publication's own acknowledgements owe nothing.
        self.clear_touched();
        self.gate.guarded.store(true, Ordering::SeqCst);
        let committed = if fail_commit_here(&self.path) && !written.is_empty() {
            Err(rusqlite::Error::InvalidParameterName(
                "injected COMMIT failure".into(),
            ))
        } else {
            self.inner.execute_batch("COMMIT")
        };
        self.gate.guarded.store(false, Ordering::SeqCst);
        if let Err(error) = committed {
            let _ = self.inner.execute_batch("ROLLBACK");
            if !written.is_empty() {
                crate::task_store::disk_first::after_rollback(
                    self.as_db(),
                    &crate::task_store::disk_first::Refusal::commit_failed(written, &error),
                );
            }
            return Err(error);
        }
        Ok(())
    }
}

/// A transaction on a [`DbConnection`]; rolled back on drop unless
/// committed.
pub struct DbTransaction<'a> {
    conn: &'a DbConnection,
    open: bool,
}

impl Deref for DbTransaction<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.conn.inner
    }
}

impl DbTransaction<'_> {
    pub fn commit(mut self) -> rusqlite::Result<()> {
        self.open = false;
        self.conn.publish_and_commit()
    }
}

impl Drop for DbTransaction<'_> {
    fn drop(&mut self) {
        if self.open {
            let _ = self.conn.inner.execute_batch("ROLLBACK");
        }
    }
}

impl Db {
    /// The database path this handle opened.
    pub(crate) fn db_path(&self) -> &str {
        self.conn.path()
    }

    /// Owe the open transaction's commit a publication of `task_id`'s
    /// records (the publisher, in `disk` mode).
    pub(crate) fn owe_task_publication(&self, task_id: &str) {
        self.conn.force_task(task_id);
    }

    pub(crate) fn owe_repo_publication(&self, repo_id: &str) {
        self.conn.force_repo(repo_id);
    }

    pub(crate) fn owe_removal_publication(&self) {
        self.conn.force_removals();
    }

    /// The tasks whose snapshot or ledger rows these are.
    pub(crate) fn task_ids_for_outbox_rows(
        &self,
        snapshots: &BTreeSet<i64>,
        entries: &BTreeSet<i64>,
    ) -> rusqlite::Result<BTreeSet<String>> {
        let mut tasks = BTreeSet::new();
        for (table, rowids) in [
            ("task_ledger_snapshot", snapshots),
            ("task_ledger_entry", entries),
        ] {
            let mut statement = self
                .conn
                .prepare_cached(&format!("SELECT task_id FROM {table} WHERE rowid = ?"))?;
            for rowid in rowids {
                if let Some(task) = statement
                    .query_row([rowid], |row| row.get::<_, String>(0))
                    .optional()?
                {
                    tasks.insert(task);
                }
            }
        }
        Ok(tasks)
    }

    pub(crate) fn repo_ids_for_outbox_rows(
        &self,
        repos: &BTreeSet<i64>,
    ) -> rusqlite::Result<BTreeSet<String>> {
        let mut statement = self
            .conn
            .prepare_cached("SELECT repo_id FROM repo_disk_snapshot WHERE rowid = ?")?;
        let mut ids = BTreeSet::new();
        for rowid in repos {
            if let Some(repo) = statement
                .query_row([rowid], |row| row.get::<_, String>(0))
                .optional()?
            {
                ids.insert(repo);
            }
        }
        Ok(ids)
    }
}
