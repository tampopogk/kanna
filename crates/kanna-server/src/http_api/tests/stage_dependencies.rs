//! Stage dependency edges (T4) through the HTTP API: creation installs the
//! edges on an unstarted task, readiness starts it from the upstream result's
//! recorded commit exactly once however many deciders race, and a closing
//! final-stage upstream releases its dependents.

use super::actions::{
    commit_branch_change, dependent_scenario_config, expect_one_spawn, spawn_dependent_start_daemon,
};
use super::*;

fn head_of(path: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

async fn create(app: &axum::Router, body: serde_json::Value) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/tasks")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&body).to_string())
}

struct Scenario {
    repo_root: PathBuf,
    daemon_dir: PathBuf,
    socket_path: PathBuf,
    config: Config,
}

impl Scenario {
    fn new(label: &str) -> Self {
        let unique = unique_test_suffix();
        let repo_root = crate::test_paths::unique_test_path(&format!("kanna-http-{label}"));
        init_test_git_repo(&repo_root);
        let daemon_dir = crate::test_paths::unique_test_path(&format!("kanna-http-{label}-daemon"));
        std::fs::create_dir_all(&daemon_dir).unwrap();
        let socket_path = daemon_socket_path_for_dir(&daemon_dir.to_string_lossy());
        let _ = std::fs::remove_file(&socket_path);
        let config = dependent_scenario_config(label, &unique, &daemon_dir);
        let db = Db::open_for_tests(&config.db_path).unwrap();
        db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
            .unwrap();
        db.insert_test_pipeline_item(
            "task-a",
            "repo-1",
            "Plan the work",
            Some("Plan"),
            "plan",
            "2026-09-23 00:00:00",
        )
        .unwrap();
        db.pin_test_stages("task-a", &["plan", "build"]);
        Self {
            repo_root,
            daemon_dir,
            socket_path,
            config,
        }
    }

    fn db(&self) -> Db {
        Db::open(&self.config.db_path).unwrap()
    }
}

impl Drop for Scenario {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_dir_all(&self.daemon_dir);
        let _ = std::fs::remove_dir_all(&self.repo_root);
    }
}

#[tokio::test]
async fn dependent_waits_then_starts_once_from_the_recorded_sha_across_restarts() {
    let scenario = Scenario::new("stage-edge-start");
    let upstream_worktree =
        commit_branch_change(&scenario.repo_root, "task-a-plan", "plan.md", "the plan");
    let recorded = head_of(&upstream_worktree);

    let app = super::router(Arc::new(AppState::new(scenario.config.clone())));
    let (status, body) = create(
        &app,
        serde_json::json!({
            "repoId": "repo-1",
            "prompt": "Build on the plan",
            "workflowName": TEST_PROVIDER_NEUTRAL_WORKFLOW,
            "agentProvider": "claude",
            "dependencies": [{ "taskId": "task-a", "stage": "plan" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let dependent: CreateTaskResponse = from_slice(body.as_bytes()).unwrap();
    assert_eq!(
        dependent.worktree_path, None,
        "unsatisfied: created unstarted"
    );
    assert!(
        !scenario.socket_path.exists(),
        "an unsatisfied dependent needs no daemon"
    );
    let db = scenario.db();
    let edges = db.list_stage_edges_into(&dependent.task_id).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].upstream_task_id, "task-a");
    assert_eq!(edges[0].upstream_stage, "plan");
    assert_eq!(edges[0].dependent_stage, "in progress");
    assert!(db
        .get_task_worktree_path(&dependent.task_id)
        .unwrap()
        .is_none());

    // The upstream leaves `plan` with a success recording its commit, then
    // keeps committing on its branch.
    let result = db.record_test_stage_result("task-a", "plan", "success", Some(&recorded));
    db.update_pipeline_item_stage("task-a", "build").unwrap();
    std::fs::write(upstream_worktree.join("later.md"), "later").unwrap();
    assert!(Command::new("git")
        .args(["add", "later.md"])
        .current_dir(&upstream_worktree)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-m", "later"])
        .current_dir(&upstream_worktree)
        .status()
        .unwrap()
        .success());
    assert_ne!(head_of(&upstream_worktree), recorded);

    let listener = tokio::net::UnixListener::bind(&scenario.socket_path).unwrap();
    let (daemon_server, mut spawned) =
        spawn_dependent_start_daemon(listener, dependent.task_id.clone());

    // The departure's own notification was lost to a restart: two startup
    // sweeps race, and a third runs after a second restart.
    let first = Arc::new(AppState::new(scenario.config.clone()));
    let sweeps = tokio::join!(
        crate::http_api::stage_dependencies::resume_stage_dependency_readiness(Arc::clone(&first)),
        crate::http_api::stage_dependencies::resume_stage_dependency_readiness(Arc::clone(&first)),
    );
    let _ = sweeps;
    crate::http_api::stage_dependencies::resume_stage_dependency_readiness(Arc::new(
        AppState::new(scenario.config.clone()),
    ))
    .await;
    expect_one_spawn(&mut spawned, "startup sweeps after a restart").await;
    daemon_server.abort();

    let db = scenario.db();
    let worktree = db
        .get_task_worktree_path(&dependent.task_id)
        .unwrap()
        .expect("dependent started");
    assert_eq!(head_of(Path::new(&worktree)), recorded);
    assert_eq!(
        db.list_stage_runs_for_task(&dependent.task_id)
            .unwrap()
            .len(),
        1,
        "exactly one first-stage run"
    );
    let edge = db
        .list_stage_edges_into(&dependent.task_id)
        .unwrap()
        .remove(0);
    assert_eq!(edge.consumed_result_id.as_deref(), Some(result.as_str()));
    assert_eq!(edge.consumed_sha.as_deref(), Some(recorded.as_str()));
}

#[tokio::test]
async fn closing_a_final_stage_upstream_starts_its_dependent() {
    let scenario = Scenario::new("stage-edge-final-close");
    let app = super::router(Arc::new(AppState::new(scenario.config.clone())));
    let (status, body) = create(
        &app,
        serde_json::json!({
            "repoId": "repo-1",
            "prompt": "After task A is done",
            "workflowName": TEST_PROVIDER_NEUTRAL_WORKFLOW,
            "agentProvider": "claude",
            "dependencies": [{ "taskId": "task-a", "stage": "build" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let dependent: CreateTaskResponse = from_slice(body.as_bytes()).unwrap();
    // Leaving `plan` does not release an edge on the final stage.
    let db = scenario.db();
    db.record_test_stage_result("task-a", "plan", "success", None);
    db.update_pipeline_item_stage("task-a", "build").unwrap();
    assert!(db
        .stage_edge_inputs(&dependent.task_id, "in progress", true)
        .unwrap()
        .is_none());

    let listener = tokio::net::UnixListener::bind(&scenario.socket_path).unwrap();
    let (daemon_server, mut spawned) =
        spawn_dependent_start_daemon(listener, dependent.task_id.clone());
    let close = app
        .oneshot(
            Request::post("/v1/tasks/task-a/actions/close")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close.status(), StatusCode::NO_CONTENT);
    expect_one_spawn(&mut spawned, "close of a final-stage upstream").await;
    daemon_server.abort();
    // No recorded commit at the final stage: the dependent forked from its
    // normal start point, never from the upstream's branch tip.
    let db = scenario.db();
    let worktree = db
        .get_task_worktree_path(&dependent.task_id)
        .unwrap()
        .expect("dependent started");
    assert_eq!(
        head_of(Path::new(&worktree)),
        head_of(&scenario.repo_root),
        "forked from main"
    );
}

#[tokio::test]
async fn invalid_dependencies_are_refused_before_a_task_exists() {
    let scenario = Scenario::new("stage-edge-invalid");
    let app = super::router(Arc::new(AppState::new(scenario.config.clone())));
    let base = |dependencies: serde_json::Value| {
        serde_json::json!({
            "repoId": "repo-1",
            "prompt": "Refused",
            "workflowName": TEST_PROVIDER_NEUTRAL_WORKFLOW,
            "agentProvider": "claude",
            "dependencies": dependencies,
        })
    };
    let (status, body) = create(
        &app,
        base(serde_json::json!([{ "taskId": "task-missing", "stage": "plan" }])),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = create(
        &app,
        base(serde_json::json!([{ "taskId": "task-a", "stage": "deploy" }])),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("stage not found"), "{body}");
    let (status, body) = create(
        &app,
        base(serde_json::json!([
            { "taskId": "task-a", "stage": "plan", "dependentStage": "review" }
        ])),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let mut with_blockers = base(serde_json::json!([{ "taskId": "task-a", "stage": "plan" }]));
    with_blockers["blockerTaskIds"] = serde_json::json!(["task-a"]);
    let (status, body) = create(&app, with_blockers).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let db = scenario.db();
    let tasks: i64 = db
        .connection_for_e2e_tests()
        .query_row("SELECT COUNT(*) FROM pipeline_item", [], |row| row.get(0))
        .unwrap();
    assert_eq!(tasks, 1, "only the upstream exists");
}
