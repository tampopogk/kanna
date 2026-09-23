use super::*;
use crate::db::{NewPipelineItem, NewRepo, NewStageRun, TaskEventScope};
use axum::body::to_bytes;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn seed_workflow_task(
    db: &Db,
    repo_path: &str,
    task_id: &str,
    workflow_name: &str,
    stage: &str,
    workflow_def: &str,
) {
    db.insert_repo(NewRepo {
        id: "repo-1",
        path: repo_path,
        name: "Dynamic Workflow Repo",
        default_branch: Some("main"),
    })
    .unwrap();
    db.insert_pipeline_item(NewPipelineItem {
        id: task_id,
        repo_id: "repo-1",
        prompt: "change this task workflow",
        display_name: Some("Dynamic workflow"),
        pipeline: workflow_name,
        pipeline_def: Some(workflow_def),
        stage,
        branch: &format!("task-{task_id}"),
        agent_type: "pty",
        agent_provider: "claude",
        activity: "working",
        port_offset: None,
        port_env_json: None,
        agent_spawn_options_json: None,
        base_ref: Some("main"),
        notify_task_id: None,
        parent_task_id: None,
    })
    .unwrap();
}

async fn set_workflow(
    app: &axum::Router,
    task_id: &str,
    workflow_name: &str,
) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/v1/tasks/{task_id}/actions/set-workflow"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "workflowName": workflow_name }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

/// The retired route and request key. Both must keep working: a caller on the
/// old naming (an older mobile build, a pinned tool catalog) still reaches the
/// same handler.
async fn set_workflow_via_legacy_surface(
    app: &axum::Router,
    task_id: &str,
    workflow_name: &str,
) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/v1/tasks/{task_id}/actions/set-pipeline"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "pipelineName": workflow_name }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

fn workflow_test_repo(label: &str) -> (tempfile::TempDir, String) {
    let temp = tempfile::Builder::new()
        .prefix(&format!("kanna-dynamic-workflow-{label}-"))
        .tempdir()
        .unwrap();
    let repo_root = temp.path().join("repo");
    init_test_git_repo(&repo_root);
    (temp, repo_root.to_string_lossy().to_string())
}

fn publish_workflow(repo_path: &str, workflow_name: &str, definition: Value) {
    let repo = Path::new(repo_path);
    std::fs::write(
        repo.join(format!(".kanna/workflows/{workflow_name}.json")),
        definition.to_string(),
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["add", "."])
        .current_dir(repo)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-m", "add dynamic workflow fixture"])
        .current_dir(repo)
        .status()
        .unwrap()
        .success());
    publish_test_origin_main(repo);
}

#[tokio::test]
async fn compatible_switch_repins_snapshot_carries_budget_emits_event_and_preserves_stickiness() {
    let (_repo_temp, repo_path) = workflow_test_repo("compatible");
    let state = test_state_with_seed("workflow-switch-compatible", "Studio Mac", move |db| {
        seed_workflow_task(
            db,
            &repo_path,
            "task-1",
            "no-review",
            "in progress",
            r#"{"name":"old-snapshot","stages":[]}"#,
        );
        assert_eq!(
            db.try_claim_agent_revision_round("task-1", 0).unwrap(),
            Some(1)
        );
        assert_eq!(
            db.try_claim_agent_revision_round("task-1", 0).unwrap(),
            Some(2)
        );
    });
    let db_path = state.config().db_path.clone();
    let app = router(Arc::clone(&state));

    let (status, body) = set_workflow(&app, "task-1", "single-reviewer").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let response: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(response["workflowName"], "single-reviewer");
    assert_eq!(response["pipelineName"], "single-reviewer");
    assert_eq!(response["stage"], "in progress");
    assert_eq!(response["revisionRounds"], 2);
    assert_eq!(response["revisionLimit"], 5);

    let db = Db::open(&db_path).unwrap();
    let item = db.get_pipeline_item("task-1").unwrap().unwrap();
    assert_eq!(item.pipeline.as_deref(), Some("single-reviewer"));
    assert_eq!(item.stage.as_deref(), Some("in progress"));
    assert_eq!(item.revision_rounds, 2);
    let pinned: Value = serde_json::from_str(item.pipeline_def.as_deref().unwrap()).unwrap();
    assert_eq!(pinned["name"], "single-reviewer");
    assert_eq!(
        pinned["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|stage| stage["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["in progress", "review", "pr"]
    );
    assert_eq!(
        db.recent_repo_workflows("repo-1", 5).unwrap(),
        vec!["no-review"],
        "mid-flight changes must not feed the creation-time sticky workflow"
    );
    let events = db
        .list_task_events(
            &TaskEventScope::Tasks(vec!["task-1".to_string()]),
            0,
            i64::MAX,
            100,
        )
        .unwrap();
    let changed = events
        .iter()
        .find(|event| event.event_type == "task.workflow_changed")
        .expect("workflow change event");
    assert_eq!(changed.payload["fromWorkflow"], "no-review");
    assert_eq!(changed.payload["toWorkflow"], "single-reviewer");
    assert_eq!(changed.payload["stage"], "in progress");
    assert_eq!(changed.payload["revisionRounds"], 2);
    assert_eq!(changed.payload["revisionLimit"], 5);

    let detail_response = app
        .oneshot(
            Request::get("/v1/tasks/task-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail_response.status(), StatusCode::OK);
    let detail: Value = serde_json::from_slice(
        &to_bytes(detail_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(detail["workflowName"], "single-reviewer");
    assert_eq!(detail["revisionRounds"], 2);
    assert_eq!(detail["revisionLimit"], 5);
}

#[tokio::test]
async fn incompatible_switch_is_rejected_without_mutating_snapshot_or_emitting_event() {
    let (_repo_temp, repo_path) = workflow_test_repo("incompatible");
    let old_snapshot =
        r#"{"name":"single-reviewer","stages":[{"name":"review","policy":{"transition":"auto"}}]}"#;
    let state = test_state_with_seed("workflow-switch-incompatible", "Studio Mac", move |db| {
        seed_workflow_task(
            db,
            &repo_path,
            "task-1",
            "single-reviewer",
            "review",
            old_snapshot,
        );
    });
    let db_path = state.config().db_path.clone();
    let app = router(Arc::clone(&state));

    let (status, body) = set_workflow(&app, "task-1", "no-review").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body.contains("current stage 'review' is not present"));
    assert!(body.contains("no-review"));

    let db = Db::open(&db_path).unwrap();
    let item = db.get_pipeline_item("task-1").unwrap().unwrap();
    assert_eq!(item.pipeline.as_deref(), Some("single-reviewer"));
    assert_eq!(item.pipeline_def.as_deref(), Some(old_snapshot));
    assert!(!db
        .list_task_events(
            &TaskEventScope::Tasks(vec!["task-1".to_string()]),
            0,
            i64::MAX,
            100,
        )
        .unwrap()
        .iter()
        .any(|event| event.event_type == "task.workflow_changed"));
}

#[tokio::test]
async fn mid_run_switch_keeps_the_live_run_and_session_for_the_next_transition() {
    let (_repo_temp, repo_path) = workflow_test_repo("mid-run");
    let state = test_state_with_seed("workflow-switch-mid-run", "Studio Mac", move |db| {
        seed_workflow_task(
            db,
            &repo_path,
            "task-1",
            "single-reviewer",
            "review",
            r#"{"name":"single-reviewer","stages":[{"name":"review","policy":{"transition":"auto"}}]}"#,
        );
        db.insert_stage_run(NewStageRun {
            id: "run-live",
            task_id: "task-1",
            stage: "review",
            kind: "main",
            agent: Some("review"),
            agent_provider: Some("claude"),
            model: None,
            effort: None,
            status: "running",
            result: None,
            feedback: None,
            session_id: Some("daemon-live"),
            provider_session_id: Some("provider-live"),
            cwd: Some("/tmp/live-worktree"),
            resumed_from_run_id: None,
        })
        .unwrap();
        db.insert_test_terminal_session(
            "terminal-live",
            "repo-1",
            "task-1",
            "live review",
            "daemon-live",
        )
        .unwrap();
    });
    let db_path = state.config().db_path.clone();
    let app = router(Arc::clone(&state));

    let (status, body) = set_workflow(&app, "task-1", "specialized-reviewers").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let db = Db::open(&db_path).unwrap();
    let run = db.latest_stage_run("task-1").unwrap().unwrap();
    assert_eq!(run.id, "run-live");
    assert_eq!(run.stage, "review");
    assert_eq!(run.status, "running");
    assert_eq!(run.session_id.as_deref(), Some("daemon-live"));
    assert_eq!(
        db.resolve_task_terminal_session_id("task-1")
            .unwrap()
            .as_deref(),
        Some("daemon-live")
    );
    let item = db.get_pipeline_item("task-1").unwrap().unwrap();
    assert_eq!(item.pipeline.as_deref(), Some("specialized-reviewers"));
    assert_eq!(item.stage.as_deref(), Some("review"));
}

#[tokio::test]
async fn retired_builtin_alias_resolves_through_the_creation_snapshot_path() {
    let (_repo_temp, repo_path) = workflow_test_repo("alias");
    let state = test_state_with_seed("workflow-switch-alias", "Studio Mac", move |db| {
        seed_workflow_task(
            db,
            &repo_path,
            "task-1",
            "single-reviewer",
            "in progress",
            r#"{"name":"old","stages":[]}"#,
        );
    });
    let db_path = state.config().db_path.clone();
    let app = router(Arc::clone(&state));

    let (status, body) = set_workflow(&app, "task-1", "default").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let db = Db::open(&db_path).unwrap();
    let item = db.get_pipeline_item("task-1").unwrap().unwrap();
    assert_eq!(item.pipeline.as_deref(), Some("default"));
    let pinned: Value = serde_json::from_str(item.pipeline_def.as_deref().unwrap()).unwrap();
    assert_eq!(pinned["name"], "no-review");
    assert_eq!(
        pinned["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|stage| stage["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["in progress", "pr"]
    );
}

#[tokio::test]
async fn exhausted_revision_rounds_are_not_reset_but_a_higher_limit_adds_headroom() {
    let (_repo_temp, repo_path) = workflow_test_repo("revision-budget");
    publish_workflow(
        &repo_path,
        "high-budget",
        serde_json::json!({
            "name": "high-budget",
            "revision_limit": 5,
            "stages": [{
                "name": "in progress",
                "policy": { "transition": "manual" }
            }]
        }),
    );
    let state = test_state_with_seed("workflow-switch-budget", "Studio Mac", move |db| {
        seed_workflow_task(
            db,
            &repo_path,
            "task-1",
            "no-review",
            "in progress",
            r#"{"name":"no-review","revision_limit":3,"stages":[{"name":"in progress","policy":{"transition":"manual"}}]}"#,
        );
        for expected_round in 1..=3 {
            assert_eq!(
                db.try_claim_agent_revision_round("task-1", 3).unwrap(),
                Some(expected_round)
            );
        }
        assert_eq!(
            db.try_claim_agent_revision_round("task-1", 3).unwrap(),
            None,
            "the old workflow budget starts exhausted"
        );
    });
    let db_path = state.config().db_path.clone();
    let app = router(Arc::clone(&state));

    let (status, body) = set_workflow(&app, "task-1", "high-budget").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let response: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(response["revisionRounds"], 3);
    assert_eq!(response["revisionLimit"], 5);

    let db = Db::open(&db_path).unwrap();
    assert_eq!(db.task_revision_rounds("task-1").unwrap(), 3);
    assert_eq!(
        db.try_claim_agent_revision_round("task-1", 5).unwrap(),
        Some(4),
        "the higher limit adds headroom without granting a fresh budget"
    );
}

#[tokio::test]
async fn the_legacy_pipeline_route_and_request_key_still_switch_the_workflow() {
    let (_repo_temp, repo_path) = workflow_test_repo("legacy-surface");
    let state = test_state_with_seed("workflow-switch-legacy", "Studio Mac", move |db| {
        seed_workflow_task(
            db,
            &repo_path,
            "task-1",
            "no-review",
            "in progress",
            r#"{"name":"old-snapshot","stages":[]}"#,
        );
    });
    let db_path = state.config().db_path.clone();
    let app = router(Arc::clone(&state));

    let (status, body) = set_workflow_via_legacy_surface(&app, "task-1", "single-reviewer").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let response: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(response["workflowName"], "single-reviewer");
    assert_eq!(response["pipelineName"], "single-reviewer");

    let db = Db::open(&db_path).unwrap();
    let item = db.get_pipeline_item("task-1").unwrap().unwrap();
    assert_eq!(item.pipeline.as_deref(), Some("single-reviewer"));
}

async fn replace_workflow(
    app: &axum::Router,
    expected: &Value,
    definition: &Value,
) -> (StatusCode, Value) {
    // Resolve the public catalog input before crossing HTTP, as MCP and CLI do.
    let request = kanna_tool_catalog::resolve_request(
        &kanna_tool_catalog::bundled_catalog(),
        "kanna_replace_task_workflow",
        &serde_json::json!({
            "task_id": "task-1", "expected_definition": expected,
            "workflow_definition": definition, "source": "operator"
        }),
    )
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::post(request.path)
                .header("content-type", "application/json")
                .body(Body::from(request.body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into())),
    )
}

fn replacement_fixture(label: &str) -> (tempfile::TempDir, Arc<AppState>, Value) {
    let (temp, repo_path) = workflow_test_repo(label);
    let before = serde_json::json!({"name": "pinned", "stages": [
        {"name": "review", "agent": "review", "agent_provider": ["claude-fable", "codex-gpt-6-astra"], "policy": {"transition": "manual"}},
        {"name": "pr", "agent": "pr", "policy": {"transition": "manual"}}
    ]});
    let saved = before.clone();
    let state = test_state_with_seed(label, "Studio Mac", move |db| {
        seed_workflow_task(
            db,
            &repo_path,
            "task-1",
            "pinned",
            "review",
            &saved.to_string(),
        );
        db.insert_stage_run(NewStageRun {
            id: "run-old",
            task_id: "task-1",
            stage: "review",
            kind: "main",
            agent: Some("review"),
            agent_provider: Some("claude"),
            model: Some("fable"),
            effort: None,
            status: "failed",
            result: None,
            feedback: None,
            session_id: Some("session-old"),
            provider_session_id: Some("provider-old"),
            cwd: Some(&repo_path),
            resumed_from_run_id: None,
        })
        .unwrap();
    });
    (temp, state, before)
}

#[tokio::test]
async fn opencode_native_model_ids_can_be_saved_in_a_future_stage() {
    let (_temp, state, before) = replacement_fixture("opencode-workflow-model");
    let app = router(Arc::clone(&state));
    let mut previous = before;
    for selector in ["opencode-local/qwen3-coder:30b", "opencode-cloud/org/coder"] {
        let mut after = previous.clone();
        after["stages"][1]["agent_provider"] = serde_json::json!(selector);
        let (status, body) = replace_workflow(&app, &previous, &after).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["supersededRunIds"], serde_json::json!([]));
        let db = Db::open(&state.config.db_path).unwrap();
        let task = db.get_pipeline_item("task-1").unwrap().unwrap();
        let saved: Value = serde_json::from_str(task.pipeline_def.as_deref().unwrap()).unwrap();
        assert_eq!(
            saved["stages"][1]["agent_provider"],
            serde_json::json!([selector])
        );
        previous = body["workflowDefinition"].clone();
    }
}

#[tokio::test]
async fn replacement_supersedes_only_changed_execution_and_is_durable_and_fenced() {
    let (_temp, state, before) = replacement_fixture("workflow-replace-incident");
    let app = router(Arc::clone(&state));
    let mut after = before.clone();
    after["stages"][0]["agent_provider"] = serde_json::json!(["codex-gpt-6-astra"]);
    let (status, body) = replace_workflow(&app, &before, &after).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["supersededRunIds"], serde_json::json!(["run-old"]));
    let db = Db::open(&state.config.db_path).unwrap();
    assert!(db
        .stage_run_workflow_superseded("task-1", "run-old")
        .unwrap());
    assert!(!db
        .stage_run_workflow_superseded("another-task", "run-old")
        .unwrap());
    assert!(!db
        .stage_run_workflow_superseded("task-1", "new-run")
        .unwrap());
    let old_run = db.latest_stage_run("task-1").unwrap().unwrap();
    assert_eq!(old_run.agent_provider.as_deref(), Some("claude"));
    assert_eq!(old_run.status, "failed");
    let events = db
        .list_task_events(
            &TaskEventScope::Tasks(vec!["task-1".into()]),
            0,
            i64::MAX,
            100,
        )
        .unwrap();
    let event = events
        .iter()
        .find(|event| event.event_type == "task.workflow_changed")
        .unwrap();
    assert_eq!(event.payload["beforeDefinition"], before);
    assert_eq!(event.payload["afterDefinition"], body["workflowDefinition"]);
    assert_eq!(event.payload["source"], "operator");
    let (status, _) = replace_workflow(&app, &before, &before).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, unchanged) = replace_workflow(
        &app,
        &body["workflowDefinition"],
        &body["workflowDefinition"],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unchanged["changed"], false);
}

#[tokio::test]
async fn replacement_rejects_unrunnable_or_history_breaking_definitions_without_a_write() {
    let (_temp, state, before) = replacement_fixture("workflow-replace-invalid");
    let app = router(Arc::clone(&state));
    let mut cases = vec![];
    for (pointer, value) in [
        ("/stages/0/name", serde_json::json!("renamed")),
        ("/stages/1/name", serde_json::json!("review")),
        ("/stages/0/agent", serde_json::json!("nonexistent-agent")),
        (
            "/stages/0/agent_provider",
            serde_json::json!("unknown-model"),
        ),
        (
            "/stages/0/policy/transition",
            serde_json::json!("sometimes"),
        ),
    ] {
        let mut invalid = before.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        cases.push(invalid);
    }
    let mut invalid = before.clone();
    invalid["stages"][0]["environment"] = serde_json::json!("absent");
    cases.push(invalid);
    let mut invalid = before.clone();
    invalid["typo"] = serde_json::json!(true);
    cases.push(invalid);
    for invalid in cases {
        let (status, body) = replace_workflow(&app, &before, &invalid).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let db = Db::open(&state.config.db_path).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(
                &db.get_pipeline_item("task-1")
                    .unwrap()
                    .unwrap()
                    .pipeline_def
                    .unwrap()
            )
            .unwrap(),
            before
        );
        assert!(!db
            .stage_run_workflow_superseded("task-1", "run-old")
            .unwrap());
    }
}

#[tokio::test]
async fn future_stage_and_description_edits_keep_current_provider_stamp() {
    let (_temp, state, before) = replacement_fixture("workflow-replace-future");
    let app = router(Arc::clone(&state));
    let mut after = before.clone();
    after["stages"][0]["description"] = serde_json::json!("updated description");
    after["stages"][1]["agent_provider"] = serde_json::json!("codex-gpt-6-astra");
    let (status, body) = replace_workflow(&app, &before, &after).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["supersededRunIds"], serde_json::json!([]));
    let db = Db::open(&state.config.db_path).unwrap();
    assert!(!db
        .stage_run_workflow_superseded("task-1", "run-old")
        .unwrap());
}

#[tokio::test]
async fn replacement_preserves_history_and_compiles_legacy_post_snapshots() {
    let (_temp, state, before) = replacement_fixture("workflow-replace-legacy");
    let db = Db::open(&state.config.db_path).unwrap();
    let mut legacy = before.clone();
    legacy["stages"][0]["post_action"] = serde_json::json!({
        "name": "commit", "agent": "commit", "prompt": "commit the changes"
    });
    db.update_test_pipeline_item_pipeline_def("task-1", &legacy.to_string())
        .unwrap();
    db.insert_stage_run(NewStageRun {
        id: "historical-post",
        task_id: "task-1",
        stage: "commit",
        kind: "post",
        agent: Some("commit"),
        agent_provider: Some("claude"),
        model: None,
        effort: None,
        status: "succeeded",
        result: None,
        feedback: None,
        session_id: None,
        provider_session_id: None,
        cwd: None,
        resumed_from_run_id: None,
    })
    .unwrap();
    let app = router(Arc::clone(&state));
    let (status, _) = replace_workflow(&app, &legacy, &before).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "historical post cannot disappear"
    );
    let mut canonical = before.clone();
    canonical["stages"][0]["post"] = legacy["stages"][0]["post_action"].clone();
    let mut moved = canonical.clone();
    moved["stages"][0].as_object_mut().unwrap().remove("post");
    moved["stages"][1]["post"] = canonical["stages"][0]["post"].clone();
    let (status, _) = replace_workflow(&app, &legacy, &moved).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "historical post cannot change owners"
    );
    let (status, body) = replace_workflow(&app, &legacy, &canonical).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["supersededRunIds"], serde_json::json!([]));
    assert!(body["workflowDefinition"]["stages"][0]
        .get("post_action")
        .is_none());
}

// --- Publishing a task's remaining stages with its plan -------------------

/// The research-grown shape: a manual `plan` stage appended to the task
/// that carried the research, with the planning run live.
/// `plan_publication_fixture` with a stop between a finalization attempt
/// acquiring the source and doing anything to it.
///
/// `arrivals` yields one message per attempt that got past acquisition, and
/// `release` hands through exactly one attempt per added permit.
struct BarrieredFixture {
    _temp: tempfile::TempDir,
    state: Arc<AppState>,
    before: Value,
    arrivals: tokio::sync::mpsc::UnboundedReceiver<String>,
    release: Arc<tokio::sync::Semaphore>,
}

fn plan_publication_fixture_with_barrier(label: &str) -> BarrieredFixture {
    let (temp, state, before) = plan_publication_fixture(label);
    let mut state = Arc::try_unwrap(state).unwrap_or_else(|_| panic!("sole owner of the fixture"));
    let (arrived, arrivals) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    state.transfer_source_barrier = Some(Arc::new(crate::http_api::TransferSourceBarrier {
        arrived,
        release: Arc::clone(&release),
    }));
    BarrieredFixture {
        _temp: temp,
        state: Arc::new(state),
        before,
        arrivals,
        release,
    }
}

fn plan_publication_fixture(label: &str) -> (tempfile::TempDir, Arc<AppState>, Value) {
    let (temp, repo_path) = workflow_test_repo(label);
    let before = serde_json::json!({"name": "research", "stages": [
        {"name": "research", "agent": "researcher", "prompt": "$TASK_PROMPT",
         "policy": {"transition": "manual"}},
        {"name": "plan", "agent": "plan", "prompt": "Deliver the chosen outcome.",
         "policy": {"transition": "manual"}}
    ]});
    let saved = before.clone();
    let state = test_state_with_seed(label, "Studio Mac", move |db| {
        seed_workflow_task(
            db,
            &repo_path,
            "task-1",
            "research",
            "plan",
            &saved.to_string(),
        );
        db.insert_stage_run(NewStageRun {
            id: "run-research",
            task_id: "task-1",
            stage: "research",
            kind: "main",
            agent: Some("researcher"),
            agent_provider: Some("claude"),
            model: None,
            effort: None,
            status: "succeeded",
            result: Some(r#"{"status":"success","summary":"brief"}"#),
            feedback: None,
            session_id: Some("session-research"),
            provider_session_id: None,
            cwd: Some(&repo_path),
            resumed_from_run_id: None,
        })
        .unwrap();
        db.insert_stage_run(NewStageRun {
            id: "run-plan",
            task_id: "task-1",
            stage: "plan",
            kind: "main",
            agent: Some("plan"),
            agent_provider: Some("claude"),
            model: None,
            effort: None,
            status: "running",
            result: None,
            feedback: None,
            session_id: Some("session-plan"),
            provider_session_id: None,
            cwd: Some(&repo_path),
            resumed_from_run_id: None,
        })
        .unwrap();
    });
    (temp, state, before)
}

fn single_reviewer_suffix(before: &Value) -> Value {
    let mut after = before.clone();
    after["revision_limit"] = serde_json::json!(3);
    let stages = after["stages"].as_array_mut().unwrap();
    stages.push(serde_json::json!({
        "name": "in progress", "agent": "implement",
        "prompt": "Deliver the approved plan: $PLAN_RESULT",
        "policy": {"transition": "manual", "revision_transition": "auto"},
        "post": {"name": "commit", "agent": "commit", "prompt": "Commit. $PLAN_RESULT"}
    }));
    stages.push(serde_json::json!({
        "name": "review", "agent": "review", "prompt": "Review $BRANCH against $PLAN_RESULT",
        "policy": {"transition": "auto"}
    }));
    stages.push(serde_json::json!({
        "name": "pr", "agent": "pr", "prompt": "Open a PR for $BRANCH.",
        "policy": {"transition": "manual"},
        "post": {"name": "approve", "agent": "approve", "prompt": "Approve $BRANCH."}
    }));
    after
}

async fn complete_plan(
    app: &axum::Router,
    summary: &str,
    extension: Option<(&Value, &Value)>,
) -> (StatusCode, Value) {
    let mut args = serde_json::json!({
        "task_id": "task-1", "status": "success", "summary": summary
    });
    if let Some((expected, definition)) = extension {
        args["expected_definition"] = expected.clone();
        args["workflow_definition"] = definition.clone();
    }
    // Resolve through the public catalog, as MCP and the CLI do, so the tool
    // surface and the route are proven together.
    let request = kanna_tool_catalog::resolve_request(
        &kanna_tool_catalog::bundled_catalog(),
        "kanna_complete_stage",
        &args,
    )
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::post(request.path)
                .header("content-type", "application/json")
                .body(Body::from(request.body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into())),
    )
}

fn pinned(state: &Arc<AppState>) -> Value {
    let db = Db::open(&state.config.db_path).unwrap();
    let task = db.get_pipeline_item("task-1").unwrap().unwrap();
    serde_json::from_str(task.pipeline_def.as_deref().unwrap()).unwrap()
}

#[tokio::test]
async fn plan_completion_publishes_its_stages_and_stamps_the_plan() {
    let (_temp, state, before) = plan_publication_fixture("plan-publish");
    let app = router(Arc::clone(&state));
    let after = single_reviewer_suffix(&before);

    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Explicit confirmation: an older server ignoring the arguments answers
    // without this field, which is what stops a plain success reading as a
    // published workflow.
    assert_eq!(body["workflowExtended"], serde_json::json!(true));

    let saved = pinned(&state);
    assert_eq!(
        saved["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|stage| stage["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["research", "plan", "in progress", "review", "pr"]
    );
    assert_eq!(saved["revision_limit"], serde_json::json!(3));
    // The plan rides inside the pinned workflow, so it survives every later
    // stage without a second durable record.
    assert_eq!(saved["plan_context"]["source_run_id"], "run-plan");
    assert_eq!(saved["plan_context"]["stage"], "plan");
    assert!(saved["plan_context"]["result"]
        .as_str()
        .unwrap()
        .contains("the full plan"));

    let db = Db::open(&state.config.db_path).unwrap();
    let run = db.stage_run("run-plan").unwrap().unwrap();
    assert_eq!(run.status, "succeeded");
    // Manual plan gate: the task stays where the human reads both.
    assert_eq!(
        db.get_pipeline_item("task-1")
            .unwrap()
            .unwrap()
            .stage
            .unwrap(),
        "plan"
    );
}

#[tokio::test]
async fn a_rejected_extension_records_no_plan_at_all() {
    let (_temp, state, before) = plan_publication_fixture("plan-publish-atomic");
    let app = router(Arc::clone(&state));
    // An unsupported suffix: no recipe ends at a bare `ship` stage.
    let mut after = single_reviewer_suffix(&before);
    after["stages"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "name": "ship", "agent": "ship", "prompt": "Ship it.",
            "policy": {"transition": "manual"}
        }));

    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let db = Db::open(&state.config.db_path).unwrap();
    assert_eq!(db.stage_run("run-plan").unwrap().unwrap().status, "running");
    assert_eq!(pinned(&state), before);
}

#[tokio::test]
async fn a_plan_may_not_rewrite_the_stages_that_produced_it() {
    let (_temp, state, before) = plan_publication_fixture("plan-publish-prefix");
    let app = router(Arc::clone(&state));
    let mut after = single_reviewer_suffix(&before);
    after["stages"][0]["agent"] = serde_json::json!("implement");

    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.as_str()
            .unwrap_or_default()
            .contains("only append stages"),
        "{body}"
    );
    assert_eq!(pinned(&state), before);
}

#[tokio::test]
async fn a_published_plan_survives_replays_and_refuses_differing_retries() {
    let (_temp, state, before) = plan_publication_fixture("plan-publish-retry");
    let app = router(Arc::clone(&state));
    let after = single_reviewer_suffix(&before);
    let (status, _) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK);
    let published = pinned(&state);

    // An exact replay is a no-op even though `plan` is no longer the tail —
    // and it still answers that the stages are published, because a missing
    // flag means "this server did not publish them".
    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workflowExtended"], serde_json::json!(true));
    assert_eq!(pinned(&state), published);

    // A differing retry must not replace the plan the published stages were
    // chosen under.
    let (status, body) = complete_plan(&app, "a different plan", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(pinned(&state), published);
    let db = Db::open(&state.config.db_path).unwrap();
    assert!(db
        .stage_run("run-plan")
        .unwrap()
        .unwrap()
        .result
        .unwrap()
        .contains("the full plan"));

    // One result and one plan entry, linked by operation and result id; the
    // replay and the refused retry added nothing.
    let files = super::actions::ledger_files(&state.config.db_path, "task-1");
    let kinds = files.iter().map(|file| file.kind).collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            crate::db::task_store::LedgerEntryKind::Result,
            crate::db::task_store::LedgerEntryKind::Plan,
        ]
    );
    let (result, plan) = (&files[0], &files[1]);
    assert_eq!(result.message.as_deref(), Some("the full plan"));
    assert_eq!(result.body()["request"]["publishesWorkflow"], true);
    assert_eq!(
        plan.envelope["operation_id"],
        result.envelope["operation_id"]
    );
    assert_eq!(plan.body()["result_id"], result.envelope["entry_id"]);
    assert_eq!(plan.body()["before"], before);
    assert_eq!(plan.body()["after"], published);
    assert_eq!(plan.body()["source"], "agent");

    // task.json projects the exact pinned workflow the task now runs.
    let task_dir = crate::task_store::task_dir_for(&db, &state.config.db_path, "task-1").unwrap();
    let snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(task_dir.join("task.json")).unwrap()).unwrap();
    assert_eq!(snapshot["workflow"]["definition"], published);
}

#[tokio::test]
async fn a_stale_read_cannot_publish_stages_over_a_concurrent_edit() {
    let (_temp, state, before) = plan_publication_fixture("plan-publish-stale");
    let app = router(Arc::clone(&state));
    let mut edited = before.clone();
    edited["stages"][1]["prompt"] = serde_json::json!("Deliver the chosen outcome, revised.");
    let (status, body) = replace_workflow(&app, &before, &edited).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let after = single_reviewer_suffix(&before);
    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let db = Db::open(&state.config.db_path).unwrap();
    assert_eq!(db.stage_run("run-plan").unwrap().unwrap().status, "running");
    // The refused publication left no result or plan entry behind; only the
    // earlier accepted edit is in the ledger.
    let files = super::actions::ledger_files(&state.config.db_path, "task-1");
    assert!(files
        .iter()
        .all(|file| file.kind == crate::db::task_store::LedgerEntryKind::Plan));
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].body()["operation"], "replace");
    assert!(files[0].body()["result_id"].is_null());
}

#[tokio::test]
async fn an_ordinary_edit_carries_the_stamped_plan_and_cannot_author_one() {
    let (_temp, state, before) = plan_publication_fixture("plan-publish-edit");
    let app = router(Arc::clone(&state));
    let after = single_reviewer_suffix(&before);
    let (status, _) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK);
    let published = pinned(&state);

    // An edit that simply does not resend the stamp keeps it.
    let mut retargeted = published.clone();
    retargeted["plan_context"].take();
    retargeted.as_object_mut().unwrap().remove("plan_context");
    retargeted["stages"][3]["agent"] = serde_json::json!("qa-dispatcher");
    let (status, body) = replace_workflow(&app, &published, &retargeted).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(pinned(&state)["plan_context"], published["plan_context"]);
    assert_eq!(pinned(&state)["stages"][3]["agent"], "qa-dispatcher");

    // An edit that rewrites it is refused: the stamp is Kanna's provenance.
    let current = pinned(&state);
    let mut forged = current.clone();
    forged["plan_context"]["result"] = serde_json::json!("a plan nobody recorded");
    let (status, body) = replace_workflow(&app, &current, &forged).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(pinned(&state), current);
}

#[tokio::test]
async fn only_a_final_manual_plan_stage_may_publish_remaining_stages() {
    let (_temp, state, before) = plan_publication_fixture("plan-publish-position");
    let app = router(Arc::clone(&state));
    {
        // Move the task back to the research stage: a researcher must not
        // be able to publish delivery stages for itself.
        let db = Db::open(&state.config.db_path).unwrap();
        db.update_pipeline_item_stage("task-1", "research").unwrap();
    }
    let after = single_reviewer_suffix(&before);
    let (status, body) = complete_plan(&app, "brief", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(pinned(&state), before);
}

#[tokio::test]
async fn an_advance_fenced_on_a_stale_workflow_is_refused_before_anything_is_scheduled() {
    let (_temp, state, before) = plan_publication_fixture("plan-advance-fence");
    let app = router(Arc::clone(&state));
    let after = single_reviewer_suffix(&before);
    let (status, _) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK);

    // The caller read the workflow before the plan published its stages, so
    // the tail it would advance into is not the one it inspected.
    let request = kanna_tool_catalog::resolve_request(
        &kanna_tool_catalog::bundled_catalog(),
        "kanna_advance_stage",
        &serde_json::json!({
            "task_id": "task-1", "source": "operator", "expected_definition": before
        }),
    )
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::post(request.path)
                .header("content-type", "application/json")
                .body(Body::from(request.body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("pinned workflow changed"),
        "{}",
        String::from_utf8_lossy(&body)
    );
    let db = Db::open(&state.config.db_path).unwrap();
    assert_eq!(
        db.get_pipeline_item("task-1")
            .unwrap()
            .unwrap()
            .stage
            .unwrap(),
        "plan"
    );
}

/// A suffix differing from the published one, under the *same* plan summary.
fn no_review_suffix(before: &Value) -> Value {
    let mut after = before.clone();
    after["revision_limit"] = serde_json::json!(2);
    let stages = after["stages"].as_array_mut().unwrap();
    stages.push(serde_json::json!({
        "name": "in progress", "agent": "implement", "prompt": "Build it.",
        "policy": {"transition": "manual"},
        "post": {"name": "commit", "agent": "commit", "prompt": "Commit."}
    }));
    stages.push(serde_json::json!({
        "name": "pr", "agent": "pr", "prompt": "Open a PR for $BRANCH.",
        "policy": {"transition": "manual"},
        "post": {"name": "approve", "agent": "approve", "prompt": "Approve $BRANCH."}
    }));
    after
}

/// The replay identity of a combined completion is the plan *and* the stages
/// it published. A summary alone cannot identify it: the same plan text with a
/// different review depth, provider or revision budget is a different
/// publication, and answering it as a successful replay would report stages
/// that were never stored.
#[tokio::test]
async fn a_same_summary_retry_with_different_stages_is_refused() {
    let (_temp, state, before) = plan_publication_fixture("plan-retry-different-suffix");
    let app = router(Arc::clone(&state));
    let published_suffix = single_reviewer_suffix(&before);
    let (status, body) =
        complete_plan(&app, "the full plan", Some((&before, &published_suffix))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let published = pinned(&state);
    assert_eq!(published["stages"].as_array().unwrap().len(), 5);

    let (status, body) = complete_plan(
        &app,
        "the full plan",
        Some((&before, &no_review_suffix(&before))),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        pinned(&state),
        published,
        "the stored suffix must be intact"
    );
}

/// Same request, different expected definition: the caller read a different
/// starting point, so it is a different operation even where the plan matches.
#[tokio::test]
async fn a_retry_against_a_different_expected_definition_is_refused() {
    let (_temp, state, before) = plan_publication_fixture("plan-retry-different-expected");
    let app = router(Arc::clone(&state));
    let after = single_reviewer_suffix(&before);
    let (status, _) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK);
    let published = pinned(&state);

    // The published document itself, offered as the thing that was read.
    let (status, body) = complete_plan(&app, "the full plan", Some((&published, &after))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(pinned(&state), published);
}

/// An ordinary completion followed by a combined one carrying the same summary
/// must still publish. Comparing results alone reads the recorded verdict as a
/// replay and returns success having stored nothing.
#[tokio::test]
async fn an_ordinary_completion_does_not_answer_for_a_later_publication() {
    let (_temp, state, before) = plan_publication_fixture("plan-ordinary-then-combined");
    let app = router(Arc::clone(&state));
    let (status, body) = complete_plan(&app, "the full plan", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["workflowExtended"].is_null());
    assert_eq!(
        pinned(&state),
        before,
        "an ordinary completion publishes nothing"
    );

    let after = single_reviewer_suffix(&before);
    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workflowExtended"], serde_json::json!(true));
    assert_eq!(
        pinned(&state)["stages"].as_array().unwrap().len(),
        5,
        "the stages must actually be stored"
    );
}

/// The exact replay still answers truthfully after the workflow has moved on.
/// The confirmation reads durable provenance stamped on the publication, not a
/// comparison with whatever is pinned at the moment.
#[tokio::test]
async fn an_exact_replay_is_still_confirmed_after_a_later_edit() {
    let (_temp, state, before) = plan_publication_fixture("plan-replay-after-edit");
    let app = router(Arc::clone(&state));
    let after = single_reviewer_suffix(&before);
    let (status, _) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK);

    // An ordinary edit retargets the review stage; the stamp rides along.
    let published = pinned(&state);
    let mut edited = published.clone();
    edited["stages"][3]["agent"] = serde_json::json!("qa-dispatcher");
    let (status, body) = replace_workflow(&app, &published, &edited).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_ne!(pinned(&state), published);

    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workflowExtended"], serde_json::json!(true));
    assert_eq!(pinned(&state)["stages"][3]["agent"], "qa-dispatcher");
}

/// Post the completion body directly, so a test can set the run binding and
/// attempt key the MCP/CLI adapters normally add.
async fn complete_plan_bound(app: &axum::Router, body: serde_json::Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/tasks/task-1/actions/complete-stage")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into())),
    )
}

/// The adapters bind a completion to the exact run that spawned, and retry
/// under a stable attempt key. Neither binding may turn a different
/// publication into a replay.
#[tokio::test]
async fn bound_and_keyed_retries_carry_the_same_publication_identity() {
    let (_temp, state, before) = plan_publication_fixture("plan-bound-retries");
    let app = router(Arc::clone(&state));
    let published_suffix = single_reviewer_suffix(&before);
    let publish = |suffix: &Value, key: Option<&str>| {
        let mut body = serde_json::json!({
            "runId": "run-plan",
            "status": "success",
            "summary": "the full plan",
            "expectedDefinition": before,
            "workflowDefinition": suffix,
        });
        if let Some(key) = key {
            body["completionAttemptKey"] = serde_json::json!(key);
        }
        body
    };

    let (status, body) = complete_plan_bound(&app, publish(&published_suffix, None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workflowExtended"], serde_json::json!(true));
    let published = pinned(&state);

    // Explicit run binding, exact request: a replay, still confirmed.
    let (status, body) = complete_plan_bound(&app, publish(&published_suffix, None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workflowExtended"], serde_json::json!(true));

    // Explicit run binding, different stages under the same summary.
    let (status, body) = complete_plan_bound(&app, publish(&no_review_suffix(&before), None)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(pinned(&state), published);

    // An attempt key does not launder it either.
    let (status, body) =
        complete_plan_bound(&app, publish(&no_review_suffix(&before), Some("attempt-1"))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(pinned(&state), published);

    // The same key replaying the exact published request stays a replay.
    let (status, body) =
        complete_plan_bound(&app, publish(&published_suffix, Some("attempt-2"))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workflowExtended"], serde_json::json!(true));
    assert_eq!(pinned(&state), published);
}

// --- Plan publication against a transfer finalizing the same task ----------

/// Seed the outgoing transfer and work item the shared source finalization path
/// reads, for the task `plan_publication_fixture` created.
fn seed_outgoing_transfer(db: &Db, transfer_id: &str) {
    let task = db.get_pipeline_item("task-1").unwrap().unwrap();
    let run = db.latest_stage_run("task-1").unwrap();
    db.insert_task_transfer(&crate::db::NewTaskTransfer {
        id: transfer_id.into(),
        direction: "outgoing".into(),
        status: "pending".into(),
        source_peer_id: Some("peer-source".into()),
        target_peer_id: Some("peer-destination".into()),
        source_desktop_id: None,
        target_desktop_id: None,
        source_task_id: Some("task-1".into()),
        local_task_id: Some("task-1".into()),
        error: None,
        payload_json: Some(
            serde_json::json!({
                "target_peer_id": "peer-destination",
                "task": {
                    "source_peer_id": "peer-source",
                    "source_task_id": "task-1",
                    "resume_session_id": null,
                    "stage": task.stage,
                    "pipeline": task.pipeline,
                    "workflow_definition": task.pipeline_def,
                    "source_run_id": run.as_ref().map(|r| &r.id),
                    "model": run.as_ref().and_then(|r| r.model.as_ref()),
                    "effort": run.as_ref().and_then(|r| r.effort.as_ref()),
                    "agent_type": "pty",
                    "agent_provider": run.as_ref().and_then(|r| r.agent_provider.as_deref()).or(task.agent_provider.as_deref()).unwrap_or("claude"),
                    "content_commitment": RECEIPT_COMMITMENT,
                },
                "repo": { "mode": "reuse-local", "path": "/repo" },
                "artifacts": [],
            })
            .to_string(),
        ),
    })
    .expect("outgoing transfer");
    db.enqueue_transfer_work(
        &format!("finalize:{transfer_id}"),
        "finalize",
        Some(transfer_id),
        &serde_json::json!({"transfer_id":transfer_id, "selection_commitment":
            crate::transfer_engine::payload::parse_outgoing_transfer_payload(&serde_json::from_str(db.get_task_transfer(transfer_id).unwrap().unwrap().payload_json.as_deref().unwrap()).unwrap()).unwrap().task.selection_commitment().unwrap()
        }).to_string(),
    )
    .expect("queue the finalization work item");
}

/// What this source persisted at push time, and what a valid receipt must
/// report back before it is allowed to close anything.
const RECEIPT_COMMITMENT: &str = "commitment-for-the-payload-that-shipped";

/// A receipt the destination would send for the payload above, in the shape the
/// real `outgoing-committed` work carries.
fn committed_receipt(transfer_id: &str) -> serde_json::Value {
    serde_json::json!({
        "transfer_id": transfer_id,
        "source_task_id": "task-1",
        "content_commitment": RECEIPT_COMMITMENT,
        "destination_repo_id": "repo-destination",
        "destination_local_task_id":
            crate::transfer_engine::session::destination_task_id(transfer_id),
    })
}

fn committed_work(transfer_id: &str) -> crate::db::TransferWorkItem {
    crate::db::TransferWorkItem {
        id: format!("committed:{transfer_id}"),
        kind: "outgoing-committed".to_string(),
        transfer_id: Some(transfer_id.to_string()),
        payload_json: committed_receipt(transfer_id).to_string(),
        attempts: 1,
    }
}

/// The event body the real dispatch reads a finalization's transfer id from.
fn finalize_payload(transfer_id: &str) -> String {
    serde_json::json!({ "transfer_id": transfer_id }).to_string()
}

fn finalize_work(state: &Arc<AppState>, transfer_id: &str) -> crate::db::TransferWorkItem {
    let db = Db::open(&state.config.db_path).unwrap();
    let transfer = db.get_task_transfer(transfer_id).unwrap().unwrap();
    let payload = crate::transfer_engine::payload::parse_outgoing_transfer_payload(
        &serde_json::from_str(transfer.payload_json.as_deref().unwrap()).unwrap(),
    )
    .unwrap();
    crate::db::TransferWorkItem {
        id: format!("finalize:{transfer_id}"),
        kind: "finalize".to_string(),
        transfer_id: Some(transfer_id.to_string()),
        payload_json: serde_json::json!({"transfer_id": transfer_id, "selection_commitment": payload.task.selection_commitment().unwrap()}).to_string(),
        attempts: 1,
    }
}

fn finalization_ran(state: &Arc<AppState>, transfer_id: &str) -> bool {
    Db::open(&state.config.db_path)
        .expect("db")
        .read_transfer_work_observation(&format!("finalize:{transfer_id}"), "finalization-outcome")
        .expect("read the finalization verdict")
        .is_some()
}

/// A plan publication and a transfer finalizing the same task cannot both
/// believe they own its workflow.
///
/// Finalization shuts the source agent down and *then* serializes the pinned
/// workflow into the payload, so a plan published in between would be handed to
/// a destination that may drop it — after the source had already quit. A
/// snapshot check before finalization's first `await` cannot close that window;
/// only shared ownership can.
///
/// Here the transfer takes the task first. The publication is refused and — the
/// part that matters — records nothing at all: not the suffix, not the plan.
#[tokio::test]
async fn a_plan_cannot_publish_while_a_transfer_owns_the_task() {
    let (_temp, state, before) = plan_publication_fixture("plan-vs-transfer-owned");
    let app = router(Arc::clone(&state));
    {
        let db = Db::open(&state.config.db_path).expect("db");
        seed_outgoing_transfer(&db, "transfer-owned");
    }

    // The real shared source path. It claims the task's workflow before it
    // observes or shuts anything down, then fails further downstream on this
    // fixture — leaving the transfer live and holding the task, which is
    // exactly the state the publication must refuse against.
    let _ = crate::transfer_engine::push::run_finalization_for_test(
        &state,
        &finalize_work(&state, "transfer-owned"),
        "transfer-owned",
    )
    .await;

    let after = single_reviewer_suffix(&before);
    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body.as_str()
            .unwrap_or_default()
            .contains("owns its workflow"),
        "the refusal must name the transfer that holds the task: {body}"
    );

    // Nothing was committed: no suffix, no stamp, and the plan run is still
    // running rather than recorded successful.
    assert_eq!(pinned(&state), before);
    let db = Db::open(&state.config.db_path).expect("db");
    let run = db.stage_run("run-plan").expect("read run").expect("run");
    assert_eq!(run.status, "running");
    assert_eq!(run.result, None);
}

/// The same race the other way round: the plan publishes first, and the
/// transfer is refused while its source is still alive.
///
/// The claim and the plan check are one transaction, so the publication that
/// committed first is seen by finalization however late it lands — including on
/// a resumed or retried attempt.
#[tokio::test]
async fn a_transfer_is_refused_when_the_plan_publishes_first() {
    let (_temp, state, before) = plan_publication_fixture("plan-vs-transfer-published");
    let app = router(Arc::clone(&state));
    {
        let db = Db::open(&state.config.db_path).expect("db");
        seed_outgoing_transfer(&db, "transfer-published");
    }

    let after = single_reviewer_suffix(&before);
    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workflowExtended"], serde_json::json!(true));

    let error = crate::transfer_engine::push::run_finalization_for_test(
        &state,
        &finalize_work(&state, "transfer-published"),
        "transfer-published",
    )
    .await
    .expect_err("a published plan was handed over");
    assert!(
        error.contains("carries a published plan"),
        "the transfer must be refused for preservation: {error}"
    );
    assert!(
        !finalization_ran(&state, "transfer-published"),
        "the source session was finalized despite the refusal"
    );
}

/// Ownership is released when the transfer settles, and the rule still holds
/// for whatever comes next.
///
/// A claim left behind by a transfer that failed must not block the task's plan
/// forever; a plan that publishes in that window must still refuse the *next*
/// transfer before it touches the source. This is the retry / late-publication
/// boundary, answered by the same ownership rule rather than a second one.
#[tokio::test]
async fn a_settled_transfer_releases_the_task_and_a_later_plan_still_refuses_the_next_one() {
    let (_temp, state, before) = plan_publication_fixture("plan-vs-transfer-retry");
    let app = router(Arc::clone(&state));
    {
        let db = Db::open(&state.config.db_path).expect("db");
        seed_outgoing_transfer(&db, "transfer-first");
    }
    let _ = crate::transfer_engine::push::run_finalization_for_test(
        &state,
        &finalize_work(&state, "transfer-first"),
        "transfer-first",
    )
    .await;

    // While it is live, the task is held.
    let after = single_reviewer_suffix(&before);
    let (status, _) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // A failed transfer row is *not* settlement while its work can still run:
    // the wrapper fails the transfer and the queue retries the same item, and
    // that retry shuts down a source. Ownership has to outlive the display
    // status.
    {
        let db = Db::open(&state.config.db_path).expect("db");
        db.fail_outgoing_task_transfer("transfer-first", "the destination went away")
            .expect("fail the transfer");
        assert_eq!(
            db.task_workflow_is_claimed_by_transfer("task-1")
                .expect("read ownership")
                .as_deref(),
            Some("transfer-first"),
            "a failed transfer whose work can still retry has not finished with the source"
        );
    }
    let (status, _) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Genuine settlement: the transfer is terminal *and* its source-effect work
    // has finished. Only now does the task stop being owned.
    {
        let db = Db::open(&state.config.db_path).expect("db");
        db.complete_transfer_work("finalize:transfer-first")
            .expect("finish the finalization work");
        assert_eq!(
            db.task_workflow_is_claimed_by_transfer("task-1")
                .expect("read ownership"),
            None,
            "a genuinely settled transfer must not keep holding the task"
        );
    }
    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workflowExtended"], serde_json::json!(true));

    // A later transfer of the same task is refused before its source is
    // touched, by the same claim.
    {
        let db = Db::open(&state.config.db_path).expect("db");
        seed_outgoing_transfer(&db, "transfer-second");
    }
    let error = crate::transfer_engine::push::run_finalization_for_test(
        &state,
        &finalize_work(&state, "transfer-second"),
        "transfer-second",
    )
    .await
    .expect_err("the retried transfer shipped a published plan");
    assert!(error.contains("carries a published plan"), "{error}");
    assert!(!finalization_ran(&state, "transfer-second"));
}

// --- Ownership across the real failure / retry / settlement lifecycle -------

/// Wait for the next attempt to reach the barrier, failing rather than hanging.
///
/// Arrival is published by the attempt itself, after it has acquired the source
/// and before it can touch it. That is what makes it a per-attempt signal: the
/// durable claim row would read as "arrived" for an attempt that has not even
/// started, because an earlier one already wrote it.
async fn next_arrival(arrivals: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(20), arrivals.recv())
        .await
        .expect("an attempt must reach the source barrier")
        .expect("the barrier channel must stay open")
}

/// Wait for a running attempt to reach the barrier, failing with what it
/// actually did if it finishes first.
///
/// An attempt refused at acquisition never arrives, and waiting for it would
/// otherwise time out. Racing its completion turns that into an assertion that
/// names the refusal.
async fn arrival_of(
    arrivals: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    attempt: &mut tokio::task::JoinHandle<Result<(), String>>,
) -> String {
    tokio::select! {
        arrived = arrivals.recv() => arrived.expect("the barrier channel must stay open"),
        // Its outcome text may be a downstream reporting failure rather than
        // the refusal itself — the wrapper replaces the reason when it cannot
        // reach the destination — so what this asserts is the *shape*: the
        // attempt ended without ever acquiring the source.
        finished = attempt => panic!(
            "the attempt ended without reaching the source barrier, so it never acquired the \
             source it was supposed to own. Its outcome was {finished:?}"
        ),
    }
}

fn work_status(state: &Arc<AppState>, work_id: &str) -> Option<String> {
    Db::open(&state.config.db_path)
        .expect("db")
        .transfer_work_status(work_id)
        .expect("read work status")
}

fn transfer_status(state: &Arc<AppState>, transfer_id: &str) -> String {
    Db::open(&state.config.db_path)
        .expect("db")
        .get_task_transfer(transfer_id)
        .expect("read transfer")
        .expect("transfer")
        .status
}

/// A retry of a *failed* transfer still owns the source, and the plan cannot
/// publish underneath it.
///
/// This is the lifecycle the display status gets wrong. The real wrapper marks
/// the transfer `failed` when finalization errors, and the real queue requeues
/// the same work item while attempts remain — so attempt two shuts a source down
/// for a transfer whose row says it is over. Reading ownership from the row
/// alone would make that attempt invisible to publication.
///
/// Both attempts are claimed from the real queue, so each carries the queue's
/// own running state and attempt count rather than a fabricated one, and the
/// retry's backoff is stepped over by claiming as of a later instant rather than
/// slept through. Attempt two publishes its own arrival after acquiring the
/// source and before touching it; that arrival — not the durable claim row,
/// which attempt one already wrote — is what proves this attempt got past
/// acquisition.
#[tokio::test]
async fn a_retry_of_a_failed_transfer_still_excludes_publication() {
    let BarrieredFixture {
        _temp,
        state,
        before,
        mut arrivals,
        release,
    } = plan_publication_fixture_with_barrier("plan-vs-transfer-retry-own");
    let app = router(Arc::clone(&state));
    {
        let db = Db::open(&state.config.db_path).expect("db");
        seed_outgoing_transfer(&db, "transfer-retry");
    }
    let work_id = "finalize:transfer-retry";

    // Attempt one, claimed from the real queue and run through the real
    // dispatch and settlement. It is released immediately; what matters here is
    // that the wrapper fails the transfer and the queue requeues the same work.
    let first_item = {
        let db = Db::open(&state.config.db_path).expect("db");
        db.claim_next_transfer_work(&[])
            .expect("claim")
            .expect("the queued finalization")
    };
    assert_eq!(first_item.id, work_id);
    assert_eq!(first_item.attempts, 1);
    let first = {
        let state = Arc::clone(&state);
        let item = first_item.clone();
        tokio::spawn(async move {
            crate::transfer_engine::run_one_work_item_for_test(&state, &item).await
        })
    };
    assert_eq!(
        next_arrival(&mut arrivals).await,
        "transfer-retry/finalize:transfer-retry"
    );
    release.add_permits(1);
    let first = first.await.expect("attempt one must not panic");
    let first_error = first.expect_err("this fixture cannot finalize");
    assert_eq!(transfer_status(&state, "transfer-retry"), "failed");
    assert_eq!(
        work_status(&state, work_id).as_deref(),
        Some("pending"),
        "the same work must still be retriable"
    );

    // Attempt two, also claimed from the real queue — stepping over the retry's
    // backoff rather than waiting it out, so the item this runs is the one the
    // queue would actually hand the engine.
    let second_item = {
        let db = Db::open(&state.config.db_path).expect("db");
        // A fixed instant far past any retry backoff: the schedule is stepped
        // over deterministically rather than slept through.
        db.claim_next_transfer_work_as_of(&[], "2099-01-01 00:00:00")
            .expect("claim")
            .expect("the requeued finalization")
    };
    assert_eq!(second_item.id, work_id, "the retry must be the same work");
    assert_eq!(second_item.attempts, 2, "the queue must count this attempt");
    assert_eq!(work_status(&state, work_id).as_deref(), Some("running"));
    let second = {
        let state = Arc::clone(&state);
        let item = second_item.clone();
        tokio::spawn(async move {
            crate::transfer_engine::run_one_work_item_for_test(&state, &item).await
        })
    };

    // The retry got past acquisition — which a status-only ownership rule would
    // never have allowed — and is now held before any source effect.
    let mut second = second;
    assert_eq!(
        arrival_of(&mut arrivals, &mut second).await,
        "transfer-retry/finalize:transfer-retry"
    );
    // Held: it has acquired the source and cannot finish until released.
    // (The finalization phase record is not a per-attempt marker — attempt one
    // already wrote it, and a retry short-circuits on that verdict.)
    assert!(
        !second.is_finished(),
        "the attempt must not pass the boundary before it is released"
    );

    // The real completion handler, against a genuinely in-flight retry.
    let db = Db::open(&state.config.db_path).expect("db");
    let events_before = db
        .list_task_events(
            &TaskEventScope::Tasks(vec!["task-1".to_string()]),
            0,
            i64::MAX,
            500,
        )
        .expect("read events")
        .len();
    let after = single_reviewer_suffix(&before);
    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(pinned(&state), before, "no suffix may be recorded");
    let run = db.stage_run("run-plan").expect("read run").expect("run");
    assert_eq!(run.status, "running", "no plan result may be recorded");
    assert_eq!(run.result, None);
    assert_eq!(
        db.list_task_events(
            &TaskEventScope::Tasks(vec!["task-1".to_string()]),
            0,
            i64::MAX,
            500,
        )
        .expect("read events")
        .len(),
        events_before,
        "a refused publication must append no event"
    );
    assert!(!second.is_finished(), "the retry must still be held");

    // Release it and prove where it actually got to. Attempt two reaches the
    // same downstream failure attempt one did, which it could only do by
    // passing acquisition — an implementation that refused failed-transfer
    // retries would end here with an ownership refusal instead, and one that
    // never acquired at all would not have reached the barrier above.
    release.add_permits(1);
    let second = second.await.expect("attempt two must not panic");
    let second_error = second.expect_err("this fixture still cannot finalize");
    assert_eq!(
        second_error, first_error,
        "the retry must reach the same downstream failure, not an ownership refusal"
    );
    assert!(
        !second_error.contains("ownership") && !second_error.contains("carries a published plan"),
        "the retry must not have been refused at acquisition: {second_error}"
    );
    assert_eq!(
        pinned(&state),
        before,
        "the plan still must not be recorded"
    );
}

/// Reverse ordering, through the real queue: a plan that published first
/// refuses a queued finalization before it wraps up, quits or stages anything.
///
/// The barrier sits after acquisition, so a refusal proves the attempt never
/// reached it — the source is untouched rather than merely un-shipped.
#[tokio::test]
async fn a_queued_finalization_after_a_publication_is_refused_before_wrap_up() {
    let BarrieredFixture {
        _temp,
        state,
        before,
        ..
    } = plan_publication_fixture_with_barrier("plan-then-queued-finalize");
    let app = router(Arc::clone(&state));
    {
        let db = Db::open(&state.config.db_path).expect("db");
        seed_outgoing_transfer(&db, "transfer-after-plan");
    }

    let after = single_reviewer_suffix(&before);
    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // No permit is ever added: reaching the barrier would hang this test, so
    // its completion is itself the proof that the attempt stopped before it.
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::transfer_engine::run_one_work_item_for_test(
            &state,
            &finalize_work(&state, "transfer-after-plan"),
        ),
    )
    .await
    .expect("the attempt must refuse rather than proceed to the source");
    assert!(outcome.is_err(), "a published plan was shipped");

    // The wrapper records the real reason on the transfer row before it tries
    // to tell the destination, so that is where the refusal is readable without
    // a sidecar to answer.
    let db = Db::open(&state.config.db_path).expect("db");
    let transfer = db
        .get_task_transfer("transfer-after-plan")
        .expect("read transfer")
        .expect("transfer");
    assert!(
        transfer
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("carries a published plan"),
        "the transfer must fail for preservation: {:?}",
        transfer.error
    );
    assert!(
        !finalization_ran(&state, "transfer-after-plan"),
        "the source session was finalized despite the refusal"
    );
}

/// A second transfer cannot take a source away from one that can still act on
/// it, and cannot make publication admissible by trying.
///
/// The active-outgoing uniqueness index does not cover failed rows, so a fresh
/// transfer for the same source really can exist beside an old failed one whose
/// work is still retriable.
#[tokio::test]
async fn a_second_transfer_cannot_take_a_source_that_is_still_owned() {
    let BarrieredFixture {
        _temp,
        state,
        before,
        ..
    } = plan_publication_fixture_with_barrier("plan-vs-transfer-second");
    let app = router(Arc::clone(&state));
    let db = Db::open(&state.config.db_path).expect("db");
    seed_outgoing_transfer(&db, "transfer-one");
    db.claim_task_workflow_for_transfer("transfer-one", "task-1")
        .expect("db")
        .expect("the first transfer owns the source");
    db.fail_outgoing_task_transfer("transfer-one", "finalization failed")
        .expect("fail the first transfer");

    // Its work is still retriable, so it has not finished with the source.
    seed_outgoing_transfer(&db, "transfer-two");
    let refusal = db
        .claim_task_workflow_for_transfer("transfer-two", "task-1")
        .expect("db")
        .expect_err("a live owner was displaced");
    assert!(refusal.contains("already owns"), "{refusal}");
    assert_eq!(
        db.task_workflow_is_claimed_by_transfer("task-1")
            .expect("read ownership")
            .as_deref(),
        Some("transfer-one"),
        "the owner must not have changed"
    );

    let after = single_reviewer_suffix(&before);
    let (status, _) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a failed takeover must not make publication admissible"
    );
    assert_eq!(pinned(&state), before);
}

/// Restart recovery keeps ownership, and only genuine exhaustion releases it.
///
/// A work row left `running` by a killed process is requeued to `pending` at
/// startup — still owned. Publication becomes possible only once the work has
/// genuinely finished with the source.
#[tokio::test]
async fn restart_recovery_keeps_ownership_until_the_work_is_genuinely_done() {
    let BarrieredFixture {
        _temp,
        state,
        before,
        ..
    } = plan_publication_fixture_with_barrier("plan-vs-transfer-restart");
    let app = router(Arc::clone(&state));
    let db = Db::open(&state.config.db_path).expect("db");
    seed_outgoing_transfer(&db, "transfer-restart");
    db.claim_task_workflow_for_transfer("transfer-restart", "task-1")
        .expect("db")
        .expect("owned");
    db.fail_outgoing_task_transfer("transfer-restart", "finalization failed")
        .expect("fail the transfer");
    // Interrupted mid-attempt, then recovered by the engine's startup requeue.
    db.claim_next_transfer_work(&[]).expect("claim the work");
    assert_eq!(
        work_status(&state, "finalize:transfer-restart").as_deref(),
        Some("running")
    );
    assert!(db
        .task_workflow_is_claimed_by_transfer("task-1")
        .expect("read ownership")
        .is_some());
    db.requeue_interrupted_transfer_work().expect("requeue");
    assert_eq!(
        work_status(&state, "finalize:transfer-restart").as_deref(),
        Some("pending")
    );

    let after = single_reviewer_suffix(&before);
    let (status, _) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "recovered work still owns the source"
    );

    // Bounded exhaustion is the end of the work, and the end of ownership.
    for attempt in 1..=8 {
        if !db
            .fail_transfer_work_attempt("finalize:transfer-restart", attempt, "still failing")
            .expect("record the failed attempt")
        {
            break;
        }
    }
    assert_eq!(
        work_status(&state, "finalize:transfer-restart").as_deref(),
        Some("failed"),
        "the attempt budget must be spendable"
    );
    assert_eq!(
        db.task_workflow_is_claimed_by_transfer("task-1")
            .expect("read ownership"),
        None
    );
    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workflowExtended"], serde_json::json!(true));
}

/// A committed receipt for an earlier payload must not close the source of a
/// plan published since.
///
/// Receipts are durable on the sidecar and replayed on its own schedule, so one
/// can arrive after this server has settled the work and after a plan has been
/// published. It proves the payload it was issued for — not that plan. Closing
/// the task would destroy it.
#[tokio::test]
async fn a_late_receipt_cannot_close_the_source_of_a_published_plan() {
    let BarrieredFixture {
        _temp,
        state,
        before,
        ..
    } = plan_publication_fixture_with_barrier("plan-vs-late-receipt");
    let app = router(Arc::clone(&state));
    {
        let db = Db::open(&state.config.db_path).expect("db");
        seed_outgoing_transfer(&db, "transfer-receipt");
        // The transfer's own work has genuinely finished, which is what lets
        // the plan publish at all.
        db.complete_transfer_work("finalize:transfer-receipt")
            .expect("finish the work");
    }
    let after = single_reviewer_suffix(&before);
    let (status, body) = complete_plan(&app, "the full plan", Some((&before, &after))).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The receipt arrives through the real queue mapping and the real
    // outgoing-committed path, carrying a proof that is genuinely valid for the
    // payload that shipped.
    {
        let db = Db::open(&state.config.db_path).expect("db");
        db.enqueue_transfer_work(
            "committed:transfer-receipt",
            "outgoing-committed",
            Some("transfer-receipt"),
            &committed_receipt("transfer-receipt").to_string(),
        )
        .expect("queue the receipt");
    }
    let refusal = crate::transfer_engine::run_one_work_item_for_test(
        &state,
        &committed_work("transfer-receipt"),
    )
    .await
    .expect_err("a late receipt was allowed to close a published plan's source");
    assert!(refusal.contains("carries a published plan"), "{refusal}");

    // The source is still open with its plan intact.
    let db = Db::open(&state.config.db_path).expect("db");
    let item = db
        .get_pipeline_item("task-1")
        .expect("read task")
        .expect("task");
    assert!(item.closed_at.is_none(), "the source must stay open");
    assert!(item
        .pipeline_def
        .as_deref()
        .expect("pinned")
        .contains("plan_context"));
}

/// The control for the test above: an ordinary receipt, for a task with no
/// published plan, still reaches the close.
///
/// Without this, refusing every receipt would look like a fix.
#[tokio::test]
async fn an_ordinary_receipt_still_reaches_the_source_close() {
    let BarrieredFixture { _temp, state, .. } =
        plan_publication_fixture_with_barrier("plan-vs-ordinary-receipt");
    {
        let db = Db::open(&state.config.db_path).expect("db");
        seed_outgoing_transfer(&db, "transfer-ordinary");
        db.complete_transfer_work("finalize:transfer-ordinary")
            .expect("finish the work");
        db.enqueue_transfer_work(
            "committed:transfer-ordinary",
            "outgoing-committed",
            Some("transfer-ordinary"),
            &committed_receipt("transfer-ordinary").to_string(),
        )
        .expect("queue the receipt");
    }

    let outcome = crate::transfer_engine::run_one_work_item_for_test(
        &state,
        &committed_work("transfer-ordinary"),
    )
    .await;

    // This fixture has no daemon to close against, so the close itself cannot
    // succeed — but it must be the close that fails, not the ownership check.
    // An unpublished task is never refused for preservation.
    if let Err(reason) = &outcome {
        assert!(
            !reason.contains("carries a published plan")
                && !reason.contains("cannot take ownership"),
            "an ordinary receipt was refused before it reached the close: {reason}"
        );
    }
}

#[tokio::test]
async fn equivalent_structured_workflow_edit_keeps_the_recorded_run_resumable() {
    let (_temp, state, before) = replacement_fixture("equivalent-harness-selection");
    let app = router(Arc::clone(&state));
    let mut after = before.clone();
    after["stages"][0]["agent_provider"] = serde_json::json!([
        {"harness":"claude", "model":"fable"}, {"harness":"codex", "model":"gpt-6-astra"}
    ]);
    let (status, body) = replace_workflow(&app, &before, &after).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["supersededRunIds"], serde_json::json!([]));
    assert!(!Db::open(&state.config.db_path)
        .unwrap()
        .stage_run_workflow_superseded("task-1", "run-old")
        .unwrap());
}

#[tokio::test]
async fn structured_selection_transfer_requires_v2_acceptance_before_finalization() {
    let (_temp, state, before) = replacement_fixture("structured-transfer-refusal");
    let app = router(Arc::clone(&state));
    let mut after = before.clone();
    after["stages"][1]["agent_provider"] =
        serde_json::json!({"harness":"opencode", "model":"local/model-high"});
    let (status, body) = replace_workflow(&app, &before, &after).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    seed_outgoing_transfer(
        &Db::open(&state.config.db_path).unwrap(),
        "transfer-structured",
    );
    let error = crate::transfer_engine::push::run_finalization_for_test(
        &state,
        &crate::db::TransferWorkItem {
            payload_json: finalize_payload("transfer-structured"),
            ..finalize_work(&state, "transfer-structured")
        },
        "transfer-structured",
    )
    .await
    .unwrap_err();
    assert!(
        error.contains("requires V2 destination acceptance"),
        "{error}"
    );
    assert!(!finalization_ran(&state, "transfer-structured"));
}

#[tokio::test]
async fn v2_acceptance_allows_structured_source_to_reach_finalization() {
    let BarrieredFixture {
        _temp,
        state,
        mut before,
        mut arrivals,
        ..
    } = plan_publication_fixture_with_barrier("structured-v2-accepted");
    before["stages"][1]["agent_provider"] = serde_json::json!({
        "harness": "opencode", "model": "local/Model-high", "effort": "custom-hi"
    });
    let db = Db::open(&state.config.db_path).unwrap();
    db.update_test_pipeline_item_pipeline_def("task-1", &before.to_string())
        .unwrap();
    seed_outgoing_transfer(&db, "transfer-v2");
    let work = finalize_work(&state, "transfer-v2");
    let attempt = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            crate::transfer_engine::push::run_finalization_for_test(&state, &work, "transfer-v2")
                .await
        })
    };
    assert_eq!(
        next_arrival(&mut arrivals).await,
        "transfer-v2/finalize:transfer-v2"
    );
    // Crossing the source-effects barrier proves acceptance. This fixture has
    // no live agent; stop before source effects, which other suites exercise.
    attempt.abort();
    assert!(attempt.await.unwrap_err().is_cancelled());
    assert!(!finalization_ran(&state, "transfer-v2"));
}

#[tokio::test]
async fn v2_acceptance_refuses_a_changed_selection_before_source_effects() {
    let (_temp, state, mut before) = replacement_fixture("structured-v2-stale");
    let db = Db::open(&state.config.db_path).unwrap();
    seed_outgoing_transfer(&db, "transfer-stale");
    let work = finalize_work(&state, "transfer-stale");
    before["stages"][1]["agent_provider"] = serde_json::json!({
        "harness": "opencode", "model": "local/Changed-high"
    });
    db.update_test_pipeline_item_pipeline_def("task-1", &before.to_string())
        .unwrap();
    let error =
        crate::transfer_engine::push::run_finalization_for_test(&state, &work, "transfer-stale")
            .await
            .unwrap_err();
    assert!(
        error.contains("changed after destination acceptance"),
        "{error}"
    );
    assert!(!finalization_ran(&state, "transfer-stale"));
}
