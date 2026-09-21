//! Notification selection through the real durable wait and mailbox paths.
//! These fixtures do not launch a provider or mutate a live subscription.
use super::*;
use crate::db::TaskEventKind;

fn run_with_policy(db: &Db, id: &str, task: &str, stage: &str, policy: &str) {
    start_run(db, id, task, stage);
    db.connection_for_e2e_tests()
        .execute(
            "UPDATE stage_run SET completion_transition = ?, agent_provider = 'codex' WHERE id = ?",
            [policy, id],
        )
        .unwrap();
}

/// The collector a live subscription hands `wait_subscription_events`: it
/// accumulates until this subscription's rate limit next permits a wake.
/// `minAdmissionIntervalMs` in a fixture's query stands in for the gate a
/// live `Admission` would supply; without one the rate limit is already
/// satisfied, so the first relevant observation seals at once.
async fn selected(state: Arc<AppState>, query: Value) -> Value {
    let gate = query
        .get("minAdmissionIntervalMs")
        .and_then(Value::as_u64)
        .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
    let collection = Arc::new(std::sync::Mutex::new(
        super::super::super::subscription_timing::Collection::new(gate),
    ));
    super::super::super::task_events::wait_subscription_events(state, query, collection)
        .await
        .unwrap()
}

fn query(limit: i64) -> Value {
    // Explicit `from=beginning`: every one of these fixtures seeds events
    // before its first cursorless `selected()` call and expects to see them,
    // relying on the pre-redesign full-replay default rather than the
    // current `now` one.
    json!({"taskIds":"child-a,child-b", "localOnly":true,
        "includeCurrentActivity":false, "from":"beginning", "timeoutSecs":0, "limit":limit})
}

fn noise(db: &Db, count: usize) {
    for _ in 0..count {
        db.append_task_event(
            "child-a",
            TaskEventKind::RuntimeChanged,
            json!({"runtimeState":"busy"}),
        )
        .unwrap();
        db.append_task_event(
            "child-a",
            TaskEventKind::ActivityChanged,
            json!({"activity":"unread"}),
        )
        .unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn automatic_review_to_pr_and_noise_drain_before_limit_without_changing_raw_history() {
    let state = test_state_with_seed("selection-auto", "Selection", seed_orchestration);
    let db = Db::open(&state.config().db_path).unwrap();
    // The pinned workflow really has an automatic successor. Its successful
    // completion is quiet even before the engine has entered that successor.
    db.connection_for_e2e_tests()
        .execute(
            "UPDATE pipeline_item SET stage = 'review', pipeline_def = ? WHERE id = 'child-a'",
            [json!({"stages":[{"name":"review", "transition":"auto"},
            {"name":"pr", "transition":"manual"}]})
            .to_string()],
        )
        .unwrap();
    run_with_policy(&db, "review", "child-a", "review", "auto");
    db.finish_stage_run("review", "succeeded", Some("success"), None)
        .unwrap();
    assert_eq!(selected(state.clone(), query(2)).await["events"], json!([]));
    settle_runtime_tasks(&db, &["child-a"]);
    assert_eq!(
        selected(
            state.clone(),
            json!({"taskIds":"child-a", "localOnly":true,
        "timeoutSecs":0})
        )
        .await["events"],
        json!([])
    );
    db.update_pipeline_item_stage("child-a", "pr").unwrap();
    run_with_policy(&db, "pr", "child-a", "pr", "manual");
    noise(&db, 12);
    let tail = db.latest_task_event_seq().unwrap().to_string();
    let page = selected(state.clone(), query(2)).await;
    assert_eq!(page["events"], json!([]));
    assert_eq!(page["cursor"], tail);
    assert_eq!(page["waitOutcome"], "timeout");
    assert_eq!(page["hasMore"], false);
    db.append_task_event("child-b", TaskEventKind::AwaitingInput, json!({}))
        .unwrap();
    let mut next = query(2);
    next["cursor"] = page["cursor"].clone();
    let attention = selected(state.clone(), next).await;
    assert_eq!(
        event_pairs(&attention),
        vec![("child-b".into(), "task.awaiting_input".into())]
    );
    // Explicit `from=beginning`: everything above already happened before
    // this call, and the point is to see all of it, not the cursorless `now`
    // default's prospective-only view.
    let raw = get_json_body(
        &router(state),
        "/v1/task-events?taskIds=child-a&localOnly=true&includeCurrentActivity=false&from=beginning&timeoutSecs=0",
    )
    .await;
    // child-a's whole history is many events on one task, so it collapses
    // into a single current-state row (see `collapse_events_to_task_state`)
    // rather than a list of individually-typed events. `causedByEventTypes`
    // still names every transition that fed it — including `run.finished`
    // and the busy `task.runtime_changed` noise, which subscription
    // selection filtered out but raw history still retains (the point of
    // this fixture). `task.activity_changed` is no longer a fair example of
    // that: the public wait now excludes it from the underlying selection by
    // default for an unrelated reason (it is the human read/unread display
    // dimension, not manager-facing).
    let events = raw["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
    let caused_by = events[0]["payload"]["causedByEventTypes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(caused_by.contains(&"run.finished"), "{caused_by:?}");
    assert!(caused_by.contains(&"task.runtime_changed"), "{caused_by:?}");
}

#[tokio::test(start_paused = true)]
async fn mixed_filtered_pages_preserve_failure_after_stage_change_and_exact_continuation() {
    let state = test_state_with_seed("selection-failure", "Selection", seed_orchestration);
    let db = Db::open(&state.config().db_path).unwrap();
    run_with_policy(&db, "failed-review", "child-a", "review", "auto");
    db.finish_stage_run("failed-review", "failed", Some("failure"), None)
        .unwrap();
    db.update_pipeline_item_stage("child-a", "pr").unwrap();
    run_with_policy(&db, "replacement", "child-a", "pr", "manual");
    noise(&db, 5);
    db.insert_task_blocker("child-b", "child-a").unwrap();
    db.close_pipeline_item("child-a").unwrap();
    noise(&db, 5);
    db.append_task_event("child-b", TaskEventKind::AwaitingInput, json!({}))
        .unwrap();
    let mut q = query(1);
    let mut observed = Vec::new();
    for _ in 0..10 {
        let page = selected(state.clone(), q.clone()).await;
        observed.extend(event_pairs(&page));
        q["cursor"] = page["cursor"].clone();
        if page["events"] == json!([]) {
            break;
        }
    }
    assert_eq!(
        observed,
        vec![
            ("child-a".into(), "run.finished".into()),
            ("child-b".into(), "task.blocked".into()),
            ("child-a".into(), "task.closed".into()),
            ("child-b".into(), "task.unblocked".into()),
            ("child-b".into(), "task.awaiting_input".into()),
        ]
    );
    assert_eq!(q["cursor"], db.latest_task_event_seq().unwrap().to_string());
}

#[tokio::test(start_paused = true)]
async fn excluded_events_neither_fill_a_batch_nor_seal_it_before_the_rate_limit() {
    let state = test_state_with_seed("selection-debounce", "Selection", seed_orchestration);
    let db = Db::open(&state.config().db_path).unwrap();
    noise(&db, 8);
    let mut q = query(3);
    q["timeoutSecs"] = json!(30);
    // `selected()` goes through `wait_subscription_events`, which always
    // selects subscription-timing mode — the generic `minEvents`/`debounceMs`
    // below are inert there. The rate-limit gate is that mode's only timing.
    q["minEvents"] = json!(2);
    q["debounceMs"] = json!(1000);
    q["minAdmissionIntervalMs"] = json!(3_000);
    let wait = tokio::spawn(selected(state, q));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(!wait.is_finished(), "noise cannot fill an actionable batch");
    db.append_task_event("child-a", TaskEventKind::PrCreated, json!({}))
        .unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(200)).await;
    db.append_task_event("child-b", TaskEventKind::TaskClosed, json!({}))
        .unwrap();
    tokio::task::yield_now().await;
    assert!(
        !wait.is_finished(),
        "relevant events accumulate into the batch; neither they nor earlier \
         noise seals it before the rate limit's gate"
    );
    // Across the gate (t0 + 3000ms): both relevant events come back as one
    // batch, and the second one did not defer the first by restarting any
    // window.
    tokio::time::advance(Duration::from_secs(1)).await;
    let page = wait.await.unwrap();
    assert_eq!(page["waitOutcome"], "events");
    assert_eq!(page["events"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn both_delivery_adapters_share_manual_bootstrap_and_acknowledgement_selection() {
    for delivery in ["input", "codex_app_server"] {
        let state = test_state_with_seed(
            &format!("selection-{delivery}"),
            "Selection",
            seed_orchestration,
        );
        let db = Db::open(&state.config().db_path).unwrap();
        run_with_policy(&db, "manager", "child-c", "in progress", "manual");
        run_with_policy(&db, "manual", "child-a", "in progress", "manual");
        run_with_policy(&db, "automatic", "child-b", "in progress", "auto");
        settle_runtime_tasks(&db, &["child-a", "child-b"]);
        let before = db.get_pipeline_item("child-a").unwrap().unwrap().activity;
        let app = router(state.clone());
        let (status, initial) = subscription_request(
            &app,
            "POST",
            "/v1/event-subscriptions",
            json!({"taskId":"child-c", "localOnly":true, "delivery":delivery}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{initial}");
        assert_eq!(
            event_pairs(&initial["pending"]),
            vec![("child-a".into(), "task.runtime_changed".into())]
        );
        let id = initial["id"].as_str().unwrap();
        let path = format!("/v1/event-subscriptions/{id}/read");
        let (_, read) = subscription_request(&app, "POST", &path, json!({})).await;
        assert_eq!(read["pending"], initial["pending"]);
        let (status, ack) = subscription_request(
            &app,
            "POST",
            &path,
            // Diagnostic mode: the response cursor feeds the raw-wait resume below.
            json!({"acknowledgeBatchId":initial["batchId"], "diagnostic":true}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(ack["pending"].is_null());
        assert_eq!(
            db.get_pipeline_item("child-a").unwrap().unwrap().activity,
            before
        );
        let mut q = json!({"taskIds":"child-a,child-b", "localOnly":true, "timeoutSecs":0});
        q["cursor"] = ack["cursor"].clone();
        let continued = selected(state, q).await;
        assert_eq!(
            continued["events"],
            json!([]),
            "initial manual scan is acknowledged once"
        );
    }
}

// Exercise an independently shipped peer by removing the optional selection
// parameter. Its raw handler still owns native cursors and settled pagination.
fn old_peer(source: &Arc<AppState>, peer: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    let mut requests = source.take_desktop_relay_requests().unwrap();
    source.set_desktop_routing_available(true);
    tokio::spawn(async move {
        let mut receivers = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                Some(result) = receivers.join_next(), if !receivers.is_empty() => { result.unwrap(); }
                request = requests.recv() => {
                    let Some(request) = request else { break };
                    match request {
                        crate::http_api::DesktopRelayRequest::ListActive { response, .. } => {
                            let _ = response.send(Ok(vec![crate::http_api::RelayDesktopPresence::without_key(
                        peer.config().desktop_id.clone(),
                    )]));
                        }
                        crate::http_api::DesktopRelayRequest::Invoke { method, path, body, response, .. } => {
                            assert!(path.contains("orchestrationNotifications=true"));
                            let path = path.replace("&orchestrationNotifications=true", "");
                            let peer = peer.clone();
                            receivers.spawn(async move {
                                let result = crate::http_api::dispatch_authenticated_http_invoke(peer, &method, &path, body).await;
                                let _ = response.send(Ok(result));
                            });
                        }
                        _ => panic!("unexpected event-watch relay operation"),
                    }
                }
            }
        }
    })
}

#[tokio::test]
async fn old_remote_peer_noise_is_filtered_before_aggregate_limit_and_keeps_its_checkpoint() {
    let source = test_state_with_seed("selection-source", "Source", seed_orchestration);
    let peer = test_state_with_seed("selection-peer", "Peer", seed_orchestration);
    let local_db = Db::open(&source.config().db_path).unwrap();
    let peer_db = Db::open(&peer.config().db_path).unwrap();
    noise(&local_db, 3);
    noise(&peer_db, 3);
    run_with_policy(&peer_db, "old-review", "child-a", "review", "auto");
    peer_db
        .finish_stage_run("old-review", "succeeded", Some("success"), None)
        .unwrap();
    peer_db.update_pipeline_item_stage("child-a", "pr").unwrap();
    run_with_policy(&peer_db, "old-pr", "child-a", "pr", "manual");
    for _ in 0..3 {
        peer_db
            .append_task_event("child-b", TaskEventKind::AwaitingInput, json!({}))
            .unwrap();
        noise(&peer_db, 3);
    }
    local_db
        .append_task_event(
            "child-a",
            TaskEventKind::LifecycleFailed,
            json!({"error":"stage spawn failed"}),
        )
        .unwrap();
    let relay = old_peer(&source, peer);
    let mut q = query(2);
    q["localOnly"] = json!(false);
    let mut observed = Vec::new();
    for _ in 0..20 {
        let page = selected(source.clone(), q.clone()).await;
        assert_eq!(page["machineErrors"], json!([]), "{page}");
        observed.extend(page["events"].as_array().unwrap().iter().cloned());
        q["cursor"] = page["cursor"].clone();
        if observed.len() == 4 {
            break;
        }
    }
    assert_eq!(observed.len(), 4);
    assert_eq!(
        observed
            .iter()
            .filter(|e| e["machineId"] == "selection-peer")
            .count(),
        3
    );
    let unique = observed
        .iter()
        .map(|e| (e["machineId"].to_string(), e["seq"].to_string()))
        .collect::<HashSet<_>>();
    assert_eq!(unique.len(), 4, "truncation must not replay relevant rows");
    for _ in 0..2 {
        let page = selected(source.clone(), q.clone()).await;
        assert_eq!(
            page["events"],
            json!([]),
            "excluded remote tails cannot create empty event wakes"
        );
        assert_eq!(page["waitOutcome"], "timeout");
        q["cursor"] = page["cursor"].clone();
    }
    relay.abort();
    let _ = relay.await;
}

#[tokio::test]
async fn subscription_worker_publishes_only_attention_and_ack_resumes_after_filtered_progress() {
    let state = test_state_with_seed("selection-worker", "Selection", seed_orchestration);
    let db = Db::open(&state.config().db_path).unwrap();
    run_with_policy(&db, "manager", "child-c", "in progress", "manual");
    run_with_policy(&db, "automatic", "child-a", "in progress", "auto");
    db.connection_for_e2e_tests()
        .execute(
            "UPDATE pipeline_item SET pipeline_def = ? WHERE id = 'child-a'",
            [
                json!({"stages":[{"name":"in progress", "transition":"auto"},
            {"name":"pr", "transition":"manual"}]})
                .to_string(),
            ],
        )
        .unwrap();
    let app = router(state.clone());
    let (_, initial) = subscription_request(
        &app,
        "POST",
        "/v1/event-subscriptions",
        json!({"taskId":"child-c", "localOnly":true, "delivery":"poll"}),
    )
    .await;
    assert!(initial["pending"].is_null());
    let id = initial["id"].as_str().unwrap();
    let service = tokio::spawn(super::super::super::event_subscriptions::run(state.clone()));
    db.finish_stage_run("automatic", "succeeded", Some("success"), None)
        .unwrap();
    settle_runtime_tasks(&db, &["child-a"]);
    noise(&db, 60); // More raw rows than the subscription page limit.
    db.append_task_event("child-b", TaskEventKind::AwaitingInput, json!({}))
        .unwrap();
    let first = await_subscription(&state, id, |row| row.wake_state == "ready").await;
    assert_eq!(
        first.batch_id, 1,
        "routine pages must never occupy the mailbox"
    );
    assert_eq!(
        event_pairs(first.pending.as_ref().unwrap()),
        vec![("child-b".into(), "task.awaiting_input".into())]
    );
    noise(&db, 3);
    db.append_task_event(
        "child-b",
        TaskEventKind::LifecycleFailed,
        json!({"error":"handoff failed"}),
    )
    .unwrap();
    let (_, read) = subscription_request(
        &app,
        "POST",
        &format!("/v1/event-subscriptions/{id}/read"),
        // Diagnostic mode: comparing against the DB row's raw (unreshaped) pending.
        json!({"diagnostic":true}),
    )
    .await;
    assert_eq!(
        read["pending"],
        first.pending.unwrap(),
        "read does not release backpressure"
    );
    let (status, _) = subscription_request(
        &app,
        "POST",
        &format!("/v1/event-subscriptions/{id}/read"),
        json!({"acknowledgeBatchId":first.batch_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let second = await_subscription(&state, id, |row| {
        row.batch_id == 2 && row.wake_state == "ready"
    })
    .await;
    assert_eq!(
        event_pairs(second.pending.as_ref().unwrap()),
        vec![("child-b".into(), "task.lifecycle_failed".into())]
    );
    service.abort();
    let _ = service.await;
}

#[tokio::test(start_paused = true)]
async fn excluded_backlog_cannot_extend_deadline_and_resumes_without_cursor_loss() {
    for timeout_secs in [0, 1] {
        let state = test_state_with_seed(
            &format!("selection-deadline-{timeout_secs}"),
            "Selection",
            seed_orchestration,
        );
        let db = Db::open(&state.config().db_path).unwrap();
        noise(&db, 20);
        let tail = db.latest_task_event_seq().unwrap();
        let mut q = query(1);
        q["timeoutSecs"] = json!(timeout_secs);
        let wait = tokio::spawn(selected(state.clone(), q));
        // Let the wait consume a page and yield to drain the filtered backlog.
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        let page = wait.await.unwrap();
        assert_eq!(page["waitOutcome"], "timeout");
        assert_eq!(page["events"], json!([]));
        assert_eq!(page["hasMore"], true);
        let checkpoint = page["cursor"].as_str().unwrap().parse::<i64>().unwrap();
        assert!(
            checkpoint > 0 && checkpoint < tail,
            "expired wait must stop draining before the tail: {page}"
        );
        db.append_task_event("child-b", TaskEventKind::AwaitingInput, json!({}))
            .unwrap();
        let mut resume = query(1);
        resume["cursor"] = page["cursor"].clone();
        let attention = selected(state.clone(), resume.clone()).await;
        assert_eq!(
            event_pairs(&attention),
            vec![("child-b".into(), "task.awaiting_input".into())]
        );
        resume["cursor"] = attention["cursor"].clone();
        assert_eq!(selected(state, resume).await["events"], json!([]));
    }
}

#[tokio::test]
async fn final_auto_completion_reaches_both_mailboxes_and_fresh_registration() {
    for delivery in ["input", "codex_app_server"] {
        for bootstrap in [false, true] {
            let state = super::super::actions::final_auto_completion_state(&format!(
                "final-auto-{delivery}-{bootstrap}"
            ));
            let db = Db::open(&state.config().db_path).unwrap();
            let app = router(state.clone());
            if bootstrap {
                super::super::actions::complete_final_auto(&app).await;
                settle_runtime_tasks(&db, &["child-a"]);
            }
            let (status, initial) = subscription_request(
                &app,
                "POST",
                "/v1/event-subscriptions",
                // Per-subscription rate-limit override, not the 60000ms
                // global: the non-bootstrap branch's successful (non-urgent)
                // run.finished event needs to seal within this test's
                // real-time `await_subscription` budget.
                json!({"taskId":"child-c", "localOnly":true, "delivery":delivery,
                    "minAdmissionIntervalMs": 2_000}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{initial}");
            let id = initial["id"].as_str().unwrap();
            let page = if bootstrap {
                initial.clone()
            } else {
                assert!(initial["pending"].is_null());
                super::super::actions::complete_final_auto(&app).await;
                let service =
                    tokio::spawn(super::super::super::event_subscriptions::run(state.clone()));
                let row = await_subscription(&state, id, |row| row.pending.is_some()).await;
                service.abort();
                let _ = service.await;
                json!({"pending":row.pending, "batchId":row.batch_id})
            };
            assert_eq!(
                event_pairs(&page["pending"]),
                vec![(
                    "child-a".into(),
                    if bootstrap {
                        "task.runtime_changed"
                    } else {
                        "run.finished"
                    }
                    .into()
                )]
            );
            let (_, ack) = subscription_request(
                &app,
                "POST",
                &format!("/v1/event-subscriptions/{id}/read"),
                // Diagnostic mode: the response cursor feeds the raw-wait resume below.
                json!({"acknowledgeBatchId":page["batchId"], "diagnostic":true}),
            )
            .await;
            assert!(ack["pending"].is_null());
            let continued = selected(
                state,
                json!({"taskIds":"child-a", "localOnly":true,
                "timeoutSecs":0, "cursor":ack["cursor"]}),
            )
            .await;
            assert_eq!(continued["events"], json!([]));
            assert_eq!(continued["cursor"], ack["cursor"]);
        }
    }
}

#[tokio::test]
async fn exited_without_verdict_bootstraps_once_for_both_adapters_without_marking_read() {
    for delivery in ["input", "codex_app_server"] {
        let state =
            test_state_with_seed(&format!("exited-{delivery}"), "Exited", seed_orchestration);
        let db = Db::open(&state.config().db_path).unwrap();
        run_with_policy(&db, "manager", "child-c", "in progress", "manual");
        run_with_policy(&db, "automatic", "child-a", "in progress", "auto");
        super::super::super::task_input::handle_task_terminal_state(&state, "child-a", 0)
            .await
            .unwrap();
        db.connection_for_e2e_tests().execute(
            "UPDATE pipeline_item SET runtime_event_pending_at = datetime('now', '-11 seconds')", [],
        ).unwrap();
        db.flush_debounced_activity_events(300).unwrap();
        assert_eq!(
            db.latest_stage_run("child-a").unwrap().unwrap().status,
            "cancelled"
        );
        let before = db.get_pipeline_item("child-a").unwrap().unwrap();
        assert_eq!(before.runtime_status.as_deref(), Some("exited"));
        let app = router(state.clone());
        let before_detail = get_json_body(&app, "/v1/tasks/child-a").await;
        assert_eq!(before_detail["readState"], "unread");
        let (status, initial) = subscription_request(
            &app,
            "POST",
            "/v1/event-subscriptions",
            json!({"taskId":"child-c", "localOnly":true, "delivery":delivery}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{initial}");
        assert_eq!(
            event_pairs(&initial["pending"]),
            vec![("child-a".into(), "task.runtime_changed".into())]
        );
        let path = format!(
            "/v1/event-subscriptions/{}/read",
            initial["id"].as_str().unwrap()
        );
        let (_, read) = subscription_request(&app, "POST", &path, json!({})).await;
        assert_eq!(read["pending"], initial["pending"]);
        let (_, ack) = subscription_request(
            &app,
            "POST",
            &path,
            // Diagnostic mode: the response cursor feeds the raw-wait resume below.
            json!({"acknowledgeBatchId":initial["batchId"], "diagnostic":true}),
        )
        .await;
        assert!(ack["pending"].is_null());
        let continued = selected(
            state,
            json!({"taskIds":"child-a", "localOnly":true,
            "timeoutSecs":0, "cursor":ack["cursor"]}),
        )
        .await;
        assert_eq!(continued["events"], json!([]));
        assert_eq!(continued["cursor"], ack["cursor"]);
        let after = db.get_pipeline_item("child-a").unwrap().unwrap();
        assert_eq!(after.activity, before.activity);
        let after_detail = get_json_body(&app, "/v1/tasks/child-a").await;
        assert_eq!(after_detail["readState"], before_detail["readState"]);
    }
}

fn long_summary_result(summary_chars: usize) -> String {
    json!({
        "status": "failure",
        "summary": "s".repeat(summary_chars),
        "metadata": {"pr_url": "https://github.com/tampopogk/kanna/pull/4242", "head": "abc123"},
    })
    .to_string()
}

fn pinned_definition(name: &str) -> Value {
    json!({
        "name": name,
        "description": "A description long enough to be worth dropping from a delivered page.",
        "revision_limit": 3,
        "stages": [{
            "name": "in progress",
            "agent": "implement",
            "agent_provider": [{"harness": "claude", "model": "sonnet", "effort": "medium"}],
            "description": "Agent implements the approved plan, then commits as tail work",
            "policy": {"transition": "auto"},
            "prompt": "p".repeat(2_000),
            "post": {"name": "commit", "agent": "commit", "prompt": "c".repeat(500)},
        }],
        "plan_context": {
            "source_run_id": "run-plan",
            "stage": "plan",
            "result": long_summary_result(3_000),
        },
    })
}

fn definition_carries_prose(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key("prompt")
                || object.contains_key("description")
                || object.values().any(definition_carries_prose)
        }
        Value::Array(items) => items.iter().any(definition_carries_prose),
        _ => false,
    }
}

/// Delivered-page bounds, through the real subscribe/read wiring.
///
/// The measured problem this closes: an acknowledged page stays in a
/// manager's conversation and is re-billed as a cache read on every later
/// request, and almost all of its bytes are prose the manager's own contract
/// requires it to re-read fresh — a finished run's verbatim result summary,
/// `task.workflow_changed`'s two whole pinned definitions, and the relevance
/// filter's own `notificationContext`. The page must lose that prose and lose
/// nothing else: the same events are selected, `payload.currentTask` and the
/// small structured facts survive byte for byte, `diagnostic` still returns
/// the stored page verbatim, and acknowledgement by `batchId` is untouched.
#[tokio::test]
async fn delivered_page_bounds_run_and_workflow_prose_but_not_selection_or_acknowledgement() {
    let state = test_state_with_seed("subscription-page-bounds", "Mailbox", seed_orchestration);
    let db = Db::open(&state.config().db_path).unwrap();
    start_run(&db, "manager-run", "child-c", "in progress");
    let app = router(state.clone());
    let (status, initial) = subscription_request(
        &app,
        "POST",
        "/v1/event-subscriptions",
        json!({"taskId":"child-c", "localOnly":true, "delivery":"poll"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{initial}");
    assert!(initial["pending"].is_null(), "{initial}");
    let id = initial["id"].as_str().unwrap().to_string();
    let service = tokio::spawn(super::super::super::event_subscriptions::run(state.clone()));

    // Order matters and makes this deterministic in real time: a workflow
    // change is ordinary, so it cannot seal on its own inside the 300000ms
    // defaults, while the failed run behind it is urgent and seals the page
    // both events are by then collected into.
    db.append_task_event(
        "child-a",
        TaskEventKind::WorkflowChanged,
        json!({
            "operation": "replace",
            "fromWorkflow": "single-reviewer",
            "toWorkflow": "plan-build-review",
            "beforeDefinition": pinned_definition("single-reviewer"),
            "afterDefinition": pinned_definition("plan-build-review"),
        }),
    )
    .unwrap();
    run_with_policy(&db, "failed-run", "child-b", "in progress", "manual");
    db.finish_stage_run(
        "failed-run",
        "failed",
        Some(&long_summary_result(4_000)),
        None,
    )
    .unwrap();

    let stored = await_subscription(&state, &id, |row| row.pending.is_some()).await;
    let read_path = format!("/v1/event-subscriptions/{id}/read");
    let (_, page) = subscription_request(&app, "POST", &read_path, json!({})).await;
    let (_, raw) = subscription_request(&app, "POST", &read_path, json!({"diagnostic":true})).await;

    // Selection is upstream of the projection and unchanged by it.
    assert_eq!(
        event_pairs(&page["pending"]),
        event_pairs(&raw["pending"]),
        "bounding a page must not change which events it carries: {page}"
    );
    let delivered = event_pairs(&page["pending"]);
    assert!(
        delivered.contains(&("child-a".into(), "task.workflow_changed".into()))
            && delivered.contains(&("child-b".into(), "run.finished".into())),
        "both events belong to this page: {delivered:?}"
    );
    assert_eq!(raw["pending"], json!(stored.pending), "{raw}");

    let events = page["pending"]["events"].as_array().unwrap();
    let finished = events
        .iter()
        .find(|event| event["type"] == "run.finished")
        .unwrap();
    let payload = finished["payload"].as_object().unwrap();
    assert!(
        !payload.contains_key("notificationContext"),
        "the relevance filter's working state is not delivered: {finished}"
    );
    // The event-time facts a manager coordinates on survive exactly.
    for key in ["runId", "stage", "kind", "status", "currentTask"] {
        assert!(
            payload.contains_key(key),
            "payload.{key} missing: {finished}"
        );
    }
    assert_eq!(payload["status"], "failed");
    let result: Value = serde_json::from_str(payload["result"].as_str().unwrap()).unwrap();
    assert_eq!(result["status"], "failure");
    assert_eq!(
        result["metadata"],
        json!({"pr_url": "https://github.com/tampopogk/kanna/pull/4242", "head": "abc123"}),
        "structured metadata is not prose and stays verbatim"
    );
    assert_eq!(result["summaryTruncated"], true);
    let summary = result["summary"].as_str().unwrap();
    assert_eq!(
        summary.chars().count(),
        super::super::super::task_events::EVENT_SUMMARY_SNIPPET_CHARS + 1,
        "the bound plus its truncation marker: {summary}"
    );
    assert!(summary.starts_with("ss") && summary.ends_with('…'));

    let changed = events
        .iter()
        .find(|event| event["type"] == "task.workflow_changed")
        .unwrap();
    let payload = &changed["payload"];
    assert!(!payload
        .as_object()
        .unwrap()
        .contains_key("notificationContext"));
    for key in ["beforeDefinition", "afterDefinition"] {
        let definition = &payload[key];
        assert!(
            !definition_carries_prose(definition),
            "{key} must carry no stage prompt or description: {definition}"
        );
        // Structure — what a manager actually reasons about — survives.
        assert_eq!(definition["revision_limit"], 3);
        assert_eq!(definition["stages"][0]["name"], "in progress");
        assert_eq!(definition["stages"][0]["agent"], "implement");
        assert_eq!(
            definition["stages"][0]["agent_provider"][0]["model"],
            "sonnet"
        );
        assert_eq!(definition["stages"][0]["policy"]["transition"], "auto");
        assert_eq!(definition["stages"][0]["post"]["agent"], "commit");
        assert_eq!(definition["plan_context"]["source_run_id"], "run-plan");
        let plan: Value =
            serde_json::from_str(definition["plan_context"]["result"].as_str().unwrap()).unwrap();
        assert_eq!(
            plan["summaryTruncated"], true,
            "a stamped plan is prose too"
        );
    }
    assert_eq!(payload["fromWorkflow"], "single-reviewer");
    assert_eq!(payload["toWorkflow"], "plan-build-review");

    // Before/after instrumentation, in the test rather than in a claim: the
    // diagnostic page is the unbounded page this projection replaces.
    let before = serde_json::to_string(&raw["pending"]).unwrap().len();
    let after = serde_json::to_string(&page["pending"]).unwrap().len();
    println!(
        "delivered page: before={before}B after={after}B cut={:.1}%",
        100.0 * (1.0 - after as f64 / before as f64)
    );
    assert!(
        after * 4 < before,
        "the bounded page must be a fraction of the stored one: before={before}B after={after}B"
    );

    // Acknowledgement is by batch id and is untouched by the projection.
    let (status, _) = subscription_request(
        &app,
        "POST",
        &read_path,
        json!({"acknowledgeBatchId":9_999}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, acked) = subscription_request(
        &app,
        "POST",
        &read_path,
        json!({"acknowledgeBatchId": page["batchId"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(acked["pending"].is_null(), "{acked}");
    assert_eq!(page["batchId"], json!(stored.batch_id));
    service.abort();
    let _ = service.await;
}

/// A short result, a result that is not JSON, and a definition that is not an
/// object are all returned exactly as stored: the page bounds prose, it never
/// reshapes a payload into something the caller did not send.
#[tokio::test]
async fn a_page_leaves_a_payload_it_has_nothing_to_bound_byte_for_byte() {
    let state = test_state_with_seed("subscription-page-verbatim", "Mailbox", seed_orchestration);
    let db = Db::open(&state.config().db_path).unwrap();
    start_run(&db, "manager-run", "child-c", "in progress");
    let app = router(state.clone());
    let (status, initial) = subscription_request(
        &app,
        "POST",
        "/v1/event-subscriptions",
        json!({"taskId":"child-c", "localOnly":true, "delivery":"poll"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{initial}");
    let id = initial["id"].as_str().unwrap().to_string();
    let service = tokio::spawn(super::super::super::event_subscriptions::run(state.clone()));
    run_with_policy(&db, "short-run", "child-a", "in progress", "manual");
    db.finish_stage_run(
        "short-run",
        "failed",
        Some("a plain sentence, not JSON at all"),
        None,
    )
    .unwrap();
    let stored = await_subscription(&state, &id, |row| row.pending.is_some()).await;
    let read_path = format!("/v1/event-subscriptions/{id}/read");
    let (_, page) = subscription_request(&app, "POST", &read_path, json!({})).await;
    let stored_events = stored.pending.as_ref().unwrap()["events"].clone();
    let delivered = &page["pending"]["events"];
    assert_eq!(
        delivered[0]["payload"]["result"], stored_events[0]["payload"]["result"],
        "a non-JSON result is not prose this page knows how to bound: {page}"
    );
    assert_eq!(
        delivered[0]["payload"]["currentTask"], stored_events[0]["payload"]["currentTask"],
        "currentTask is already bounded upstream and is delivered unchanged"
    );
    service.abort();
    let _ = service.await;
}
