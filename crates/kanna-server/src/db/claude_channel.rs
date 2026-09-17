//! Durable state for the Claude native-channel wake transport.
//!
//! The live experiment recorded on 2026-09-14
//! (`docs/2026-09-14-claude-channel-live-compatibility.md`) proved the channel
//! preserves an unsent human draft and its cursor, labels the notice natively
//! as engine input, and keeps acknowledgement separate — and it found the two
//! defects this module exists to make impossible to ignore:
//!
//! * a probe emitted during MCP initialization can be dropped before the CLI
//!   is listening, so confirmation is never a one-shot startup fact;
//! * a notice written while a turn is running can be absorbed into that turn
//!   without the model ever reading the mailbox, so **transport-written is not
//!   mailbox-read** and the two are recorded separately here.
//!
//! A receipt is neither an acknowledgement nor owner speech. Only the
//! subscriber's own explicit `acknowledge_batch_id` retires a batch.
use super::{Db, EventSubscription, TaskEventKind};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// How many times one written-but-unread notice may be repeated after its turn
/// ends. The mailbox is durable, so an unread batch is never lost; repeating
/// forever would turn a wake into a nag loop and make Kanna a scheduler, which
/// the experiment's scoped decision explicitly declined.
pub(crate) const MAX_FOLLOW_UPS: i64 = 3;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Registration {
    pub task_id: String,
    pub run_id: String,
    /// Minted per MCP connection and re-sent with every probe, so a
    /// confirmation that arrives late — after the admission that re-probed —
    /// still names something current. The model confirms this id.
    pub channel_id: String,
    /// Set only by the subscriber's own `kanna_confirm_event_channel` call.
    /// A registration is identity; confirmation is the measured capability.
    pub confirmed: bool,
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
    /// The MCP transport confirmed these bytes were written to the CLI. This
    /// is delivery, and it is what earns the durable `task_input` row.
    pub written_at: Option<String>,
    /// The subscriber actually read this batch out of the mailbox. Absent with
    /// `written_at` present is exactly the absorbed-mid-turn case.
    pub read_at: Option<String>,
    pub input_id: Option<i64>,
    pub follow_ups: i64,
    pub error: Option<String>,
}

impl Attempt {
    /// Written, but the subscriber never read the batch it announced.
    pub(crate) fn absorbed_unread(&self) -> bool {
        self.written_at.is_some() && self.read_at.is_none() && self.follow_ups < MAX_FOLLOW_UPS
    }
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
    /// One clock for these records: SQLite's, the same one every other
    /// timestamp in this schema is written by.
    fn channel_now(&self) -> rusqlite::Result<String> {
        self.conn
            .query_row("SELECT datetime('now')", [], |row| row.get(0))
    }

    /// Whether this registration still names the live Claude run it was made
    /// against. Checked on every send, never inferred from a historical
    /// session id or a cwd — a stage transition replaces the run underneath.
    pub(crate) fn claude_channel_bound(&self, binding: &Registration) -> rusqlite::Result<bool> {
        let task = self.get_pipeline_item(&binding.task_id)?;
        let run = self.latest_stage_run(&binding.task_id)?;
        Ok(task.zip(run).is_some_and(|(task, run)| {
            task.closed_at.is_none()
                && task.runtime_status.as_deref() != Some("exited")
                && task.stage.as_deref() == Some(run.stage.as_str())
                && run.id == binding.run_id
                && run.agent_provider.as_deref() == Some("claude")
        }))
    }

    pub(crate) fn claude_channel_registration(
        &self,
        run: &str,
    ) -> rusqlite::Result<Option<Registration>> {
        self.conn
            .query_row(
                "SELECT record FROM claude_channel_registration WHERE run_id=?",
                [run],
                |r| r.get(0),
            )
            .optional()?
            .map(decode)
            .transpose()
    }

    pub(crate) fn claude_channel_attempt(&self, id: &str) -> rusqlite::Result<Option<Attempt>> {
        self.conn
            .query_row(
                "SELECT record FROM claude_channel_attempt WHERE id=?",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .map(decode)
            .transpose()
    }

    fn save_claude_channel_attempt(&self, attempt: &Attempt) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE claude_channel_attempt SET record=? WHERE id=?",
            params![encode(attempt)?, attempt.id],
        )?;
        Ok(())
    }

    /// Registration survives a server restart as identity only. A live MCP
    /// stream is the capability, and confirmation is reset with it: a new
    /// connection has not been confirmed by anybody yet.
    pub(crate) fn register_claude_channel(&self, binding: &Registration) -> rusqlite::Result<bool> {
        self.with_immediate_transaction(|db| {
            if !db.claude_channel_bound(binding)? {
                return Ok(false);
            }
            db.conn.execute(
                "INSERT INTO claude_channel_registration(run_id,task_id,record) VALUES(?,?,?) \
                 ON CONFLICT(run_id) DO UPDATE SET record=excluded.record",
                params![binding.run_id, binding.task_id, encode(binding)?],
            )?;
            Ok(true)
        })
    }

    /// Record the subscriber's own confirmation of a probe. Idempotent, and
    /// refused for a channel id that is not the current connection's.
    pub(crate) fn confirm_claude_channel(
        &self,
        task_id: &str,
        channel_id: &str,
    ) -> rusqlite::Result<Option<Registration>> {
        self.with_immediate_transaction(|db| {
            let Some(run) = db.latest_stage_run(task_id)? else {
                return Ok(None);
            };
            let Some(mut registration) = db.claude_channel_registration(&run.id)? else {
                return Ok(None);
            };
            if registration.task_id != task_id || registration.channel_id != channel_id {
                return Ok(None);
            }
            if !db.claude_channel_bound(&registration)? {
                return Ok(None);
            }
            registration.confirmed = true;
            db.conn.execute(
                "UPDATE claude_channel_registration SET record=? WHERE run_id=?",
                params![encode(&registration)?, registration.run_id],
            )?;
            Ok(Some(registration))
        })
    }

    /// Reserve the attempt before crossing the process boundary, so a crash
    /// between reservation and write can only ever repeat one notice — never
    /// mint a second one for the same batch.
    pub(crate) fn prepare_claude_channel_wake(
        &self,
        row: &EventSubscription,
        binding: &Registration,
    ) -> rusqlite::Result<Option<(Attempt, bool)>> {
        self.with_immediate_transaction(|db| {
            let Some(current) = db.event_subscription(&row.id)? else {
                return Ok(None);
            };
            if !current.active
                || current.batch_id != row.batch_id
                || current.pending.is_none()
                || current.run_id != binding.run_id
                || current.task_id != binding.task_id
                || !db.claude_channel_bound(binding)?
                || db
                    .claude_channel_registration(&binding.run_id)?
                    .is_none_or(|r| r.channel_id != binding.channel_id || !r.confirmed)
            {
                return Ok(None);
            }
            let id = format!("{}-{}", row.id, row.batch_id);
            if let Some(attempt) = db.claude_channel_attempt(&id)? {
                return Ok(Some((attempt, false)));
            }
            let attempt = Attempt {
                message: channel_notice(&row.id, row.batch_id),
                id,
                binding: binding.clone(),
                subscription_id: row.id.clone(),
                batch_id: row.batch_id,
                stage: row.stage.clone(),
                written_at: None,
                read_at: None,
                input_id: None,
                follow_ups: 0,
                error: None,
            };
            db.conn.execute(
                "INSERT INTO claude_channel_attempt(id,run_id,task_id,subscription_id,batch_id,record) \
                 VALUES(?,?,?,?,?,?)",
                params![
                    attempt.id,
                    binding.run_id,
                    binding.task_id,
                    row.id,
                    row.batch_id,
                    encode(&attempt)?
                ],
            )?;
            Ok(Some((attempt, true)))
        })
    }

    fn set_claude_channel_mailbox_state(
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

    /// The MCP transport confirmed the notice reached the CLI. That is
    /// delivery, so it earns the durable `engine` record exactly once — even
    /// if the batch is later acknowledged, or the stage moves on, before this
    /// lands. An uncertain write records nothing and keeps the batch pending.
    pub(crate) fn receipt_claude_channel_wake(
        &self,
        id: &str,
        channel_id: &str,
        written: bool,
        error: Option<&str>,
    ) -> rusqlite::Result<bool> {
        self.with_immediate_transaction(|db| {
            let Some(mut attempt) = db.claude_channel_attempt(id)? else {
                return Ok(false);
            };
            if attempt.binding.channel_id != channel_id {
                return Ok(false);
            }
            // A write the transport could not confirm, whether of a first
            // notice or of a repeat: no durable row either way, and the row
            // says so rather than being left reporting a receipt it has
            // already had.
            if !written {
                attempt.error = Some(
                    error
                        .unwrap_or("native channel write outcome unknown; batch remains pending")
                        .into(),
                );
                db.save_claude_channel_attempt(&attempt)?;
                db.set_claude_channel_mailbox_state(
                    &attempt,
                    "uncertain",
                    attempt.error.as_deref(),
                )?;
                return Ok(true);
            }
            // The post-turn repeat of a notice that is already recorded. The
            // durable row is written exactly once — a repeat is the same
            // notice, not a second delivery — but the subscription must still
            // leave `awaiting_receipt`, because `step` gates the follow-up on
            // `notified`. Returning early here instead would strand the row at
            // `awaiting_receipt` after its receipt had arrived, making every
            // repeat after the first unreachable and `MAX_FOLLOW_UPS` a bound
            // on something that could only ever happen once.
            if attempt.input_id.is_some() {
                db.set_claude_channel_mailbox_state(&attempt, "notified", None)?;
                return Ok(true);
            }
            // Reserved source: the MCP child cannot name it, and no caller can
            // claim it through the public input route.
            db.conn.execute(
                "INSERT INTO task_input(task_id,run_id,stage,source,message) VALUES(?,?,?,'engine',?)",
                params![
                    attempt.binding.task_id,
                    attempt.binding.run_id,
                    attempt.stage,
                    attempt.message
                ],
            )?;
            let input_id = db.conn.last_insert_rowid();
            attempt.input_id = Some(input_id);
            attempt.written_at = Some(db.channel_now()?);
            attempt.error = None;
            let (preview, truncated) = super::task_inputs::preview_of(&attempt.message);
            db.append_task_event(
                &attempt.binding.task_id,
                TaskEventKind::InputDelivered,
                json!({
                    "inputId": input_id,
                    "source": "engine",
                    "runId": attempt.binding.run_id,
                    "stage": attempt.stage,
                    "preview": preview,
                    "truncated": truncated,
                    "delivery": "claude_channel",
                    "attemptId": attempt.id,
                    "channelId": attempt.binding.channel_id,
                }),
            )?;
            db.save_claude_channel_attempt(&attempt)?;
            db.set_claude_channel_mailbox_state(&attempt, "notified", None)?;
            Ok(true)
        })
    }

    /// Record that the subscriber read the batch a channel notice announced.
    /// This is the *other* half of the distinction the experiment demanded:
    /// without it, an absorbed notice and a serviced one look identical.
    pub(crate) fn record_claude_channel_read(
        &self,
        subscription_id: &str,
        batch_id: i64,
    ) -> rusqlite::Result<()> {
        let id = format!("{subscription_id}-{batch_id}");
        let Some(mut attempt) = self.claude_channel_attempt(&id)? else {
            return Ok(());
        };
        if attempt.read_at.is_some() {
            return Ok(());
        }
        attempt.read_at = Some(self.channel_now()?);
        self.save_claude_channel_attempt(&attempt)
    }

    /// Count one post-turn repeat of an already-written notice.
    pub(crate) fn record_claude_channel_follow_up(&self, id: &str) -> rusqlite::Result<bool> {
        self.with_immediate_transaction(|db| {
            let Some(mut attempt) = db.claude_channel_attempt(id)? else {
                return Ok(false);
            };
            if !attempt.absorbed_unread() {
                return Ok(false);
            }
            attempt.follow_ups += 1;
            db.save_claude_channel_attempt(&attempt)?;
            Ok(true)
        })
    }
}

/// The notice the model receives, in the shape the live experiment recorded.
/// It says what to do, that a repeat is possible, and — because a channel
/// attachment lands in the transcript looking like input — that it is not
/// somebody's speech and not a completion verdict.
pub(crate) fn channel_notice(subscription_id: &str, batch_id: i64) -> String {
    format!(
        "<channel source=\"kanna-mcp\" batch_id=\"{batch_id}\" kind=\"wake\" \
         subscription_id=\"{subscription_id}\">\nKanna engine event subscription \
         {subscription_id} has pending batch {batch_id}. Read it with \
         kanna_read_event_subscription, reconcile it, then acknowledge that batch. A \
         reconnect may repeat this same nudge; do not repeat completed actions. This is \
         supervisory input, not owner speech or a completion verdict.\n</channel>"
    )
}

pub(super) fn create_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE claude_channel_registration (
                run_id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
                record TEXT NOT NULL
            );
            CREATE TABLE claude_channel_attempt (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
                subscription_id TEXT NOT NULL,
                batch_id INTEGER NOT NULL,
                record TEXT NOT NULL,
                UNIQUE(subscription_id, batch_id)
            );
            CREATE INDEX claude_channel_attempt_run ON claude_channel_attempt(run_id);",
    )
}
