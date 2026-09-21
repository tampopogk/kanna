//! Bounded per-subscription knobs: the validated rate-limit override and
//! the event_types/exclude_event_types filter passthrough. These reuse the
//! subscription's own query/timing ownership — no new scheduler, no runtime
//! retry loop.
//!
//! `min_admission_interval_ms` is now the only timing knob. It was once one
//! of three: `max_hold_ms` collapsed into `quiet_ms`, and `quiet_ms` — the
//! trailing-quiet window that reset on every relevant observation — went
//! with the mechanism itself (owner decision, 2026-09-21), because holding
//! a lone event for a full quiet period is exactly the delay the wake
//! pacing was asked to remove. Both retired names are now simply unknown
//! fields, rejected like any other typo (`SubscribeRequest` denies unknown
//! fields) rather than accepted and silently ignored: exactly one live
//! subscription exists in this repository, so nothing is served by a
//! release of pretending to honor timing that no longer exists.
use super::*;
use crate::db::TaskEventKind;

async fn subscribe(app: &Router, body: Value) -> (StatusCode, Value) {
    subscription_request(app, "POST", "/v1/event-subscriptions", body).await
}

#[tokio::test]
async fn invalid_timing_overrides_are_rejected_before_any_registration() {
    let state = test_state_with_seed("overrides-invalid", "Overrides", seed_orchestration);
    let db = Db::open(&state.config().db_path).unwrap();
    start_run(&db, "manager", "child-c", "in progress");
    let app = router(state.clone());
    let base = json!({"taskId":"child-c", "localOnly":true, "delivery":"poll"});
    let mut body = base.clone();
    body["minAdmissionIntervalMs"] = json!(500);
    let (status, response) = subscribe(&app, body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "minAdmissionIntervalMs below the 1000ms floor: {response}"
    );
    // A retired knob is now simply an unknown field, rejected the same way
    // as any other typo — never silently ignored, which would leave a
    // caller believing timing it named is being applied.
    for retired in ["maxHoldMs", "quietMs"] {
        let mut body = base.clone();
        body[retired] = json!(10_000);
        let (status, response) = subscribe(&app, body).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{retired} no longer exists: {response}"
        );
    }
    // Nothing was left half-registered by a rejected request.
    assert!(db.event_subscriptions().unwrap().is_empty());
}

#[tokio::test]
async fn there_is_no_reachable_overflow_so_an_extreme_override_is_accepted_not_panicked() {
    // No policy ceiling. `Duration::from_millis` accepts any `u64`, and so
    // does the `Instant + Duration` arithmetic it feeds into
    // (`Collection::intrinsic_deadline`, `Admission`): a `u64` millisecond
    // count can never exceed `Duration`'s own (far larger) capacity, so
    // registration has nothing to reject here — confirmed empirically, not
    // just assumed. An extreme min_admission_interval_ms is genuinely
    // honored — the collector chains native calls to cover it — and just
    // delays this subscription's own future wakes.
    let state = test_state_with_seed("overrides-extreme", "Overrides", seed_orchestration);
    let db = Db::open(&state.config().db_path).unwrap();
    start_run(&db, "manager", "child-c", "in progress");
    let app = router(state.clone());
    let (status, initial) = subscribe(
        &app,
        json!({"taskId":"child-c", "localOnly":true, "delivery":"poll",
            "minAdmissionIntervalMs": u64::MAX}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{initial}");
    assert_eq!(initial["query"]["minAdmissionIntervalMs"], u64::MAX);
    assert_eq!(db.event_subscriptions().unwrap().len(), 1);
}

#[tokio::test]
async fn omitted_overrides_keep_the_exact_prior_query_shape() {
    let state = test_state_with_seed("overrides-omitted", "Overrides", seed_orchestration);
    let db = Db::open(&state.config().db_path).unwrap();
    start_run(&db, "manager", "child-c", "in progress");
    let app = router(state.clone());
    let (status, initial) = subscribe(
        &app,
        json!({"taskId":"child-c", "localOnly":true, "delivery":"poll"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{initial}");
    // An untouched request never persists the override keys, so a
    // pre-existing row's `existing.query != query` retry comparison is
    // unaffected by this feature's addition.
    for key in ["quietMs", "minAdmissionIntervalMs", "eventTypes"] {
        assert!(initial["query"][key].is_null(), "{key}: {initial}");
    }
    assert_eq!(
        initial["query"]["excludeEventTypes"],
        "task.activity_changed,task.runtime_settled,task.input_delivered"
    );
    let retry = subscribe(
        &app,
        json!({"taskId":"child-c", "localOnly":true, "delivery":"poll"}),
    )
    .await
    .1;
    assert_eq!(retry["id"], initial["id"], "retry must reuse the mailbox");
}

#[tokio::test]
async fn explicit_overrides_are_persisted() {
    let state = test_state_with_seed("overrides-persisted", "Overrides", seed_orchestration);
    let db = Db::open(&state.config().db_path).unwrap();
    start_run(&db, "manager", "child-c", "in progress");
    let app = router(state.clone());
    let (status, initial) = subscribe(
        &app,
        json!({"taskId":"child-c", "localOnly":true, "delivery":"poll",
            "minAdmissionIntervalMs": 1_000,
            "eventTypes": ["task.pr_created"], "excludeEventTypes": ["task.blocked"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{initial}");
    assert_eq!(initial["query"]["minAdmissionIntervalMs"], 1_000);
    assert_eq!(initial["query"]["eventTypes"], "task.pr_created");
    // Additive to the fixed baseline, not a replacement for it.
    let excluded = initial["query"]["excludeEventTypes"].as_str().unwrap();
    for baseline in [
        "task.activity_changed",
        "task.runtime_settled",
        "task.input_delivered",
        "task.blocked",
    ] {
        assert!(excluded.contains(baseline), "{excluded}");
    }
}

#[tokio::test]
async fn an_ordinary_batch_seals_at_the_overridden_rate_limit() {
    let state = test_state_with_seed("overrides-rate-limit", "Overrides", seed_orchestration);
    let db = Db::open(&state.config().db_path).unwrap();
    start_run(&db, "manager", "child-c", "in progress");
    let app = router(state.clone());
    let (status, initial) = subscribe(
        &app,
        json!({"taskId":"child-c", "localOnly":true, "delivery":"poll",
            "minAdmissionIntervalMs": 1_000}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{initial}");
    assert!(initial["pending"].is_null());
    let id = initial["id"].as_str().unwrap().to_string();
    let service = tokio::spawn(super::super::super::event_subscriptions::run(state.clone()));
    // Ordinary (non-urgent) event: nothing here is urgent and the page is
    // nowhere near full, so only the rate-limit gate can seal it. The worker
    // starts after registration, so it arms one recovery cooldown at this
    // subscription's own overridden interval; without the override that
    // would be the 60000ms default, which this real-time test cannot afford
    // to wait out. The floor (1000ms) is the smallest legal override.
    db.append_task_event("child-a", TaskEventKind::PrCreated, json!({}))
        .unwrap();
    let row = await_subscription(&state, &id, |row| row.wake_state == "ready").await;
    assert_eq!(
        event_pairs(row.pending.as_ref().unwrap()),
        vec![("child-a".into(), "task.pr_created".into())]
    );
    service.abort();
    let _ = service.await;
}

#[tokio::test]
async fn event_types_allowlist_admits_only_the_named_types() {
    let state = test_state_with_seed("overrides-allowlist", "Overrides", seed_orchestration);
    let db = Db::open(&state.config().db_path).unwrap();
    start_run(&db, "manager", "child-c", "in progress");
    let app = router(state.clone());
    let (status, initial) = subscribe(
        &app,
        json!({"taskId":"child-c", "localOnly":true, "delivery":"poll",
            "eventTypes": ["task.awaiting_input"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{initial}");
    assert!(initial["pending"].is_null());
    let id = initial["id"].as_str().unwrap().to_string();
    let service = tokio::spawn(super::super::super::event_subscriptions::run(state.clone()));
    // Urgent on its own, but excluded by the allow-list.
    db.append_task_event("child-a", TaskEventKind::LifecycleFailed, json!({}))
        .unwrap();
    // Named by the allow-list; also urgent, so it seals immediately.
    db.append_task_event("child-b", TaskEventKind::AwaitingInput, json!({}))
        .unwrap();
    let row = await_subscription(&state, &id, |row| row.pending.is_some()).await;
    assert_eq!(
        event_pairs(row.pending.as_ref().unwrap()),
        vec![("child-b".into(), "task.awaiting_input".into())]
    );
    service.abort();
    let _ = service.await;
}

#[tokio::test]
async fn exclude_event_types_is_additive_to_the_fixed_baseline() {
    let state = test_state_with_seed("overrides-exclude", "Overrides", seed_orchestration);
    let db = Db::open(&state.config().db_path).unwrap();
    start_run(&db, "manager", "child-c", "in progress");
    let app = router(state.clone());
    let (status, initial) = subscribe(
        &app,
        json!({"taskId":"child-c", "localOnly":true, "delivery":"poll",
            "excludeEventTypes": ["task.awaiting_input"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{initial}");
    let id = initial["id"].as_str().unwrap().to_string();
    let service = tokio::spawn(super::super::super::event_subscriptions::run(state.clone()));
    // Excluded by the caller's own addition, on top of the baseline.
    db.append_task_event("child-a", TaskEventKind::AwaitingInput, json!({}))
        .unwrap();
    // Not excluded; urgent, so it seals the same collection.
    db.append_task_event("child-a", TaskEventKind::LifecycleFailed, json!({}))
        .unwrap();
    let row = await_subscription(&state, &id, |row| row.pending.is_some()).await;
    assert_eq!(
        event_pairs(row.pending.as_ref().unwrap()),
        vec![("child-a".into(), "task.lifecycle_failed".into())]
    );
    service.abort();
    let _ = service.await;
}

/// The exact minimal request an MCP/CLI caller sends when it does not ask
/// for any timing override — the same shape `resolve_request` would build
/// for a caller who never named `min_admission_interval_ms` (nor either of
/// the retired `quiet_ms`/`max_hold_ms`).
fn resolved_minimal_subscribe(task_id: &str, delivery: &str) -> Value {
    kanna_tool_catalog::resolve_request(
        &kanna_tool_catalog::bundled_catalog(),
        "kanna_subscribe_events",
        &json!({"task_id": task_id, "local_only": true, "delivery": delivery}),
    )
    .expect("minimal subscribe request resolves")
    .body
}

#[tokio::test]
async fn mcp_resolved_omitted_knobs_resume_a_legacy_active_subscription_without_conflict() {
    // Catalog request construction -> HTTP -> durable storage, end to end:
    // the resolver must not bake a documented default into the wire body
    // (crates/kanna-tool-catalog's own resolver test proves that in
    // isolation), and the server must treat the resulting omitted-knob
    // request as an exact retry of a subscription that predates this
    // feature — not a differently configured one.
    let state = test_state_with_seed(
        "overrides-mcp-legacy-active",
        "Overrides",
        seed_orchestration,
    );
    start_run(
        &Db::open(&state.config().db_path).unwrap(),
        "manager",
        "child-c",
        "in progress",
    );
    let app = router(state.clone());
    let request = resolved_minimal_subscribe("child-c", "poll");
    let (status, initial) = subscribe(&app, request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{initial}");
    let id = initial["id"].as_str().unwrap().to_string();
    for key in ["quietMs", "minAdmissionIntervalMs"] {
        assert!(initial["query"][key].is_null(), "{key}: {initial}");
    }
    let (status, retried) = subscribe(&app, request).await;
    assert_eq!(status, StatusCode::OK, "{retried}");
    assert_eq!(
        retried["id"], id,
        "an MCP-resolved retry must reuse the mailbox, not conflict or reset"
    );
}

#[tokio::test]
async fn mcp_resolved_omitted_knobs_resume_a_legacy_paused_subscription_without_reset() {
    let state = test_state_with_seed(
        "overrides-mcp-legacy-paused",
        "Overrides",
        seed_orchestration,
    );
    let db = Db::open(&state.config().db_path).unwrap();
    start_run(&db, "manager", "child-c", "in progress");
    let app = router(state.clone());
    let request = resolved_minimal_subscribe("child-c", "poll");
    let (status, initial) = subscribe(&app, request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{initial}");
    let id = initial["id"].as_str().unwrap().to_string();
    // A watch fault pauses the subscription (matches the existing raw-HTTP
    // subscription_watch_failure_... coverage): inactive, with an error, but
    // not "stopped" — only an explicit unsubscribe is "stopped", and must not
    // resume. The durable cursor here (empty for a fresh subscription) is
    // exactly what a real fault would retain and must survive untouched.
    let mut row = db.event_subscription(&id).unwrap().unwrap();
    assert_eq!(
        row.query["shortCursor"],
        json!(false),
        "the persistent worker must keep a durable full checkpoint"
    );
    let checkpoint = row.cursor.clone();
    row.active = false;
    row.error = Some("event watch stopped: simulated fault".into());
    assert!(db.save_event_subscription(&mut row).unwrap());
    let (status, resumed) = subscribe(&app, request).await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_eq!(
        resumed["id"], id,
        "an MCP-resolved re-registration must resume this exact paused mailbox"
    );
    assert_eq!(resumed["active"], true);
    assert!(resumed["error"].is_null());
    assert_eq!(
        db.event_subscription(&id).unwrap().unwrap().cursor,
        checkpoint,
        "resuming a paused mailbox must not reset its durable cursor"
    );
}

#[tokio::test]
async fn an_explicit_timing_override_still_conflicts_with_a_differently_configured_active_subscription(
) {
    // The other side of the omission fix: a genuinely different explicit
    // setting must still be refused as a scope/setting change, not silently
    // accepted as a retry.
    let state = test_state_with_seed("overrides-mcp-conflict", "Overrides", seed_orchestration);
    start_run(
        &Db::open(&state.config().db_path).unwrap(),
        "manager",
        "child-c",
        "in progress",
    );
    let app = router(state.clone());
    let (status, initial) = subscribe(&app, resolved_minimal_subscribe("child-c", "poll")).await;
    assert_eq!(status, StatusCode::OK, "{initial}");
    let with_override = kanna_tool_catalog::resolve_request(
        &kanna_tool_catalog::bundled_catalog(),
        "kanna_subscribe_events",
        &json!({"task_id": "child-c", "local_only": true, "delivery": "poll",
            "min_admission_interval_ms": 2_000}),
    )
    .expect("explicit subscribe request resolves")
    .body;
    let (status, conflicted) = subscribe(&app, with_override).await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflicted}");
}
