use super::state::{db_write_error, AppState};
use super::task_blockers::resolve_existing_task_id;
use crate::db::{Db, TaskServicedWatermark};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use std::sync::Arc;

#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecordServicedRequest {
    /// The `stage_run` doing the servicing. Provenance for the record; it never
    /// decides what the filter shows.
    run_id: Option<String>,
    /// The `task-events` cursor the caller had read this task through. Omitted
    /// means "the log's head right now", which can suppress a change that
    /// landed while the caller was working - so a manager that read a cursor
    /// should record that cursor.
    observed_event_seq: Option<i64>,
}

pub(super) async fn record_task_serviced(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    body: Option<Json<RecordServicedRequest>>,
) -> Result<Json<TaskServicedWatermark>, (StatusCode, String)> {
    let request = body.map(|Json(request)| request).unwrap_or_default();
    let db = Db::open(&state.config.db_path).map_err(|e| db_write_error("db error", e))?;
    let task_id = resolve_existing_task_id(&db, &task_id)?;
    let head = db
        .latest_task_event_seq()
        .map_err(|e| db_write_error("db error", e))?;
    if let Some(seq) = request.observed_event_seq {
        if seq < 0 {
            return Err((
                StatusCode::BAD_REQUEST,
                "observedEventSeq must not be negative".to_string(),
            ));
        }
        // A mark past the head would suppress events nobody has read yet, which
        // is the one way this record can lose work rather than merely repeat it.
        if seq > head {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("observedEventSeq {seq} is ahead of the event log head {head}"),
            ));
        }
    }
    let watermark = db
        .record_task_serviced(
            &task_id,
            request.run_id.as_deref(),
            request.observed_event_seq,
        )
        .map_err(|e| db_write_error("db error", e))?;
    // Deliberately no `publish_state_changed` and no task event: servicing
    // changes nothing a human surface shows, and an event would put this write
    // itself past the mark it just set.
    Ok(Json(watermark))
}
