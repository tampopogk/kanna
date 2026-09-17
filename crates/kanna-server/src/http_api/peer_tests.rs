//! Negative-security and authority tests for sealed desktop-to-desktop
//! sessions, driven through the same socket runner production uses with a
//! sibling desktop played by `kanna_secure_channel`'s initiator side, plus
//! one real loopback run of the outbound peer channel and transfer tunnel
//! against a served router.

use super::lan_trust::DesktopLocalAccess;
use super::secure_channel::{SealedPopulation, StreamOrigin};
use super::state::AppState;
use super::RelayAccess;
use crate::config::Config;
use crate::peer_trust::{PeerDesktop, PeerTrustStore};
use crate::relay_client::RelayEntitlement;
use axum::extract::State;
use kanna_secure_channel::{
    Domain, HelloIntent, InitiatorHello, Keypair, PendingInitiator, Received,
};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;

struct FakeSocket {
    rx: tokio_mpsc::UnboundedReceiver<String>,
    tx: tokio_mpsc::UnboundedSender<String>,
}

#[derive(Debug)]
struct SocketClosed;

impl futures_util::Stream for FakeSocket {
    type Item = Result<String, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx).map(|item| item.map(Ok))
    }
}

impl futures_util::Sink<String> for FakeSocket {
    type Error = SocketClosed;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: String) -> Result<(), Self::Error> {
        self.tx.send(item).map_err(|_| SocketClosed)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

/// The sibling's end of an in-memory socket to a desktop's peer endpoint.
struct Sibling {
    to_server: Option<tokio_mpsc::UnboundedSender<String>>,
    from_server: tokio_mpsc::UnboundedReceiver<String>,
    session: tokio::task::JoinHandle<()>,
}

impl Sibling {
    fn connect(state: &Arc<AppState>, origin: StreamOrigin, population: SealedPopulation) -> Self {
        let (to_server, server_rx) = tokio_mpsc::unbounded_channel();
        let (server_tx, from_server) = tokio_mpsc::unbounded_channel();
        let socket = FakeSocket {
            rx: server_rx,
            tx: server_tx,
        };
        let session = tokio::spawn(crate::ksp::handle_test_socket_for(
            socket,
            Arc::clone(state),
            origin,
            population,
        ));
        Self {
            to_server: Some(to_server),
            from_server,
            session,
        }
    }

    fn send_raw(&self, text: String) {
        self.to_server
            .as_ref()
            .expect("still connected")
            .send(text)
            .expect("server socket open");
    }

    async fn recv_raw(&mut self) -> Option<String> {
        tokio::time::timeout(Duration::from_secs(10), self.from_server.recv())
            .await
            .expect("server frame within 10s")
    }

    async fn ended(mut self) {
        self.to_server.take();
        while self.recv_raw().await.is_some() {}
        let _ = tokio::time::timeout(Duration::from_secs(10), self.session).await;
    }
}

struct SealedSibling {
    sibling: Sibling,
    sender: kanna_secure_channel::Sender,
    receiver: kanna_secure_channel::Receiver,
}

impl SealedSibling {
    /// Opens a peer-domain session to `state` as `identity`, claiming to be
    /// `source_desktop_id`.
    async fn establish(
        state: &Arc<AppState>,
        origin: StreamOrigin,
        identity: &Keypair,
        source_desktop_id: &str,
        intent: HelloIntent,
        service: Option<&str>,
    ) -> Result<Self, String> {
        let hello = InitiatorHello {
            version: kanna_secure_channel::PROTOCOL_VERSION,
            intent,
            device_id: None,
            source_desktop_id: Some(source_desktop_id.into()),
            service: service.map(str::to_string),
            capabilities: vec![],
        };
        let mut sibling = Sibling::connect(state, origin, SealedPopulation::Peer);
        let (pending, message1) = PendingInitiator::start_in(
            Domain::Peer,
            identity,
            &peer_key(state),
            &state.config().desktop_id,
            &hello,
        )
        .unwrap();
        sibling.send_raw(message1);
        let reply = sibling.recv_raw().await.ok_or("no handshake reply")?;
        if !kanna_secure_channel::is_wire_frame(&reply) {
            return Err(reply);
        }
        let (channel, responder_hello) =
            pending.finish(&reply).map_err(|error| error.to_string())?;
        assert_eq!(responder_hello.desktop_id, state.config().desktop_id);
        let (sender, receiver) = channel.split();
        Ok(Self {
            sibling,
            sender,
            receiver,
        })
    }

    fn send_json(&mut self, json: serde_json::Value) {
        let wire = self.sender.seal(json.to_string().as_bytes()).unwrap();
        self.sibling.send_raw(wire);
    }

    async fn recv(&mut self) -> Option<Received> {
        loop {
            let raw = self.sibling.recv_raw().await?;
            let mut received = self.receiver.open(&raw).expect("open server frame");
            if received.is_empty() {
                continue;
            }
            return Some(received.remove(0));
        }
    }

    async fn recv_json(&mut self) -> serde_json::Value {
        loop {
            match self.recv().await {
                Some(Received::Message(bytes)) => {
                    let frame: serde_json::Value =
                        serde_json::from_slice(&bytes).expect("json frame");
                    if frame["type"] == "state_changed" {
                        continue;
                    }
                    return frame;
                }
                other => panic!("expected a sealed JSON frame, got {other:?}"),
            }
        }
    }

    async fn auth(&mut self) {
        self.send_json(serde_json::json!({ "type": "auth", "capabilities": [] }));
        let frame = self.recv_json().await;
        assert_eq!(frame["type"], "auth_ok", "{frame}");
    }

    async fn request(
        &mut self,
        id: u64,
        method: &str,
        path: &str,
        body: serde_json::Value,
    ) -> serde_json::Value {
        let mut frame =
            serde_json::json!({ "type": "request", "id": id, "method": method, "path": path });
        if !body.is_null() {
            frame["body"] = body;
        }
        self.send_json(frame);
        let response = self.recv_json().await;
        assert_eq!(response["type"], "response", "{response}");
        assert_eq!(response["id"], id);
        response
    }
}

/// A fully isolated desktop: its own pairing-store directory (so the peer
/// identity and trust store never collide with another desktop's) and a
/// transfer port bound to a fake sidecar the test owns.
fn isolated_config(label: &str, transfer_port: u16) -> Config {
    let dir = crate::test_paths::unique_test_dir(&format!("peer-{label}"));
    Config {
        relay_url: String::new(),
        device_token: "device-token".to_string(),
        firebase_project_id: "kanna-local".to_string(),
        firebase_auth_emulator_url: None,
        firebase_firestore_emulator_host: None,
        daemon_dir: dir.join("daemon").to_string_lossy().into_owned(),
        db_path: crate::db::Db::test_db_path(&format!("peer-{label}")),
        kanna_cli_path: None,
        desktop_id: format!("desktop-{label}"),
        desktop_secret: Some("desktop-secret".to_string()),
        desktop_name: format!("{label} Mac"),
        version: "test-version".to_string(),
        environment: "development".to_string(),
        lan_host: "127.0.0.1".to_string(),
        lan_port: 48120,
        transfer_port,
        lan_routing_port: 4460,
        activity_event_debounce_seconds: 300,
        pairing_store_path: dir.join("pairings.json").to_string_lossy().into_owned(),
    }
}

fn state(label: &str) -> Arc<AppState> {
    state_with_transfer_port(label, 4455)
}

fn state_with_transfer_port(label: &str, transfer_port: u16) -> Arc<AppState> {
    let config = isolated_config(label, transfer_port);
    let _ = crate::db::Db::open_for_tests(&config.db_path).expect("open test db");
    let state = Arc::new(AppState::new(config));
    state.peer_channel_identity().expect("peer identity");
    state.set_relay_access(RelayAccess::Enforced(entitlement(true)));
    state
}

fn entitlement(active: bool) -> RelayEntitlement {
    RelayEntitlement {
        active,
        status: if active { "active" } else { "grace" }.to_string(),
        current_period_ends_at: None,
        grace_ends_at: None,
        reason: None,
    }
}

/// The refusal a handler answered, for handlers whose success type has no
/// `Debug`.
fn refusal<T>(
    result: Result<T, (axum::http::StatusCode, String)>,
) -> (axum::http::StatusCode, String) {
    match result {
        Ok(_) => panic!("the call must be refused"),
        Err(refusal) => refusal,
    }
}

fn peer_key(state: &AppState) -> [u8; 32] {
    *state.peer_channel_identity().unwrap().public_key()
}

fn set_peer_legacy_refused(state: &AppState) {
    let db = crate::db::Db::open(&state.config().db_path).unwrap();
    db.set_setting(
        super::secure_channel::DESKTOP_PEER_LEGACY_ACCESS_SETTING,
        super::secure_channel::DESKTOP_PEER_LEGACY_ACCESS_REFUSED,
    )
    .unwrap();
}

/// Pins `identity` on `state` as the sibling `desktop_id`, as a completed
/// ceremony would have.
fn pin_peer(state: &AppState, desktop_id: &str, identity: &Keypair) {
    let path = state.config().peer_trust_store_path().unwrap();
    let mut store = PeerTrustStore::load(&path).unwrap();
    store
        .upsert(PeerDesktop {
            desktop_id: desktop_id.into(),
            display_name: format!("{desktop_id} Mac"),
            channel_public_key: identity.encoded_public_key(),
            transfer_peer_id: None,
            transfer_public_key: None,
            environment: "development".into(),
            account_uid: None,
            paired_at_unix_ms: 1,
            last_seen_unix_ms: None,
        })
        .unwrap();
    store.save(&path).unwrap();
}

#[tokio::test]
async fn a_phone_handshake_is_refused_at_the_peer_endpoint_and_vice_versa() {
    let state = state("domains");
    let stranger = Keypair::generate().unwrap();
    // A mobile-domain message 1 (against the *mobile* identity) at the
    // peer endpoint fails the prologue before any hello is read.
    let mobile_key = *state.secure_channel_identity().unwrap().public_key();
    let (_, message1) = PendingInitiator::start(
        &stranger,
        &mobile_key,
        &state.config().desktop_id,
        &InitiatorHello {
            version: kanna_secure_channel::PROTOCOL_VERSION,
            intent: HelloIntent::Session,
            device_id: Some("phone".into()),
            source_desktop_id: None,
            service: None,
            capabilities: vec![],
        },
    )
    .unwrap();
    let mut sibling = Sibling::connect(&state, StreamOrigin::Lan, SealedPopulation::Peer);
    sibling.send_raw(message1);
    let reply = sibling.recv_raw().await.expect("refusal");
    let frame: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(frame["code"], "secure_channel_refused", "{frame}");
    sibling.ended().await;

    // ...and a peer-domain message 1 at the phone endpoint likewise.
    let (_, message1) = PendingInitiator::start_in(
        Domain::Peer,
        &stranger,
        &peer_key(&state),
        &state.config().desktop_id,
        &InitiatorHello {
            version: kanna_secure_channel::PROTOCOL_VERSION,
            intent: HelloIntent::PeerSession,
            device_id: None,
            source_desktop_id: Some("desktop-x".into()),
            service: None,
            capabilities: vec![],
        },
    )
    .unwrap();
    let mut phone = Sibling::connect(&state, StreamOrigin::Lan, SealedPopulation::Mobile);
    phone.send_raw(message1);
    let reply = phone.recv_raw().await.expect("refusal");
    let frame: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(frame["code"], "secure_channel_refused", "{frame}");

    // The peer endpoint has no plaintext shape at all.
    let plain = Sibling::connect(&state, StreamOrigin::Lan, SealedPopulation::Peer);
    plain.send_raw(serde_json::json!({ "type": "auth" }).to_string());
    let mut plain = plain;
    let reply = plain.recv_raw().await.expect("refusal");
    let frame: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(frame["code"], "peer_channel_refused", "{frame}");
}

#[tokio::test]
async fn an_unknown_peer_key_gets_pairing_only_authority() {
    let state = state("pairing-only");
    let stranger = Keypair::generate().unwrap();
    let mut sibling = SealedSibling::establish(
        &state,
        StreamOrigin::Lan,
        &stranger,
        "desktop-stranger",
        HelloIntent::PeerSession,
        None,
    )
    .await
    .unwrap();
    sibling.auth().await;
    for (method, path) in [
        ("GET", "/v1/status"),
        ("GET", "/v1/tasks/recent"),
        ("POST", "/v1/pairing/sessions/claim"),
        ("GET", "/v1/peers/transfer-identity"),
        ("POST", "/v1/peers/pairing-offers"),
    ] {
        let response = sibling
            .request(1, method, path, serde_json::Value::Null)
            .await;
        assert_eq!(response["status"], 401, "{method} {path}: {response}");
    }
    // The claim route is reachable, and answers for the empty offer.
    let response = sibling
        .request(
            2,
            "POST",
            "/v1/peers/pairing/claim",
            serde_json::json!({
                "code": "AAAAAA", "secret": "B", "desktopId": "desktop-stranger",
                "desktopName": "S", "environment": "development"
            }),
        )
        .await;
    assert_eq!(response["status"], 409, "{response}");
    // A stream attach ends the connection.
    sibling.send_json(serde_json::json!({ "type": "attach", "task_id": "t", "kind": "terminal" }));
    let error = sibling.recv_json().await;
    assert_eq!(error["code"], "unauthorized");
    assert!(matches!(
        sibling.recv().await,
        Some(Received::Closed(_)) | None
    ));
    // A tunnel from an unpaired key is refused at the handshake.
    let refused = SealedSibling::establish(
        &state,
        StreamOrigin::Lan,
        &stranger,
        "desktop-stranger",
        HelloIntent::PeerTunnel,
        Some("task-transfer"),
    )
    .await;
    let reply = match refused {
        Ok(_) => panic!("a tunnel from an unpaired key must be refused"),
        Err(reply) => reply,
    };
    assert!(reply.contains("secure_channel_refused"), "{reply}");
}

#[tokio::test]
async fn a_paired_peer_gets_the_sibling_route_set_but_no_desktop_local_or_relay_attested_authority()
{
    let state = state("paired");
    let sibling_identity = Keypair::generate().unwrap();
    pin_peer(&state, "desktop-sibling", &sibling_identity);
    for origin in [StreamOrigin::Lan, StreamOrigin::RelayTunnel] {
        let mut sibling = SealedSibling::establish(
            &state,
            origin,
            &sibling_identity,
            "desktop-sibling",
            HelloIntent::PeerSession,
            None,
        )
        .await
        .unwrap();
        sibling.auth().await;
        let response = sibling
            .request(1, "GET", "/v1/status", serde_json::Value::Null)
            .await;
        assert_eq!(response["status"], 200, "{origin:?}: {response}");
        assert_eq!(response["body"]["desktopId"], state.config().desktop_id);
        assert_ne!(
            response["body"]["state"], "pairing_required",
            "{origin:?}: a sibling is an authenticated caller"
        );
        let response = sibling
            .request(2, "GET", "/v1/tasks/recent", serde_json::Value::Null)
            .await;
        assert_eq!(response["status"], 200, "{origin:?}: {response}");
        // Reachable (not 401): the sibling route set includes it; it is
        // 503 only because no sidecar runs in this test.
        let response = sibling
            .request(
                3,
                "GET",
                "/v1/peers/transfer-identity",
                serde_json::Value::Null,
            )
            .await;
        assert_eq!(response["status"], 503, "{origin:?}: {response}");
        // Desktop-local controls stay out of reach...
        for (method, path) in [
            ("POST", "/v1/peers/pairing-offers"),
            ("GET", "/v1/peers"),
            ("GET", "/v1/pairing/pending-confirmation"),
            ("POST", "/v1/pairing/pending-confirmation/confirm"),
        ] {
            let response = sibling
                .request(4, method, path, serde_json::Value::Null)
                .await;
            assert_eq!(
                response["status"], 401,
                "{origin:?} {method} {path}: {response}"
            );
        }
        // ...and so does the relay-attested CA bootstrap and the mobile
        // pairing claim.
        let response = sibling
            .request(
                5,
                "POST",
                "/v1/lan-routing/bootstrap",
                serde_json::json!({ "candidateSecret": "x" }),
            )
            .await;
        assert_eq!(response["status"], 401, "{origin:?}: {response}");
        let response = sibling
            .request(
                6,
                "POST",
                "/v1/pairing/sessions/claim",
                serde_json::json!({ "code": "A", "deviceId": "d", "deviceName": "n" }),
            )
            .await;
        assert_eq!(response["status"], 401, "{origin:?}: {response}");
        let response = sibling
            .request(
                7,
                "POST",
                "/v1/peers/pairing/claim",
                serde_json::json!({
                    "code": "AAAAAA", "secret": "B", "desktopId": "desktop-sibling",
                    "desktopName": "S", "environment": "development"
                }),
            )
            .await;
        assert_eq!(
            response["status"], 401,
            "{origin:?}: a paired sibling is not a pairing-only session: {response}"
        );
        sibling.sibling.ended().await;
    }
}

#[tokio::test]
async fn the_pairing_ceremony_pins_the_handshake_key_and_refuses_a_wrong_secret_or_code() {
    let issuer = state("issuer");
    let offer = super::peers::create_pairing_offer(DesktopLocalAccess, State(Arc::clone(&issuer)))
        .await
        .unwrap()
        .0;
    let parsed = crate::peer_pairing::parse_pairing_string(&offer.pairing_string).unwrap();
    assert_eq!(parsed.channel_public_key, peer_key(&issuer));
    assert_eq!(parsed.desktop_id, issuer.config().desktop_id);

    let claimant = Keypair::generate().unwrap();
    let mut sibling = SealedSibling::establish(
        &issuer,
        StreamOrigin::RelayTunnel,
        &claimant,
        "desktop-claimant",
        HelloIntent::PeerPairing,
        None,
    )
    .await
    .unwrap();
    sibling.auth().await;
    let claim = |code: &str, secret: &str| {
        serde_json::json!({
            "code": code, "secret": secret, "desktopId": "desktop-claimant",
            "desktopName": "Claimant Mac", "environment": "development",
            "transferIdentity": { "peerId": "peer-claimant", "publicKey": "tkey-claimant" }
        })
    };
    let response = sibling
        .request(
            1,
            "POST",
            "/v1/peers/pairing/claim",
            claim(&parsed.code, "WRONGSECRET"),
        )
        .await;
    assert_eq!(response["status"], 400, "{response}");
    let response = sibling
        .request(
            2,
            "POST",
            "/v1/peers/pairing/claim",
            claim("000000", &parsed.secret),
        )
        .await;
    assert_eq!(response["status"], 400, "{response}");
    assert!(
        issuer.peer_trust_store().unwrap().peers.is_empty(),
        "a failed claim pins nothing"
    );
    let response = sibling
        .request(
            3,
            "POST",
            "/v1/peers/pairing/claim",
            claim(&parsed.code, &parsed.secret),
        )
        .await;
    assert_eq!(response["status"], 200, "{response}");
    assert_eq!(response["body"]["desktopId"], issuer.config().desktop_id);
    assert_eq!(response["body"]["environment"], "development");
    let store = issuer.peer_trust_store().unwrap();
    let pinned = store
        .peer_by_channel_key(&claimant.encoded_public_key(), "development")
        .expect("the claimant's handshake key is pinned");
    assert_eq!(pinned.desktop_id, "desktop-claimant");
    assert_eq!(pinned.transfer_peer_id.as_deref(), Some("peer-claimant"));
    assert_eq!(pinned.transfer_public_key.as_deref(), Some("tkey-claimant"));
    // The offer is consumed.
    let response = sibling
        .request(
            4,
            "POST",
            "/v1/peers/pairing/claim",
            claim(&parsed.code, &parsed.secret),
        )
        .await;
    assert_eq!(response["status"], 409, "{response}");
    sibling.sibling.ended().await;

    // The next handshake with that key is a sibling session.
    let mut paired = SealedSibling::establish(
        &issuer,
        StreamOrigin::Lan,
        &claimant,
        "desktop-claimant",
        HelloIntent::PeerSession,
        None,
    )
    .await
    .unwrap();
    paired.auth().await;
    let response = paired
        .request(1, "GET", "/v1/status", serde_json::Value::Null)
        .await;
    assert_eq!(response["status"], 200, "{response}");
    // A hello declaring a different desktop id than the pin is refused.
    let mismatch = SealedSibling::establish(
        &issuer,
        StreamOrigin::Lan,
        &claimant,
        "desktop-impostor",
        HelloIntent::PeerSession,
        None,
    )
    .await;
    let mismatch = match mismatch {
        Ok(_) => panic!("a declared id that disagrees with the pin must be refused"),
        Err(reply) => reply,
    };
    assert!(mismatch.contains("secure_channel_refused"), "{mismatch}");
    // The list shows the peer as end-to-end encrypted.
    let list = super::peers::list_peers(DesktopLocalAccess, State(Arc::clone(&issuer)))
        .await
        .unwrap()
        .0;
    let view = serde_json::to_value(&list).unwrap();
    assert_eq!(view["peers"][0]["desktopId"], "desktop-claimant");
    assert_eq!(view["peers"][0]["encryption"], "e2ee");
    assert_eq!(view["peers"][0]["transferIdentityPinned"], true);
}

/// Re-pairing is how a rotated key on either side becomes trusted again: a
/// `peer_pairing` hello from a key this desktop already pins gets
/// pairing-only authority (not sibling authority), and its claim replaces
/// the record.
#[tokio::test]
async fn a_pairing_hello_from_an_already_pinned_key_is_pairing_only_and_replaces_the_record() {
    let issuer = state("repair");
    let claimant = Keypair::generate().unwrap();
    pin_peer(&issuer, "desktop-claimant", &claimant);
    let offer = super::peers::create_pairing_offer(DesktopLocalAccess, State(Arc::clone(&issuer)))
        .await
        .unwrap()
        .0;
    let parsed = crate::peer_pairing::parse_pairing_string(&offer.pairing_string).unwrap();
    let mut sibling = SealedSibling::establish(
        &issuer,
        StreamOrigin::Lan,
        &claimant,
        "desktop-claimant",
        HelloIntent::PeerPairing,
        None,
    )
    .await
    .unwrap();
    sibling.auth().await;
    // Pairing-only: the sibling route set is not reachable on this session.
    let response = sibling
        .request(1, "GET", "/v1/status", serde_json::Value::Null)
        .await;
    assert_eq!(response["status"], 401, "{response}");
    let response = sibling
        .request(
            2,
            "POST",
            "/v1/peers/pairing/claim",
            serde_json::json!({
                "code": parsed.code, "secret": parsed.secret, "desktopId": "desktop-claimant",
                "desktopName": "Renamed Claimant", "environment": "development"
            }),
        )
        .await;
    assert_eq!(response["status"], 200, "{response}");
    let store = issuer.peer_trust_store().unwrap();
    assert_eq!(store.peers.len(), 1);
    assert_eq!(store.peers[0].display_name, "Renamed Claimant");
    assert_eq!(
        store.peers[0].channel_public_key,
        claimant.encoded_public_key()
    );
}

#[tokio::test]
async fn the_pairing_string_binds_its_audience() {
    let issuer = state("audience");
    let offer = super::peers::create_pairing_offer(DesktopLocalAccess, State(Arc::clone(&issuer)))
        .await
        .unwrap()
        .0;
    let parsed = crate::peer_pairing::parse_pairing_string(&offer.pairing_string).unwrap();
    let claimant = Keypair::generate().unwrap();
    let mut sibling = SealedSibling::establish(
        &issuer,
        StreamOrigin::Lan,
        &claimant,
        "desktop-claimant",
        HelloIntent::PeerPairing,
        None,
    )
    .await
    .unwrap();
    sibling.auth().await;
    // A claim naming a desktop other than the handshake declared.
    let response = sibling
        .request(
            1,
            "POST",
            "/v1/peers/pairing/claim",
            serde_json::json!({
                "code": parsed.code, "secret": parsed.secret, "desktopId": "desktop-other",
                "desktopName": "Other", "environment": "development"
            }),
        )
        .await;
    assert_eq!(response["status"], 400, "{response}");
    // A claim from another environment.
    let response = sibling
        .request(
            2,
            "POST",
            "/v1/peers/pairing/claim",
            serde_json::json!({
                "code": parsed.code, "secret": parsed.secret, "desktopId": "desktop-claimant",
                "desktopName": "Claimant", "environment": "staging"
            }),
        )
        .await;
    assert_eq!(response["status"], 400, "{response}");
    assert!(
        issuer.peer_pairing_offer.lock().await.is_some(),
        "neither spent the offer"
    );
    assert!(issuer.peer_trust_store().unwrap().peers.is_empty());
}

#[tokio::test]
async fn a_tampered_or_replayed_frame_ends_the_peer_session() {
    let state = state("tamper");
    let sibling_identity = Keypair::generate().unwrap();
    pin_peer(&state, "desktop-sibling", &sibling_identity);
    let mut sibling = SealedSibling::establish(
        &state,
        StreamOrigin::Lan,
        &sibling_identity,
        "desktop-sibling",
        HelloIntent::PeerSession,
        None,
    )
    .await
    .unwrap();
    sibling.auth().await;
    let wire = sibling
        .sender
        .seal(serde_json::json!({ "type": "request", "id": 9, "method": "GET", "path": "/v1/status" }).to_string().as_bytes())
        .unwrap();
    sibling.sibling.send_raw(wire.clone());
    let response = sibling.recv_json().await;
    assert_eq!(response["status"], 200);
    // Replay: the same frame again is refused and the session is dead.
    sibling.sibling.send_raw(wire);
    let after = tokio::time::timeout(Duration::from_secs(5), sibling.sibling.session).await;
    assert!(after.is_ok(), "the session task ends on a replayed frame");
}

#[tokio::test]
async fn unpairing_closes_live_sessions_and_the_next_handshake_is_pairing_only() {
    let state = state("revoke");
    let sibling_identity = Keypair::generate().unwrap();
    pin_peer(&state, "desktop-sibling", &sibling_identity);
    let mut sibling = SealedSibling::establish(
        &state,
        StreamOrigin::Lan,
        &sibling_identity,
        "desktop-sibling",
        HelloIntent::PeerSession,
        None,
    )
    .await
    .unwrap();
    sibling.auth().await;
    let status = super::peers::remove_peer(
        DesktopLocalAccess,
        State(Arc::clone(&state)),
        axum::extract::Path("desktop-sibling".into()),
    )
    .await
    .unwrap();
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT);
    let close = loop {
        match sibling.recv().await {
            Some(Received::Closed(reason)) => break reason,
            Some(Received::Message(_)) => continue,
            None => panic!("the session ended without an authenticated close"),
        }
    };
    assert_eq!(close, "peer revoked");
    let mut again = SealedSibling::establish(
        &state,
        StreamOrigin::Lan,
        &sibling_identity,
        "desktop-sibling",
        HelloIntent::PeerSession,
        None,
    )
    .await
    .unwrap();
    again.auth().await;
    let response = again
        .request(1, "GET", "/v1/status", serde_json::Value::Null)
        .await;
    assert_eq!(response["status"], 401, "{response}");
    let missing = super::peers::remove_peer(
        DesktopLocalAccess,
        State(Arc::clone(&state)),
        axum::extract::Path("desktop-sibling".into()),
    )
    .await;
    assert_eq!(missing.unwrap_err().0, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn relay_origin_peer_sessions_are_gated_by_relay_account_access() {
    let state = state("relay-access");
    let sibling_identity = Keypair::generate().unwrap();
    pin_peer(&state, "desktop-sibling", &sibling_identity);
    state.set_relay_access(RelayAccess::Enforced(entitlement(false)));
    let mut sibling = SealedSibling::establish(
        &state,
        StreamOrigin::RelayTunnel,
        &sibling_identity,
        "desktop-sibling",
        HelloIntent::PeerSession,
        None,
    )
    .await
    .unwrap();
    sibling.auth().await;
    let response = sibling
        .request(1, "GET", "/v1/status", serde_json::Value::Null)
        .await;
    assert_eq!(response["status"], 402, "{response}");
    state.set_relay_access(RelayAccess::Unknown);
    let response = sibling
        .request(2, "GET", "/v1/status", serde_json::Value::Null)
        .await;
    assert_eq!(response["status"], 402, "{response}");
    // The same session on the LAN is not gated.
    let mut lan = SealedSibling::establish(
        &state,
        StreamOrigin::Lan,
        &sibling_identity,
        "desktop-sibling",
        HelloIntent::PeerSession,
        None,
    )
    .await
    .unwrap();
    lan.auth().await;
    let response = lan
        .request(1, "GET", "/v1/status", serde_json::Value::Null)
        .await;
    assert_eq!(response["status"], 200, "{response}");
}

#[tokio::test]
async fn a_transfer_identity_is_pinned_once_and_a_rotated_one_is_refused() {
    let state = state("transfer-pin");
    let sibling_identity = Keypair::generate().unwrap();
    pin_peer(&state, "desktop-sibling", &sibling_identity);
    let identity = crate::peer_pairing::PeerTransferIdentity {
        peer_id: "peer-sib".into(),
        public_key: "tkey-sib".into(),
    };
    let pinned = super::peers::pin_transfer_identity(&state, "desktop-sibling", &identity)
        .await
        .unwrap();
    assert_eq!(pinned.transfer_public_key.as_deref(), Some("tkey-sib"));
    super::peers::pin_transfer_identity(&state, "desktop-sibling", &identity)
        .await
        .expect("the same identity again is fine");
    let rotated = crate::peer_pairing::PeerTransferIdentity {
        peer_id: "peer-sib".into(),
        public_key: "tkey-rotated".into(),
    };
    let error = super::peers::pin_transfer_identity(&state, "desktop-sibling", &rotated)
        .await
        .unwrap_err();
    assert!(error.contains("differs from the pinned one"), "{error}");
    assert_eq!(
        state
            .paired_peer("desktop-sibling")
            .unwrap()
            .transfer_public_key
            .as_deref(),
        Some("tkey-sib")
    );
    assert!(
        super::peers::pin_transfer_identity(&state, "desktop-unknown", &identity)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn the_legacy_gate_refuses_every_plaintext_sibling_path_when_off() {
    let state = state("gate");
    assert!(state.legacy_peer_access_allowed(), "default on");
    set_peer_legacy_refused(&state);
    assert!(!state.legacy_peer_access_allowed());

    // The relay-attested CA bootstrap.
    let bootstrap = super::lan_bootstrap::bootstrap_lan_trust(
        super::lan_trust::RelayAttestedSource {
            source_desktop_id: "desktop-sibling".into(),
            account_uid: "uid-1".into(),
        },
        State(Arc::clone(&state)),
        axum::Json(serde_json::from_value(serde_json::json!({ "candidateSecret": "s" })).unwrap()),
    )
    .await;
    let (status, message) = refusal(bootstrap);
    assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
    assert!(
        message.starts_with("peer_legacy_access_refused"),
        "{message}"
    );

    // The bearer-secret LAN machine-invoke listener.
    let refused = super::lan_listener::handle_invoke_for_test(
        "desktop-sibling",
        State(Arc::clone(&state)),
        serde_json::json!({ "method": "GET", "path": "/v1/status", "body": null }),
    )
    .await;
    let (status, message) = refusal(refused);
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
    assert!(
        message.starts_with("peer_legacy_access_refused"),
        "{message}"
    );

    // The renderer's Firestore-keyed external peers, the sidecar's mDNS
    // pairing, and the renderer-credentialed cloud transfer proxy.
    for operation in ["upsert-external-peer", "start-pairing", "accept-pairing"] {
        let refused = super::transfer_sidecar::run_transfer_control(
            DesktopLocalAccess,
            State(Arc::clone(&state)),
            axum::extract::Path(operation.into()),
            None,
        )
        .await;
        let (status, message) = refusal(refused);
        assert_eq!(status, axum::http::StatusCode::FORBIDDEN, "{operation}");
        assert!(
            message.starts_with("peer_legacy_access_refused"),
            "{message}"
        );
    }
    let refused = super::transfer_sidecar::ensure_cloud_transfer_proxy(
        DesktopLocalAccess,
        State(Arc::clone(&state)),
        axum::Json(
            serde_json::from_value(serde_json::json!({
                "peerId": "p", "desktopId": "d", "relayUrl": "ws://127.0.0.1:1", "idToken": "t"
            }))
            .unwrap(),
        ),
    )
    .await;
    let (status, _) = refusal(refused);
    assert_eq!(status, axum::http::StatusCode::FORBIDDEN);

    // The sidecar is spawned loopback-only with discovery disabled.
    std::env::set_var(
        "KANNA_TRANSFER_ROOT",
        crate::test_paths::unique_test_path_string("gate-root"),
    );
    std::env::set_var("KANNA_TRANSFER_PEER_ID", "peer-gate");
    std::env::set_var("KANNA_TRANSFER_DISPLAY_NAME", "Gate");
    let env = crate::transfer_sidecar::build_transfer_sidecar_env(state.config()).unwrap();
    assert!(env.contains(&(
        "KANNA_TRANSFER_DISCOVERY".to_string(),
        "disabled".to_string()
    )));

    // An unpaired sibling cannot be invoked at all.
    let error = super::invoke_desktop::invoke_desktop(
        Arc::clone(&state),
        "desktop-unpaired".into(),
        "GET".into(),
        "/v1/status".into(),
        serde_json::Value::Null,
    )
    .await
    .unwrap_err();
    assert!(error.starts_with("peer_pairing_required"), "{error}");
    // The machine list says so.
    let machines =
        super::cloud_desktops::list_cloud_desktops(DesktopLocalAccess, State(Arc::clone(&state)))
            .await
            .0;
    let view = serde_json::to_value(&machines).unwrap();
    assert_eq!(view["machines"][0]["encryption"], "local");
}

/// A TCP tap between two ends that records every byte in both directions,
/// so a test can assert what an on-path observer could see.
async fn start_tap(upstream: SocketAddr) -> (SocketAddr, Arc<std::sync::Mutex<Vec<u8>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = Arc::clone(&observed);
    tokio::spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else {
                break;
            };
            let Ok(mut server) = tokio::net::TcpStream::connect(upstream).await else {
                break;
            };
            let recorder = Arc::clone(&recorder);
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let (mut client_read, mut client_write) = client.split();
                let (mut server_read, mut server_write) = server.split();
                let up = {
                    let recorder = Arc::clone(&recorder);
                    async move {
                        let mut buffer = [0u8; 8192];
                        loop {
                            let Ok(count) = client_read.read(&mut buffer).await else {
                                break;
                            };
                            if count == 0 {
                                break;
                            }
                            recorder.lock().unwrap().extend_from_slice(&buffer[..count]);
                            if server_write.write_all(&buffer[..count]).await.is_err() {
                                break;
                            }
                        }
                        let _ = server_write.shutdown().await;
                    }
                };
                let down = async move {
                    let mut buffer = [0u8; 8192];
                    loop {
                        let Ok(count) = server_read.read(&mut buffer).await else {
                            break;
                        };
                        if count == 0 {
                            break;
                        }
                        recorder.lock().unwrap().extend_from_slice(&buffer[..count]);
                        if client_write.write_all(&buffer[..count]).await.is_err() {
                            break;
                        }
                    }
                    let _ = client_write.shutdown().await;
                };
                tokio::join!(up, down);
            });
        }
    });
    (address, observed)
}

/// Two real servers on loopback: A's pooled peer session and its sealed
/// transfer tunnel reach B's served router through a recording tap, and a
/// marker carried in a request, a response and a transfer byte stream
/// never appears on the wire.
#[tokio::test]
async fn a_real_sealed_invoke_and_transfer_tunnel_cross_loopback_without_a_plaintext_marker() {
    let fake_sidecar = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let transfer_port = fake_sidecar.local_addr().unwrap().port();
    let desktop_b = state_with_transfer_port("real-b", transfer_port);
    desktop_b
        .transfer_sidecar()
        .assume_externally_owned_for_test();
    let desktop_a = state("real-a");
    // Pair both ways, as a completed ceremony leaves them, with B's
    // transfer identity pinned on A.
    let a_identity = desktop_a.peer_channel_identity().unwrap();
    let b_identity = desktop_b.peer_channel_identity().unwrap();
    // A pins B first; B has not pinned A yet.
    pin_peer(&desktop_a, &desktop_b.config().desktop_id, &b_identity);
    super::peers::pin_transfer_identity(
        &desktop_a,
        &desktop_b.config().desktop_id,
        &crate::peer_pairing::PeerTransferIdentity {
            peer_id: "peer-b".into(),
            public_key: "tkey-b".into(),
        },
    )
    .await
    .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let served = listener.local_addr().unwrap();
    let router = super::router(Arc::clone(&desktop_b));
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    let (tap, observed) = start_tap(served).await;
    desktop_a.set_lan_api_candidate(desktop_b.config().desktop_id.clone(), tap);

    // 0. B answers the handshake but grants pairing-only authority, which
    // A refuses at the handshake rather than pooling: nothing sibling-shaped
    // ever rides a session the other side did not grant.
    let refused = super::invoke_desktop::invoke_desktop(
        Arc::clone(&desktop_a),
        desktop_b.config().desktop_id.clone(),
        "GET".into(),
        "/v1/status".into(),
        serde_json::Value::Null,
    )
    .await
    .expect_err("B has not pinned A");
    assert!(refused.starts_with("peer_pairing_required"), "{refused}");
    pin_peer(&desktop_b, &desktop_a.config().desktop_id, &a_identity);

    // 1. A pooled sealed invoke, route peer-lan.
    let marker = "MARKER-INVOKE-7f3a9c";
    let routed = super::invoke_desktop::invoke_desktop(
        Arc::clone(&desktop_a),
        desktop_b.config().desktop_id.clone(),
        "GET".into(),
        format!("/v1/tasks/search?query={marker}"),
        serde_json::Value::Null,
    )
    .await
    .unwrap_or_else(|error| panic!("sealed invoke failed: {error}"));
    assert_eq!(routed.route.as_str(), "peer-lan");
    assert_eq!(routed.response.status, 200, "{:?}", routed.response);
    // A second invoke reuses the pooled session.
    let status = super::invoke_desktop::invoke_desktop(
        Arc::clone(&desktop_a),
        desktop_b.config().desktop_id.clone(),
        "GET".into(),
        "/v1/status".into(),
        serde_json::Value::Null,
    )
    .await
    .unwrap_or_else(|error| panic!("second sealed invoke failed: {error}"));
    assert_eq!(status.response.status, 200);
    assert_eq!(
        status.response.body.as_ref().unwrap()["desktopId"],
        desktop_b.config().desktop_id
    );

    // 2. A sealed transfer tunnel: A's sidecar-facing loopback proxy to B's
    // sidecar port, through the tap.
    let route = desktop_a
        .peer_transfer_proxies()
        .ensure_for_peer(
            &desktop_a,
            &desktop_a
                .paired_peer(&desktop_b.config().desktop_id)
                .unwrap(),
        )
        .await
        .unwrap();
    let transfer_marker = b"MARKER-TRANSFER-c41d0e\n";
    let mut sidecar_side = tokio::net::TcpStream::connect(&route.endpoint)
        .await
        .unwrap();
    let b_sidecar = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = fake_sidecar.accept().await.unwrap();
        let mut line = vec![0u8; transfer_marker.len()];
        socket.read_exact(&mut line).await.unwrap();
        socket.write_all(b"REPLY-MARKER-5b2e\n").await.unwrap();
        socket.shutdown().await.unwrap();
        line
    });
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        sidecar_side.write_all(transfer_marker).await.unwrap();
        let mut reply = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(10),
            sidecar_side.read_to_end(&mut reply),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(reply, b"REPLY-MARKER-5b2e\n");
    }
    assert_eq!(b_sidecar.await.unwrap(), transfer_marker);

    // 3. Nothing an on-path observer saw contains any marker, and every
    // frame after the upgrade is a sealed one.
    let wire = observed.lock().unwrap().clone();
    let wire_text = String::from_utf8_lossy(&wire);
    assert!(
        !wire_text.contains(marker),
        "invoke marker leaked on the wire"
    );
    assert!(
        !wire_text.contains("MARKER-TRANSFER"),
        "transfer marker leaked on the wire"
    );
    assert!(
        !wire_text.contains("REPLY-MARKER"),
        "transfer reply leaked on the wire"
    );
    assert!(
        !wire_text.contains("\"type\":\"request\""),
        "a plaintext KSP frame crossed the wire"
    );
    assert!(
        wire_text.contains("ksc1:"),
        "sealed frames must have crossed the wire"
    );
    // The transfer targets list carries the sealed route as a ready cloud
    // route bound to B's machine id.
    let routes = desktop_a.peer_transfer_proxies().routes().await;
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].transfer_peer_id, "peer-b");
    assert_eq!(routes[0].desktop_id, desktop_b.config().desktop_id);
}

#[test]
fn peer_sessions_and_sockets_report_their_route_names() {
    assert_eq!(crate::peer_channel::PeerRoute::Lan.as_str(), "peer-lan");
    assert_eq!(crate::peer_channel::PeerRoute::Relay.as_str(), "peer-relay");
}
