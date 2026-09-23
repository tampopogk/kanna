//! Stages with no role (spec §5, T3) through the advance and result
//! endpoints: entering runs setup and parks without an agent, and a person
//! leaving records the gate's result and runs its teardown.
use super::actions::{
    commit_branch_change, get_task_detail, ledger_files, ledger_fixture_config, post_json,
    spawn_recording_daemon, wait_for_running_task_stage,
};
use super::*;
use crate::db::task_store::LedgerEntryKind;
use kanna_daemon::protocol::Command as DaemonCommand;

const TASK: &str = "gate-1";
const SETUP_MARKER: &str = ".stakeholders-mailed";
const TEARDOWN_COMMAND: &str = "printf STAKEHOLDER_WINDOW_CLOSED";

fn gate_workflow() -> serde_json::Value {
    serde_json::json!({
        "name": "gate-flow",
        "routing": "exits",
        "stages": [
            { "name": "in progress", "agent": "implement", "prompt": "$TASK_PROMPT",
              "policy": { "transition": "manual" } },
            { "name": "stakeholder",
              "setup": [format!("printf \"$KANNA_TASK_ID\" >> {SETUP_MARKER}")],
              "teardown": [TEARDOWN_COMMAND],
              "policy": { "transition": "manual" } },
            { "name": "review", "agent": "review", "prompt": "Review the branch.",
              "policy": { "transition": "manual" } }
        ]
    })
}

struct GateFixture {
    state: Arc<AppState>,
    app: axum::Router,
    db_path: String,
    commands: Arc<std::sync::Mutex<Vec<DaemonCommand>>>,
    worktree: PathBuf,
    repo_root: PathBuf,
    daemon_dir: PathBuf,
}

impl GateFixture {
    fn db(&self) -> Db {
        Db::open(&self.db_path).unwrap()
    }

    async fn post(&self, action: &str, body: serde_json::Value) -> (StatusCode, String) {
        post_json(
            &self.app,
            &format!("/v1/tasks/{TASK}/actions/{action}"),
            body,
        )
        .await
    }

    async fn settle(&self) {
        crate::http_api::wait_for_task_mutation_to_finish(&self.state, TASK).await;
    }

    fn entries(&self, kind: LedgerEntryKind) -> Vec<crate::task_store::LedgerFile> {
        crate::task_store::flush_task(&self.db(), &self.db_path, TASK).unwrap();
        ledger_files(&self.db_path, TASK)
            .into_iter()
            .filter(|file| file.kind == kind)
            .collect()
    }

    /// Agent sessions the daemon was asked to start for the task.
    fn agent_spawns(&self) -> usize {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .filter(|command| match command {
                DaemonCommand::Spawn { session_id, .. }
                | DaemonCommand::SpawnAgent { session_id, .. } => session_id == TASK,
                _ => false,
            })
            .count()
    }

    fn teardown_spawns(&self) -> Vec<String> {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .filter_map(|command| match command {
                DaemonCommand::Spawn {
                    session_id, args, ..
                } if session_id.starts_with("td-") => Some(args.join(" ")),
                _ => None,
            })
            .collect()
    }
}

impl Drop for GateFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.daemon_dir);
        let _ = std::fs::remove_dir_all(&self.repo_root);
    }
}

fn gate_fixture(label: &str) -> GateFixture {
    let workflow = gate_workflow();
    let repo_root = crate::test_paths::unique_test_path(&format!("kanna-gate-{label}"));
    init_test_git_repo(&repo_root);
    std::fs::write(
        repo_root.join(".kanna/workflows/gate-flow.json"),
        workflow.to_string(),
    )
    .unwrap();
    for args in [
        vec!["add", ".kanna/workflows/gate-flow.json"],
        vec!["commit", "-qm", "add gate workflow"],
    ] {
        assert!(Command::new("git")
            .args(&args)
            .current_dir(&repo_root)
            .status()
            .unwrap()
            .success());
    }
    publish_test_origin_main(&repo_root);
    let branch = format!("task-gate-{label}");
    let worktree = commit_branch_change(&repo_root, &branch, "mockup.html", "<p>mockup</p>");
    let daemon_dir = crate::test_paths::unique_test_path(&format!("kanna-gate-{label}-d"));
    std::fs::create_dir_all(&daemon_dir).unwrap();
    let commands = spawn_recording_daemon(&daemon_dir);
    let config = ledger_fixture_config(&format!("gate-{label}"), &daemon_dir);
    let db = Db::open_for_tests(&config.db_path).unwrap();
    db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
        .unwrap();
    db.insert_test_pipeline_item(
        TASK,
        "repo-1",
        "Design the thing",
        Some("Design the thing"),
        "in progress",
        "2026-09-23 10:00:00",
    )
    .unwrap();
    db.update_test_pipeline_item_stage_context(TASK, &branch, "gate-flow", None, "claude")
        .unwrap();
    db.update_test_pipeline_item_pipeline_def(TASK, &workflow.to_string())
        .unwrap();
    db.upsert_worktree("wt-gate-1", TASK, &worktree.to_string_lossy(), &branch)
        .unwrap();
    db.insert_stage_run(crate::db::NewStageRun {
        id: "impl-run",
        task_id: TASK,
        stage: "in progress",
        kind: "main",
        agent: Some("implement"),
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
    GateFixture {
        state,
        app,
        db_path: config.db_path,
        commands,
        worktree,
        repo_root,
        daemon_dir,
    }
}

async fn wait_for_stage(db: &Db, stage: &str) {
    for _ in 0..100 {
        if db
            .get_pipeline_item(TASK)
            .unwrap()
            .unwrap()
            .stage
            .as_deref()
            == Some(stage)
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("task never reached stage {stage}");
}

fn head(directory: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(directory)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Park at the gate: implement records its result, a person advances, and
/// the stage with no role is entered.
async fn enter_gate(fixture: &GateFixture) -> crate::db::StageRun {
    let (status, text) = fixture
        .post(
            "complete-stage",
            serde_json::json!({
                "runId": "impl-run", "status": "success", "summary": "Mockup ready",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    fixture.settle().await;
    let (status, text) = fixture
        .post("advance-stage", serde_json::json!({ "source": "operator" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let db = fixture.db();
    wait_for_stage(&db, "stakeholder").await;
    fixture.settle().await;
    db.list_stage_runs_for_task(TASK)
        .unwrap()
        .into_iter()
        .find(|run| run.stage == "stakeholder")
        .expect("the gate's run")
}

#[tokio::test]
async fn a_stage_with_no_role_runs_setup_parks_and_records_the_operator_who_leaves_it() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = gate_fixture("depart");
    let gate_run = enter_gate(&fixture).await;
    let db = fixture.db();

    // Entered: a fresh workspace from the implementer's commit, setup run
    // there once, and no agent started.
    assert_eq!(fixture.agent_spawns(), 0, "a gate spawns no agent");
    assert_eq!(gate_run.kind, "main");
    assert_eq!(gate_run.agent, None);
    assert_eq!(gate_run.session_id, None);
    assert_eq!(gate_run.status, "running", "parked, awaiting a person");
    let gate_workspace = PathBuf::from(gate_run.cwd.clone().unwrap());
    assert_ne!(gate_workspace, fixture.worktree);
    assert_eq!(head(&gate_workspace), head(&fixture.worktree));
    assert_eq!(
        std::fs::read_to_string(gate_workspace.join(SETUP_MARKER)).unwrap(),
        TASK,
        "setup ran once, in the gate's workspace, with the task environment"
    );
    let item = db.get_pipeline_item(TASK).unwrap().unwrap();
    assert_eq!(item.activity.as_deref(), Some("unread"));
    assert!(db.list_lifecycle_operation_intents().unwrap().is_empty());

    // T11b: task detail says a person, not a session, must decide.
    let detail = get_task_detail(&fixture.app, TASK).await;
    assert_eq!(detail.gate_parked, Some(true));
    let entry = fixture.entries(LedgerEntryKind::Transition).pop().unwrap();
    assert_eq!(entry.body()["to_stage"], "stakeholder");
    assert_eq!(entry.body()["exit_source"], "operator");

    // No session reports on a gate; resume and rerun have nothing to start.
    let (status, text) = fixture
        .post(
            "complete-stage",
            serde_json::json!({ "status": "success", "summary": "I am not here" }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{text}");
    assert!(text.contains("no role"), "{text}");
    // Gate fields are refused on a stage with an agent (checked on the next
    // stage below), and accepted here.
    let (status, text) = fixture
        .post(
            "advance-stage",
            serde_json::json!({
                "source": "operator",
                "summary": "Stakeholders approved the mockup\n\nTwo asked for a darker header.",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    wait_for_running_task_stage(&db, TASK, "review").await;
    fixture.settle().await;

    // Leaving: the gate's result is the operator's, on the verified channel.
    let departed = db.stage_run(&gate_run.id).unwrap().unwrap();
    assert_eq!(departed.status, "succeeded");
    let results = fixture.entries(LedgerEntryKind::Result);
    let gate_result = results.last().unwrap();
    assert_eq!(gate_result.body()["stage"], "stakeholder");
    assert_eq!(gate_result.body()["status"], "success");
    assert!(gate_result
        .message
        .as_deref()
        .unwrap_or_default()
        .starts_with("Stakeholders approved the mockup"));
    assert_eq!(gate_result.envelope["declared_role"], "operator");
    // The request's own channel (an in-process test request carries no
    // verified peer, so it reads unknown), never the engine's.
    assert_ne!(gate_result.envelope["channel_identity"]["kind"], "server");
    let transition = fixture.entries(LedgerEntryKind::Transition).pop().unwrap();
    assert_eq!(transition.body()["from_stage"], "stakeholder");
    assert_eq!(transition.body()["to_stage"], "review");
    assert_eq!(transition.body()["exit"], "advance");
    assert_eq!(transition.body()["exit_source"], "operator");
    assert_eq!(
        transition.body()["triggering_result_id"].as_str(),
        gate_result.entry_id()
    );
    // The next stage starts from the gate's commit, and the gate's teardown
    // runs in the workspace it leaves.
    let review_run = db
        .list_stage_runs_for_task(TASK)
        .unwrap()
        .into_iter()
        .find(|run| run.stage == "review")
        .unwrap();
    assert_eq!(
        head(Path::new(review_run.cwd.as_deref().unwrap())),
        head(&gate_workspace)
    );
    assert_eq!(fixture.agent_spawns(), 1, "only the review agent started");
    // T11b: departed onto a stage with a role, so the parked signal clears,
    // and the review run's own session joins the task's session history.
    let detail = get_task_detail(&fixture.app, TASK).await;
    assert_eq!(detail.gate_parked, Some(false));
    assert!(
        detail
            .session_history
            .iter()
            .any(|entry| entry.stage == "review"),
        "{:?}",
        detail.session_history
    );
    let teardowns = fixture.teardown_spawns();
    assert_eq!(teardowns.len(), 1, "{teardowns:?}");
    assert!(
        teardowns[0].contains("STAKEHOLDER_WINDOW_CLOSED"),
        "{}",
        teardowns[0]
    );
    // Setup was not run again on the way out.
    assert_eq!(
        std::fs::read_to_string(gate_workspace.join(SETUP_MARKER)).unwrap(),
        TASK
    );

    let (status, text) = fixture
        .post(
            "advance-stage",
            serde_json::json!({ "source": "operator", "summary": "not a gate" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
}

#[tokio::test]
async fn a_gate_without_a_message_records_who_left_it() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = gate_fixture("default");
    let gate_run = enter_gate(&fixture).await;
    let (status, text) = fixture
        .post("advance-stage", serde_json::json!({ "source": "manager" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let db = fixture.db();
    wait_for_running_task_stage(&db, TASK, "review").await;
    fixture.settle().await;
    let gate_result = fixture
        .entries(LedgerEntryKind::Result)
        .into_iter()
        .find(|entry| entry.envelope["run_id"] == gate_run.id.as_str())
        .expect("the gate's result");
    assert_eq!(gate_result.envelope["declared_role"], "manager");
    assert_eq!(
        gate_result.message.as_deref().map(str::trim),
        Some("Left stage 'stakeholder' (manager advance)")
    );
}
