//! Outbound sealed sessions to paired sibling desktops - the only way this
//! process talks to another desktop once legacy routing is off.
//!
//! One channel type, two outer routes. A [`dial_peer`] opens a WebSocket to
//! the sibling - its `/v1/peers/channel` endpoint at the address LAN
//! discovery last observed, or a relay tunnel opened with this desktop's own
//! desktop-secret credential when no LAN candidate answers - and then runs
//! the `kanna-ksc-peer` initiator handshake against the sibling's *pinned*
//! peer channel key. The transport is only ever where to try; the handshake
//! is what proves who answered, and a pinned peer never falls back to a
//! plaintext route: the failure is reported as `peer_pairing_required` (no
//! pin) or `peer_upgrade_required` (a pin but no peer handshake answer).
//!
//! What rides inside is decided by the hello intent: `peer_session` carries
//! KSP frames (server-originated invokes through the pooled [`PeerSessions`],
//! or a renderer's own sibling view spliced 1:1 by the loopback proxy in
//! `http_api::peers`), and `peer_tunnel` carries the raw bytes of a task
//! transfer (`peer_transfer_proxy`). Every (re)connect is a fresh handshake
//! with fresh ephemerals.

use crate::http_api::{AppState, HttpInvokeResponse};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use kanna_secure_channel::{Domain, HelloIntent, InitiatorHello, PendingInitiator, Received};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex, Semaphore};
use tokio::time::{timeout, Duration};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// How long a LAN candidate gets to accept the WebSocket before the relay
/// route is tried instead.
const LAN_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the relay gets to open a tunnel.
const RELAY_TUNNEL_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the sibling gets to answer message 1 - the same budget the
/// phone gives a desktop.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the sibling gets to answer the KSP `auth` frame.
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
/// Mirrors the relay path's own budget: a remote task-event wait may occupy
/// almost the MCP client's whole 240 s window.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(270);

pub(crate) type PeerWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Responder-hello capability: the session was granted sibling authority.
pub(crate) const PEER_SESSION_CAPABILITY: &str = "peer-session";
/// Responder-hello capability: the session may only claim a pairing string.
pub(crate) const PEER_PAIRING_ONLY_CAPABILITY: &str = "peer-pairing-only";

/// The outer route a sealed peer connection took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeerRoute {
    Lan,
    Relay,
}

impl PeerRoute {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Lan => "peer-lan",
            Self::Relay => "peer-relay",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PeerDialError {
    /// The sibling is not a paired peer of this desktop.
    PairingRequired,
    /// This desktop is not (yet) a paired peer of the sibling: it answered
    /// the handshake but granted only pairing authority.
    NotPairedBySibling,
    /// This desktop's own peer channel identity is unusable.
    IdentityUnavailable(String),
    /// A pinned peer answered, but not with a peer handshake: an older
    /// Kanna, or one whose peer channel is unavailable.
    UpgradeRequired(String),
    /// The handshake against the pinned key failed: a rotated key or an
    /// impostor answered.
    IdentityMismatch(String),
    /// No route reached the sibling at all.
    Unreachable(String),
    /// The pooled session was retired before the request was sent; the
    /// pool re-dials once and the error never leaves this module.
    SessionEnded,
}

impl PeerDialError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::PairingRequired | Self::NotPairedBySibling => "peer_pairing_required",
            Self::IdentityUnavailable(_) => "peer_identity_unavailable",
            Self::UpgradeRequired(_) => "peer_upgrade_required",
            Self::IdentityMismatch(_) => "peer_identity_mismatch",
            Self::Unreachable(_) | Self::SessionEnded => "peer_unreachable",
        }
    }
}

impl std::fmt::Display for PeerDialError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PairingRequired => formatter.write_str(
                "this desktop is not paired with that machine; pair it from Preferences → Machines",
            ),
            Self::NotPairedBySibling => formatter.write_str(
                "that machine has not paired this desktop (its pin may be stale); pair the machines again",
            ),
            Self::IdentityUnavailable(detail) => {
                write!(formatter, "peer secure channel unavailable: {detail}")
            }
            Self::UpgradeRequired(detail) => write!(
                formatter,
                "the paired machine did not answer the peer handshake (it may need a newer Kanna): {detail}"
            ),
            Self::IdentityMismatch(detail) => write!(
                formatter,
                "the paired machine's identity changed; remove it and pair again: {detail}"
            ),
            Self::Unreachable(detail) => write!(formatter, "peer unreachable: {detail}"),
            Self::SessionEnded => formatter.write_str("peer unreachable: the sealed session ended"),
        }
    }
}

/// What the initiator is opening the session for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PeerHello {
    Pairing,
    Session,
    Tunnel { service: String },
}

impl PeerHello {
    fn initiator_hello(&self, source_desktop_id: &str) -> InitiatorHello {
        let (intent, service) = match self {
            Self::Pairing => (HelloIntent::PeerPairing, None),
            Self::Session => (HelloIntent::PeerSession, None),
            Self::Tunnel { service } => (HelloIntent::PeerTunnel, Some(service.clone())),
        };
        InitiatorHello {
            version: kanna_secure_channel::PROTOCOL_VERSION,
            intent,
            device_id: None,
            source_desktop_id: Some(source_desktop_id.to_string()),
            service,
            capabilities: vec!["ksp".into()],
        }
    }
}

/// Sending half of an established sealed peer connection.
pub(crate) struct SealedPeerWriter {
    sink: SplitSink<PeerWebSocket, Message>,
    sender: kanna_secure_channel::Sender,
}

impl SealedPeerWriter {
    pub(crate) async fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        let wire = self.sender.seal(bytes).map_err(|error| error.to_string())?;
        self.sink
            .send(Message::Text(wire.into()))
            .await
            .map_err(|error| format!("peer socket send failed: {error}"))
    }

    /// An authenticated close, then the WebSocket close.
    pub(crate) async fn close(mut self, reason: &str) {
        if let Ok(wire) = self.sender.seal_close(reason) {
            let _ = self.sink.send(Message::Text(wire.into())).await;
        }
        let _ = self.sink.close().await;
    }
}

/// Receiving half of an established sealed peer connection.
pub(crate) struct SealedPeerReader {
    stream: SplitStream<PeerWebSocket>,
    receiver: kanna_secure_channel::Receiver,
    queued: std::collections::VecDeque<Vec<u8>>,
}

impl SealedPeerReader {
    /// The next logical message; `None` once the peer closed (authenticated
    /// or not) - any failure to open a frame is terminal and reported as an
    /// error, after which nothing more is delivered.
    pub(crate) async fn next(&mut self) -> Result<Option<Vec<u8>>, String> {
        loop {
            if let Some(message) = self.queued.pop_front() {
                return Ok(Some(message));
            }
            let Some(frame) = self.stream.next().await else {
                return Ok(None);
            };
            let frame = frame.map_err(|error| format!("peer socket failed: {error}"))?;
            let text = match frame {
                Message::Text(text) => text.to_string(),
                Message::Close(_) => return Ok(None),
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
                Message::Binary(_) => {
                    return Err("peer sent a binary frame outside the sealed channel".into())
                }
            };
            if !kanna_secure_channel::is_wire_frame(&text) {
                // Relay tunnel control chatter (`tunnel_ready`) is the one
                // legitimate plaintext frame; anything else is refused.
                if crate::ksp::is_relay_tunnel_control_message(&text) {
                    continue;
                }
                return Err("peer sent a plaintext frame on a sealed session".into());
            }
            let received = self
                .receiver
                .open(&text)
                .map_err(|error| format!("sealed peer session ended: {error}"))?;
            for item in received {
                match item {
                    Received::Message(bytes) => self.queued.push_back(bytes),
                    Received::Closed(reason) => {
                        log::info!("[peer] sibling closed the sealed session: {reason:?}");
                        return Ok(self.queued.pop_front());
                    }
                }
            }
        }
    }
}

/// An established, authenticated sealed connection to a sibling.
pub(crate) struct SealedPeerSocket {
    pub(crate) route: PeerRoute,
    /// The LAN candidate that was tried and failed before the relay route
    /// was taken, if any. A pooled relay session is upgraded to a LAN one
    /// only once discovery offers a *different* candidate, so an
    /// unreachable address is not retried on every invoke.
    pub(crate) failed_lan_candidate: Option<SocketAddr>,
    ws: PeerWebSocket,
    sender: kanna_secure_channel::Sender,
    receiver: kanna_secure_channel::Receiver,
}

impl SealedPeerSocket {
    pub(crate) fn split(self) -> (SealedPeerWriter, SealedPeerReader) {
        let (sink, stream) = self.ws.split();
        (
            SealedPeerWriter {
                sink,
                sender: self.sender,
            },
            SealedPeerReader {
                stream,
                receiver: self.receiver,
                queued: Default::default(),
            },
        )
    }
}

/// Dials the paired sibling `desktop_id` with the key pinned for it.
pub(crate) async fn dial_peer(
    state: &Arc<AppState>,
    desktop_id: &str,
    hello: PeerHello,
) -> Result<SealedPeerSocket, PeerDialError> {
    let peer = state
        .paired_peer(desktop_id)
        .ok_or(PeerDialError::PairingRequired)?;
    let pinned = kanna_secure_channel::decode_key(&peer.channel_public_key)
        .map_err(|error| PeerDialError::IdentityMismatch(error.to_string()))?;
    dial_peer_with_key(state, desktop_id, pinned, hello).await
}

/// Dials `desktop_id` against an explicitly pinned key - the pairing
/// ceremony, where the key came from the pasted string and no record
/// exists yet.
pub(crate) async fn dial_peer_with_key(
    state: &Arc<AppState>,
    desktop_id: &str,
    pinned_key: [u8; 32],
    hello: PeerHello,
) -> Result<SealedPeerSocket, PeerDialError> {
    let identity = state
        .peer_channel_identity()
        .map_err(PeerDialError::IdentityUnavailable)?;
    let (ws, route, failed_lan_candidate) = connect_transport(state, desktop_id).await?;
    let mut socket = complete_handshake(
        ws,
        route,
        &identity,
        pinned_key,
        desktop_id,
        &state.config().desktop_id,
        hello,
    )
    .await?;
    socket.failed_lan_candidate = failed_lan_candidate;
    Ok(socket)
}

async fn connect_transport(
    state: &Arc<AppState>,
    desktop_id: &str,
) -> Result<(PeerWebSocket, PeerRoute, Option<SocketAddr>), PeerDialError> {
    let mut failures = Vec::new();
    let mut failed_lan_candidate = None;
    if let Some(candidate) = state.lan_api_candidate_for(desktop_id) {
        let url = format!("ws://{candidate}/v1/peers/channel");
        match timeout(LAN_CONNECT_TIMEOUT, tokio_tungstenite::connect_async(&url)).await {
            Ok(Ok((ws, _))) => return Ok((ws, PeerRoute::Lan, None)),
            Ok(Err(error)) => failures.push(format!("LAN {candidate}: {error}")),
            Err(_) => failures.push(format!("LAN {candidate}: connect timed out")),
        }
        failed_lan_candidate = Some(candidate);
    } else {
        failures.push("no LAN candidate discovered".to_string());
    }
    let config = state.config();
    if config.relay_url.trim().is_empty() {
        failures.push("no relay configured".to_string());
    } else if config.desktop_secret.is_none() {
        failures.push("relay route needs this desktop's cloud credential (sign in)".to_string());
    } else if !state.desktop_tunnel_available() {
        failures.push(
            "the relay does not offer desktop peer tunnels yet (relay upgrade required)"
                .to_string(),
        );
    } else {
        match timeout(
            RELAY_TUNNEL_TIMEOUT,
            crate::relay_client::connect_desktop_tunnel_client(config, desktop_id),
        )
        .await
        {
            Ok(Ok(ws)) => return Ok((ws, PeerRoute::Relay, failed_lan_candidate)),
            Ok(Err(error)) => failures.push(format!("relay: {error}")),
            Err(_) => failures.push("relay: tunnel setup timed out".to_string()),
        }
    }
    Err(PeerDialError::Unreachable(failures.join("; ")))
}

async fn complete_handshake(
    mut ws: PeerWebSocket,
    route: PeerRoute,
    identity: &kanna_secure_channel::Keypair,
    pinned_key: [u8; 32],
    desktop_id: &str,
    source_desktop_id: &str,
    hello: PeerHello,
) -> Result<SealedPeerSocket, PeerDialError> {
    let (pending, message1) = PendingInitiator::start_in(
        Domain::Peer,
        identity,
        &pinned_key,
        desktop_id,
        &hello.initiator_hello(source_desktop_id),
    )
    .map_err(|error| PeerDialError::IdentityUnavailable(error.to_string()))?;
    ws.send(Message::Text(message1.into()))
        .await
        .map_err(|error| PeerDialError::Unreachable(format!("handshake send failed: {error}")))?;
    let reply = timeout(HANDSHAKE_TIMEOUT, async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    if crate::ksp::is_relay_tunnel_control_message(&text) {
                        continue;
                    }
                    return Ok(text.to_string());
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
                Some(Ok(Message::Binary(_))) => {
                    return Err("binary frame instead of a handshake reply".to_string())
                }
                Some(Ok(Message::Close(frame))) => {
                    return Err(format!(
                        "socket closed before the handshake reply: {frame:?}"
                    ))
                }
                Some(Err(error)) => return Err(format!("socket failed: {error}")),
                None => return Err("socket ended before the handshake reply".to_string()),
            }
        }
    })
    .await
    .map_err(|_| PeerDialError::UpgradeRequired("no handshake reply within 10 s".into()))?
    .map_err(PeerDialError::UpgradeRequired)?;
    if !kanna_secure_channel::is_wire_frame(&reply) {
        // A plaintext refusal: either the sibling has no peer channel (older
        // Kanna, or its identity failed to load) or it refused the
        // handshake, which against a pinned key means the key is not the
        // one it holds.
        let code = serde_json::from_str::<serde_json::Value>(&reply)
            .ok()
            .and_then(|value| {
                value
                    .get("code")
                    .and_then(|code| code.as_str())
                    .map(str::to_string)
            });
        let _ = ws.close(None).await;
        return Err(match code.as_deref() {
            Some("secure_channel_refused") => PeerDialError::IdentityMismatch(reply),
            _ => PeerDialError::UpgradeRequired(reply),
        });
    }
    let (channel, responder_hello) = match pending.finish(&reply) {
        Ok(established) => established,
        Err(error) => {
            let _ = ws.close(None).await;
            return Err(PeerDialError::IdentityMismatch(error.to_string()));
        }
    };
    if responder_hello.desktop_id != desktop_id {
        let _ = ws.close(None).await;
        return Err(PeerDialError::IdentityMismatch(format!(
            "responder identified as {} instead of {desktop_id}",
            responder_hello.desktop_id
        )));
    }
    let wants_sibling_authority = !matches!(hello, PeerHello::Pairing);
    let granted_sibling_authority = responder_hello
        .capabilities
        .iter()
        .any(|capability| capability == PEER_SESSION_CAPABILITY);
    if wants_sibling_authority && !granted_sibling_authority {
        let _ = ws.close(None).await;
        return Err(PeerDialError::NotPairedBySibling);
    }
    let (sender, receiver) = channel.split();
    Ok(SealedPeerSocket {
        route,
        failed_lan_candidate: None,
        ws,
        sender,
        receiver,
    })
}

/// What one pooled invoke resolved to. Mirrors `invoke_desktop`'s LAN
/// contract: a definite response propagates unchanged, and a request that
/// was handed to the session but never answered is *uncertain* - the
/// sibling may have applied it - and must never be retried on another
/// route.
#[derive(Debug)]
pub(crate) enum PeerInvokeOutcome {
    Definite(HttpInvokeResponse),
    Uncertain,
}

struct PeerSession {
    desktop_id: String,
    route: PeerRoute,
    failed_lan_candidate: Option<SocketAddr>,
    outbound: mpsc::Sender<String>,
    pending: Mutex<HashMap<u64, oneshot::Sender<HttpInvokeResponse>>>,
    next_id: AtomicU64,
    alive: Arc<AtomicBool>,
    short: Arc<Semaphore>,
    long_poll: Arc<Semaphore>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl PeerSession {
    async fn establish(
        state: &Arc<AppState>,
        desktop_id: &str,
    ) -> Result<Arc<Self>, PeerDialError> {
        let socket = dial_peer(state, desktop_id, PeerHello::Session).await?;
        let route = socket.route;
        let failed_lan_candidate = socket.failed_lan_candidate;
        let (mut writer, mut reader) = socket.split();
        let (outbound, mut outbound_rx) = mpsc::channel::<String>(256);
        let alive = Arc::new(AtomicBool::new(true));
        let session = Arc::new(Self {
            desktop_id: desktop_id.to_string(),
            route,
            failed_lan_candidate,
            outbound,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            alive: Arc::clone(&alive),
            short: Arc::new(Semaphore::new(crate::ksp::request_concurrency())),
            long_poll: Arc::new(Semaphore::new(crate::ksp::request_concurrency())),
            tasks: Mutex::new(Vec::new()),
        });
        let (authed_tx, authed_rx) = oneshot::channel::<Result<(), String>>();
        let writer_alive = Arc::clone(&alive);
        let writer_task = tokio::spawn(async move {
            while let Some(frame) = outbound_rx.recv().await {
                if writer.send(frame.as_bytes()).await.is_err() {
                    break;
                }
            }
            writer_alive.store(false, Ordering::Relaxed);
            writer.close("session closed").await;
        });
        let reader_session = Arc::clone(&session);
        let reader_task = tokio::spawn(async move {
            let mut authed_tx = Some(authed_tx);
            loop {
                let message = match reader.next().await {
                    Ok(Some(message)) => message,
                    Ok(None) => break,
                    Err(error) => {
                        log::warn!(
                            "[peer] sealed session to {} ended: {error}",
                            reader_session.desktop_id
                        );
                        break;
                    }
                };
                let Ok(frame) = serde_json::from_slice::<serde_json::Value>(&message) else {
                    continue;
                };
                match frame.get("type").and_then(|value| value.as_str()) {
                    Some("auth_ok") => {
                        if let Some(sender) = authed_tx.take() {
                            let _ = sender.send(Ok(()));
                        }
                    }
                    Some("response") => {
                        let Some(id) = frame.get("id").and_then(|id| id.as_u64()) else {
                            continue;
                        };
                        let status = frame
                            .get("status")
                            .and_then(|status| status.as_u64())
                            .unwrap_or(0) as u16;
                        let body = frame.get("body").cloned().filter(|body| !body.is_null());
                        let error = body
                            .as_ref()
                            .and_then(|body| body.get("error"))
                            .and_then(|error| error.as_str())
                            .map(str::to_string)
                            .filter(|_| !(200..300).contains(&status));
                        if let Some(waiter) = reader_session.pending.lock().await.remove(&id) {
                            let _ = waiter.send(HttpInvokeResponse {
                                status,
                                body,
                                error,
                            });
                        }
                    }
                    Some("error") => {
                        if let Some(sender) = authed_tx.take() {
                            let _ = sender.send(Err(frame
                                .get("message")
                                .and_then(|message| message.as_str())
                                .unwrap_or("peer refused the session")
                                .to_string()));
                        }
                    }
                    _ => {}
                }
            }
            reader_session.alive.store(false, Ordering::Relaxed);
            // Anything still waiting was handed to the sibling and never
            // answered: uncertain, never retried.
            reader_session.pending.lock().await.clear();
        });
        session
            .tasks
            .lock()
            .await
            .extend([writer_task, reader_task]);
        let auth = serde_json::json!({ "type": "auth", "capabilities": [] }).to_string();
        session
            .outbound
            .send(auth)
            .await
            .map_err(|_| PeerDialError::Unreachable("peer session writer stopped".into()))?;
        match timeout(AUTH_TIMEOUT, authed_rx).await {
            Ok(Ok(Ok(()))) => Ok(session),
            Ok(Ok(Err(message))) => {
                session.shutdown().await;
                Err(PeerDialError::UpgradeRequired(message))
            }
            Ok(Err(_)) | Err(_) => {
                session.shutdown().await;
                Err(PeerDialError::UpgradeRequired(
                    "the sibling did not acknowledge the session".into(),
                ))
            }
        }
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Retires a relay session in favour of a LAN handshake when discovery
    /// now offers a candidate this session did not already fail against.
    /// The retirement happens under the pending-request lock, so a request
    /// is either answered by this session or refused before it is sent -
    /// never stranded as uncertain by the swap.
    async fn retire_for_lan_upgrade(&self, candidate: Option<SocketAddr>) -> bool {
        if self.route != PeerRoute::Relay
            || candidate.is_none()
            || candidate == self.failed_lan_candidate
        {
            return false;
        }
        let pending = self.pending.lock().await;
        if !pending.is_empty() {
            return false;
        }
        self.alive.store(false, Ordering::Relaxed);
        drop(pending);
        self.shutdown().await;
        true
    }

    async fn invoke(
        &self,
        method: &str,
        path: &str,
        body: serde_json::Value,
    ) -> Result<PeerInvokeOutcome, PeerDialError> {
        let permits = if path.split('?').next() == Some("/v1/task-events") {
            Arc::clone(&self.long_poll)
        } else {
            Arc::clone(&self.short)
        };
        let _permit = permits
            .acquire_owned()
            .await
            .map_err(|_| PeerDialError::Unreachable("peer session closed".into()))?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (response_tx, response_rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if !self.is_alive() {
                return Err(PeerDialError::SessionEnded);
            }
            pending.insert(id, response_tx);
        }
        let mut frame = serde_json::json!({
            "type": "request",
            "id": id,
            "method": method,
            "path": path,
        });
        if !body.is_null() {
            frame["body"] = body;
        }
        if self.outbound.send(frame.to_string()).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err(PeerDialError::Unreachable(
                "peer session writer stopped".into(),
            ));
        }
        match timeout(REQUEST_TIMEOUT, response_rx).await {
            Ok(Ok(response)) => Ok(PeerInvokeOutcome::Definite(response)),
            Ok(Err(_)) => Ok(PeerInvokeOutcome::Uncertain),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Ok(PeerInvokeOutcome::Uncertain)
            }
        }
    }

    async fn shutdown(&self) {
        self.alive.store(false, Ordering::Relaxed);
        for task in self.tasks.lock().await.drain(..) {
            task.abort();
        }
        self.pending.lock().await.clear();
    }
}

/// One pooled session per sibling, opened lazily and replaced on the next
/// use after it ends.
#[derive(Default)]
pub(crate) struct PeerSessions {
    sessions: Mutex<HashMap<String, Arc<PeerSession>>>,
}

impl PeerSessions {
    async fn session_for(
        &self,
        state: &Arc<AppState>,
        desktop_id: &str,
    ) -> Result<Arc<PeerSession>, PeerDialError> {
        let mut sessions = self.sessions.lock().await;
        if let Some(existing) = sessions.get(desktop_id) {
            if !existing.is_alive() {
                sessions.remove(desktop_id);
            } else if existing
                .retire_for_lan_upgrade(state.lan_api_candidate_for(desktop_id))
                .await
            {
                // The LAN route is preferred whenever discovery offers it: a
                // relay session opened before the sibling was discovered
                // gives way to a fresh LAN handshake. Should the LAN dial
                // fail, the replacement is a relay session that remembers
                // the candidate it failed against and is not retried on it.
                log::info!("[peer] retrying the LAN route to {desktop_id}");
                sessions.remove(desktop_id);
            } else {
                return Ok(Arc::clone(existing));
            }
        }
        let session = PeerSession::establish(state, desktop_id).await?;
        log::info!(
            "[peer] sealed session to {desktop_id} established over {}",
            session.route.as_str()
        );
        sessions.insert(desktop_id.to_string(), Arc::clone(&session));
        Ok(session)
    }

    /// One KSP request over the pooled session, with the route it took.
    pub(crate) async fn invoke(
        &self,
        state: &Arc<AppState>,
        desktop_id: &str,
        method: &str,
        path: &str,
        body: serde_json::Value,
    ) -> Result<(PeerInvokeOutcome, PeerRoute), PeerDialError> {
        let session = self.session_for(state, desktop_id).await?;
        let route = session.route;
        match session.invoke(method, path, body.clone()).await {
            // The pooled session was retired between lookup and send (a
            // LAN upgrade or an explicit close); nothing left this desktop,
            // so the request is safe to place on the replacement once.
            Err(PeerDialError::SessionEnded) => {
                let session = self.session_for(state, desktop_id).await?;
                let route = session.route;
                session
                    .invoke(method, path, body)
                    .await
                    .map(|outcome| (outcome, route))
            }
            result => result.map(|outcome| (outcome, route)),
        }
    }

    pub(crate) async fn close(&self, desktop_id: &str) {
        if let Some(session) = self.sessions.lock().await.remove(desktop_id) {
            session.shutdown().await;
        }
    }
}
