//! The fully-resolved prompt text, per stage run.
//!
//! Partials are expanded and `$VAR` substitution has already run by the time
//! a stage run spawns, but until this table existed that text lived only in
//! the daemon spawn command — if an agent behaved strangely there was no way
//! to read back what it was actually told. One row per stage run, written
//! best-effort right beside the run's own `stage_run` insert, the same
//! pattern `workspace_setup_run` uses: kept out of `get_task`/`get_tasks` and
//! every other hot query path, and reachable only through its own endpoint.
use super::Db;
use rusqlite::{params, OptionalExtension};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageRunPrompt {
    pub run_id: String,
    pub resolved_prompt: String,
    pub created_at: String,
}

impl Db {
    /// Record one stage run's fully-resolved prompt. First writer wins: a
    /// retry that re-prepares a spawn for a run already holding a record
    /// leaves the original alone rather than rewriting history somebody may
    /// have read.
    pub fn record_stage_run_prompt(
        &self,
        run_id: &str,
        resolved_prompt: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO stage_run_prompt (run_id, resolved_prompt)
             VALUES (?, ?)
             ON CONFLICT(run_id) DO NOTHING",
            params![run_id, resolved_prompt],
        )?;
        Ok(())
    }

    /// One stage run's resolved prompt, scoped to the task that owns the run
    /// so a reader cannot address another task's workspace by run id alone.
    pub fn stage_run_prompt(
        &self,
        task_id: &str,
        run_id: &str,
    ) -> Result<Option<StageRunPrompt>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT p.run_id, p.resolved_prompt, p.created_at
                 FROM stage_run_prompt p
                 JOIN stage_run sr ON sr.id = p.run_id
                 WHERE sr.task_id = ? AND p.run_id = ?",
                params![task_id, run_id],
                stage_run_prompt_from_row,
            )
            .optional()
    }
}

fn stage_run_prompt_from_row(row: &rusqlite::Row<'_>) -> Result<StageRunPrompt, rusqlite::Error> {
    Ok(StageRunPrompt {
        run_id: row.get(0)?,
        resolved_prompt: row.get(1)?,
        created_at: row.get(2)?,
    })
}
