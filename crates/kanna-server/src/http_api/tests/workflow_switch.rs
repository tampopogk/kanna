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

/// The consultation-grown shape: a manual `plan` stage appended to the task
/// that carried the consultation, with the planning run live.
fn plan_publication_fixture(label: &str) -> (tempfile::TempDir, Arc<AppState>, Value) {
    let (temp, repo_path) = workflow_test_repo(label);
    let before = serde_json::json!({"name": "consultation", "stages": [
        {"name": "consultation", "agent": "consultant", "prompt": "$TASK_PROMPT",
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
            "consultation",
            "plan",
            &saved.to_string(),
        );
        db.insert_stage_run(NewStageRun {
            id: "run-consultation",
            task_id: "task-1",
            stage: "consultation",
            kind: "main",
            agent: Some("consultant"),
            agent_provider: Some("claude"),
            model: None,
            effort: None,
            status: "succeeded",
            result: Some(r#"{"status":"success","summary":"brief"}"#),
            feedback: None,
            session_id: Some("session-consultation"),
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
        ["consultation", "plan", "in progress", "review", "pr"]
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
        // Move the task back to the consultation stage: a consultant must not
        // be able to publish delivery stages for itself.
        let db = Db::open(&state.config.db_path).unwrap();
        db.update_pipeline_item_stage("task-1", "consultation")
            .unwrap();
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
