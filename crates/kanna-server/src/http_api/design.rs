//! App Design HTTP API (docs/specs/app-design.md).
//!
//! Three audiences, three kinds of route:
//!
//! - **Anyone with task access** (the desktop, a paired phone, the task's
//!   agent through MCP/CLI) reads the design and waits for its changes.
//! - **The person** — the desktop presenting its local control credential,
//!   or a paired phone over LAN or the relay — writes the document through
//!   Yjs sync and creates, answers, resolves and re-sends feedback
//!   ([`DesignOperator`]). A comment written here is queued to the agent.
//! - **The agent** edits blocks, replies and resolves through the typed
//!   `/design/agent/*` routes. Its replies are never queued back to itself,
//!   and it cannot push raw Yjs updates: typed operations are how it writes.
//!
//! Approval is not an HTTP operation at all: a candidate is prepared here by
//! the desktop, but confirming it goes through the native control socket
//! only the desktop process can open (`crate::human_control`).
//!
//! Every body is JSON (Yjs bytes as base64), because that is the one shape
//! both phone transports carry: the relay tunnels desktop invocations as
//! JSON, and the LAN client posts the same body to the same route.

use super::blocking::run_handler_blocking;
use super::lan_trust::PrivilegedTaskAccess;
use super::mutation_provenance::RequestChannel;
use super::state::AppState;
use crate::db::Db;
use crate::design::document::BlockOp;
use crate::design::service::{self, CreateThreadRequest, DesignError, ReplyRequest};
use crate::mutation_provenance::{ChannelIdentity, LocalProcessEvidence};
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// The person, on the desktop or a paired phone. An agent reaches the
/// server over loopback without the desktop's local control credential and
/// is refused here; the agent-facing routes are separate.
///
/// The local control credential is a file an agent running as the same OS
/// user could read. That is why approval is not behind this extractor but
/// behind the native control socket's kernel-verified peer.
#[derive(Debug, Clone)]
pub(super) struct DesignOperator {
    channel: ChannelIdentity,
}

impl DesignOperator {
    fn channel_json(&self) -> String {
        self.channel.to_json().to_string()
    }

    pub(super) fn allows(channel: &ChannelIdentity) -> bool {
        matches!(
            channel,
            ChannelIdentity::LocalProcess {
                evidence: LocalProcessEvidence::LocalControlCredential,
            } | ChannelIdentity::PairedDevice { .. }
                | ChannelIdentity::RelayAccount { .. }
        )
    }
}

impl FromRequestParts<Arc<AppState>> for DesignOperator {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let channel = super::mutation_provenance::channel_identity_of(&parts.extensions);
        if Self::allows(&channel) {
            Ok(Self { channel })
        } else {
            Err(failure(
                StatusCode::FORBIDDEN,
                "operator_only",
                "design feedback is the person's: send it from the Kanna desktop app or a paired phone. \
                 Agents use the kanna_design_* tools.",
            ))
        }
    }
}

/// The desktop app itself (local control credential over a direct loopback
/// socket): preparing and withdrawing approval candidates.
#[derive(Debug, Clone, Copy)]
pub(super) struct DesignDesktop;

impl FromRequestParts<Arc<AppState>> for DesignDesktop {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        match super::mutation_provenance::channel_identity_of(&parts.extensions) {
            ChannelIdentity::LocalProcess {
                evidence: LocalProcessEvidence::LocalControlCredential,
            } => Ok(Self),
            _ => Err(failure(
                StatusCode::FORBIDDEN,
                "desktop_only",
                "approving a design is done in the Kanna desktop app",
            )),
        }
    }
}

fn failure(status: StatusCode, reason: &str, message: &str) -> Response {
    (
        status,
        Json(json!({ "ok": false, "reason": reason, "message": message })),
    )
        .into_response()
}

fn design_failure(error: DesignError) -> Response {
    let status = match &error {
        DesignError::NotFound { .. } => StatusCode::NOT_FOUND,
        DesignError::NotDesigning { .. } | DesignError::Conflict { .. } => StatusCode::CONFLICT,
        DesignError::Invalid { .. } => StatusCode::BAD_REQUEST,
        DesignError::Schema { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        DesignError::Unavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
        DesignError::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut body = serde_json::to_value(&error).unwrap_or_else(|_| json!({}));
    body["ok"] = Value::Bool(false);
    (status, Json(body)).into_response()
}

/// Run design work on the blocking pool with a database handle.
async fn with_db<T: serde::Serialize + Send + 'static>(
    state: &Arc<AppState>,
    label: &'static str,
    work: impl FnOnce(&Db, &crate::design::DesignRuntime, &str) -> Result<T, DesignError>
        + Send
        + 'static,
) -> Response {
    let db_path = state.config.db_path.clone();
    let runtime = state.design.clone();
    let result = run_handler_blocking(label, move || {
        let db = Db::open(&db_path).map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {error}"),
            )
        })?;
        Ok(work(&db, &runtime, &db_path))
    })
    .await;
    match result {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => design_failure(error),
        Err((status, message)) => failure(status, "internal", &message),
    }
}

fn changed(state: &AppState) {
    state.publish_state_changed(kanna_agent_protocol::StateChangeScope::Tasks);
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(super) struct DesignQuery {
    #[serde(default)]
    include: Option<String>,
}

pub(super) async fn get_design(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Query(query): Query<DesignQuery>,
) -> Response {
    let include_document = query
        .include
        .as_deref()
        .is_some_and(|include| include.split(',').any(|part| part.trim() == "document"));
    with_db(&state, "design view", move |db, runtime, db_path| {
        service::view(db, runtime, db_path, &task_id, include_document)
    })
    .await
}

#[derive(Debug, Deserialize)]
pub(super) struct AgentDesignQuery {
    #[serde(default)]
    document: Option<bool>,
    #[serde(default)]
    resolved: Option<bool>,
}

/// `kanna_design_get`: the compact reading in `design::agent_view`.
pub(super) async fn get_design_for_agent(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Query(query): Query<AgentDesignQuery>,
) -> Response {
    let include_document = query.document.unwrap_or(true);
    let include_resolved = query.resolved.unwrap_or(false);
    with_db(&state, "design view", move |db, runtime, db_path| {
        crate::design::agent_view::agent_view(
            db,
            runtime,
            db_path,
            &task_id,
            include_document,
            include_resolved,
        )
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChangesQuery {
    #[serde(default)]
    doc: Option<i64>,
    #[serde(default)]
    feed: Option<u64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// Long poll: answers when the document revision or the feed revision moves
/// past what the client has, or when the timeout (at most 25 s) passes.
/// Both clients use it to know when to sync or refetch; neither depends on it
/// for correctness, since every sync catches up by state vector.
pub(super) async fn wait_for_changes(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Query(query): Query<ChangesQuery>,
) -> Response {
    let timeout = std::time::Duration::from_millis(query.timeout_ms.unwrap_or(20_000).min(25_000));
    let current_doc = {
        let db_path = state.config.db_path.clone();
        let task = task_id.clone();
        run_handler_blocking("design revision", move || {
            let db = Db::open(&db_path).map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {error}"),
                )
            })?;
            Ok(db
                .design_session(&task)
                .map_err(|error| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("db error: {error}"),
                    )
                })?
                .map(|session| session.doc_revision)
                .unwrap_or(0))
        })
        .await
    };
    let current_doc = match current_doc {
        Ok(revision) => revision,
        Err((status, message)) => return failure(status, "internal", &message),
    };
    let mut docs = state.design.documents.subscribe(&task_id, current_doc);
    let mut feed = state.design.subscribe_feed(&task_id);
    let doc_ahead = |docs: &tokio::sync::watch::Receiver<i64>| {
        query
            .doc
            .is_none_or(|known| (*docs.borrow()).max(current_doc) != known)
    };
    let feed_ahead = |feed: &tokio::sync::watch::Receiver<u64>| {
        query.feed.is_none_or(|known| *feed.borrow() != known)
    };
    if !doc_ahead(&docs) && !feed_ahead(&feed) {
        let _ = tokio::time::timeout(timeout, async {
            tokio::select! {
                _ = docs.changed() => {}
                _ = feed.changed() => {}
            }
        })
        .await;
    }
    let doc_revision = (*docs.borrow()).max(current_doc);
    let feed_revision = *feed.borrow();
    Json(json!({ "docRevision": doc_revision, "feedRevision": feed_revision })).into_response()
}

// ---------------------------------------------------------------------------
// Document sync
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SyncRequest {
    schema_version: String,
    #[serde(default)]
    state_vector: String,
    #[serde(default)]
    update: Option<String>,
}

fn decode(value: &str, label: &str) -> Result<Vec<u8>, DesignError> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    service::decode_base64_field(value, label)
}

async fn sync_document(
    state: Arc<AppState>,
    task_id: String,
    request: SyncRequest,
    may_write: bool,
) -> Response {
    let wrote = request
        .update
        .as_deref()
        .is_some_and(|update| !update.is_empty());
    if wrote && !may_write {
        return failure(
            StatusCode::FORBIDDEN,
            "operator_only",
            "only the person's editor writes the document through sync; agents edit with kanna_design_edit",
        );
    }
    let response = with_db(&state, "design sync", move |db, runtime, db_path| {
        let vector = decode(&request.state_vector, "stateVector")?;
        let update = request
            .update
            .as_deref()
            .map(|update| decode(update, "update"))
            .transpose()?;
        let outcome = service::sync_document(
            db,
            runtime,
            db_path,
            &task_id,
            &request.schema_version,
            &vector,
            update.as_deref(),
        )?;
        let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        Ok(json!({
            "update": encode(&outcome.update),
            "stateVector": encode(&outcome.state_vector),
            "revision": outcome.revision,
        }))
    })
    .await;
    if wrote && response.status().is_success() {
        changed(&state);
    }
    response
}

/// The person's editor: send an update (optional) and receive what it lacks.
pub(super) async fn sync_document_as_operator(
    _operator: DesignOperator,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(request): Json<SyncRequest>,
) -> Response {
    sync_document(state, task_id, request, true).await
}

/// Read-only sync for any client with task access (a viewer that is not the
/// person's editor).
pub(super) async fn sync_document_read_only(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(request): Json<SyncRequest>,
) -> Response {
    sync_document(state, task_id, request, false).await
}

// ---------------------------------------------------------------------------
// The person's feedback
// ---------------------------------------------------------------------------

pub(super) async fn create_thread(
    operator: DesignOperator,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(request): Json<CreateThreadRequest>,
) -> Response {
    let channel = operator.channel_json();
    let response = with_db(&state, "design comment", move |db, runtime, _| {
        service::create_thread(db, runtime, &task_id, request, Some(&channel))
    })
    .await;
    changed(&state);
    response
}

pub(super) async fn reply_to_thread(
    operator: DesignOperator,
    State(state): State<Arc<AppState>>,
    Path((task_id, thread_id)): Path<(String, String)>,
    Json(request): Json<ReplyRequest>,
) -> Response {
    let channel = operator.channel_json();
    let response = with_db(&state, "design reply", move |db, runtime, _| {
        service::reply_as_operator(db, runtime, &task_id, &thread_id, request, Some(&channel))
    })
    .await;
    changed(&state);
    response
}

#[derive(Debug, Deserialize)]
pub(super) struct ResolveRequest {
    resolved: bool,
}

pub(super) async fn resolve_thread_as_operator(
    _operator: DesignOperator,
    State(state): State<Arc<AppState>>,
    Path((task_id, thread_id)): Path<(String, String)>,
    Json(request): Json<ResolveRequest>,
) -> Response {
    with_db(&state, "design resolve", move |db, runtime, _| {
        service::set_resolved(
            db,
            runtime,
            &task_id,
            &thread_id,
            request.resolved,
            "operator",
        )
    })
    .await
}

pub(super) async fn retry_delivery(
    _operator: DesignOperator,
    State(state): State<Arc<AppState>>,
    Path((task_id, delivery_id)): Path<(String, String)>,
) -> Response {
    with_db(&state, "design retry", move |db, runtime, _| {
        service::retry_delivery(db, runtime, &task_id, &delivery_id)?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[derive(Debug, Deserialize)]
pub(super) struct PositionRequest {
    position: String,
}

pub(super) async fn set_position(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(request): Json<PositionRequest>,
) -> Response {
    let response = with_db(&state, "design position", move |db, runtime, _| {
        let position = service::set_position(db, runtime, &task_id, &request.position)?;
        Ok(json!({ "position": position }))
    })
    .await;
    changed(&state);
    response
}

// ---------------------------------------------------------------------------
// The agent
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentEditRequest {
    op_id: String,
    ops: Vec<BlockOp>,
}

pub(super) async fn agent_edit(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(request): Json<AgentEditRequest>,
) -> Response {
    with_db(&state, "design edit", move |db, runtime, db_path| {
        service::agent_edit(db, runtime, db_path, &task_id, &request.op_id, &request.ops)
    })
    .await
}

pub(super) async fn agent_publish_mockup(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(request): Json<crate::design::mockup::PublishMockupRequest>,
) -> Response {
    let state_for_work = Arc::clone(&state);
    let response = with_db(&state, "design mockup", move |db, runtime, db_path| {
        crate::design::mockup::publish(&state_for_work, db, runtime, db_path, &task_id, &request)
    })
    .await;
    changed(&state);
    response
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentReplyRequest {
    op_id: String,
    body: String,
}

pub(super) async fn agent_reply(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((task_id, thread_id)): Path<(String, String)>,
    Json(request): Json<AgentReplyRequest>,
) -> Response {
    let response = with_db(&state, "design agent reply", move |db, runtime, _| {
        service::reply_as_agent(
            db,
            runtime,
            &task_id,
            &thread_id,
            &request.op_id,
            &request.body,
        )
    })
    .await;
    changed(&state);
    response
}

pub(super) async fn agent_resolve(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((task_id, thread_id)): Path<(String, String)>,
    Json(request): Json<ResolveRequest>,
) -> Response {
    with_db(&state, "design agent resolve", move |db, runtime, _| {
        service::set_resolved(db, runtime, &task_id, &thread_id, request.resolved, "agent")
    })
    .await
}

// ---------------------------------------------------------------------------
// Approval (candidate and reopen; confirmation is native-only)
// ---------------------------------------------------------------------------

pub(super) async fn prepare_candidate(
    _desktop: DesignDesktop,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Response {
    let state_for_work = Arc::clone(&state);
    let response = with_db(&state, "design candidate", move |db, runtime, db_path| {
        crate::design::approval::prepare_candidate(&state_for_work, db, runtime, db_path, &task_id)
    })
    .await;
    changed(&state);
    response
}

pub(super) async fn reopen_design(
    _desktop: DesignDesktop,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Response {
    let response = with_db(&state, "design reopen", move |db, runtime, _| {
        crate::design::approval::reopen(db, runtime, &task_id)
    })
    .await;
    changed(&state);
    response
}

pub(super) async fn retry_handoff(
    _desktop: DesignDesktop,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Response {
    with_db(&state, "design hand-off retry", move |db, runtime, _| {
        crate::design::approval::retry_handoff(db, runtime, &task_id)
    })
    .await
}

/// The hand-off's own stage advance: it dispatches the design stage's commit
/// step to the live session. Refused by the stage engine for any other
/// caller (see `design::approval::guard_design_exit`).
pub(crate) async fn advance_design_stage(
    state: Arc<AppState>,
    task_id: &str,
) -> Result<(), String> {
    let request: super::task_actions::AdvanceStageRequest =
        serde_json::from_value(json!({ "source": "operator" }))
            .map_err(|error| error.to_string())?;
    let response = super::task_actions::advance_stage(
        PrivilegedTaskAccess,
        State(state),
        Path(task_id.to_string()),
        Query(super::task_federation::LocalOnlyQuery { local_only: true }),
        RequestChannel(ChannelIdentity::Server),
        Some(Json(request)),
    )
    .await
    .map_err(|(status, message)| format!("{status}: {message}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("stage advance answered {}", response.status()))
    }
}
