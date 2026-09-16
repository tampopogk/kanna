//! Task transfers between paired siblings ride the sealed peer channel.
//!
//! The task-transfer sidecar speaks plain TCP to an endpoint it is given,
//! and seals only some of its payloads (`ReadTaskFile` content, directory
//! listings, diffs, terminal output and typed keystrokes all travel in
//! clear). So the server never lets the sidecar dial a sibling directly:
//! for every paired peer whose transfer identity is pinned, this module
//! binds a loopback listener, registers it with the sidecar as that peer's
//! *external* endpoint (with the pinned transfer key, never one read from
//! Firestore or mDNS), and turns every accepted connection into a fresh
//! `peer_tunnel` session to the sibling - over LAN or the relay, whichever
//! `peer_channel::dial_peer` reaches - splicing bytes to sealed frames.
//! The sibling's server admits the tunnel only from a paired key and
//! splices it to its own sidecar's loopback port ([`serve_inbound_transfer_tunnel`]).
//! The sidecar's own sealing stays underneath as defense in depth.
//!
//! This replaces both the plaintext LAN transport and the renderer-owned
//! `cloud_transfer_proxy` (a Firebase-token relay tunnel) for paired
//! siblings; those remain only as the legacy path.

use crate::http_api::AppState;
use crate::peer_channel::{dial_peer, PeerHello};
use crate::peer_trust::PeerDesktop;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Weak};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch, Mutex, Semaphore};
use tokio::task::JoinHandle;

const MAX_PROXY_CONNECTIONS: usize = 16;
const READ_BUFFER_LEN: usize = 64 * 1024;

/// One sealed transfer route, as `transfer_targets` merges it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PeerTransferRoute {
    pub(crate) desktop_id: String,
    pub(crate) display_name: String,
    pub(crate) transfer_peer_id: String,
    #[serde(skip)]
    pub(crate) transfer_public_key: String,
    /// Loopback address the sidecar dials; never serialized to agents.
    #[serde(skip)]
    pub(crate) endpoint: String,
}

struct ProxyHandle {
    route: PeerTransferRoute,
    cancel: watch::Sender<bool>,
    listener_task: JoinHandle<()>,
}

/// Every provisioned sealed transfer route, by sibling desktop id.
#[derive(Default)]
pub(crate) struct PeerTransferProxies {
    proxies: Mutex<HashMap<String, ProxyHandle>>,
    /// Set on first use; the proxies need the server state to dial, and the
    /// state owns the proxies.
    state: std::sync::OnceLock<Weak<AppState>>,
}

impl PeerTransferProxies {
    fn attach(&self, state: &Arc<AppState>) {
        let _ = self.state.set(Arc::downgrade(state));
    }

    fn state(&self) -> Option<Arc<AppState>> {
        self.state.get().and_then(Weak::upgrade)
    }

    /// Reconciles the routes with the peer trust store: a sealed route for
    /// every paired sibling whose transfer identity is pinned, none for
    /// anyone else. A sibling paired before its sidecar reported an
    /// identity gets the identity fetched over the sealed session (and
    /// pinned on first sight) in the background, then a route.
    pub(crate) async fn sync_from_store(&self, state: &Arc<AppState>) {
        self.attach(state);
        let peers = match state.peer_trust_store() {
            Ok(store) => store.peers,
            Err(error) => {
                log::warn!("[peer-transfer] cannot read the peer trust store: {error}");
                return;
            }
        };
        let environment = &state.config().environment;
        let desired: Vec<PeerDesktop> = peers
            .into_iter()
            .filter(|peer| &peer.environment == environment)
            .collect();
        let desired_ids: std::collections::HashSet<&str> = desired
            .iter()
            .map(|peer| peer.desktop_id.as_str())
            .collect();
        let stale: Vec<String> = self
            .proxies
            .lock()
            .await
            .keys()
            .filter(|desktop_id| !desired_ids.contains(desktop_id.as_str()))
            .cloned()
            .collect();
        for desktop_id in stale {
            self.remove(&desktop_id).await;
        }
        for peer in desired {
            match (&peer.transfer_peer_id, &peer.transfer_public_key) {
                (Some(_), Some(_)) => {
                    if let Err(error) = self.ensure_for_peer(state, &peer).await {
                        log::warn!(
                            "[peer-transfer] no sealed transfer route for {}: {error}",
                            peer.desktop_id
                        );
                    }
                }
                _ => {
                    let state = Arc::clone(state);
                    tokio::spawn(async move {
                        if let Err(error) =
                            fetch_and_pin_transfer_identity(&state, &peer.desktop_id).await
                        {
                            log::info!(
                                "[peer-transfer] transfer identity of {} not available yet: {error}",
                                peer.desktop_id
                            );
                        }
                    });
                }
            }
        }
    }

    /// Binds (or keeps) the loopback listener for `peer` and registers it
    /// with a running sidecar. Requires a pinned transfer identity.
    pub(crate) async fn ensure_for_peer(
        &self,
        state: &Arc<AppState>,
        peer: &PeerDesktop,
    ) -> Result<PeerTransferRoute, String> {
        self.attach(state);
        let (Some(transfer_peer_id), Some(transfer_public_key)) =
            (&peer.transfer_peer_id, &peer.transfer_public_key)
        else {
            return Err("the sibling's transfer identity has not been exchanged yet".into());
        };
        let mut proxies = self.proxies.lock().await;
        if let Some(existing) = proxies.get(&peer.desktop_id) {
            if !existing.listener_task.is_finished()
                && existing.route.transfer_peer_id == *transfer_peer_id
                && existing.route.transfer_public_key == *transfer_public_key
            {
                let route = existing.route.clone();
                drop(proxies);
                self.register_with_running_sidecar(state, &route).await;
                return Ok(route);
            }
        }
        if let Some(previous) = proxies.remove(&peer.desktop_id) {
            let _ = previous.cancel.send(true);
            previous.listener_task.abort();
        }
        let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .map_err(|error| format!("failed to bind a loopback transfer proxy: {error}"))?;
        let endpoint = listener
            .local_addr()
            .map_err(|error| format!("proxy address unavailable: {error}"))?
            .to_string();
        let route = PeerTransferRoute {
            desktop_id: peer.desktop_id.clone(),
            display_name: peer.display_name.clone(),
            transfer_peer_id: transfer_peer_id.clone(),
            transfer_public_key: transfer_public_key.clone(),
            endpoint,
        };
        let (cancel, cancel_rx) = watch::channel(false);
        let listener_task = tokio::spawn(run_listener(
            listener,
            Arc::downgrade(state),
            peer.desktop_id.clone(),
            cancel_rx,
        ));
        proxies.insert(
            peer.desktop_id.clone(),
            ProxyHandle {
                route: route.clone(),
                cancel,
                listener_task,
            },
        );
        drop(proxies);
        log::info!(
            "[peer-transfer] sealed transfer route to {} at {}",
            route.desktop_id,
            route.endpoint
        );
        self.register_with_running_sidecar(state, &route).await;
        Ok(route)
    }

    async fn register_with_running_sidecar(
        &self,
        state: &Arc<AppState>,
        route: &PeerTransferRoute,
    ) {
        if let Some(client) = state.transfer_sidecar().running_client().await {
            register_route(&client, route).await;
        }
    }

    pub(crate) async fn remove(&self, desktop_id: &str) {
        let removed = self.proxies.lock().await.remove(desktop_id);
        let Some(handle) = removed else {
            return;
        };
        let _ = handle.cancel.send(true);
        handle.listener_task.abort();
        if let Some(state) = self.state() {
            if let Some(client) = state.transfer_sidecar().running_client().await {
                let _ = client
                    .request(
                        "remove_external_peer",
                        serde_json::json!({ "peer_id": handle.route.transfer_peer_id }),
                    )
                    .await;
            }
        }
    }

    pub(crate) async fn routes(&self) -> Vec<PeerTransferRoute> {
        let mut routes: Vec<PeerTransferRoute> = self
            .proxies
            .lock()
            .await
            .values()
            .filter(|handle| !handle.listener_task.is_finished())
            .map(|handle| handle.route.clone())
            .collect();
        routes.sort_by(|left, right| left.desktop_id.cmp(&right.desktop_id));
        routes
    }

    /// The sidecar spawn hook: a fresh sidecar knows no external peers.
    pub(crate) fn on_sidecar_spawned(
        self: &Arc<Self>,
        client: Arc<crate::transfer_sidecar::TransferSidecarClient>,
    ) {
        let proxies = Arc::clone(self);
        tokio::spawn(async move {
            for route in proxies.routes().await {
                register_route(&client, &route).await;
            }
        });
    }
}

async fn register_route(
    client: &crate::transfer_sidecar::TransferSidecarClient,
    route: &PeerTransferRoute,
) {
    let result = client
        .request(
            "upsert_external_peer",
            serde_json::json!({
                "peer": {
                    "peer_id": route.transfer_peer_id,
                    "display_name": route.display_name,
                    "endpoint": route.endpoint,
                    "public_key": route.transfer_public_key,
                    "protocol_version": 1,
                    "accepting_transfers": true,
                }
            }),
        )
        .await;
    if let Err(error) = result {
        log::warn!(
            "[peer-transfer] failed to register the sealed route to {} with the sidecar: {error}",
            route.desktop_id
        );
    }
}

/// Asks the sibling for its transfer identity over the sealed session and
/// pins it. A different identity than the pinned one is refused: only a
/// fresh pairing accepts a rotated sidecar key.
pub(crate) async fn fetch_and_pin_transfer_identity(
    state: &Arc<AppState>,
    desktop_id: &str,
) -> Result<PeerDesktop, String> {
    let (outcome, _) = state
        .peer_sessions()
        .invoke(
            state,
            desktop_id,
            "GET",
            "/v1/peers/transfer-identity",
            serde_json::Value::Null,
        )
        .await
        .map_err(|error| error.to_string())?;
    let response = match outcome {
        crate::peer_channel::PeerInvokeOutcome::Definite(response) => response,
        crate::peer_channel::PeerInvokeOutcome::Uncertain => {
            return Err("the sibling did not answer".into())
        }
    };
    if response.status != 200 {
        return Err(response
            .error
            .unwrap_or_else(|| format!("HTTP {}", response.status)));
    }
    let identity: crate::peer_pairing::PeerTransferIdentity = serde_json::from_value(
        response
            .body
            .ok_or_else(|| "empty transfer identity".to_string())?,
    )
    .map_err(|error| format!("malformed transfer identity: {error}"))?;
    let peer = crate::http_api::peers::pin_transfer_identity(state, desktop_id, &identity).await?;
    state
        .peer_transfer_proxies()
        .ensure_for_peer(state, &peer)
        .await?;
    Ok(peer)
}

async fn run_listener(
    listener: TcpListener,
    state: Weak<AppState>,
    desktop_id: String,
    mut cancel: watch::Receiver<bool>,
) {
    let permits = Arc::new(Semaphore::new(MAX_PROXY_CONNECTIONS));
    loop {
        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                let Ok((socket, _)) = accepted else { break };
                let Some(state) = state.upgrade() else { break };
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                    drop(socket);
                    continue;
                };
                let desktop_id = desktop_id.clone();
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(error) = bridge_outbound(socket, &state, &desktop_id, cancel).await {
                        log::warn!("[peer-transfer] tunnel to {desktop_id} failed: {error}");
                    }
                });
            }
        }
    }
}

/// One sidecar connection → one sealed tunnel to the sibling.
async fn bridge_outbound(
    mut local: TcpStream,
    state: &Arc<AppState>,
    desktop_id: &str,
    mut cancel: watch::Receiver<bool>,
) -> Result<(), String> {
    let sealed = dial_peer(
        state,
        desktop_id,
        PeerHello::Tunnel {
            service: "task-transfer".into(),
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    let (mut writer, mut reader) = sealed.split();
    let mut buffer = vec![0_u8; READ_BUFFER_LEN];
    loop {
        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    writer.close("route removed").await;
                    let _ = local.shutdown().await;
                    return Ok(());
                }
            }
            read = local.read(&mut buffer) => {
                let count = read.map_err(|error| format!("local read failed: {error}"))?;
                if count == 0 {
                    writer.close("local end closed").await;
                    return Ok(());
                }
                writer.send(&buffer[..count]).await?;
            }
            message = reader.next() => {
                match message? {
                    Some(bytes) => local
                        .write_all(&bytes)
                        .await
                        .map_err(|error| format!("local write failed: {error}"))?,
                    None => {
                        let _ = local.shutdown().await;
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// The sibling side of a transfer tunnel: an admitted `peer_tunnel` session
/// (already authenticated as a paired peer by `ksp::admit_sealed_session`)
/// spliced to this desktop's own sidecar over loopback. Generic over the
/// same socket shapes `ksp::run_socket_session` handles, so the LAN
/// endpoint and the relay tunnel share it.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve_inbound_transfer_tunnel<S, M, E>(
    state: Arc<AppState>,
    mut ws_tx: SplitSink<S, M>,
    mut ws_rx: SplitStream<S>,
    mut sender: kanna_secure_channel::Sender,
    mut receiver: kanna_secure_channel::Receiver,
    inbound: fn(M) -> crate::ksp::SocketInbound,
    outbound: fn(String) -> M,
    mut revocations: broadcast::Receiver<String>,
    peer_desktop_id: String,
) where
    S: futures_util::Stream<Item = Result<M, E>> + futures_util::Sink<M> + Unpin + Send + 'static,
    M: Send + 'static,
    E: Send + 'static,
    <S as futures_util::Sink<M>>::Error: Send,
{
    let close_with =
        |sender: &mut kanna_secure_channel::Sender, reason: &str| sender.seal_close(reason).ok();
    if let Err(error) = state
        .transfer_sidecar()
        .ensure_running_for_inbound_tunnel()
        .await
    {
        log::error!(
            "[peer-transfer] inbound tunnel from {peer_desktop_id}: sidecar unavailable: {error}"
        );
        if let Some(close) = close_with(&mut sender, "transfer sidecar unavailable") {
            let _ = ws_tx.send(outbound(close)).await;
        }
        return;
    }
    let transfer_address = SocketAddr::from((Ipv4Addr::LOCALHOST, state.config().transfer_port));
    let mut sidecar = match TcpStream::connect(transfer_address).await {
        Ok(stream) => stream,
        Err(error) => {
            log::error!("[peer-transfer] inbound tunnel from {peer_desktop_id}: sidecar connect failed: {error}");
            if let Some(close) = close_with(&mut sender, "transfer sidecar unreachable") {
                let _ = ws_tx.send(outbound(close)).await;
            }
            return;
        }
    };
    let mut buffer = vec![0_u8; READ_BUFFER_LEN];
    loop {
        tokio::select! {
            revoked = revocations.recv() => {
                match revoked {
                    Ok(desktop_id) if desktop_id == peer_desktop_id => {
                        log::info!("[peer-transfer] closing inbound tunnel: peer {desktop_id} was revoked");
                        if let Some(close) = close_with(&mut sender, "peer revoked") {
                            let _ = ws_tx.send(outbound(close)).await;
                        }
                        let _ = sidecar.shutdown().await;
                        return;
                    }
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        if state.paired_peer(&peer_desktop_id).is_none() {
                            if let Some(close) = close_with(&mut sender, "peer revoked") {
                                let _ = ws_tx.send(outbound(close)).await;
                            }
                            let _ = sidecar.shutdown().await;
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {}
                }
            }
            read = sidecar.read(&mut buffer) => {
                let Ok(count) = read else { break };
                if count == 0 {
                    if let Some(close) = close_with(&mut sender, "sidecar closed") {
                        let _ = ws_tx.send(outbound(close)).await;
                    }
                    return;
                }
                let Ok(wire) = sender.seal(&buffer[..count]) else { break };
                if ws_tx.send(outbound(wire)).await.is_err() {
                    break;
                }
            }
            frame = ws_rx.next() => {
                let Some(Ok(message)) = frame else { break };
                let text = match inbound(message) {
                    crate::ksp::SocketInbound::Text(text) => text,
                    crate::ksp::SocketInbound::Close => break,
                    crate::ksp::SocketInbound::Other => continue,
                };
                if crate::ksp::is_relay_tunnel_control_message(&text) {
                    continue;
                }
                let received = match receiver.open(&text) {
                    Ok(received) => received,
                    Err(error) => {
                        log::warn!("[peer-transfer] inbound tunnel from {peer_desktop_id} ended: {error}");
                        break;
                    }
                };
                for item in received {
                    match item {
                        kanna_secure_channel::Received::Message(bytes) => {
                            if sidecar.write_all(&bytes).await.is_err() {
                                return;
                            }
                        }
                        kanna_secure_channel::Received::Closed(_) => {
                            let _ = sidecar.shutdown().await;
                            return;
                        }
                    }
                }
            }
        }
    }
    let _ = sidecar.shutdown().await;
}
