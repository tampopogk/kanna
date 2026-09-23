//! The commit step of a transition (spec §5, component T3).
//!
//! A stage that declares `exit_commit` does not transition on the request
//! that asks it to: the request starts a commit run in the stage's session
//! (or a short commit session in its workspace), and the transition fires on
//! that run's result. One row binds one commit run to the one transition it
//! was requested for — the stage it leaves and the exit it takes — so the
//! run's result can settle it exactly once, cannot name another exit, and
//! cannot ask for a further commit.

use super::{Db, TransitionExit};
use rusqlite::OptionalExtension;

/// Schema of migration `099_transition_commit`.
pub(super) const TRANSITION_COMMIT_SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS transition_commit (
        run_id TEXT PRIMARY KEY,
        task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        stage TEXT NOT NULL,
        exit TEXT,
        state TEXT NOT NULL DEFAULT 'requested'
            CHECK (state IN ('requested', 'succeeded', 'failed')),
        result_id TEXT,
        committed_sha TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        settled_at TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_transition_commit_task
        ON transition_commit(task_id);
"#;

/// One requested commit step and, once its run recorded a result, how it
/// settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionCommit {
    /// The commit run; also the operation's identity.
    pub run_id: String,
    pub task_id: String,
    /// The stage whose transition this commit step belongs to.
    pub stage: String,
    /// The exit the requested transition takes, for a named-exit task.
    pub exit: Option<TransitionExit>,
    /// `requested`, `succeeded` or `failed`.
    pub state: String,
    /// The ledger result entry that settled it.
    pub result_id: Option<String>,
    /// The commit that result recorded: what the next stage forks from.
    pub committed_sha: Option<String>,
}

impl TransitionCommit {
    pub const REQUESTED: &'static str = "requested";
    pub const SUCCEEDED: &'static str = "succeeded";
    pub const FAILED: &'static str = "failed";
}

impl Db {
    /// Record that `run_id` is the commit step of a transition out of
    /// `stage`. Idempotent: restart reconciliation of the same accepted
    /// delivery records the same row, never a second request.
    pub(crate) fn insert_transition_commit(
        &self,
        run_id: &str,
        task_id: &str,
        stage: &str,
        exit: Option<&TransitionExit>,
    ) -> Result<(), rusqlite::Error> {
        self.refuse_while_transferring(task_id)?;
        let exit = exit.map(|exit| exit.to_json().to_string());
        // A new commit step supersedes any earlier one still requested (its
        // run was replaced by a rerun and can never be current again), so a
        // task never holds more than one requested commit step.
        self.conn.execute(
            "UPDATE transition_commit SET state = 'failed', settled_at = datetime('now')
             WHERE task_id = ? AND state = 'requested' AND run_id <> ?",
            rusqlite::params![task_id, run_id],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO transition_commit (run_id, task_id, stage, exit)
             VALUES (?, ?, ?, ?)",
            rusqlite::params![run_id, task_id, stage, exit],
        )?;
        Ok(())
    }

    /// Move a requested commit step from the run a restart replaced to its
    /// replacement, inside the caller's write transaction. `false` when the
    /// step is not requested any more (it settled), so a restart can never
    /// revive a settled step.
    pub(crate) fn rekey_requested_transition_commit(
        &self,
        replaced_run_id: &str,
        replacement_run_id: &str,
    ) -> Result<bool, rusqlite::Error> {
        let changed = self.conn.execute(
            "UPDATE transition_commit SET run_id = ?
             WHERE run_id = ? AND state = 'requested'",
            rusqlite::params![replacement_run_id, replaced_run_id],
        )?;
        Ok(changed == 1)
    }

    /// The commit step `run_id` runs, if it is one.
    pub(crate) fn transition_commit(
        &self,
        run_id: &str,
    ) -> Result<Option<TransitionCommit>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT run_id, task_id, stage, exit, state, result_id, committed_sha
                 FROM transition_commit WHERE run_id = ?",
                [run_id],
                |row| {
                    let exit: Option<String> = row.get(3)?;
                    Ok(TransitionCommit {
                        run_id: row.get(0)?,
                        task_id: row.get(1)?,
                        stage: row.get(2)?,
                        // An unreadable exit is recorded as none rather than
                        // failing every read of the task's commit step.
                        exit: exit.and_then(|exit| serde_json::from_str(&exit).ok()),
                        state: row.get(4)?,
                        result_id: row.get(5)?,
                        committed_sha: row.get(6)?,
                    })
                },
            )
            .optional()
    }

    /// Settle a requested commit step with its run's result, inside the
    /// caller's write transaction. `false` when it is not requested any more,
    /// so a second result can never settle it again.
    pub(crate) fn settle_transition_commit_in_transaction(
        &self,
        run_id: &str,
        succeeded: bool,
        result_id: &str,
        committed_sha: Option<&str>,
    ) -> Result<bool, rusqlite::Error> {
        let state = if succeeded {
            TransitionCommit::SUCCEEDED
        } else {
            TransitionCommit::FAILED
        };
        let changed = self.conn.execute(
            "UPDATE transition_commit
             SET state = ?, result_id = ?, committed_sha = ?, settled_at = datetime('now')
             WHERE run_id = ? AND state = 'requested'",
            rusqlite::params![state, result_id, committed_sha, run_id],
        )?;
        Ok(changed == 1)
    }
}
