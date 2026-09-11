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
