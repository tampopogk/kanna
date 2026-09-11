use super::*;
use base64::Engine as _;

/// A router whose database actually contains the tasks a test names.
///
/// The push and task-transfers routes resolve their `{task_id}` path parameter
/// against `pipeline_item` — a task id or one of its branch names, like every
/// other task route — so a test that names a task the database has never heard
/// of is answered 404 rather than exercising the transfer surface.
fn test_router_with_tasks(desktop_id: &str, task_ids: &[&str]) -> axum::Router {
    let task_ids = task_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
    super::test_router_with_seed(desktop_id, "Studio Mac", move |db| {
        db.insert_test_repo("repo-transfer", "Transfer Repo")
            .expect("repo");
        for task_id in &task_ids {
            db.insert_test_pipeline_item(
                task_id,
                "repo-transfer",
                "transfer fixture task",
                Some("Transfer Fixture"),
                "in progress",
                "2026-09-06 00:00:00",
            )
            .expect("task");
        }
    })
}

fn outgoing_transfer_body(transfer_id: &str, source_task_id: &str) -> String {
    serde_json::json!({
        "transfer": {
            "id": transfer_id,
            "direction": "outgoing",
            "status": "pending",
            "source_peer_id": "peer-source",
            "target_peer_id": "peer-target",
            "source_desktop_id": "desktop-source",
            "target_desktop_id": "desktop-target",
            "source_task_id": source_task_id,
            "local_task_id": source_task_id,
            "error": null,
            "payload_json": "{}",
        }
    })
    .to_string()
}

fn test_id_token(expires_at: i64) -> String {
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::json!({ "exp": expires_at }).to_string());
    format!("header.{claims}.signature")
}

fn cloud_only_target(
    route: crate::cloud_transfer_proxy::CloudTransferRoute,
) -> crate::transfer_targets::TransferTarget {
    crate::transfer_targets::TransferTarget {
        peer_id: route.peer_id.clone(),
        name: "MacBook Pro".to_string(),
        machine_id: Some(route.machine_id.clone()),
        trusted: true,
        accepting_transfers: true,
        lan_available: false,
        cloud_available: true,
        preferred_transport: "cloud".to_string(),
        cloud_fallback: false,
        transferable: route.ready(),
        unavailable_reason: None,
        cloud_route: Some(route),
    }
}

async fn acknowledge_cloud_refresh(
    app: axum::Router,
    request_id: &str,
    outcome: &str,
) -> serde_json::Value {
    let mut request = Request::post("/v1/transfers/cloud-credential-refreshes/ack")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "requestId": request_id, "outcome": outcome }).to_string(),
        ))
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            49152,
        ))));
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn loopback_post(path: &str, body: serde_json::Value) -> Request<Body> {
    let mut request = Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            49152,
        ))));
    request
}

async fn wait_for_cloud_refresh_command(app: axum::Router) -> serde_json::Value {
    let mut request = Request::get("/v1/transfers/cloud-credential-commands?limit=1&timeoutSecs=5")
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            49152,
        ))));
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = from_slice::<serde_json::Value>(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["waitOutcome"], "events", "{body}");
    body["events"][0]["event"].clone()
}

async fn refresh_cloud_route(app: axum::Router, relay_url: &str) {
    let response = app
        .oneshot(loopback_post(
            "/v1/transfers/cloud-proxies",
            serde_json::json!({
                "peerId": "peer-studio",
                "desktopId": "desktop-studio",
                "relayUrl": relay_url,
                "idToken": test_id_token(4_000_000_000),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

struct TransferSidecarEnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl TransferSidecarEnvGuard {
    const NAMES: [&'static str; 3] = [
        "KANNA_TRANSFER_ROOT",
        "KANNA_TRANSFER_PEER_ID",
        "KANNA_TRANSFER_DISPLAY_NAME",
    ];

    fn set(root: &std::path::Path) -> Self {
        let saved = Self::NAMES
            .iter()
            .map(|&name| (name, std::env::var(name).ok()))
            .collect();
        std::env::set_var("KANNA_TRANSFER_ROOT", root);
        std::env::set_var("KANNA_TRANSFER_PEER_ID", "peer-local");
        std::env::set_var("KANNA_TRANSFER_DISPLAY_NAME", "Test Mac");
        Self { saved }
    }
}

impl Drop for TransferSidecarEnvGuard {
    fn drop(&mut self) {
        for (name, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

fn write_cloud_route_sidecar(root: &std::path::Path, endpoint: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(root).expect("sidecar fixture root");
    let stub = root.join("cloud-route-sidecar.sh");
    std::fs::write(
        &stub,
        format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"type":"list_peers"'*)
      printf '{{"request_id":"%s","peers":[{{"peer_id":"peer-studio","display_name":"Studio Mac","endpoint":"{endpoint}","trusted":true,"accepting_transfers":true}}]}}\n' "$id"
      ;;
    *'"type":"request_task_pull"'*)
      printf '%s\n' "$line" >> "$KANNA_TRANSFER_ROOT/pull-requests"
      printf '{{"request_id":"%s","pull_request_id":"pull-request-1"}}\n' "$id"
      ;;
    *)
      printf '{{"request_id":"%s","type":"error","message":"unexpected fixture request"}}\n' "$id"
      ;;
  esac
done
"#
        ),
    )
    .expect("write sidecar fixture");
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
        .expect("make sidecar fixture executable");
    stub
}

async fn cloud_route_http_test_state(
    label: &str,
    root: &std::path::Path,
) -> (Arc<AppState>, String) {
    let base = super::test_state_with_seed(label, "Test Mac", |db| {
        db.insert_test_repo("repo-transfer", "Transfer Repo")
            .expect("repo");
        db.insert_test_pipeline_item(
            "task-source",
            "repo-transfer",
            "transfer fixture task",
            Some("Transfer Fixture"),
            "in progress",
            "2026-09-06 00:00:00",
        )
        .expect("task");
    });
    let config = base.config().clone();
    let work = base.transfer_work();
    let stub = root.join("cloud-route-sidecar.sh");
    let supervisor = crate::transfer_sidecar::TransferSidecarSupervisor::with_binary_for_test(
        config.clone(),
        work,
        stub,
    );
    let state = Arc::new(AppState::with_transfer_sidecar_for_test(config, supervisor));
    let relay_url = "ws://127.0.0.1:9".to_string();
    let endpoint = crate::cloud_transfer_proxy::ensure_cloud_transfer_proxy_in_state(
        state.cloud_transfer_proxies(),
        "peer-studio".to_string(),
        "desktop-studio".to_string(),
        relay_url.clone(),
        test_id_token(1),
    )
    .await
    .expect("stale cloud route");
    write_cloud_route_sidecar(root, &endpoint.endpoint);
    (state, relay_url)
}

#[tokio::test]
async fn push_entrypoint_refreshes_an_expired_cloud_route_before_queueing() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let root = crate::test_paths::unique_test_path("http-cloud-push-env");
    let _env_guard = TransferSidecarEnvGuard::set(&root);
    let (state, relay_url) = cloud_route_http_test_state("push-refresh", &root).await;
    let app = super::router(Arc::clone(&state));

    let request_app = app.clone();
    let request = tokio::spawn(async move {
        request_app
            .oneshot(loopback_post(
                "/v1/tasks/task-source/actions/push-to-peer",
                serde_json::json!({ "peerId": "peer-studio", "transport": "cloud" }),
            ))
            .await
            .unwrap()
    });
    let command = wait_for_cloud_refresh_command(app.clone()).await;
    assert_eq!(command["peerId"], "peer-studio");
    assert!(
        command.get("idToken").is_none(),
        "credential leaked into {command}"
    );
    assert!(
        !request.is_finished(),
        "push passed the stale route preflight"
    );
    assert!(
        state
            .transfer_work()
            .open_db()
            .unwrap()
            .claim_next_transfer_work(&Vec::<String>::new())
            .unwrap()
            .is_none(),
        "push work was queued before the refreshed route was observed"
    );

    refresh_cloud_route(app.clone(), &relay_url).await;
    acknowledge_cloud_refresh(app, command["requestId"].as_str().unwrap(), "refreshed").await;

    let response = tokio::time::timeout(std::time::Duration::from_secs(5), request)
        .await
        .expect("push did not finish after refresh")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = from_slice::<serde_json::Value>(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["scheduled"], true);
    assert_eq!(body["target"]["transport"], "cloud");
    assert_eq!(body["target"]["peerId"], "peer-studio");
}

#[tokio::test]
async fn pull_entrypoint_refreshes_an_expired_cloud_route_before_forwarding() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let root = crate::test_paths::unique_test_path("http-cloud-pull-env");
    let _env_guard = TransferSidecarEnvGuard::set(&root);
    let (state, relay_url) = cloud_route_http_test_state("pull-refresh", &root).await;
    let app = super::router(Arc::clone(&state));

    let request_app = app.clone();
    let request = tokio::spawn(async move {
        request_app
            .oneshot(loopback_post(
                "/v1/transfers/actions/pull-task",
                serde_json::json!({
                    "sourceTaskId": "task-remote",
                    "sourceMachine": "peer-studio",
                    "transport": "cloud",
                }),
            ))
            .await
            .unwrap()
    });
    let command = wait_for_cloud_refresh_command(app.clone()).await;
    assert!(
        !request.is_finished(),
        "pull passed the stale route preflight"
    );
    assert!(
        !root.join("pull-requests").exists(),
        "pull was forwarded before the refreshed route was observed"
    );

    refresh_cloud_route(app.clone(), &relay_url).await;
    acknowledge_cloud_refresh(app, command["requestId"].as_str().unwrap(), "refreshed").await;

    let response = tokio::time::timeout(std::time::Duration::from_secs(5), request)
        .await
        .expect("pull did not finish after refresh")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = from_slice::<serde_json::Value>(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["accepted"], true);
    assert_eq!(body["source"]["transport"], "cloud");
    let forwarded =
        std::fs::read_to_string(root.join("pull-requests")).expect("forwarded pull request");
    assert!(forwarded.contains("request_task_pull"), "{forwarded}");
    assert!(forwarded.contains("task-remote"), "{forwarded}");
}

#[tokio::test]
async fn push_entrypoint_reports_sign_in_required_without_queueing() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let root = crate::test_paths::unique_test_path("http-cloud-sign-in-env");
    let _env_guard = TransferSidecarEnvGuard::set(&root);
    let (state, _) = cloud_route_http_test_state("push-sign-in-required", &root).await;
    let app = super::router(Arc::clone(&state));

    let request_app = app.clone();
    let request = tokio::spawn(async move {
        request_app
            .oneshot(loopback_post(
                "/v1/tasks/task-source/actions/push-to-peer",
                serde_json::json!({ "peerId": "peer-studio", "transport": "cloud" }),
            ))
            .await
            .unwrap()
    });
    let command = wait_for_cloud_refresh_command(app.clone()).await;
    acknowledge_cloud_refresh(
        app,
        command["requestId"].as_str().unwrap(),
        "sign_in_required",
    )
    .await;

    let response = tokio::time::timeout(std::time::Duration::from_secs(5), request)
        .await
        .expect("sign-in-required response was not bounded")
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("not signed in"), "{body}");
    assert!(body.contains("retry the transfer"), "{body}");
    assert!(
        state
            .transfer_work()
            .open_db()
            .unwrap()
            .claim_next_transfer_work(&Vec::<String>::new())
            .unwrap()
            .is_none(),
        "a sign-in-required push must not queue work"
    );
}

/// The moving-day regression: an agent-facing transfer encounters a stale
/// outbound route, the renderer rotates it through the existing proxy owner,
/// and route admission observes the new credential rather than trusting the
/// acknowledgement alone.
#[tokio::test]
async fn expired_cloud_credential_can_be_refreshed_and_then_admitted() {
    let state = super::test_state_with_seed("desktop-refreshable", "MacBook Pro", |_| {});
    let relay_url = "ws://127.0.0.1:9";
    crate::cloud_transfer_proxy::ensure_cloud_transfer_proxy_in_state(
        state.cloud_transfer_proxies(),
        "peer-studio".to_string(),
        "desktop-studio".to_string(),
        relay_url.to_string(),
        test_id_token(1),
    )
    .await
    .unwrap();
    let stale = crate::cloud_transfer_proxy::cloud_transfer_routes(state.cloud_transfer_proxies())
        .await
        .remove(0);
    assert_eq!(stale.status, "credential_expired");
    assert!(crate::transfer_targets::plan_route(&cloud_only_target(stale), Some("cloud")).is_err());

    let waiting_state = Arc::clone(&state);
    let waiting = tokio::spawn(async move {
        crate::http_api::ensure_engine_cloud_transfer_credential(
            &waiting_state,
            "peer-studio",
            Some("cloud"),
        )
        .await
    });
    let command = loop {
        let batch = state.cloud_transfer_refresh_commands().read(None, None, 10);
        if let Some(command) = batch.events.first() {
            break command["event"].clone();
        }
        tokio::task::yield_now().await;
    };
    assert_eq!(command["type"], "cloud_transfer_credential_refresh");
    assert_eq!(command["peerId"], "peer-studio");
    assert!(
        command.get("idToken").is_none(),
        "credential leaked into {command}"
    );

    crate::cloud_transfer_proxy::ensure_cloud_transfer_proxy_in_state(
        state.cloud_transfer_proxies(),
        "peer-studio".to_string(),
        "desktop-studio".to_string(),
        relay_url.to_string(),
        test_id_token(4_000_000_000),
    )
    .await
    .unwrap();
    let ack = acknowledge_cloud_refresh(
        super::router(Arc::clone(&state)),
        command["requestId"].as_str().unwrap(),
        "refreshed",
    )
    .await;
    assert_eq!(ack["acknowledged"], true);
    waiting.await.unwrap().unwrap();

    let fresh = crate::cloud_transfer_proxy::cloud_transfer_routes(state.cloud_transfer_proxies())
        .await
        .remove(0);
    assert_eq!(fresh.status, "ready");
    let admitted = crate::transfer_targets::plan_route(&cloud_only_target(fresh), Some("cloud"))
        .expect("the refreshed route is admitted");
    assert_eq!(admitted.transport, "cloud");
}

#[tokio::test]
async fn signed_out_renderer_returns_an_explicit_bounded_refresh_failure() {
    let state = super::test_state_with_seed("desktop-signed-out", "MacBook Pro", |_| {});
    crate::cloud_transfer_proxy::ensure_cloud_transfer_proxy_in_state(
        state.cloud_transfer_proxies(),
        "peer-studio".to_string(),
        "desktop-studio".to_string(),
        "ws://127.0.0.1:9".to_string(),
        test_id_token(1),
    )
    .await
    .unwrap();
    let waiting_state = Arc::clone(&state);
    let waiting = tokio::spawn(async move {
        crate::http_api::ensure_engine_cloud_transfer_credential(
            &waiting_state,
            "peer-studio",
            Some("cloud"),
        )
        .await
    });
    let command = loop {
        let batch = state.cloud_transfer_refresh_commands().read(None, None, 10);
        if let Some(command) = batch.events.first() {
            break command["event"].clone();
        }
        tokio::task::yield_now().await;
    };
    acknowledge_cloud_refresh(
        super::router(Arc::clone(&state)),
        command["requestId"].as_str().unwrap(),
        "sign_in_required",
    )
    .await;
    let failure = waiting.await.unwrap().expect_err("sign-in is required");
    assert_eq!(
        failure,
        crate::http_api::CloudTransferRefreshFailure::SignInRequired
    );
    let error = failure.to_string();
    assert!(error.contains("not signed in"), "{error}");
    assert!(error.contains("retry the transfer"), "{error}");
}

#[tokio::test]
async fn unavailable_desktop_bounds_cloud_credential_refresh() {
    let state = super::test_state_with_seed("desktop-closed", "MacBook Pro", |_| {});
    state.set_cloud_transfer_refresh_timeout_ms(20);
    let error = crate::http_api::transfers::request_cloud_transfer_refresh(&state, "peer-studio")
        .await
        .expect_err("an absent renderer cannot refresh")
        .to_string();
    assert!(
        error.contains("no Kanna desktop window acknowledged"),
        "{error}"
    );
}

async fn post_transfer(
    app: &axum::Router,
    transfer_id: &str,
    source_task_id: &str,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/transfers")
                .header("content-type", "application/json")
                .body(Body::from(outgoing_transfer_body(
                    transfer_id,
                    source_task_id,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, from_slice(&body).unwrap())
}

/// A second push for a task that already has one in flight is a race between
/// two `task-pull-requested` deliveries, not a broken write. On 2026-08-06 it
/// surfaced as a raw 500 from `idx_task_transfer_active_outgoing_source`, which
/// gave the caller no way to tell "already in flight" from "the insert failed"
/// — so it kept its orphaned sidecar reservation instead of releasing it.
#[tokio::test]
async fn duplicate_outgoing_transfer_insert_answers_409_with_the_transfer_in_flight() {
    let app = super::test_router("desktop-1", "Studio Mac");

    let (status, body) = post_transfer(&app, "transfer-first", "task-source").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], "transfer-first");

    let (status, body) = post_transfer(&app, "transfer-second", "task-source").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "active_outgoing_transfer_exists");
    assert_eq!(body["sourceTaskId"], "task-source");
    // The caller learns which transfer owns the task, so it can report the one
    // that is really running rather than the reservation it just abandoned.
    assert_eq!(body["transferId"], "transfer-first");

    // A different source task is not this constraint's business.
    let (status, _) = post_transfer(&app, "transfer-other", "task-other").await;
    assert_eq!(status, StatusCode::OK);
}

/// Re-sending the same insert is how a retried request arrives; the row's own
/// id conflict has always been a no-op, and it must not be mistaken for the
/// active-transfer conflict.
#[tokio::test]
async fn reinserting_the_same_outgoing_transfer_stays_a_success() {
    let app = super::test_router("desktop-1", "Studio Mac");

    let (status, _) = post_transfer(&app, "transfer-retry", "task-source").await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = post_transfer(&app, "transfer-retry", "task-source").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], "transfer-retry");
}

/// The eligibility read a push makes before starting work. It has to agree with
/// the index exactly: anything it reports as free, the index must accept.
#[tokio::test]
async fn active_outgoing_route_reports_only_transfers_the_index_still_holds() {
    let app = super::test_router("desktop-1", "Studio Mac");

    let read = |source_task_id: &'static str| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    Request::get(format!("/v1/transfers/outgoing/active/{source_task_id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            from_slice::<serde_json::Value>(&body).unwrap()["transfer"].clone()
        }
    };

    assert!(read("task-source").await.is_null());

    let (status, _) = post_transfer(&app, "transfer-active", "task-source").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read("task-source").await["id"], "transfer-active");
    assert!(read("task-other").await.is_null());

    // Once the transfer reaches a terminal state the index frees the task, and
    // so must this read — otherwise a push is refused forever.
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/transfers/transfer-active/actions/fail-outgoing")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "reason": "peer went away" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    assert!(read("task-source").await.is_null());
    let (status, _) = post_transfer(&app, "transfer-retry", "task-source").await;
    assert_eq!(status, StatusCode::OK);
}

/// Every push is its own intent, and only an explicit idempotency key collapses
/// two into one.
///
/// `transfer_work.id` is a permanent primary key and no row is ever pruned, so
/// keying a push on anything that repeats — the peer id was the first attempt —
/// makes every push of a task to that peer after the first schedule nothing,
/// forever. Pushing the same task to the same machine again is ordinary: the
/// first one failed and the operator fixed it, or they simply want it there
/// again. The duplicate-*delivery* race the T3 note describes is handled a
/// layer down, by the engine's eligibility read against
/// `idx_task_transfer_active_outgoing_source`, not by this key.
#[tokio::test]
async fn each_push_is_its_own_intent_unless_the_caller_supplies_a_key() {
    let app = test_router_with_tasks("desktop-push-intent-key", &["task-source"]);

    let push = |intent_key: Option<&'static str>| {
        let app = app.clone();
        async move {
            let mut body = serde_json::json!({ "peerId": "peer-target" });
            if let Some(intent_key) = intent_key {
                body["intentKey"] = serde_json::json!(intent_key);
            }
            let response = app
                .oneshot(
                    Request::post("/v1/tasks/task-source/actions/push-to-peer")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (status, from_slice::<serde_json::Value>(&body).unwrap())
        }
    };

    // Every click schedules. Before this the second one silently did nothing,
    // for the rest of the database's life.
    for attempt in 0..3 {
        let (status, body) = push(None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["scheduled"], true, "push {attempt} scheduled nothing");
    }

    // A caller that retries its own request and does not want the retry to
    // become a second push says so, and is answered `false` the second time.
    let (status, body) = push(Some("operator-retry")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["scheduled"], true);
    let (status, body) = push(Some("operator-retry")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["scheduled"], false);
}

/// Approve and reject are intents against a transfer that must exist and must
/// be incoming — a route that queued work for an unknown id would leave the
/// engine failing an item nobody can act on.
#[tokio::test]
async fn incoming_intents_require_an_incoming_transfer_and_are_idempotent() {
    let app = super::test_router("desktop-1", "Studio Mac");

    let intent = |action: &'static str, transfer_id: &'static str| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    Request::post(format!("/v1/transfers/{transfer_id}/actions/{action}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (
                status,
                from_slice::<serde_json::Value>(&body).unwrap_or_default(),
            )
        }
    };

    let (status, _) = intent("approve", "transfer-unknown").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // An outgoing transfer is not something this machine approves.
    let (status, _) = post_transfer(&app, "transfer-outgoing", "task-source").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = intent("approve", "transfer-outgoing").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/transfers")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "transfer": {
                            "id": "transfer-incoming",
                            "direction": "incoming",
                            "status": "pending",
                            "source_peer_id": "peer-source",
                            "target_peer_id": null,
                            "source_desktop_id": null,
                            "target_desktop_id": null,
                            "source_task_id": "task-remote",
                            "local_task_id": null,
                            "error": null,
                            "payload_json": "{}",
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let (status, body) = intent("approve", "transfer-incoming").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["scheduled"], true);
    // A second click is the same intent, not a second import.
    let (status, body) = intent("approve", "transfer-incoming").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["scheduled"], false);

    let (status, body) = intent("reject-incoming", "transfer-incoming").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["scheduled"], true);
}

/// A push answers what actually happened, and never lets a queued intent read
/// as a finished move.
///
/// A task manager on 2026-09-06 read `scheduled: true` from this route as a
/// completed transfer while the move was in fact dying on a relay socket the
/// caller could not see. The intent is still just an intent, so the response
/// says so in the same breath it says the work was queued.
#[tokio::test]
async fn a_push_response_states_that_nothing_has_moved_yet() {
    let app = test_router_with_tasks("desktop-push-intent", &["task-source"]);

    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/tasks/task-source/actions/push-to-peer")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "peerId": "peer-target", "transport": "lan" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = from_slice::<serde_json::Value>(&body).unwrap();

    assert_eq!(body["scheduled"], true);
    assert_eq!(body["state"], "scheduled");
    assert_eq!(body["moved"], false);
    assert_eq!(body["sourceTaskId"], "task-source");
    assert_eq!(body["target"]["peerId"], "peer-target");
    assert_eq!(body["target"]["transport"], "lan");
    assert!(
        body["nextStep"]
            .as_str()
            .is_some_and(|step| step.contains("kanna_task_transfers")),
        "{body}"
    );
    // No peer registry is reachable in this fixture, so the desktop's own
    // spelling — a peer it already resolved — still schedules, and says the
    // resolution was the caller's.
    assert!(
        body["note"]
            .as_str()
            .is_some_and(|note| note.contains("caller's own peer id")),
        "{body}"
    );
}

/// A destination has to be named. Before this route resolved one centrally the
/// only spelling was a raw peer id an agent had to find in desktop source, so
/// the refusal names the argument that replaces that.
#[tokio::test]
async fn a_push_with_no_destination_is_refused_rather_than_queued() {
    let app = test_router_with_tasks("desktop-push-nodest", &["task-source"]);

    let response = app
        .oneshot(
            Request::post("/v1/tasks/task-source/actions/push-to-peer")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("targetMachine"),
        "{}",
        String::from_utf8_lossy(&body)
    );
}

/// The duplicate an agent is most likely to create: asking for a move that is
/// already running. The engine would skip it silently, so the route reports the
/// transfer that owns the task instead of a fresh `scheduled: true` the caller
/// would read as a second move.
#[tokio::test]
async fn a_push_at_a_task_already_in_flight_names_the_transfer_that_owns_it() {
    let app = test_router_with_tasks("desktop-push-dup", &["task-source"]);
    let (status, _) = post_transfer(&app, "transfer-live", "task-source").await;
    assert_eq!(status, StatusCode::OK);

    let response = app
        .oneshot(
            Request::post("/v1/tasks/task-source/actions/push-to-peer")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "peerId": "peer-target" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = from_slice::<serde_json::Value>(&body).unwrap();

    assert_eq!(body["scheduled"], false);
    assert_eq!(body["state"], "already_in_flight");
    assert_eq!(body["moved"], false);
    assert_eq!(body["activeTransfer"]["id"], "transfer-live");
    assert_eq!(body["activeTransfer"]["state"], "pending");
    assert_eq!(body["activeTransfer"]["direction"], "outgoing");
}

/// The observation half. A move is two rows on two machines tied together by
/// the task's ids, and this is the surface that answers "did it actually
/// happen?" — with the coarse verdict, because the engine's own status
/// vocabulary is longer than an agent should have to learn.
#[tokio::test]
async fn task_transfers_report_each_recorded_move_with_a_coarse_verdict() {
    let app = test_router_with_tasks(
        "desktop-task-transfers",
        &["task-source", "task-other", "task-arrived"],
    );

    let read = |task_id: &'static str| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    Request::get(format!("/v1/tasks/{task_id}/transfers"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            from_slice::<serde_json::Value>(&body).unwrap()
        }
    };

    // A task nothing has happened to reports an empty list, which is not the
    // same claim as "no move was requested".
    assert_eq!(
        read("task-source").await["transfers"],
        serde_json::json!([])
    );

    let (status, _) = post_transfer(&app, "transfer-live", "task-source").await;
    assert_eq!(status, StatusCode::OK);
    let listed = read("task-source").await;
    assert_eq!(listed["taskId"], "task-source");
    assert_eq!(listed["transfers"][0]["id"], "transfer-live");
    assert_eq!(listed["transfers"][0]["status"], "pending");
    assert_eq!(listed["transfers"][0]["state"], "pending");
    assert_eq!(listed["transfers"][0]["sourceMachineId"], "desktop-source");
    assert_eq!(listed["transfers"][0]["targetMachineId"], "desktop-target");
    assert!(read("task-other").await["transfers"]
        .as_array()
        .is_some_and(|transfers| transfers.is_empty()));

    // The destination side of the same move: found by the id the task carries
    // there, and reported as incoming.
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/transfers")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "transfer": {
                            "id": "transfer-incoming",
                            "direction": "incoming",
                            "status": "completed",
                            "source_peer_id": "peer-source",
                            "target_peer_id": "peer-target",
                            "source_desktop_id": "desktop-source",
                            "target_desktop_id": "desktop-target",
                            "source_task_id": "task-source",
                            "local_task_id": "task-arrived",
                            "error": null,
                            "payload_json": "{}",
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let arrived = read("task-arrived").await;
    assert_eq!(arrived["transfers"][0]["id"], "transfer-incoming");
    assert_eq!(arrived["transfers"][0]["direction"], "incoming");
    assert_eq!(arrived["transfers"][0]["state"], "completed");
    assert_eq!(arrived["transfers"][0]["sourceTaskId"], "task-source");
    assert_eq!(arrived["transfers"][0]["localTaskId"], "task-arrived");

    // Asked by the durable source id, both halves answer.
    let both = read("task-source").await;
    let ids = both["transfers"]
        .as_array()
        .expect("transfers")
        .iter()
        .map(|transfer| transfer["id"].as_str().unwrap_or_default().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        ids,
        ["transfer-incoming", "transfer-live"]
            .into_iter()
            .map(str::to_string)
            .collect::<std::collections::BTreeSet<_>>()
    );
}

/// A failed transfer must read as failed, not as "not completed yet". The
/// distinction is the whole reason the coarse verdict exists.
#[tokio::test]
async fn a_failed_transfer_reports_failed_with_its_reason() {
    let app = test_router_with_tasks("desktop-failed-transfer", &["task-source"]);
    let (status, _) = post_transfer(&app, "transfer-doomed", "task-source").await;
    assert_eq!(status, StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/transfers/transfer-doomed/actions/fail-outgoing")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "reason": "cloud transfer relay rejected tunnel" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::get("/v1/tasks/task-source/transfers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = from_slice::<serde_json::Value>(&body).unwrap();
    assert_eq!(body["transfers"][0]["state"], "failed");
    assert_eq!(
        body["transfers"][0]["error"],
        "cloud transfer relay rejected tunnel"
    );
}

/// The 2026-09-08 report, from the asking machine's side.
///
/// The Studio pulled a task, the MacBook refused it, and
/// `GET /v1/tasks/afed27d1/transfers` here answered 404 — "no such task",
/// which is exactly what the operator already knew. The refusal is recorded
/// against the *source's* id, so that id has to answer even though nothing by
/// that name ever arrived here.
#[tokio::test]
async fn a_refused_pull_is_readable_on_the_machine_that_asked_for_it() {
    let app = test_router_with_tasks("desktop-refused-pull", &["task-local"]);

    let read = |task_id: &'static str| {
        let app = app.clone();
        async move {
            app.oneshot(
                Request::get(format!("/v1/tasks/{task_id}/transfers"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };

    // Before the refusal is recorded, a task that lives on another machine is
    // genuinely unknown here.
    assert_eq!(read("afed27d1").await.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/transfers")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "transfer": {
                            "id": "refused-pull-peer-mbp-pull-3",
                            "direction": "incoming",
                            "status": "failed",
                            "source_peer_id": "peer-mbp",
                            "target_peer_id": null,
                            "source_desktop_id": null,
                            "target_desktop_id": null,
                            "source_task_id": "afed27d1",
                            "local_task_id": null,
                            "error": "task afed27d1 resumes codex session 5a2eb492 but its rollout could not be found under ~/.codex/sessions",
                            "payload_json": null,
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = read("afed27d1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = from_slice::<serde_json::Value>(&body).unwrap();
    assert_eq!(body["taskId"], "afed27d1");
    assert_eq!(body["transfers"][0]["state"], "failed");
    assert_eq!(body["transfers"][0]["direction"], "incoming");
    assert!(body["transfers"][0]["error"]
        .as_str()
        .is_some_and(|reason| reason.contains("rollout could not be found")));

    // A task id nothing was ever attempted for still answers 404: the fallback
    // reports a real record, never invents one.
    assert_eq!(
        read("never-heard-of-it").await.status(),
        StatusCode::NOT_FOUND
    );
}

/// A pull moves a task onto *this* machine, so it is expressed only by a
/// process running on it — the same `DesktopLocalAccess` boundary the rest of
/// the sidecar control plane keeps, and deliberately narrower than the push it
/// asks the other machine to perform. Neither an authenticated relay tunnel nor
/// anything short of a direct connection to this desktop's own listener may
/// move a task here.
#[tokio::test]
async fn a_pull_is_reachable_only_from_this_desktop_s_own_loopback_connection() {
    let response = crate::http_api::routes::dispatch_authenticated_http_invoke(
        super::test_state_with_seed("desktop-pull-tunnel", "Studio Mac", |_| {}),
        "POST",
        "/v1/transfers/actions/pull-task",
        serde_json::json!({ "sourceTaskId": "task-source", "sourceMachine": "peer-primary" }),
    )
    .await;
    assert_eq!(response.status, 401, "{response:?}");
    assert!(
        response
            .error
            .as_deref()
            .into_iter()
            .chain(response.body.as_ref().and_then(|body| body.as_str()))
            .any(|message| message.contains("direct desktop loopback connection")),
        "{response:?}"
    );

    // The same route on a request with no connection identity at all, which is
    // what a synthesized in-process caller is. It must not be admitted either:
    // the guard is positive proof of a direct desktop connection, never the
    // absence of evidence against one.
    let app = super::test_router("desktop-pull-guard", "Studio Mac");
    let response = app
        .oneshot(
            Request::post("/v1/transfers/actions/pull-task")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "sourceTaskId": "task-source",
                        "sourceMachine": "peer-primary",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// The catalog promises both spellings of a task's identity, and an agent
/// routinely holds the branch one — `kanna_get_task` reports it as `branch` and
/// it names the worktree directory.
///
/// Answering a branch name with `transfers: []` is the worst shape this surface
/// can take: the tool description tells the caller to read an empty list as
/// "nothing has arrived yet, never that a pull was not requested", so an
/// unresolved identifier turns a real transfer into a confident "no".
#[tokio::test]
async fn task_transfers_answer_to_a_branch_name_exactly_as_to_the_task_id() {
    let app = super::test_router_with_seed("desktop-transfers-branch", "Studio Mac", |db| {
        db.insert_test_repo("repo-branch", "Branch Repo")
            .expect("repo");
        db.insert_test_pipeline_item(
            "task-source",
            "repo-branch",
            "moved by branch name",
            Some("Branch Task"),
            "in progress",
            "2026-09-06 00:00:00",
        )
        .expect("task");
    });

    let read = |identifier: &'static str| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    Request::get(format!("/v1/tasks/{identifier}/transfers"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (
                status,
                from_slice::<serde_json::Value>(&body).unwrap_or_default(),
            )
        }
    };

    let (status, _) = post_transfer(&app, "transfer-by-branch", "task-source").await;
    assert_eq!(status, StatusCode::OK);

    let (status, by_id) = read("task-source").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(by_id["transfers"][0]["id"], "transfer-by-branch");

    // `insert_test_pipeline_item` names the branch `branch-{id}`, so this is a
    // genuinely different string from the durable id.
    let (status, by_branch) = read("branch-task-source").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        by_branch, by_id,
        "a branch name must answer with the task's own records, not an empty list"
    );
    // …including the id the caller can carry onward, which is the durable one.
    assert_eq!(by_branch["taskId"], "task-source");

    // An identifier that names no task is a 404, not an empty list that reads
    // as "nothing has arrived yet".
    let (status, _) = read("task-does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The same resolution on the push, where an unresolved identifier fails
/// *silently and forever*.
///
/// The route answered `scheduled: true` and the engine then failed
/// `SourceTask::load` with a **retriable** "task not found"
/// (`transfer_engine/push.rs`), so no `task_transfer` row was ever written and
/// the work item retried out of sight — while the caller polled a surface the
/// engine had given nothing to report.
#[tokio::test]
async fn a_push_named_by_branch_queues_work_for_the_durable_task_id() {
    let state = super::test_state_with_seed("desktop-push-branch", "Studio Mac", |db| {
        db.insert_test_repo("repo-branch", "Branch Repo")
            .expect("repo");
        db.insert_test_pipeline_item(
            "task-source",
            "repo-branch",
            "pushed by branch name",
            Some("Branch Task"),
            "in progress",
            "2026-09-06 00:00:00",
        )
        .expect("task");
    });
    let app = super::router(std::sync::Arc::clone(&state));

    let response = app
        .oneshot(
            Request::post("/v1/tasks/branch-task-source/actions/push-to-peer")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "peerId": "peer-target", "transport": "lan" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = from_slice::<serde_json::Value>(&body).unwrap();

    assert_eq!(body["scheduled"], true);
    assert_eq!(
        body["sourceTaskId"], "task-source",
        "the answer must name the task the engine will actually load"
    );
    assert!(
        body["workId"]
            .as_str()
            .is_some_and(|work_id| work_id.starts_with("push:task-source:")),
        "{body}"
    );

    // The payload the engine reads is the part that used to be unloadable.
    let db = state.transfer_work().open_db().expect("db");
    let queued = db
        .claim_next_transfer_work(&Vec::<String>::new())
        .expect("claim")
        .expect("the push queued work");
    let payload = serde_json::from_str::<serde_json::Value>(&queued.payload_json).expect("payload");
    assert_eq!(payload["sourceTaskId"], "task-source");
}

/// An identifier that resolves to no task must be refused before anything is
/// queued, rather than answering like a scheduled push.
#[tokio::test]
async fn a_push_at_an_unknown_task_is_refused_with_nothing_queued() {
    let state = super::test_state_with_seed("desktop-push-unknown", "Studio Mac", |db| {
        db.insert_test_repo("repo-branch", "Branch Repo")
            .expect("repo");
    });
    let app = super::router(std::sync::Arc::clone(&state));

    let response = app
        .oneshot(
            Request::post("/v1/tasks/task-does-not-exist/actions/push-to-peer")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "peerId": "peer-target" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("task not found"),
        "{}",
        String::from_utf8_lossy(&body)
    );

    let db = state.transfer_work().open_db().expect("db");
    assert!(
        db.claim_next_transfer_work(&Vec::<String>::new())
            .expect("claim")
            .is_none(),
        "a refused push must leave the engine nothing to retry",
    );
}
