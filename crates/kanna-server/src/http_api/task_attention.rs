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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AttentionResponse {
    task_id: String,
    attention_requested: bool,
    changed: bool,
}

pub(super) async fn set_task_attention(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Query(local_only): Query<super::task_federation::LocalOnlyQuery>,
) -> Result<Response, (StatusCode, String)> {
    write_attention(
        &state,
        &task_id,
        local_only.local_only,
        "PUT",
        "/attention",
        &serde_json::json!({}),
        true,
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
        false,
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
    attention_requested: bool,
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
        .set_task_attention(&task_id, attention_requested)
        .map_err(|e| db_write_error("db error", e))?;
    if changed {
        state.publish_state_changed(StateChangeScope::Tasks);
    }
    Ok(Json(AttentionResponse {
        task_id,
        attention_requested,
        changed,
    })
    .into_response())
}
