//! The Claude native-channel wake transport, through the real router, the real
//! subscription worker, and a real SSE stream standing in for the task's own
//! `kanna-mcp` child.
//!
//! These pin the two defects the 2026-09-14 live experiment found and the one
//! rule the whole transport exists for: a wake never becomes terminal input,
//! and it never locks ordinary input.
use super::*;
use crate::db::claude_channel::Attempt;

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
        .expect("channel stream timeout")
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
        let state = test_state_with_seed("claude-channel", "Claude channel", seed_orchestration);
        let db = Db::open(&state.config().db_path).unwrap();
        start_run(&db, "manager-run", "child-c", "in progress");
        db.connection_for_e2e_tests()
            .execute(
                "UPDATE stage_run SET agent_provider='claude' WHERE id='manager-run'",
                [],
            )
            .unwrap();
        db.update_pipeline_item_runtime_status("child-a", "busy", None)
            .unwrap();
        let app = router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/tasks/child-c/claude-channel",
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
        let (status, row) = subscription_request(
            &app,
            "POST",
            "/v1/event-subscriptions",
            json!({
                "taskId": "child-c", "taskIds": ["child-a"], "localOnly": true,
                "minAdmissionIntervalMs": 1000, "diagnostic": true,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{row}");
        // A Claude run keeps plain `input`: the transport is resolved per
        // delivery from a measured capability, never pinned at subscribe time.
        assert_eq!(row["delivery"], "input");
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

    fn runtime(&self, status: &str) {
        self.db
            .update_pipeline_item_runtime_status("child-c", status, None)
            .unwrap();
        self.state
            .publish_state_changed(kanna_agent_protocol::StateChangeScope::Tasks);
    }

    async fn stream(&self) -> Stream {
        let response = reqwest::get(format!("{}?runId=manager-run", self.url))
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
        stream
    }

    async fn confirm(&self, channel_id: &str) -> StatusCode {
        reqwest::Client::new()
            .post(format!("{}/confirm", self.url))
            .json(&json!({ "channelId": channel_id }))
            .send()
            .await
            .unwrap()
            .status()
    }

    async fn receipt(&self, attempt: &Attempt, extra: Value) -> StatusCode {
        let mut body = json!({
            "runId": attempt.binding.run_id,
            "channelId": attempt.binding.channel_id,
            "attemptId": attempt.id,
        });
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
            .map(|batch| json!({ "acknowledgeBatchId": batch }))
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

    async fn wake(&self, stream: &mut Stream) -> Attempt {
        let frame = stream.next().await;
        assert_eq!(frame["type"], "wake", "{frame}");
        serde_json::from_value(frame["attempt"].clone()).unwrap()
    }
}

/// An unconfirmed channel is the startup race the experiment found: the probe
/// emitted at initialization can be lost, and nothing else would ever ask
/// again. Every admitted wake therefore re-probes, keeps the batch pending,
/// and — this is the whole point — types nothing anywhere.
#[tokio::test]
async fn an_unconfirmed_channel_reprobes_from_admission_and_never_writes_terminal_input() {
    let f = Fixture::new().await;
    let mut stream = f.stream().await;
    let first = stream.next().await;
    assert_eq!(first["type"], "probe");
    let channel_id = first["channelId"].as_str().unwrap().to_owned();

    f.emit();
    let pending = await_subscription(&f.state, &f.id, |r| {
        r.wake_state == "pending" && r.error.is_some()
    })
    .await;
    assert!(
        pending.error.unwrap().contains("not confirmed"),
        "an unconfirmed channel must say so rather than fall back to the composer"
    );
    // The admission re-probed, with the same channel id, so a confirmation
    // that arrives late still names something current.
    let reprobe = stream.next().await;
    assert_eq!(reprobe["type"], "probe");
    assert_eq!(reprobe["channelId"], channel_id.as_str());
    assert_eq!(f.db.count_task_inputs("child-c").unwrap(), 0);
    assert!(
        f.state
            .try_begin_requested_task_mutation("child-c")
            .is_some(),
        "a pending notification must never lock ordinary input"
    );

    // An invented id proves nothing and is refused.
    assert_eq!(f.confirm("channel-nope").await, StatusCode::CONFLICT);
    assert_eq!(f.confirm(&channel_id).await, StatusCode::OK);

    let attempt = f.wake(&mut stream).await;
    assert_eq!(attempt.batch_id, pending.batch_id);
    assert!(attempt.message.contains("kanna_read_event_subscription"));
    assert!(attempt.message.contains("not owner speech"));
    assert_eq!(
        f.db.count_task_inputs("child-c").unwrap(),
        0,
        "nothing is recorded until the transport confirms the write"
    );

    // The host reports only a transport write. It cannot claim a read or an
    // acknowledgement, and a failed write records nothing.
    assert_eq!(
        f.receipt(&attempt, json!({"kind": "read"})).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.receipt(
            &attempt,
            json!({"kind": "uncertain", "error": "pipe closed"})
        )
        .await,
        StatusCode::OK
    );
    await_subscription(&f.state, &f.id, |r| r.wake_state == "uncertain").await;
    assert_eq!(f.db.count_task_inputs("child-c").unwrap(), 0);

    // A confirmed write is delivery: one durable engine row, once, however
    // many times the receipt is replayed.
    for _ in 0..2 {
        assert_eq!(
            f.receipt(&attempt, json!({"kind": "written"})).await,
            StatusCode::OK
        );
    }
    await_subscription(&f.state, &f.id, |r| r.wake_state == "notified").await;
    let inputs = f.db.list_task_inputs("child-c", 10).unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].source, "engine");
    assert!(inputs[0].message.contains("<channel"));

    // The batch is still pending: a receipt is not an acknowledgement.
    let row = f.read(None).await;
    assert_eq!(row["batchId"], pending.batch_id);
    assert!(!row["pending"].is_null());
    let row = f.read(Some(pending.batch_id)).await;
    assert!(row["pending"].is_null());
    assert_eq!(row["wakeState"], "idle");
}

/// The second defect: a notice written into a running turn was absorbed by it
/// with no mailbox read, and the prototype still called that `notified`.
/// Written is not read — so when the turn ends unread, the same durable
/// attempt is repeated, without minting a second notice or a second record.
#[tokio::test]
async fn a_notice_absorbed_by_a_busy_turn_is_repeated_once_the_turn_ends_unread() {
    let f = Fixture::new().await;
    let mut stream = f.stream().await;
    let probe = stream.next().await;
    assert_eq!(
        f.confirm(probe["channelId"].as_str().unwrap()).await,
        StatusCode::OK
    );
    f.runtime("busy");

    f.emit();
    let attempt = f.wake(&mut stream).await;
    assert_eq!(
        f.receipt(&attempt, json!({"kind": "written"})).await,
        StatusCode::OK
    );
    await_subscription(&f.state, &f.id, |r| r.wake_state == "notified").await;

    // Still busy, still unread: nothing repeats while the turn is running.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        f.db.event_subscription(&f.id).unwrap().unwrap().wake_state,
        "notified"
    );

    // The turn ends without the subscriber ever reading its batch.
    f.runtime("idle");
    let repeat = f.wake(&mut stream).await;
    assert_eq!(
        repeat.id, attempt.id,
        "the same durable attempt is repeated"
    );
    assert_eq!(
        repeat.message, attempt.message,
        "no second notice is minted"
    );
    assert_eq!(
        f.db.count_task_inputs("child-c").unwrap(),
        1,
        "a repeat is not a second delivery record"
    );

    // This time the subscriber reads it. A read is not an acknowledgement, but
    // it does end the follow-up: the notice was consumed.
    let row = f.read(None).await;
    assert!(!row["pending"].is_null());
    let recorded =
        f.db.claude_channel_attempt(&attempt.id)
            .unwrap()
            .expect("attempt");
    assert!(recorded.written_at.is_some());
    assert!(
        recorded.read_at.is_some(),
        "a mailbox read is recorded separately from the write"
    );
    assert!(!recorded.absorbed_unread());
    assert!(
        f.state
            .try_begin_requested_task_mutation("child-c")
            .is_some(),
        "a repeated notification must never lock ordinary input"
    );
}
