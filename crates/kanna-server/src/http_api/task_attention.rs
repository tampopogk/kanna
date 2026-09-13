use super::state::{db_write_error, AppState};
use super::task_blockers::resolve_existing_task_id;
use crate::db::Db;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use kanna_agent_protocol::StateChangeScope;
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SetAttentionRequest {
    reason: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AttentionResponse {
    task_id: String,
    attention_reason: Option<String>,
    changed: bool,
}

pub(super) async fn set_task_attention(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(request): Json<SetAttentionRequest>,
) -> Result<Json<AttentionResponse>, (StatusCode, String)> {
    let reason = crate::db::normalize_attention_reason(&request.reason)
        .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
    write_attention(&state, &task_id, Some(reason))
}

pub(super) async fn clear_task_attention(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Json<AttentionResponse>, (StatusCode, String)> {
    write_attention(&state, &task_id, None)
}

fn write_attention(
    state: &AppState,
    task_id: &str,
    attention_reason: Option<String>,
) -> Result<Json<AttentionResponse>, (StatusCode, String)> {
    let db = Db::open(&state.config.db_path).map_err(|e| db_write_error("db error", e))?;
    let task_id = resolve_existing_task_id(&db, task_id)?;
    let changed = db
        .set_task_attention(&task_id, attention_reason.as_deref())
        .map_err(|e| db_write_error("db error", e))?;
    if changed {
        state.publish_state_changed(StateChangeScope::Tasks);
    }
    Ok(Json(AttentionResponse {
        task_id,
        attention_reason,
        changed,
    }))
}
