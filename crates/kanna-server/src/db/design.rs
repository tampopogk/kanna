//! Durable App Design state (docs/specs/app-design.md): one design session
//! per task, the live document's accepted updates, comment threads, the
//! feedback outbox that queues them to the live agent session, idempotent
//! agent operations, and approvals with their hand-off phases.
//!
//! Every table but `design_doc_update` is carried in `task.json`
//! ([`super::task_state`]). The document's bytes are not: they live in the
//! task directory's `design/document.ydoc`, rewritten after every accepted
//! update, and a database rebuilt from disk reloads the document from it.

use super::Db;
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;

/// Schema of migration `105_app_design`.
pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS design_session (
        task_id TEXT PRIMARY KEY REFERENCES pipeline_item(id) ON DELETE CASCADE,
        stage TEXT NOT NULL,
        -- Bumped when the design is reopened after the software factory
        -- started: the earlier approval no longer hands anything off.
        epoch INTEGER NOT NULL DEFAULT 1,
        position TEXT NOT NULL,
        schema_version TEXT NOT NULL,
        -- The number of accepted document updates; quiet in task.json.
        doc_revision INTEGER NOT NULL DEFAULT 0,
        next_thread_number INTEGER NOT NULL DEFAULT 1,
        next_delivery_sequence INTEGER NOT NULL DEFAULT 1,
        -- designing | handing_off | handed_off
        status TEXT NOT NULL DEFAULT 'designing'
            CHECK (status IN ('designing', 'handing_off', 'handed_off')),
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    );
    CREATE TABLE IF NOT EXISTS design_doc_update (
        task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        seq INTEGER NOT NULL,
        update_bytes BLOB NOT NULL,
        origin TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        PRIMARY KEY (task_id, seq)
    );
    CREATE TABLE IF NOT EXISTS design_thread (
        id TEXT PRIMARY KEY,
        task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        epoch INTEGER NOT NULL,
        -- Assigned once, in creation order; never changes on reply or resolve.
        number INTEGER NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('comment', 'message')),
        anchor_block_id TEXT,
        quoted_text TEXT,
        -- The document state vector the creating client held once its anchor
        -- was written: delivery waits until the server's document covers it.
        anchor_state_vector BLOB,
        status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved')),
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        resolved_at TEXT,
        resolved_by TEXT,
        UNIQUE (task_id, number)
    );
    CREATE TABLE IF NOT EXISTS design_comment (
        id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL REFERENCES design_thread(id) ON DELETE CASCADE,
        task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        author TEXT NOT NULL CHECK (author IN ('operator', 'agent')),
        body TEXT NOT NULL,
        -- The caller's idempotency key: a retried send is the same comment.
        client_op_id TEXT,
        -- The verified channel the comment arrived on (T8 provenance).
        channel_identity TEXT,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        UNIQUE (task_id, client_op_id)
    );
    CREATE INDEX IF NOT EXISTS idx_design_comment_thread ON design_comment(thread_id);
    CREATE TABLE IF NOT EXISTS design_delivery (
        -- Stable across attempts; the daemon's receipt key.
        id TEXT PRIMARY KEY,
        task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        epoch INTEGER NOT NULL,
        sequence INTEGER NOT NULL,
        -- A person's comment, or the engine's hand-off instruction.
        comment_id TEXT REFERENCES design_comment(id) ON DELETE CASCADE,
        kind TEXT NOT NULL DEFAULT 'feedback' CHECK (kind IN ('feedback', 'handoff')),
        body TEXT,
        state TEXT NOT NULL DEFAULT 'queued'
            CHECK (state IN ('queued', 'delivering', 'delivered', 'uncertain', 'cancelled')),
        -- The batch a delivery was sent in, and the daemon it was sent to.
        attempt_id TEXT,
        daemon_instance TEXT,
        attempts INTEGER NOT NULL DEFAULT 0,
        detail TEXT,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        delivered_at TEXT,
        UNIQUE (task_id, sequence)
    );
    CREATE INDEX IF NOT EXISTS idx_design_delivery_state ON design_delivery(state, task_id);
    CREATE TABLE IF NOT EXISTS design_agent_op (
        task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        op_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        result TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        PRIMARY KEY (task_id, op_id)
    );
    CREATE TABLE IF NOT EXISTS design_approval (
        id TEXT PRIMARY KEY,
        task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        epoch INTEGER NOT NULL,
        phase TEXT NOT NULL CHECK (phase IN (
            'candidate', 'approved', 'exported', 'committing', 'committed',
            'entered', 'invalidated', 'failed')),
        doc_revision INTEGER NOT NULL,
        doc_sha256 TEXT NOT NULL,
        source_commit TEXT,
        artifact_repo_id TEXT,
        artifact_id TEXT,
        policy_json TEXT NOT NULL,
        -- The retained files and their digests, once exported.
        retained_json TEXT,
        -- The worktree HEAD when the commit step was requested: the commit
        -- step may change only the retained files after it.
        handoff_base_sha TEXT,
        committed_sha TEXT,
        -- SHA-256 of the single-use confirmation token shown with the candidate.
        confirmation_hash TEXT,
        confirmation_expires_at TEXT,
        approved_at TEXT,
        approved_by TEXT,
        error TEXT,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    );
    CREATE INDEX IF NOT EXISTS idx_design_approval_task ON design_approval(task_id, epoch);
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignSessionRow {
    pub task_id: String,
    pub stage: String,
    pub epoch: i64,
    pub position: String,
    pub schema_version: String,
    pub doc_revision: i64,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

impl DesignSessionRow {
    pub const DESIGNING: &'static str = "designing";
    pub const HANDING_OFF: &'static str = "handing_off";
    pub const HANDED_OFF: &'static str = "handed_off";

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            task_id: row.get("task_id")?,
            stage: row.get("stage")?,
            epoch: row.get("epoch")?,
            position: row.get("position")?,
            schema_version: row.get("schema_version")?,
            doc_revision: row.get("doc_revision")?,
            status: row.get("status")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignThreadRow {
    pub id: String,
    pub task_id: String,
    pub epoch: i64,
    pub number: i64,
    pub kind: String,
    pub anchor_block_id: Option<String>,
    pub quoted_text: Option<String>,
    #[serde(skip)]
    pub anchor_state_vector: Option<Vec<u8>>,
    pub status: String,
    pub created_at: String,
    pub resolved_at: Option<String>,
    pub resolved_by: Option<String>,
}

impl DesignThreadRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            task_id: row.get("task_id")?,
            epoch: row.get("epoch")?,
            number: row.get("number")?,
            kind: row.get("kind")?,
            anchor_block_id: row.get("anchor_block_id")?,
            quoted_text: row.get("quoted_text")?,
            anchor_state_vector: row.get("anchor_state_vector")?,
            status: row.get("status")?,
            created_at: row.get("created_at")?,
            resolved_at: row.get("resolved_at")?,
            resolved_by: row.get("resolved_by")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignCommentRow {
    pub id: String,
    pub thread_id: String,
    pub task_id: String,
    pub author: String,
    pub body: String,
    #[serde(skip)]
    pub client_op_id: Option<String>,
    pub created_at: String,
}

impl DesignCommentRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            thread_id: row.get("thread_id")?,
            task_id: row.get("task_id")?,
            author: row.get("author")?,
            body: row.get("body")?,
            client_op_id: row.get("client_op_id")?,
            created_at: row.get("created_at")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignDeliveryRow {
    pub id: String,
    pub task_id: String,
    pub epoch: i64,
    pub sequence: i64,
    pub comment_id: Option<String>,
    pub kind: String,
    #[serde(skip)]
    pub body: Option<String>,
    pub state: String,
    #[serde(skip)]
    pub attempt_id: Option<String>,
    #[serde(skip)]
    pub daemon_instance: Option<String>,
    pub attempts: i64,
    pub detail: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub delivered_at: Option<String>,
}

impl DesignDeliveryRow {
    pub const QUEUED: &'static str = "queued";
    pub const DELIVERING: &'static str = "delivering";
    pub const DELIVERED: &'static str = "delivered";
    pub const UNCERTAIN: &'static str = "uncertain";
    pub const CANCELLED: &'static str = "cancelled";

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            task_id: row.get("task_id")?,
            epoch: row.get("epoch")?,
            sequence: row.get("sequence")?,
            comment_id: row.get("comment_id")?,
            kind: row.get("kind")?,
            body: row.get("body")?,
            state: row.get("state")?,
            attempt_id: row.get("attempt_id")?,
            daemon_instance: row.get("daemon_instance")?,
            attempts: row.get("attempts")?,
            detail: row.get("detail")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            delivered_at: row.get("delivered_at")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignApprovalRow {
    pub id: String,
    pub task_id: String,
    pub epoch: i64,
    pub phase: String,
    pub doc_revision: i64,
    pub doc_sha256: String,
    pub source_commit: Option<String>,
    pub artifact_repo_id: Option<String>,
    pub artifact_id: Option<String>,
    pub policy_json: String,
    pub retained_json: Option<String>,
    pub handoff_base_sha: Option<String>,
    pub committed_sha: Option<String>,
    #[serde(skip)]
    pub confirmation_hash: Option<String>,
    pub confirmation_expires_at: Option<String>,
    pub approved_at: Option<String>,
    pub approved_by: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl DesignApprovalRow {
    pub const CANDIDATE: &'static str = "candidate";
    pub const APPROVED: &'static str = "approved";
    pub const EXPORTED: &'static str = "exported";
    pub const COMMITTING: &'static str = "committing";
    pub const COMMITTED: &'static str = "committed";
    pub const ENTERED: &'static str = "entered";
    pub const INVALIDATED: &'static str = "invalidated";
    pub const FAILED: &'static str = "failed";

    /// Phases after the person's confirmation and before the software
    /// factory started: the hand-off is under way and resumes from here.
    pub fn is_handing_off(&self) -> bool {
        matches!(
            self.phase.as_str(),
            Self::APPROVED | Self::EXPORTED | Self::COMMITTING | Self::COMMITTED
        )
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            task_id: row.get("task_id")?,
            epoch: row.get("epoch")?,
            phase: row.get("phase")?,
            doc_revision: row.get("doc_revision")?,
            doc_sha256: row.get("doc_sha256")?,
            source_commit: row.get("source_commit")?,
            artifact_repo_id: row.get("artifact_repo_id")?,
            artifact_id: row.get("artifact_id")?,
            policy_json: row.get("policy_json")?,
            retained_json: row.get("retained_json")?,
            handoff_base_sha: row.get("handoff_base_sha")?,
            committed_sha: row.get("committed_sha")?,
            confirmation_hash: row.get("confirmation_hash")?,
            confirmation_expires_at: row.get("confirmation_expires_at")?,
            approved_at: row.get("approved_at")?,
            approved_by: row.get("approved_by")?,
            error: row.get("error")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }
}

/// A comment created by [`Db::create_design_thread`] or
/// [`Db::add_design_comment`], and whether this call created it (`false` for
/// a retried request with the same idempotency key).
#[derive(Debug, Clone)]
pub struct CreatedDesignComment {
    pub thread: DesignThreadRow,
    pub comment: DesignCommentRow,
    pub created: bool,
}

pub struct NewDesignThread<'a> {
    pub thread_id: &'a str,
    pub comment_id: &'a str,
    pub kind: &'a str,
    pub anchor_block_id: Option<&'a str>,
    pub quoted_text: Option<&'a str>,
    pub anchor_state_vector: Option<&'a [u8]>,
    pub body: &'a str,
    pub author: &'a str,
    pub client_op_id: Option<&'a str>,
    pub channel_identity: Option<&'a str>,
    /// Queue it to the agent with this delivery id (operator feedback only).
    pub delivery_id: Option<&'a str>,
}

const SESSION_COLUMNS: &str = "task_id, stage, epoch, position, schema_version, doc_revision, \
     status, created_at, updated_at";
const THREAD_COLUMNS: &str = "id, task_id, epoch, number, kind, anchor_block_id, quoted_text, \
     anchor_state_vector, status, created_at, resolved_at, resolved_by";
const COMMENT_COLUMNS: &str =
    "id, thread_id, task_id, author, body, client_op_id, created_at";
const DELIVERY_COLUMNS: &str = "id, task_id, epoch, sequence, comment_id, kind, body, state, \
     attempt_id, daemon_instance, attempts, detail, created_at, updated_at, delivered_at";
const APPROVAL_COLUMNS: &str = "id, task_id, epoch, phase, doc_revision, doc_sha256, \
     source_commit, artifact_repo_id, artifact_id, policy_json, retained_json, handoff_base_sha, \
     committed_sha, \
     confirmation_hash, confirmation_expires_at, approved_at, approved_by, error, created_at, \
     updated_at";

const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";

impl Db {
    // -- session ------------------------------------------------------------

    pub(crate) fn design_session(
        &self,
        task_id: &str,
    ) -> Result<Option<DesignSessionRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!("SELECT {SESSION_COLUMNS} FROM design_session WHERE task_id = ?"),
                [task_id],
                DesignSessionRow::from_row,
            )
            .optional()
    }

    /// The task's design session, created at `position` when it has none.
    pub(crate) fn ensure_design_session(
        &self,
        task_id: &str,
        stage: &str,
        position: &str,
        schema_version: &str,
    ) -> Result<(DesignSessionRow, bool), rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            if let Some(existing) = db.design_session(task_id)? {
                return Ok((existing, false));
            }
            db.refuse_while_transferring(task_id)?;
            db.conn.execute(
                "INSERT INTO design_session (task_id, stage, position, schema_version)
                 VALUES (?, ?, ?, ?)",
                params![task_id, stage, position, schema_version],
            )?;
            Ok((
                db.design_session(task_id)?
                    .expect("design session was just inserted"),
                true,
            ))
        })
    }

    pub(crate) fn set_design_position(
        &self,
        task_id: &str,
        position: &str,
    ) -> Result<bool, rusqlite::Error> {
        let changed = self.conn.execute(
            &format!(
                "UPDATE design_session SET position = ?, updated_at = {NOW}
                 WHERE task_id = ? AND position <> ?"
            ),
            params![position, task_id, position],
        )?;
        Ok(changed == 1)
    }

    pub(crate) fn set_design_session_status(
        &self,
        task_id: &str,
        status: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            &format!("UPDATE design_session SET status = ?, updated_at = {NOW} WHERE task_id = ?"),
            params![status, task_id],
        )?;
        Ok(())
    }

    /// Reopen a design whose earlier approval already started the software
    /// factory: a new epoch, so the old approval can never hand off again,
    /// and feedback still queued from the old epoch moves to the new one.
    pub(crate) fn begin_design_epoch(
        &self,
        task_id: &str,
        stage: &str,
    ) -> Result<i64, rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            db.conn.execute(
                &format!(
                    "UPDATE design_session SET epoch = epoch + 1, status = 'designing', stage = ?,
                     updated_at = {NOW} WHERE task_id = ?"
                ),
                params![stage, task_id],
            )?;
            let epoch: i64 = db.conn.query_row(
                "SELECT epoch FROM design_session WHERE task_id = ?",
                [task_id],
                |row| row.get(0),
            )?;
            db.conn.execute(
                &format!(
                    "UPDATE design_approval SET phase = 'invalidated', updated_at = {NOW}
                     WHERE task_id = ? AND epoch < ? AND phase IN ('candidate', 'approved', 'exported', 'committing')"
                ),
                params![task_id, epoch],
            )?;
            db.conn.execute(
                &format!(
                    "UPDATE design_delivery SET epoch = ?, updated_at = {NOW}
                     WHERE task_id = ? AND state = 'queued' AND kind = 'feedback'"
                ),
                params![epoch, task_id],
            )?;
            Ok(epoch)
        })
    }

    // -- document -----------------------------------------------------------

    pub(crate) fn design_doc_updates(
        &self,
        task_id: &str,
    ) -> Result<Vec<(i64, Vec<u8>)>, rusqlite::Error> {
        let mut statement = self.conn.prepare(
            "SELECT seq, update_bytes FROM design_doc_update WHERE task_id = ? ORDER BY seq",
        )?;
        let rows = statement
            .query_map([task_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect();
        rows
    }

    /// Persist one accepted update and advance the document revision in the
    /// same transaction. Returns the new revision.
    pub(crate) fn append_design_doc_update(
        &self,
        task_id: &str,
        update: &[u8],
        origin: &str,
    ) -> Result<i64, rusqlite::Error> {
        self.append_design_doc_update_recording(task_id, update, origin, None)
    }

    /// [`Self::append_design_doc_update`], also recording the agent operation
    /// that produced the update in the same transaction: a retried operation
    /// then finds it applied exactly once.
    pub(crate) fn append_design_doc_update_recording(
        &self,
        task_id: &str,
        update: &[u8],
        origin: &str,
        op: Option<(&str, &str, &str)>,
    ) -> Result<i64, rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            if let Some((op_id, kind, result)) = op {
                db.record_design_agent_op(task_id, op_id, kind, result)?;
            }
            db.refuse_while_transferring(task_id)?;
            let seq: i64 = db.conn.query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM design_doc_update WHERE task_id = ?",
                [task_id],
                |row| row.get(0),
            )?;
            db.conn.execute(
                "INSERT INTO design_doc_update (task_id, seq, update_bytes, origin)
                 VALUES (?, ?, ?, ?)",
                params![task_id, seq, update, origin],
            )?;
            db.conn.execute(
                "UPDATE design_session SET doc_revision = doc_revision + 1 WHERE task_id = ?",
                [task_id],
            )?;
            db.conn.query_row(
                "SELECT doc_revision FROM design_session WHERE task_id = ?",
                [task_id],
                |row| row.get(0),
            )
        })
    }

    /// Replace every stored update with one encoded state. The revision is
    /// unchanged: compaction changes the storage, not the document.
    pub(crate) fn compact_design_doc(
        &self,
        task_id: &str,
        state: &[u8],
    ) -> Result<(), rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            let through: i64 = db.conn.query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM design_doc_update WHERE task_id = ?",
                [task_id],
                |row| row.get(0),
            )?;
            db.conn.execute(
                "DELETE FROM design_doc_update WHERE task_id = ?",
                [task_id],
            )?;
            db.conn.execute(
                "INSERT INTO design_doc_update (task_id, seq, update_bytes, origin)
                 VALUES (?, ?, ?, 'compacted')",
                params![task_id, through.max(1), state],
            )?;
            Ok(())
        })
    }

    /// Restore the document from its task-directory file after a rebuild
    /// from disk: the stored updates are empty and the carried revision may
    /// trail the file's.
    pub(crate) fn restore_design_doc(
        &self,
        task_id: &str,
        state: &[u8],
        revision: i64,
    ) -> Result<(), rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            let existing: i64 = db.conn.query_row(
                "SELECT COUNT(*) FROM design_doc_update WHERE task_id = ?",
                [task_id],
                |row| row.get(0),
            )?;
            if existing > 0 {
                return Ok(());
            }
            db.conn.execute(
                "INSERT INTO design_doc_update (task_id, seq, update_bytes, origin)
                 VALUES (?, 1, ?, 'restored')",
                params![task_id, state],
            )?;
            db.conn.execute(
                "UPDATE design_session SET doc_revision = MAX(doc_revision, ?) WHERE task_id = ?",
                params![revision, task_id],
            )?;
            Ok(())
        })
    }

    // -- threads ------------------------------------------------------------

    pub(crate) fn design_threads(
        &self,
        task_id: &str,
    ) -> Result<Vec<DesignThreadRow>, rusqlite::Error> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {THREAD_COLUMNS} FROM design_thread WHERE task_id = ? ORDER BY number"
        ))?;
        let rows = statement
            .query_map([task_id], DesignThreadRow::from_row)?
            .collect();
        rows
    }

    pub(crate) fn design_thread(
        &self,
        thread_id: &str,
    ) -> Result<Option<DesignThreadRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!("SELECT {THREAD_COLUMNS} FROM design_thread WHERE id = ?"),
                [thread_id],
                DesignThreadRow::from_row,
            )
            .optional()
    }

    pub(crate) fn design_comments(
        &self,
        task_id: &str,
    ) -> Result<Vec<DesignCommentRow>, rusqlite::Error> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {COMMENT_COLUMNS} FROM design_comment WHERE task_id = ?
             ORDER BY created_at, rowid"
        ))?;
        let rows = statement
            .query_map([task_id], DesignCommentRow::from_row)?
            .collect();
        rows
    }

    pub(crate) fn design_comment(
        &self,
        comment_id: &str,
    ) -> Result<Option<DesignCommentRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!("SELECT {COMMENT_COLUMNS} FROM design_comment WHERE id = ?"),
                [comment_id],
                DesignCommentRow::from_row,
            )
            .optional()
    }

    fn design_comment_by_op(
        &self,
        task_id: &str,
        client_op_id: &str,
    ) -> Result<Option<DesignCommentRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COMMENT_COLUMNS} FROM design_comment
                     WHERE task_id = ? AND client_op_id = ?"
                ),
                params![task_id, client_op_id],
                DesignCommentRow::from_row,
            )
            .optional()
    }

    fn enqueue_design_delivery_in_tx(
        &self,
        delivery_id: &str,
        task_id: &str,
        epoch: i64,
        comment_id: Option<&str>,
        kind: &str,
        body: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        let sequence: i64 = self.conn.query_row(
            "SELECT next_delivery_sequence FROM design_session WHERE task_id = ?",
            [task_id],
            |row| row.get(0),
        )?;
        self.conn.execute(
            "UPDATE design_session SET next_delivery_sequence = next_delivery_sequence + 1
             WHERE task_id = ?",
            [task_id],
        )?;
        self.conn.execute(
            "INSERT INTO design_delivery (id, task_id, epoch, sequence, comment_id, kind, body)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![delivery_id, task_id, epoch, sequence, comment_id, kind, body],
        )?;
        Ok(())
    }

    /// Create a thread with its first comment and, for a person's feedback,
    /// its outbox entry — one transaction, so accepted feedback is never
    /// without its delivery. A retry with the same thread id or idempotency
    /// key returns what the first call created.
    pub(crate) fn create_design_thread(
        &self,
        task_id: &str,
        new: NewDesignThread<'_>,
    ) -> Result<CreatedDesignComment, rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            if let Some(thread) = db.design_thread(new.thread_id)? {
                if thread.task_id != task_id {
                    return Err(rusqlite::Error::InvalidParameterName(format!(
                        "thread {} belongs to another task",
                        new.thread_id
                    )));
                }
                let comment = db
                    .design_comments(task_id)?
                    .into_iter()
                    .find(|comment| comment.thread_id == thread.id)
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                return Ok(CreatedDesignComment {
                    thread,
                    comment,
                    created: false,
                });
            }
            db.refuse_while_transferring(task_id)?;
            let (epoch, number): (i64, i64) = db.conn.query_row(
                "SELECT epoch, next_thread_number FROM design_session WHERE task_id = ?",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            db.conn.execute(
                "UPDATE design_session SET next_thread_number = next_thread_number + 1
                 WHERE task_id = ?",
                [task_id],
            )?;
            db.conn.execute(
                "INSERT INTO design_thread
                    (id, task_id, epoch, number, kind, anchor_block_id, quoted_text, anchor_state_vector)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    new.thread_id,
                    task_id,
                    epoch,
                    number,
                    new.kind,
                    new.anchor_block_id,
                    new.quoted_text,
                    new.anchor_state_vector,
                ],
            )?;
            db.conn.execute(
                "INSERT INTO design_comment
                    (id, thread_id, task_id, author, body, client_op_id, channel_identity)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    new.comment_id,
                    new.thread_id,
                    task_id,
                    new.author,
                    new.body,
                    new.client_op_id,
                    new.channel_identity,
                ],
            )?;
            if let Some(delivery_id) = new.delivery_id {
                db.enqueue_design_delivery_in_tx(
                    delivery_id,
                    task_id,
                    epoch,
                    Some(new.comment_id),
                    "feedback",
                    None,
                )?;
            }
            Ok(CreatedDesignComment {
                thread: db.design_thread(new.thread_id)?.expect("inserted"),
                comment: db.design_comment(new.comment_id)?.expect("inserted"),
                created: true,
            })
        })
    }

    /// Add a reply. A person's reply is queued to the agent in the same
    /// transaction; the agent's is not (it must never deliver to itself).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add_design_comment(
        &self,
        task_id: &str,
        thread_id: &str,
        comment_id: &str,
        author: &str,
        body: &str,
        client_op_id: Option<&str>,
        channel_identity: Option<&str>,
        delivery_id: Option<&str>,
    ) -> Result<CreatedDesignComment, rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            let thread = db
                .design_thread(thread_id)?
                .filter(|thread| thread.task_id == task_id)
                .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            if let Some(op) = client_op_id {
                if let Some(comment) = db.design_comment_by_op(task_id, op)? {
                    return Ok(CreatedDesignComment {
                        thread,
                        comment,
                        created: false,
                    });
                }
            }
            db.refuse_while_transferring(task_id)?;
            db.conn.execute(
                "INSERT INTO design_comment
                    (id, thread_id, task_id, author, body, client_op_id, channel_identity)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    comment_id,
                    thread_id,
                    task_id,
                    author,
                    body,
                    client_op_id,
                    channel_identity
                ],
            )?;
            if let Some(delivery_id) = delivery_id {
                let epoch: i64 = db.conn.query_row(
                    "SELECT epoch FROM design_session WHERE task_id = ?",
                    [task_id],
                    |row| row.get(0),
                )?;
                db.enqueue_design_delivery_in_tx(
                    delivery_id,
                    task_id,
                    epoch,
                    Some(comment_id),
                    "feedback",
                    None,
                )?;
            }
            Ok(CreatedDesignComment {
                thread,
                comment: db.design_comment(comment_id)?.expect("inserted"),
                created: true,
            })
        })
    }

    /// Resolve or reopen a thread. Idempotent: returns whether it changed.
    pub(crate) fn set_design_thread_resolved(
        &self,
        task_id: &str,
        thread_id: &str,
        resolved: bool,
        by: &str,
    ) -> Result<bool, rusqlite::Error> {
        let changed = if resolved {
            self.conn.execute(
                &format!(
                    "UPDATE design_thread SET status = 'resolved', resolved_at = {NOW}, resolved_by = ?
                     WHERE id = ? AND task_id = ? AND status = 'open'"
                ),
                params![by, thread_id, task_id],
            )?
        } else {
            self.conn.execute(
                "UPDATE design_thread SET status = 'open', resolved_at = NULL, resolved_by = NULL
                 WHERE id = ? AND task_id = ? AND status = 'resolved'",
                params![thread_id, task_id],
            )?
        };
        Ok(changed == 1)
    }

    // -- deliveries ---------------------------------------------------------

    pub(crate) fn design_delivery(
        &self,
        delivery_id: &str,
    ) -> Result<Option<DesignDeliveryRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!("SELECT {DELIVERY_COLUMNS} FROM design_delivery WHERE id = ?"),
                [delivery_id],
                DesignDeliveryRow::from_row,
            )
            .optional()
    }

    pub(crate) fn design_deliveries(
        &self,
        task_id: &str,
    ) -> Result<Vec<DesignDeliveryRow>, rusqlite::Error> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {DELIVERY_COLUMNS} FROM design_delivery WHERE task_id = ? ORDER BY sequence"
        ))?;
        let rows = statement
            .query_map([task_id], DesignDeliveryRow::from_row)?
            .collect();
        rows
    }

    /// Tasks with feedback waiting or in flight.
    pub(crate) fn tasks_with_open_design_deliveries(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut statement = self.conn.prepare(
            "SELECT DISTINCT task_id FROM design_delivery
             WHERE state IN ('queued', 'delivering') ORDER BY task_id",
        )?;
        let rows = statement.query_map([], |row| row.get(0))?.collect();
        rows
    }

    /// Mark deliveries as being written in one attempt, before a byte is
    /// sent: a restart then knows to ask the daemon about them rather than
    /// send them again.
    pub(crate) fn reserve_design_deliveries(
        &self,
        ids: &[String],
        attempt_id: &str,
        daemon_instance: &str,
    ) -> Result<(), rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            for id in ids {
                let changed = db.conn.execute(
                    &format!(
                        "UPDATE design_delivery
                         SET state = 'delivering', attempt_id = ?, daemon_instance = ?,
                             attempts = attempts + 1, updated_at = {NOW}
                         WHERE id = ? AND state = 'queued'"
                    ),
                    params![attempt_id, daemon_instance, id],
                )?;
                if changed != 1 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
            }
            Ok(())
        })
    }

    /// Return a reserved attempt to the queue: the daemon reported nothing
    /// was written (the agent was not free after all, or the session is gone).
    pub(crate) fn release_design_attempt(
        &self,
        attempt_id: &str,
        detail: &str,
    ) -> Result<usize, rusqlite::Error> {
        self.conn.execute(
            &format!(
                "UPDATE design_delivery SET state = 'queued', detail = ?, updated_at = {NOW}
                 WHERE attempt_id = ? AND state = 'delivering'"
            ),
            params![detail, attempt_id],
        )
    }

    pub(crate) fn mark_design_attempt_uncertain(
        &self,
        attempt_id: &str,
        detail: &str,
    ) -> Result<usize, rusqlite::Error> {
        self.conn.execute(
            &format!(
                "UPDATE design_delivery SET state = 'uncertain', detail = ?, updated_at = {NOW}
                 WHERE attempt_id = ? AND state = 'delivering'"
            ),
            params![detail, attempt_id],
        )
    }

    /// Settle an attempt the daemon confirmed. Returns the rows settled now;
    /// a repeat settles nothing.
    pub(crate) fn mark_design_attempt_delivered(
        &self,
        attempt_id: &str,
    ) -> Result<Vec<DesignDeliveryRow>, rusqlite::Error> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {DELIVERY_COLUMNS} FROM design_delivery
             WHERE attempt_id = ? AND state IN ('delivering', 'uncertain') ORDER BY sequence"
        ))?;
        let rows: Vec<DesignDeliveryRow> = statement
            .query_map([attempt_id], DesignDeliveryRow::from_row)?
            .collect::<Result<_, _>>()?;
        self.conn.execute(
            &format!(
                "UPDATE design_delivery SET state = 'delivered', detail = NULL,
                 delivered_at = {NOW}, updated_at = {NOW}
                 WHERE attempt_id = ? AND state IN ('delivering', 'uncertain')"
            ),
            [attempt_id],
        )?;
        Ok(rows)
    }

    /// A person asks for an uncertain delivery to be sent again, knowing the
    /// agent may already have it. Same delivery id: a daemon that did write
    /// it answers from its receipt instead of typing it twice.
    pub(crate) fn retry_design_delivery(
        &self,
        task_id: &str,
        delivery_id: &str,
    ) -> Result<bool, rusqlite::Error> {
        let changed = self.conn.execute(
            &format!(
                "UPDATE design_delivery SET state = 'queued', updated_at = {NOW},
                 detail = 'retry requested by the person'
                 WHERE id = ? AND task_id = ? AND state = 'uncertain'"
            ),
            params![delivery_id, task_id],
        )?;
        Ok(changed == 1)
    }

    /// Say why queued deliveries are waiting, without changing their state.
    pub(crate) fn note_design_deliveries(
        &self,
        ids: &[String],
        detail: &str,
    ) -> Result<(), rusqlite::Error> {
        for id in ids {
            self.conn.execute(
                "UPDATE design_delivery SET detail = ? WHERE id = ? AND state = 'queued'",
                params![detail, id],
            )?;
        }
        Ok(())
    }

    pub(crate) fn delivering_design_deliveries(
        &self,
    ) -> Result<Vec<DesignDeliveryRow>, rusqlite::Error> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {DELIVERY_COLUMNS} FROM design_delivery WHERE state = 'delivering'
             ORDER BY task_id, sequence"
        ))?;
        let rows = statement
            .query_map([], DesignDeliveryRow::from_row)?
            .collect();
        rows
    }

    // -- agent operations ---------------------------------------------------

    pub(crate) fn design_agent_op(
        &self,
        task_id: &str,
        op_id: &str,
    ) -> Result<Option<(String, String)>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT kind, result FROM design_agent_op WHERE task_id = ? AND op_id = ?",
                params![task_id, op_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
    }

    pub(crate) fn record_design_agent_op(
        &self,
        task_id: &str,
        op_id: &str,
        kind: &str,
        result: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT OR IGNORE INTO design_agent_op (task_id, op_id, kind, result)
             VALUES (?, ?, ?, ?)",
            params![task_id, op_id, kind, result],
        )?;
        Ok(())
    }

    // -- approvals ----------------------------------------------------------

    pub(crate) fn design_approvals(
        &self,
        task_id: &str,
    ) -> Result<Vec<DesignApprovalRow>, rusqlite::Error> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {APPROVAL_COLUMNS} FROM design_approval WHERE task_id = ?
             ORDER BY created_at, rowid"
        ))?;
        let rows = statement
            .query_map([task_id], DesignApprovalRow::from_row)?
            .collect();
        rows
    }

    pub(crate) fn design_approval(
        &self,
        approval_id: &str,
    ) -> Result<Option<DesignApprovalRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!("SELECT {APPROVAL_COLUMNS} FROM design_approval WHERE id = ?"),
                [approval_id],
                DesignApprovalRow::from_row,
            )
            .optional()
    }

    /// The approval of the current epoch that is a candidate or handing off.
    pub(crate) fn current_design_approval(
        &self,
        task_id: &str,
    ) -> Result<Option<DesignApprovalRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {APPROVAL_COLUMNS} FROM design_approval a
                     WHERE task_id = ?
                       AND epoch = (SELECT epoch FROM design_session s WHERE s.task_id = a.task_id)
                       AND phase NOT IN ('invalidated')
                     ORDER BY created_at DESC, rowid DESC LIMIT 1"
                ),
                [task_id],
                DesignApprovalRow::from_row,
            )
            .optional()
    }

    /// Approvals whose hand-off is under way, across tasks.
    pub(crate) fn handing_off_design_approvals(
        &self,
    ) -> Result<Vec<DesignApprovalRow>, rusqlite::Error> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {APPROVAL_COLUMNS} FROM design_approval
             WHERE phase IN ('approved', 'exported', 'committing', 'committed')
             ORDER BY created_at, rowid"
        ))?;
        let rows = statement
            .query_map([], DesignApprovalRow::from_row)?
            .collect();
        rows
    }

    /// Record a new candidate, superseding any earlier unconfirmed one.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn insert_design_candidate(
        &self,
        approval_id: &str,
        task_id: &str,
        epoch: i64,
        doc_revision: i64,
        doc_sha256: &str,
        source_commit: &str,
        artifact_repo_id: Option<&str>,
        artifact_id: Option<&str>,
        policy_json: &str,
        confirmation_hash: &str,
        confirmation_expires_at: &str,
    ) -> Result<DesignApprovalRow, rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            db.refuse_while_transferring(task_id)?;
            db.conn.execute(
                &format!(
                    "UPDATE design_approval SET phase = 'invalidated', updated_at = {NOW},
                     error = 'superseded by a newer candidate'
                     WHERE task_id = ? AND phase = 'candidate'"
                ),
                [task_id],
            )?;
            db.conn.execute(
                "INSERT INTO design_approval
                   (id, task_id, epoch, phase, doc_revision, doc_sha256, source_commit,
                    artifact_repo_id, artifact_id, policy_json, confirmation_hash,
                    confirmation_expires_at)
                 VALUES (?, ?, ?, 'candidate', ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    approval_id,
                    task_id,
                    epoch,
                    doc_revision,
                    doc_sha256,
                    source_commit,
                    artifact_repo_id,
                    artifact_id,
                    policy_json,
                    confirmation_hash,
                    confirmation_expires_at,
                ],
            )?;
            Ok(db.design_approval(approval_id)?.expect("inserted"))
        })
    }

    /// Move an approval from one phase to the next. Refused (returns false)
    /// when it is no longer in `from`, so two workers never both advance it.
    pub(crate) fn advance_design_approval(
        &self,
        approval_id: &str,
        from: &[&str],
        to: &str,
        update: &DesignApprovalUpdate<'_>,
    ) -> Result<bool, rusqlite::Error> {
        let placeholders = from.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "UPDATE design_approval SET phase = ?, updated_at = {NOW},
               retained_json = COALESCE(?, retained_json),
               handoff_base_sha = COALESCE(?, handoff_base_sha),
               committed_sha = COALESCE(?, committed_sha),
               approved_at = COALESCE(?, approved_at),
               approved_by = COALESCE(?, approved_by),
               confirmation_hash = CASE WHEN ? THEN NULL ELSE confirmation_hash END,
               error = ?
             WHERE id = ? AND phase IN ({placeholders})"
        );
        let mut values: Vec<&dyn rusqlite::ToSql> = vec![
            &to,
            &update.retained_json,
            &update.handoff_base_sha,
            &update.committed_sha,
            &update.approved_at,
            &update.approved_by,
            &update.consume_confirmation,
            &update.error,
            &approval_id,
        ];
        for phase in from {
            values.push(phase);
        }
        let changed = self.conn.execute(&sql, values.as_slice())?;
        Ok(changed == 1)
    }
}

/// Fields an approval phase change may set; `None` keeps the stored value.
#[derive(Default)]
pub struct DesignApprovalUpdate<'a> {
    pub retained_json: Option<&'a str>,
    pub handoff_base_sha: Option<&'a str>,
    pub committed_sha: Option<&'a str>,
    pub approved_at: Option<&'a str>,
    pub approved_by: Option<&'a str>,
    pub consume_confirmation: bool,
    pub error: Option<&'a str>,
}

#[cfg(test)]
#[path = "design_tests.rs"]
mod tests;
