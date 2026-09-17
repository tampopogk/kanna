//! Desktop-to-desktop peer routes: the pairing ceremony, the paired-peer
//! list, the sealed peer endpoint siblings dial, and the loopback proxy the
//! desktop renderer uses to view a sibling through this server.
//!
//! Authority model:
//! - Pairing controls, the peer list and unpairing are `DesktopLocalAccess`:
//!   this desktop's own person, never a paired device, tunnel or sibling.
//! - `POST /v1/peers/pairing/claim` is reachable only inside a sealed peer
//!   session whose key is *not yet* paired (`SealedPeerPairingContext`).
//! - `GET /v1/peers/transfer-identity` answers a paired sibling
//!   (`TrustedPeerDesktopAccess`) or the local desktop.
//! - `GET /v1/peers/channel` is the sealed endpoint itself; nothing about
//!   it is authenticated at the HTTP layer, the handshake is.
//! - `GET /v1/peers/{desktop_id}/ksp` is loopback-only: the desktop webview
//!   proves the local control credential in its first `auth` frame, a
//!   native loopback process needs nothing, and everything else is refused.

use super::lan_trust::{
    BrowserOriginatedRequest, DesktopLocalAccess, LocalControlCredential, TrustedPeerDesktopAccess,
};
use super::secure_channel::{SealedPeerPairingContext, SealedPopulation};
use super::state::AppState;
use crate::peer_channel::{dial_peer, PeerDialError, PeerHello, PeerInvokeOutcome};
use crate::peer_pairing::{
    self, PeerPairingClaim, PeerPairingClaimResponse, PeerPairingOfferView, PeerTransferIdentity,
};
use crate::peer_trust::{PeerDesktop, PeerTrustStore, TransferIdentityPin};
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// How long the renderer proxy waits for the webview's first `auth` frame.
const PROXY_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PeerReachability {
    lan: bool,
    relay: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PeerView {
    desktop_id: String,
    display_name: String,
    /// Always `e2ee`: a record here *is* a pinned peer. Reported explicitly
    /// so the machine list can say what it means.
    encryption: &'static str,
    paired_at_unix_ms: u64,
    last_seen_unix_ms: Option<u64>,
    transfer_identity_pinned: bool,
    reachable: PeerReachability,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PeerListResponse {
    desktop_id: String,
    desktop_name: String,
    /// Whether this desktop's peer channel identity loaded; `false` means
    /// no sibling can be paired or reached until the file is repaired.
    peer_channel_available: bool,
    legacy_access_allowed: bool,
    /// The relay session offers desktop peer tunnels (the cloud route).
    relay_peer_tunnels_available: bool,
    peers: Vec<PeerView>,
}

pub(super) async fn list_peers(
    _access: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
) -> Result<Json<PeerListResponse>, (StatusCode, String)> {
    let store = state
        .peer_trust_store()
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    let environment = state.config().environment.clone();
    let relay_active = state.list_active_relay_desktops().await.unwrap_or_default();
    let peers = store
        .peers
        .iter()
        .filter(|peer| peer.environment == environment)
        .map(|peer| PeerView {
            desktop_id: peer.desktop_id.clone(),
            display_name: peer.display_name.clone(),
            encryption: "e2ee",
            paired_at_unix_ms: peer.paired_at_unix_ms,
            last_seen_unix_ms: peer.last_seen_unix_ms,
            transfer_identity_pinned: peer.transfer_peer_id.is_some()
                && peer.transfer_public_key.is_some(),
            reachable: PeerReachability {
                lan: state.lan_api_candidate_for(&peer.desktop_id).is_some(),
                relay: state.desktop_tunnel_available()
                    && relay_active.iter().any(|id| id == &peer.desktop_id),
            },
        })
        .collect();
    Ok(Json(PeerListResponse {
        desktop_id: state.config().desktop_id.clone(),
        desktop_name: state.config().desktop_name.clone(),
        peer_channel_available: state.peer_channel_identity().is_ok(),
        legacy_access_allowed: state.legacy_peer_access_allowed(),
        relay_peer_tunnels_available: state.desktop_tunnel_available(),
        peers,
    }))
}

/// Mints the pairing string this desktop shows. One live offer at a time;
/// a new one replaces the previous.
pub(super) async fn create_pairing_offer(
    _access: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
) -> Result<Json<PeerPairingOfferView>, (StatusCode, String)> {
    let identity = state.peer_channel_identity().map_err(|error| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("peer secure channel unavailable: {error}"),
        )
    })?;
    let now_ms = unix_time_ms()?;
    let offer =
        peer_pairing::create_offer(&state.config().desktop_id, identity.public_key(), now_ms)
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    let view = PeerPairingOfferView {
        desktop_id: state.config().desktop_id.clone(),
        desktop_name: state.config().desktop_name.clone(),
        code: offer.code.clone(),
        pairing_string: offer.pairing_string.clone(),
        expires_at_unix_ms: offer.expires_at_unix_ms,
    };
    *state.peer_pairing_offer.lock().await = Some(offer);
    Ok(Json(view))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PairRequest {
    pairing_string: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PairResponse {
    desktop_id: String,
    display_name: String,
    encryption: &'static str,
    route: &'static str,
    transfer_identity_pinned: bool,
}

/// The claimant side: parse the string, pin the issuer's key from it, open
/// a sealed pairing session against exactly that key, claim, and record
/// the issuer as a paired peer. Every failure is reported by its cause
/// (`peer_upgrade_required`, `peer_identity_mismatch`, `peer_unreachable`,
/// or the issuer's own refusal) and pins nothing.
pub(super) async fn pair_with_string(
    _access: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
    Json(request): Json<PairRequest>,
) -> Result<Json<PairResponse>, (StatusCode, String)> {
    let parsed = peer_pairing::parse_pairing_string(&request.pairing_string)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let config = state.config();
    if parsed.desktop_id == config.desktop_id {
        return Err((
            StatusCode::BAD_REQUEST,
            "that pairing string is this desktop's own".to_string(),
        ));
    }
    let local_transfer_identity = local_transfer_identity(&state).await;
    let sealed = dial_peer_with_key_for_pairing(&state, &parsed).await?;
    let route = sealed.route.as_str();
    let (mut writer, mut reader) = sealed.split();
    let claim = PeerPairingClaim {
        code: parsed.code.clone(),
        secret: parsed.secret.clone(),
        desktop_id: config.desktop_id.clone(),
        desktop_name: config.desktop_name.clone(),
        environment: config.environment.clone(),
        transfer_identity: local_transfer_identity,
    };
    let outcome = async {
        writer
            .send(
                serde_json::json!({ "type": "auth", "capabilities": [] })
                    .to_string()
                    .as_bytes(),
            )
            .await?;
        expect_frame(&mut reader, "auth_ok").await?;
        writer
            .send(
                serde_json::json!({
                    "type": "request",
                    "id": 1,
                    "method": "POST",
                    "path": "/v1/peers/pairing/claim",
                    "body": claim,
                })
                .to_string()
                .as_bytes(),
            )
            .await?;
        let response = expect_frame(&mut reader, "response").await?;
        Ok::<serde_json::Value, String>(response)
    };
    let response = match tokio::time::timeout(Duration::from_secs(20), outcome).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            writer.close("pairing failed").await;
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("peer pairing failed: {error}"),
            ));
        }
        Err(_) => {
            writer.close("pairing timed out").await;
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                "the other desktop did not answer the pairing claim".to_string(),
            ));
        }
    };
    writer.close("pairing complete").await;
    let status = response
        .get("status")
        .and_then(|status| status.as_u64())
        .unwrap_or(0);
    if status != 200 {
        let reason = response
            .get("body")
            .and_then(|body| body.get("error"))
            .and_then(|error| error.as_str())
            .unwrap_or("the other desktop refused the pairing claim");
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("peer pairing refused: {reason}"),
        ));
    }
    let body: PeerPairingClaimResponse = response
        .get("body")
        .cloned()
        .ok_or_else(|| (StatusCode::BAD_GATEWAY, "empty pairing reply".to_string()))
        .and_then(|body| {
            serde_json::from_value(body).map_err(|error| {
                (
                    StatusCode::BAD_GATEWAY,
                    format!("malformed pairing reply: {error}"),
                )
            })
        })?;
    if body.desktop_id != parsed.desktop_id {
        return Err((
            StatusCode::BAD_GATEWAY,
            "the pairing reply named a different desktop than the string".to_string(),
        ));
    }
    if body.environment != config.environment {
        return Err((
            StatusCode::BAD_REQUEST,
            "the desktops run in different Kanna environments".to_string(),
        ));
    }
    let now_ms = unix_time_ms()?;
    let peer = PeerDesktop {
        desktop_id: parsed.desktop_id.clone(),
        display_name: body.desktop_name.clone(),
        channel_public_key: kanna_secure_channel::encode_key(&parsed.channel_public_key),
        transfer_peer_id: body
            .transfer_identity
            .as_ref()
            .map(|identity| identity.peer_id.clone()),
        transfer_public_key: body
            .transfer_identity
            .as_ref()
            .map(|identity| identity.public_key.clone()),
        environment: config.environment.clone(),
        account_uid: state.authenticated_account_uid(),
        paired_at_unix_ms: now_ms,
        last_seen_unix_ms: Some(now_ms),
    };
    persist_peer(&state, peer.clone()).await?;
    state.publish_state_changed(kanna_agent_protocol::StateChangeScope::Settings);
    // A pooled session from before the (re)pairing would carry an old pin.
    state.peer_sessions().close(&parsed.desktop_id).await;
    let transfer_identity_pinned = peer.transfer_peer_id.is_some();
    let proxies = state.peer_transfer_proxies();
    let sync_state = Arc::clone(&state);
    tokio::spawn(async move { proxies.sync_from_store(&sync_state).await });
    Ok(Json(PairResponse {
        desktop_id: parsed.desktop_id,
        display_name: body.desktop_name,
        encryption: "e2ee",
        route,
        transfer_identity_pinned,
    }))
}

async fn dial_peer_with_key_for_pairing(
    state: &Arc<AppState>,
    parsed: &peer_pairing::ParsedPairingString,
) -> Result<crate::peer_channel::SealedPeerSocket, (StatusCode, String)> {
    crate::peer_channel::dial_peer_with_key(
        state,
        &parsed.desktop_id,
        parsed.channel_public_key,
        PeerHello::Pairing,
    )
    .await
    .map_err(dial_error_response)
}

fn dial_error_response(error: PeerDialError) -> (StatusCode, String) {
    let status = match error {
        PeerDialError::PairingRequired | PeerDialError::NotPairedBySibling => {
            StatusCode::PRECONDITION_FAILED
        }
        PeerDialError::IdentityUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        PeerDialError::UpgradeRequired(_) | PeerDialError::IdentityMismatch(_) => {
            StatusCode::BAD_GATEWAY
        }
        PeerDialError::Unreachable(_) => StatusCode::BAD_GATEWAY,
    };
    (status, format!("{}: {error}", error.code()))
}

async fn expect_frame(
    reader: &mut crate::peer_channel::SealedPeerReader,
    kind: &str,
) -> Result<serde_json::Value, String> {
    loop {
        let Some(message) = reader.next().await? else {
            return Err("the other desktop closed the session".to_string());
        };
        let frame: serde_json::Value = serde_json::from_slice(&message)
            .map_err(|error| format!("malformed frame from the other desktop: {error}"))?;
        match frame.get("type").and_then(|value| value.as_str()) {
            Some(received) if received == kind => return Ok(frame),
            Some("error") => {
                return Err(frame
                    .get("message")
                    .and_then(|message| message.as_str())
                    .unwrap_or("the other desktop refused")
                    .to_string())
            }
            _ => continue,
        }
    }
}

/// This desktop's own transfer identity, if the sidecar can report one.
/// Absent (not an error) when the sidecar is unavailable: pairing still
/// succeeds and the identity is exchanged later over the sealed session.
async fn local_transfer_identity(state: &Arc<AppState>) -> Option<PeerTransferIdentity> {
    match state
        .transfer_sidecar()
        .control("identity", serde_json::json!({}))
        .await
    {
        Ok(value) => {
            let peer_id = value.get("peerId")?.as_str()?.to_string();
            let public_key = value.get("publicKey")?.as_str()?.to_string();
            Some(PeerTransferIdentity {
                peer_id,
                public_key,
            })
        }
        Err(error) => {
            log::info!("[peer] transfer identity not available for pairing: {error}");
            None
        }
    }
}

async fn persist_peer(
    state: &Arc<AppState>,
    peer: PeerDesktop,
) -> Result<(), (StatusCode, String)> {
    let path = state.config().peer_trust_store_path().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "no pairing store configured".to_string(),
    ))?;
    let _guard = crate::peer_trust::persistence_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut store =
        PeerTrustStore::load(&path).map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    store
        .upsert(peer)
        .map_err(|error| (StatusCode::CONFLICT, error))?;
    store
        .save(&path)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// Pins a sibling's transfer identity the first time it is seen and
/// refuses a change. Shared by the pairing reply and the later fetch over
/// the sealed session.
pub(crate) async fn pin_transfer_identity(
    state: &Arc<AppState>,
    desktop_id: &str,
    identity: &PeerTransferIdentity,
) -> Result<PeerDesktop, String> {
    if identity.peer_id.trim().is_empty() || identity.public_key.trim().is_empty() {
        return Err("empty transfer identity".to_string());
    }
    let path = state
        .config()
        .peer_trust_store_path()
        .ok_or_else(|| "no pairing store configured".to_string())?;
    let _guard = crate::peer_trust::persistence_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut store = PeerTrustStore::load(&path)?;
    match store.pin_transfer_identity(desktop_id, &identity.peer_id, &identity.public_key)? {
        TransferIdentityPin::Pinned => store.save(&path)?,
        TransferIdentityPin::Unchanged => {}
        TransferIdentityPin::Conflict => {
            return Err(format!(
                "the transfer identity of {desktop_id} differs from the pinned one; unpair and pair again to accept it"
            ))
        }
    }
    store
        .peer_by_desktop_id(desktop_id, &state.config().environment)
        .cloned()
        .ok_or_else(|| format!("desktop {desktop_id} is not a paired peer"))
}

/// The issuer side: reachable only inside a sealed peer session whose key
/// is not yet paired. On success the claimant's *handshake* key is pinned
/// under the desktop id it claimed, and the reply carries this desktop's
/// transfer identity.
pub(super) async fn claim_pairing_offer(
    State(state): State<Arc<AppState>>,
    sealed: Option<Extension<SealedPeerPairingContext>>,
    Json(claim): Json<PeerPairingClaim>,
) -> Result<Json<PeerPairingClaimResponse>, (StatusCode, String)> {
    let Some(Extension(context)) = sealed else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "a peer pairing claim is only accepted inside a sealed peer session".to_string(),
        ));
    };
    let config = state.config();
    let now_ms = unix_time_ms()?;
    {
        let mut offer = state.peer_pairing_offer.lock().await;
        peer_pairing::verify_claim(
            &mut offer,
            &claim,
            context.declared_desktop_id.as_deref(),
            &config.desktop_id,
            &config.environment,
            now_ms,
        )
        .map_err(|error| {
            let status = match error {
                peer_pairing::PeerClaimError::NoActiveOffer => StatusCode::CONFLICT,
                peer_pairing::PeerClaimError::Expired => StatusCode::GONE,
                peer_pairing::PeerClaimError::InvalidCode => StatusCode::BAD_REQUEST,
                peer_pairing::PeerClaimError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
                peer_pairing::PeerClaimError::EnvironmentMismatch
                | peer_pairing::PeerClaimError::SelfPairing
                | peer_pairing::PeerClaimError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            };
            (status, error.to_string())
        })?;
    }
    let peer = PeerDesktop {
        desktop_id: claim.desktop_id.clone(),
        display_name: claim.desktop_name.trim().to_string(),
        channel_public_key: context.encoded_remote_static(),
        transfer_peer_id: claim
            .transfer_identity
            .as_ref()
            .map(|identity| identity.peer_id.clone()),
        transfer_public_key: claim
            .transfer_identity
            .as_ref()
            .map(|identity| identity.public_key.clone()),
        environment: config.environment.clone(),
        account_uid: state.authenticated_account_uid(),
        paired_at_unix_ms: now_ms,
        last_seen_unix_ms: Some(now_ms),
    };
    persist_peer(&state, peer).await?;
    state.publish_state_changed(kanna_agent_protocol::StateChangeScope::Settings);
    log::info!(
        "[peer] paired with {} ({:?}) over {:?}",
        claim.desktop_id,
        claim.desktop_name,
        context.origin
    );
    state.peer_sessions().close(&claim.desktop_id).await;
    let proxies = state.peer_transfer_proxies();
    let sync_state = Arc::clone(&state);
    tokio::spawn(async move { proxies.sync_from_store(&sync_state).await });
    Ok(Json(PeerPairingClaimResponse {
        desktop_id: config.desktop_id.clone(),
        desktop_name: config.desktop_name.clone(),
        environment: config.environment.clone(),
        transfer_identity: local_transfer_identity(&state).await,
    }))
}

/// This desktop's sidecar identity, for a paired sibling (over its sealed
/// session) or the local desktop.
pub(super) async fn transfer_identity(
    State(state): State<Arc<AppState>>,
    peer: Option<Extension<TrustedPeerDesktopAccess>>,
    tunneled: Option<Extension<super::state::TunneledHttpInvoke>>,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
) -> Result<Json<PeerTransferIdentity>, (StatusCode, String)> {
    let local = tunneled.is_none()
        && connect
            .as_ref()
            .is_some_and(|Extension(ConnectInfo(peer))| peer.ip().is_loopback());
    if peer.is_none() && !local {
        return Err((
            StatusCode::UNAUTHORIZED,
            "the transfer identity is only readable by a paired sibling or this desktop"
                .to_string(),
        ));
    }
    if let Some(Extension(peer)) = &peer {
        touch_peer(&state, peer.desktop_id());
    }
    local_transfer_identity(&state).await.map(Json).ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "the transfer sidecar has not reported an identity".to_string(),
    ))
}

fn touch_peer(state: &Arc<AppState>, desktop_id: &str) {
    let Some(path) = state.config().peer_trust_store_path() else {
        return;
    };
    let Ok(now_ms) = unix_time_ms() else {
        return;
    };
    let _guard = crate::peer_trust::persistence_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Ok(mut store) = PeerTrustStore::load(&path) {
        store.touch(desktop_id, now_ms);
        let _ = store.save(&path);
    }
}

/// Unpairs a sibling: persisted first, then announced so every live sealed
/// session for it closes with an authenticated `peer revoked` close, the
/// pooled outbound session ends and its transfer route is withdrawn.
pub(super) async fn remove_peer(
    _access: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(desktop_id): axum::extract::Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let path = state.config().peer_trust_store_path().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "no pairing store configured".to_string(),
    ))?;
    let removed = {
        let _guard = crate::peer_trust::persistence_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut store = PeerTrustStore::load(&path)
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
        let removed = store.remove(&desktop_id);
        if removed {
            store
                .save(&path)
                .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
        }
        removed
    };
    if !removed {
        return Err((StatusCode::NOT_FOUND, "peer not found".to_string()));
    }
    state.announce_peer_revocation(&desktop_id).await;
    state.publish_state_changed(kanna_agent_protocol::StateChangeScope::Settings);
    Ok(StatusCode::NO_CONTENT)
}

/// The sealed peer endpoint a sibling dials over the LAN. Plaintext is
/// never admitted here (see `ksp::run_socket_session`).
pub(super) async fn peer_channel_stream(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(move |socket| {
        crate::ksp::handle_stream_for(
            socket,
            state,
            crate::ksp::AuthMode::RequirePairedDevice,
            false,
            SealedPopulation::Peer,
        )
    })
}

/// How the renderer's proxy socket authenticates, mirroring
/// `ksp::direct_stream_auth_mode` minus the paired-device shapes: a
/// browser-originated loopback upgrade proves the local control credential
/// in its first `auth` frame, a native loopback process needs nothing, and
/// a non-loopback peer is refused outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProxyAuth {
    LocalControlToken,
    LoopbackProcess,
    Refused,
}

fn proxy_auth(
    peer: Option<&ConnectInfo<SocketAddr>>,
    browser_originated: bool,
    local_credential_at_upgrade: bool,
) -> ProxyAuth {
    let loopback = peer.is_some_and(|ConnectInfo(peer)| peer.ip().is_loopback());
    if !loopback {
        return ProxyAuth::Refused;
    }
    if browser_originated && !local_credential_at_upgrade {
        ProxyAuth::LocalControlToken
    } else {
        ProxyAuth::LoopbackProcess
    }
}

/// The renderer's sibling view: a loopback WebSocket carrying plaintext KSP
/// frames that this server splices 1:1 into a fresh sealed `peer_session`
/// to `desktop_id`. The renderer never holds a relay socket, a Firebase
/// token or a peer key on this path; the server is the only party that
/// talks to the sibling.
pub(super) async fn peer_ksp_proxy_stream(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    browser: Option<Extension<BrowserOriginatedRequest>>,
    local_credential: Option<Extension<LocalControlCredential>>,
    axum::extract::Path(desktop_id): axum::extract::Path<String>,
) -> Response {
    let auth = proxy_auth(
        peer.as_ref().map(|Extension(peer)| peer),
        browser.is_some(),
        local_credential.is_some(),
    );
    if auth == ProxyAuth::Refused {
        return (
            StatusCode::UNAUTHORIZED,
            "the peer view proxy is only available to this desktop",
        )
            .into_response();
    }
    ws.on_upgrade(move |socket| run_peer_ksp_proxy(socket, state, desktop_id, auth))
}

async fn send_error_and_close(socket: &mut WebSocket, code: &str, message: String) {
    let frame = serde_json::json!({ "type": "error", "code": code, "message": message });
    let _ = socket.send(WsMessage::Text(frame.to_string().into())).await;
    let _ = socket.close().await;
}

async fn run_peer_ksp_proxy(
    mut socket: WebSocket,
    state: Arc<AppState>,
    desktop_id: String,
    auth: ProxyAuth,
) {
    // The renderer's first frame is its `auth`; the credential in it is
    // proved *here* and never forwarded - the sibling authenticates this
    // desktop by the peer handshake, not by a token.
    let first = tokio::time::timeout(PROXY_FIRST_FRAME_TIMEOUT, async {
        loop {
            match socket.next().await {
                Some(Ok(WsMessage::Text(text))) => return Some(text.to_string()),
                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => return None,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await
    .ok()
    .flatten();
    let Some(first) = first else {
        return;
    };
    let mut auth_frame: serde_json::Value = match serde_json::from_str(&first) {
        Ok(frame) => frame,
        Err(_) => {
            send_error_and_close(&mut socket, "bad_frame", "unparseable auth frame".into()).await;
            return;
        }
    };
    if auth_frame.get("type").and_then(|value| value.as_str()) != Some("auth") {
        send_error_and_close(
            &mut socket,
            "unauthorized",
            "first frame must be auth".into(),
        )
        .await;
        return;
    }
    let credential = auth_frame
        .get("credential")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    if auth == ProxyAuth::LocalControlToken {
        let valid = credential.as_deref().is_some_and(|presented| {
            state
                .local_task_events_token
                .as_deref()
                .is_some_and(|expected| {
                    crate::pairing::constant_time_eq(expected.as_bytes(), presented.as_bytes())
                })
        });
        if !valid {
            send_error_and_close(
                &mut socket,
                "unauthorized",
                "invalid stream credential".into(),
            )
            .await;
            return;
        }
    }
    if let Some(frame) = auth_frame.as_object_mut() {
        frame.remove("credential");
    }
    let sealed = match dial_peer(&state, &desktop_id, PeerHello::Session).await {
        Ok(sealed) => sealed,
        Err(error) => {
            log::warn!("[peer] renderer view of {desktop_id} refused: {error}");
            send_error_and_close(&mut socket, error.code(), error.to_string()).await;
            return;
        }
    };
    log::info!(
        "[peer] renderer view of {desktop_id} spliced over {}",
        sealed.route.as_str()
    );
    let (mut writer, mut reader) = sealed.split();
    if writer
        .send(auth_frame.to_string().as_bytes())
        .await
        .is_err()
    {
        send_error_and_close(&mut socket, "peer_unreachable", "peer session ended".into()).await;
        return;
    }
    let mut revocations = state.subscribe_peer_revocations();
    loop {
        tokio::select! {
            revoked = revocations.recv() => {
                if matches!(revoked, Ok(ref id) if id == &desktop_id) {
                    writer.close("peer revoked").await;
                    send_error_and_close(&mut socket, "peer_revoked", "the machine was unpaired".into()).await;
                    return;
                }
            }
            frame = socket.next() => {
                match frame {
                    Some(Ok(WsMessage::Text(text))) => {
                        if writer.send(text.as_bytes()).await.is_err() {
                            send_error_and_close(&mut socket, "peer_unreachable", "peer session ended".into()).await;
                            return;
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => {
                        writer.close("renderer closed").await;
                        return;
                    }
                    Some(Ok(_)) => continue,
                }
            }
            message = reader.next() => {
                match message {
                    Ok(Some(bytes)) => {
                        let Ok(text) = String::from_utf8(bytes) else { continue };
                        if socket.send(WsMessage::Text(text.into())).await.is_err() {
                            writer.close("renderer closed").await;
                            return;
                        }
                    }
                    Ok(None) => {
                        let _ = socket.close().await;
                        return;
                    }
                    Err(error) => {
                        send_error_and_close(&mut socket, "peer_session_ended", error).await;
                        return;
                    }
                }
            }
        }
    }
}

/// Route the pooled peer session's outcome into the shape `invoke_desktop`
/// reports.
pub(crate) fn peer_invoke_outcome_response(
    outcome: PeerInvokeOutcome,
) -> super::state::HttpInvokeResponse {
    match outcome {
        PeerInvokeOutcome::Definite(response) => response,
        PeerInvokeOutcome::Uncertain => super::state::HttpInvokeResponse {
            status: 0,
            body: None,
            error: Some("delivery_uncertain".to_string()),
        },
    }
}

fn unix_time_ms() -> Result<u64, (StatusCode, String)> {
    crate::peer_trust::unix_time_ms().map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn peer(ip: IpAddr) -> ConnectInfo<SocketAddr> {
        ConnectInfo(SocketAddr::new(ip, 49152))
    }

    #[test]
    fn the_renderer_proxy_is_loopback_only_and_browsers_prove_the_local_token() {
        let loopback = peer(IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(
            proxy_auth(Some(&loopback), true, false),
            ProxyAuth::LocalControlToken
        );
        assert_eq!(
            proxy_auth(Some(&loopback), true, true),
            ProxyAuth::LoopbackProcess
        );
        assert_eq!(
            proxy_auth(Some(&loopback), false, false),
            ProxyAuth::LoopbackProcess
        );
        let lan = peer(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)));
        assert_eq!(proxy_auth(Some(&lan), false, false), ProxyAuth::Refused);
        assert_eq!(proxy_auth(None, false, false), ProxyAuth::Refused);
    }
}
