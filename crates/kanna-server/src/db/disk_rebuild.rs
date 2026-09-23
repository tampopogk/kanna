//! SQLite half of the offline rebuild from task directories (spec §16.11,
//! T13 first increment). [`crate::task_store::rebuild`] reads and projects;
//! this writes the projection into a fresh database.
//!
//! Every write is an upsert of exactly the projected columns, and no row
//! relies on a `datetime('now')` default, so applying the same projection
//! again leaves the database byte-for-byte as it was.

use super::task_state::json_to_sql;
use super::Db;
use crate::task_store::rebuild::{
    BudgetRow, CarriedRow, InputRow, LedgerRow, Projection, StageRunRow,
};
use rusqlite::{params, OptionalExtension};
use serde_json::{Map, Value};

/// The primary-key columns of `table`, in key order; empty for a table
/// without a declared primary key.
pub(super) fn primary_key(db: &Db, table: &str) -> Result<Vec<String>, rusqlite::Error> {
    let mut statement = db
        .conn
        .prepare("SELECT name FROM pragma_table_info(?) WHERE pk > 0 ORDER BY pk")?;
    let columns = statement.query_map([table], |row| row.get(0))?;
    columns.collect()
}

/// Insert one row as carried, `rowid` included. A re-application finds the
/// same row (same primary key) under its rowid and leaves it as it is; any
/// other row under that rowid, or the same key under another rowid, is a
/// collision and refuses the rebuild rather than dropping the carried row.
pub(super) fn insert_row(
    db: &Db,
    table: &str,
    row: &Map<String, Value>,
) -> Result<(), rusqlite::Error> {
    let collision = |detail: String| {
        rusqlite::Error::InvalidParameterName(format!("{table}: carried row collides: {detail}"))
    };
    let key: Vec<String> = primary_key(db, table)?
        .into_iter()
        .filter(|column| row.contains_key(column))
        .collect();
    let key_values = key
        .iter()
        .map(|column| {
            json_to_sql(&row[column]).map_err(|error| {
                rusqlite::Error::InvalidParameterName(format!("{table}.{column}: {error}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let key_where = key
        .iter()
        .map(|column| format!("\"{column}\" IS ?"))
        .collect::<Vec<_>>()
        .join(" AND ");
    let rowid = row.get("rowid").and_then(Value::as_i64);
    if let Some(rowid) = rowid {
        let occupant: Option<bool> = db
            .conn
            .query_row(
                &format!(
                    "SELECT {} FROM \"{table}\" WHERE rowid = ?",
                    if key.is_empty() {
                        "1".to_string()
                    } else {
                        format!("({key_where})")
                    }
                ),
                rusqlite::params_from_iter(
                    key_values
                        .iter()
                        .cloned()
                        .chain([rusqlite::types::Value::Integer(rowid)]),
                ),
                |row| row.get(0),
            )
            .optional()?;
        match occupant {
            Some(true) => return Ok(()),
            Some(false) => return Err(collision(format!("rowid {rowid} holds another row"))),
            None => {}
        }
    }
    if !key.is_empty() {
        let elsewhere: Option<i64> = db
            .conn
            .query_row(
                &format!("SELECT rowid FROM \"{table}\" WHERE {key_where}"),
                rusqlite::params_from_iter(key_values.iter().cloned()),
                |row| row.get(0),
            )
            .optional()?;
        if let Some(elsewhere) = elsewhere {
            // A row the projection added (no carried rowid) is applied
            // once; its key already present is a re-application.
            if rowid.is_none() {
                return Ok(());
            }
            return Err(collision(format!(
                "its key is already held by rowid {elsewhere}"
            )));
        }
    }
    let mut columns = Vec::with_capacity(row.len());
    let mut values = Vec::with_capacity(row.len());
    for (column, value) in row {
        columns.push(format!("\"{column}\""));
        values.push(json_to_sql(value).map_err(|error| {
            rusqlite::Error::InvalidParameterName(format!("{table}.{column}: {error}"))
        })?);
    }
    let placeholders = vec!["?"; values.len()].join(", ");
    db.conn.execute(
        &format!(
            "INSERT INTO \"{table}\" ({}) VALUES ({placeholders})",
            columns.join(", ")
        ),
        rusqlite::params_from_iter(values),
    )?;
    Ok(())
}

pub(super) fn upsert_stage_run(db: &Db, run: &StageRunRow) -> Result<(), rusqlite::Error> {
    db.conn.execute(
        "INSERT INTO stage_run
            (id, task_id, stage, kind, status, result, feedback, started_at,
             finished_at, result_declared_role, result_channel_identity,
             workspace_id, session_branch, session_name, transcript_ref,
             no_work_termination)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            task_id = excluded.task_id, stage = excluded.stage,
            kind = excluded.kind, status = excluded.status,
            result = excluded.result, feedback = excluded.feedback,
            no_work_termination = excluded.no_work_termination,
            started_at = excluded.started_at, finished_at = excluded.finished_at,
            result_declared_role = excluded.result_declared_role,
            result_channel_identity = excluded.result_channel_identity,
            workspace_id = excluded.workspace_id,
            session_branch = excluded.session_branch,
            session_name = excluded.session_name,
            transcript_ref = excluded.transcript_ref",
        params![
            run.id,
            run.task_id,
            run.stage,
            run.kind,
            run.status,
            run.result,
            run.feedback,
            run.started_at,
            run.finished_at,
            run.result_declared_role,
            run.result_channel_identity,
            run.workspace_id,
            run.session_branch,
            run.session_name,
            run.transcript_ref,
            run.no_work_termination,
        ],
    )?;
    Ok(())
}

pub(super) fn upsert_input(db: &Db, input: &InputRow) -> Result<(), rusqlite::Error> {
    db.conn.execute(
        "INSERT INTO task_input
            (id, task_id, run_id, stage, source, message, delivered_at,
             origin_peer_id, origin_task_id, origin_input_id, origin_run_id,
             channel_identity)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            task_id = excluded.task_id, run_id = excluded.run_id,
            stage = excluded.stage, source = excluded.source,
            message = excluded.message, delivered_at = excluded.delivered_at,
            origin_peer_id = excluded.origin_peer_id,
            origin_task_id = excluded.origin_task_id,
            origin_input_id = excluded.origin_input_id,
            origin_run_id = excluded.origin_run_id,
            channel_identity = excluded.channel_identity",
        params![
            input.id,
            input.task_id,
            input.run_id,
            input.stage,
            input.source,
            input.message,
            input.delivered_at,
            input.origin_peer_id,
            input.origin_task_id,
            input.origin_input_id,
            input.origin_run_id,
            input.channel_identity,
        ],
    )?;
    Ok(())
}

pub(super) fn upsert_budget(db: &Db, budget: &BudgetRow) -> Result<(), rusqlite::Error> {
    db.conn.execute(
        "INSERT INTO task_stage_budget (task_id, stage, spent, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(task_id, stage) DO UPDATE SET
            spent = excluded.spent, updated_at = excluded.updated_at",
        params![
            budget.task_id,
            budget.stage,
            budget.spent,
            budget.updated_at
        ],
    )?;
    Ok(())
}

pub(super) fn upsert_published_ledger_row(
    db: &Db,
    entry: &LedgerRow,
) -> Result<(), rusqlite::Error> {
    db.conn.execute(
        "INSERT INTO task_ledger_entry
            (task_id, sequence, entry_id, kind, operation_id, source_kind,
             source_id, file_name, payload, created_at, published_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(task_id, sequence) DO UPDATE SET
            entry_id = excluded.entry_id, kind = excluded.kind,
            operation_id = excluded.operation_id,
            source_kind = excluded.source_kind, source_id = excluded.source_id,
            file_name = excluded.file_name, payload = excluded.payload,
            created_at = excluded.created_at, published_at = excluded.published_at",
        params![
            entry.task_id,
            entry.sequence,
            entry.entry_id,
            entry.kind,
            entry.operation_id,
            entry.source_kind,
            entry.source_id,
            entry.file_name,
            entry.payload,
            entry.recorded_at,
            entry.recorded_at,
        ],
    )?;
    Ok(())
}

impl Db {
    /// A database a rebuild may write into: no tasks, runs, inputs or
    /// ledger rows.
    pub(crate) fn is_empty_for_disk_rebuild(&self) -> Result<bool, rusqlite::Error> {
        self.conn.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM pipeline_item)
                AND NOT EXISTS(SELECT 1 FROM stage_run)
                AND NOT EXISTS(SELECT 1 FROM task_input)
                AND NOT EXISTS(SELECT 1 FROM task_ledger_entry)",
            [],
            |row| row.get(0),
        )
    }

    /// Write a projection in one transaction. Idempotent.
    pub(crate) fn apply_disk_projection(
        &self,
        projection: &Projection,
    ) -> Result<(), rusqlite::Error> {
        self.with_immediate_transaction(|db| {
            // Registrations from repo.json; a placeholder only satisfies
            // `pipeline_item.repo_id`'s foreign key for a repository whose
            // registration is not on disk.
            for repo_id in &projection.repos {
                let record = projection
                    .repo_records
                    .iter()
                    .find(|record| &record.repo_id == repo_id);
                let present: bool = db.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM repo WHERE id = ?)",
                    [repo_id],
                    |row| row.get(0),
                )?;
                match record {
                    Some(_) if present => {}
                    Some(record) => insert_row(db, "repo", &record.registration)?,
                    None => {
                        db.conn.execute(
                            "INSERT OR IGNORE INTO repo (id, path, name, created_at, last_opened_at)
                             VALUES (?1, '', ?1, '', '')",
                            params![repo_id],
                        )?;
                    }
                }
                if let Some(record) = record {
                    let hash = record
                        .registration
                        .get("remote_url_hash")
                        .and_then(Value::as_str);
                    if let (Some(hash), Some(order)) = (hash, record.sidebar_order) {
                        db.conn.execute(
                            "INSERT OR IGNORE INTO repo_sidebar_order (remote_url_hash, sort_order)
                             VALUES (?, ?)",
                            params![hash, order],
                        )?;
                    }
                }
                // Current as written: nothing is owed to publish.
                let revision = record.map_or(1, |record| record.snapshot_revision.max(1));
                db.conn.execute(
                    "INSERT INTO repo_disk_snapshot (repo_id, revision, published_revision)
                     VALUES (?1, ?2, ?2)
                     ON CONFLICT(repo_id) DO UPDATE SET
                        revision = excluded.revision,
                        published_revision = excluded.published_revision,
                        publish_error = NULL",
                    params![repo_id, revision],
                )?;
            }
            // Carried rows keep their rowids, so they go in before any row
            // whose rowid SQLite allocates: first the carried task rows,
            // then the tasks of task.json files without `state` (whose rows
            // other tasks' carried edges may name), then everything else
            // carried, table by table in foreign-key order.
            let (carried_tasks, carried_rest): (Vec<&CarriedRow>, Vec<&CarriedRow>) = projection
                .carried
                .iter()
                .partition(|carried| carried.table == "pipeline_item");
            for CarriedRow { table, row, .. } in carried_tasks {
                insert_row(db, table, row)?;
            }
            // Required columns the snapshot leaves empty take the schema's
            // own default, as an insert that omitted them would.
            for task in projection.tasks.iter().filter(|task| !task.from_state) {
                db.conn.execute(
                    "INSERT INTO pipeline_item
                        (id, repo_id, prompt, display_name, pipeline, pipeline_def, stage,
                         branch, base_ref, parent_task_id, pr_url, pr_number,
                         created_at, updated_at, closed_at)
                     VALUES (?, ?, ?, ?, COALESCE(?, 'default'), ?, COALESCE(?, 'in_progress'),
                             ?, ?, ?, ?, ?, COALESCE(?, ''), COALESCE(?, ''), ?)
                     ON CONFLICT(id) DO UPDATE SET
                        repo_id = excluded.repo_id, prompt = excluded.prompt,
                        display_name = excluded.display_name, pipeline = excluded.pipeline,
                        pipeline_def = excluded.pipeline_def, stage = excluded.stage,
                        branch = excluded.branch, base_ref = excluded.base_ref,
                        parent_task_id = excluded.parent_task_id, pr_url = excluded.pr_url,
                        pr_number = excluded.pr_number, created_at = excluded.created_at,
                        updated_at = excluded.updated_at, closed_at = excluded.closed_at",
                    params![
                        task.id,
                        task.repo_id,
                        task.prompt,
                        task.display_name,
                        task.workflow_name,
                        task.workflow_definition,
                        task.stage,
                        task.branch,
                        task.base_ref,
                        task.parent_task_id,
                        task.pr_url,
                        task.pr_number,
                        task.created_at,
                        task.updated_at,
                        task.closed_at,
                    ],
                )?;
            }
            for CarriedRow { table, row, .. } in carried_rest {
                insert_row(db, table, row)?;
            }
            for (blocked, blocker) in &projection.blockers {
                db.conn.execute(
                    "INSERT OR IGNORE INTO task_blocker (blocked_item_id, blocker_item_id)
                     VALUES (?, ?)",
                    params![blocked, blocker],
                )?;
            }
            for run in &projection.stage_runs {
                upsert_stage_run(db, run)?;
            }
            for input in &projection.inputs {
                upsert_input(db, input)?;
            }
            for budget in &projection.budgets {
                upsert_budget(db, budget)?;
            }
            // The outbox, already published: the files are what it describes.
            for entry in &projection.ledger {
                upsert_published_ledger_row(db, entry)?;
            }
            for marker in &projection.markers {
                db.conn.execute(
                    "INSERT INTO task_ledger_snapshot (task_id, revision, published_revision)
                     VALUES (?1, ?2, ?2)
                     ON CONFLICT(task_id) DO UPDATE SET
                        revision = excluded.revision,
                        published_revision = excluded.published_revision,
                        publish_error = NULL",
                    params![marker.task_id, marker.snapshot_revision],
                )?;
                db.conn.execute(
                    "INSERT INTO task_ledger_backfill (task_id, imported_entries, completed_at)
                     VALUES (?, ?, ?)
                     ON CONFLICT(task_id) DO UPDATE SET
                        imported_entries = excluded.imported_entries,
                        completed_at = excluded.completed_at",
                    params![marker.task_id, marker.historical_entries, marker.marked_at],
                )?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
impl Db {
    /// Every row of every table, rendered and sorted per table: two
    /// databases with equal dumps hold the same data.
    pub(crate) fn disk_rebuild_dump_for_tests(&self) -> Vec<String> {
        let tables: Vec<String> = self
            .conn
            // Written by the migrations that created the file, stamped with
            // when they ran (`schema_migrations`, and settings such as when
            // analytics coverage started); a rebuild writes neither.
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table' AND name NOT IN ('schema_migrations', 'settings')
                 ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let mut dump = Vec::new();
        for table in tables {
            let mut statement = self
                .conn
                .prepare(&format!("SELECT * FROM \"{table}\""))
                .unwrap();
            let columns = statement.column_count();
            let mut rows: Vec<String> = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|index| row.get::<_, rusqlite::types::Value>(index))
                        .collect::<Result<Vec<_>, _>>()
                        .map(|values| format!("{table}: {values:?}"))
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            rows.sort();
            dump.extend(rows);
        }
        dump
    }
}
