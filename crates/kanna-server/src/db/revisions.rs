//! The durable record of review revisions.
//!
//! `pipeline_item.revision_rounds` is a *budget counter*, not a history: a
//! human-requested revision resets it to zero by design, so a task that went
//! round three times can read as zero. The `task.revision_requested` event
//! carries the fact but the event feed is pruned after 14 days. Analytics
//! therefore keeps its own append-only row, written in the same transaction
//! as the event it mirrors.

use super::Db;
use rusqlite::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordedRevisionOrigin {
    Agent,
    Human,
}

impl RecordedRevisionOrigin {
    fn as_str(self) -> &'static str {
        match self {
            RecordedRevisionOrigin::Agent => "agent",
            RecordedRevisionOrigin::Human => "human",
        }
    }
}

/// Append one revision request to the durable record.
///
/// `applied` distinguishes a revision that actually started from one the
/// budget parked: a parked request is a review verdict, not a round the task
/// spent, and averaging the two together would overstate churn.
pub(super) fn record_revision_request(
    conn: &Connection,
    task_id: &str,
    origin: RecordedRevisionOrigin,
    target_stage: Option<&str>,
    applied: bool,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO task_revision (task_id, origin, target_stage, applied)
         VALUES (?, ?, ?, ?)",
        rusqlite::params![task_id, origin.as_str(), target_stage, applied as i64],
    )?;
    Ok(())
}

impl Db {
    pub fn record_revision_request_in_transaction(
        &self,
        task_id: &str,
        origin: RecordedRevisionOrigin,
        target_stage: Option<&str>,
        applied: bool,
    ) -> Result<(), rusqlite::Error> {
        record_revision_request(&self.conn, task_id, origin, target_stage, applied)
    }

    #[cfg(test)]
    pub fn count_test_task_revisions(&self, task_id: &str) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM task_revision WHERE task_id = ? AND applied = 1",
            [task_id],
            |row| row.get(0),
        )
    }
}

/// Loops spent into each destination stage of a named-exit task (spec §5).
/// One row per task and stage; a person sending the task back resets it.
pub(super) const STAGE_BUDGET_SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS task_stage_budget (
        task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        stage TEXT NOT NULL,
        spent INTEGER NOT NULL DEFAULT 0,
        updated_at TEXT NOT NULL DEFAULT (datetime('now')),
        PRIMARY KEY (task_id, stage)
    );
"#;

/// One unit of a destination stage's budget, as a loop spent it (or would
/// have, when `exhausted`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StageBudgetSpend {
    pub stage: String,
    /// Loops into `stage` spent after this one; unchanged when exhausted.
    pub spent: i64,
    pub limit: i64,
    pub exhausted: bool,
}

/// Which exit a transition took and who chose it, recorded on the ledger's
/// transition entry (spec §5: "the chosen exit is provenance").
///
/// `source` is `explicit` (the session named the exit in its result),
/// `default` (no exit named: success took `advance`), or `operator` (a person
/// or manager moved the task, which no session chose). Legacy-routed tasks
/// record none.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TransitionExit {
    pub exit: Option<String>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<StageBudgetSpend>,
}

impl TransitionExit {
    pub const EXPLICIT: &'static str = "explicit";
    pub const DEFAULT: &'static str = "default";
    pub const OPERATOR: &'static str = "operator";

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

impl Db {
    /// Loops already spent into `stage` for this task.
    pub fn stage_budget_spent(&self, task_id: &str, stage: &str) -> Result<i64, rusqlite::Error> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT spent FROM task_stage_budget WHERE task_id = ? AND stage = ?",
                [task_id, stage],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    /// Spend one loop into `stage` if its budget allows, inside the caller's
    /// write transaction so two results cannot both take the last unit.
    pub(crate) fn claim_stage_budget_in_transaction(
        &self,
        task_id: &str,
        stage: &str,
        limit: i64,
    ) -> Result<StageBudgetSpend, rusqlite::Error> {
        let spent = self.stage_budget_spent(task_id, stage)?;
        if spent >= limit {
            return Ok(StageBudgetSpend {
                stage: stage.to_string(),
                spent,
                limit,
                exhausted: true,
            });
        }
        self.conn.execute(
            "INSERT INTO task_stage_budget (task_id, stage, spent) VALUES (?, ?, 1)
             ON CONFLICT(task_id, stage) DO UPDATE SET
                spent = spent + 1, updated_at = datetime('now')",
            [task_id, stage],
        )?;
        Ok(StageBudgetSpend {
            stage: stage.to_string(),
            spent: spent + 1,
            limit,
            exhausted: false,
        })
    }

    /// A person or manager sent the task back to `stage`: its agents get a
    /// fresh budget there. Other stages keep theirs.
    pub(crate) fn reset_stage_budget(
        &self,
        task_id: &str,
        stage: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM task_stage_budget WHERE task_id = ? AND stage = ?",
            [task_id, stage],
        )?;
        Ok(())
    }
}
