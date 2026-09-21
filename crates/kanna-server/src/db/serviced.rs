//! The durable per-task serviced watermark: what a task manager has already
//! dealt with, so an idle task it has *not* dealt with keeps re-surfacing.
//!
//! The shape is the event cursor's, narrowed to one task. A manager records
//! how far through the task's own event log it had read when it serviced the
//! task; the work-set filter re-admits the task as soon as the log moves past
//! that mark. Two properties follow, and both are the reason this is a
//! watermark rather than a `handled` flag:
//!
//! - **It degrades safely.** A manager that dies mid-batch — after reading a
//!   task, before recording it — simply sees that task again. Nothing is lost
//!   by a write that never landed, because suppression exists only on the
//!   strength of a row that committed.
//! - **It is monotonic.** A replayed or late recording never rewinds a mark a
//!   newer one already moved forward.
//!
//! Recording servicing deliberately appends no task event and touches no
//! `pipeline_item` column. An event would put the servicing write itself past
//! the mark it just set, so every serviced task would immediately re-enter the
//! work set; a `pipeline_item` write would move `updated_at` and change
//! ordering for a fact no human surface shows.
//!
//! **This is not a second source of truth about blocked state.** The attention
//! badge (`pipeline_item.attention_requested`) and `task_blocker` stay
//! authoritative for "a human owes this task something". The watermark only
//! answers "has the manager looked at this since it last changed", and the
//! filter below reads the badge directly rather than caching anything about it
//! here.

use rusqlite::{params, OptionalExtension};

use super::Db;

/// One task's serviced mark, as it is read back.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskServicedWatermark {
    pub task_id: String,
    pub serviced_at: String,
    /// The `stage_run` that did the servicing, when the caller named one.
    /// Provenance only — nothing reads it to decide visibility.
    pub serviced_run_id: Option<String>,
    /// The `task_event.seq` the manager had read this task through.
    pub serviced_event_seq: i64,
}

/// The work-set predicate: unserviced idle work, with human-blocked tasks
/// withheld.
///
/// Written as a bare SQL fragment against the enclosing `pipeline_item` row so
/// callers can splice it into their own `WHERE` without renumbering bind
/// parameters.
///
/// Two independent clauses, both load-bearing:
///
/// 1. **Changed since serviced.** Never serviced, or the task's event log has
///    moved past the mark. `task.activity_changed` is excluded from that
///    comparison on purpose and by owner decision: it is the human read/unread
///    display dimension, so counting it would let a person *reading* a task
///    wake the manager. Nothing else is filtered — every other event is a
///    change to the task itself.
/// 2. **Human-blocked exclusion.** A task carrying an attention badge is
///    withheld until there is positive evidence that a human *acted*: the badge
///    was cleared, an `operator` input was delivered, or the runtime went back
///    to `busy`. Evidence is counted from whichever is later, the serviced mark
///    or the event that raised the badge now standing — so an operator input
///    that predates the badge does not release it.
///
/// Reading a task clears its unread state and bumps `updated_at`; neither
/// appears here, which is what keeps reading from being mistaken for acting.
pub(super) const UNSERVICED_WORK_PREDICATE: &str = r#"
(
    (
        NOT EXISTS (
            SELECT 1 FROM task_serviced_watermark w
            WHERE w.task_id = pipeline_item.id
        )
        OR EXISTS (
            SELECT 1 FROM task_event e
            WHERE e.task_id = pipeline_item.id
              AND e.type <> 'task.activity_changed'
              AND e.seq > (
                  SELECT w.serviced_event_seq FROM task_serviced_watermark w
                  WHERE w.task_id = pipeline_item.id
              )
        )
    )
    AND (
        pipeline_item.attention_requested = 0
        OR EXISTS (
            SELECT 1 FROM task_event e
            WHERE e.task_id = pipeline_item.id
              AND e.seq > MAX(
                  COALESCE((
                      SELECT MAX(badge.seq) FROM task_event badge
                      WHERE badge.task_id = pipeline_item.id
                        AND badge.type = 'task.attention_changed'
                        -- Retained pre-boolean events carry the reason string
                        -- instead of the flag; a raise is still a raise.
                        AND (
                            json_extract(badge.payload, '$.attentionRequested') = 1
                            OR json_extract(badge.payload, '$.attentionReason') IS NOT NULL
                        )
                  ), 0),
                  COALESCE((
                      SELECT w.serviced_event_seq FROM task_serviced_watermark w
                      WHERE w.task_id = pipeline_item.id
                  ), 0)
              )
              AND (
                  (
                      e.type = 'task.attention_changed'
                      AND json_extract(e.payload, '$.attentionRequested') = 0
                  )
                  OR (
                      e.type = 'task.input_delivered'
                      AND json_extract(e.payload, '$.source') = 'operator'
                  )
                  OR (
                      e.type = 'task.runtime_changed'
                      AND json_extract(e.payload, '$.runtimeState') = 'busy'
                  )
              )
        )
    )
)
"#;

pub(super) fn create_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_serviced_watermark (
             task_id TEXT PRIMARY KEY REFERENCES pipeline_item(id) ON DELETE CASCADE,
             serviced_at TEXT NOT NULL DEFAULT (datetime('now')),
             serviced_run_id TEXT,
             serviced_event_seq INTEGER NOT NULL
         );",
    )
}

impl Db {
    /// Record that a manager has serviced `task_id` through
    /// `observed_event_seq`, or through the log's current head when the caller
    /// names no cursor.
    ///
    /// The mark never moves backwards: a stale or replayed recording leaves a
    /// newer one alone. `serviced_at` and `serviced_run_id` always describe the
    /// most recent recording, because they are provenance for the write rather
    /// than inputs to the comparison.
    pub fn record_task_serviced(
        &self,
        task_id: &str,
        serviced_run_id: Option<&str>,
        observed_event_seq: Option<i64>,
    ) -> Result<TaskServicedWatermark, rusqlite::Error> {
        let seq = match observed_event_seq {
            Some(seq) => seq,
            None => self.latest_task_event_seq()?,
        };
        self.conn.execute(
            "INSERT INTO task_serviced_watermark (task_id, serviced_at, serviced_run_id, serviced_event_seq)
             VALUES (?1, datetime('now'), ?2, ?3)
             ON CONFLICT(task_id) DO UPDATE SET
                 serviced_at = excluded.serviced_at,
                 serviced_run_id = excluded.serviced_run_id,
                 serviced_event_seq = MAX(
                     excluded.serviced_event_seq,
                     task_serviced_watermark.serviced_event_seq
                 )",
            params![task_id, serviced_run_id, seq],
        )?;
        self.task_serviced_watermark(task_id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    /// The task's serviced mark, or `None` when it has never been serviced.
    pub fn task_serviced_watermark(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskServicedWatermark>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT task_id, serviced_at, serviced_run_id, serviced_event_seq
                 FROM task_serviced_watermark
                 WHERE task_id = ?1",
                params![task_id],
                |row| {
                    Ok(TaskServicedWatermark {
                        task_id: row.get(0)?,
                        serviced_at: row.get(1)?,
                        serviced_run_id: row.get(2)?,
                        serviced_event_seq: row.get(3)?,
                    })
                },
            )
            .optional()
    }
}
