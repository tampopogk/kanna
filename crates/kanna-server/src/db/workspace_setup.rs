//! The workspace setup stream, per stage run.
//!
//! A repository's `setup` commands used to leave nothing addressable behind.
//! On a task's first spawn they were inlined into the agent's own PTY command,
//! so their output was the head of that session's scrollback with no stored
//! boundary; on every stage after it they ran on the server-side workspace
//! command runner, whose buffer was folded into the error string on failure
//! and dropped on success. A stage therefore had either an inseparable setup
//! stream or none at all.
//!
//! Both paths now run on the same runner and land here: one row per stage run,
//! written whether setup succeeded or failed, carrying the runner's already
//! capped buffer and the real exit status. `run_id` is the stage run the setup
//! prepared the workspace for, so the record is addressable by the same
//! identity as that stage's terminal attempt.
use super::Db;
use rusqlite::{params, OptionalExtension};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSetupRun {
    pub run_id: String,
    /// `succeeded` | `failed`. A timeout is a failure with no exit code.
    pub status: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// The runner's output cap was reached and the tail was dropped.
    pub truncated: bool,
    /// The setup commands, in the order they ran.
    pub commands: Vec<String>,
    pub output: String,
    /// Wall-clock milliseconds the setup commands ran for.
    pub duration_ms: i64,
    pub finished_at: String,
}

/// What a completed workspace-setup run recorded, before it is bound to the
/// stage run it prepared. The runner produces this; the caller persists it
/// once the run row exists.
#[derive(Debug, Clone)]
pub struct WorkspaceSetupOutcome {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub truncated: bool,
    pub commands: Vec<String>,
    pub output: String,
    pub duration_ms: i64,
}

impl WorkspaceSetupOutcome {
    pub fn succeeded(&self) -> bool {
        !self.timed_out && self.exit_code == Some(0)
    }

    pub fn status(&self) -> &'static str {
        if self.succeeded() {
            "succeeded"
        } else {
            "failed"
        }
    }
}

impl Db {
    /// Record one stage run's workspace setup. First writer wins: a retry that
    /// re-runs setup for a run already holding a record leaves the original
    /// alone rather than rewriting history somebody may have read.
    pub fn record_workspace_setup_run(
        &self,
        run_id: &str,
        outcome: &WorkspaceSetupOutcome,
    ) -> Result<(), rusqlite::Error> {
        let commands =
            serde_json::to_string(&outcome.commands).map_err(|_| rusqlite::Error::InvalidQuery)?;
        self.conn.execute(
            "INSERT INTO workspace_setup_run
               (run_id, status, exit_code, timed_out, truncated, commands, output, duration_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(run_id) DO NOTHING",
            params![
                run_id,
                outcome.status(),
                outcome.exit_code,
                outcome.timed_out,
                outcome.truncated,
                commands,
                outcome.output,
                outcome.duration_ms,
            ],
        )?;
        Ok(())
    }

    /// One stage run's setup record, scoped to the task that owns the run so a
    /// reader cannot address another task's workspace by run id alone.
    pub fn workspace_setup_run(
        &self,
        task_id: &str,
        run_id: &str,
    ) -> Result<Option<WorkspaceSetupRun>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT w.run_id, w.status, w.exit_code, w.timed_out, w.truncated, w.commands,
                        w.output, w.duration_ms, w.finished_at
                 FROM workspace_setup_run w
                 JOIN stage_run sr ON sr.id = w.run_id
                 WHERE sr.task_id = ? AND w.run_id = ?",
                params![task_id, run_id],
                workspace_setup_run_from_row,
            )
            .optional()
    }

    /// Every setup record this task has, oldest first.
    pub fn workspace_setup_runs(
        &self,
        task_id: &str,
    ) -> Result<Vec<WorkspaceSetupRun>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT w.run_id, w.status, w.exit_code, w.timed_out, w.truncated, w.commands,
                    w.output, w.duration_ms, w.finished_at
             FROM workspace_setup_run w
             JOIN stage_run sr ON sr.id = w.run_id
             WHERE sr.task_id = ?
             ORDER BY sr.rowid ASC",
        )?;
        let rows = stmt.query_map([task_id], workspace_setup_run_from_row)?;
        rows.collect()
    }
}

fn workspace_setup_run_from_row(
    row: &rusqlite::Row<'_>,
) -> Result<WorkspaceSetupRun, rusqlite::Error> {
    let commands: String = row.get(5)?;
    Ok(WorkspaceSetupRun {
        run_id: row.get(0)?,
        status: row.get(1)?,
        exit_code: row.get(2)?,
        timed_out: row.get(3)?,
        truncated: row.get(4)?,
        commands: serde_json::from_str(&commands).unwrap_or_default(),
        output: row.get(6)?,
        duration_ms: row.get(7)?,
        finished_at: row.get(8)?,
    })
}
