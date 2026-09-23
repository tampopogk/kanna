//! Fixture runs for the T10d intake lineup (spec §10) against the bundled
//! `shaped`, `planned` and `designed` workflow definitions themselves (not a
//! synthetic stand-in), through the same complete-stage/advance-stage/
//! request-revision endpoints exit_routing.rs and roleless_stage.rs exercise.
//! `research` is unchanged by this card and already covered elsewhere.
use super::actions::{
    commit_branch_change, ledger_files, ledger_fixture_config, post_json, spawn_recording_daemon,
    wait_for_running_task_stage,
};
use super::*;
use crate::db::task_store::LedgerEntryKind;

const SHAPED_JSON: &str = include_str!("../../../../../.kanna/workflows/shaped.json");
const PLANNED_JSON: &str = include_str!("../../../../../.kanna/workflows/planned.json");
const DESIGNED_JSON: &str = include_str!("../../../../../.kanna/workflows/designed.json");

struct LineupFixture {
    state: Arc<AppState>,
    app: axum::Router,
    db_path: String,
    commands: Arc<std::sync::Mutex<Vec<kanna_daemon::protocol::Command>>>,
    repo_root: PathBuf,
    daemon_dir: PathBuf,
    task_id: &'static str,
}

impl LineupFixture {
    fn db(&self) -> Db {
        Db::open(&self.db_path).unwrap()
    }

    async fn post(&self, action: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let (status, text) = post_json(
            &self.app,
            &format!("/v1/tasks/{}/actions/{action}", self.task_id),
            body,
        )
        .await;
        let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
        (status, value)
    }

    async fn settle(&self) {
        crate::http_api::wait_for_task_mutation_to_finish(&self.state, self.task_id).await;
    }

    async fn publish_artifact(&self, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let (status, text) = post_json(
            &self.app,
            &format!("/v1/tasks/{}/artifacts", self.task_id),
            body,
        )
        .await;
        let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
        (status, value)
    }

    async fn wait_for_stage(&self, stage: &str) {
        let db = self.db();
        wait_for_running_task_stage(&db, self.task_id, stage).await;
    }

    fn entries(&self, kind: LedgerEntryKind) -> Vec<crate::task_store::LedgerFile> {
        let db = self.db();
        crate::task_store::flush_task(&db, &self.db_path, self.task_id).unwrap();
        ledger_files(&self.db_path, self.task_id)
            .into_iter()
            .filter(|file| file.kind == kind)
            .collect()
    }

    fn running_run(&self, stage: &str, kind: &str) -> crate::db::StageRun {
        self.db()
            .list_stage_runs_for_task(self.task_id)
            .unwrap()
            .into_iter()
            .find(|run| run.stage == stage && run.kind == kind && run.status == "running")
            .unwrap_or_else(|| panic!("no running {kind} run at stage {stage}"))
    }

    fn agent_spawns(&self) -> usize {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .filter(|command| match command {
                kanna_daemon::protocol::Command::Spawn { session_id, .. }
                | kanna_daemon::protocol::Command::SpawnAgent { session_id, .. } => {
                    session_id == self.task_id
                }
                _ => false,
            })
            .count()
    }
}

impl Drop for LineupFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.daemon_dir);
        let _ = std::fs::remove_dir_all(&self.repo_root);
    }
}

/// A task pinned to one of the bundled T10d workflows (read verbatim from the
/// repo, not a repo-local override), parked in `initial_stage` with a running
/// main run there, in a real worktree.
fn lineup_fixture(
    label: &str,
    task_id: &'static str,
    workflow_json: &str,
    initial_stage: &str,
    initial_agent: &str,
    initial_run_id: &str,
) -> LineupFixture {
    let workflow: serde_json::Value = serde_json::from_str(workflow_json).unwrap();
    let repo_root = crate::test_paths::unique_test_path(&format!("kanna-lineup-{label}"));
    init_test_git_repo(&repo_root);
    publish_test_origin_main(&repo_root);
    let branch = format!("task-lineup-{label}");
    let worktree = commit_branch_change(&repo_root, &branch, "mockup.html", "<p>mockup</p>");
    let daemon_dir = crate::test_paths::unique_test_path(&format!("kanna-lineup-{label}-d"));
    std::fs::create_dir_all(&daemon_dir).unwrap();
    let commands = spawn_recording_daemon(&daemon_dir);
    let config = ledger_fixture_config(&format!("lineup-{label}"), &daemon_dir);
    let db = Db::open_for_tests(&config.db_path).unwrap();
    db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
        .unwrap();
    db.insert_test_pipeline_item(
        task_id,
        "repo-1",
        "Build the thing",
        Some("Build the thing"),
        initial_stage,
        "2026-09-23 10:00:00",
    )
    .unwrap();
    db.update_test_pipeline_item_stage_context(
        task_id,
        &branch,
        workflow["name"].as_str().unwrap(),
        None,
        "claude",
    )
    .unwrap();
    db.update_test_pipeline_item_pipeline_def(task_id, &workflow.to_string())
        .unwrap();
    db.upsert_worktree(
        &format!("wt-{label}"),
        task_id,
        &worktree.to_string_lossy(),
        &branch,
    )
    .unwrap();
    db.insert_stage_run(crate::db::NewStageRun {
        id: initial_run_id,
        task_id,
        stage: initial_stage,
        kind: "main",
        agent: Some(initial_agent),
        agent_provider: Some("claude"),
        model: None,
        effort: None,
        status: "running",
        result: None,
        feedback: None,
        session_id: Some(task_id),
        provider_session_id: None,
        cwd: Some(&worktree.to_string_lossy()),
        resumed_from_run_id: None,
    })
    .unwrap();
    drop(db);
    let state = Arc::new(super::AppState::new(config.clone()));
    let app = super::router(Arc::clone(&state));
    LineupFixture {
        state,
        app,
        db_path: config.db_path,
        commands,
        repo_root,
        daemon_dir,
        task_id,
    }
}

/// `shaped`: the review stage's `revise` exit loops once to "in progress",
/// the loop-entered stage's `loop_transition: auto` (plus its `exit_commit`
/// commit step, whose live session this fixture's daemon reports as dead, so
/// it falls back to a fresh commit session in the same workspace) carries it
/// back to review automatically, and a plain success with no exit then
/// advances to pr once an operator confirms it.
#[tokio::test]
async fn shaped_review_revise_loops_once_and_a_clean_pass_advances_to_pr() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = lineup_fixture(
        "shaped",
        "shaped-1",
        SHAPED_JSON,
        "review",
        "review",
        "review-run",
    );

    let (status, body) = fixture
        .post(
            "complete-stage",
            serde_json::json!({
                "runId": "review-run",
                "status": "success",
                "summary": "Two defects\n\nfix the parser and the retry",
                "exit": "revise",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["routing"]["destination"], "in progress");
    assert_eq!(body["routing"]["outcome"], "loop");
    fixture.wait_for_stage("in progress").await;
    fixture.settle().await;
    assert_eq!(fixture.agent_spawns(), 1, "implement respawned once");

    let implement_run = fixture.running_run("in progress", "main");
    assert_eq!(
        implement_run.completion_transition.as_deref(),
        Some("auto"),
        "the loop-entered run leaves by loop_transition, not first-entry transition"
    );

    // The implementer's own result: exit_commit means this fires the commit
    // step, not the transition, before "in progress" is actually left.
    let (status, body) = fixture
        .post(
            "complete-stage",
            serde_json::json!({
                "runId": implement_run.id,
                "status": "success",
                "summary": "Fixed both defects",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.settle().await;
    let commit_run = fixture
        .db()
        .list_stage_runs_for_task(fixture.task_id)
        .unwrap()
        .into_iter()
        .find(|run| run.kind == "post" && run.stage == "in progress commit")
        .expect("the commit step's own run");
    assert_eq!(commit_run.status, "running");

    // The commit step's own result fires the transition; loop_transition
    // auto is what carries "in progress" back to review with no operator
    // action.
    let (status, body) = fixture
        .post(
            "complete-stage",
            serde_json::json!({
                "runId": commit_run.id,
                "status": "success",
                "summary": "Committed the fix",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.wait_for_stage("review").await;
    fixture.settle().await;
    let second_review_run = fixture.running_run("review", "main");
    assert_ne!(second_review_run.id, "review-run");

    // A clean pass: status success, no exit. Review's own transition is
    // manual, so the routing outcome is "advance" but the task parks until
    // an operator confirms it.
    let (status, body) = fixture
        .post(
            "complete-stage",
            serde_json::json!({
                "runId": second_review_run.id,
                "status": "success",
                "summary": "Looks good now",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["routing"]["outcome"], "advance");
    fixture.settle().await;
    assert_eq!(
        fixture
            .db()
            .get_pipeline_item(fixture.task_id)
            .unwrap()
            .unwrap()
            .stage
            .as_deref(),
        Some("review"),
        "advance never operates a manual gate by itself"
    );

    let (status, body) = fixture
        .post("advance-stage", serde_json::json!({ "source": "operator" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.wait_for_stage("pr").await;
    fixture.settle().await;
    assert_eq!(
        fixture.running_run("pr", "main").agent.as_deref(),
        Some("pr")
    );
}

/// `planned`: the review stage's `replan` exit returns to plan under its own
/// budget, and a plan result carrying a `workflow_definition` whose stages
/// after plan differ from the pinned `expected_definition` (per the plan
/// stage's own workflow-local instruction, publishing the remaining stages
/// its revised plan actually chose) is accepted as the T1 remaining-plan
/// replacement — the task's pinned definition becomes the submitted one —
/// after which the task continues to implement under the new definition.
#[tokio::test]
async fn planned_review_replan_returns_to_plan_and_republishes_the_remaining_plan() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = lineup_fixture(
        "planned",
        "planned-1",
        PLANNED_JSON,
        "review",
        "review",
        "review-run",
    );

    let (status, body) = fixture
        .post(
            "complete-stage",
            serde_json::json!({
                "runId": "review-run",
                "status": "success",
                "summary": "Structurally wrong approach\n\nneeds a different design",
                "exit": "replan",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["routing"]["destination"], "plan");
    assert_eq!(body["routing"]["outcome"], "loop");
    fixture.wait_for_stage("plan").await;
    fixture.settle().await;

    // The feedback that caused the replan is in the ledger context the new
    // plan session reads.
    let results = fixture.entries(LedgerEntryKind::Result);
    let replan_result = results
        .iter()
        .find(|entry| entry.body()["exit"] == "replan")
        .expect("the review's replan result");
    assert!(replan_result
        .message
        .as_deref()
        .unwrap_or_default()
        .contains("needs a different design"));

    let pinned: serde_json::Value = serde_json::from_str(
        &fixture
            .db()
            .get_pipeline_item(fixture.task_id)
            .unwrap()
            .unwrap()
            .pipeline_def
            .unwrap(),
    )
    .unwrap();
    let plan_run = fixture.running_run("plan", "main");

    // The plan session's own workflow-local instruction: on a replan visit,
    // record the revised plan and publish the remaining stages it actually
    // chose — here, a changed "in progress" prompt — through the T1
    // remaining-plan replacement contract. `workflow_definition` need not
    // match `expected_definition` (the pinned document read); only the
    // current stage's role must survive.
    const REVISED_IMPLEMENT_PROMPT: &str =
        "$TASK_PROMPT\n\nSplit the migration into two steps, per the revised plan.";
    let mut revised = pinned.clone();
    let stages = revised["stages"].as_array_mut().unwrap();
    let implement_stage = stages
        .iter_mut()
        .find(|stage| stage["name"] == "in progress")
        .unwrap();
    implement_stage["prompt"] = serde_json::json!(REVISED_IMPLEMENT_PROMPT);

    let (status, body) = fixture
        .post(
            "complete-stage",
            serde_json::json!({
                "runId": plan_run.id,
                "status": "success",
                "summary": "Revised plan: split the migration into two steps",
                "expectedDefinition": pinned,
                "workflowDefinition": revised,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workflowExtended"], true);
    fixture.settle().await;
    // plan's own transition is manual: a person confirms it before implement
    // starts, even after a replan.
    let item = fixture
        .db()
        .get_pipeline_item(fixture.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(item.stage.as_deref(), Some("plan"));
    // The task's pinned workflow is now the submitted one.
    let now_pinned: serde_json::Value = serde_json::from_str(&item.pipeline_def.unwrap()).unwrap();
    // The server's own serialization drops `$schema` (not part of the
    // resolved WorkflowDefinition); everything the caller submitted survives.
    let mut expected = revised.clone();
    expected.as_object_mut().unwrap().remove("$schema");
    assert_eq!(now_pinned, expected);

    let (status, body) = fixture
        .post("advance-stage", serde_json::json!({ "source": "operator" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.wait_for_stage("in progress").await;
    fixture.settle().await;
    let implement_run = fixture.running_run("in progress", "main");
    assert_eq!(implement_run.agent.as_deref(), Some("implement"));
    // The stage that runs next follows the new definition, not the pinned
    // one the plan session read.
    let resolved_prompt = fixture
        .db()
        .stage_run_prompt(fixture.task_id, &implement_run.id)
        .unwrap()
        .expect("the spawned run's resolved prompt is recorded")
        .resolved_prompt;
    assert!(
        resolved_prompt.contains("Split the migration into two steps"),
        "{resolved_prompt}"
    );
}

/// `designed`: mockup publishes an artifact and (once an operator confirms
/// it) advances into the roleless stakeholder gate, which spawns no agent
/// session; an `origin: "human"` revision targeting "mockup" is accepted
/// from the gate and re-enters mockup with the operator's feedback in the
/// new run's context; once mockup completes again, the gate's recorded
/// decision plus operator departure (advance-stage with a summary) continues
/// to plan.
#[tokio::test]
async fn designed_mockup_gate_revise_loop_and_operator_departure_to_plan() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = lineup_fixture(
        "designed",
        "designed-1",
        DESIGNED_JSON,
        "mockup",
        "mockup",
        "mockup-run",
    );

    let (status, published) = fixture
        .publish_artifact(serde_json::json!({ "path": "mockup.html", "kind": "mockup" }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    let artifact_id = published["artifactId"].as_str().unwrap().to_string();

    let (status, body) = fixture
        .post(
            "complete-stage",
            serde_json::json!({
                "runId": "mockup-run",
                "status": "success",
                "summary": "Mockup ready: the dashboard redesign",
                "artifacts": { "mockup": artifact_id },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.settle().await;
    // mockup's own transition is manual: an operator confirms it before the
    // gate is entered.
    let (status, body) = fixture
        .post("advance-stage", serde_json::json!({ "source": "operator" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.wait_for_stage("stakeholder").await;
    fixture.settle().await;
    assert_eq!(fixture.agent_spawns(), 0, "a gate spawns no agent session");

    let results = fixture.entries(LedgerEntryKind::Result);
    let mockup_result = results.first().unwrap();
    assert_eq!(
        mockup_result.envelope["artifacts"]["mockup"]["artifactId"],
        artifact_id
    );

    // Roleless stages cannot yet declare their own loop exits (spec §5, T3);
    // the stakeholder gate's own iteration back to mockup is the operator's
    // decision, taken through the same origin:"human" revision path any
    // manual stage's send-back uses under named-exit routing.
    let (status, body) = fixture
        .post(
            "request-revision",
            serde_json::json!({
                "targetStage": "mockup",
                "summary": "Needs another pass",
                "prompt": "Make the header darker",
                "origin": "human",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.wait_for_stage("mockup").await;
    fixture.settle().await;
    let second_mockup_run = fixture.running_run("mockup", "main");
    assert_ne!(second_mockup_run.id, "mockup-run");
    assert_eq!(
        second_mockup_run.feedback.as_deref(),
        Some("Make the header darker"),
        "the operator's feedback reaches the re-entered mockup session"
    );

    let (status, body) = fixture
        .post(
            "complete-stage",
            serde_json::json!({
                "runId": second_mockup_run.id,
                "status": "success",
                "summary": "Mockup ready: darker header",
                "artifacts": { "mockup": artifact_id },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.settle().await;
    let (status, body) = fixture
        .post("advance-stage", serde_json::json!({ "source": "operator" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.wait_for_stage("stakeholder").await;
    fixture.settle().await;

    // The gate's recorded decision plus the operator's departure continues
    // the task to plan.
    let (status, body) = fixture
        .post(
            "advance-stage",
            serde_json::json!({
                "source": "operator",
                "summary": "Stakeholders approved after the header fix",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.wait_for_stage("plan").await;
    fixture.settle().await;
    let gate_result = fixture
        .entries(LedgerEntryKind::Result)
        .into_iter()
        .find(|entry| {
            entry.body()["stage"] == "stakeholder"
                && entry
                    .message
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with("Stakeholders approved")
        })
        .expect("the gate's recorded decision");
    assert_eq!(gate_result.envelope["declared_role"], "operator");
    assert_eq!(
        fixture.running_run("plan", "main").agent.as_deref(),
        Some("plan")
    );
}
