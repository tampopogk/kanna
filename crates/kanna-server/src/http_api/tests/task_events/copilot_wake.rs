use super::*;
use crate::db::copilot_wake::Attempt;

struct Stream {
    response: reqwest::Response,
    buffer: String,
}
impl Stream {
    async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(end) = self.buffer.find("\n\n") {
                    let block = self.buffer[..end].to_owned();
                    self.buffer.drain(..end + 2);
                    if let Some(data) = block.lines().find_map(|line| line.strip_prefix("data: ")) {
                        return serde_json::from_str(data).unwrap();
                    }
                } else {
                    let chunk = self.response.chunk().await.unwrap().expect("stream ended");
                    self.buffer.push_str(std::str::from_utf8(&chunk).unwrap());
                }
            }
        })
        .await
        .expect("wake stream timeout")
    }
}
struct Fixture {
    state: Arc<AppState>,
    app: Router,
    db: Db,
    id: String,
    url: String,
    http: tokio::task::JoinHandle<()>,
    service: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.http.abort();
        self.service.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let state = test_state_with_seed("copilot-wake", "Copilot wake", seed_orchestration);
        let db = Db::open(&state.config().db_path).unwrap();
        start_run(&db, "manager-run", "child-c", "in progress");
        db.connection_for_e2e_tests().execute("UPDATE stage_run SET agent_provider='copilot', provider_session_id='native-session' WHERE id='manager-run'", []).unwrap();
        db.update_pipeline_item_runtime_status("child-a", "busy", None)
            .unwrap();
        let app = router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/tasks/child-c/copilot-wake",
            listener.local_addr().unwrap()
        );
        let http_app = app.clone();
        let http = tokio::spawn(async move {
            axum::serve(
                listener,
                http_app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let (status, row) = subscription_request(&app,"POST","/v1/event-subscriptions", json!({
            "taskId":"child-c", "taskIds":["child-a"], "localOnly":true, "minAdmissionIntervalMs":1000, "diagnostic":true,
        })).await;
        assert_eq!(status, StatusCode::OK, "{row}");
        assert_eq!(row["delivery"], "copilot_extension");
        assert!(row["pending"].is_null(), "{row}");
        let id = row["id"].as_str().unwrap().into();
        let service = tokio::spawn(super::super::super::event_subscriptions::run(state.clone()));
        Self {
            state,
            app,
            db,
            id,
            url,
            http,
            service,
        }
    }
    fn emit(&self) {
        self.db
            .append_task_event(
                "child-a",
                crate::db::TaskEventKind::AwaitingInput,
                json!({}),
            )
            .unwrap();
        self.state
            .publish_state_changed(kanna_agent_protocol::StateChangeScope::Tasks);
    }
    async fn stream(&self) -> Stream {
        let response = reqwest::get(format!(
            "{}?runId=manager-run&sessionId=native-session",
            self.url
        ))
        .await
        .unwrap();
        if response.status() != StatusCode::OK {
            panic!("registration: {}", response.text().await.unwrap());
        }
        let mut stream = Stream {
            response,
            buffer: String::new(),
        };
        let frame = stream.next().await;
        assert_eq!(frame["type"], "registered");
        assert_eq!(frame["binding"]["sessionId"], "native-session");
        stream
    }
    async fn receipt(&self, attempt: &Attempt, extra: Value) -> StatusCode {
        let mut body = json!({"runId":attempt.binding.run_id,"sessionId":attempt.binding.session_id,
            "connectionId":attempt.binding.connection_id,"attemptId":attempt.id});
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        reqwest::Client::new()
            .post(format!("{}/receipt", self.url))
            .json(&body)
            .send()
            .await
            .unwrap()
            .status()
    }
    async fn read(&self, ack: Option<i64>) -> Value {
        let body = ack
            .map(|batch| json!({"acknowledgeBatchId":batch}))
            .unwrap_or(json!({}));
        let (status, row) = subscription_request(
            &self.app,
            "POST",
            &format!("/v1/event-subscriptions/{}/read", self.id),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{row}");
        row
    }
    async fn attempt(&self, stream: &mut Stream, operation: &str) -> Attempt {
        let frame = stream.next().await;
        assert_eq!(frame["type"], operation, "{frame}");
        serde_json::from_value(frame["attempt"].clone()).unwrap()
    }
}

#[tokio::test]
async fn copilot_wake_default_and_legacy_input_never_use_composer_and_ack_is_separate() {
    let f = Fixture::new().await;
    // An old persisted `input` subscription takes the same native-only path.
    let mut row = f.db.event_subscription(&f.id).unwrap().unwrap();
    row.delivery = "input".into();
    f.db.save_event_subscription(&mut row).unwrap();
    f.emit();
    let pending = await_subscription(&f.state, &f.id, |r| {
        r.wake_state == "pending" && r.error.is_some()
    })
    .await;
    assert!(pending.error.unwrap().contains("extension unavailable"));
    assert_eq!(f.db.count_task_inputs("child-c").unwrap(), 0);
    assert!(f
        .state
        .try_begin_requested_task_mutation("child-c")
        .is_some());
    let mut stream = f.stream().await;
    let attempt = f.attempt(&mut stream, "send").await;
    assert_eq!(attempt.batch_id, pending.batch_id);
    assert!(
        f.state
            .try_begin_requested_task_mutation("child-c")
            .is_some(),
        "receipt wait must not lock ordinary input"
    );
    assert_eq!(
        f.receipt(
            &attempt,
            json!({"kind":"accepted","messageId":"queue-1","source":"operator"})
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        f.receipt(&attempt, json!({"kind":"accepted","messageId":""}))
            .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(f.db.count_task_inputs("child-c").unwrap(), 0);
    for _ in 0..2 {
        assert_eq!(
            f.receipt(&attempt, json!({"kind":"accepted","messageId":"queue-1"}))
                .await,
            StatusCode::OK
        );
    }
    await_subscription(&f.state, &f.id, |r| r.wake_state == "notified").await;
    let inputs = f.db.list_task_inputs("child-c", 10).unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].source, "engine");
    assert_eq!(inputs[0].run_id.as_deref(), Some("manager-run"));
    assert_eq!(inputs[0].message, attempt.message);
    f.emit();
    assert_eq!(f.read(None).await["batchId"], attempt.batch_id);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err()
    );
    f.read(Some(attempt.batch_id)).await;
    let next = f.attempt(&mut stream, "send").await;
    assert_eq!(next.batch_id, attempt.batch_id + 1);
    assert_eq!(
        f.db.count_task_inputs("child-c").unwrap(),
        1,
        "queue admission still awaits receipt"
    );
}

#[tokio::test]
async fn copilot_wake_disconnect_reconciles_once_without_resend_or_silent_ack() {
    let f = Fixture::new().await;
    let mut first = f.stream().await;
    f.emit();
    let attempt = f.attempt(&mut first, "send").await;
    drop(first);
    await_subscription(&f.state, &f.id, |r| r.wake_state == "uncertain").await;
    // Reopen DB as a recovery reader; the attempt survives independently of RAM.
    let reopened = Db::open(&f.state.config().db_path).unwrap();
    assert!(reopened
        .copilot_attempt(&attempt.id)
        .unwrap()
        .unwrap()
        .input_id
        .is_none());
    let mut second = f.stream().await;
    let recovered = f.attempt(&mut second, "inspect").await;
    assert_eq!(recovered.id, attempt.id);
    assert_eq!(recovered.message, attempt.message);
    assert_ne!(
        recovered.binding.connection_id,
        attempt.binding.connection_id
    );
    assert_eq!(
        f.receipt(
            &attempt,
            json!({"kind":"accepted","messageId":"late-old-epoch"})
        )
        .await,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.receipt(
            &recovered,
            json!({"kind":"uncertain","error":"not yet in native history"})
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(f.db.count_task_inputs("child-c").unwrap(), 0);
    assert!(f.read(None).await["pending"].is_object());
    assert!(f
        .state
        .try_begin_requested_task_mutation("child-c")
        .is_some());
    assert_eq!(
        f.receipt(
            &recovered,
            json!({"kind":"observed","eventId":"history-1","content":"owner text"})
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    for _ in 0..2 {
        assert_eq!(
            f.receipt(
                &recovered,
                json!({"kind":"observed","eventId":"history-1","content":recovered.message})
            )
            .await,
            StatusCode::OK
        );
    }
    assert_eq!(f.db.count_task_inputs("child-c").unwrap(), 1);
    assert!(f.read(None).await["pending"].is_object());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), second.next())
            .await
            .is_err()
    );
    let saved = reopened.copilot_attempt(&attempt.id).unwrap().unwrap();
    assert_eq!(saved.event_id.as_deref(), Some("history-1"));
    assert_eq!(saved.message_id, None);
}

#[tokio::test]
async fn copilot_wake_late_receipt_keeps_original_run_and_does_not_restore_acked_batch() {
    let f = Fixture::new().await;
    let mut stream = f.stream().await;
    f.emit();
    let attempt = f.attempt(&mut stream, "send").await;
    f.read(Some(attempt.batch_id)).await;
    start_run(&f.db, "replacement", "child-c", "in progress");
    f.state
        .publish_state_changed(kanna_agent_protocol::StateChangeScope::Tasks);
    assert_eq!(
        f.receipt(
            &attempt,
            json!({"kind":"accepted","messageId":"late-native-id"})
        )
        .await,
        StatusCode::OK
    );
    let inputs = f.db.list_task_inputs("child-c", 10).unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].run_id.as_deref(), Some("manager-run"));
    assert!(f.read(None).await["pending"].is_null());
    let response = reqwest::get(format!(
        "{}?runId=manager-run&sessionId=native-session",
        f.url
    ))
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn copilot_wake_receipt_transaction_rolls_back_and_invalid_registration_cannot_deliver() {
    let f = Fixture::new().await;
    {
        let _lease = f
            .state
            .try_begin_requested_task_mutation("child-c")
            .unwrap();
        let response = reqwest::get(format!(
            "{}?runId=manager-run&sessionId=native-session",
            f.url
        ))
        .await
        .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "a transient lease must not permanently retire the extension"
        );
    }
    for query in [
        "runId=manager-run&sessionId=wrong",
        "runId=wrong&sessionId=native-session",
    ] {
        assert_eq!(
            reqwest::get(format!("{}?{query}", f.url))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
    }
    let mut stream = f.stream().await;
    f.emit();
    let attempt = f.attempt(&mut stream, "send").await;
    f.db.connection_for_e2e_tests().execute_batch("CREATE TRIGGER reject_receipt BEFORE UPDATE ON copilot_wake_attempt BEGIN SELECT RAISE(ABORT,'fixture disk fault'); END;").unwrap();
    assert_eq!(
        f.receipt(&attempt, json!({"kind":"accepted","messageId":"queue-1"}))
            .await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(f.db.count_task_inputs("child-c").unwrap(), 0);
    assert!(f
        .db
        .copilot_attempt(&attempt.id)
        .unwrap()
        .unwrap()
        .input_id
        .is_none());
    f.db.connection_for_e2e_tests()
        .execute_batch("DROP TRIGGER reject_receipt")
        .unwrap();
    assert_eq!(
        f.receipt(&attempt, json!({"kind":"accepted","messageId":"queue-1"}))
            .await,
        StatusCode::OK
    );
    assert_eq!(f.db.count_task_inputs("child-c").unwrap(), 1);
}

#[tokio::test]
async fn copilot_wake_server_restart_recovers_persisted_attempt_without_send() {
    let mut f = Fixture::new().await;
    let mut stream = f.stream().await;
    f.emit();
    let attempt = f.attempt(&mut stream, "send").await;
    drop(stream);
    await_subscription(&f.state, &f.id, |r| r.wake_state == "uncertain").await;
    f.service.abort();
    let _ = (&mut f.service).await;
    f.http.abort();
    let _ = (&mut f.http).await;
    // Discard all in-memory registration/observer state, retain only SQLite.
    f.state = Arc::new(AppState::new(f.state.config().clone()));
    assert!(f.state.copilot_wakes.lock().unwrap().is_empty());
    f.app = router(f.state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    f.url = format!(
        "http://{}/v1/tasks/child-c/copilot-wake",
        listener.local_addr().unwrap()
    );
    let app = f.app.clone();
    f.http = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    f.service = tokio::spawn(super::super::super::event_subscriptions::run(
        f.state.clone(),
    ));
    let mut recovered = f.stream().await;
    let inspect = f.attempt(&mut recovered, "inspect").await;
    assert_eq!(inspect.id, attempt.id);
    assert_ne!(inspect.binding.connection_id, attempt.binding.connection_id);
    assert_eq!(
        f.receipt(
            &inspect,
            json!({"kind":"observed","eventId":"native-history","content":inspect.message})
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(f.db.count_task_inputs("child-c").unwrap(), 1);
    assert!(f.read(None).await["pending"].is_object());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), recovered.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn copilot_wake_legacy_subscription_retry_preserves_paused_mailbox() {
    let f = Fixture::new().await;
    f.emit();
    let mut row = await_subscription(&f.state, &f.id, |r| {
        r.pending.is_some() && r.error.is_some()
    })
    .await;
    f.service.abort();
    row.delivery = "input".into();
    row.active = false;
    row.wake_state = "error".into();
    row.error = Some("legacy outcome remains uncertain".into());
    f.db.save_event_subscription(&mut row).unwrap();
    let (status, retried) = subscription_request(&f.app,"POST","/v1/event-subscriptions",json!({
        "taskId":"child-c", "taskIds":["child-a"], "localOnly":true, "minAdmissionIntervalMs":1000, "diagnostic":true,
    })).await;
    assert_eq!(status, StatusCode::OK, "{retried}");
    assert_eq!(retried["id"], f.id);
    assert_eq!(retried["batchId"], row.batch_id);
    assert_eq!(retried["error"], json!(row.error));
    assert_eq!(retried["pending"], row.pending.unwrap());
    assert_eq!(retried["cursor"], json!(row.cursor));
    assert_eq!(f.db.event_subscriptions().unwrap().len(), 1);
    assert_eq!(f.db.count_task_inputs("child-c").unwrap(), 0);
}
