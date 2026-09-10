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
