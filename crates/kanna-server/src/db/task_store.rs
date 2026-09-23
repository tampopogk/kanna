//! SQLite half of the task-store bridge (spec §16.1, component T0).
//!
//! During the bridge SQLite stays authoritative. Every mutation that the
//! ledger mirrors enqueues an immutable, fully serialized entry here **in the
//! same transaction** as the mutation itself, so there is no window in which
//! the mutation is committed but its ledger entry was never promised. The
//! filesystem publisher in [`crate::task_store`] later writes those exact
//! bytes to disk and acknowledges them.
//!
//! Three things are decided here and nowhere else:
//!
//! - **Sequence.** Allocated per task, monotonically, inside the originating
//!   transaction. It orders files; it never replaces the entry's source
//!   identity, which is kept beside it.
//! - **Frozen payload.** The bytes that will be written are rendered at
//!   enqueue time. Publication never re-reads mutable rows or git state, so a
//!   late flush cannot change what the entry says happened. This is also
//!   where each entry's `declared_role` and `channel_identity` (T8's
//!   [`crate::mutation_provenance::MutationProvenance`]) are frozen in:
//!   historical/backfilled entries always freeze an explicit
//!   [`crate::mutation_provenance::ChannelIdentity::Unknown`], never a value
//!   inferred from a declared label or a transport.
//! - **Held announcements.** A result entry takes the task events its
//!   originating mutation appended for the same task (`run.finished`,
//!   `task.revision_requested`) and re-appends them only when the entry is
//!   acknowledged, so nobody hears of a durable completion whose ledger file
//!   is not yet readable. Re-appending allocates a fresh event `seq`, which
//!   keeps the event cursor's no-skip guarantee intact. Transition, input and
//!   plan entries hold nothing: their events are appended with the SQL
//!   mutation that remains authoritative during the bridge, and their files
//!   follow immediately.

use super::{Db, TaskEventKind};
use crate::mutation_provenance::ChannelIdentity;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS task_ledger_entry (
        task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        sequence INTEGER NOT NULL,
        entry_id TEXT NOT NULL UNIQUE,
        -- NULL while the sequence is only reserved (see reserve_ledger_sequence).
        kind TEXT,
        operation_id TEXT,
        source_kind TEXT,
        source_id TEXT,
        file_name TEXT,
        payload BLOB,
        held_events TEXT,
        reserved_at TEXT,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        published_at TEXT,
        publish_error TEXT,
        PRIMARY KEY (task_id, sequence)
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_task_ledger_entry_source
        ON task_ledger_entry(task_id, source_kind, source_id)
        WHERE source_kind IS NOT NULL;
    CREATE INDEX IF NOT EXISTS idx_task_ledger_entry_pending
        ON task_ledger_entry(task_id, sequence)
        WHERE published_at IS NULL;
    CREATE TABLE IF NOT EXISTS task_ledger_snapshot (
        task_id TEXT PRIMARY KEY REFERENCES pipeline_item(id) ON DELETE CASCADE,
        revision INTEGER NOT NULL DEFAULT 1,
        published_revision INTEGER NOT NULL DEFAULT 0,
        publish_error TEXT
    );
    CREATE TABLE IF NOT EXISTS task_ledger_continuation (
        task_id TEXT PRIMARY KEY REFERENCES pipeline_item(id) ON DELETE CASCADE,
        operation_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE TABLE IF NOT EXISTS task_ledger_backfill (
        task_id TEXT PRIMARY KEY REFERENCES pipeline_item(id) ON DELETE CASCADE,
        imported_entries INTEGER NOT NULL,
        completed_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
"#;

/// The four immutable entry kinds of the ledger directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerEntryKind {
    Result,
    Input,
    Transition,
    Plan,
}

impl LedgerEntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Result => "result",
            Self::Input => "input",
            Self::Transition => "transition",
            Self::Plan => "plan",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "result" => Some(Self::Result),
            "input" => Some(Self::Input),
            "transition" => Some(Self::Transition),
            "plan" => Some(Self::Plan),
            _ => None,
        }
    }

    /// Results and inputs carry a verbatim message body, so they are
    /// Markdown with JSON front matter; the others are plain JSON.
    pub fn has_message_body(self) -> bool {
        matches!(self, Self::Result | Self::Input)
    }

    pub fn extension(self) -> &'static str {
        if self.has_message_body() {
            "md"
        } else {
            "json"
        }
    }
}

/// One entry to enqueue. `body` is the kind-specific object (for example the
/// `result` object); the common envelope is added here.
pub struct NewLedgerEntry<'a> {
    pub task_id: &'a str,
    pub kind: LedgerEntryKind,
    /// Groups the entries of one accepted mutation. `None` makes the entry
    /// its own operation, named after its entry id.
    pub operation_id: Option<&'a str>,
    pub source_kind: &'a str,
    pub source_id: &'a str,
    /// Provenance an import carried from another machine or database.
    pub source_origin: Option<Value>,
    /// Backfilled from history rather than observed as it happened.
    pub historical: bool,
    /// When the fact happened, for historical entries. `None` is now.
    pub recorded_at: Option<&'a str>,
    pub run_id: Option<&'a str>,
    /// A role the caller genuinely declared (`operator`, `manager`, `agent`),
    /// never inferred from a transport or a source label.
    pub declared_role: Option<&'a str>,
    /// The channel this server verified the mutation arrived on (T8). Always
    /// explicit: [`ChannelIdentity::Unknown`] for historical/backfilled
    /// entries, never guessed from `declared_role` or the transport.
    pub channel_identity: &'a ChannelIdentity,
    pub body: Value,
    /// Verbatim message body for result and input entries.
    pub message: Option<&'a str>,
    /// Hold this task's events appended after this `task_event.seq` until the
    /// entry is published. `None` holds nothing.
    pub hold_events_after: Option<i64>,
    /// A sequence reserved earlier by [`Db::reserve_ledger_sequence`].
    pub reserved_sequence: Option<i64>,
}

/// Where an enqueued entry will live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntryRef {
    pub sequence: i64,
    pub entry_id: String,
    pub file_name: String,
    pub operation_id: String,
}

/// One outbox row the publisher still has to write.
#[derive(Debug, Clone)]
pub struct PendingLedgerEntry {
    pub sequence: i64,
    pub entry_id: String,
    /// `None` for a sequence that is reserved but not yet filled: publication
    /// stops there so files always appear in sequence order.
    pub file_name: Option<String>,
    pub payload: Option<Vec<u8>>,
}

/// A durable promise to continue an accepted operation once its ledger
/// entries are published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerContinuation {
    pub task_id: String,
    pub operation_id: String,
    pub kind: String,
    pub payload: Value,
}

/// Dispatch the stage transition a recorded completion asked for.
pub const STAGE_COMPLETION_CONTINUATION: &str = "stage_completion";

/// Start the reviser an accepted revision request owes (its round is already
/// spent and the reviewer's run already finished).
pub const REVISION_CONTINUATION: &str = "revision";

/// The envelope's session reference. T0 defined `{kind: "stage_run", id}`;
/// T2 adds the session's identity beside those keys when the run recorded
/// one — `workspace_id`, `branch`, `name` and `transcript {provider,
/// session_id, path}` — and never changes the two it inherited. Runs from
/// before session identity keep exactly T0's shape.
fn session_ref(db: &Db, run_id: &str) -> Result<Value, rusqlite::Error> {
    let mut reference = json!({ "kind": "stage_run", "id": run_id });
    if let Some(session) = db.stage_run_session(run_id)? {
        let object = reference
            .as_object_mut()
            .expect("session reference is an object");
        if let Some(workspace_id) = session.workspace_id {
            object.insert("workspace_id".into(), Value::String(workspace_id));
        }
        if let Some(branch) = session.branch {
            object.insert("branch".into(), Value::String(branch));
        }
        if let Some(name) = session.name {
            object.insert("name".into(), Value::String(name));
        }
        if let Some(transcript) = session.transcript {
            object.insert(
                "transcript".into(),
                json!({
                    "provider": transcript.provider,
                    "session_id": transcript.session_id,
                    "path": transcript.path,
                }),
            );
        }
    }
    Ok(reference)
}

pub fn ledger_entry_id(task_id: &str, sequence: i64) -> String {
    format!("{task_id}-{sequence:06}")
}

pub fn ledger_file_name(sequence: i64, kind: LedgerEntryKind) -> String {
    format!("{sequence:06}-{}.{}", kind.as_str(), kind.extension())
}

/// Render the immutable bytes of one entry. Markdown kinds put the envelope
/// in JSON front matter and the message verbatim after it; the reader in
/// `crate::task_store` inverts exactly this.
pub fn render_ledger_entry(envelope: &Value, message: Option<&str>) -> Vec<u8> {
    let json = serde_json::to_string_pretty(envelope).unwrap_or_else(|_| "{}".to_string());
    match message {
        Some(message) => format!("---\n{json}\n---\n\n{message}").into_bytes(),
        None => format!("{json}\n").into_bytes(),
    }
}

impl Db {
    /// Highest task-event sequence so far, for [`NewLedgerEntry::hold_events_after`].
    pub(crate) fn ledger_event_floor(&self) -> Result<i64, rusqlite::Error> {
        self.latest_task_event_seq()
    }

    /// Reserve the next sequence for an entry whose content is decided later
    /// in the same operation (a revision prepares the reviser's session before
    /// it records the reviewer's result). Publication stops at a reservation,
    /// so nothing enqueued after it can appear on disk first.
    pub(crate) fn reserve_ledger_sequence(&self, task_id: &str) -> Result<i64, rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            let sequence = db.next_ledger_sequence(task_id)?;
            db.conn.execute(
                "INSERT INTO task_ledger_entry (task_id, sequence, entry_id, reserved_at)
                 VALUES (?, ?, ?, datetime('now'))",
                params![task_id, sequence, ledger_entry_id(task_id, sequence)],
            )?;
            Ok(sequence)
        })
    }

    /// Give up a reservation the operation did not fill. The sequence becomes
    /// a gap, which readers treat as nothing.
    pub(crate) fn release_ledger_reservation(
        &self,
        task_id: &str,
        sequence: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM task_ledger_entry
             WHERE task_id = ? AND sequence = ? AND kind IS NULL",
            params![task_id, sequence],
        )?;
        crate::task_store::wake_publisher();
        Ok(())
    }

    /// Reservations cannot outlive the process that made them: its operation
    /// either filled the row or never will.
    pub(crate) fn release_stale_ledger_reservations(&self) -> Result<usize, rusqlite::Error> {
        self.conn
            .execute("DELETE FROM task_ledger_entry WHERE kind IS NULL", [])
    }

    fn next_ledger_sequence(&self, task_id: &str) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM task_ledger_entry WHERE task_id = ?",
            [task_id],
            |row| row.get(0),
        )
    }

    /// Enqueue one immutable entry inside the caller's transaction (or a new
    /// one). A replay of the same source identity returns the original entry
    /// and writes nothing.
    pub(crate) fn enqueue_ledger_entry(
        &self,
        entry: NewLedgerEntry<'_>,
    ) -> Result<LedgerEntryRef, rusqlite::Error> {
        self.enqueue_ledger_entry_with_artifacts(entry, None)
    }

    /// [`Db::enqueue_ledger_entry`] filling the envelope's `artifacts` with
    /// T6's named references (name → tagged reference) a result carried.
    /// `None` records `{}`.
    pub(crate) fn enqueue_ledger_entry_with_artifacts(
        &self,
        entry: NewLedgerEntry<'_>,
        artifacts: Option<&Value>,
    ) -> Result<LedgerEntryRef, rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            if let Some(existing) =
                db.ledger_entry_for_source(entry.task_id, entry.source_kind, entry.source_id)?
            {
                if let Some(sequence) = entry.reserved_sequence {
                    db.release_ledger_reservation(entry.task_id, sequence)?;
                }
                return Ok(existing);
            }
            let sequence = match entry.reserved_sequence {
                Some(sequence) => sequence,
                None => db.next_ledger_sequence(entry.task_id)?,
            };
            let entry_id = ledger_entry_id(entry.task_id, sequence);
            let operation_id = entry
                .operation_id
                .map(str::to_string)
                .unwrap_or_else(|| format!("op-{entry_id}"));
            let file_name = ledger_file_name(sequence, entry.kind);
            let recorded_at: String = match entry.recorded_at {
                Some(at) => at.to_string(),
                None => db.conn.query_row(
                    "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                    [],
                    |row| row.get(0),
                )?,
            };
            let mut body = entry.body;
            if entry.kind == LedgerEntryKind::Result {
                if let Some(object) = body.as_object_mut() {
                    object.insert("result_id".into(), Value::String(entry_id.clone()));
                    object
                        .entry("timestamp")
                        .or_insert_with(|| Value::String(recorded_at.clone()));
                }
            }
            let envelope = json!({
                "schema_version": crate::task_store::SCHEMA_VERSION,
                "entry_id": entry_id,
                "task_id": entry.task_id,
                "sequence": sequence,
                "kind": entry.kind.as_str(),
                "operation_id": operation_id,
                "source": {
                    "kind": entry.source_kind,
                    "id": entry.source_id,
                    "origin": entry.source_origin,
                },
                "recorded_at": recorded_at,
                "historical": entry.historical,
                "run_id": entry.run_id,
                "session_ref": match entry.run_id {
                    Some(run_id) => session_ref(db, run_id)?,
                    None => Value::Null,
                },
                "declared_role": entry.declared_role,
                "channel_identity": entry.channel_identity.to_json(),
                "artifacts": artifacts.cloned().unwrap_or_else(|| json!({})),
                entry.kind.as_str(): body,
            });
            let payload = render_ledger_entry(
                &envelope,
                entry
                    .kind
                    .has_message_body()
                    .then(|| entry.message.unwrap_or("")),
            );
            let held_events = match entry.hold_events_after {
                Some(floor) => db.take_task_events_after(entry.task_id, floor)?,
                None => None,
            };
            let changed = if entry.reserved_sequence.is_some() {
                db.conn.execute(
                    "UPDATE task_ledger_entry
                     SET kind = ?, operation_id = ?, source_kind = ?, source_id = ?,
                         file_name = ?, payload = ?, held_events = ?, reserved_at = NULL
                     WHERE task_id = ? AND sequence = ? AND kind IS NULL",
                    params![
                        entry.kind.as_str(),
                        operation_id,
                        entry.source_kind,
                        entry.source_id,
                        file_name,
                        payload,
                        held_events,
                        entry.task_id,
                        sequence,
                    ],
                )?
            } else {
                db.conn.execute(
                    "INSERT INTO task_ledger_entry
                     (task_id, sequence, entry_id, kind, operation_id, source_kind, source_id,
                      file_name, payload, held_events)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        entry.task_id,
                        sequence,
                        entry_id,
                        entry.kind.as_str(),
                        operation_id,
                        entry.source_kind,
                        entry.source_id,
                        file_name,
                        payload,
                        held_events,
                    ],
                )?
            };
            if changed != 1 {
                return Err(rusqlite::Error::InvalidParameterName(format!(
                    "ledger sequence {sequence} of task {} is not reserved",
                    entry.task_id
                )));
            }
            db.mark_task_snapshot_dirty(entry.task_id)?;
            // A join member's first result resolves it and is delivered to
            // its parent in this same transaction (T5).
            if entry.kind == LedgerEntryKind::Result && !entry.historical {
                if let Some(result) = envelope.get(entry.kind.as_str()) {
                    db.resolve_join_member_on_result(
                        entry.task_id,
                        &entry_id,
                        result,
                        entry.message.unwrap_or(""),
                    )?;
                }
            }
            crate::task_store::wake_publisher();
            Ok(LedgerEntryRef {
                sequence,
                entry_id,
                file_name,
                operation_id,
            })
        })
    }

    pub(crate) fn ledger_entry_for_source(
        &self,
        task_id: &str,
        source_kind: &str,
        source_id: &str,
    ) -> Result<Option<LedgerEntryRef>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT sequence, entry_id, file_name, operation_id FROM task_ledger_entry
                 WHERE task_id = ? AND source_kind = ? AND source_id = ?",
                params![task_id, source_kind, source_id],
                |row| {
                    Ok(LedgerEntryRef {
                        sequence: row.get(0)?,
                        entry_id: row.get(1)?,
                        file_name: row.get(2)?,
                        operation_id: row.get(3)?,
                    })
                },
            )
            .optional()
    }

    /// How many result entries a run already has; the next one is a
    /// correction, not a retry, and gets its own source identity.
    pub(crate) fn ledger_result_count_for_run(
        &self,
        task_id: &str,
        run_id: &str,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM task_ledger_entry
             WHERE task_id = ?1 AND kind = 'result' AND source_kind = 'stage_run'
               AND (source_id = ?2 OR substr(source_id, 1, length(?2) + 1) = ?2 || '#')",
            params![task_id, run_id],
            |row| row.get(0),
        )
    }

    /// Did work reach `run_id` after its latest result? True when tool input
    /// was delivered to the task after that result, or when the run's
    /// workspace now holds a different commit than the result recorded.
    ///
    /// Completion treats a byte-identical result as a retry of the one already
    /// recorded; a person who kept working with a parked session and has it
    /// record the same words again is making a new result, and this is how
    /// the two are told apart (T1). `false` when the run has no result entry.
    pub(crate) fn ledger_work_after_latest_result(
        &self,
        task_id: &str,
        run_id: &str,
        observed_sha: Option<&str>,
    ) -> Result<bool, rusqlite::Error> {
        let latest = self
            .conn
            .query_row(
                "SELECT sequence, file_name, payload FROM task_ledger_entry
                 WHERE task_id = ?1 AND kind = 'result' AND source_kind = 'stage_run'
                   AND (source_id = ?2 OR substr(source_id, 1, length(?2) + 1) = ?2 || '#')
                 ORDER BY sequence DESC LIMIT 1",
                params![task_id, run_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((sequence, file_name, payload)) = latest else {
            return Ok(false);
        };
        let input_since: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_ledger_entry
                           WHERE task_id = ? AND kind = 'input' AND sequence > ?)",
            params![task_id, sequence],
            |row| row.get(0),
        )?;
        if input_since {
            return Ok(true);
        }
        let recorded_sha = crate::task_store::parse_ledger_file(&file_name, &payload)
            .ok()
            .and_then(|entry| entry.body()["committed_sha"].as_str().map(str::to_string));
        Ok(match (recorded_sha.as_deref(), observed_sha) {
            (Some(recorded), Some(observed)) => recorded != observed,
            _ => false,
        })
    }

    /// The result that caused the transition being recorded now: the newest
    /// result entered since the task's previous transition. `None` when no
    /// result was recorded in between (a manual advance without one); a
    /// trigger is never borrowed from an earlier stage.
    pub(crate) fn ledger_transition_trigger(
        &self,
        task_id: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT entry_id FROM task_ledger_entry
                 WHERE task_id = ?1 AND kind = 'result'
                   AND sequence > COALESCE(
                       (SELECT MAX(sequence) FROM task_ledger_entry
                        WHERE task_id = ?1 AND kind = 'transition'),
                       0)
                 ORDER BY sequence DESC LIMIT 1",
                [task_id],
                |row| row.get(0),
            )
            .optional()
    }

    /// Move this task's events appended after `floor` off the feed and return
    /// them, oldest first, for release on publication.
    fn take_task_events_after(
        &self,
        task_id: &str,
        floor: i64,
    ) -> Result<Option<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT type, payload FROM task_event
             WHERE task_id = ? AND seq > ? ORDER BY seq ASC",
        )?;
        let events = stmt
            .query_map(params![task_id, floor], |row| {
                Ok(json!({
                    "type": row.get::<_, String>(0)?,
                    "payload": row.get::<_, Option<String>>(1)?,
                }))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if events.is_empty() {
            return Ok(None);
        }
        self.conn.execute(
            "DELETE FROM task_event WHERE task_id = ? AND seq > ?",
            params![task_id, floor],
        )?;
        Ok(Some(Value::Array(events).to_string()))
    }

    /// Unpublished entries of one task, in sequence order.
    pub(crate) fn pending_ledger_entries(
        &self,
        task_id: &str,
    ) -> Result<Vec<PendingLedgerEntry>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT sequence, entry_id, file_name, payload FROM task_ledger_entry
             WHERE task_id = ? AND published_at IS NULL
             ORDER BY sequence ASC",
        )?;
        let rows = stmt.query_map([task_id], |row| {
            Ok(PendingLedgerEntry {
                sequence: row.get(0)?,
                entry_id: row.get(1)?,
                file_name: row.get(2)?,
                payload: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// Record that the entry's file is durable and release the announcements
    /// it held, in one transaction. Idempotent: a second acknowledgement
    /// releases nothing.
    pub(crate) fn acknowledge_ledger_entry(
        &self,
        task_id: &str,
        sequence: i64,
    ) -> Result<bool, rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            let held: Option<Option<String>> = db
                .conn
                .query_row(
                    "SELECT held_events FROM task_ledger_entry
                     WHERE task_id = ? AND sequence = ? AND published_at IS NULL
                       AND kind IS NOT NULL",
                    params![task_id, sequence],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(held) = held else {
                return Ok(false);
            };
            db.conn.execute(
                "UPDATE task_ledger_entry
                 SET published_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     publish_error = NULL, held_events = NULL
                 WHERE task_id = ? AND sequence = ?",
                params![task_id, sequence],
            )?;
            if let Some(held) = held {
                let events: Vec<Value> = serde_json::from_str(&held).unwrap_or_default();
                for event in events {
                    let Some(event_type) = event.get("type").and_then(Value::as_str) else {
                        continue;
                    };
                    db.append_raw_task_event(
                        task_id,
                        event_type,
                        event.get("payload").and_then(Value::as_str),
                    )?;
                }
            }
            Ok(true)
        })
    }

    pub(crate) fn record_ledger_publish_error(
        &self,
        task_id: &str,
        sequence: i64,
        error: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE task_ledger_entry SET publish_error = ? WHERE task_id = ? AND sequence = ?",
            params![error, task_id, sequence],
        )?;
        Ok(())
    }

    /// The first unpublished entry's error, which is what blocks the task.
    #[cfg(test)]
    pub(crate) fn ledger_pending_error(
        &self,
        task_id: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT publish_error FROM task_ledger_entry
                 WHERE task_id = ? AND published_at IS NULL
                 ORDER BY sequence ASC LIMIT 1",
                [task_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(Option::flatten)
    }

    pub(crate) fn ledger_published_through(&self, task_id: &str) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COALESCE(MAX(sequence), 0) FROM task_ledger_entry
             WHERE task_id = ? AND published_at IS NOT NULL",
            [task_id],
            |row| row.get(0),
        )
    }

    /// Every task with an unpublished filled entry or a stale `task.json`.
    pub(crate) fn ledger_tasks_with_pending_work(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id FROM task_ledger_entry
             WHERE published_at IS NULL AND kind IS NOT NULL
             UNION
             SELECT task_id FROM task_ledger_snapshot
             WHERE published_revision < revision
             ORDER BY task_id",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// `task.json` is a replaceable projection: any change to what it shows
    /// bumps its revision in the mutation's own transaction, and the publisher
    /// rewrites it from current rows.
    pub(crate) fn mark_task_snapshot_dirty(&self, task_id: &str) -> Result<(), rusqlite::Error> {
        let changed = self.conn.execute(
            "INSERT INTO task_ledger_snapshot (task_id)
             SELECT id FROM pipeline_item WHERE id = ?1
             ON CONFLICT(task_id) DO UPDATE SET revision = revision + 1",
            [task_id],
        );
        match changed {
            Ok(_) => {
                crate::task_store::wake_publisher();
                Ok(())
            }
            // Schema-only fixtures that predate the bridge have nothing to
            // mark, and marking is never what a mutation is about.
            Err(error) if is_missing_ledger_table(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn task_snapshot_revisions(
        &self,
        task_id: &str,
    ) -> Result<Option<(i64, i64)>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT revision, published_revision FROM task_ledger_snapshot WHERE task_id = ?",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
    }

    pub(crate) fn acknowledge_task_snapshot(
        &self,
        task_id: &str,
        revision: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE task_ledger_snapshot
             SET published_revision = MAX(published_revision, ?), publish_error = NULL
             WHERE task_id = ?",
            params![revision, task_id],
        )?;
        Ok(())
    }

    pub(crate) fn record_task_snapshot_error(
        &self,
        task_id: &str,
        error: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE task_ledger_snapshot SET publish_error = ? WHERE task_id = ?",
            params![error, task_id],
        )?;
        Ok(())
    }

    /// The facts `task.json` projects, read from current rows.
    pub(crate) fn task_snapshot_facts(
        &self,
        task_id: &str,
    ) -> Result<Option<Value>, rusqlite::Error> {
        let Some(item) = self.get_pipeline_item(task_id)? else {
            return Ok(None);
        };
        let dependencies = self.list_task_blocker_ids(task_id)?;
        let (revision, _) = self.task_snapshot_revisions(task_id)?.unwrap_or((0, 0));
        let pinned = item
            .pipeline_def
            .as_deref()
            .map(|definition| serde_json::from_str::<Value>(definition).unwrap_or(Value::Null));
        Ok(Some(json!({
            "schema_version": crate::task_store::SCHEMA_VERSION,
            "task_id": item.id,
            "repo_id": item.repo_id,
            "title": item.display_name,
            "origin_prompt": item.prompt,
            "workflow": {
                "name": item.pipeline,
                "definition": pinned,
            },
            "links": {
                "parent": item.parent_task_id,
                "dependencies": dependencies,
                // T4 stage edges into this task, in edge order, with what
                // each consumed and what superseded it. `dependencies`
                // above keeps listing the legacy task-level blockers.
                "stage_dependencies": self.stage_edge_links(task_id)?,
                // T5 subtask joins this task created, with each member's
                // outcome and the input that delivered it.
                "subtask_joins": self.subtask_join_links(task_id)?,
                "pr": item.pr_url.as_ref().map(|url| json!({
                    "url": url,
                    "number": item.pr_number,
                    // The head sha is T4/T9 territory; not observed here.
                    "head_sha": Value::Null,
                })),
            },
            "stage": item.stage,
            "branch": item.branch,
            "base_ref": item.base_ref,
            // One owning machine per task (§11). The bridge does not yet
            // record it per task, so it says so rather than guessing.
            "owning_machine": Value::Null,
            "created_at": item.created_at,
            "updated_at": item.updated_at,
            "closed_at": item.closed_at,
            "snapshot_revision": revision,
            "ledger": {
                "published_through": self.ledger_published_through(task_id)?,
            },
        })))
    }

    /// Replace the task's pending continuation. At most one is outstanding:
    /// a corrected verdict supersedes what the earlier one asked for.
    pub(crate) fn put_ledger_continuation(
        &self,
        task_id: &str,
        operation_id: &str,
        kind: &str,
        payload: &Value,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO task_ledger_continuation (task_id, operation_id, kind, payload)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(task_id) DO UPDATE SET
                operation_id = excluded.operation_id,
                kind = excluded.kind,
                payload = excluded.payload,
                created_at = datetime('now')",
            params![task_id, operation_id, kind, payload.to_string()],
        )?;
        Ok(())
    }

    /// The task's run generation: the highest agent stage-run rowid, or 0
    /// when it has none. Every lifecycle operation that replaces the task's
    /// session (a spawn, resume, rerun, transition or revision) inserts a run
    /// and so advances it; a continuation is fenced to the generation it was
    /// accepted against.
    pub(crate) fn task_run_generation(&self, task_id: &str) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            &format!(
                "SELECT COALESCE(MAX(rowid), 0) FROM stage_run
                 WHERE task_id = ? AND kind IN {}",
                super::stage_runs::AGENT_RUN_KINDS
            ),
            [task_id],
            |row| row.get(0),
        )
    }

    pub(crate) fn clear_ledger_continuation(&self, task_id: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM task_ledger_continuation WHERE task_id = ?",
            [task_id],
        )?;
        Ok(())
    }

    /// Take the task's continuation, but only once every entry it waited for
    /// is published. Taking it deletes it, so exactly one caller dispatches.
    pub(crate) fn claim_ledger_continuation(
        &self,
        task_id: &str,
    ) -> Result<Option<LedgerContinuation>, rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            let pending: i64 = db.conn.query_row(
                "SELECT COUNT(*) FROM task_ledger_entry
                 WHERE task_id = ? AND published_at IS NULL AND kind IS NOT NULL",
                [task_id],
                |row| row.get(0),
            )?;
            if pending > 0 {
                return Ok(None);
            }
            let continuation = db
                .conn
                .query_row(
                    "SELECT operation_id, kind, payload FROM task_ledger_continuation
                     WHERE task_id = ?",
                    [task_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            let Some((operation_id, kind, payload)) = continuation else {
                return Ok(None);
            };
            db.clear_ledger_continuation(task_id)?;
            Ok(Some(LedgerContinuation {
                task_id: task_id.to_string(),
                operation_id,
                kind,
                payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
            }))
        })
    }

    pub(crate) fn ledger_continuation_task_ids(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT task_id FROM task_ledger_continuation ORDER BY created_at, task_id")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    #[cfg(test)]
    pub(crate) fn has_ledger_continuation(&self, task_id: &str) -> Result<bool, rusqlite::Error> {
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_ledger_continuation WHERE task_id = ?)",
            [task_id],
            |row| row.get(0),
        )
    }

    /// A task created after the bridge exists has no history to import.
    pub(crate) fn mark_task_ledger_backfilled(
        &self,
        task_id: &str,
        imported_entries: i64,
    ) -> Result<(), rusqlite::Error> {
        let result = self.conn.execute(
            "INSERT OR IGNORE INTO task_ledger_backfill (task_id, imported_entries) VALUES (?, ?)",
            params![task_id, imported_entries],
        );
        match result {
            Ok(_) => Ok(()),
            Err(error) if is_missing_ledger_table(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Open tasks whose history has not been imported into the ledger.
    pub(crate) fn tasks_needing_ledger_backfill(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM pipeline_item
             WHERE closed_at IS NULL
               AND id NOT IN (SELECT task_id FROM task_ledger_backfill)
             ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// Import a task's available history into its ledger exactly once.
    ///
    /// Sources, in actual historical order where timestamps allow and a
    /// deterministic order otherwise: finished runs that recorded a genuine
    /// verdict (never teardown or no-verdict endings), `task_input` rows, and
    /// the `stage.changed`/`task.workflow_changed` events retention kept.
    /// Every entry is marked `historical`; branch, committed SHA, triggering
    /// result and channel are recorded as unknown (null) rather than derived
    /// from today's git state. Gaps in retained history stay gaps: no
    /// transition is invented to connect them.
    pub(crate) fn backfill_task_ledger(&self, task_id: &str) -> Result<usize, rusqlite::Error> {
        self.with_immediate_transaction(|db| {
            let already: bool = db.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM task_ledger_backfill WHERE task_id = ?)",
                [task_id],
                |row| row.get(0),
            )?;
            if already {
                return Ok(0);
            }
            let mut history = Vec::new();
            for run in db.finished_stage_runs(task_id)? {
                if run.no_work_termination.is_some() || run.result.is_none() {
                    continue;
                }
                let at = run
                    .finished_at
                    .clone()
                    .unwrap_or_else(|| run.started_at.clone());
                history.push((at, 1, run.id.clone(), HistoricalFact::Result(Box::new(run))));
            }
            for input in db.list_all_task_inputs(task_id)? {
                history.push((
                    input.delivered_at.clone(),
                    0,
                    format!("{:020}", input.id),
                    HistoricalFact::Input(input),
                ));
            }
            let mut stmt = db.conn.prepare(
                "SELECT seq, type, payload, created_at FROM task_event
                 WHERE task_id = ? AND type IN (?, ?) ORDER BY seq ASC",
            )?;
            let events = stmt
                .query_map(
                    params![
                        task_id,
                        TaskEventKind::StageChanged.as_str(),
                        TaskEventKind::WorkflowChanged.as_str()
                    ],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?;
            drop(stmt);
            for (seq, event_type, payload, created_at) in events {
                let payload = payload
                    .and_then(|payload| serde_json::from_str::<Value>(&payload).ok())
                    .unwrap_or(Value::Null);
                history.push((
                    created_at,
                    2,
                    format!("{seq:020}"),
                    HistoricalFact::Event {
                        seq,
                        event_type,
                        payload,
                    },
                ));
            }
            // SQLite timestamps share one format, so they order as text; the
            // rank and source id make equal-second facts deterministic.
            history.sort_by(|a, b| (&a.0, a.1, &a.2).cmp(&(&b.0, b.1, &b.2)));
            let mut imported = 0usize;
            for (at, _, _, fact) in history {
                let recorded_at = sqlite_time_to_iso(&at);
                let entry = match &fact {
                    HistoricalFact::Result(run) => {
                        historical_result_entry(task_id, run, &recorded_at)
                    }
                    HistoricalFact::Input(input) => {
                        historical_input_entry(task_id, input, &recorded_at)
                    }
                    HistoricalFact::Event {
                        seq,
                        event_type,
                        payload,
                    } => historical_event_entry(task_id, *seq, event_type, payload, &recorded_at),
                };
                if db
                    .ledger_entry_for_source(task_id, entry.source_kind, &entry.source_id)?
                    .is_some()
                {
                    continue;
                }
                db.enqueue_ledger_entry(NewLedgerEntry {
                    task_id,
                    kind: entry.kind,
                    operation_id: None,
                    source_kind: entry.source_kind,
                    source_id: &entry.source_id,
                    source_origin: entry.source_origin,
                    historical: true,
                    recorded_at: Some(&recorded_at),
                    run_id: entry.run_id.as_deref(),
                    declared_role: entry.declared_role.as_deref(),
                    channel_identity: &ChannelIdentity::Unknown,
                    body: entry.body,
                    message: entry.message.as_deref(),
                    hold_events_after: None,
                    reserved_sequence: None,
                })?;
                imported += 1;
            }
            db.conn.execute(
                "INSERT INTO task_ledger_backfill (task_id, imported_entries) VALUES (?, ?)",
                params![task_id, imported as i64],
            )?;
            db.mark_task_snapshot_dirty(task_id)?;
            Ok(imported)
        })
    }
}

enum HistoricalFact {
    Result(Box<super::StageRun>),
    Input(super::TaskInputRecord),
    Event {
        seq: i64,
        event_type: String,
        payload: Value,
    },
}

struct HistoricalEntry {
    kind: LedgerEntryKind,
    source_kind: &'static str,
    source_id: String,
    source_origin: Option<Value>,
    run_id: Option<String>,
    declared_role: Option<String>,
    body: Value,
    message: Option<String>,
}

/// SQLite's `datetime('now')` shape to the ledger's ISO-8601 UTC shape. Any
/// other shape is kept verbatim: an unfamiliar timestamp is still evidence.
pub(crate) fn sqlite_time_to_iso(value: &str) -> String {
    if value.len() == 19 && value.as_bytes().get(10) == Some(&b' ') {
        format!("{}T{}Z", &value[..10], &value[11..])
    } else {
        value.to_string()
    }
}

/// A stage run's recorded result, normalized to status and message.
///
/// Recorded results are the `{status, summary, metadata}` JSON the result
/// call writes. Anything else is a legacy free-form result: its status comes
/// from the run's lifecycle column and its text is the message, flagged so a
/// reader knows the normalization was not the agent's own.
pub(crate) fn normalize_recorded_result(
    run_status: &str,
    result: &str,
) -> (String, String, Value, bool) {
    if let Ok(Value::Object(object)) = serde_json::from_str::<Value>(result) {
        if let Some(status) = object.get("status").and_then(Value::as_str) {
            let message = object
                .get("summary")
                .or_else(|| object.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let metadata = object.get("metadata").cloned().unwrap_or(Value::Null);
            return (status.to_string(), message, metadata, false);
        }
    }
    let status = if run_status == "succeeded" {
        "success"
    } else {
        "failure"
    };
    (status.to_string(), result.to_string(), Value::Null, true)
}

fn historical_result_entry(
    task_id: &str,
    run: &super::StageRun,
    recorded_at: &str,
) -> HistoricalEntry {
    let (status, message, metadata, legacy_format) =
        normalize_recorded_result(&run.status, run.result.as_deref().unwrap_or(""));
    let _ = task_id;
    HistoricalEntry {
        kind: LedgerEntryKind::Result,
        source_kind: "stage_run",
        source_id: run.id.clone(),
        source_origin: None,
        run_id: Some(run.id.clone()),
        declared_role: None,
        body: json!({
            "status": status,
            "stage": run.stage,
            "run_kind": run.kind,
            // Not captured when this result was recorded, and today's HEAD
            // is not evidence of what it was.
            "branch": Value::Null,
            "committed_sha": Value::Null,
            "provenance": { "branch": "unknown", "committed_sha": "unknown" },
            "timestamp": recorded_at,
            "metadata": metadata,
            "legacy_format": legacy_format,
            "request": { "kind": "historical" },
        }),
        message: Some(message),
    }
}

fn historical_input_entry(
    task_id: &str,
    input: &super::TaskInputRecord,
    recorded_at: &str,
) -> HistoricalEntry {
    let _ = task_id;
    HistoricalEntry {
        kind: LedgerEntryKind::Input,
        source_kind: "task_input",
        source_id: input.id.to_string(),
        source_origin: input.origin.as_ref().map(|origin| {
            json!({
                "peer_id": origin.peer_id,
                "task_id": origin.task_id,
                "input_id": origin.input_id,
                "run_id": origin.run_id,
            })
        }),
        run_id: input.run_id.clone(),
        declared_role: declared_input_role(&input.source),
        body: json!({
            "input_id": input.id,
            "source": input.source,
            "stage": input.stage,
            "delivered_at": recorded_at,
        }),
        message: Some(input.message.clone()),
    }
}

fn historical_event_entry(
    task_id: &str,
    seq: i64,
    event_type: &str,
    payload: &Value,
    recorded_at: &str,
) -> HistoricalEntry {
    let _ = task_id;
    if event_type == TaskEventKind::StageChanged.as_str() {
        let trigger = payload
            .get("trigger")
            .and_then(Value::as_str)
            .unwrap_or("unspecified");
        return HistoricalEntry {
            kind: LedgerEntryKind::Transition,
            source_kind: "task_event",
            source_id: seq.to_string(),
            source_origin: None,
            run_id: None,
            declared_role: declared_transition_role(trigger),
            body: json!({
                "from_stage": payload.get("fromStage"),
                "to_stage": payload.get("toStage"),
                "branch": payload.get("branch"),
                "trigger": trigger,
                "operation": "stage_change",
                // Which result caused a historical transition was never
                // recorded; the bridge does not guess.
                "triggering_result_id": Value::Null,
                "exit": Value::Null,
                "exit_source": Value::Null,
                "at": recorded_at,
            }),
            message: None,
        };
    }
    HistoricalEntry {
        kind: LedgerEntryKind::Plan,
        source_kind: "task_event",
        source_id: seq.to_string(),
        source_origin: None,
        run_id: None,
        declared_role: None,
        body: json!({
            "operation": payload.get("operation"),
            "source": payload.get("source"),
            "stage": payload.get("stage"),
            "from_workflow": payload.get("fromWorkflow"),
            "to_workflow": payload.get("toWorkflow"),
            "before": payload.get("beforeDefinition"),
            "after": payload.get("afterDefinition"),
            "superseded_run_ids": payload.get("supersededRunIds"),
            "changed_execution_stages": payload.get("changedExecutionStages"),
            "result_id": Value::Null,
        }),
        message: None,
    }
}

/// `operator` and `manager` are caller declarations; everything else
/// (`unspecified`, the engine's own wakes, retired labels) declares no role.
pub(crate) fn declared_input_role(source: &str) -> Option<String> {
    matches!(source, "operator" | "manager").then(|| source.to_string())
}

pub(crate) fn declared_transition_role(trigger: &str) -> Option<String> {
    matches!(trigger, "operator" | "manager").then(|| trigger.to_string())
}

/// A workflow edit's declared `source` additionally allows `agent` (the
/// `complete-stage` convention label for a combined verdict-and-publish); a
/// bare `unspecified` declares no role, same as elsewhere.
pub(crate) fn declared_workflow_role(source: &str) -> Option<String> {
    matches!(source, "operator" | "manager" | "agent").then(|| source.to_string())
}

fn is_missing_ledger_table(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::SqliteFailure(_, Some(message))
        if message.contains("no such table: task_ledger"))
}

#[cfg(test)]
impl Db {
    pub(crate) fn count_task_events_of_type_for_tests(
        &self,
        task_id: &str,
        event_type: &str,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM task_event WHERE task_id = ? AND type = ?",
            params![task_id, event_type],
            |row| row.get(0),
        )
    }

    /// Model a database whose history predates the bridge.
    pub(crate) fn delete_ledger_entries_for_tests(
        &self,
        task_id: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn
            .execute("DELETE FROM task_ledger_entry WHERE task_id = ?", [task_id])?;
        self.conn.execute(
            "DELETE FROM task_ledger_backfill WHERE task_id = ?",
            [task_id],
        )?;
        Ok(())
    }
}
