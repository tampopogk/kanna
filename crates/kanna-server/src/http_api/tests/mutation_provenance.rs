//! Declared role versus server-derived channel identity, asserted on the
//! durable rows and events each mutation writes rather than on the extractor
//! alone. Every case pairs a caller-declared `operator` (or the operation's
//! own convention) with a channel only this server could have verified, so a
//! record that merged the two would fail here.

use super::workflow_switch::{
    plan_publication_fixture, replacement_fixture, seed_workflow_task, single_reviewer_suffix,
    workflow_test_repo,
};
use super::*;
use crate::db::TaskEventScope;
use crate::mutation_provenance::{
    ChannelIdentity, LocalProcessEvidence, PairedDeviceEvidence, PeerDesktopEvidence,
    SecureChannelTransport,
};
use serde_json::Value;

fn task_events(state: &Arc<AppState>, task_id: &str, event_type: &str) -> Vec<Value> {
    let db = Db::open(&state.config.db_path).unwrap();
    let head = db.latest_task_event_seq().unwrap();
    db.list_task_events(
        &TaskEventScope::Tasks(vec![task_id.to_string()]),
        0,
        head,
        500,
    )
    .unwrap()
    .into_iter()
    .filter(|event| event.event_type == event_type)
    .map(|event| event.payload)
    .collect()
}

fn json_request(path: &str, body: &Value) -> Request<Body> {
    Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn with_peer(mut request: Request<Body>, peer: [u8; 4]) -> Request<Body> {
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            peer, 50_000,
        ))));
    request
}

fn channel(value: &Value) -> ChannelIdentity {
    serde_json::from_value(value.clone()).expect("a channel identity")
}

fn pinned_definition(state: &Arc<AppState>) -> Value {
    let db = Db::open(&state.config.db_path).unwrap();
    let task = db.get_pipeline_item("task-1").unwrap().unwrap();
    serde_json::from_str(task.pipeline_def.as_deref().unwrap()).unwrap()
}

fn sealed_pairing(
    origin: crate::http_api::secure_channel::StreamOrigin,
) -> crate::http_api::secure_channel::SealedPairingContext {
    crate::http_api::secure_channel::SealedPairingContext {
        remote_static: [7u8; 32],
        handshake_hash: [9u8; 32],
        origin,
    }
}

/// One workflow replacement per transport, each declaring `operator`. The
/// recorded channel is whatever this server verified about that transport —
/// never the declaration, never a forged header, and never "local" for the
/// synthetic loopback address every in-process dispatch carries.
#[tokio::test]
async fn each_transport_records_its_verified_channel_beside_a_declared_operator() {
    use crate::http_api::secure_channel::StreamOrigin;

    let (_temp, state, _before) = replacement_fixture("provenance-transports");
    let device_secret = "provenance-device-secret";
    let mut pairings = crate::pairing::PairingStore::default();
    pairings.add_trusted_device(
        &state.config.desktop_id,
        "phone-lan",
        "Kanna Mobile",
        &crate::pairing::hash_device_secret(device_secret),
    );
    let pairing_path = std::path::PathBuf::from(&state.config.pairing_store_path);
    std::fs::create_dir_all(pairing_path.parent().unwrap()).unwrap();
    pairings.save(&pairing_path).unwrap();
    let app = router(Arc::clone(&state));
    let path = "/v1/tasks/task-1/actions/replace-workflow";

    let mut expected_channels = Vec::new();
    for round in 0..10 {
        let before = pinned_definition(&state);
        let mut after = before.clone();
        after["stages"][1]["description"] = serde_json::json!(format!("edit {round}"));
        let body = serde_json::json!({
            "workflowDefinition": after,
            "expectedDefinition": before,
            "source": "operator",
        });
        let (status, expected) = match round {
            // A real loopback socket, with forged identity headers a local
            // process could set: a device id without its secret proves
            // nothing, and no header names a channel.
            0 => {
                let mut request = with_peer(json_request(path, &body), [127, 0, 0, 1]);
                request
                    .headers_mut()
                    .insert("x-kanna-device-id", "phone-lan".parse().unwrap());
                request.headers_mut().insert(
                    "x-kanna-channel-identity",
                    r#"{"kind":"relayAccount","accountUid":"forged"}"#.parse().unwrap(),
                );
                let response = app.clone().oneshot(request).await.unwrap();
                (
                    response.status().as_u16(),
                    ChannelIdentity::LocalProcess {
                        evidence: LocalProcessEvidence::Loopback,
                    },
                )
            }
            // The legacy LAN device secret, verified against the store.
            1 => {
                let mut request = with_peer(json_request(path, &body), [192, 168, 1, 42]);
                request
                    .headers_mut()
                    .insert("x-kanna-device-id", "phone-lan".parse().unwrap());
                request
                    .headers_mut()
                    .insert("x-kanna-device-secret", device_secret.parse().unwrap());
                let response = app.clone().oneshot(request).await.unwrap();
                (
                    response.status().as_u16(),
                    ChannelIdentity::PairedDevice {
                        device_id: "phone-lan".into(),
                        evidence: PairedDeviceEvidence::LanDeviceSecret,
                    },
                )
            }
            // An in-process router call has no socket peer at all.
            2 => {
                let response = app
                    .clone()
                    .oneshot(json_request(path, &body))
                    .await
                    .unwrap();
                (response.status().as_u16(), ChannelIdentity::Unknown)
            }
            3 => {
                let response = crate::http_api::dispatch_sealed_device_http_invoke(
                    Arc::clone(&state),
                    "phone-sealed".into(),
                    sealed_pairing(StreamOrigin::RelayTunnel),
                    StreamOrigin::RelayTunnel,
                    "POST",
                    path,
                    body,
                )
                .await;
                (
                    response.status,
                    ChannelIdentity::PairedDevice {
                        device_id: "phone-sealed".into(),
                        evidence: PairedDeviceEvidence::SecureChannel {
                            transport: SecureChannelTransport::Relay,
                        },
                    },
                )
            }
            4 => {
                let response = crate::http_api::dispatch_sealed_peer_http_invoke(
                    Arc::clone(&state),
                    "desk-sibling".into(),
                    StreamOrigin::Lan,
                    "POST",
                    path,
                    body,
                )
                .await;
                (
                    response.status,
                    ChannelIdentity::PeerDesktop {
                        desktop_id: "desk-sibling".into(),
                        evidence: PeerDesktopEvidence::SecureChannel {
                            transport: SecureChannelTransport::Lan,
                        },
                        account_uid: None,
                    },
                )
            }
            5 => {
                let response = crate::http_api::dispatch_authenticated_relay_http_invoke(
                    Arc::clone(&state),
                    "uid-owner".into(),
                    None,
                    "POST",
                    path,
                    body,
                )
                .await;
                (
                    response.status,
                    ChannelIdentity::RelayAccount {
                        account_uid: "uid-owner".into(),
                        source_desktop_id: None,
                    },
                )
            }
            6 => {
                let response = crate::http_api::dispatch_authenticated_relay_http_invoke(
                    Arc::clone(&state),
                    "uid-owner".into(),
                    Some("desk-attested".into()),
                    "POST",
                    path,
                    body,
                )
                .await;
                (
                    response.status,
                    ChannelIdentity::RelayAccount {
                        account_uid: "uid-owner".into(),
                        source_desktop_id: Some("desk-attested".into()),
                    },
                )
            }
            // Shares the relay's `AuthenticatedHttpInvoke` marker, but the
            // bearer secret proved a sibling desktop, not a relay account.
            7 => {
                let response = crate::http_api::routes::dispatch_authenticated_lan_http_invoke(
                    Arc::clone(&state),
                    "desk-lan".into(),
                    "POST",
                    path,
                    body,
                )
                .await;
                (
                    response.status,
                    ChannelIdentity::PeerDesktop {
                        desktop_id: "desk-lan".into(),
                        evidence: PeerDesktopEvidence::LanMachineTrust,
                        account_uid: state.authenticated_account_uid(),
                    },
                )
            }
            // Authenticated by nothing this dispatch can name.
            8 => {
                let response = crate::http_api::dispatch_authenticated_http_invoke(
                    Arc::clone(&state),
                    "POST",
                    path,
                    body,
                )
                .await;
                (response.status, ChannelIdentity::Unknown)
            }
            // A tunnel that proved nothing is refused before it can record.
            _ => {
                let response =
                    crate::http_api::dispatch_http_invoke(Arc::clone(&state), "POST", path, body)
                        .await;
                assert_eq!(response.status, 401, "{:?}", response.body);
                continue;
            }
        };
        assert_eq!(status, 200, "round {round}");
        expected_channels.push(expected);
    }

    let events = task_events(&state, "task-1", "task.workflow_changed");
    assert_eq!(events.len(), expected_channels.len(), "{events:#?}");
    for (event, expected) in events.iter().zip(&expected_channels) {
        // The declaration is kept verbatim and separately...
        assert_eq!(event["source"], "operator");
        assert_eq!(event["declaredRole"], "operator");
        // ...and never decides the channel.
        assert_eq!(&channel(&event["channelIdentity"]), expected, "{event}");
    }
}

/// A result carries its own provenance, recorded beside the verdict and never
/// over the run's entry; an identical retry arriving on another channel is a
/// replay and leaves the first recording in place.
#[tokio::test]
async fn a_result_records_its_channel_apart_from_the_entry_and_keeps_it_on_retry() {
    use crate::http_api::secure_channel::StreamOrigin;

    let (_temp, state, _before) = plan_publication_fixture("provenance-result");
    let body = serde_json::json!({
        "runId": "run-plan",
        "status": "success",
        "summary": "the plan",
        // Payload fields name no channel: an unknown key is ignored, and
        // `source` is not part of this request at all.
        "channelIdentity": { "kind": "localProcess", "evidence": "loopback" },
        "source": "operator",
    });
    let response = crate::http_api::dispatch_sealed_peer_http_invoke(
        Arc::clone(&state),
        "desk-sibling".into(),
        StreamOrigin::RelayTunnel,
        "POST",
        "/v1/tasks/task-1/actions/complete-stage",
        body.clone(),
    )
    .await;
    assert_eq!(response.status, 200, "{:?}", response.body);
    let sibling = ChannelIdentity::PeerDesktop {
        desktop_id: "desk-sibling".into(),
        evidence: PeerDesktopEvidence::SecureChannel {
            transport: SecureChannelTransport::Relay,
        },
        account_uid: None,
    };

    let db = Db::open(&state.config.db_path).unwrap();
    let run = db.stage_run("run-plan").unwrap().unwrap();
    let provenance = run.result_provenance.clone().expect("result provenance");
    assert_eq!(provenance.declared_role, "agent");
    assert_eq!(provenance.channel_identity, sibling);
    // The fixture's run predates channel identity: its entry stays unknown
    // rather than borrowing the result's channel.
    assert_eq!(run.entry_channel_identity, ChannelIdentity::Unknown);
    let finished = task_events(&state, "task-1", "run.finished");
    let finished = finished
        .iter()
        .find(|event| event["runId"] == "run-plan")
        .expect("run.finished");
    assert_eq!(finished["declaredRole"], "agent");
    assert_eq!(channel(&finished["channelIdentity"]), sibling);

    // Same verdict, another channel: a replay, which writes nothing.
    let retry = crate::http_api::dispatch_authenticated_relay_http_invoke(
        Arc::clone(&state),
        "uid-owner".into(),
        None,
        "POST",
        "/v1/tasks/task-1/actions/complete-stage",
        body,
    )
    .await;
    assert_eq!(retry.status, 200, "{:?}", retry.body);
    let run = db.stage_run("run-plan").unwrap().unwrap();
    assert_eq!(run.result_provenance.unwrap().channel_identity, sibling);

    // The latest-run projection reports both facts.
    let detail = router(Arc::clone(&state))
        .oneshot(
            Request::get("/v1/tasks/task-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let detail: Value = serde_json::from_slice(
        &axum::body::to_bytes(detail.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        detail["latestRun"]["resultProvenance"]["declaredRole"],
        "agent"
    );
    assert_eq!(
        channel(&detail["latestRun"]["resultProvenance"]["channelIdentity"]),
        sibling
    );
    assert_eq!(
        channel(&detail["latestRun"]["entryChannelIdentity"]),
        ChannelIdentity::Unknown
    );
}

/// Publishing stages with a plan is one mutation: the run's verdict and the
/// workflow edit it carries record the same declared role and channel.
#[tokio::test]
async fn a_plan_extension_records_the_same_provenance_as_its_result() {
    let (_temp, state, before) = plan_publication_fixture("provenance-plan-extension");
    let after = single_reviewer_suffix(&before);
    let response = crate::http_api::dispatch_authenticated_relay_http_invoke(
        Arc::clone(&state),
        "uid-owner".into(),
        Some("desk-attested".into()),
        "POST",
        "/v1/tasks/task-1/actions/complete-stage",
        serde_json::json!({
            "runId": "run-plan",
            "status": "success",
            "summary": "plan with stages",
            "expectedDefinition": before,
            "workflowDefinition": after,
        }),
    )
    .await;
    assert_eq!(response.status, 200, "{:?}", response.body);
    let relay = ChannelIdentity::RelayAccount {
        account_uid: "uid-owner".into(),
        source_desktop_id: Some("desk-attested".into()),
    };
    let changed = task_events(&state, "task-1", "task.workflow_changed");
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["source"], "agent");
    assert_eq!(changed[0]["declaredRole"], "agent");
    assert_eq!(channel(&changed[0]["channelIdentity"]), relay);
    let run = Db::open(&state.config.db_path)
        .unwrap()
        .stage_run("run-plan")
        .unwrap()
        .unwrap();
    assert_eq!(run.result_provenance.unwrap().channel_identity, relay);
}

/// A named workflow switch declares no role; it still records its channel.
#[tokio::test]
async fn a_named_workflow_switch_records_its_channel_with_an_undeclared_role() {
    let (_repo_temp, repo_path) = workflow_test_repo("provenance-select");
    let state = test_state_with_seed("provenance-select", "Studio Mac", move |db| {
        seed_workflow_task(
            db,
            &repo_path,
            "task-1",
            "no-review",
            "in progress",
            r#"{"name":"old-snapshot","stages":[]}"#,
        );
    });
    let response = router(Arc::clone(&state))
        .oneshot(with_peer(
            json_request(
                "/v1/tasks/task-1/actions/set-workflow",
                &serde_json::json!({ "workflowName": "single-reviewer" }),
            ),
            [127, 0, 0, 1],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let changed = task_events(&state, "task-1", "task.workflow_changed");
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["operation"], "select");
    assert_eq!(changed[0]["declaredRole"], "unspecified");
    assert_eq!(
        channel(&changed[0]["channelIdentity"]),
        ChannelIdentity::LocalProcess {
            evidence: LocalProcessEvidence::Loopback
        }
    );
}

/// Delivered text: a sibling desktop declaring `operator` is recorded as
/// exactly that — an operator declaration arriving over that sibling's
/// verified session — on the durable row, its event, and the readback.
#[tokio::test]
async fn delivered_input_records_the_declared_source_and_the_verified_channel() {
    use crate::http_api::secure_channel::StreamOrigin;
    use kanna_daemon::protocol::{
        Command as DaemonCommand, Event as DaemonEvent, SessionInfo, SessionState, SessionStatus,
    };
    use tokio::io::{AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    let unique = format!("provenance-input-{}", unique_test_suffix());
    let daemon_dir = crate::test_paths::unique_test_path(&unique);
    std::fs::create_dir_all(&daemon_dir).unwrap();
    let socket_path = daemon_socket_path_for_dir(&daemon_dir.to_string_lossy());
    let listener = UnixListener::bind(&socket_path).unwrap();
    let daemon_server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        for _ in 0..2 {
            let command = read_test_daemon_command(&mut reader, &mut write_half).await;
            let response = match &command {
                DaemonCommand::List => DaemonEvent::SessionList {
                    sessions: vec![SessionInfo {
                        session_id: "task-live".to_string(),
                        pid: 42,
                        cwd: "/tmp".to_string(),
                        state: SessionState::Active,
                        idle_seconds: 0,
                        status: SessionStatus::Idle,
                        status_observed: true,
                        kind: Default::default(),
                        composer_text: None,
                        composer_attestation: Default::default(),
                        attempt_id: None,
                    }],
                },
                DaemonCommand::SubmitInputIfSession { .. } => DaemonEvent::Ok,
                other => panic!("unexpected daemon command: {other:?}"),
            };
            write_half
                .write_all(format!("{}\n", serde_json::to_string(&response).unwrap()).as_bytes())
                .await
                .unwrap();
        }
    });
    let state = crate::http_api::test_support::test_state_with_daemon_dir(
        &unique,
        "Studio Mac",
        &daemon_dir.to_string_lossy(),
        |db| {
            db.insert_test_repo("repo-1", "Repo One").unwrap();
            db.insert_test_pipeline_item(
                "task-live",
                "repo-1",
                "Live task",
                Some("Live task"),
                "in progress",
                "2026-09-22 00:00:00",
            )
            .unwrap();
        },
    );

    let response = crate::http_api::dispatch_sealed_peer_http_invoke(
        Arc::clone(&state),
        "desk-sibling".into(),
        StreamOrigin::Lan,
        "POST",
        "/v1/tasks/task-live/input",
        serde_json::json!({ "input": "ship it", "source": "operator" }),
    )
    .await;
    assert_eq!(response.status, 204, "{:?}", response.body);
    daemon_server.await.unwrap();
    let sibling = ChannelIdentity::PeerDesktop {
        desktop_id: "desk-sibling".into(),
        evidence: PeerDesktopEvidence::SecureChannel {
            transport: SecureChannelTransport::Lan,
        },
        account_uid: None,
    };

    let db = Db::open(&state.config.db_path).unwrap();
    let inputs = db.list_task_inputs("task-live", 10).unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].source, "operator");
    assert_eq!(inputs[0].channel_identity, sibling);
    let delivered = task_events(&state, "task-live", "task.input_delivered");
    assert_eq!(delivered[0]["source"], "operator");
    assert_eq!(delivered[0]["declaredRole"], "operator");
    assert_eq!(channel(&delivered[0]["channelIdentity"]), sibling);

    // The row, not the 14-day event, is the record: pruning every event
    // leaves the provenance readable.
    rusqlite::Connection::open(&state.config.db_path)
        .unwrap()
        .execute("DELETE FROM task_event", [])
        .unwrap();
    let readback = router(Arc::clone(&state))
        .oneshot(
            Request::get("/v1/tasks/task-live/inputs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let readback: Value = serde_json::from_slice(
        &axum::body::to_bytes(readback.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(readback["inputs"][0]["source"], "operator");
    assert_eq!(channel(&readback["inputs"][0]["channelIdentity"]), sibling);

    let _ = std::fs::remove_file(socket_path);
    let _ = std::fs::remove_dir_all(daemon_dir);
}
