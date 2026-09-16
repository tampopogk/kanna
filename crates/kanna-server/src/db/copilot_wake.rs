//! A native enqueue receipt is neither a mailbox acknowledgement nor owner speech.
//! Attempts outlive their batch: an ack or stage change can precede a late receipt.
use super::{Db, EventSubscription, TaskEventKind};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Registration {
    pub task_id: String,
    pub run_id: String,
    pub session_id: String,
    pub connection_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Attempt {
    pub id: String,
    pub binding: Registration,
    pub subscription_id: String,
    pub batch_id: i64,
    pub stage: Option<String>,
    pub message: String,
    pub input_id: Option<i64>,
    pub message_id: Option<String>,
    pub event_id: Option<String>,
    pub error: Option<String>,
}

fn encode<T: Serialize>(value: &T) -> rusqlite::Result<String> {
    serde_json::to_string(value).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}
fn decode<T: for<'de> Deserialize<'de>>(text: String) -> rusqlite::Result<T> {
    serde_json::from_str(&text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

impl Db {
    pub(crate) fn copilot_wake_bound(&self, binding: &Registration) -> rusqlite::Result<bool> {
        let task = self.get_pipeline_item(&binding.task_id)?;
        let run = self.latest_stage_run(&binding.task_id)?;
        Ok(task.zip(run).is_some_and(|(task, run)| {
            task.closed_at.is_none()
                && task.runtime_status.as_deref() != Some("exited")
                && task.stage.as_deref() == Some(run.stage.as_str())
                && run.id == binding.run_id
                && run.agent_provider.as_deref() == Some("copilot")
                && run.provider_session_id.as_deref() == Some(binding.session_id.as_str())
        }))
    }

    pub(crate) fn copilot_registration(&self, run: &str) -> rusqlite::Result<Option<Registration>> {
        self.conn
            .query_row(
                "SELECT record FROM copilot_wake_registration WHERE run_id=?",
                [run],
                |r| r.get(0),
            )
            .optional()?
            .map(decode)
            .transpose()
    }

    pub(crate) fn copilot_attempt(&self, id: &str) -> rusqlite::Result<Option<Attempt>> {
        self.conn
            .query_row(
                "SELECT record FROM copilot_wake_attempt WHERE id=?",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .map(decode)
            .transpose()
    }

    fn save_copilot_attempt(&self, attempt: &Attempt) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE copilot_wake_attempt SET record=? WHERE id=?",
            params![encode(attempt)?, attempt.id],
        )?;
        Ok(())
    }

    pub(crate) fn pending_copilot_attempts(&self, run: &str) -> rusqlite::Result<Vec<Attempt>> {
        let records = self
            .conn
            .prepare("SELECT record FROM copilot_wake_attempt WHERE run_id=? ORDER BY rowid")?
            .query_map([run], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let attempts = records
            .into_iter()
            .map(decode::<Attempt>)
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(attempts
            .into_iter()
            .filter(|a| a.input_id.is_none())
            .collect())
    }

    /// Registration survives server recovery as identity, never as proof of a
    /// live connection. The HTTP registry alone owns the current stream.
    pub(crate) fn register_copilot_wake(
        &self,
        binding: &Registration,
    ) -> rusqlite::Result<Option<Vec<Attempt>>> {
        self.with_immediate_transaction(|db| {
            if !db.copilot_wake_bound(binding)? { return Ok(None); }
            db.conn.execute("INSERT INTO copilot_wake_registration(run_id,task_id,record) VALUES(?,?,?) ON CONFLICT(run_id) DO UPDATE SET record=excluded.record",
                params![binding.run_id, binding.task_id, encode(binding)?])?;
            let mut pending = Vec::new();
            for mut attempt in db.pending_copilot_attempts(&binding.run_id)? {
                    attempt.binding = binding.clone();
                    attempt.error = Some("reconciling native history after reconnect; never resend uncertain input".into());
                    db.save_copilot_attempt(&attempt)?;
                    db.set_copilot_mailbox_state(&attempt, "uncertain", attempt.error.as_deref())?;
                    pending.push(attempt);
            }
            Ok(Some(pending))
        })
    }

    /// Reserve before crossing the process boundary. Existing attempts can only
    /// be inspected, never resent, including after a crash before the write.
    pub(crate) fn prepare_copilot_wake(
        &self,
        row: &EventSubscription,
        binding: &Registration,
    ) -> rusqlite::Result<Option<(Attempt, bool)>> {
        self.with_immediate_transaction(|db| {
            let Some(current) = db.event_subscription(&row.id)? else { return Ok(None); };
            if !current.active || current.batch_id != row.batch_id || current.pending.is_none()
                || current.run_id != binding.run_id || current.task_id != binding.task_id
                || !db.copilot_wake_bound(binding)?
                || db.copilot_registration(&binding.run_id)?.is_none_or(|r| r.connection_id != binding.connection_id)
            { return Ok(None); }
            let id = format!("{}-{}", row.id, row.batch_id);
            if let Some(attempt) = db.copilot_attempt(&id)? { return Ok(Some((attempt, false))); }
            let message = format!("[Kanna supervisor] Event subscription {} has pending events (batch {}). Read them with kanna_read_event_subscription, then acknowledge that batch after reconciling it. This is an engine wakeup, not an owner directive or a task-completion verdict. [Kanna wake {}/{}/{}/{}]", row.id, row.batch_id, row.task_id, row.run_id, binding.session_id, id);
            let attempt = Attempt { id, binding: binding.clone(), subscription_id:row.id.clone(), batch_id:row.batch_id, stage:row.stage.clone(), message, input_id:None, message_id:None, event_id:None, error:None };
            db.conn.execute("INSERT INTO copilot_wake_attempt(id,run_id,task_id,subscription_id,batch_id,record) VALUES(?,?,?,?,?,?)",
                params![attempt.id, binding.run_id, binding.task_id, row.id, row.batch_id, encode(&attempt)?])?;
            db.set_copilot_mailbox_state(&attempt, "awaiting_receipt", None)?;
            Ok(Some((attempt, true)))
        })
    }

    fn set_copilot_mailbox_state(
        &self,
        attempt: &Attempt,
        state: &str,
        error: Option<&str>,
    ) -> rusqlite::Result<()> {
        if let Some(mut row) = self.event_subscription(&attempt.subscription_id)? {
            if row.active
                && row.run_id == attempt.binding.run_id
                && row.batch_id == attempt.batch_id
                && row.pending.is_some()
            {
                row.wake_state = state.into();
                row.error = error.map(str::to_owned);
                self.save_event_subscription(&mut row)?;
            }
        }
        Ok(())
    }

    /// Both confirmed enqueue and positive native-history evidence record the
    /// server-authored text once, on its original run, even after ack/transition.
    pub(crate) fn receipt_copilot_wake(
        &self,
        id: &str,
        connection: &str,
        message_id: Option<&str>,
        event_id: Option<&str>,
        error: Option<&str>,
    ) -> rusqlite::Result<bool> {
        self.with_immediate_transaction(|db| {
            let Some(mut attempt) = db.copilot_attempt(id)? else { return Ok(false); };
            if attempt.binding.connection_id != connection { return Ok(false); }
            if attempt.input_id.is_some() { return Ok(true); }
            if message_id.is_none() && event_id.is_none() {
                attempt.error = Some(error.unwrap_or("native outcome unknown; batch remains pending").into());
                db.save_copilot_attempt(&attempt)?;
                db.set_copilot_mailbox_state(&attempt, "uncertain", attempt.error.as_deref())?;
                return Ok(true);
            }
            // Source is reserved here, never supplied by the extension or caller.
            db.conn.execute("INSERT INTO task_input(task_id,run_id,stage,source,message) VALUES(?,?,?,'engine',?)",
                params![attempt.binding.task_id, attempt.binding.run_id, attempt.stage, attempt.message])?;
            let input_id = db.conn.last_insert_rowid();
            attempt.input_id = Some(input_id);
            attempt.message_id = message_id.map(str::to_owned);
            attempt.event_id = event_id.map(str::to_owned);
            attempt.error = None;
            let (preview, truncated) = super::task_inputs::preview_of(&attempt.message);
            db.append_task_event(&attempt.binding.task_id, TaskEventKind::InputDelivered, json!({
                "inputId":input_id, "source":"engine", "runId":attempt.binding.run_id,
                "stage":attempt.stage, "preview":preview, "truncated":truncated,
                "delivery":"copilot_enqueue", "attemptId":attempt.id,
                "nativeMessageId":attempt.message_id, "nativeEventId":attempt.event_id,
            }))?;
            db.save_copilot_attempt(&attempt)?;
            db.set_copilot_mailbox_state(&attempt, "notified", None)?;
            Ok(true)
        })
    }
}

pub(super) fn create_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE copilot_wake_registration (
                run_id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
                record TEXT NOT NULL
            );
            CREATE TABLE copilot_wake_attempt (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
                subscription_id TEXT NOT NULL,
                batch_id INTEGER NOT NULL,
                record TEXT NOT NULL,
                UNIQUE(subscription_id, batch_id)
            );
            CREATE INDEX copilot_wake_attempt_run ON copilot_wake_attempt(run_id);",
    )
}
