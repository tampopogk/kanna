//! The 2026-09-16 capacity incident, end to end through the real server
//! wiring — and the negative that matters most.
//!
//! The Codex CLI refused a turn on the iOS release Ship task with
//! `⚠ Selected model is at capacity. Please try a different model.`, printed
//! one line and went back to its composer. No runtime edge fired, because the
//! refusal never sustained a busy classification, and no notice fired, because
//! the sentence matched no measured rule. Nothing woke.
//!
//! These live under the quota-recovery fixture deliberately: the fixture's
//! stage names an ordered candidate list, so if a capacity refusal were
//! classified as a spent allowance this would fall back to the other
//! candidate, close the running attempt as failed and burn a provider the
//! stage still has every right to use. The tests drive the real terminal-state
//! watcher and the real event feed, and assert that none of that happens while
//! the fact still reaches a supervisor.

use super::*;

/// The measured capacity refusal, as the daemon announces it after matching
/// it. The chrome is pinned in
/// `tests/cli-contract/fixtures/provider-capacity-refusal.json` and exercised
/// against the real classifier by the daemon's own suite; what this needs is
/// the announcement that reaches the server.
fn capacity_refusal(session_id: &str) -> kanna_daemon::protocol::Event {
    kanna_daemon::protocol::Event::ProviderNotice {
        session_id: session_id.to_string(),
        kind: kanna_daemon::protocol::ProviderNoticeKind::CapacityRefusal,
        session_kind: kanna_daemon::protocol::SessionKind::Pty,
        agent_provider: Some(kanna_daemon::protocol::AgentProvider::Codex),
        rule_id: "codex/notice/capacity-refusal".to_string(),
        scope: None,
        text: "⚠ Selected model is at capacity. Please try a different model.".to_string(),
        cli_version: Some("0.153.4".to_string()),
    }
}

/// The still-running Codex attempt the refusal belongs to. Codex is the
/// *second* candidate the fixture's stage names, so a quota-style recovery
/// would have a candidate left to walk to — which is exactly what must not
/// happen here.
fn insert_running_codex_attempt(db: &Db, repo_root: &std::path::Path, id: &str) {
    insert_running_review_run(db, repo_root, id, "codex", Some("gpt-6-astra"), Some("low"));
}

/// Every durable event row the real subscription selection would deliver for
/// this task.
///
/// The observation mode is the *durable feed*, and both parameters that
/// choose it are load-bearing. `from=beginning` is the deliberate opt-in to
/// reading below the tail: a cursorless wait defaults to `from=now`, so
/// without it the checkpoint is established above an event that was already
/// appended and this is structurally unable to see it.
/// `includeCurrentActivity=false` keeps the cold-start snapshot out, so what
/// comes back is the durable rows themselves and nothing else — which is
/// what this test is asserting about. (The snapshot is the *other* channel a
/// cold-starting supervisor reads; it is not what a refusal's durable record
/// is checked through.)
///
/// Driven through the HTTP wait with `orchestrationNotifications=true`, which
/// is the surface `kanna_subscribe_events` and `kanna_wait_events` sit on.
/// That flag also keeps this task's burst out of the collapse into a
/// synthetic `task.runtime_changed` state row, so the rows below arrive as
/// themselves.
async fn subscription_events(config: &Config) -> Vec<serde_json::Value> {
    let app = crate::http_api::router(std::sync::Arc::new(crate::http_api::AppState::new(
        config.clone(),
    )));
    let response = tower::ServiceExt::oneshot(
        app,
        axum::http::Request::get(format!(
            "/v1/task-events?taskIds={TASK_ID}&timeoutSecs=0&localOnly=true\
             &from=beginning&includeCurrentActivity=false\
             &orchestrationNotifications=true"
        ))
        .body(axum::body::Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    body["events"]
        .as_array()
        .expect("the wait answers with an event list")
        .clone()
}

/// Task detail, as `kanna_get_task` serves it.
async fn task_detail(config: &Config) -> serde_json::Value {
    let app = crate::http_api::router(std::sync::Arc::new(crate::http_api::AppState::new(
        config.clone(),
    )));
    let response = tower::ServiceExt::oneshot(
        app,
        axum::http::Request::get(format!("/v1/tasks/{TASK_ID}"))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn a_capacity_refusal_wakes_a_supervisor_without_entering_quota_recovery() {
    let config = test_config("capacity-notice");
    let (repo_root, db) = init_quota_fixture("capacity-notice", &config);
    insert_running_codex_attempt(&db, &repo_root, "run-capacity");

    let state = crate::http_api::AppState::new(config.clone());
    let replacements = state.session_replacements();
    let fake_daemon = spawn_fake_daemon_expecting_no_recovery(
        config.daemon_dir.clone(),
        vec![capacity_refusal(TASK_ID)],
    )
    .await;
    run_watcher(&state, &replacements).await;
    fake_daemon.await.unwrap();

    // The fact is recorded, once, with the claim exactly as wide as the
    // provider made it: the model this run selected, and no stated scope.
    let recorded = db.provider_capacity_notices_for_task(TASK_ID).unwrap();
    assert_eq!(recorded.len(), 1, "one refusal, one row: {recorded:?}");
    assert_eq!(recorded[0].provider, "codex");
    assert_eq!(recorded[0].model.as_deref(), Some("gpt-6-astra"));
    assert_eq!(recorded[0].stage, "review");
    assert_eq!(recorded[0].stage_run_id, "run-capacity");
    assert_eq!(recorded[0].scope, None);
    assert_eq!(recorded[0].source, "pty");
    assert_eq!(recorded[0].cli_version.as_deref(), Some("0.153.4"));

    // Nothing entered quota recovery. No rejection row, so no candidate is
    // burned; no rejection or parked event; and the attempt is untouched —
    // still running, never closed as failed, never replaced.
    assert!(db.provider_rejections_for_task(TASK_ID).unwrap().is_empty());
    assert!(db
        .providers_rejected_at_stage(TASK_ID, "review")
        .unwrap()
        .is_empty());
    assert!(events_of(&db, "task.provider_quota_rejected").is_empty());
    assert!(events_of(&db, "task.provider_quota_parked").is_empty());
    let runs = db.list_stage_runs_for_task(TASK_ID).unwrap();
    assert_eq!(runs.len(), 1, "no replacement run was started: {runs:?}");
    assert_eq!(runs[0].status, "running");
    assert_eq!(runs[0].result, None);
    assert_eq!(runs[0].no_work_termination, None);

    // The task is waiting for somebody to retry the turn, which is what
    // unread already means.
    assert_eq!(
        db.get_pipeline_item(TASK_ID)
            .unwrap()
            .unwrap()
            .activity
            .as_deref(),
        Some("unread")
    );

    let refusals = events_of(&db, "task.provider_capacity_refused");
    assert_eq!(refusals.len(), 1, "one refusal, one event: {refusals:?}");
    let payload = &refusals[0];
    assert_eq!(payload["provider"], "codex");
    assert_eq!(payload["model"], "gpt-6-astra");
    assert_eq!(payload["effort"], "low");
    assert!(payload["scope"].is_null(), "no scope was stated: {payload}");
    assert_eq!(payload["stage"], "review");
    assert_eq!(payload["stageRunId"], "run-capacity");
    assert_eq!(payload["source"], "pty");
    assert_eq!(payload["ruleId"], "codex/notice/capacity-refusal");
    assert_eq!(
        payload["matchedText"],
        "⚠ Selected model is at capacity. Please try a different model."
    );
    let action = payload["action"].as_str().expect("an action sentence");
    assert!(
        action.contains("Retry the turn") && action.contains("kanna_send_task_input"),
        "the recovery is to retry the turn, in words: {action}"
    );
    drop(db);

    // And it reaches a supervisor. This is the whole point: the incident was
    // invisible because neither channel woke anybody. What is asserted is the
    // durable row on the subscription selection — a supervisor that reads the
    // feed gets the refusal itself, not a state row it has to interpret.
    let delivered = subscription_events(&config).await;
    assert!(
        delivered
            .iter()
            .any(|event| event["type"] == "task.provider_capacity_refused"),
        "a capacity refusal must wake a subscription: {delivered:?}"
    );

    // And a reader of the task sees why it is waiting. A refused session parks
    // looking exactly like a healthy idle one, so neither activity nor
    // runtimeState can answer this.
    let detail = task_detail(&config).await;
    let reported = &detail["providerCapacityNotice"];
    assert_eq!(reported["provider"], "codex");
    assert_eq!(reported["model"], "gpt-6-astra");
    assert_eq!(reported["stage"], "review");
    assert_eq!(reported["stageRunId"], "run-capacity");
    assert!(reported.get("scope").is_none(), "no scope was stated");
    assert!(reported["action"]
        .as_str()
        .is_some_and(|action| action.contains("Retry the turn")));
    assert!(
        detail.get("providerRejection").is_none(),
        "a capacity refusal is never reported as a spent allowance: {detail}"
    );
}

/// A re-adopted session repaints the screen it is parked in front of, and the
/// daemon's latch is per session incarnation — so the server has to be the
/// thing that makes one refusal one observation.
#[tokio::test]
async fn a_replayed_capacity_refusal_records_and_wakes_once() {
    let config = test_config("capacity-notice-replay");
    let (repo_root, db) = init_quota_fixture("capacity-notice-replay", &config);
    insert_running_codex_attempt(&db, &repo_root, "run-capacity-replay");

    let state = crate::http_api::AppState::new(config.clone());
    let replacements = state.session_replacements();
    let fake_daemon = spawn_fake_daemon_expecting_no_recovery(
        config.daemon_dir.clone(),
        vec![capacity_refusal(TASK_ID), capacity_refusal(TASK_ID)],
    )
    .await;
    run_watcher(&state, &replacements).await;
    fake_daemon.await.unwrap();

    assert_eq!(
        db.provider_capacity_notices_for_task(TASK_ID)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(events_of(&db, "task.provider_capacity_refused").len(), 1);
}

/// A refusal announced for a run that is no longer the task's running attempt
/// describes a run that no longer exists. It is recorded nowhere, for the same
/// reason a stale quota rejection is.
#[tokio::test]
async fn a_capacity_refusal_for_a_finished_attempt_records_nothing() {
    let config = test_config("capacity-notice-stale");
    let (repo_root, db) = init_quota_fixture("capacity-notice-stale", &config);
    insert_running_codex_attempt(&db, &repo_root, "run-capacity-stale");
    db.finish_stage_run("run-capacity-stale", "succeeded", Some("done"), None)
        .unwrap();

    let state = crate::http_api::AppState::new(config.clone());
    let replacements = state.session_replacements();
    let fake_daemon = spawn_fake_daemon_expecting_no_recovery(
        config.daemon_dir.clone(),
        vec![capacity_refusal(TASK_ID)],
    )
    .await;
    run_watcher(&state, &replacements).await;
    fake_daemon.await.unwrap();

    assert!(db
        .provider_capacity_notices_for_task(TASK_ID)
        .unwrap()
        .is_empty());
    assert!(events_of(&db, "task.provider_capacity_refused").is_empty());
}
