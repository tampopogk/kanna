//! Subtask joins (T5) through the HTTP API: children are created in a join
//! forked from the parent's committed HEAD, the parent is held and reads as
//! blocked until every member resolves, each child's result reaches the
//! parent's inputs once and is typed into its live session, a crashed child
//! stays unresolved and actionable, and a restarted launch creates each
//! member once.

use super::actions::{commit_branch_change, ledger_fixture_config, post_json, spawn_count};
use super::*;
use kanna_daemon::protocol::{
    Command as DaemonCommand, Event as DaemonEvent, SessionInfo, SessionState, SessionStatus,
};

const PARENT: &str = "task-parent";
const PARENT_PID: u32 = 4242;

type Commands = Arc<std::sync::Mutex<Vec<DaemonCommand>>>;

fn head_of(path: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A daemon on which the parent has a live PTY session: it lists that
/// session, accepts every spawn, kill and fenced submission, and records
/// each command.
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

/// Text typed into the parent's session, in order.
fn typed_into_parent(commands: &Commands) -> Vec<String> {
    commands
        .lock()
        .unwrap()
        .iter()
        .filter_map(|command| match command {
            DaemonCommand::SubmitInputIfSession {
                session_id, data, ..
            } if session_id == PARENT => Some(String::from_utf8_lossy(data).to_string()),
            _ => None,
        })
        .collect()
}

struct JoinFixture {
    state: Arc<AppState>,
    app: axum::Router,
    db_path: String,
    commands: Commands,
    parent_worktree: PathBuf,
    repo_root: PathBuf,
    daemon_dir: PathBuf,
}

impl Drop for JoinFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.daemon_dir);
        let _ = std::fs::remove_dir_all(&self.repo_root);
    }
}

impl JoinFixture {
    fn db(&self) -> Db {
        Db::open(&self.db_path).unwrap()
    }

    async fn get(&self, uri: &str) -> serde_json::Value {
        let response = self
            .app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        serde_json::from_slice(&body).unwrap()
    }

    async fn post(&self, uri: &str, body: serde_json::Value) -> (StatusCode, String) {
        post_json(&self.app, uri, body).await
    }

    async fn create_children(&self, children: serde_json::Value) -> serde_json::Value {
        let (status, body) = self
            .post(
                &format!("/v1/tasks/{PARENT}/subtasks"),
                serde_json::json!({ "children": children }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_str(&body).unwrap()
    }

    async fn complete(&self, task_id: &str, status: &str, summary: &str) -> (StatusCode, String) {
        let (code, body) = self
            .post(
                &format!("/v1/tasks/{task_id}/actions/complete-stage"),
                serde_json::json!({ "status": status, "summary": summary }),
            )
            .await;
        crate::http_api::wait_for_task_mutation_to_finish(&self.state, task_id).await;
        (code, body)
    }

    fn join_inputs(&self) -> Vec<crate::db::TaskInputRecord> {
        self.db()
            .list_task_inputs(PARENT, 100)
            .unwrap()
            .into_iter()
            .filter(|input| input.source == "subtask_join")
            .collect()
    }

    /// Wait for the detached notice sweep to type `count` notices.
    async fn typed(&self, count: usize) -> Vec<String> {
        for _ in 0..100 {
            let typed = typed_into_parent(&self.commands);
            if typed.len() >= count {
                return typed;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        typed_into_parent(&self.commands)
    }
}

fn child(prompt: &str) -> serde_json::Value {
    serde_json::json!({
        "prompt": prompt,
        "displayName": prompt,
        "workflowName": TEST_PROVIDER_NEUTRAL_WORKFLOW,
        "agentProvider": "claude",
    })
}

/// A parent in its running `in progress` stage with a real worktree holding
/// one commit past `main` and an uncommitted edit on top.
fn join_fixture(label: &str) -> JoinFixture {
    let repo_root = crate::test_paths::unique_test_path(&format!("kanna-join-{label}"));
    init_test_git_repo(&repo_root);
    let parent_worktree =
        commit_branch_change(&repo_root, PARENT, "parent.txt", "committed parent work");
    std::fs::write(parent_worktree.join("draft.txt"), "uncommitted").unwrap();
    let daemon_dir = crate::test_paths::unique_test_path(&format!("kanna-join-{label}-d"));
    std::fs::create_dir_all(&daemon_dir).unwrap();
    let commands = spawn_join_daemon(&daemon_dir);
    let config = ledger_fixture_config(&format!("join-{label}"), &daemon_dir);
    let db = Db::open_for_tests(&config.db_path).unwrap();
    db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
        .unwrap();
    db.insert_test_pipeline_item(
        PARENT,
        "repo-1",
        "Review the change",
        Some("Review the change"),
        "in progress",
        "2026-09-23 10:00:00",
    )
    .unwrap();
    db.update_test_pipeline_item_stage_context(
        PARENT,
        PARENT,
        TEST_PROVIDER_NEUTRAL_WORKFLOW,
        None,
        "claude",
    )
    .unwrap();
    db.update_test_pipeline_item_pipeline_def(
        PARENT,
        &serde_json::json!({
            "name": TEST_PROVIDER_NEUTRAL_WORKFLOW,
            "stages": [{
                "name": "in progress",
                "prompt": "$TASK_PROMPT",
                "policy": { "transition": "manual" }
            }]
        })
        .to_string(),
    )
    .unwrap();
    db.upsert_worktree(
        "wt-task-parent",
        PARENT,
        &parent_worktree.to_string_lossy(),
        PARENT,
    )
    .unwrap();
    db.insert_stage_run(crate::db::NewStageRun {
        id: "parent-run",
        task_id: PARENT,
        stage: "in progress",
        kind: "main",
        agent: Some("review"),
        agent_provider: Some("claude"),
        model: None,
        effort: None,
        status: "running",
        result: None,
        feedback: None,
        session_id: Some(PARENT),
        provider_session_id: None,
        cwd: Some(&parent_worktree.to_string_lossy()),
        resumed_from_run_id: None,
    })
    .unwrap();
    drop(db);
    let state = Arc::new(AppState::new(config.clone()));
    let app = super::router(Arc::clone(&state));
    JoinFixture {
        state,
        app,
        db_path: config.db_path,
        commands,
        parent_worktree,
        repo_root,
        daemon_dir,
    }
}

#[tokio::test]
async fn children_start_at_the_parent_sha_and_the_parent_waits_for_every_result() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = join_fixture("flow");
    let parent_sha = head_of(&fixture.parent_worktree);

    // Fields the join owns are refused, not silently dropped.
    let (status, _) = fixture
        .post(
            &format!("/v1/tasks/{PARENT}/subtasks"),
            serde_json::json!({ "children": [{ "prompt": "x", "baseRef": "main" }] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(fixture.db().list_task_joins(PARENT).unwrap().is_empty());

    let created = fixture
        .create_children(serde_json::json!([child("security"), child("performance")]))
        .await;
    assert_eq!(created["baseSha"], parent_sha.as_str());
    let children: Vec<String> = created["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|child| {
            assert_eq!(child["created"], true, "{child}");
            child["taskId"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(children.len(), 2);
    assert_eq!(spawn_count(&fixture.commands), 2);

    let db = fixture.db();
    let join_id = created["joinId"].as_str().unwrap().to_string();
    let join = db.task_join(&join_id).unwrap().unwrap();
    assert_eq!(join.base_sha, parent_sha);
    assert_eq!(join.parent_run_id.as_deref(), Some("parent-run"));
    for child_id in &children {
        let item = db.get_pipeline_item(child_id).unwrap().unwrap();
        assert_eq!(item.parent_task_id.as_deref(), Some(PARENT));
        assert_eq!(item.base_ref.as_deref(), Some(parent_sha.as_str()));
        let worktree = PathBuf::from(db.get_task_worktree_path(child_id).unwrap().unwrap());
        assert_eq!(
            head_of(&worktree),
            parent_sha,
            "{child_id} forked elsewhere"
        );
        assert!(worktree.join("parent.txt").exists());
        assert!(
            !worktree.join("draft.txt").exists(),
            "uncommitted parent edits are not part of the fork point"
        );
    }

    // The parent reads as blocked on its children and cannot progress.
    let joins = fixture.get(&format!("/v1/tasks/{PARENT}/joins")).await;
    assert_eq!(joins["blocked"], true);
    assert_eq!(joins["waitingOn"], serde_json::json!(children));
    assert_eq!(joins["joins"][0]["members"][0]["state"], "running");
    let detail = fixture.get(&format!("/v1/tasks/{PARENT}")).await;
    assert_eq!(detail["blockedByTaskIds"], serde_json::json!(children));
    let (status, body) = fixture.complete(PARENT, "success", "Combined").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("task is blocked:"), "{body}");
    assert!(body.contains(&children[0]) && body.contains(&children[1]));
    let parent_run = db.stage_run("parent-run").unwrap().unwrap();
    assert_eq!(parent_run.status, "running", "nothing was recorded");
    let (status, body) = fixture
        .post(
            &format!("/v1/tasks/{PARENT}/actions/advance-stage"),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("task is blocked:"), "{body}");
    let (status, body) = fixture
        .post(
            &format!("/v1/tasks/{PARENT}/actions/close"),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains(&children[0]) && body.contains(&children[1]));

    // A success result: delivered once as a parent input and typed into the
    // parent's live session.
    let (status, body) = fixture
        .complete(&children[0], "success", "Security: no findings")
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let inputs = fixture.join_inputs();
    assert_eq!(inputs.len(), 1);
    assert!(inputs[0].message.contains("Security: no findings"));
    let typed = fixture.typed(1).await;
    assert_eq!(typed.len(), 1);
    assert!(typed[0].contains("Security: no findings"), "{typed:?}");
    let (status, _) = fixture.complete(PARENT, "success", "Combined").await;
    assert_eq!(status, StatusCode::CONFLICT, "still one child to go");

    // A non-success result completes the join.
    let (status, body) = fixture
        .complete(&children[1], "failure", "Performance: benchmark broke")
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let inputs = fixture.join_inputs();
    assert_eq!(inputs.len(), 2);
    assert!(inputs[1].message.contains("recorded failure"));
    assert!(inputs[1].message.contains("Performance: benchmark broke"));
    let typed = fixture.typed(2).await;
    assert_eq!(typed.len(), 2);
    let joins = fixture.get(&format!("/v1/tasks/{PARENT}/joins")).await;
    assert_eq!(joins["blocked"], false);
    assert_eq!(joins["joins"][0]["complete"], true);
    assert_eq!(joins["joins"][0]["members"][1]["status"], "failure");
    assert_eq!(joins["joins"][0]["members"][1]["state"], "resolved");

    // Nothing is delivered or typed twice when more happens afterwards.
    super::super::subtask_joins::spawn_join_notices(&fixture.state);
    let (status, _) = fixture
        .complete(&children[0], "success", "Security: one more note")
        .await;
    assert_eq!(status, StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(fixture.join_inputs().len(), 2);
    assert_eq!(typed_into_parent(&fixture.commands).len(), 2);

    // The join no longer holds the parent; its open children still hold its
    // close.
    let (status, body) = fixture.complete(PARENT, "success", "Combined").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = fixture
        .post(
            &format!("/v1/tasks/{PARENT}/actions/close"),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("open subtasks"), "{body}");
}

#[tokio::test]
async fn a_crashed_child_stays_unresolved_and_actionable_until_closed() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = join_fixture("crash");
    let created = fixture
        .create_children(serde_json::json!([child("flaky")]))
        .await;
    let child_id = created["children"][0]["taskId"]
        .as_str()
        .unwrap()
        .to_string();

    // Its session dies without a result.
    let db = fixture.db();
    let run = db.latest_stage_run(&child_id).unwrap().unwrap();
    db.finish_stage_run(&run.id, "failed", None, None).unwrap();
    db.connection_for_e2e_tests()
        .execute(
            "UPDATE pipeline_item SET runtime_status = 'exited' WHERE id = ?",
            [&child_id],
        )
        .unwrap();

    let joins = fixture.get(&format!("/v1/tasks/{PARENT}/joins")).await;
    let member = &joins["joins"][0]["members"][0];
    assert_eq!(member["state"], "stalled", "{member}");
    assert_eq!(
        member["actions"],
        serde_json::json!(["kanna_resume_task", "kanna_rerun_stage", "kanna_close_task"])
    );
    assert_eq!(joins["blocked"], true, "never silently counted");
    assert!(fixture.join_inputs().is_empty());
    let (status, _) = fixture.complete(PARENT, "success", "Combined").await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Closing it is the explicit resolution; the parent is told.
    let (status, body) = fixture
        .post(
            &format!("/v1/tasks/{child_id}/actions/close"),
            serde_json::json!({}),
        )
        .await;
    assert!(status.is_success(), "{status}: {body}");
    let inputs = fixture.join_inputs();
    assert_eq!(inputs.len(), 1);
    assert!(inputs[0]
        .message
        .contains("was closed without recording a result"));
    let joins = fixture.get(&format!("/v1/tasks/{PARENT}/joins")).await;
    assert_eq!(joins["blocked"], false);
    assert_eq!(joins["joins"][0]["members"][0]["outcome"], "closed");
    assert_eq!(fixture.typed(1).await.len(), 1);
}

#[tokio::test]
async fn a_child_that_cannot_be_created_is_reported_to_the_parent() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = join_fixture("not-created");
    let created = fixture
        .create_children(serde_json::json!([
            child("fine"),
            { "prompt": "broken", "workflowName": "no-such-workflow", "agentProvider": "claude" }
        ]))
        .await;
    assert_eq!(created["children"][0]["created"], true);
    assert_eq!(created["children"][1]["created"], false);
    let broken = created["children"][1]["taskId"].as_str().unwrap();
    assert!(fixture.db().get_pipeline_item(broken).unwrap().is_none());
    let member = &created["join"]["members"][1];
    assert_eq!(member["outcome"], "not_created");
    let inputs = fixture.join_inputs();
    assert_eq!(inputs.len(), 1);
    assert!(inputs[0].message.contains("could not be created"));
    // The parent still waits on the child that exists.
    let fine = created["children"][0]["taskId"].as_str().unwrap();
    assert_eq!(
        fixture.db().unresolved_join_children(PARENT).unwrap(),
        vec![fine.to_string()]
    );
}

/// A launch interrupted after its join was recorded but before a member's
/// task existed creates that member on restart, from its recorded id and
/// spec, exactly once however often the sweep runs.
#[tokio::test]
async fn a_restart_creates_each_uncreated_member_once() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = join_fixture("resume");
    let parent_sha = head_of(&fixture.parent_worktree);
    let db = fixture.db();
    db.create_task_join(&crate::db::NewTaskJoin {
        id: "join-resume".to_string(),
        parent_task_id: PARENT.to_string(),
        parent_stage: Some("in progress".to_string()),
        parent_run_id: Some("parent-run".to_string()),
        base_sha: parent_sha.clone(),
        base_branch: Some(PARENT.to_string()),
        members: vec![crate::db::NewJoinMember {
            child_task_id: "c0ffee01".to_string(),
            spec: child("resumed").to_string(),
        }],
    })
    .unwrap();
    assert_eq!(db.list_blocking_task_ids(PARENT).unwrap(), vec!["c0ffee01"]);

    let restarted = Arc::new(AppState::new(fixture.state.config.clone()));
    super::super::subtask_joins::resume_subtask_joins(Arc::clone(&restarted)).await;
    super::super::subtask_joins::resume_subtask_joins(Arc::clone(&restarted)).await;
    assert_eq!(spawn_count(&fixture.commands), 1);
    let item = db.get_pipeline_item("c0ffee01").unwrap().unwrap();
    assert_eq!(item.parent_task_id.as_deref(), Some(PARENT));
    let worktree = PathBuf::from(db.get_task_worktree_path("c0ffee01").unwrap().unwrap());
    assert_eq!(head_of(&worktree), parent_sha);
    assert!(db.list_uncreated_join_members().unwrap().is_empty());
    assert!(fixture.join_inputs().is_empty());
}

#[tokio::test]
async fn status_advertises_subtask_joins() {
    let fixture = join_fixture("capability");
    let status = fixture.get("/v1/status").await;
    assert_eq!(
        status["subtaskJoinsVersion"],
        kanna_tool_catalog::SUBTASK_JOINS_VERSION
    );
    kanna_tool_catalog::confirm_subtask_joins_supported(&status).unwrap();
}
