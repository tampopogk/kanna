use super::state::{db_write_error, AppState};
use crate::db::Db;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use kanna_agent_protocol::StateChangeScope;
use std::sync::Arc;

#[derive(serde::Deserialize, serde::Serialize)]
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
    Query(local_only): Query<super::task_federation::LocalOnlyQuery>,
    Json(request): Json<SetAttentionRequest>,
) -> Result<Response, (StatusCode, String)> {
    let reason = crate::db::normalize_attention_reason(&request.reason)
        .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
    let forward_body = serde_json::to_value(&SetAttentionRequest {
        reason: reason.clone(),
    })
    .unwrap_or(serde_json::Value::Null);
    write_attention(
        &state,
        &task_id,
        local_only.local_only,
        "PUT",
        "/attention",
        &forward_body,
        Some(reason),
    )
    .await
}

pub(super) async fn clear_task_attention(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Query(local_only): Query<super::task_federation::LocalOnlyQuery>,
) -> Result<Response, (StatusCode, String)> {
    write_attention(
        &state,
        &task_id,
        local_only.local_only,
        "DELETE",
        "/attention",
        &serde_json::Value::Null,
        None,
    )
    .await
}

async fn write_attention(
    state: &Arc<AppState>,
    task_id: &str,
    local_only: bool,
    method: &str,
    path_suffix: &str,
    forward_body: &serde_json::Value,
    attention_reason: Option<String>,
) -> Result<Response, (StatusCode, String)> {
    let path = super::task_federation::task_path(task_id, path_suffix);
    let task_id = match super::task_federation::resolve_task_route(
        state,
        task_id,
        local_only,
        method,
        &path,
        forward_body,
    )
    .await?
    {
        super::task_federation::TaskRoute::Local(id) => id,
        super::task_federation::TaskRoute::Remote(response) => return Ok(response),
    };
    let db = Db::open(&state.config.db_path).map_err(|e| db_write_error("db error", e))?;
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
    })
    .into_response())
}
