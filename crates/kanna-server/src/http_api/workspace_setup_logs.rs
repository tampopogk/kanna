//! Read a stage's Setup stream.
//!
//! Sibling of `terminal_archives`: that surface serves the scrollback of a
//! task's launched PTY sessions, this one the output of the workspace setup
//! that prepared each of them. Both are addressed by the same identity — the
//! stage run — so one list of history items can carry Setup beside Agent and
//! Teardown.
use super::{lan_trust::PrivilegedTaskAccess, state::AppState};
use crate::db::{Db, WorkspaceSetupRun};
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

pub(super) async fn list(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(task): Path<String>,
) -> Result<Json<Vec<WorkspaceSetupRun>>, Error> {
    let db = Db::open(&state.config.db_path).map_err(internal)?;
    let task = resolve(&db, &task)?;
    db.workspace_setup_runs(&task).map(Json).map_err(internal)
}

pub(super) async fn read(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((task, run)): Path<(String, String)>,
) -> Result<Json<Option<WorkspaceSetupRun>>, Error> {
    let db = Db::open(&state.config.db_path).map_err(internal)?;
    let task = resolve(&db, &task)?;
    db.workspace_setup_run(&task, &run)
        .map(Json)
        .map_err(internal)
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request, http::StatusCode};
    use tower::ServiceExt;

    fn seed(db: &crate::db::Db) {
        crate::db::terminal_archives::tests::seed(db);
        db.record_workspace_setup_run(
            "run-task-a-1",
            &crate::db::WorkspaceSetupOutcome {
                exit_code: Some(0),
                timed_out: false,
                truncated: false,
                commands: vec!["pnpm install".to_string()],
                output: "INSTALLED".to_string(),
                duration_ms: 1200,
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn creation_progress_is_readable_before_task_row_and_after_failure() {
        let app = crate::http_api::test_support::test_router_with_seed(
            "creation-live",
            "creation-live",
            seed,
        );
        let id = "deadbeef";
        crate::creation_progress::begin(id);
        crate::creation_progress::scoped(id, || {
            crate::creation_progress::phase("Running workspace setup");
            crate::creation_progress::output("FIRST\n");
        });
        for failed in [false, true] {
            if failed {
                crate::creation_progress::finish(id, Some("CONTROLLED_FAILURE exit 23"));
            }
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/v1/tasks/deadbeef/creation-progress")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let snapshot: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                snapshot["status"],
                if failed { "failed" } else { "running" }
            );
            assert!(snapshot["output"].as_str().unwrap().contains("FIRST"));
            if failed {
                assert_eq!(snapshot["error"], "CONTROLLED_FAILURE exit 23");
            }
        }
    }

    #[tokio::test]
    async fn setup_log_routes_serve_the_record_and_scope_it_to_its_task() {
        let app =
            crate::http_api::test_support::test_router_with_seed("setup-logs", "setup-logs", seed);
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

        let (status, listed) = body("/v1/tasks/task-a/setup-logs").await;
        assert_eq!(status, StatusCode::OK);
        let listed: Vec<serde_json::Value> = serde_json::from_str(&listed).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["runId"], "run-task-a-1");
        assert_eq!(listed[0]["status"], "succeeded");
        assert_eq!(listed[0]["exitCode"], 0);
        assert_eq!(listed[0]["output"], "INSTALLED");
        assert_eq!(listed[0]["commands"][0], "pnpm install");

        let (status, read) = body("/v1/tasks/task-a/setup-logs/run-task-a-1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&read).unwrap()["runId"],
            "run-task-a-1"
        );

        // A run id is not an address into another task's workspace.
        let (status, read) = body("/v1/tasks/task-b/setup-logs/run-task-a-1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read, "null");

        // A run with no setup has no record, rather than an empty one.
        let (status, read) = body("/v1/tasks/task-a/setup-logs/run-task-a-2").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read, "null");
    }
}

/// Available before the task row exists, under the same privileged access as setup logs.
pub(super) async fn creation(
    _access: PrivilegedTaskAccess,
    Path(task): Path<String>,
) -> Json<Option<crate::creation_progress::Snapshot>> {
    Json(crate::creation_progress::read(&task))
}
