//! Read a stage run's fully-resolved prompt.
//!
//! Sibling of `workspace_setup_logs`: both are per-run text kept out of
//! `get_task`/`get_tasks` and addressed only by their own endpoint, scoped to
//! the task that owns the run so a reader cannot address another task's
//! record by run id alone.
use super::{lan_trust::PrivilegedTaskAccess, state::AppState};
use crate::db::{Db, StageRunPrompt};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use std::sync::Arc;

type Error = (StatusCode, String);

fn internal(error: impl ToString) -> Error {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn resolve(db: &Db, task: &str) -> Result<String, Error> {
    db.resolve_pipeline_item_id(task)
        .map_err(internal)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Task not found".into()))
}

pub(super) async fn read(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((task, run)): Path<(String, String)>,
) -> Result<Json<Option<StageRunPrompt>>, Error> {
    let db = Db::open(&state.config.db_path).map_err(internal)?;
    let task = resolve(&db, &task)?;
    db.stage_run_prompt(&task, &run).map(Json).map_err(internal)
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request, http::StatusCode};
    use tower::ServiceExt;

    fn seed(db: &crate::db::Db) {
        crate::db::terminal_archives::tests::seed(db);
        db.record_stage_run_prompt("run-task-a-1", "## Agent Instructions\n\nDo the thing.")
            .unwrap();
    }

    #[tokio::test]
    async fn resolved_prompt_route_serves_the_record_and_scopes_it_to_its_task() {
        let app = crate::http_api::test_support::test_router_with_seed(
            "stage-run-prompts",
            "stage-run-prompts",
            seed,
        );
        let body = |path: &'static str| {
            let app = app.clone();
            async move {
                let response = app
                    .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                let status = response.status();
                let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                (status, String::from_utf8(bytes.to_vec()).unwrap())
            }
        };

        let (status, read) = body("/v1/tasks/task-a/runs/run-task-a-1/resolved-prompt").await;
        assert_eq!(status, StatusCode::OK);
        let read: serde_json::Value = serde_json::from_str(&read).unwrap();
        assert_eq!(read["runId"], "run-task-a-1");
        assert_eq!(
            read["resolvedPrompt"],
            "## Agent Instructions\n\nDo the thing."
        );

        // A run id is not an address into another task's record.
        let (status, read) = body("/v1/tasks/task-b/runs/run-task-a-1/resolved-prompt").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read, "null");

        // A run with no recorded prompt has no record, rather than an empty one.
        let (status, read) = body("/v1/tasks/task-a/runs/run-task-a-2/resolved-prompt").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read, "null");
    }
}
