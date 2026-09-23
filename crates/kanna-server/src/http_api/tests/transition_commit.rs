//! The commit step of a transition (`exit_commit`, spec §5, T3) through the
//! advance and result endpoints.
use super::actions::{ledger_files, ledger_fixture_config, post_json, wait_for_running_task_stage};
use super::*;
use crate::db::task_store::LedgerEntryKind;
use kanna_daemon::protocol::{Command as DaemonCommand, Event as DaemonEvent};

const TASK: &str = "commit-1";

/// How the fake daemon answers input to the task's live session.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Session {
    /// The session is alive and takes the instruction.
    Live,
    /// The session is gone: the commit step falls back to a fresh session.
    Dead,
    /// The instruction crosses the socket and the answer is lost, once.
    LoseAnswerOnce,
}

type Recorded = Arc<std::sync::Mutex<Vec<DaemonCommand>>>;

fn spawn_commit_daemon(daemon_dir: &Path, session: Session) -> Recorded {
    use tokio::io::{AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    let socket_path = daemon_socket_path_for_dir(&daemon_dir.to_string_lossy());
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path).unwrap();
    let commands: Recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = Arc::clone(&commands);
    let mode = Arc::new(std::sync::Mutex::new(session));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let recorded = Arc::clone(&recorded);
            let mode = Arc::clone(&mode);
            tokio::spawn(async move {
                let (read_half, mut write_half) = stream.into_split();
                let mut reader = BufReader::new(read_half);
                while let Some(command) =
                    read_test_daemon_command_optional(&mut reader, &mut write_half).await
                {
                    if super::answer_terminal_carryover_probe(&command, &mut write_half).await {
                        continue;
                    }
                    let not_found = DaemonEvent::Error {
                        code: Some(kanna_daemon::protocol::ErrorCode::SessionNotFound),
                        message: "session not found".to_string(),
                    };
                    let response = match &command {
                        DaemonCommand::List => DaemonEvent::SessionList {
                            sessions: Vec::new(),
                        },
                        DaemonCommand::Spawn { session_id, .. }
                        | DaemonCommand::SpawnAgent { session_id, .. } => {
                            DaemonEvent::SessionCreated {
                                session_id: session_id.clone(),
                            }
                        }
                        DaemonCommand::SubmitInput { .. } => {
                            let current = *mode.lock().unwrap();
                            match current {
                                Session::Live => DaemonEvent::Ok,
                                Session::Dead => not_found,
                                Session::LoseAnswerOnce => {
                                    *mode.lock().unwrap() = Session::Live;
                                    recorded.lock().unwrap().push(command);
                                    // EOF after the command: the answer is lost.
                                    return;
                                }
                            }
                        }
                        _ => not_found,
                    };
                    recorded.lock().unwrap().push(command);
                    write_half
                        .write_all(
                            format!("{}\n", serde_json::to_string(&response).unwrap()).as_bytes(),
                        )
                        .await
                        .unwrap();
                }
            });
        }
    });
    commands
}

fn spawns(commands: &Recorded) -> Vec<(String, String)> {
    commands
        .lock()
        .unwrap()
        .iter()
        .filter_map(|command| match command {
            DaemonCommand::Spawn {
                session_id, cwd, ..
            } => Some((session_id.clone(), cwd.clone())),
            DaemonCommand::SpawnAgent { session_id, params } => {
                Some((session_id.clone(), params.cwd.clone()))
            }
            _ => None,
        })
        .collect()
}

fn inputs(commands: &Recorded) -> Vec<String> {
    commands
        .lock()
        .unwrap()
        .iter()
        .filter_map(|command| match command {
            DaemonCommand::SubmitInput { data, .. } => {
                Some(String::from_utf8_lossy(data).to_string())
            }
            _ => None,
        })
        .collect()
}

fn commit_workflow(transition: &str) -> serde_json::Value {
    serde_json::json!({
        "name": "commit-flow",
        "routing": "exits",
        "stages": [
            { "name": "in progress", "agent": "implement", "prompt": "$TASK_PROMPT",
              "exit_commit": true, "policy": { "transition": transition } },
            { "name": "review", "agent": "review", "prompt": "Review the branch.",
              "policy": { "transition": "manual" } }
        ]
    })
}

struct CommitFixture {
    state: Arc<AppState>,
    app: axum::Router,
    db_path: String,
    commands: Recorded,
    worktree: PathBuf,
    repo_root: PathBuf,
    daemon_dir: PathBuf,
}

impl CommitFixture {
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

    async fn advance(&self) -> (StatusCode, String) {
        post_json(
            &self.app,
            &format!("/v1/tasks/{TASK}/actions/advance-stage"),
            serde_json::json!({ "source": "operator" }),
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

    /// The commit step's run: the newest post run of the task.
    fn commit_run(&self) -> crate::db::StageRun {
        self.db()
            .list_stage_runs_for_task(TASK)
            .unwrap()
            .into_iter()
            .rev()
            .find(|run| run.kind == "post")
            .expect("a commit step run")
    }

    /// Commit `file` in the stage's workspace, as the agent would.
    fn commit_in_workspace(&self, file: &str) -> String {
        std::fs::write(self.worktree.join(file), file).unwrap();
        for args in [vec!["add", file], vec!["commit", "-qm", file]] {
            assert!(Command::new("git")
                .args(&args)
                .current_dir(&self.worktree)
                .status()
                .unwrap()
                .success());
        }
        head(&self.worktree)
    }
}

impl Drop for CommitFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.daemon_dir);
        let _ = std::fs::remove_dir_all(&self.repo_root);
    }
}

fn head(directory: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(directory)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A named-exit task in `in progress` with a running implement run in a real
/// worktree, its workflow pinned.
fn commit_fixture(label: &str, workflow: serde_json::Value, session: Session) -> CommitFixture {
    let repo_root = crate::test_paths::unique_test_path(&format!("kanna-commit-{label}"));
    init_test_git_repo(&repo_root);
    std::fs::write(
        repo_root.join(".kanna/workflows/commit-flow.json"),
        workflow.to_string(),
    )
    .unwrap();
    for args in [
        vec!["add", ".kanna/workflows/commit-flow.json"],
        vec!["commit", "-qm", "add commit workflow"],
    ] {
        assert!(Command::new("git")
            .args(&args)
            .current_dir(&repo_root)
            .status()
            .unwrap()
            .success());
    }
    publish_test_origin_main(&repo_root);
    let branch = format!("task-commit-{label}");
    let worktree =
        super::actions::commit_branch_change(&repo_root, &branch, "work.txt", "implemented");
    let daemon_dir = crate::test_paths::unique_test_path(&format!("kanna-commit-{label}-d"));
    std::fs::create_dir_all(&daemon_dir).unwrap();
    let commands = spawn_commit_daemon(&daemon_dir, session);
    let config = ledger_fixture_config(&format!("commit-{label}"), &daemon_dir);
    let db = Db::open_for_tests(&config.db_path).unwrap();
    db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
        .unwrap();
    db.insert_test_pipeline_item(
        TASK,
        "repo-1",
        "Build the thing",
        Some("Build the thing"),
        "in progress",
        "2026-09-23 10:00:00",
    )
    .unwrap();
    db.update_test_pipeline_item_stage_context(TASK, &branch, "commit-flow", None, "claude")
        .unwrap();
    db.update_test_pipeline_item_pipeline_def(TASK, &workflow.to_string())
        .unwrap();
    db.upsert_worktree("wt-commit-1", TASK, &worktree.to_string_lossy(), &branch)
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
    CommitFixture {
        state,
        app,
        db_path: config.db_path,
        commands,
        worktree,
        repo_root,
        daemon_dir,
    }
}

/// Record the implementer's result on a manual stage: it parks, no commit
/// step runs until a person advances.
async fn record_implementation(fixture: &CommitFixture) {
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": "impl-run",
            "status": "success",
            "summary": "Implemented the thing",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.settle().await;
    assert!(inputs(&fixture.commands).is_empty());
    assert!(spawns(&fixture.commands).is_empty());
}

#[tokio::test]
async fn a_live_session_commits_in_place_and_the_next_stage_forks_from_that_commit() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = commit_fixture("live", commit_workflow("manual"), Session::Live);
    record_implementation(&fixture).await;
    // Work continues after the result, at the manual gate: the commit step is
    // what carries it forward.
    let (status, text) = fixture.advance().await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let instructions = inputs(&fixture.commands);
    assert_eq!(instructions.len(), 1, "one commit instruction");
    assert!(
        instructions[0].contains("Commit the work"),
        "{}",
        instructions[0]
    );
    assert!(
        spawns(&fixture.commands).is_empty(),
        "the live session commits"
    );
    let db = fixture.db();
    assert_eq!(
        db.get_pipeline_item(TASK)
            .unwrap()
            .unwrap()
            .stage
            .as_deref(),
        Some("in progress"),
        "the transition waits for the commit step's result"
    );
    let commit_run = fixture.commit_run();
    assert_eq!(commit_run.stage, "in progress commit");
    assert_eq!(commit_run.session_id.as_deref(), Some(TASK));
    let commit = db.transition_commit(&commit_run.id).unwrap().unwrap();
    assert_eq!(commit.state, "requested");
    assert_eq!(commit.stage, "in progress");
    let requested_exit = commit.exit.clone().unwrap();
    assert_eq!(requested_exit.exit.as_deref(), Some("advance"));
    assert_eq!(requested_exit.source, "operator");
    // Task detail shows the run as the transition's commit step.
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::get(format!("/v1/tasks/{TASK}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let detail: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let step = &detail["latestRun"]["commitStep"];
    assert_eq!(step["stage"], "in progress", "{detail}");
    assert_eq!(step["state"], "requested");
    assert_eq!(step["exit"], "advance");
    assert_eq!(step["exitSource"], "operator");
    // A second advance while the commit runs is not a second commit.
    let (status, text) = fixture.advance().await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(inputs(&fixture.commands).len(), 1);

    let committed = fixture.commit_in_workspace("late.txt");
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": commit_run.id,
            "status": "success",
            "summary": "Implemented the thing; committed late.txt",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let task = wait_for_running_task_stage(&db, TASK, "review").await;
    fixture.settle().await;

    // The next stage forks from the commit the commit step's result recorded.
    let review_run = db
        .list_stage_runs_for_task(TASK)
        .unwrap()
        .into_iter()
        .find(|run| run.stage == "review")
        .unwrap();
    let review_workspace = PathBuf::from(review_run.cwd.clone().unwrap());
    assert_ne!(review_workspace, fixture.worktree);
    assert_eq!(head(&review_workspace), committed);
    assert!(review_workspace.join("late.txt").exists());
    assert_ne!(task.branch.as_deref(), Some("task-commit-live"));

    let settled = db.transition_commit(&commit_run.id).unwrap().unwrap();
    assert_eq!(settled.state, "succeeded");
    assert_eq!(settled.committed_sha.as_deref(), Some(committed.as_str()));
    let results = fixture.entries(LedgerEntryKind::Result);
    let commit_result = results.last().unwrap();
    assert_eq!(commit_result.body()["committed_sha"], committed.as_str());
    assert_eq!(commit_result.body()["exit"], "advance");
    assert_eq!(commit_result.body()["exit_source"], "operator");
    assert_eq!(
        settled.result_id.as_deref(),
        commit_result.entry_id(),
        "the commit step settles on its own result"
    );
    let transitions = fixture.entries(LedgerEntryKind::Transition);
    assert_eq!(transitions.len(), 1, "one transition out of the stage");
    let transition = transitions[0].body();
    assert_eq!(transition["from_stage"], "in progress");
    assert_eq!(transition["to_stage"], "review");
    assert_eq!(transition["exit"], "advance");
    assert_eq!(transition["exit_source"], "operator");
    assert_eq!(
        transition["triggering_result_id"].as_str(),
        commit_result.entry_id()
    );
    // Exactly one commit step, one next-stage session.
    assert_eq!(inputs(&fixture.commands).len(), 1);
    assert_eq!(
        spawns(&fixture.commands)
            .iter()
            .filter(|(session, _)| session == TASK)
            .count(),
        1
    );

    // The settled commit step authorizes nothing further: a restarted
    // continuation sweep finds nothing owed.
    crate::http_api::task_actions::resume_ledger_continuations(Arc::clone(&fixture.state)).await;
    fixture.settle().await;
    assert_eq!(
        spawns(&fixture.commands)
            .iter()
            .filter(|(session, _)| session == TASK)
            .count(),
        1
    );
}

#[tokio::test]
async fn a_dead_session_gets_a_commit_session_in_the_same_workspace() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = commit_fixture("dead", commit_workflow("manual"), Session::Dead);
    record_implementation(&fixture).await;
    let (status, text) = fixture.advance().await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let db = fixture.db();
    let commit_run = fixture.commit_run();
    let fallback = spawns(&fixture.commands);
    assert_eq!(fallback.len(), 1, "one commit session");
    assert_eq!(
        PathBuf::from(&fallback[0].1).canonicalize().unwrap(),
        fixture.worktree.canonicalize().unwrap(),
        "the commit session runs in the stage's own workspace"
    );
    assert_eq!(commit_run.agent.as_deref(), Some("commit"));
    assert_eq!(commit_run.stage, "in progress commit");
    assert_eq!(
        db.get_pipeline_item(TASK)
            .unwrap()
            .unwrap()
            .stage
            .as_deref(),
        Some("in progress")
    );
    let commit = db.transition_commit(&commit_run.id).unwrap().unwrap();
    assert_eq!(commit.state, "requested");
    assert_eq!(commit.exit.unwrap().source, "operator");

    let committed = fixture.commit_in_workspace("rescued.txt");
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": commit_run.id,
            "status": "success",
            "summary": "Committed rescued.txt",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    wait_for_running_task_stage(&db, TASK, "review").await;
    fixture.settle().await;
    let review_run = db
        .list_stage_runs_for_task(TASK)
        .unwrap()
        .into_iter()
        .find(|run| run.stage == "review")
        .unwrap();
    assert_eq!(head(Path::new(&review_run.cwd.unwrap())), committed);
}

#[tokio::test]
async fn a_failed_commit_parks_with_no_transition_and_cannot_be_corrected_into_one() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = commit_fixture("failed", commit_workflow("manual"), Session::Live);
    record_implementation(&fixture).await;
    let (status, text) = fixture.advance().await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let commit_run = fixture.commit_run();
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": commit_run.id,
            "status": "failure",
            "summary": "Cannot tell whether scratch.rs belongs to the task",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.settle().await;
    let db = fixture.db();
    assert_eq!(
        db.get_pipeline_item(TASK)
            .unwrap()
            .unwrap()
            .stage
            .as_deref(),
        Some("in progress")
    );
    assert!(spawns(&fixture.commands).is_empty(), "nothing started");
    assert!(fixture.entries(LedgerEntryKind::Transition).is_empty());
    assert_eq!(
        db.transition_commit(&commit_run.id).unwrap().unwrap().state,
        "failed"
    );

    // The failed commit step settled: a corrected verdict on it is refused
    // rather than turning into the transition it did not authorize.
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": commit_run.id,
            "status": "success",
            "summary": "Actually fine",
        }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    fixture.settle().await;
    assert!(spawns(&fixture.commands).is_empty());
    assert!(fixture.entries(LedgerEntryKind::Transition).is_empty());

    // A person advancing again requests a new commit step.
    let (status, text) = fixture.advance().await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(inputs(&fixture.commands).len(), 2);
    let second = fixture.commit_run();
    assert_ne!(second.id, commit_run.id);
    assert_eq!(
        db.transition_commit(&second.id).unwrap().unwrap().state,
        "requested"
    );
}

#[tokio::test]
async fn a_commit_result_cannot_choose_an_exit_or_request_another_commit() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = commit_fixture("exit", commit_workflow("manual"), Session::Live);
    record_implementation(&fixture).await;
    let (status, text) = fixture.advance().await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let commit_run = fixture.commit_run();
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": commit_run.id,
            "status": "success",
            "summary": "Committed",
            "exit": "revise",
        }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("commit step"), "{body}");
    let db = fixture.db();
    assert_eq!(
        db.stage_run(&commit_run.id).unwrap().unwrap().status,
        "running",
        "nothing was recorded"
    );

    // Naming the requested exit is the same result; it fires the one
    // transition and never a second commit step.
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": commit_run.id,
            "status": "success",
            "summary": "Committed",
            "exit": "advance",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    wait_for_running_task_stage(&db, TASK, "review").await;
    fixture.settle().await;
    assert_eq!(inputs(&fixture.commands).len(), 1);
    assert_eq!(
        db.list_stage_runs_for_task(TASK)
            .unwrap()
            .iter()
            .filter(|run| run.kind == "post")
            .count(),
        1
    );
    // Recorded again after it settled, it is refused: one commit step
    // authorizes one transition.
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": commit_run.id,
            "status": "success",
            "summary": "Committed again",
        }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(fixture.entries(LedgerEntryKind::Transition).len(), 1);
}

#[tokio::test]
async fn an_auto_stage_runs_its_commit_step_on_the_result_that_advances_it() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = commit_fixture("auto", commit_workflow("auto"), Session::Live);
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": "impl-run",
            "status": "success",
            "summary": "Implemented the thing",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.settle().await;
    assert_eq!(inputs(&fixture.commands).len(), 1, "the commit step ran");
    let db = fixture.db();
    assert_eq!(
        db.get_pipeline_item(TASK)
            .unwrap()
            .unwrap()
            .stage
            .as_deref(),
        Some("in progress")
    );
    let commit_run = fixture.commit_run();
    let commit = db.transition_commit(&commit_run.id).unwrap().unwrap();
    let exit = commit.exit.unwrap();
    assert_eq!(exit.exit.as_deref(), Some("advance"));
    assert_eq!(exit.source, "default", "the result named no exit");
    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": commit_run.id,
            "status": "success",
            "summary": "Committed",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    wait_for_running_task_stage(&db, TASK, "review").await;
    fixture.settle().await;
    let transition = fixture.entries(LedgerEntryKind::Transition).pop().unwrap();
    assert_eq!(transition.body()["exit_source"], "default");
    assert_eq!(inputs(&fixture.commands).len(), 1);
}

/// The commit instruction crosses the socket and its answer is lost. A retry
/// reconciles the delivery the daemon accepted (the commit step is recorded
/// once) and is refused instead of instructing the session twice; the one
/// commit result then fires one transition, and a restarted continuation
/// sweep adds none.
#[tokio::test]
async fn a_lost_commit_acknowledgement_commits_once_and_transitions_once() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = commit_fixture("ack", commit_workflow("manual"), Session::LoseAnswerOnce);
    record_implementation(&fixture).await;
    let (status, text) = fixture.advance().await;
    assert_ne!(status, StatusCode::OK, "{text}");
    let db = fixture.db();
    assert_eq!(db.list_lifecycle_operation_intents().unwrap().len(), 1);
    assert!(db
        .list_stage_runs_for_task(TASK)
        .unwrap()
        .iter()
        .all(|run| run.kind != "post"));

    let (status, text) = fixture.advance().await;
    assert_ne!(status, StatusCode::OK, "{text}");
    assert!(text.contains("still running"), "{text}");
    assert!(db.list_lifecycle_operation_intents().unwrap().is_empty());
    assert_eq!(inputs(&fixture.commands).len(), 1, "never instructed twice");
    let commit_run = fixture.commit_run();
    assert_eq!(
        db.transition_commit(&commit_run.id).unwrap().unwrap().state,
        "requested"
    );

    let (status, body) = fixture
        .complete(serde_json::json!({
            "runId": commit_run.id,
            "status": "success",
            "summary": "Committed",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    wait_for_running_task_stage(&db, TASK, "review").await;
    fixture.settle().await;
    crate::http_api::task_actions::resume_ledger_continuations(Arc::clone(&fixture.state)).await;
    fixture.settle().await;
    assert_eq!(fixture.entries(LedgerEntryKind::Transition).len(), 1);
    assert_eq!(
        spawns(&fixture.commands)
            .iter()
            .filter(|(session, _)| session == TASK)
            .count(),
        1
    );
}
