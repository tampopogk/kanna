//! SQLite half of disk authority (spec §11, §16.11 — T13 third increment).
//! [`crate::task_store::authority`] decides which tasks the database must
//! take from disk; this reads the facts it compares and writes a projection
//! over the rows of those tasks only.
//!
//! Reconciling a task makes its rows exactly what its directory projects:
//! rows the directory does not hold are removed, rows it holds are written
//! (updated in place when the row exists, keeping its rowid), and the
//! ledger rows become the directory's files, published. Nothing is
//! executed: owed work is restored as rows, and restart reconciliation
//! resumes it as it would after a crash. Statistics and transient rows of
//! the task are left alone, except where removing a run the directory does
//! not know cascades to them.

use super::disk_rebuild::{
    insert_row, primary_key, upsert_budget, upsert_input, upsert_published_ledger_row,
    upsert_stage_run,
};
use super::task_state::{json_to_sql, CARRIED_TABLES};
use super::Db;
use crate::task_store::rebuild::{CarriedRow, Projection, RepoRecord};
use rusqlite::{params, OptionalExtension};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// One `task_ledger_entry` row, as the authority check compares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SqlLedgerRow {
    pub sequence: i64,
    /// `None` for a sequence only reserved.
    pub kind: Option<String>,
    pub payload: Option<Vec<u8>>,
    pub published: bool,
}

/// A task as the database holds it, for the authority check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SqlTask {
    pub id: String,
    pub repo_id: String,
    pub closed: bool,
}

/// What reconciling tasks from disk changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ReconcileChanges {
    pub rows_written: usize,
    pub rows_removed: usize,
    /// Ledger entries the database had committed that the directory does
    /// not hold (or holds with other bytes): `(task, sequence)`. Disk is the
    /// record; they are dropped.
    pub discarded_entries: Vec<(String, i64)>,
    /// Inputs recovered under a new row id because another task's input
    /// holds the one their ledger entry names (a restored older database
    /// handed the id out again): `(task, ledger's id, new id)`.
    pub renumbered_inputs: Vec<(String, i64, i64)>,
    /// Carried rows written under a new rowid because another task's row
    /// holds theirs: `(task, table, disk's rowid)`.
    pub relocated_rows: Vec<(String, String, i64)>,
}

/// A reconciliation refused because a target's rows changed after they were
/// compared with its directory: a writer outside the caller's lease
/// committed in between, and the projection no longer describes what it
/// would overwrite.
pub(crate) const CHANGED_SINCE_COMPARED: &str = "changed since it was compared with disk";

/// Tables whose counter never goes down: a sequence or branch number the
/// database handed out stays spent even when disk never recorded it.
const HIGH_WATER_COLUMNS: &[(&str, &str)] = &[
    ("task_ledger_sequence", "high_water"),
    ("task_branch_counter", "last_allocated"),
];

/// Migration `104_disk_divergence`.
pub(super) const DIVERGENCE_SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS disk_divergence (
        -- A task whose disk the publisher found ahead of this database: it
        -- publishes nothing until a repair from disk deletes the row.
        task_id TEXT PRIMARY KEY REFERENCES pipeline_item(id) ON DELETE CASCADE,
        reason TEXT NOT NULL,
        detected_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    );
"#;

/// Schema-only fixtures that predate migration 104.
fn is_missing_divergence_table(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::SqliteFailure(_, Some(message))
        if message.contains("no such table: disk_divergence"))
}

fn row_object(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

fn rowid_of(row: &Map<String, Value>) -> Option<i64> {
    row.get("rowid").and_then(Value::as_i64)
}

/// The live row `wanted` is: the one under its rowid, else the one with its
/// primary key.
fn matching<'a>(
    wanted: &Map<String, Value>,
    live: &'a [Map<String, Value>],
    key: &[String],
) -> Option<&'a Map<String, Value>> {
    if let Some(rowid) = rowid_of(wanted) {
        if let Some(row) = live.iter().find(|row| rowid_of(row) == Some(rowid)) {
            return Some(row);
        }
    }
    if key.is_empty() || !key.iter().all(|column| wanted.contains_key(column)) {
        return None;
    }
    live.iter().find(|row| {
        key.iter()
            .all(|column| row.get(column) == wanted.get(column))
    })
}

/// The column that is the table's rowid (a lone `INTEGER PRIMARY KEY`).
fn rowid_alias(db: &Db, table: &str) -> Result<Option<String>, rusqlite::Error> {
    let mut statement = db
        .conn
        .prepare("SELECT name, type FROM pragma_table_info(?) WHERE pk > 0")?;
    let key: Vec<(String, String)> = statement
        .query_map([table], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    Ok(match key.as_slice() {
        [(name, kind)] if kind.eq_ignore_ascii_case("INTEGER") => Some(name.clone()),
        _ => None,
    })
}

fn update_row(
    db: &Db,
    table: &str,
    rowid: i64,
    columns: &Map<String, Value>,
) -> Result<(), rusqlite::Error> {
    let mut assignments = Vec::new();
    let mut values = Vec::new();
    for (column, value) in columns {
        assignments.push(format!("\"{column}\" = ?"));
        values.push(json_to_sql(value).map_err(|error| {
            rusqlite::Error::InvalidParameterName(format!("{table}.{column}: {error}"))
        })?);
    }
    if assignments.is_empty() {
        return Ok(());
    }
    values.push(rusqlite::types::Value::Integer(rowid));
    db.conn.execute(
        &format!(
            "UPDATE \"{table}\" SET {} WHERE rowid = ?",
            assignments.join(", ")
        ),
        rusqlite::params_from_iter(values),
    )?;
    Ok(())
}

impl Db {
    /// A transfer's claim on a task's workflow is active until its transfer
    /// ends: removing it would let competing workflow work in.
    fn transfer_claim_is_active(
        &self,
        claim: &Map<String, Value>,
    ) -> Result<bool, rusqlite::Error> {
        let task = claim
            .get("pipeline_item_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let transfer = claim.get("transfer_id").and_then(Value::as_str);
        // T9's own rule (the one claiming and plan completion use): the
        // transfer is live, or still has finalization or commit work pending
        // or running, whatever its display status says.
        Ok(self.task_workflow_is_claimed_by_transfer(task)?.as_deref() == transfer)
    }

    /// The publisher found the disk ahead of the database for this task:
    /// nothing of it is published until a repair from disk clears this.
    /// Durable, so a restart between the finding and the repair keeps it.
    pub(crate) fn flag_disk_divergence(
        &self,
        task_id: &str,
        why: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO disk_divergence (task_id, reason) SELECT id, ?2 FROM pipeline_item WHERE id = ?1
             ON CONFLICT(task_id) DO UPDATE SET reason = excluded.reason",
            params![task_id, why],
        )?;
        Ok(())
    }

    pub(crate) fn is_disk_divergent(&self, task_id: &str) -> Result<bool, rusqlite::Error> {
        match self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM disk_divergence WHERE task_id = ?)",
            [task_id],
            |row| row.get(0),
        ) {
            Err(error) if is_missing_divergence_table(&error) => Ok(false),
            other => other,
        }
    }

    pub(crate) fn disk_divergent_task_ids(&self) -> Result<Vec<String>, rusqlite::Error> {
        let statement = self
            .conn
            .prepare("SELECT task_id FROM disk_divergence ORDER BY task_id");
        let mut statement = match statement {
            Err(error) if is_missing_divergence_table(&error) => return Ok(Vec::new()),
            other => other?,
        };
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    pub(crate) fn clear_disk_divergence(&self, task_id: &str) -> Result<(), rusqlite::Error> {
        match self
            .conn
            .execute("DELETE FROM disk_divergence WHERE task_id = ?", [task_id])
        {
            Err(error) if is_missing_divergence_table(&error) => Ok(()),
            other => other.map(|_| ()),
        }
    }

    /// Now, as the ledger writes times.
    pub(crate) fn current_utc_timestamp(&self) -> Result<String, rusqlite::Error> {
        self.conn
            .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |row| {
                row.get(0)
            })
    }

    pub(crate) fn sql_tasks_for_authority(&self) -> Result<Vec<SqlTask>, rusqlite::Error> {
        let mut statement = self
            .conn
            .prepare("SELECT id, repo_id, closed_at IS NOT NULL FROM pipeline_item ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok(SqlTask {
                id: row.get(0)?,
                repo_id: row.get(1)?,
                closed: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    pub(crate) fn sql_repo_ids(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut statement = self.conn.prepare("SELECT id FROM repo ORDER BY id")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// Run and join ids the database holds, which a projection of some of
    /// its tasks may name.
    pub(crate) fn sql_run_and_join_ids(
        &self,
    ) -> Result<(BTreeSet<String>, BTreeSet<String>), rusqlite::Error> {
        let ids = |sql: &str| -> Result<BTreeSet<String>, rusqlite::Error> {
            let mut statement = self.conn.prepare(sql)?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect()
        };
        Ok((
            ids("SELECT id FROM stage_run")?,
            ids("SELECT id FROM task_join")?,
        ))
    }

    pub(crate) fn ledger_rows_for_authority(
        &self,
        task_id: &str,
    ) -> Result<Vec<SqlLedgerRow>, rusqlite::Error> {
        let mut statement = self.conn.prepare(
            "SELECT sequence, kind, payload, published_at IS NOT NULL FROM task_ledger_entry
             WHERE task_id = ? ORDER BY sequence",
        )?;
        let rows = statement.query_map([task_id], |row| {
            Ok(SqlLedgerRow {
                sequence: row.get(0)?,
                kind: row.get(1)?,
                payload: row.get(2)?,
                published: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// An operation holds a reservation on the task: it is mid-flight.
    pub(crate) fn has_ledger_reservation(&self, task_id: &str) -> Result<bool, rusqlite::Error> {
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_ledger_entry WHERE task_id = ? AND kind IS NULL)",
            [task_id],
            |row| row.get(0),
        )
    }

    /// Before switching to disk authority, owe a record for everything the
    /// disk must hold: a `task.json` for every task never published (closed
    /// tasks that predate migration 103 have none), and every `repo.json`,
    /// so each is rewritten naming this installation. Returns how many
    /// tasks were owed.
    pub(crate) fn owe_every_disk_record(&self) -> Result<usize, rusqlite::Error> {
        self.with_immediate_transaction(|db| {
            db.conn.execute(
                "INSERT OR IGNORE INTO task_ledger_snapshot (task_id, revision, published_revision)
                 SELECT id, 0, 0 FROM pipeline_item WHERE true",
                [],
            )?;
            let tasks = db.conn.execute(
                "UPDATE task_ledger_snapshot SET revision = revision + 1
                 WHERE published_revision = 0",
                [],
            )?;
            db.conn.execute(
                "INSERT INTO repo_disk_snapshot (repo_id) SELECT id FROM repo WHERE true
                 ON CONFLICT(repo_id) DO UPDATE SET revision = revision + 1",
                [],
            )?;
            Ok(tasks)
        })
    }

    /// Owe `repo.json` again (it does not name this installation).
    pub(crate) fn owe_repo_disk_record(&self, repo_id: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO repo_disk_snapshot (repo_id) SELECT id FROM repo WHERE id = ?1
             ON CONFLICT(repo_id) DO UPDATE SET revision = revision + 1",
            [repo_id],
        )?;
        Ok(())
    }

    /// The disk lost records this database published (a restored or
    /// damaged store): owe them again. Ledger rows are republished from
    /// their stored bytes, and `task.json` is rewritten.
    pub(crate) fn reowe_disk_publication(
        &self,
        task_id: &str,
        missing: &[i64],
    ) -> Result<(), rusqlite::Error> {
        self.with_immediate_transaction(|db| {
            for sequence in missing {
                db.conn.execute(
                    "UPDATE task_ledger_entry SET published_at = NULL, publish_error = NULL
                     WHERE task_id = ? AND sequence = ? AND kind IS NOT NULL",
                    params![task_id, sequence],
                )?;
            }
            db.mark_task_snapshot_dirty(task_id)
        })
    }

    /// Raise the snapshot revision to one the disk already holds, so the
    /// next `task.json` this database writes is newer than it. One that was
    /// owed stays owed.
    pub(crate) fn raise_task_snapshot_revision(
        &self,
        task_id: &str,
        disk_revision: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE task_ledger_snapshot
             SET revision = CASE WHEN revision > published_revision
                                 THEN MAX(revision, ?2) + 1 ELSE MAX(revision, ?2) END,
                 published_revision = MAX(published_revision, ?2)
             WHERE task_id = ?1",
            params![task_id, disk_revision],
        )?;
        Ok(())
    }

    /// A repository registration from `repo.json`: inserted when the
    /// database lacks it, updated when `repo.json` is newer than what this
    /// database published and differs from it. Returns whether it wrote.
    pub(crate) fn sync_repo_from_disk(&self, record: &RepoRecord) -> Result<bool, rusqlite::Error> {
        self.with_immediate_transaction(|db| {
            let present: bool = db.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM repo WHERE id = ?)",
                [&record.repo_id],
                |row| row.get(0),
            )?;
            let (revision, _) = db.repo_disk_revisions(&record.repo_id)?.unwrap_or((0, 0));
            let wrote = if !present {
                insert_row(db, "repo", &record.registration)?;
                true
            } else if record.snapshot_revision > revision {
                let live = db
                    .repo_disk_record(&record.repo_id)?
                    .and_then(|live| live.get("registration").cloned())
                    .map(|live| row_object(&live))
                    .unwrap_or_default();
                let mut changed = record.registration.clone();
                changed.retain(|column, value| {
                    column != "id" && column != "last_opened_at" && live.get(column) != Some(value)
                });
                let rowid: i64 = db.conn.query_row(
                    "SELECT rowid FROM repo WHERE id = ?",
                    [&record.repo_id],
                    |row| row.get(0),
                )?;
                update_row(db, "repo", rowid, &changed)?;
                !changed.is_empty()
            } else {
                false
            };
            if !wrote {
                return Ok(false);
            }
            let hash = record
                .registration
                .get("remote_url_hash")
                .and_then(Value::as_str);
            if let (Some(hash), Some(order)) = (hash, record.sidebar_order) {
                db.conn.execute(
                    "INSERT INTO repo_sidebar_order (remote_url_hash, sort_order) VALUES (?, ?)
                     ON CONFLICT(remote_url_hash) DO UPDATE SET sort_order = excluded.sort_order",
                    params![hash, order],
                )?;
            }
            // What disk holds is current; this database's next write owes
            // a newer record.
            db.conn.execute(
                "INSERT INTO repo_disk_snapshot (repo_id, revision, published_revision)
                 VALUES (?1, ?2, ?2)
                 ON CONFLICT(repo_id) DO UPDATE SET
                    revision = MAX(revision, excluded.revision),
                    published_revision = excluded.published_revision,
                    publish_error = NULL",
                params![record.repo_id, record.snapshot_revision.max(1)],
            )?;
            Ok(true)
        })
    }

    /// Make the rows of `targets` exactly what `projection` holds for them,
    /// in one transaction. `disk_revisions` is each target's `task.json`
    /// revision: afterwards the database owes a `task.json` newer than it,
    /// written from the reconciled rows.
    pub(crate) fn reconcile_tasks_from_projection(
        &self,
        targets: &BTreeSet<String>,
        projection: &Projection,
        disk_revisions: &BTreeMap<String, i64>,
        compared_at: &BTreeMap<String, Option<i64>>,
    ) -> Result<ReconcileChanges, rusqlite::Error> {
        self.with_immediate_transaction(|db| {
            // Every change to a carried row bumps the task's snapshot
            // revision in the changing statement, so an unchanged revision
            // means unchanged rows. `None`: the task did not exist.
            for task in targets {
                let now: Option<i64> = db
                    .conn
                    .query_row(
                        "SELECT snapshot.revision FROM task_ledger_snapshot snapshot
                         JOIN pipeline_item item ON item.id = snapshot.task_id
                         WHERE snapshot.task_id = ?",
                        [task],
                        |row| row.get(0),
                    )
                    .optional()?;
                let exists = db.get_pipeline_item(task)?.is_some();
                let expected = compared_at.get(task).copied().flatten();
                if now != expected || (expected.is_none() && exists) {
                    return Err(rusqlite::Error::InvalidParameterName(format!(
                        "task {task} {CHANGED_SINCE_COMPARED} (revision {expected:?}, now {now:?})"
                    )));
                }
            }
            let input_ids = db.plan_input_ids(targets, projection)?;
            let moved = with_moved_inputs(projection, &input_ids);
            let projection = &moved;
            let mut changes = ReconcileChanges::default();
            let wanted = |table: &str, task: &str| -> Vec<&CarriedRow> {
                projection
                    .carried
                    .iter()
                    .filter(|carried| carried.table == table && carried.task_id == task)
                    .collect()
            };
            // Runs and budgets the projection derived from newer ledger
            // entries rather than carried rows: kept, then upserted below.
            let derived_runs: BTreeSet<&str> = projection
                .stage_runs
                .iter()
                .filter(|run| targets.contains(&run.task_id))
                .map(|run| run.id.as_str())
                .collect();
            let derived_budgets: BTreeSet<(&str, &str)> = projection
                .budgets
                .iter()
                .filter(|budget| targets.contains(&budget.task_id))
                .map(|budget| (budget.task_id.as_str(), budget.stage.as_str()))
                .collect();
            let derived = |table: &str, task: &str, row: &Map<String, Value>| {
                let text = |column: &str| row.get(column).and_then(Value::as_str).unwrap_or("");
                match table {
                    "stage_run" => derived_runs.contains(text("id")),
                    "task_stage_budget" => derived_budgets.contains(&(task, text("stage"))),
                    _ => false,
                }
            };
            let live_rows = |table: &super::task_state::CarriedTable,
                             task: &str|
             -> Result<Vec<Map<String, Value>>, rusqlite::Error> {
                Ok(db
                    .carried_rows(table, task)?
                    .iter()
                    .map(row_object)
                    .collect())
            };

            // The task rows first: every other carried row names one.
            for task in targets {
                for carried in wanted("pipeline_item", task) {
                    let rowid: Option<i64> = db
                        .conn
                        .query_row("SELECT rowid FROM pipeline_item WHERE id = ?", [task], |row| {
                            row.get(0)
                        })
                        .optional()?;
                    match rowid {
                        Some(rowid) => {
                            let live = live_rows(&CARRIED_TABLES[0], task)?;
                            let mut changed = carried.row.clone();
                            changed.remove("rowid");
                            changed.retain(|column, value| {
                                live.first().and_then(|row| row.get(column)) != Some(value)
                            });
                            if !changed.is_empty() {
                                update_row(db, "pipeline_item", rowid, &changed)?;
                                changes.rows_written += 1;
                            }
                        }
                        None => {
                            insert_row(db, "pipeline_item", &carried.row)?;
                            changes.rows_written += 1;
                        }
                    }
                }
            }
            // Rows the directory does not hold, children before parents.
            for table in CARRIED_TABLES.iter().skip(1).rev() {
                let key = primary_key(db, table.table)?;
                for task in targets {
                    let wanted: Vec<Map<String, Value>> = wanted(table.table, task)
                        .into_iter()
                        .map(|carried| carried.row.clone())
                        .collect();
                    for live in live_rows(table, task)? {
                        let kept = wanted
                            .iter()
                            .any(|row| matching(row, std::slice::from_ref(&live), &key).is_some())
                            || derived(table.table, task, &live)
                            // A counter never goes down: before a task's
                            // first reservation disk holds no row for it.
                            || HIGH_WATER_COLUMNS.iter().any(|(name, _)| *name == table.table)
                            || (table.table == "task_transfer_workflow_claim"
                                && db.transfer_claim_is_active(&live)?)
                            // The transfer holding it stays with it.
                            || (table.table == "task_transfer"
                                && db.transfer_still_owns_its_source(
                                    live.get("id").and_then(Value::as_str).unwrap_or(""),
                                )?);
                        if kept {
                            continue;
                        }
                        if let Some(rowid) = rowid_of(&live) {
                            db.conn.execute(
                                &format!("DELETE FROM \"{}\" WHERE rowid = ?", table.table),
                                [rowid],
                            )?;
                            changes.rows_removed += 1;
                        }
                    }
                }
            }
            // Rows the directory holds, parents before children, re-read
            // after the removals (a removed run takes its prompt with it).
            for table in CARRIED_TABLES.iter().skip(1) {
                let key = primary_key(db, table.table)?;
                let high_water = HIGH_WATER_COLUMNS
                    .iter()
                    .find(|(name, _)| *name == table.table)
                    .map(|(_, column)| *column);
                for task in targets {
                    let live = live_rows(table, task)?;
                    for carried in wanted(table.table, task) {
                        let mut row = carried.row.clone();
                        match matching(&row, &live, &key) {
                            Some(existing) => {
                                if let Some(column) = high_water {
                                    let live_mark = existing.get(column).and_then(Value::as_i64);
                                    let disk_mark = row.get(column).and_then(Value::as_i64);
                                    if let (Some(live_mark), Some(disk_mark)) = (live_mark, disk_mark)
                                    {
                                        row.insert(column.into(), live_mark.max(disk_mark).into());
                                    }
                                }
                                let rowid = rowid_of(existing).unwrap_or_default();
                                let mut changed = row.clone();
                                changed.retain(|column, value| existing.get(column) != Some(value));
                                if !changed.is_empty() {
                                    update_row(db, table.table, rowid, &changed)?;
                                    changes.rows_written += 1;
                                }
                            }
                            None => {
                                // A restored older database may have handed
                                // this rowid to another task's row: that row
                                // stays, and this one takes a new rowid.
                                let taken = match rowid_of(&row) {
                                    Some(rowid) => db
                                        .conn
                                        .query_row(
                                            &format!(
                                                "SELECT EXISTS(SELECT 1 FROM \"{}\" WHERE rowid = ?)",
                                                table.table
                                            ),
                                            [rowid],
                                            |row| row.get::<_, bool>(0),
                                        )?
                                        .then_some(rowid),
                                    None => None,
                                };
                                if let Some(rowid) = taken {
                                    row.remove("rowid");
                                    if let Some(alias) = rowid_alias(db, table.table)? {
                                        row.remove(&alias);
                                    }
                                    changes
                                        .relocated_rows
                                        .push((task.clone(), table.table.to_string(), rowid));
                                }
                                insert_row(db, table.table, &row)?;
                                changes.rows_written += 1;
                            }
                        }
                    }
                }
            }
            // What the directory holds outside `state`.
            for run in projection
                .stage_runs
                .iter()
                .filter(|run| targets.contains(&run.task_id))
            {
                upsert_stage_run(db, run)?;
                changes.rows_written += 1;
            }
            for budget in projection
                .budgets
                .iter()
                .filter(|budget| targets.contains(&budget.task_id))
            {
                upsert_budget(db, budget)?;
                changes.rows_written += 1;
            }
            for task in targets {
                let blockers: BTreeSet<&str> = projection
                    .blockers
                    .iter()
                    .filter(|(blocked, _)| blocked == task)
                    .map(|(_, blocker)| blocker.as_str())
                    .collect();
                for live in db.list_task_blocker_ids(task)? {
                    if !blockers.contains(live.as_str()) {
                        db.conn.execute(
                            "DELETE FROM task_blocker WHERE blocked_item_id = ? AND blocker_item_id = ?",
                            params![task, live],
                        )?;
                        changes.rows_removed += 1;
                    }
                }
                for blocker in blockers {
                    changes.rows_written += db.conn.execute(
                        "INSERT OR IGNORE INTO task_blocker (blocked_item_id, blocker_item_id)
                         SELECT ?1, id FROM pipeline_item WHERE id = ?2",
                        params![task, blocker],
                    )?;
                }

                // The ledger: the directory's files, published. A row with
                // the same bytes keeps its place (and, if its publication
                // was never acknowledged, releases what it held now); any
                // other row the database committed is dropped.
                let disk: BTreeMap<i64, &crate::task_store::rebuild::LedgerRow> = projection
                    .ledger
                    .iter()
                    .filter(|entry| &entry.task_id == task)
                    .map(|entry| (entry.sequence, entry))
                    .collect();
                let mut same = BTreeSet::new();
                for live in db.ledger_rows_for_authority(task)? {
                    let on_disk = disk.get(&live.sequence);
                    if live.kind.is_some()
                        && on_disk.is_some_and(|entry| live.payload.as_deref() == Some(&entry.payload[..]))
                    {
                        same.insert(live.sequence);
                        if !live.published {
                            db.acknowledge_ledger_entry(task, live.sequence)?;
                        }
                        continue;
                    }
                    if live.kind.is_some() {
                        changes.discarded_entries.push((task.clone(), live.sequence));
                    }
                    db.conn.execute(
                        "DELETE FROM task_ledger_entry WHERE task_id = ? AND sequence = ?",
                        params![task, live.sequence],
                    )?;
                    changes.rows_removed += 1;
                }
                for (sequence, entry) in &disk {
                    if !same.contains(sequence) {
                        upsert_published_ledger_row(db, entry)?;
                        changes.rows_written += 1;
                    }
                }
                // Current as the directory holds it; the rows are now at
                // least as new, so a newer task.json is owed from them.
                let disk_revision = disk_revisions.get(task).copied().unwrap_or(0);
                db.conn.execute(
                    "UPDATE task_ledger_snapshot
                     SET revision = MAX(revision, ?2) + 1, published_revision = ?2,
                         publish_error = NULL
                     WHERE task_id = ?1",
                    params![task, disk_revision],
                )?;
                if let Some(marker) = projection.markers.iter().find(|marker| &marker.task_id == task)
                {
                    db.conn.execute(
                        "INSERT OR IGNORE INTO task_ledger_backfill
                            (task_id, imported_entries, completed_at)
                         VALUES (?, ?, ?)",
                        params![marker.task_id, marker.historical_entries, marker.marked_at],
                    )?;
                }
            }
            // Inputs at their planned ids: every target's rows the plan does
            // not keep are removed first (never another task's), then the
            // planned rows are written.
            let planned: Vec<crate::task_store::rebuild::InputRow> = projection
                .inputs
                .iter()
                .filter(|input| targets.contains(&input.task_id))
                .map(|input| {
                    let mut input = input.clone();
                    if let Some(id) = input_ids.get(&(input.task_id.clone(), input.id)) {
                        input.id = *id;
                    }
                    input
                })
                .collect();
            for task in targets {
                let keep: BTreeSet<i64> = planned
                    .iter()
                    .filter(|input| &input.task_id == task)
                    .map(|input| input.id)
                    .collect();
                let live: Vec<i64> = {
                    let mut statement =
                        db.conn.prepare("SELECT id FROM task_input WHERE task_id = ?")?;
                    let rows = statement.query_map([task], |row| row.get(0))?;
                    rows.collect::<Result<_, _>>()?
                };
                for id in live.into_iter().filter(|id| !keep.contains(id)) {
                    db.conn.execute(
                        "DELETE FROM task_input WHERE id = ? AND task_id = ?",
                        params![id, task],
                    )?;
                    changes.rows_removed += 1;
                }
            }
            for input in &planned {
                upsert_input(db, input)?;
            }
            for ((task, from), to) in &input_ids {
                changes.renumbered_inputs.push((task.clone(), *from, *to));
            }
            // Repaired: the fence comes down with the repair, atomically.
            for task in targets {
                db.clear_disk_divergence(task)?;
            }
            Ok(changes)
        })
    }

    /// The id every target's input takes. An input keeps the id its ledger
    /// entry names unless a row this repair must not touch holds it: another
    /// task's input (a restored older database handed the id out again), or
    /// another target's input at the same id. Such an input takes the id an
    /// earlier repair gave it (its own row with the same content), else a
    /// new one past every id held, projected or ever allocated. Every id is
    /// reserved before any is written, so no planned input lands on another.
    /// Returns only the inputs that move: `(task, ledger's id) -> id`.
    fn plan_input_ids(
        &self,
        targets: &BTreeSet<String>,
        projection: &Projection,
    ) -> Result<BTreeMap<(String, i64), i64>, rusqlite::Error> {
        let live: Vec<(i64, String)> = {
            let mut statement = self.conn.prepare("SELECT id, task_id FROM task_input")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        let mut projected: Vec<&crate::task_store::rebuild::InputRow> = projection
            .inputs
            .iter()
            .filter(|input| targets.contains(&input.task_id))
            .collect();
        projected.sort_by(|a, b| (&a.task_id, a.id).cmp(&(&b.task_id, b.id)));
        let projects = |task: &str, id: i64| {
            projected
                .iter()
                .any(|input| input.task_id == task && input.id == id)
        };
        // Ids nothing may take from their holder: every other task's rows,
        // and a target's rows it projects itself.
        let mut claimed: BTreeMap<i64, String> = live
            .iter()
            .filter(|(id, task)| !targets.contains(task) || projects(task, *id))
            .map(|(id, task)| (*id, task.clone()))
            .collect();
        let allocated: i64 = self.conn.query_row(
            "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'task_input'), 0)",
            [],
            |row| row.get(0),
        )?;
        let mut next = live
            .iter()
            .map(|(id, _)| *id)
            .chain(projected.iter().map(|input| input.id))
            .chain([allocated])
            .max()
            .unwrap_or(0)
            + 1;
        let mut moved = BTreeMap::new();
        for input in &projected {
            match claimed.get(&input.id) {
                None => {
                    claimed.insert(input.id, input.task_id.clone());
                }
                Some(holder) if holder == &input.task_id => {}
                Some(_) => {
                    let earlier: Option<i64> = self
                        .conn
                        .query_row(
                            "SELECT id FROM task_input
                             WHERE task_id = ? AND source = ? AND message = ?
                               AND delivered_at = ? AND stage IS ?
                             ORDER BY id LIMIT 1",
                            params![
                                input.task_id,
                                input.source,
                                input.message,
                                input.delivered_at,
                                input.stage
                            ],
                            |row| row.get(0),
                        )
                        .optional()?
                        .filter(|id| !claimed.contains_key(id));
                    let id = match earlier {
                        Some(id) => id,
                        None => {
                            while claimed.contains_key(&next) {
                                next += 1;
                            }
                            next
                        }
                    };
                    claimed.insert(id, input.task_id.clone());
                    moved.insert((input.task_id.clone(), input.id), id);
                }
            }
        }
        Ok(moved)
    }
}

/// Carried columns that hold a `task_input` id of the row's task, found by
/// reading [`CARRIED_TABLES`] (a test keeps this list complete): a join
/// member's delivered outcome. An input a repair moves to a new id is moved
/// here too, so nothing names another task's input.
pub(crate) const INPUT_ID_REFERENCES: &[(&str, &str)] = &[("task_join_member", "input_id")];

/// `projection` with every carried reference to a moved input following it.
fn with_moved_inputs(projection: &Projection, moved: &BTreeMap<(String, i64), i64>) -> Projection {
    let mut projection = projection.clone();
    if moved.is_empty() {
        return projection;
    }
    for carried in &mut projection.carried {
        for (table, column) in INPUT_ID_REFERENCES {
            if carried.table != *table {
                continue;
            }
            let Some(old) = carried.row.get(*column).and_then(Value::as_i64) else {
                continue;
            };
            if let Some(new) = moved.get(&(carried.task_id.clone(), old)) {
                carried.row.insert((*column).to_string(), Value::from(*new));
            }
        }
    }
    projection
}

#[cfg(test)]
impl Db {
    /// PRAGMA integrity and foreign-key checks, as one list of problems.
    pub(crate) fn consistency_problems_for_tests(&self) -> Vec<String> {
        let mut problems: Vec<String> = self
            .conn
            .prepare("PRAGMA quick_check")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .filter(|line| line != "ok")
            .collect();
        let mut statement = self.conn.prepare("PRAGMA foreign_key_check").unwrap();
        let violations = statement
            .query_map([], |row| {
                Ok(format!(
                    "foreign key: {} rowid {:?} -> {}",
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?
                ))
            })
            .unwrap()
            .map(Result::unwrap);
        problems.extend(violations);
        problems
    }
}
