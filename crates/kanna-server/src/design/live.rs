//! The live document of each design session, held in memory and made
//! durable before it is acknowledged.
//!
//! An accepted update is stored in the database (its own row, with the
//! revision it produced) and the whole document is rewritten to the task
//! directory's `design/document.ydoc` before the client hears it was
//! accepted, so neither a restart nor a database rebuilt from disk loses an
//! acknowledged edit. Clients catch up from this durable state by state
//! vector; they never need the server to have been running when they wrote.

use super::document::{DesignDocument, DocumentError};
use crate::db::Db;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// Compact the stored updates into one state after this many.
const COMPACT_AFTER_UPDATES: usize = 200;
const FILE_MAGIC: &[u8; 8] = b"KDESIGN1";

struct LiveDocument {
    document: DesignDocument,
    revision: i64,
    stored_updates: usize,
}

/// Per-process cache of live documents and their change signals.
#[derive(Clone, Default)]
pub(crate) struct LiveDocuments {
    documents: Arc<Mutex<HashMap<String, Arc<Mutex<LiveDocument>>>>>,
    revisions: Arc<Mutex<HashMap<String, watch::Sender<i64>>>>,
}

#[derive(Debug)]
pub(crate) enum LiveError {
    Document(DocumentError),
    Db(String),
}

impl std::fmt::Display for LiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Document(error) => write!(f, "{error}"),
            Self::Db(error) => write!(f, "db error: {error}"),
        }
    }
}

impl From<DocumentError> for LiveError {
    fn from(error: DocumentError) -> Self {
        Self::Document(error)
    }
}

impl From<rusqlite::Error> for LiveError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Db(error.to_string())
    }
}

/// What a sync request returns: everything the client lacked, the server's
/// state vector, and the revision it reflects.
#[derive(Debug, Clone)]
pub(crate) struct SyncOutcome {
    pub(crate) update: Vec<u8>,
    pub(crate) state_vector: Vec<u8>,
    pub(crate) revision: i64,
}

/// The document file beside a task's ledger.
pub(crate) fn document_file(db_path: &str, task_id: &str) -> Option<PathBuf> {
    crate::task_store::task_dir_for_db_path(db_path, task_id)
        .map(|dir| dir.join("design").join("document.ydoc"))
}

fn encode_file(revision: i64, state: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(state.len() + 48);
    bytes.extend_from_slice(FILE_MAGIC);
    bytes.extend_from_slice(&revision.to_be_bytes());
    bytes.extend_from_slice(&Sha256::digest(state));
    bytes.extend_from_slice(state);
    bytes
}

/// The revision and state a document file holds, when it is intact.
pub(crate) fn decode_file(bytes: &[u8]) -> Option<(i64, Vec<u8>)> {
    if bytes.len() < 48 || &bytes[..8] != FILE_MAGIC {
        return None;
    }
    let revision = i64::from_be_bytes(bytes[8..16].try_into().ok()?);
    let (digest, state) = bytes[16..].split_at(32);
    (Sha256::digest(state).as_slice() == digest).then(|| (revision, state.to_vec()))
}

fn write_file(db_path: &str, task_id: &str, revision: i64, state: &[u8]) {
    let Some(path) = document_file(db_path, task_id) else {
        return;
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    if let Err(error) =
        crate::task_store::replace_atomically(dir, "document.ydoc", &encode_file(revision, state))
    {
        // The database row is the acknowledged record; the file is what a
        // rebuild from disk reads. Say so rather than failing the edit.
        log::warn!("design document file for {task_id} not written: {error}");
    }
}

/// SHA-256 of a document's encoded state, as hex.
pub(crate) fn state_digest(state: &[u8]) -> String {
    Sha256::digest(state)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl LiveDocuments {
    fn load(
        &self,
        db: &Db,
        db_path: &str,
        task_id: &str,
    ) -> Result<Arc<Mutex<LiveDocument>>, LiveError> {
        if let Some(live) = self.documents.lock().unwrap().get(task_id) {
            return Ok(Arc::clone(live));
        }
        let session = db
            .design_session(task_id)?
            .ok_or_else(|| LiveError::Db(format!("task {task_id} has no design session")))?;
        let mut updates = db.design_doc_updates(task_id)?;
        if updates.is_empty() {
            // A database rebuilt from disk: the document is the task
            // directory's file, verified by its own digest.
            if let Some((revision, state)) = document_file(db_path, task_id)
                .and_then(|path| std::fs::read(path).ok())
                .and_then(|bytes| decode_file(&bytes))
            {
                db.restore_design_doc(task_id, &state, revision)?;
                updates = db.design_doc_updates(task_id)?;
            }
        }
        let mut document = DesignDocument::new();
        for (_, update) in &updates {
            // Stored updates were validated when accepted; an unreadable one
            // is a damaged store, reported rather than skipped.
            document.apply_client_update_trusted(update)?;
        }
        let mut revision = db
            .design_session(task_id)?
            .map(|session| session.doc_revision)
            .unwrap_or(session.doc_revision);
        if updates.is_empty() {
            // A new session starts with one empty paragraph, like BlockNote.
            let seed = document.seed_empty();
            revision = db.append_design_doc_update(task_id, &seed, "seed")?;
            write_file(db_path, task_id, revision, &document.encode_state());
        }
        let live = Arc::new(Mutex::new(LiveDocument {
            document,
            revision,
            stored_updates: updates.len().max(1),
        }));
        let mut documents = self.documents.lock().unwrap();
        Ok(Arc::clone(
            documents.entry(task_id.to_string()).or_insert(live),
        ))
    }

    fn forget(&self, task_id: &str) {
        self.documents.lock().unwrap().remove(task_id);
    }

    fn publish(&self, task_id: &str, revision: i64) {
        let mut revisions = self.revisions.lock().unwrap();
        let sender = revisions
            .entry(task_id.to_string())
            .or_insert_with(|| watch::channel(revision).0);
        sender.send_replace(revision);
    }

    /// A receiver that changes whenever the task's document revision does.
    pub(crate) fn subscribe(&self, task_id: &str, current: i64) -> watch::Receiver<i64> {
        let mut revisions = self.revisions.lock().unwrap();
        revisions
            .entry(task_id.to_string())
            .or_insert_with(|| watch::channel(current).0)
            .subscribe()
    }

    /// Apply a client's update (if any) and answer with what it lacks.
    pub(crate) fn sync(
        &self,
        db: &Db,
        db_path: &str,
        task_id: &str,
        state_vector: &[u8],
        update: Option<&[u8]>,
        origin: &str,
    ) -> Result<SyncOutcome, LiveError> {
        let live = self.load(db, db_path, task_id)?;
        let mut live = live.lock().unwrap();
        if let Some(update) = update.filter(|update| !update.is_empty()) {
            self.accept(db, db_path, task_id, &mut live, update, origin)?;
        }
        Ok(SyncOutcome {
            update: live.document.diff_since(state_vector)?,
            state_vector: live.document.state_vector(),
            revision: live.revision,
        })
    }

    fn accept(
        &self,
        db: &Db,
        db_path: &str,
        task_id: &str,
        live: &mut LiveDocument,
        update: &[u8],
        origin: &str,
    ) -> Result<(), LiveError> {
        if !live.document.apply_client_update(update)? {
            return Ok(());
        }
        // Durable before acknowledged. A failed write leaves the memory ahead
        // of the store; forget it so the next reader reloads what is stored.
        let revision = match db.append_design_doc_update(task_id, update, origin) {
            Ok(revision) => revision,
            Err(error) => {
                self.forget(task_id);
                return Err(error.into());
            }
        };
        live.revision = revision;
        live.stored_updates += 1;
        let state = live.document.encode_state();
        if live.stored_updates >= COMPACT_AFTER_UPDATES {
            if let Err(error) = db.compact_design_doc(task_id, &state) {
                log::warn!("design document compaction for {task_id} deferred: {error}");
            } else {
                live.stored_updates = 1;
            }
        }
        write_file(db_path, task_id, revision, &state);
        self.publish(task_id, revision);
        Ok(())
    }

    /// Run an agent's edit against the document, exactly once per `op_id`.
    ///
    /// Under the document's lock: a recorded `op_id` returns its recorded
    /// result (with `replayed: true`) without running `edit`; otherwise the
    /// update `edit` produces and the operation's record are written in one
    /// transaction. An edit that produces no update (a conflict) records
    /// nothing, so the agent retries it against the current text.
    pub(crate) fn edit_once(
        &self,
        db: &Db,
        db_path: &str,
        task_id: &str,
        op_id: &str,
        edit: impl FnOnce(
            &mut DesignDocument,
        ) -> Result<(serde_json::Value, Option<Vec<u8>>), LiveError>,
    ) -> Result<(serde_json::Value, i64), LiveError> {
        let live = self.load(db, db_path, task_id)?;
        let mut guard = live.lock().unwrap();
        if let Some((_, recorded)) = db.design_agent_op(task_id, op_id)? {
            let mut value: serde_json::Value = serde_json::from_str(&recorded)
                .map_err(|error| LiveError::Db(error.to_string()))?;
            value["replayed"] = serde_json::Value::Bool(true);
            return Ok((value, guard.revision));
        }
        // Work on a copy: a failed write must leave the served document as
        // the store has it.
        let mut candidate = DesignDocument::from_state(&guard.document.encode_state())?;
        let (value, update) = edit(&mut candidate)?;
        let Some(update) = update else {
            return Ok((value, guard.revision));
        };
        let revision = db.append_design_doc_update_recording(
            task_id,
            &update,
            "agent",
            Some((op_id, "edit", &value.to_string())),
        )?;
        guard.document.apply_client_update_trusted(&update)?;
        guard.revision = revision;
        guard.stored_updates += 1;
        write_file(db_path, task_id, revision, &guard.document.encode_state());
        self.publish(task_id, revision);
        Ok((value, revision))
    }

    /// Read the document (a projection is always taken from a copy).
    pub(crate) fn read<T>(
        &self,
        db: &Db,
        db_path: &str,
        task_id: &str,
        read: impl FnOnce(&DesignDocument, i64) -> Result<T, LiveError>,
    ) -> Result<T, LiveError> {
        let live = self.load(db, db_path, task_id)?;
        let live = live.lock().unwrap();
        read(&live.document, live.revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_file_round_trips_and_detects_damage() {
        let bytes = encode_file(7, b"state");
        assert_eq!(decode_file(&bytes), Some((7, b"state".to_vec())));
        let mut damaged = bytes.clone();
        *damaged.last_mut().unwrap() ^= 1;
        assert_eq!(decode_file(&damaged), None);
        assert_eq!(decode_file(b"short"), None);
    }
}
