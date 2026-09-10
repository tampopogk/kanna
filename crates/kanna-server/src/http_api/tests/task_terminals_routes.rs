use super::*;
use serde_json::Value;

/// The Workspace log is where a closed attempt tab is found again, so a
/// retained agent attempt has to reach it with the handle that reopens it.
///
/// Reconciliation deliberately never reopens a tab the reader closed — that is
/// their decision — which is exactly why the log must carry the record id, the
/// archived flag and the status of every attempt it lists.
#[tokio::test]
async fn the_activity_log_carries_the_handle_that_reopens_a_retained_attempt() {
    let state = test_state_with_seed("activity-retained-attempt", "Activity", |db| {
        db.insert_test_repo("repo-1", "Repo One").unwrap();
        db.insert_test_pipeline_item(
            "task-1",
            "repo-1",
            "do the thing",
            None,
            "review",
            "2026-09-09T00:00:00Z",
        )
        .unwrap();
        db.insert_stage_run(crate::db::NewStageRun {
            id: "run-1",
            task_id: "task-1",
            stage: "in progress",
            kind: "main",
            agent: Some("build"),
            agent_provider: Some("claude"),
            model: None,
            effort: None,
            status: "succeeded",
            result: None,
            feedback: None,
            session_id: Some("task-1"),
            provider_session_id: None,
            cwd: Some("/tmp/wt"),
            resumed_from_run_id: None,
        })
        .unwrap();
        // The launch's startup shell, and the agent attempt that stage ran.
        db.upsert_task_terminal_session(crate::db::NewTaskTerminalSession {
            id: "setup-task-1-1",
            repo_id: "repo-1",
            task_id: Some("task-1"),
            daemon_session_id: Some("setup-task-1-1"),
            role: crate::db::ROLE_SETUP,
            stage: Some("in progress"),
            attempt: 1,
            stage_run_id: None,
            title: Some("Startup · in progress"),
            cwd: Some("/tmp/wt"),
        })
        .unwrap();
        db.retire_task_terminal_session("setup-task-1-1", Some(0))
            .unwrap();
        db.upsert_task_terminal_session(crate::db::NewTaskTerminalSession {
            id: "agent-task-1-2",
            repo_id: "repo-1",
            task_id: Some("task-1"),
            // The task's own session id, which every attempt shares.
            daemon_session_id: Some("task-1"),
            role: crate::db::ROLE_AGENT,
            stage: Some("in progress"),
            attempt: 2,
            stage_run_id: Some("run-1"),
            title: Some("Agent · in progress · attempt 2"),
            cwd: Some("/tmp/wt"),
        })
        .unwrap();
        db.record_terminal_session_archive("agent-task-1-2", 80, 24, "OUTGOING_AGENT_FRAME")
            .unwrap();
        db.retire_task_terminal_session_record("agent-task-1-2", Some(0))
            .unwrap();
    });

    let response = crate::http_api::dispatch_authenticated_http_invoke(
        state,
        "GET",
        "/v1/tasks/task-1/activity",
        Value::Null,
    )
    .await;
    assert_eq!(response.status, 200, "{:?}", response.body);
    let body = response
        .body
        .expect("the activity route answers with a body");
    let entries = body["entries"].as_array().unwrap().clone();

    let startup = entries
        .iter()
        .find(|entry| entry["kind"] == "terminal")
        .expect("the startup shell is one of the workspace's operations");
    assert_eq!(startup["terminalSessionId"], "setup-task-1-1");

    let agent = entries
        .iter()
        .find(|entry| entry["kind"] == "agent")
        .expect("the stage's agent run is listed");
    assert_eq!(
        agent["terminalSessionId"], "agent-task-1-2",
        "the run carries the record its retained output lives in: {agent:?}"
    );
    assert_eq!(agent["archived"], true);
    assert_eq!(agent["exitCode"], 0);
    assert_eq!(agent["attempt"], 2);
    assert_eq!(agent["title"], "Agent · in progress · attempt 2");
}

/// The agent view waits on the server's answer, not on a missing PTY.
///
/// A launch that failed writes a failed run and no runtime state at all, so
/// the task row alone cannot tell it from a startup terminal that is still
/// running — and the daemon refuses the attach identically for both.
#[tokio::test]
async fn the_terminals_list_says_whether_a_launch_can_still_start_the_agent() {
    let pending = test_state_with_seed("terminals-launch-pending", "Pending", |db| {
        db.insert_test_repo("repo-1", "Repo One").unwrap();
        db.insert_test_pipeline_item(
            "task-1",
            "repo-1",
            "still starting",
            None,
            "in progress",
            "2026-09-09T00:00:00Z",
        )
        .unwrap();
    });
    let response = crate::http_api::dispatch_authenticated_http_invoke(
        pending,
        "GET",
        "/v1/tasks/task-1/terminals",
        Value::Null,
    )
    .await;
    assert_eq!(response.status, 200, "{:?}", response.body);
    let body = response
        .body
        .expect("the terminals route answers with a body");
    assert_eq!(
        body["agentLaunchPending"], true,
        "a task whose launch has recorded nothing yet is still starting"
    );

    let failed = test_state_with_seed("terminals-launch-failed", "Failed", |db| {
        db.insert_test_repo("repo-1", "Repo One").unwrap();
        db.insert_test_pipeline_item(
            "task-2",
            "repo-1",
            "failed launch",
            None,
            "in progress",
            "2026-09-09T00:00:00Z",
        )
        .unwrap();
        db.insert_stage_run(crate::db::NewStageRun {
            id: "run-failed",
            task_id: "task-2",
            stage: "in progress",
            kind: "main",
            agent: None,
            agent_provider: None,
            model: None,
            effort: None,
            status: "failed",
            result: Some(
                "workspace setup failed (exit 23); see the startup terminal for this stage",
            ),
            feedback: None,
            session_id: Some("task-2"),
            provider_session_id: None,
            cwd: Some("/tmp/wt"),
            resumed_from_run_id: None,
        })
        .unwrap();
    });
    let response = crate::http_api::dispatch_authenticated_http_invoke(
        failed,
        "GET",
        "/v1/tasks/task-2/terminals",
        Value::Null,
    )
    .await;
    assert_eq!(response.status, 200, "{:?}", response.body);
    let body = response
        .body
        .expect("the terminals route answers with a body");
    assert_eq!(
        body["agentLaunchPending"], false,
        "a launch that failed will never produce an agent, and the view must stop waiting"
    );
}

/// Closing a task keeps its agent's last screen, and the log can still open it.
///
/// A close is the one kill with no successor, so nothing will ever print that
/// output again — and it is exactly when someone asks what the task was doing
/// when it ended. This drives the real close route against a daemon holding a
/// live agent session and then reads the task's activity feed, because the
/// record only matters if the log can reach it.
#[tokio::test]
async fn closing_a_task_keeps_its_agent_screen_and_the_log_can_reopen_it() {
    use kanna_daemon::protocol::{Command as DaemonCommand, Event as DaemonEvent};
    use tokio::io::{AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    let daemon_dir = crate::test_paths::unique_test_path("kanna-http-close-retain-daemon");
    std::fs::create_dir_all(&daemon_dir).unwrap();
    let socket_path = super::daemon_socket_path_for_dir(&daemon_dir.to_string_lossy());
    let _ = std::fs::remove_file(&socket_path);
    let daemon_listener = UnixListener::bind(&socket_path).unwrap();
    let daemon_server = tokio::spawn(async move {
        let (stream, _) = daemon_listener.accept().await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let mut killed = Vec::new();
        // This daemon really holds the task's agent session, so the frame the
        // close reads is a frame — not the empty answer a session-less fixture
        // gives, which would prove nothing about what was kept.
        while killed.len() < 3 {
            let command = super::read_scripted_test_daemon_command(&mut reader, &mut write_half).await;
            let response = match &command {
                DaemonCommand::Snapshot { session_id } if session_id == "710917fb" => {
                    DaemonEvent::Snapshot {
                        session_id: session_id.clone(),
                        snapshot: kanna_daemon::protocol::TerminalSnapshot {
                            version: 1,
                            rows: 24,
                            cols: 80,
                            cursor_row: 0,
                            cursor_col: 0,
                            cursor_visible: true,
                            vt: "CLOSING_AGENT_FRAME".to_string(),
                            saved_at: 0,
                            sequence: 1,
                        },
                        agent_provider: None,
                    }
                }
                DaemonCommand::Kill { session_id } => {
                    killed.push(session_id.clone());
                    DaemonEvent::Ok
                }
                other => panic!("unexpected daemon command: {other:?}"),
            };
            write_half
                .write_all(format!("{}\n", serde_json::to_string(&response).unwrap()).as_bytes())
                .await
                .unwrap();
        }
        killed
    });

    let state = crate::http_api::test_support::test_state_with_daemon_dir(
        "close-retains-agent",
        "Close",
        &daemon_dir.to_string_lossy(),
        |db| {
            db.insert_test_repo("repo-1", "Repo One").unwrap();
            db.insert_test_pipeline_item(
                "710917fb",
                "repo-1",
                "do the thing",
                None,
                "in progress",
                "2026-05-11 10:00:00",
            )
            .unwrap();
            db.update_test_pipeline_item_stage_context(
                "710917fb",
                "task-710917fb",
                "default",
                None,
                "claude",
            )
            .unwrap();
            db.insert_stage_run(crate::db::NewStageRun {
                id: "run-closing-main",
                task_id: "710917fb",
                stage: "in progress",
                kind: "main",
                agent: Some("implement"),
                agent_provider: Some("claude"),
                model: None,
                effort: None,
                status: "running",
                result: None,
                feedback: None,
                session_id: Some("710917fb"),
                provider_session_id: None,
                cwd: Some("/tmp/wt"),
                resumed_from_run_id: None,
            })
            .unwrap();
        },
    );
    let db_path = state.config().db_path.clone();

    let response = super::router(Arc::clone(&state))
        .oneshot(
            Request::post("/v1/tasks/task-710917fb/actions/close")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let killed = daemon_server.await.unwrap();
    assert_eq!(
        killed.first().map(String::as_str),
        Some("710917fb"),
        "the agent session is the first thing a close ends: {killed:?}"
    );

    let db = Db::open(&db_path).expect("reopen db");
    let retained: Vec<_> = db
        .list_task_terminal_sessions("710917fb")
        .unwrap()
        .into_iter()
        .filter(|terminal| terminal.role == crate::db::ROLE_AGENT)
        .collect();
    assert_eq!(
        retained.len(),
        1,
        "a closed task keeps the attempt it was running: {retained:?}"
    );
    let attempt = &retained[0];
    assert_eq!(attempt.state, "retired");
    assert_eq!(attempt.stage.as_deref(), Some("in progress"));
    assert_eq!(attempt.stage_run_id.as_deref(), Some("run-closing-main"));
    assert_eq!(
        db.read_terminal_session_archive(&attempt.id)
            .unwrap()
            .expect("the closed task keeps its agent's last screen")
            .vt,
        "CLOSING_AGENT_FRAME"
    );
    drop(db);

    // The record only matters if the log can reach it: a closed task's tabs
    // are gone, and this is the only way back to what its agent printed.
    let activity = crate::http_api::dispatch_authenticated_http_invoke(
        state,
        "GET",
        "/v1/tasks/710917fb/activity",
        Value::Null,
    )
    .await;
    assert_eq!(activity.status, 200, "{:?}", activity.body);
    let body = activity.body.expect("the activity route answers with a body");
    let agent = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "agent")
        .cloned()
        .expect("the closed task's agent run is listed");
    assert_eq!(agent["terminalSessionId"], attempt.id);
    assert_eq!(agent["archived"], true);

    let _ = std::fs::remove_dir_all(daemon_dir);
    let _ = std::fs::remove_file(db_path);
}
