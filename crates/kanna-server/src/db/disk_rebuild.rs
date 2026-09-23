//! SQLite half of the offline rebuild from task directories (spec §16.11,
//! T13 first increment). [`crate::task_store::rebuild`] reads and projects;
//! this writes the projection into a fresh database.
//!
//! Every write is an upsert of exactly the projected columns, and no row
//! relies on a `datetime('now')` default, so applying the same projection
//! again leaves the database byte-for-byte as it was.

use super::Db;
use crate::task_store::rebuild::Projection;
use rusqlite::params;

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
            // Placeholders that satisfy `pipeline_item.repo_id`'s foreign key:
            // the registration itself is not on disk.
            for repo_id in &projection.repos {
                db.conn.execute(
                    "INSERT OR IGNORE INTO repo (id, path, name, created_at, last_opened_at)
                     VALUES (?1, '', ?1, '', '')",
                    params![repo_id],
                )?;
            }
            // Required columns the snapshot leaves empty take the schema's
            // own default, as an insert that omitted them would.
            for task in &projection.tasks {
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
            for (blocked, blocker) in &projection.blockers {
                db.conn.execute(
                    "INSERT OR IGNORE INTO task_blocker (blocked_item_id, blocker_item_id)
                     VALUES (?, ?)",
                    params![blocked, blocker],
                )?;
            }
            for run in &projection.stage_runs {
                db.conn.execute(
                    "INSERT INTO stage_run
                        (id, task_id, stage, kind, status, result, feedback, started_at,
                         finished_at, result_declared_role, result_channel_identity,
                         workspace_id, session_branch, session_name, transcript_ref)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                     ON CONFLICT(id) DO UPDATE SET
                        task_id = excluded.task_id, stage = excluded.stage,
                        kind = excluded.kind, status = excluded.status,
                        result = excluded.result, feedback = excluded.feedback,
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
                    ],
                )?;
            }
            for input in &projection.inputs {
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
            }
            for budget in &projection.budgets {
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
            }
            // The outbox, already published: the files are what it describes.
            for entry in &projection.ledger {
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
