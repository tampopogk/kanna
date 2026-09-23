//! T10c acceptance fixture (round-1 QA finding 2): the real shipped
//! `.kanna/workflows/specialized-reviewers.json` (named-exit routing)
//! dispatches two `specialty-review` children through one
//! `kanna_create_subtasks` join, each verdict reaches the parent's inputs
//! exactly once, a blocking FAIL records success with `exit: "revise"`
//! (looping to `in progress`), an all-PASS panel records plain success
//! (advancing to `pr`), and a task still pinned to the pre-T10c
//! legacy-routed snapshot revises through `kanna_request_revision`.

use super::actions::{
    commit_branch_change, ledger_fixture_config, post_json, wait_for_running_task_stage,
};
use super::*;
use kanna_daemon::protocol::{
    Command as DaemonCommand, Event as DaemonEvent, SessionInfo, SessionState, SessionStatus,
};

const PARENT: &str = "task-qa-parent";
const PARENT_PID: u32 = 4343;

/// The real shipped named-exit workflow this task's `review` stage runs.
const SPECIALIZED_REVIEWERS_JSON: &str =
    include_str!("../../../../../.kanna/workflows/specialized-reviewers.json");

/// The pre-T10c legacy-routed snapshot a task created before this
/// conversion still carries pinned. Legacy routing has no `routing`/`exits`
/// key; findings route back through `kanna_request_revision`, not `exit`.
fn legacy_specialized_reviewers_snapshot() -> serde_json::Value {
    serde_json::json!({
        "name": "specialized-reviewers",
        "revision_limit": 5,
        "stages": [
            {
                "name": "in progress",
                "agent": "implement",
                "prompt": "$TASK_PROMPT",
                "policy": { "transition": "manual", "revision_transition": "auto" },
                "post": { "name": "commit", "agent": "commit", "prompt": "Commit the work." }
            },
            {
                "name": "review",
                "agent": "qa-dispatcher",
                "prompt": "Dispatch the specialty reviews needed for branch $BRANCH.",
                "policy": { "transition": "auto" }
            },
            {
                "name": "pr",
                "agent": "pr",
                "prompt": "Create a PR for the reviewed work on branch $BRANCH.",
                "policy": { "transition": "manual" },
                "post": { "name": "approve", "agent": "approve", "prompt": "Approve the PR." }
            }
        ]
    })
}

type Commands = Arc<std::sync::Mutex<Vec<DaemonCommand>>>;

/// A daemon on which the parent has a live PTY session: it lists that
/// session, accepts every spawn and fenced submission, and records each
/// command — the same shape T5's own `subtask_joins.rs` fixture uses, so a
/// resolved join can type its notice into the parent's session.
fn spawn_join_daemon(daemon_dir: &Path) -> Commands {
    use tokio::io::{AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    let socket_path = daemon_socket_path_for_dir(&daemon_dir.to_string_lossy());
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path).unwrap();
    let commands: Commands = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = Arc::clone(&commands);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let (read_half, mut write_half) = stream.into_split();
                let mut reader = BufReader::new(read_half);
                while let Some(command) =
                    read_test_daemon_command_optional(&mut reader, &mut write_half).await
                {
                    if answer_terminal_carryover_probe(&command, &mut write_half).await {
                        continue;
                    }
                    let response = match &command {
                        DaemonCommand::List => DaemonEvent::SessionList {
                            sessions: vec![SessionInfo {
                                session_id: PARENT.to_string(),
                                pid: PARENT_PID,
                                cwd: "/tmp".to_string(),
                                state: SessionState::Active,
                                idle_seconds: 0,
                                status: SessionStatus::Waiting,
                                status_observed: true,
                                kind: Default::default(),
                                composer_text: None,
                                composer_attestation: Default::default(),
                                attempt_id: None,
                            }],
                        },
                        DaemonCommand::Spawn { session_id, .. }
                        | DaemonCommand::SpawnAgent { session_id, .. } => {
                            DaemonEvent::SessionCreated {
                                session_id: session_id.clone(),
                            }
                        }
                        DaemonCommand::SubmitInputIfSession {
                            session_id,
                            expected_pid,
                            ..
                        } if session_id == PARENT && *expected_pid == PARENT_PID => DaemonEvent::Ok,
                        DaemonCommand::Kill { .. } => DaemonEvent::Ok,
                        _ => DaemonEvent::Error {
                            code: Some(kanna_daemon::protocol::ErrorCode::SessionNotFound),
                            message: "session not found".to_string(),
                        },
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

struct QaDispatchFixture {
    state: Arc<AppState>,
    app: axum::Router,
    db_path: String,
    repo_root: PathBuf,
    daemon_dir: PathBuf,
}

impl Drop for QaDispatchFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.daemon_dir);
        let _ = std::fs::remove_dir_all(&self.repo_root);
    }
}

impl QaDispatchFixture {
    fn db(&self) -> Db {
        Db::open(&self.db_path).unwrap()
    }

    async fn post(&self, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let (status, text) = post_json(&self.app, uri, body).await;
        let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
        (status, value)
    }

    async fn create_children(&self, children: serde_json::Value) -> serde_json::Value {
        let (status, body) = self
            .post(
                &format!("/v1/tasks/{PARENT}/subtasks"),
                serde_json::json!({ "children": children }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn complete_child(&self, task_id: &str, status: &str, summary: &str) {
        let (code, body) = self
            .post(
                &format!("/v1/tasks/{task_id}/actions/complete-stage"),
                serde_json::json!({ "status": status, "summary": summary }),
            )
            .await;
        assert_eq!(code, StatusCode::OK, "{body}");
        crate::http_api::wait_for_task_mutation_to_finish(&self.state, task_id).await;
    }

    async fn complete_parent(&self, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let (status, value) = self
            .post(&format!("/v1/tasks/{PARENT}/actions/complete-stage"), body)
            .await;
        crate::http_api::wait_for_task_mutation_to_finish(&self.state, PARENT).await;
        (status, value)
    }

    async fn request_revision(&self, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let (status, value) = self
            .post(
                &format!("/v1/tasks/{PARENT}/actions/request-revision"),
                body,
            )
            .await;
        crate::http_api::wait_for_task_mutation_to_finish(&self.state, PARENT).await;
        (status, value)
    }

    fn join_inputs(&self) -> Vec<crate::db::TaskInputRecord> {
        self.db()
            .list_task_inputs(PARENT, 100)
            .unwrap()
            .into_iter()
            .filter(|input| input.source == "subtask_join")
            .collect()
    }
}

/// A parent task at its `review` stage of `workflow`, with a real worktree
/// holding one committed change (the reviewed commit every child forks
/// from) and a daemon that accepts every spawn and fenced submission.
fn qa_dispatch_fixture(label: &str, workflow: serde_json::Value, stage: &str) -> QaDispatchFixture {
    let repo_root = crate::test_paths::unique_test_path(&format!("kanna-qa-dispatch-{label}"));
    init_test_git_repo(&repo_root);
    let branch = format!("task-qa-parent-{label}");
    let worktree = commit_branch_change(
        &repo_root,
        &branch,
        "reviewed.txt",
        "the change under review",
    );
    let daemon_dir = crate::test_paths::unique_test_path(&format!("kanna-qa-dispatch-{label}-d"));
    std::fs::create_dir_all(&daemon_dir).unwrap();
    spawn_join_daemon(&daemon_dir);
    let config = ledger_fixture_config(&format!("qa-dispatch-{label}"), &daemon_dir);
    let db = Db::open_for_tests(&config.db_path).unwrap();
    db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
        .unwrap();
    db.insert_test_pipeline_item(
        PARENT,
        "repo-1",
        "Fix the sticky workflow",
        Some("Fix the sticky workflow"),
        stage,
        "2026-09-23 10:00:00",
    )
    .unwrap();
    db.update_test_pipeline_item_stage_context(
        PARENT,
        &branch,
        "specialized-reviewers",
        None,
        "claude",
    )
    .unwrap();
    db.update_test_pipeline_item_pipeline_def(PARENT, &workflow.to_string())
        .unwrap();
    db.upsert_worktree("wt-qa-parent", PARENT, &worktree.to_string_lossy(), &branch)
        .unwrap();
    db.insert_stage_run(crate::db::NewStageRun {
        id: "qa-parent-run",
        task_id: PARENT,
        stage,
        kind: "main",
        agent: Some("qa-dispatcher"),
        agent_provider: Some("claude"),
        model: None,
        effort: None,
        status: "running",
        result: None,
        feedback: None,
        session_id: Some(PARENT),
        provider_session_id: None,
        cwd: Some(&worktree.to_string_lossy()),
        resumed_from_run_id: None,
    })
    .unwrap();
    drop(db);
    let state = Arc::new(AppState::new(config.clone()));
    let app = super::router(Arc::clone(&state));
    QaDispatchFixture {
        state,
        app,
        db_path: config.db_path,
        repo_root,
        daemon_dir,
    }
}

fn specialty_child(agent: &str, subject: &str) -> serde_json::Value {
    serde_json::json!({
        "prompt": format!("{agent} review dispatched from task {PARENT}."),
        "displayName": format!("{subject} review: sticky workflow"),
        "workflowName": "specialty-review",
        "agent": agent,
        "agentProvider": "claude",
    })
}

#[tokio::test]
async fn joined_specialty_verdicts_arrive_once_and_a_blocking_fail_revises_the_named_exit_workflow()
{
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let workflow: serde_json::Value = serde_json::from_str(SPECIALIZED_REVIEWERS_JSON).unwrap();
    assert_eq!(
        workflow["routing"], "exits",
        "sanity: the shipped workflow uses named exits"
    );
    let fixture = qa_dispatch_fixture("revise", workflow, "review");

    let created = fixture
        .create_children(serde_json::json!([
            specialty_child("review-security", "Security"),
            specialty_child("review-compat", "Compatibility"),
        ]))
        .await;
    let children = created["children"]
        .as_array()
        .expect("subtasks response carries the created children")
        .clone();
    assert_eq!(children.len(), 2, "{created}");
    let security_id = children[0]["taskId"].as_str().unwrap().to_string();
    let compat_id = children[1]["taskId"].as_str().unwrap().to_string();

    // Each child resolves once, on its own first recorded result.
    fixture
        .complete_child(
            &security_id,
            "success",
            "PASS: no security-relevant surface touched",
        )
        .await;
    fixture
        .complete_child(
            &compat_id,
            "failure",
            "FAIL: api.rs:42 removed a required field without a version bump",
        )
        .await;

    // The parent read as blocked while any member was unresolved (T5); once
    // both resolve, each verdict reached its inputs exactly once — not
    // merged or deduplicated by the engine, and never delivered twice.
    let inputs = fixture.join_inputs();
    assert_eq!(inputs.len(), 2, "{inputs:?}");
    let messages: Vec<&str> = inputs.iter().map(|input| input.message.as_str()).collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("no security-relevant surface")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("removed a required field")),
        "{messages:?}"
    );

    // qa-dispatcher aggregates: one blocking FAIL survives, so it records
    // success with exit "revise" and a closed list of findings.
    let (status, body) = fixture
        .complete_parent(serde_json::json!({
            "runId": "qa-parent-run",
            "status": "success",
            "summary": "QA failed: Compatibility FAIL (api.rs:42, removed a required field without a version bump)",
            "exit": "revise",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["routing"]["exit"], "revise");
    assert_eq!(body["routing"]["destination"], "in progress");
    assert_eq!(body["routing"]["outcome"], "loop");

    let db = fixture.db();
    wait_for_running_task_stage(&db, PARENT, "in progress").await;
}

#[tokio::test]
async fn an_all_pass_panel_records_plain_success_and_advances_past_review() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let workflow: serde_json::Value = serde_json::from_str(SPECIALIZED_REVIEWERS_JSON).unwrap();
    let fixture = qa_dispatch_fixture("pass", workflow, "review");

    let created = fixture
        .create_children(serde_json::json!([specialty_child(
            "review-security",
            "Security"
        )]))
        .await;
    let child_id = created["children"][0]["taskId"]
        .as_str()
        .unwrap()
        .to_string();
    fixture
        .complete_child(
            &child_id,
            "success",
            "PASS: nothing security-relevant in this diff",
        )
        .await;

    assert_eq!(fixture.join_inputs().len(), 1);

    // Nothing blocks: plain success, no exit — the "review" stage's own
    // `transition: auto` policy advances it past "pr" on its implicit
    // `advance` exit.
    let (status, body) = fixture
        .complete_parent(serde_json::json!({
            "runId": "qa-parent-run",
            "status": "success",
            "summary": "QA passed: Security PASS",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["routing"]["outcome"], "advance");

    let db = fixture.db();
    wait_for_running_task_stage(&db, PARENT, "pr").await;
}

#[tokio::test]
async fn a_task_pinned_to_the_pre_t10c_legacy_snapshot_still_revises_through_request_revision() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let legacy = legacy_specialized_reviewers_snapshot();
    assert!(
        legacy.get("routing").is_none(),
        "sanity: the pinned snapshot predates named-exit routing"
    );
    let fixture = qa_dispatch_fixture("legacy", legacy, "review");

    let (status, body) = fixture
        .request_revision(serde_json::json!({
            "runId": "qa-parent-run",
            "targetStage": "in progress",
            "summary": "QA failed: Compatibility FAIL",
            "prompt": "Fix api.rs:42 — a required field was added without a version bump.",
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["revisionBudget"].is_object(), "{body}");

    let db = fixture.db();
    wait_for_running_task_stage(&db, PARENT, "in progress").await;
}
