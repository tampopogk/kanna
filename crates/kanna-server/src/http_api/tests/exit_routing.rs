//! Named-exit result routing through the result endpoint (spec §5, T1).
use super::actions::{
    commit_branch_change, ledger_files, ledger_fixture_config, post_json, spawn_count,
    spawn_recording_daemon, wait_for_running_task_stage,
};
use super::*;
use crate::db::task_store::LedgerEntryKind;

const TASK: &str = "exits-1";

fn exit_workflow(review_transition: &str, implement_budget: i64) -> serde_json::Value {
    serde_json::json!({
        "name": "exits-flow",
        "routing": "exits",
        "stages": [
            { "name": "plan", "agent": "plan", "prompt": "$TASK_PROMPT",
              "policy": { "transition": "manual" } },
            { "name": "in progress", "agent": "implement", "prompt": "$TASK_PROMPT",
              "budget": implement_budget,
              "policy": { "transition": "manual", "loop_transition": "auto" } },
            { "name": "review", "agent": "review", "prompt": "Review the branch.",
              "exits": { "revise": "in progress", "replan": "plan" },
              "policy": { "transition": review_transition } },
            { "name": "pr", "agent": "pr", "prompt": "Open the PR.",
              "policy": { "transition": "manual" } }
        ]
    })
}

struct ExitFixture {
    state: Arc<AppState>,
    app: axum::Router,
    db_path: String,
    commands: Arc<std::sync::Mutex<Vec<kanna_daemon::protocol::Command>>>,
    worktree: PathBuf,
    repo_root: PathBuf,
    daemon_dir: PathBuf,
}

impl ExitFixture {
    fn db(&self) -> Db {
        Db::open(&self.db_path).unwrap()
    }

    async fn complete(&self, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let (status, text) = post_json(
            &self.app,
            &format!("/v1/tasks/{TASK}/actions/complete-stage"),
            body,
        )
        .await;
        let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
        (status, value)
    }

    async fn settle(&self) {
        crate::http_api::wait_for_task_mutation_to_finish(&self.state, TASK).await;
    }

    fn flushed_ledger(&self) -> Vec<crate::task_store::LedgerFile> {
        crate::task_store::flush_task(&self.db(), &self.db_path, TASK).unwrap();
        ledger_files(&self.db_path, TASK)
    }

    fn entries(&self, kind: LedgerEntryKind) -> Vec<crate::task_store::LedgerFile> {
        self.flushed_ledger()
            .into_iter()
            .filter(|file| file.kind == kind)
            .collect()
    }
}

impl Drop for ExitFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.daemon_dir);
        let _ = std::fs::remove_dir_all(&self.repo_root);
    }
}

/// A named-exit task parked in `review` with a running review run in a real
/// worktree, its workflow pinned, and a daemon that accepts every spawn.
fn exit_fixture(label: &str, workflow: serde_json::Value) -> ExitFixture {
    let repo_root = crate::test_paths::unique_test_path(&format!("kanna-exits-{label}"));
    init_test_git_repo(&repo_root);
    std::fs::write(
        repo_root.join(".kanna/workflows/exits-flow.json"),
        workflow.to_string(),
    )
    .unwrap();
    for args in [
        vec!["add", ".kanna/workflows/exits-flow.json"],
        vec!["commit", "-qm", "add exits workflow"],
    ] {
        assert!(Command::new("git")
            .args(&args)
            .current_dir(&repo_root)
            .status()
            .unwrap()
            .success());
    }
    publish_test_origin_main(&repo_root);
    let branch = format!("task-exits-{label}");
    let worktree = commit_branch_change(&repo_root, &branch, "work.txt", "reviewed work");
    let daemon_dir = crate::test_paths::unique_test_path(&format!("kanna-exits-{label}-d"));
    std::fs::create_dir_all(&daemon_dir).unwrap();
    let commands = spawn_recording_daemon(&daemon_dir);
    let config = ledger_fixture_config(&format!("exits-{label}"), &daemon_dir);
    let db = Db::open_for_tests(&config.db_path).unwrap();
    db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
        .unwrap();
    db.insert_test_pipeline_item(
        TASK,
        "repo-1",
        "Build the thing",
        Some("Build the thing"),
        "review",
        "2026-09-23 10:00:00",
    )
    .unwrap();
    db.update_test_pipeline_item_stage_context(TASK, &branch, "exits-flow", None, "claude")
        .unwrap();
    db.update_test_pipeline_item_pipeline_def(TASK, &workflow.to_string())
        .unwrap();
    db.upsert_worktree("wt-exits-1", TASK, &worktree.to_string_lossy(), &branch)
        .unwrap();
    db.insert_stage_run(crate::db::NewStageRun {
        id: "review-run",
        task_id: TASK,
        stage: "review",
        kind: "main",
        agent: Some("review"),
        agent_provider: Some("claude"),
        model: None,
        effort: None,
        status: "running",
        result: None,
        feedback: None,
        session_id: Some(TASK),
        provider_session_id: None,
        cwd: Some(&worktree.to_string_lossy()),
        resumed_from_run_id: None,
    })
    .unwrap();
    drop(db);
    let state = Arc::new(super::AppState::new(config.clone()));
    let app = super::router(Arc::clone(&state));
    ExitFixture {
        state,
        app,
        db_path: config.db_path,
        commands,
        worktree,
        repo_root,
        daemon_dir,
    }
}

#[tokio::test]
async fn success_with_revise_loops_to_its_destination_and_records_the_exit() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = exit_fixture("revise", exit_workflow("auto", 2));
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": "review-run",
            "status": "success",
            "summary": "Two defects\n\nfix the parser and the retry",
            "exit": "revise",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["routing"]["exit"], "revise");
    assert_eq!(body["routing"]["exitSource"], "explicit");
    assert_eq!(body["routing"]["destination"], "in progress");
    assert_eq!(body["routing"]["outcome"], "loop");
    assert_eq!(body["revisionBudget"]["rounds"], 1);
    assert_eq!(body["revisionBudget"]["limit"], 2);

    let db = fixture.db();
    wait_for_running_task_stage(&db, TASK, "in progress").await;
    fixture.settle().await;
    assert_eq!(spawn_count(&fixture.commands), 1);
    // The review did its job: its status is success, not the legacy failure.
    assert_eq!(
        db.stage_run("review-run").unwrap().unwrap().status,
        "succeeded"
    );
    assert_eq!(db.stage_budget_spent(TASK, "in progress").unwrap(), 1);
    assert_eq!(db.stage_budget_spent(TASK, "plan").unwrap(), 0);
    // The re-entered stage leaves by its loop_transition, not its transition.
    let reviser = db
        .list_stage_runs_for_task(TASK)
        .unwrap()
        .into_iter()
        .find(|run| run.stage == "in progress")
        .unwrap();
    assert_eq!(reviser.completion_transition.as_deref(), Some("auto"));

    let results = fixture.entries(LedgerEntryKind::Result);
    assert_eq!(results.len(), 1);
    let result = results[0].body();
    assert_eq!(result["status"], "success");
    assert_eq!(result["exit"], "revise");
    assert_eq!(result["exit_source"], "explicit");
    assert_eq!(result["exit_destination"], "in progress");
    assert_eq!(result["exit_outcome"], "loop");
    assert_eq!(result["budget"]["spent"], 1);
    assert_eq!(result["budget"]["limit"], 2);
    let transitions = fixture.entries(LedgerEntryKind::Transition);
    let transition = transitions.last().unwrap().body();
    assert_eq!(transition["from_stage"], "review");
    assert_eq!(transition["to_stage"], "in progress");
    assert_eq!(transition["exit"], "revise");
    assert_eq!(transition["exit_source"], "explicit");
    assert_eq!(transition["budget"]["stage"], "in progress");
    assert_eq!(transition["budget"]["spent"], 1);
    assert_eq!(
        transition["triggering_result_id"],
        results[0].entry_id().unwrap()
    );
    // The loop is the engine applying the workflow: its transition records the
    // server's channel, while the result keeps the caller's.
    let transition_channel = &transitions.last().unwrap().envelope["channel_identity"];
    assert_eq!(transition_channel["kind"], "server", "{transition_channel}");
    assert_eq!(results[0].envelope["declared_role"], "agent");
    assert_ne!(results[0].envelope["channel_identity"]["kind"], "server");
}

#[tokio::test]
async fn success_with_replan_reaches_plan_and_spends_only_plans_budget() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = exit_fixture("replan", exit_workflow("auto", 2));
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": "review-run",
            "status": "success",
            "summary": "The approach cannot work: the cache is per-process",
            "exit": "replan",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["routing"]["destination"], "plan");
    let db = fixture.db();
    wait_for_running_task_stage(&db, TASK, "plan").await;
    fixture.settle().await;
    assert_eq!(db.stage_budget_spent(TASK, "plan").unwrap(), 1);
    assert_eq!(db.stage_budget_spent(TASK, "in progress").unwrap(), 0);
    let transition = fixture.entries(LedgerEntryKind::Transition).pop().unwrap();
    assert_eq!(transition.body()["to_stage"], "plan");
    assert_eq!(transition.body()["exit"], "replan");
}

#[tokio::test]
async fn a_spent_destination_budget_records_and_parks_while_other_destinations_still_loop() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = exit_fixture("exhausted", exit_workflow("auto", 1));
    // One loop into `in progress` was already taken this task.
    {
        let db = fixture.db();
        db.with_immediate_transaction(|db| {
            db.claim_stage_budget_in_transaction(TASK, "in progress", 1)
        })
        .unwrap();
    }
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": "review-run",
            "status": "success",
            "summary": "Still one defect",
            "exit": "revise",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["routing"]["outcome"], "parked");
    assert_eq!(body["revisionBudget"]["exhausted"], true);
    fixture.settle().await;
    let db = fixture.db();
    assert_eq!(spawn_count(&fixture.commands), 0);
    assert!(!db.has_ledger_continuation(TASK).unwrap());
    let item = db.get_pipeline_item(TASK).unwrap().unwrap();
    assert_eq!(item.stage.as_deref(), Some("review"));
    assert_eq!(db.stage_budget_spent(TASK, "in progress").unwrap(), 1);
    let result = fixture.entries(LedgerEntryKind::Result).pop().unwrap();
    assert_eq!(result.body()["status"], "success");
    assert_eq!(result.body()["exit_outcome"], "parked");
    assert_eq!(result.body()["budget"]["exhausted"], true);

    // `plan` has its own budget: the same reviewer may still send it there.
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": "review-run",
            "status": "success",
            "summary": "Still one defect, and it is structural",
            "exit": "replan",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["routing"]["outcome"], "loop");
    wait_for_running_task_stage(&db, TASK, "plan").await;
    fixture.settle().await;
    assert_eq!(db.stage_budget_spent(TASK, "plan").unwrap(), 1);
}

#[tokio::test]
async fn a_person_sending_the_task_back_resets_only_that_destinations_budget() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = exit_fixture("reset", exit_workflow("auto", 1));
    {
        let db = fixture.db();
        db.with_immediate_transaction(|db| {
            db.claim_stage_budget_in_transaction(TASK, "in progress", 1)?;
            db.claim_stage_budget_in_transaction(TASK, "plan", 5)
        })
        .unwrap();
    }
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": "review-run",
            "status": "success",
            "summary": "Still one defect",
            "exit": "revise",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["routing"]["outcome"], "parked");
    fixture.settle().await;

    let (status, text) = post_json(
        &fixture.app,
        &format!("/v1/tasks/{TASK}/actions/request-revision"),
        serde_json::json!({
            "targetStage": "in progress",
            "summary": "Go round once more",
            "prompt": "Fix the last defect",
            "origin": "human",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let db = fixture.db();
    wait_for_running_task_stage(&db, TASK, "in progress").await;
    fixture.settle().await;
    assert_eq!(db.stage_budget_spent(TASK, "in progress").unwrap(), 0);
    assert_eq!(db.stage_budget_spent(TASK, "plan").unwrap(), 1);
    let transition = fixture.entries(LedgerEntryKind::Transition).pop().unwrap();
    assert_eq!(transition.body()["to_stage"], "in progress");
    assert_eq!(transition.body()["exit"], "revise");
    assert_eq!(transition.body()["exit_source"], "operator");
}

#[tokio::test]
async fn a_non_success_status_parks_whatever_exit_it_names() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = exit_fixture("non-success", exit_workflow("auto", 2));
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": "review-run",
            "status": "partial",
            "summary": "Reviewed half; the rest needs the fixture repo",
            "exit": "revise",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["routing"]["outcome"], "parked");
    assert!(body.get("revisionBudget").is_none(), "{body}");
    fixture.settle().await;
    let db = fixture.db();
    assert_eq!(spawn_count(&fixture.commands), 0);
    assert_eq!(db.stage_budget_spent(TASK, "in progress").unwrap(), 0);
    assert!(!db.has_ledger_continuation(TASK).unwrap());
    let result = fixture.entries(LedgerEntryKind::Result).pop().unwrap();
    assert_eq!(result.body()["exit"], "revise");
    assert_eq!(result.body()["exit_outcome"], "parked");
    assert!(result.body()["budget"].is_null());

    // A later success naming nothing takes `advance` by default; the stage
    // is auto, so the task moves and the transition says the default applied.
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": "review-run",
            "status": "success",
            "summary": "Reviewed all of it; passes",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["routing"]["exit"], "advance");
    assert_eq!(body["routing"]["exitSource"], "default");
    wait_for_running_task_stage(&db, TASK, "pr").await;
    fixture.settle().await;
    let transition = fixture.entries(LedgerEntryKind::Transition).pop().unwrap();
    assert_eq!(transition.body()["to_stage"], "pr");
    assert_eq!(transition.body()["exit"], "advance");
    assert_eq!(transition.body()["exit_source"], "default");
}

#[tokio::test]
async fn advance_at_a_manual_gate_parks_and_a_repeated_result_after_more_work_is_new() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = exit_fixture("manual", exit_workflow("manual", 2));
    let result = serde_json::json!({
        "runId": "review-run",
        "status": "success",
        "summary": "Passes review",
        "exit": "advance",
    });
    let (status, body) = fixture.complete(result.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["routing"]["outcome"], "advance");
    assert_eq!(body["routing"]["exitSource"], "explicit");
    fixture.settle().await;
    let db = fixture.db();
    // Naming `advance` never operates a manual gate.
    assert_eq!(spawn_count(&fixture.commands), 0);
    assert_eq!(
        db.get_pipeline_item(TASK)
            .unwrap()
            .unwrap()
            .stage
            .as_deref(),
        Some("review")
    );

    // The identical request again is a retry: nothing new is recorded.
    let (status, body) = fixture.complete(result.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(fixture.entries(LedgerEntryKind::Result).len(), 1);

    // The person keeps working with the parked session; its commit moves.
    std::fs::write(fixture.worktree.join("more.txt"), "more").unwrap();
    for args in [vec!["add", "more.txt"], vec!["commit", "-qm", "more work"]] {
        assert!(Command::new("git")
            .args(&args)
            .current_dir(&fixture.worktree)
            .status()
            .unwrap()
            .success());
    }
    // The same words recorded now are a new result, not a retry.
    let (status, body) = fixture.complete(result).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.settle().await;
    let results = fixture.entries(LedgerEntryKind::Result);
    assert_eq!(results.len(), 2);
    assert_ne!(
        results[0].body()["committed_sha"],
        results[1].body()["committed_sha"]
    );
    assert_eq!(spawn_count(&fixture.commands), 0);

    // A person operates the gate; the transition records that.
    let (status, text) = post_json(
        &fixture.app,
        &format!("/v1/tasks/{TASK}/actions/advance-stage"),
        serde_json::json!({ "source": "operator" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    wait_for_running_task_stage(&db, TASK, "pr").await;
    fixture.settle().await;
    let transition = fixture.entries(LedgerEntryKind::Transition).pop().unwrap();
    assert_eq!(transition.body()["exit"], "advance");
    assert_eq!(transition.body()["exit_source"], "operator");
    assert_eq!(
        transition.body()["triggering_result_id"],
        results[1].entry_id().unwrap()
    );
}

#[tokio::test]
async fn an_undeclared_exit_and_a_stage_named_revision_are_refused_before_recording() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = exit_fixture("refusals", exit_workflow("auto", 2));
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": "review-run",
            "status": "success",
            "summary": "Ship it",
            "exit": "ship",
        }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let text = body.to_string();
    assert!(text.contains("declares no exit 'ship'"), "{text}");
    assert!(text.contains("'revise' (to 'in progress')"), "{text}");

    let (status, text) = post_json(
        &fixture.app,
        &format!("/v1/tasks/{TASK}/actions/request-revision"),
        serde_json::json!({
            "runId": "review-run",
            "targetStage": "in progress",
            "summary": "Needs work",
            "prompt": "Fix it",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{text}");
    assert!(text.contains("routes results by named exits"), "{text}");
    fixture.settle().await;
    let db = fixture.db();
    let run = db.stage_run("review-run").unwrap().unwrap();
    assert_eq!(run.status, "running");
    assert!(run.result.is_none());
    assert!(fixture.flushed_ledger().is_empty());
    assert_eq!(db.stage_budget_spent(TASK, "in progress").unwrap(), 0);
}

#[tokio::test]
async fn a_legacy_pinned_task_refuses_an_exit_and_routes_as_before() {
    let legacy = serde_json::json!({
        "name": "legacy",
        "revision_limit": 5,
        "stages": [
            { "name": "in progress", "agent": "implement", "policy": { "transition": "manual" } },
            { "name": "review", "agent": "review", "policy": { "transition": "auto" } }
        ]
    });
    let state = super::test_state_with_seed("exits-legacy", "Studio Mac", move |db| {
        db.insert_test_repo("repo-1", "Repo One").unwrap();
        db.insert_test_pipeline_item(
            "task-1",
            "repo-1",
            "Implement it",
            Some("Implement it"),
            "in progress",
            "2026-09-22 00:00:00",
        )
        .unwrap();
        db.update_test_pipeline_item_pipeline_def("task-1", &legacy.to_string())
            .unwrap();
        db.insert_stage_run(crate::db::NewStageRun {
            id: "run-1",
            task_id: "task-1",
            stage: "in progress",
            kind: "main",
            agent: Some("implement"),
            agent_provider: Some("claude"),
            model: None,
            effort: None,
            status: "running",
            result: None,
            feedback: None,
            session_id: Some("task-1"),
            provider_session_id: None,
            cwd: None,
            resumed_from_run_id: None,
        })
        .unwrap();
    });
    let db_path = state.config.db_path.clone();
    let app = super::router(state);
    let (status, text) = post_json(
        &app,
        "/v1/tasks/task-1/actions/complete-stage",
        serde_json::json!({ "runId": "run-1", "status": "success", "summary": "done", "exit": "advance" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("does not route by named exits"), "{text}");
    let db = Db::open(&db_path).unwrap();
    assert_eq!(db.stage_run("run-1").unwrap().unwrap().status, "running");

    // Without an exit the legacy adapter records exactly what it always did:
    // no routing in the response, no exit fields on the result.
    let (status, text) = post_json(
        &app,
        "/v1/tasks/task-1/actions/complete-stage",
        serde_json::json!({ "runId": "run-1", "status": "partial", "summary": "half" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let body: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(body.get("routing").is_none(), "{body}");
    let result = ledger_files(&db_path, "task-1").pop().unwrap();
    assert!(result.body().get("exit").is_none());
    let recorded: serde_json::Value = serde_json::from_str(
        db.stage_run("run-1")
            .unwrap()
            .unwrap()
            .result
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        recorded,
        serde_json::json!({ "status": "partial", "summary": "half", "metadata": null })
    );
}

#[tokio::test]
async fn a_result_replaces_the_remaining_plan_fenced_on_the_definition_it_read() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let workflow = exit_workflow("manual", 2);
    let fixture = exit_fixture("replace", workflow.clone());
    let mut replacement = workflow.clone();
    replacement["stages"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "name": "ship", "agent": "pr", "prompt": "Ship it.",
            "policy": { "transition": "manual" }
        }));

    // A stale fence records nothing.
    let mut stale = workflow.clone();
    stale["description"] = serde_json::json!("an older read");
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": "review-run",
            "status": "success",
            "summary": "Passes; add a ship stage",
            "workflowDefinition": replacement,
            "expectedDefinition": stale,
        }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        fixture
            .db()
            .stage_run("review-run")
            .unwrap()
            .unwrap()
            .status,
        "running"
    );

    // The current stage must keep its role.
    let mut recast = replacement.clone();
    recast["stages"][2]["agent"] = serde_json::json!("implement");
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": "review-run",
            "status": "success",
            "summary": "Passes; add a ship stage",
            "workflowDefinition": recast,
            "expectedDefinition": workflow,
        }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("must keep its role"), "{body}");

    let request = serde_json::json!({
        "runId": "review-run",
        "status": "success",
        "summary": "Passes; add a ship stage",
        "workflowDefinition": replacement,
        "expectedDefinition": workflow,
    });
    let (status, body) = fixture.complete(request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workflowExtended"], true);
    // An exact retry is recognized, and still confirms the publication.
    let (status, body) = fixture.complete(request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workflowExtended"], true);
    fixture.settle().await;

    let db = fixture.db();
    let pinned: serde_json::Value = serde_json::from_str(
        &db.get_pipeline_item(TASK)
            .unwrap()
            .unwrap()
            .pipeline_def
            .unwrap(),
    )
    .unwrap();
    assert_eq!(pinned["stages"].as_array().unwrap().len(), 5);
    assert!(pinned.get("plan_context").is_none());
    let results = fixture.entries(LedgerEntryKind::Result);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].body()["request"]["publishesWorkflow"], true);
    let plans = fixture.entries(LedgerEntryKind::Plan);
    let plan = plans.last().unwrap();
    assert_eq!(plan.body()["operation"], "replace");
    assert_eq!(plan.body()["result_id"], results[0].entry_id().unwrap());
    assert_eq!(
        plan.envelope["operation_id"],
        results[0].envelope["operation_id"]
    );
}
