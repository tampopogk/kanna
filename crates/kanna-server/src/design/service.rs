//! Design sessions, threads, positions and agent edits: the operations every
//! transport (desktop, phone, MCP/CLI) reaches through `http_api::design`.
//!
//! Everything here is synchronous database work plus the live document; the
//! HTTP layer runs it on the blocking pool.

use super::document::{self, BlockOp, EditOutcome, ProjectedBlock};
use super::live::LiveError;
use super::DesignRuntime;
use crate::db::design::{
    DesignApprovalRow, DesignCommentRow, DesignDeliveryRow, DesignSessionRow, DesignThreadRow,
    NewDesignThread,
};
use crate::db::Db;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

const MAX_BODY_CHARS: usize = 20_000;
const MAX_QUOTE_CHARS: usize = 2_000;

/// Why a design operation was refused. Each maps to one HTTP status.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub(crate) enum DesignError {
    NotFound { message: String },
    /// The task has no design stage, or is not in it for this operation.
    NotDesigning { message: String },
    Invalid { message: String },
    Conflict { message: String },
    /// A client or document on another schema version.
    Schema { message: String },
    Unavailable { message: String },
    Internal { message: String },
}

impl DesignError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::NotFound { message }
            | Self::NotDesigning { message }
            | Self::Invalid { message }
            | Self::Conflict { message }
            | Self::Schema { message }
            | Self::Unavailable { message }
            | Self::Internal { message } => message,
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid {
            message: message.into(),
        }
    }

    pub(crate) fn internal(message: impl std::fmt::Display) -> Self {
        Self::Internal {
            message: message.to_string(),
        }
    }
}

impl From<rusqlite::Error> for DesignError {
    fn from(error: rusqlite::Error) -> Self {
        match error {
            rusqlite::Error::QueryReturnedNoRows => Self::NotFound {
                message: "not found".into(),
            },
            error if error.to_string().contains("transfer") => Self::Conflict {
                message: format!("the task is being transferred; try again after it settles ({error})"),
            },
            error => Self::internal(format!("db error: {error}")),
        }
    }
}

impl From<LiveError> for DesignError {
    fn from(error: LiveError) -> Self {
        match error {
            LiveError::Document(document::DocumentError::UnsupportedSchema { detail }) => {
                Self::Schema {
                    message: format!("the document holds content this schema does not know: {detail}"),
                }
            }
            LiveError::Document(document::DocumentError::BlockNotFound { block_id }) => {
                Self::NotFound {
                    message: format!("block {block_id} is not in the document"),
                }
            }
            LiveError::Document(error) => Self::invalid(error.to_string()),
            LiveError::Db(error) => Self::internal(error),
        }
    }
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PositionView {
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) artifact: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnchorView {
    pub(crate) block_id: Option<String>,
    /// The text the person selected when they commented.
    pub(crate) quoted_text: Option<String>,
    /// `attached` (the mark is in the document), `pending` (its update has
    /// not reached the server yet) or `detached` (the anchored text was
    /// deleted; the thread and its quotation remain).
    pub(crate) state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) current_text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommentView {
    pub(crate) id: String,
    pub(crate) author: String,
    pub(crate) body: String,
    pub(crate) created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) delivery: Option<DeliveryView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeliveryView {
    pub(crate) id: String,
    /// `queued`, `delivering`, `delivered`, `uncertain` or `cancelled`.
    pub(crate) state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) delivered_at: Option<String>,
}

impl From<&DesignDeliveryRow> for DeliveryView {
    fn from(row: &DesignDeliveryRow) -> Self {
        Self {
            id: row.id.clone(),
            state: row.state.clone(),
            detail: row.detail.clone(),
            delivered_at: row.delivered_at.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadView {
    pub(crate) id: String,
    pub(crate) number: i64,
    /// `comment` (anchored in the document) or `message` (an `/agent`
    /// message, with no anchor).
    pub(crate) kind: String,
    /// `open` or `resolved`; independent of delivery.
    pub(crate) status: String,
    pub(crate) anchor: Option<AnchorView>,
    pub(crate) comments: Vec<CommentView>,
    /// The person-facing summary of where the latest feedback is:
    /// `queued`, `delivering`, `delivered`, `agent_replied`, `uncertain`,
    /// `held` (the design has been handed off) or `none`.
    pub(crate) delivery_status: String,
    pub(crate) created_at: String,
    pub(crate) resolved_at: Option<String>,
    pub(crate) resolved_by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesignView {
    pub(crate) task_id: String,
    pub(crate) stage: String,
    pub(crate) next_stage: Option<String>,
    pub(crate) current_stage: Option<String>,
    /// Whether the task is in its design stage now (and so is designing).
    pub(crate) in_design_stage: bool,
    pub(crate) stage_chain: Vec<String>,
    pub(crate) epoch: i64,
    pub(crate) status: String,
    pub(crate) position: String,
    pub(crate) positions: Vec<PositionView>,
    pub(crate) schema_version: String,
    pub(crate) doc_revision: i64,
    pub(crate) feed_revision: u64,
    pub(crate) threads: Vec<ThreadView>,
    pub(crate) approval: Option<ApprovalView>,
    /// The daemon's runtime verdict for the live session:
    /// `busy`, `waiting`, `idle`, `exited` or unknown (absent).
    pub(crate) agent_runtime: Option<String>,
    /// Where throwaway prototype code goes (outside the task's worktree).
    pub(crate) scratch_repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) document: Option<DocumentView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DocumentView {
    pub(crate) revision: i64,
    pub(crate) blocks: Vec<ProjectedBlock>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalView {
    pub(crate) id: String,
    pub(crate) phase: String,
    pub(crate) epoch: i64,
    pub(crate) doc_revision: i64,
    pub(crate) artifact_id: Option<String>,
    pub(crate) artifact_repo_id: Option<String>,
    pub(crate) source_commit: Option<String>,
    pub(crate) committed_sha: Option<String>,
    pub(crate) retained: Option<Value>,
    pub(crate) policy: Value,
    pub(crate) error: Option<String>,
    pub(crate) approved_at: Option<String>,
    pub(crate) confirmation_expires_at: Option<String>,
    /// The candidate no longer matches the document: confirming it is
    /// refused and a new candidate is prepared instead.
    pub(crate) stale: bool,
}

impl ApprovalView {
    pub(crate) fn from_row(row: &DesignApprovalRow, current_revision: i64) -> Self {
        Self {
            id: row.id.clone(),
            phase: row.phase.clone(),
            epoch: row.epoch,
            doc_revision: row.doc_revision,
            artifact_id: row.artifact_id.clone(),
            artifact_repo_id: row.artifact_repo_id.clone(),
            source_commit: row.source_commit.clone(),
            committed_sha: row.committed_sha.clone(),
            retained: row
                .retained_json
                .as_deref()
                .and_then(|json| serde_json::from_str(json).ok()),
            policy: serde_json::from_str(&row.policy_json).unwrap_or(Value::Null),
            error: row.error.clone(),
            approved_at: row.approved_at.clone(),
            confirmation_expires_at: row.confirmation_expires_at.clone(),
            stale: row.phase == DesignApprovalRow::CANDIDATE && row.doc_revision != current_revision,
        }
    }
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// The task's design stage and session, creating the session the first time
/// the task is seen in its design stage, and starting a new epoch when a
/// handed-off design is back in its stage (reopened after the factory
/// started).
pub(crate) fn ensure_session(
    db: &Db,
    task_id: &str,
) -> Result<(crate::task_creator::TaskDesignStage, DesignSessionRow), DesignError> {
    if db.get_pipeline_item(task_id)?.is_none() {
        return Err(DesignError::NotFound {
            message: format!("task not found: {task_id}"),
        });
    }
    let stage = crate::task_creator::task_design_stage(db, task_id)
        .map_err(DesignError::internal)?
        .ok_or_else(|| DesignError::NotDesigning {
            message: format!("task {task_id}'s workflow has no design stage"),
        })?;
    let first = stage
        .design
        .positions
        .first()
        .map(|position| position.name.clone())
        .ok_or_else(|| DesignError::internal("design stage has no positions"))?;
    let session = match db.design_session(task_id)? {
        Some(session)
            if stage.is_current() && session.status == DesignSessionRow::HANDED_OFF =>
        {
            db.begin_design_epoch(task_id, &stage.stage)?;
            db.design_session(task_id)?.expect("session exists")
        }
        Some(session) => session,
        None if stage.is_current() => {
            db.ensure_design_session(task_id, &stage.stage, &first, document::SCHEMA_VERSION)?
                .0
        }
        None => {
            return Err(DesignError::NotDesigning {
                message: format!(
                    "task {task_id} is not in its design stage ({}) and never started a design",
                    stage.stage
                ),
            })
        }
    };
    Ok((stage, session))
}

/// Refuse a change to the design unless the task is designing now.
fn require_designing(
    stage: &crate::task_creator::TaskDesignStage,
    session: &DesignSessionRow,
) -> Result<(), DesignError> {
    if !stage.is_current() || session.status != DesignSessionRow::DESIGNING {
        return Err(DesignError::NotDesigning {
            message: format!(
                "the design is {} (the task is at stage {}); reopen the design to change it",
                session.status.replace('_', " "),
                stage.current_stage.as_deref().unwrap_or("none"),
            ),
        });
    }
    Ok(())
}

/// Where the design's throwaway prototype code lives: a git repository in
/// the task's directory, outside the monorepo worktree, created on first use.
pub(crate) fn scratch_repository(db_path: &str, task_id: &str, epoch: i64) -> Option<std::path::PathBuf> {
    let dir = crate::task_store::task_dir_for_db_path(db_path, task_id)?
        .join("design")
        .join(format!("scratch-{epoch}"));
    if !dir.join(".git").exists() {
        std::fs::create_dir_all(&dir).ok()?;
        git2::Repository::init(&dir).ok()?;
    }
    Some(dir)
}

pub(crate) fn view(
    db: &Db,
    runtime: &DesignRuntime,
    db_path: &str,
    task_id: &str,
    include_document: bool,
) -> Result<DesignView, DesignError> {
    let (stage, session) = ensure_session(db, task_id)?;
    let stage_chain = crate::task_creator::task_stage_names(db, task_id).map_err(DesignError::internal)?;
    let (blocks, revision, covers) = runtime.documents.read(db, db_path, task_id, |document, revision| {
        let blocks = document.project();
        let threads = db.design_threads(task_id)?;
        let mut covers = HashMap::new();
        for thread in &threads {
            if let Some(vector) = &thread.anchor_state_vector {
                covers.insert(thread.id.clone(), document.covers(vector).unwrap_or(true));
            }
        }
        Ok((blocks, revision, covers))
    })?;
    let anchors = blocks
        .as_ref()
        .map(|blocks| document::comment_anchors(blocks))
        .unwrap_or_default();
    let threads = thread_views(db, task_id, &session, &anchors, &covers)?;
    let approval = db
        .current_design_approval(task_id)?
        .map(|row| ApprovalView::from_row(&row, revision));
    let item = db.get_pipeline_item(task_id)?;
    let document = if include_document {
        Some(DocumentView {
            revision,
            blocks: blocks.map_err(|error| DesignError::from(LiveError::Document(error)))?,
        })
    } else {
        None
    };
    Ok(DesignView {
        task_id: task_id.to_string(),
        in_design_stage: stage.is_current(),
        stage: stage.stage,
        next_stage: stage.next_stage,
        current_stage: stage.current_stage,
        stage_chain,
        epoch: session.epoch,
        status: session.status,
        position: session.position,
        positions: stage
            .design
            .positions
            .iter()
            .map(|position| PositionView {
                name: position.name.clone(),
                label: position.label.clone(),
                artifact: position.artifact.clone(),
            })
            .collect(),
        schema_version: session.schema_version,
        doc_revision: revision,
        feed_revision: runtime.feed_revision(task_id),
        threads,
        approval,
        agent_runtime: item.and_then(|item| item.runtime_status),
        scratch_repository: scratch_repository(db_path, task_id, session.epoch)
            .map(|path| path.display().to_string()),
        document,
    })
}

fn thread_views(
    db: &Db,
    task_id: &str,
    session: &DesignSessionRow,
    anchors: &BTreeMap<String, document::AnchorLocation>,
    covers: &HashMap<String, bool>,
) -> Result<Vec<ThreadView>, DesignError> {
    let threads = db.design_threads(task_id)?;
    let mut comments: HashMap<String, Vec<DesignCommentRow>> = HashMap::new();
    for comment in db.design_comments(task_id)? {
        comments.entry(comment.thread_id.clone()).or_default().push(comment);
    }
    let deliveries: HashMap<String, DesignDeliveryRow> = db
        .design_deliveries(task_id)?
        .into_iter()
        .filter_map(|delivery| Some((delivery.comment_id.clone()?, delivery)))
        .collect();
    Ok(threads
        .into_iter()
        .map(|thread| thread_view(thread, &comments, &deliveries, anchors, covers, session))
        .collect())
}

fn thread_view(
    thread: DesignThreadRow,
    comments: &HashMap<String, Vec<DesignCommentRow>>,
    deliveries: &HashMap<String, DesignDeliveryRow>,
    anchors: &BTreeMap<String, document::AnchorLocation>,
    covers: &HashMap<String, bool>,
    session: &DesignSessionRow,
) -> ThreadView {
    let rows = comments.get(&thread.id).cloned().unwrap_or_default();
    let comment_views: Vec<CommentView> = rows
        .iter()
        .map(|comment| CommentView {
            id: comment.id.clone(),
            author: comment.author.clone(),
            body: comment.body.clone(),
            created_at: comment.created_at.clone(),
            delivery: deliveries.get(&comment.id).map(DeliveryView::from),
        })
        .collect();
    let last_operator = comment_views
        .iter()
        .rposition(|comment| comment.author == "operator");
    let delivery_status = match last_operator {
        None => "none".to_string(),
        Some(index) => {
            let replied = comment_views[index + 1..]
                .iter()
                .any(|comment| comment.author == "agent");
            match comment_views[index].delivery.as_ref() {
                _ if replied => "agent_replied".to_string(),
                Some(delivery)
                    if delivery.state == DesignDeliveryRow::QUEUED
                        && session.status != DesignSessionRow::DESIGNING =>
                {
                    "held".to_string()
                }
                Some(delivery) => delivery.state.clone(),
                None => "none".to_string(),
            }
        }
    };
    let anchor = (thread.kind == "comment").then(|| {
        let location = anchors.get(&thread.id);
        let state = if location.is_some() {
            "attached"
        } else if covers.get(&thread.id) == Some(&false) {
            "pending"
        } else {
            "detached"
        };
        AnchorView {
            block_id: location
                .map(|location| location.block_id.clone())
                .or_else(|| thread.anchor_block_id.clone()),
            quoted_text: thread.quoted_text.clone(),
            state,
            current_text: location.map(|location| location.text.clone()),
        }
    });
    ThreadView {
        id: thread.id,
        number: thread.number,
        kind: thread.kind,
        status: thread.status,
        anchor,
        comments: comment_views,
        delivery_status,
        created_at: thread.created_at,
        resolved_at: thread.resolved_at,
        resolved_by: thread.resolved_by,
    }
}

// ---------------------------------------------------------------------------
// Document sync
// ---------------------------------------------------------------------------

pub(crate) fn check_schema(client_schema: &str) -> Result<(), DesignError> {
    if client_schema != document::SCHEMA_VERSION {
        return Err(DesignError::Schema {
            message: format!(
                "this client edits with schema '{client_schema}', but the document uses '{}'; \
                 update the client before joining (a narrower schema would delete content)",
                document::SCHEMA_VERSION
            ),
        });
    }
    Ok(())
}

pub(crate) fn sync_document(
    db: &Db,
    runtime: &DesignRuntime,
    db_path: &str,
    task_id: &str,
    client_schema: &str,
    state_vector: &[u8],
    update: Option<&[u8]>,
) -> Result<super::live::SyncOutcome, DesignError> {
    check_schema(client_schema)?;
    let (stage, session) = ensure_session(db, task_id)?;
    if session.schema_version != document::SCHEMA_VERSION {
        return Err(DesignError::Schema {
            message: format!(
                "the document was written with schema '{}'; this server serves '{}'",
                session.schema_version,
                document::SCHEMA_VERSION
            ),
        });
    }
    let update = update.filter(|update| !update.is_empty());
    if update.is_some() {
        require_designing(&stage, &session)?;
    }
    let outcome = runtime
        .documents
        .sync(db, db_path, task_id, state_vector, update, "client")?;
    if update.is_some() {
        // A thread waiting for its anchor may be deliverable now.
        runtime.wake_delivery();
    }
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnchorRequest {
    pub(crate) block_id: String,
    pub(crate) quoted_text: String,
    /// Base64 of the client's document state vector once its comment mark
    /// was written: delivery waits until the server's document covers it.
    #[serde(default)]
    pub(crate) state_vector: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateThreadRequest {
    pub(crate) thread_id: String,
    pub(crate) comment_id: String,
    /// `comment` (with an anchor) or `message` (an `/agent` message).
    pub(crate) kind: String,
    pub(crate) body: String,
    #[serde(default)]
    pub(crate) anchor: Option<AnchorRequest>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReplyRequest {
    pub(crate) comment_id: String,
    pub(crate) body: String,
}

fn check_id(label: &str, id: &str) -> Result<(), DesignError> {
    let ok = !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(DesignError::invalid(format!(
            "{label} must be 1-80 letters, digits, '-' or '_'"
        )))
    }
}

fn check_body(body: &str) -> Result<&str, DesignError> {
    let body = body.trim();
    if body.is_empty() {
        return Err(DesignError::invalid("feedback is empty; nothing was sent"));
    }
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(DesignError::invalid(format!(
            "feedback is longer than {MAX_BODY_CHARS} characters"
        )));
    }
    Ok(body)
}

fn new_id(prefix: &str) -> Result<String, DesignError> {
    crate::artifacts::random_hex(12)
        .map(|hex| format!("{prefix}-{hex}"))
        .map_err(DesignError::internal)
}

/// A person's comment or `/agent` message: the thread, its first comment and
/// its queued delivery, in one transaction.
pub(crate) fn create_thread(
    db: &Db,
    runtime: &DesignRuntime,
    task_id: &str,
    request: CreateThreadRequest,
    channel: Option<&str>,
) -> Result<ThreadView, DesignError> {
    check_id("threadId", &request.thread_id)?;
    check_id("commentId", &request.comment_id)?;
    let body = check_body(&request.body)?;
    let (stage, session) = ensure_session(db, task_id)?;
    require_designing(&stage, &session)?;
    let vector = match (&request.kind[..], &request.anchor) {
        ("comment", Some(anchor)) => {
            if anchor.block_id.is_empty() || anchor.quoted_text.trim().is_empty() {
                return Err(DesignError::invalid("a comment needs the text it is anchored to"));
            }
            anchor
                .state_vector
                .as_deref()
                .map(|vector| decode_base64(vector, "anchor.stateVector"))
                .transpose()?
        }
        ("comment", None) => {
            return Err(DesignError::invalid("a comment needs an anchor; send an /agent message instead"))
        }
        ("message", None) => None,
        ("message", Some(_)) => return Err(DesignError::invalid("a message has no anchor")),
        (other, _) => return Err(DesignError::invalid(format!("unknown thread kind {other}"))),
    };
    let quoted: Option<String> = request
        .anchor
        .as_ref()
        .map(|anchor| anchor.quoted_text.chars().take(MAX_QUOTE_CHARS).collect());
    let delivery_id = new_id("dl")?;
    let created = db.create_design_thread(
        task_id,
        NewDesignThread {
            thread_id: &request.thread_id,
            comment_id: &request.comment_id,
            kind: &request.kind,
            anchor_block_id: request.anchor.as_ref().map(|anchor| anchor.block_id.as_str()),
            quoted_text: quoted.as_deref(),
            anchor_state_vector: vector.as_deref(),
            body,
            author: "operator",
            client_op_id: Some(&request.comment_id),
            channel_identity: channel,
            delivery_id: Some(&delivery_id),
        },
    )?;
    if created.created {
        runtime.feed_changed(task_id);
        runtime.wake_delivery();
    }
    thread_by_id(db, runtime, task_id, &request.thread_id)
}

/// A person's reply: queued to the agent like the first comment.
pub(crate) fn reply_as_operator(
    db: &Db,
    runtime: &DesignRuntime,
    task_id: &str,
    thread_id: &str,
    request: ReplyRequest,
    channel: Option<&str>,
) -> Result<ThreadView, DesignError> {
    check_id("commentId", &request.comment_id)?;
    let body = check_body(&request.body)?;
    let (stage, session) = ensure_session(db, task_id)?;
    require_designing(&stage, &session)?;
    let delivery_id = new_id("dl")?;
    let created = db.add_design_comment(
        task_id,
        thread_id,
        &request.comment_id,
        "operator",
        body,
        Some(&request.comment_id),
        channel,
        Some(&delivery_id),
    )?;
    if created.created {
        runtime.feed_changed(task_id);
        runtime.wake_delivery();
    }
    thread_by_id(db, runtime, task_id, thread_id)
}

/// The agent's reply. Never queued: an agent reply must not be delivered
/// back to the agent. `op_id` makes a retried call the same reply.
pub(crate) fn reply_as_agent(
    db: &Db,
    runtime: &DesignRuntime,
    task_id: &str,
    thread_id: &str,
    op_id: &str,
    body: &str,
) -> Result<ThreadView, DesignError> {
    check_id("opId", op_id)?;
    let body = check_body(body)?;
    ensure_session(db, task_id)?;
    let comment_id = new_id("cm")?;
    let created = db
        .add_design_comment(task_id, thread_id, &comment_id, "agent", body, Some(op_id), None, None)
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => DesignError::NotFound {
                message: format!("thread {thread_id} is not in task {task_id}"),
            },
            error => error.into(),
        })?;
    if created.created {
        runtime.feed_changed(task_id);
    }
    thread_by_id(db, runtime, task_id, thread_id)
}

pub(crate) fn set_resolved(
    db: &Db,
    runtime: &DesignRuntime,
    task_id: &str,
    thread_id: &str,
    resolved: bool,
    by: &str,
) -> Result<ThreadView, DesignError> {
    ensure_session(db, task_id)?;
    if db
        .design_thread(thread_id)?
        .is_none_or(|thread| thread.task_id != task_id)
    {
        return Err(DesignError::NotFound {
            message: format!("thread {thread_id} is not in task {task_id}"),
        });
    }
    if db.set_design_thread_resolved(task_id, thread_id, resolved, by)? {
        runtime.feed_changed(task_id);
    }
    thread_by_id(db, runtime, task_id, thread_id)
}

pub(crate) fn thread_by_id(
    db: &Db,
    runtime: &DesignRuntime,
    task_id: &str,
    thread_id: &str,
) -> Result<ThreadView, DesignError> {
    let db_path = db.db_path().to_string();
    view(db, runtime, &db_path, task_id, false)?
        .threads
        .into_iter()
        .find(|thread| thread.id == thread_id)
        .ok_or_else(|| DesignError::NotFound {
            message: format!("thread {thread_id} is not in task {task_id}"),
        })
}

pub(crate) fn retry_delivery(
    db: &Db,
    runtime: &DesignRuntime,
    task_id: &str,
    delivery_id: &str,
) -> Result<(), DesignError> {
    if !db.retry_design_delivery(task_id, delivery_id)? {
        return Err(DesignError::Conflict {
            message: format!(
                "delivery {delivery_id} is not uncertain; only a delivery whose outcome is unknown can be sent again"
            ),
        });
    }
    runtime.feed_changed(task_id);
    runtime.wake_delivery();
    Ok(())
}

// ---------------------------------------------------------------------------
// Positions
// ---------------------------------------------------------------------------

pub(crate) fn set_position(
    db: &Db,
    runtime: &DesignRuntime,
    task_id: &str,
    position: &str,
) -> Result<String, DesignError> {
    let (stage, session) = ensure_session(db, task_id)?;
    require_designing(&stage, &session)?;
    if !stage
        .design
        .positions
        .iter()
        .any(|candidate| candidate.name == position)
    {
        return Err(DesignError::invalid(format!(
            "'{position}' is not a position of stage {}; choose one of {}",
            stage.stage,
            stage
                .design
                .positions
                .iter()
                .map(|position| position.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    // A position is design state inside the one stage: no transition, no new
    // session, no workspace, no reviewer, no budget.
    if db.set_design_position(task_id, position)? {
        runtime.feed_changed(task_id);
    }
    Ok(position.to_string())
}

// ---------------------------------------------------------------------------
// Agent edits
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditResult {
    /// `applied` or `conflict`.
    pub(crate) status: &'static str,
    pub(crate) op_id: String,
    pub(crate) revision: i64,
    pub(crate) schema_version: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) block_ids: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) conflicts: Vec<document::EditConflict>,
    /// A retried `op_id`: this is the first call's result, applied once.
    pub(crate) replayed: bool,
}

pub(crate) fn agent_edit(
    db: &Db,
    runtime: &DesignRuntime,
    db_path: &str,
    task_id: &str,
    op_id: &str,
    ops: &[BlockOp],
) -> Result<Value, DesignError> {
    check_id("opId", op_id)?;
    if ops.len() > 200 {
        return Err(DesignError::invalid("at most 200 operations per edit"));
    }
    let (stage, session) = ensure_session(db, task_id)?;
    require_designing(&stage, &session)?;
    let (mut result, revision) =
        runtime
            .documents
            .edit_once(db, db_path, task_id, op_id, |document| {
                let outcome = document.apply_ops(ops)?;
                let (result, update) = match outcome {
                    EditOutcome::Applied(applied) => (
                        EditResult {
                            status: "applied",
                            op_id: op_id.to_string(),
                            revision: 0,
                            schema_version: document::SCHEMA_VERSION,
                            block_ids: applied.block_ids,
                            conflicts: Vec::new(),
                            replayed: false,
                        },
                        Some(applied.update),
                    ),
                    EditOutcome::Conflict(conflicts) => (
                        EditResult {
                            status: "conflict",
                            op_id: op_id.to_string(),
                            revision: 0,
                            schema_version: document::SCHEMA_VERSION,
                            block_ids: Vec::new(),
                            conflicts,
                            replayed: false,
                        },
                        None,
                    ),
                };
                let value = serde_json::to_value(result)
                    .map_err(|error| LiveError::Db(error.to_string()))?;
                Ok((value, update))
            })?;
    if result["replayed"] != Value::Bool(true) {
        result["revision"] = Value::from(revision);
    }
    Ok(result)
}

fn decode_base64(value: &str, label: &str) -> Result<Vec<u8>, DesignError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|error| DesignError::invalid(format!("{label} is not base64: {error}")))
}

pub(crate) fn decode_base64_field(value: &str, label: &str) -> Result<Vec<u8>, DesignError> {
    decode_base64(value, label)
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
