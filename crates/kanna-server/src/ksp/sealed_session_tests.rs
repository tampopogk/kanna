//! Negative-security and authority tests for secure-channel KSP sessions,
//! driven through the same socket runner production uses, with a phone
//! played by `kanna_secure_channel`'s initiator side.

use super::*;
use crate::http_api::secure_channel::{
    StreamOrigin, MOBILE_LEGACY_ACCESS_REFUSED, MOBILE_LEGACY_ACCESS_SETTING,
};
use crate::http_api::test_state_with_seed;
use crate::pairing::{self as pairing_domain, PairingStore};
use kanna_secure_channel::{HelloIntent, InitiatorHello, Keypair, PendingInitiator, Received};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc as tokio_mpsc;

/// One end of an in-memory WebSocket: text frames in, text frames out.
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

/// The phone's end: raw text frames plus, once sealed, the channel halves.
struct Phone {
    to_server: Option<tokio_mpsc::UnboundedSender<String>>,
    from_server: tokio_mpsc::UnboundedReceiver<String>,
    session: JoinHandle<()>,
}

impl Phone {
    fn connect(state: &Arc<AppState>, origin: StreamOrigin) -> Self {
        let (to_server, server_rx) = tokio_mpsc::unbounded_channel();
        let (server_tx, from_server) = tokio_mpsc::unbounded_channel();
        let socket = FakeSocket {
            rx: server_rx,
            tx: server_tx,
        };
        let session = tokio::spawn(handle_test_socket(socket, Arc::clone(state), origin));
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

    /// Hang up from the phone's side: the server's reader sees end of stream.
    fn disconnect(&mut self) {
        self.to_server.take();
    }

    async fn recv_raw(&mut self) -> Option<String> {
        tokio::time::timeout(Duration::from_secs(10), self.from_server.recv())
            .await
            .expect("server frame within 10s")
    }

    async fn ended(mut self) {
        self.disconnect();
        while self.recv_raw().await.is_some() {}
        let _ = tokio::time::timeout(Duration::from_secs(10), self.session).await;
    }
}

struct SealedPhone {
    phone: Phone,
    #[allow(dead_code)]
    established: bool,
    sender: kanna_secure_channel::Sender,
    receiver: kanna_secure_channel::Receiver,
    handshake_hash: [u8; 32],
    sas: String,
}

impl SealedPhone {
    async fn establish(
        state: &Arc<AppState>,
        origin: StreamOrigin,
        identity: &Keypair,
        desktop_key: &[u8; 32],
        hello: InitiatorHello,
    ) -> Result<Self, String> {
        let mut phone = Phone::connect(state, origin);
        let (pending, message1) =
            PendingInitiator::start(identity, desktop_key, &state.config().desktop_id, &hello)
                .unwrap();
        phone.send_raw(message1);
        let reply = phone.recv_raw().await.ok_or("no handshake reply")?;
        if !kanna_secure_channel::is_wire_frame(&reply) {
            return Err(reply);
        }
        let (channel, responder_hello) =
            pending.finish(&reply).map_err(|error| error.to_string())?;
        assert_eq!(responder_hello.desktop_id, state.config().desktop_id);
        let handshake_hash = *channel.handshake_hash();
        let sas = channel.sas_code();
        let (sender, receiver) = channel.split();
        Ok(Self {
            phone,
            established: true,
            sender,
            receiver,
            handshake_hash,
            sas,
        })
    }

    fn send_json(&mut self, json: serde_json::Value) {
        let wire = self.sender.seal(json.to_string().as_bytes()).unwrap();
        self.phone.send_raw(wire);
    }

    async fn recv(&mut self) -> Option<Received> {
        loop {
            let raw = self.phone.recv_raw().await?;
            let mut received = self.receiver.open(&raw).expect("open server frame");
            if received.is_empty() {
                continue;
            }
            return Some(received.remove(0));
        }
    }

    /// The next sealed JSON frame that is not a broadcast (`state_changed`
    /// fan-out reaches every authenticated session and is not what a test
    /// asked for).
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

fn session_hello(device_id: &str) -> InitiatorHello {
    InitiatorHello {
        version: kanna_secure_channel::PROTOCOL_VERSION,
        intent: HelloIntent::Session,
        device_id: Some(device_id.into()),
        capabilities: vec![],
    }
}

fn pairing_hello() -> InitiatorHello {
    InitiatorHello {
        version: kanna_secure_channel::PROTOCOL_VERSION,
        intent: HelloIntent::Pairing,
        device_id: None,
        capabilities: vec![],
    }
}

fn state(label: &str) -> Arc<AppState> {
    let state = test_state_with_seed(&format!("desktop-sealed-{label}"), "Sealed Desktop", |_| {});
    state.secure_channel_identity().expect("channel identity");
    state
}

fn desktop_key(state: &AppState) -> [u8; 32] {
    *state.secure_channel_identity().unwrap().public_key()
}

fn set_legacy_refused(state: &AppState) {
    let db = crate::db::Db::open(&state.config().db_path).unwrap();
    db.set_setting(MOBILE_LEGACY_ACCESS_SETTING, MOBILE_LEGACY_ACCESS_REFUSED)
        .unwrap();
}

/// Starts a pairing session the way the desktop UI does and returns the
/// code and the QR-only secret.
async fn start_pairing(state: &Arc<AppState>) -> (String, String) {
    let active = pairing_domain::create_active_pairing_session_with_channel(
        state.config(),
        &desktop_key(state),
    )
    .unwrap();
    let code = active.session.code.clone();
    let qr_secret = active.qr_secret.clone().unwrap();
    assert!(active.session.pairing_payload.starts_with("KANNA2:"));
    *state.pairing_session.lock().await = Some(active);
    (code, qr_secret)
}

/// Pairs `identity` as `device_id` over a sealed QR-anchored claim.
async fn pair_by_qr(state: &Arc<AppState>, identity: &Keypair, device_id: &str) {
    let (code, qr_secret) = start_pairing(state).await;
    let mut phone = SealedPhone::establish(
        state,
        StreamOrigin::Lan,
        identity,
        &desktop_key(state),
        pairing_hello(),
    )
    .await
    .unwrap();
    phone.auth().await;
    let response = phone
        .request(
            1,
            "POST",
            "/v1/pairing/sessions/claim",
            serde_json::json!({ "code": code, "deviceId": device_id, "deviceName": "Test Phone", "qrSecret": qr_secret }),
        )
        .await;
    assert_eq!(response["status"], 200, "{response}");
    assert_eq!(response["body"]["secureChannel"], true, "{response}");
    assert_eq!(response["body"]["desktopId"], state.config().desktop_id);
    let store = PairingStore::load(Path::new(&state.config().pairing_store_path)).unwrap();
    let device = store
        .device_by_channel_key(&state.config().desktop_id, &identity.encoded_public_key())
        .expect("device registered under its channel key");
    assert_eq!(device.device_id, device_id);
    phone.phone.ended().await;
}

#[tokio::test]
async fn an_unknown_static_key_gets_pairing_only_authority() {
    let state = state("pairing-only");
    let identity = Keypair::generate().unwrap();
    let mut phone = SealedPhone::establish(
        &state,
        StreamOrigin::Lan,
        &identity,
        &desktop_key(&state),
        session_hello("stranger"),
    )
    .await
    .unwrap();
    phone.auth().await;
    // Nothing but the pairing requests is routable...
    let response = phone
        .request(1, "GET", "/v1/tasks/recent", serde_json::Value::Null)
        .await;
    assert_eq!(response["status"], 401, "{response}");
    let response = phone
        .request(2, "GET", "/v1/status", serde_json::Value::Null)
        .await;
    assert_eq!(response["status"], 401, "{response}");
    // ...and a stream attach ends the connection.
    phone.send_json(serde_json::json!({ "type": "attach", "task_id": "t", "kind": "terminal" }));
    let error = phone.recv_json().await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["code"], "unauthorized");
    assert!(matches!(
        phone.recv().await,
        Some(Received::Closed(_)) | None
    ));
}

#[tokio::test]
async fn a_qr_anchored_sealed_claim_pairs_the_handshake_key_and_grants_device_authority() {
    let state = state("qr-claim");
    let identity = Keypair::generate().unwrap();
    pair_by_qr(&state, &identity, "phone-qr").await;

    for origin in [StreamOrigin::Lan, StreamOrigin::RelayTunnel] {
        let mut phone = SealedPhone::establish(
            &state,
            origin,
            &identity,
            &desktop_key(&state),
            session_hello("phone-qr"),
        )
        .await
        .unwrap();
        phone.auth().await;
        let response = phone
            .request(1, "GET", "/v1/status", serde_json::Value::Null)
            .await;
        assert_eq!(response["status"], 200, "{origin:?}: {response}");
        assert_eq!(response["body"]["desktopId"], state.config().desktop_id);
        assert!(response["body"]["channelPublicKey"].is_string());
        // LAN-paired authority, not desktop-local authority: the desktop's
        // own pairing controls stay out of reach.
        let response = phone
            .request(
                2,
                "POST",
                "/v1/pairing/pending-confirmation/confirm",
                serde_json::Value::Null,
            )
            .await;
        assert_eq!(response["status"], 401, "{origin:?}: {response}");
        let response = phone
            .request(
                3,
                "GET",
                "/v1/pairing/pending-confirmation",
                serde_json::Value::Null,
            )
            .await;
        assert_eq!(response["status"], 401, "{origin:?}: {response}");
        // The relay-only same-account bootstrap is not a paired phone's either.
        let response = phone
            .request(
                4,
                "POST",
                "/v1/lan-routing/bootstrap",
                serde_json::json!({}),
            )
            .await;
        assert_eq!(response["status"], 401, "{origin:?}: {response}");
        phone.phone.ended().await;
    }
}

#[tokio::test]
async fn a_typed_code_sealed_claim_waits_for_the_desktop_sas_confirmation() {
    let state = state("typed-claim");
    let identity = Keypair::generate().unwrap();
    let (code, _qr_secret) = start_pairing(&state).await;
    let mut phone = SealedPhone::establish(
        &state,
        StreamOrigin::Lan,
        &identity,
        &desktop_key(&state),
        pairing_hello(),
    )
    .await
    .unwrap();
    phone.auth().await;
    let response = phone
        .request(
            1,
            "POST",
            "/v1/pairing/sessions/claim",
            serde_json::json!({ "code": code, "deviceId": "phone-typed", "deviceName": "Typed Phone" }),
        )
        .await;
    assert_eq!(response["status"], 202, "{response}");
    assert_eq!(response["body"]["status"], "confirmation_required");
    // The code is consumed: a second claim finds no session.
    assert!(state.pairing_session.lock().await.is_none());
    // Nothing is persisted until the person confirms.
    let store = PairingStore::load(Path::new(&state.config().pairing_store_path)).unwrap();
    assert!(!store.is_trusted(&state.config().desktop_id, "phone-typed"));
    let pending = state
        .pairing_confirmation
        .current()
        .await
        .expect("pending confirmation");
    assert_eq!(pending.device_name, "Typed Phone");
    assert_eq!(pending.sas, phone.sas, "both screens show the same SAS");
    assert_eq!(pending.handshake_hash, phone.handshake_hash);
    assert_eq!(pending.channel_public_key, identity.encoded_public_key());

    // The person confirms on the desktop; the phone's poll resolves.
    let confirmed = state
        .pairing_confirmation
        .confirm(state.config(), &state.pairing_persistence_mutation)
        .await
        .unwrap();
    assert!(confirmed.secure_channel);
    let response = phone
        .request(
            2,
            "GET",
            "/v1/pairing/confirmation",
            serde_json::Value::Null,
        )
        .await;
    assert_eq!(response["status"], 200, "{response}");
    assert_eq!(response["body"]["secureChannel"], true);
    assert_eq!(
        response["body"]["deviceSecret"],
        serde_json::Value::String(confirmed.device_secret.clone())
    );
    let store = PairingStore::load(Path::new(&state.config().pairing_store_path)).unwrap();
    assert_eq!(
        store
            .device_by_channel_key(&state.config().desktop_id, &identity.encoded_public_key())
            .map(|device| device.device_id.as_str()),
        Some("phone-typed")
    );
    // Handed out once.
    let response = phone
        .request(
            3,
            "GET",
            "/v1/pairing/confirmation",
            serde_json::Value::Null,
        )
        .await;
    assert_eq!(response["status"], 410, "{response}");
    assert!(state.pairing_confirmation.current().await.is_none());
}

#[tokio::test]
async fn a_rejected_or_abandoned_typed_code_pairing_persists_nothing() {
    let state = state("typed-reject");
    let identity = Keypair::generate().unwrap();
    let (code, _) = start_pairing(&state).await;
    let mut phone = SealedPhone::establish(
        &state,
        StreamOrigin::Lan,
        &identity,
        &desktop_key(&state),
        pairing_hello(),
    )
    .await
    .unwrap();
    phone.auth().await;
    let response = phone
        .request(
            1,
            "POST",
            "/v1/pairing/sessions/claim",
            serde_json::json!({ "code": code, "deviceId": "p", "deviceName": "P" }),
        )
        .await;
    assert_eq!(response["status"], 202, "{response}");
    assert!(state.pairing_confirmation.reject().await);
    let response = phone
        .request(
            2,
            "GET",
            "/v1/pairing/confirmation",
            serde_json::Value::Null,
        )
        .await;
    assert_eq!(response["status"], 403, "{response}");
    let store = PairingStore::load(Path::new(&state.config().pairing_store_path)).unwrap();
    assert!(!store.is_trusted(&state.config().desktop_id, "p"));
    // A second phone claims and then disappears before anyone confirms.
    let (code, _) = start_pairing(&state).await;
    let identity2 = Keypair::generate().unwrap();
    let mut phone2 = SealedPhone::establish(
        &state,
        StreamOrigin::Lan,
        &identity2,
        &desktop_key(&state),
        pairing_hello(),
    )
    .await
    .unwrap();
    phone2.auth().await;
    let response = phone2
        .request(
            1,
            "POST",
            "/v1/pairing/sessions/claim",
            serde_json::json!({ "code": code, "deviceId": "q", "deviceName": "Q" }),
        )
        .await;
    assert_eq!(response["status"], 202, "{response}");
    assert!(state.pairing_confirmation.current().await.is_some());
    phone2.phone.disconnect();
    phone2.phone.ended().await;
    assert!(
        state.pairing_confirmation.current().await.is_none(),
        "a confirmation bound to a dead session must not survive it"
    );
    assert!(state
        .pairing_confirmation
        .confirm(state.config(), &state.pairing_persistence_mutation)
        .await
        .is_err());
}

#[tokio::test]
async fn a_typed_code_claim_is_refused_over_the_relay() {
    let state = state("typed-relay");
    let identity = Keypair::generate().unwrap();
    let (code, _) = start_pairing(&state).await;
    let mut phone = SealedPhone::establish(
        &state,
        StreamOrigin::RelayTunnel,
        &identity,
        &desktop_key(&state),
        pairing_hello(),
    )
    .await
    .unwrap();
    phone.auth().await;
    let response = phone
        .request(
            1,
            "POST",
            "/v1/pairing/sessions/claim",
            serde_json::json!({ "code": code, "deviceId": "p", "deviceName": "P" }),
        )
        .await;
    assert_eq!(response["status"], 400, "{response}");
    assert!(state.pairing_confirmation.current().await.is_none());
}

#[tokio::test]
async fn a_wrong_qr_secret_counts_as_a_failed_claim() {
    let state = state("wrong-qr-secret");
    let identity = Keypair::generate().unwrap();
    let (code, _) = start_pairing(&state).await;
    let mut phone = SealedPhone::establish(
        &state,
        StreamOrigin::Lan,
        &identity,
        &desktop_key(&state),
        pairing_hello(),
    )
    .await
    .unwrap();
    phone.auth().await;
    let response = phone
        .request(
            1,
            "POST",
            "/v1/pairing/sessions/claim",
            serde_json::json!({ "code": code, "deviceId": "p", "deviceName": "P", "qrSecret": "NOTTHESECRET" }),
        )
        .await;
    assert_eq!(response["status"], 400, "{response}");
    assert_eq!(
        state
            .pairing_session
            .lock()
            .await
            .as_ref()
            .unwrap()
            .failed_claims,
        1
    );
    assert!(state.pairing_confirmation.current().await.is_none());
}

#[tokio::test]
async fn a_handshake_against_the_wrong_desktop_key_is_refused_in_the_clear() {
    let state = state("wrong-key");
    let identity = Keypair::generate().unwrap();
    let impostor_desktop = Keypair::generate().unwrap();
    let Err(error) = SealedPhone::establish(
        &state,
        StreamOrigin::Lan,
        &identity,
        impostor_desktop.public_key(),
        pairing_hello(),
    )
    .await
    else {
        panic!("must not establish");
    };
    let frame: serde_json::Value = serde_json::from_str(&error).expect("plaintext error frame");
    assert_eq!(frame["type"], "error");
    assert_eq!(frame["code"], "secure_channel_refused");
}

#[tokio::test]
async fn a_hello_naming_another_devices_id_is_refused() {
    let state = state("device-id-mismatch");
    let identity = Keypair::generate().unwrap();
    pair_by_qr(&state, &identity, "phone-real").await;
    let Err(error) = SealedPhone::establish(
        &state,
        StreamOrigin::Lan,
        &identity,
        &desktop_key(&state),
        session_hello("phone-other"),
    )
    .await
    else {
        panic!("must not establish");
    };
    let frame: serde_json::Value = serde_json::from_str(&error).unwrap();
    assert_eq!(frame["code"], "secure_channel_refused");
}

#[tokio::test]
async fn a_tampered_or_replayed_frame_ends_the_session_without_a_response() {
    let state = state("tamper");
    let identity = Keypair::generate().unwrap();
    pair_by_qr(&state, &identity, "phone-tamper").await;

    let mut phone = SealedPhone::establish(
        &state,
        StreamOrigin::Lan,
        &identity,
        &desktop_key(&state),
        session_hello("phone-tamper"),
    )
    .await
    .unwrap();
    phone.auth().await;
    let genuine = phone
        .sender
        .seal(serde_json::json!({ "type": "request", "id": 7, "method": "GET", "path": "/v1/status" }).to_string().as_bytes())
        .unwrap();
    let mut tampered = genuine.clone();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'A' { 'B' } else { 'A' });
    phone.phone.send_raw(tampered);
    // The server ends the session: its authenticated close arrives, then
    // nothing - the genuine frame, replayed now, is never answered.
    let closing = phone.recv().await;
    assert!(
        matches!(closing, Some(Received::Closed(_)) | None),
        "{closing:?}"
    );
    // The server has hung up; whether the genuine frame is still deliverable
    // or the socket is already gone, it is never answered.
    if let Some(to_server) = phone.phone.to_server.as_ref() {
        let _ = to_server.send(genuine);
    }
    assert!(phone.phone.recv_raw().await.is_none());

    let mut phone = SealedPhone::establish(
        &state,
        StreamOrigin::Lan,
        &identity,
        &desktop_key(&state),
        session_hello("phone-tamper"),
    )
    .await
    .unwrap();
    phone.auth().await;
    let wire = phone
        .sender
        .seal(serde_json::json!({ "type": "request", "id": 1, "method": "GET", "path": "/v1/status" }).to_string().as_bytes())
        .unwrap();
    phone.phone.send_raw(wire.clone());
    let response = phone.recv_json().await;
    assert_eq!(response["status"], 200);
    phone.phone.send_raw(wire);
    let closing = phone.recv().await;
    assert!(
        matches!(closing, Some(Received::Closed(_)) | None),
        "replay must end the session: {closing:?}"
    );
}

#[tokio::test]
async fn revoking_a_device_closes_its_live_session_and_the_next_handshake_is_unpaired() {
    let state = state("revoke");
    let identity = Keypair::generate().unwrap();
    pair_by_qr(&state, &identity, "phone-revoke").await;
    let mut phone = SealedPhone::establish(
        &state,
        StreamOrigin::RelayTunnel,
        &identity,
        &desktop_key(&state),
        session_hello("phone-revoke"),
    )
    .await
    .unwrap();
    phone.auth().await;
    assert!(state.remove_trusted_device("phone-revoke").await.unwrap());
    assert_eq!(
        phone.recv().await,
        Some(Received::Closed("device revoked".into()))
    );

    let mut again = SealedPhone::establish(
        &state,
        StreamOrigin::Lan,
        &identity,
        &desktop_key(&state),
        pairing_hello(),
    )
    .await
    .unwrap();
    again.auth().await;
    let response = again
        .request(1, "GET", "/v1/status", serde_json::Value::Null)
        .await;
    assert_eq!(
        response["status"], 401,
        "revoked key must be pairing-only: {response}"
    );
}

#[tokio::test]
async fn a_plaintext_relay_tunnel_is_admitted_only_while_legacy_access_is_allowed() {
    let state = state("legacy-tunnel");
    let mut phone = Phone::connect(&state, StreamOrigin::RelayTunnel);
    phone.send_raw(serde_json::json!({ "type": "tunnel_ready" }).to_string());
    phone.send_raw(serde_json::json!({ "type": "auth", "capabilities": [] }).to_string());
    let frame: serde_json::Value = serde_json::from_str(&phone.recv_raw().await.unwrap()).unwrap();
    assert_eq!(frame["type"], "auth_ok", "default: legacy allowed");
    phone.disconnect();
    phone.ended().await;

    set_legacy_refused(&state);
    let mut phone = Phone::connect(&state, StreamOrigin::RelayTunnel);
    phone.send_raw(serde_json::json!({ "type": "auth", "capabilities": [] }).to_string());
    let frame: serde_json::Value = serde_json::from_str(&phone.recv_raw().await.unwrap()).unwrap();
    assert_eq!(frame["type"], "error");
    assert_eq!(frame["code"], "legacy_access_refused");
    assert!(
        phone.recv_raw().await.is_none(),
        "the refused tunnel is closed"
    );

    // Sealed sessions are unaffected by the switch.
    let identity = Keypair::generate().unwrap();
    let mut sealed = SealedPhone::establish(
        &state,
        StreamOrigin::RelayTunnel,
        &identity,
        &desktop_key(&state),
        pairing_hello(),
    )
    .await
    .unwrap();
    sealed.auth().await;
}

#[tokio::test]
async fn legacy_bearer_credentials_stop_verifying_when_legacy_access_is_off() {
    let state = state("legacy-bearer");
    // Pair legacy-style (plaintext claim) so the device has a bearer secret.
    let active = pairing_domain::create_active_pairing_session(state.config()).unwrap();
    let code = active.session.code.clone();
    *state.pairing_session.lock().await = Some(active);
    let claimed = {
        let mut active = state.pairing_session.lock().await;
        pairing_domain::claim_pairing_session(
            state.config(),
            &mut active,
            pairing_domain::PairingClaimRequest {
                code,
                device_id: "legacy-phone".into(),
                device_name: "Legacy".into(),
                qr_secret: None,
            },
        )
        .unwrap()
    };
    assert!(!claimed.secure_channel);

    let credential =
        serde_json::json!({ "deviceId": "legacy-phone", "deviceSecret": claimed.device_secret })
            .to_string();
    let mut phone = Phone::connect(&state, StreamOrigin::Lan);
    phone.send_raw(
        serde_json::json!({ "type": "auth", "credential": credential, "capabilities": [] })
            .to_string(),
    );
    let frame: serde_json::Value = serde_json::from_str(&phone.recv_raw().await.unwrap()).unwrap();
    assert_eq!(
        frame["type"], "auth_ok",
        "legacy allowed by default: {frame}"
    );
    phone.disconnect();
    phone.ended().await;

    set_legacy_refused(&state);
    let mut phone = Phone::connect(&state, StreamOrigin::Lan);
    phone.send_raw(
        serde_json::json!({ "type": "auth", "credential": credential, "capabilities": [] })
            .to_string(),
    );
    let frame: serde_json::Value = serde_json::from_str(&phone.recv_raw().await.unwrap()).unwrap();
    assert_eq!(frame["type"], "error", "{frame}");
    assert_eq!(frame["code"], "unauthorized");

    // The same bearer as HTTP headers no longer marks the request trusted.
    let router = crate::http_api::router(Arc::clone(&state));
    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/v1/tasks/recent")
        .header("x-kanna-device-id", "legacy-phone")
        .header("x-kanna-device-secret", claimed.device_secret.clone())
        .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [192, 168, 1, 20],
            5000,
        ))))
        .body(axum::body::Body::empty())
        .unwrap();
    let response = tower::ServiceExt::oneshot(router, request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);

    // And the plaintext claim route is closed.
    let active = pairing_domain::create_active_pairing_session(state.config()).unwrap();
    let code = active.session.code.clone();
    *state.pairing_session.lock().await = Some(active);
    let router = crate::http_api::router(Arc::clone(&state));
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/pairing/sessions/claim")
        .header("content-type", "application/json")
        .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [192, 168, 1, 20],
            5000,
        ))))
        .body(axum::body::Body::from(
            serde_json::json!({ "code": code, "deviceId": "new-legacy", "deviceName": "N" })
                .to_string(),
        ))
        .unwrap();
    let response = tower::ServiceExt::oneshot(router, request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_relay_invoke_cannot_claim_a_pairing_code() {
    let state = state("relay-claim");
    let active = pairing_domain::create_active_pairing_session(state.config()).unwrap();
    let code = active.session.code.clone();
    *state.pairing_session.lock().await = Some(active);
    let response = crate::http_api::dispatch_authenticated_relay_http_invoke(
        Arc::clone(&state),
        "user-1".into(),
        None,
        "POST",
        "/v1/pairing/sessions/claim",
        serde_json::json!({ "code": code, "deviceId": "relay-phone", "deviceName": "R" }),
    )
    .await;
    assert_eq!(response.status, 401, "{response:?}");
    let store = PairingStore::load(Path::new(&state.config().pairing_store_path)).unwrap();
    assert!(!store.is_trusted(&state.config().desktop_id, "relay-phone"));
}

#[tokio::test]
async fn sealed_requests_are_never_visible_on_the_socket() {
    let state = state("opaque");
    let identity = Keypair::generate().unwrap();
    pair_by_qr(&state, &identity, "phone-opaque").await;
    let mut phone = SealedPhone::establish(
        &state,
        StreamOrigin::RelayTunnel,
        &identity,
        &desktop_key(&state),
        session_hello("phone-opaque"),
    )
    .await
    .unwrap();
    // Everything the phone sends is a ksc1 frame that does not contain its
    // plaintext, and every server frame is likewise opaque.
    let auth = phone
        .sender
        .seal(br#"{"type":"auth","capabilities":["term_input_boundary"]}"#)
        .unwrap();
    assert!(auth.starts_with("ksc1:") && !auth.contains("auth"));
    phone.phone.send_raw(auth);
    let raw = phone.phone.recv_raw().await.unwrap();
    assert!(
        raw.starts_with("ksc1:") && !raw.contains("auth_ok"),
        "{raw}"
    );
    let opened = phone.receiver.open(&raw).unwrap();
    assert!(
        matches!(&opened[0], Received::Message(bytes) if bytes.starts_with(br#"{"type":"auth_ok""#))
    );
}

/// The relay-tunnel path is a `tokio-tungstenite` client socket the desktop
/// dialled. Drive `handle_tungstenite_stream` over a real WebSocket pair so
/// the runner's first-frame wait is exercised on the same stream type
/// production uses - the in-memory socket above cannot catch a stream-type
/// specific early return.
#[tokio::test]
async fn a_relay_tunnel_socket_waits_for_the_phone_and_admits_a_sealed_session() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let state = state("tungstenite-tunnel");
    let identity = Keypair::generate().unwrap();
    pair_by_qr(&state, &identity, "phone-tunnel").await;

    // The "relay": accepts the desktop's dial and then relays the phone's
    // frames, which the test writes by hand after a deliberate pause.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let relay = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        // Nothing arrives for a while: the desktop must keep waiting.
        tokio::time::sleep(Duration::from_millis(300)).await;
        socket
            .send(Message::Text(
                serde_json::json!({ "type": "tunnel_ready" }).to_string().into(),
            ))
            .await
            .unwrap();
        socket
    });
    let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
        .await
        .unwrap();
    let desktop_state = Arc::clone(&state);
    let session = tokio::spawn(async move {
        handle_tungstenite_stream(socket, desktop_state, StreamOrigin::RelayTunnel).await;
    });
    let mut relay_socket = relay.await.unwrap();
    assert!(
        !session.is_finished(),
        "the desktop dropped the tunnel before the phone sent anything"
    );

    let (pending, message1) = PendingInitiator::start(
        &identity,
        &desktop_key(&state),
        &state.config().desktop_id,
        &session_hello("phone-tunnel"),
    )
    .unwrap();
    relay_socket.send(Message::Text(message1.into())).await.unwrap();
    let reply = match relay_socket.next().await {
        Some(Ok(Message::Text(text))) => text.to_string(),
        other => panic!("expected the handshake reply, got {other:?}"),
    };
    let (channel, _) = pending.finish(&reply).expect("desktop answered the handshake");
    let (mut tx, mut rx) = channel.split();
    relay_socket
        .send(Message::Text(
            tx.seal(br#"{"type":"auth","capabilities":[]}"#).unwrap().into(),
        ))
        .await
        .unwrap();
    let raw = match relay_socket.next().await {
        Some(Ok(Message::Text(text))) => text.to_string(),
        other => panic!("expected a sealed frame, got {other:?}"),
    };
    let opened = rx.open(&raw).unwrap();
    assert!(matches!(&opened[0], Received::Message(bytes) if bytes.starts_with(br#"{"type":"auth_ok""#)));
    drop(relay_socket);
    let _ = tokio::time::timeout(Duration::from_secs(10), session).await;
}
