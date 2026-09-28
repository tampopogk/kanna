use super::state::AppState;
use crate::db::Db;
use axum::{
    extract::{Path, Query, State},
    Json,
};
use std::sync::Arc;

#[derive(serde::Deserialize)]
pub(super) struct DeliveryQuery {
    delivery_id: Option<String>,
}

pub(super) async fn get_task_input_deliveries(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Query(query): Query<DeliveryQuery>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let db = Db::open(&state.config.db_path).map_err(|error| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            error.to_string(),
        )
    })?;
    let task_id = db
        .resolve_pipeline_item_id(&task_id)
        .map_err(|error| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                error.to_string(),
            )
        })?
        .ok_or((
            axum::http::StatusCode::NOT_FOUND,
            "task not found".to_string(),
        ))?;
    let attempts = match query.delivery_id {
        Some(id) => db
            .task_input_delivery(&task_id, &id)
            .map(|attempt| attempt.into_iter().collect()),
        None => db.task_input_deliveries(&task_id),
    }
    .map_err(|error| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            error.to_string(),
        )
    })?;
    Ok(Json(
        serde_json::json!({"taskId": task_id, "deliveries": attempts}),
    ))
}
