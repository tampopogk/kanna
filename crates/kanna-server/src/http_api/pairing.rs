use super::lan_trust::{DesktopLocalAccess, TrustedLanDeviceAccess};
use super::secure_channel::{
    PairingConfirmationError, PairingConfirmationOutcome, PendingPairingConfirmation,
    SealedPairingContext, StreamOrigin, PAIRING_CONFIRMATION_TTL,
};
use super::state::{AppState, TunneledHttpInvoke};
use crate::pairing::{
    self as pairing_domain, PairingCertificateError, PairingClaimError, PairingClaimProof,
    PairingClaimRequest, PairingClaimResponse, PairingSession, PushPairingMaterial,
};
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a sealed phone's confirmation poll may hang before answering
/// `pending`; the phone polls again.
const CONFIRMATION_POLL_TIMEOUT: Duration = Duration::from_secs(25);

pub(super) async fn remove_trusted_device(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    tunneled: Option<Extension<TunneledHttpInvoke>>,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(device_id): axum::extract::Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    if !peer.ip().is_loopback() || tunneled.is_some() {
        return Err((
            StatusCode::FORBIDDEN,
            "trusted devices can only be removed from the desktop app".to_string(),
        ));
    }
    if device_id.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "device id is required".to_string()));
    }
    match state.remove_trusted_device(&device_id).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            "trusted device not found".to_string(),
        )),
        Err(error) => Err((StatusCode::INTERNAL_SERVER_ERROR, error)),
    }
}

pub(super) async fn create_pairing_session(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    tunneled: Option<Extension<TunneledHttpInvoke>>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<PairingSession>, (axum::http::StatusCode, String)> {
    if !peer.ip().is_loopback() || tunneled.is_some() {
        return Err((
            StatusCode::FORBIDDEN,
            "pairing sessions can only be started from the desktop app".to_string(),
        ));
    }
    // A desktop with a channel identity always offers the key-bearing QR:
    // a phone that scans it pins the desktop from the screen, never from
    // the network. A desktop whose identity failed to load still pairs in
    // the legacy shape (and says so in its log) so pairing is not bricked
    // by a corrupt identity file, but nothing it pairs is E2EE.
    let active = match state.secure_channel_identity() {
        Ok(identity) => pairing_domain::create_active_pairing_session_with_channel(
            &state.config,
            identity.public_key(),
        ),
        Err(error) => {
            log::warn!("pairing without a secure-channel identity: {error}");
            pairing_domain::create_active_pairing_session(&state.config)
        }
    }
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let session = active.session.clone();
    {
        let mut pairing_session = state.pairing_session.lock().await;
        *pairing_session = Some(active);
    }
    Ok(Json(session))
}

fn claim_error_response(error: PairingClaimError) -> (StatusCode, String) {
    let status = match error {
        PairingClaimError::InvalidRequest | PairingClaimError::InvalidCode => {
            StatusCode::BAD_REQUEST
        }
        PairingClaimError::Expired => StatusCode::GONE,
        PairingClaimError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        PairingClaimError::NoActiveSession => StatusCode::CONFLICT,
        PairingClaimError::Persistence(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, error.to_string())
}

/// The pairing claim, on either of its two paths:
///
/// - **Sealed** (the request arrived inside a secure-channel session, so it
///   carries `SealedPairingContext`): the phone's static key is the one the
///   handshake authenticated. With the QR-only secret the device is
///   registered at once; with the typed code alone the claim consumes the
///   code and parks a confirmation the person must accept on the desktop
///   after comparing short authentication strings, and the phone polls
///   `GET /v1/pairing/confirmation` for the outcome.
/// - **Legacy** (plaintext LAN POST from a phone that predates the secure
///   channel): allowed only while legacy mobile access is on, and never
///   from a relay invoke - a tunnel has no LAN presence to claim with.
pub(super) async fn claim_pairing_session(
    State(state): State<Arc<AppState>>,
    tunneled: Option<Extension<TunneledHttpInvoke>>,
    sealed: Option<Extension<SealedPairingContext>>,
    Json(request): Json<PairingClaimRequest>,
) -> Result<Response, (StatusCode, String)> {
    if let Some(Extension(context)) = sealed {
        return claim_sealed(state, context, request).await;
    }
    if tunneled.is_some() {
        return Err((
            StatusCode::UNAUTHORIZED,
            "pairing can only be claimed from the local network or a secure channel".into(),
        ));
    }
    if !state.legacy_mobile_access_allowed() {
        return Err((
            StatusCode::FORBIDDEN,
            "this desktop only pairs end-to-end encrypted devices; update Kanna Mobile".into(),
        ));
    }
    let mut active = state.pairing_session.lock().await;
    let _persistence_mutation = state.pairing_persistence_mutation.lock().await;
    pairing_domain::claim_pairing_session(&state.config, &mut active, request)
        .map(|response| Json(response).into_response())
        .map_err(claim_error_response)
}

async fn claim_sealed(
    state: Arc<AppState>,
    context: SealedPairingContext,
    request: PairingClaimRequest,
) -> Result<Response, (StatusCode, String)> {
    let now_ms = unix_time_ms()?;
    let device_id = request.device_id.trim().to_string();
    let device_name = request.device_name.trim().to_string();
    let session = {
        let mut active = state.pairing_session.lock().await;
        pairing_domain::verify_pairing_claim_at(&mut active, &request, now_ms)
            .map_err(claim_error_response)?
    };
    let channel_public_key = context.encoded_remote_static();
    match session.proof {
        PairingClaimProof::QrAnchored => {
            let _persistence_mutation = state.pairing_persistence_mutation.lock().await;
            let response = pairing_domain::register_trusted_device_at(
                &state.config,
                &session,
                &device_id,
                &device_name,
                Some(&channel_public_key),
                now_ms,
            )
            .map_err(claim_error_response)?;
            state
                .pairing_confirmation
                .abandon(&context.handshake_hash)
                .await;
            Ok(Json(response).into_response())
        }
        PairingClaimProof::TypedCode if context.origin == StreamOrigin::RelayTunnel => {
            // A typed code pins nothing: the phone's idea of the desktop key
            // came from the network, and the SAS comparison that would
            // repair that assumes both screens are in the same room. Over
            // the relay the only anchored path is the QR.
            Err((
                StatusCode::BAD_REQUEST,
                "typed-code pairing works on the local network only; scan the desktop's QR code to pair remotely".into(),
            ))
        }
        PairingClaimProof::TypedCode => {
            let sas = kanna_secure_channel::sas_code(&context.handshake_hash);
            let created_at = Instant::now();
            state
                .pairing_confirmation
                .begin(PendingPairingConfirmation {
                    device_id,
                    device_name,
                    channel_public_key,
                    sas,
                    handshake_hash: context.handshake_hash,
                    session,
                    expires_at: created_at + PAIRING_CONFIRMATION_TTL,
                    expires_at_unix_ms: now_ms + PAIRING_CONFIRMATION_TTL.as_millis() as u64,
                    decision: None,
                })
                .await;
            state.publish_state_changed(kanna_agent_protocol::StateChangeScope::Settings);
            Ok((
                StatusCode::ACCEPTED,
                Json(serde_json::json!({
                    "status": "confirmation_required",
                    "expiresInMs": PAIRING_CONFIRMATION_TTL.as_millis() as u64,
                })),
            )
                .into_response())
        }
    }
}

/// Sealed phone polls for the person's decision on its typed-code pairing.
pub(super) async fn pairing_confirmation(
    State(state): State<Arc<AppState>>,
    sealed: Option<Extension<SealedPairingContext>>,
) -> Result<Response, (StatusCode, String)> {
    let Some(Extension(context)) = sealed else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "pairing confirmation is only readable inside the secure channel that claimed".into(),
        ));
    };
    match state
        .pairing_confirmation
        .wait_decision(&context.handshake_hash, CONFIRMATION_POLL_TIMEOUT)
        .await
    {
        PairingConfirmationOutcome::Confirmed(response) => Ok(Json(*response).into_response()),
        PairingConfirmationOutcome::Pending => Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "status": "pending" })),
        )
            .into_response()),
        PairingConfirmationOutcome::Rejected => Err((
            StatusCode::FORBIDDEN,
            "the pairing was rejected on the desktop".into(),
        )),
        PairingConfirmationOutcome::Gone => Err((
            StatusCode::GONE,
            "no pairing confirmation is pending for this connection".into(),
        )),
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PendingPairingConfirmationView {
    device_name: String,
    device_id: String,
    sas: String,
    expires_at_unix_ms: u64,
}

/// What the desktop UI shows while a typed-code pairing waits.
pub(super) async fn pending_pairing_confirmation(
    _desktop: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let pending =
        state
            .pairing_confirmation
            .current()
            .await
            .map(|entry| PendingPairingConfirmationView {
                device_name: entry.device_name,
                device_id: entry.device_id,
                sas: entry.sas,
                expires_at_unix_ms: entry.expires_at_unix_ms,
            });
    Json(serde_json::json!({ "pending": pending }))
}

/// The person compared the two strings and they matched.
pub(super) async fn confirm_pending_pairing(
    _desktop: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
) -> Result<Json<PairingClaimResponse>, (StatusCode, String)> {
    let response = state
        .pairing_confirmation
        .confirm(&state.config, &state.pairing_persistence_mutation)
        .await
        .map_err(|error| match error {
            PairingConfirmationError::NothingPending => (StatusCode::CONFLICT, error.to_string()),
            PairingConfirmationError::Persistence(message) => {
                (StatusCode::INTERNAL_SERVER_ERROR, message)
            }
        })?;
    state.publish_state_changed(kanna_agent_protocol::StateChangeScope::Settings);
    Ok(Json(response))
}

pub(super) async fn reject_pending_pairing(
    _desktop: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, (StatusCode, String)> {
    if state.pairing_confirmation.reject().await {
        state.publish_state_changed(kanna_agent_protocol::StateChangeScope::Settings);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::CONFLICT,
            "no pairing confirmation is pending".into(),
        ))
    }
}

fn unix_time_ms() -> Result<u64, (StatusCode, String)> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

pub(super) async fn reissue_push_pairing_certificate(
    State(state): State<Arc<AppState>>,
    trusted: Option<Extension<TrustedLanDeviceAccess>>,
) -> Result<Json<PushPairingMaterial>, (StatusCode, String)> {
    let Some(Extension(trusted)) = trusted else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "pairing certificate re-issue requires a paired LAN device".to_string(),
        ));
    };
    let _persistence_mutation = state.pairing_persistence_mutation.lock().await;
    pairing_domain::reissue_push_pairing_certificate(&state.config, trusted.device_id())
        .map(Json)
        .map_err(|error| {
            let status = match error {
                PairingCertificateError::NotPaired => StatusCode::UNAUTHORIZED,
                PairingCertificateError::IdentityChanged => StatusCode::CONFLICT,
                PairingCertificateError::Persistence(_) => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, error.to_string())
        })
}

/// Only a device proving its own pairing secret can report its installed build.
pub(super) async fn report_mobile_build(
    State(state): State<Arc<AppState>>,
    trusted: Option<Extension<TrustedLanDeviceAccess>>,
    Json(build): Json<pairing_domain::MobileBuildReport>,
) -> Result<StatusCode, (StatusCode, String)> {
    let Some(Extension(trusted)) = trusted else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "build report requires a paired LAN device".into(),
        ));
    };
    if !matches!(build.environment.as_str(), "dev" | "staging" | "prod")
        || !matches!(build.channel.as_str(), "staging" | "production" | "None")
        || !matches!(
            build.source.as_str(),
            "ota" | "embedded" | "development" | "unknown"
        )
        || [
            &build.runtime_version,
            &build.native_version,
            &build.native_build,
            &build.update_id,
        ]
        .into_iter()
        .flatten()
        .any(|value| {
            value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control)
        })
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid mobile build report".into(),
        ));
    }
    let _mutation = state.pairing_persistence_mutation.lock().await;
    let path = std::path::Path::new(&state.config.pairing_store_path);
    let mut store = pairing_domain::PairingStore::load(path)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    let device = store
        .trusted_devices
        .get_mut(&state.config.desktop_id)
        .and_then(|devices| {
            devices
                .iter_mut()
                .find(|device| device.device_id == trusted.device_id())
        })
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "device is no longer paired".into(),
        ))?;
    device.mobile_build = Some(pairing_domain::MobileBuildObservation {
        build,
        reported_at_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    });
    store
        .save(path)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Explicit projection: never return pairing secrets or push credentials.
pub(super) async fn mobile_builds(
    _access: super::lan_trust::DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store =
        pairing_domain::PairingStore::load(std::path::Path::new(&state.config.pairing_store_path))
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    let devices: Vec<_> = store
        .trusted_devices
        .get(&state.config.desktop_id)
        .into_iter()
        .flatten()
        .map(|device| {
            serde_json::json!({
                "deviceId": device.device_id,
                "deviceName": device.device_name,
                "build": device.mobile_build,
                // Whether this device can only reach the desktop through a
                // secure-channel session (true) or still relies on the
                // legacy bearer secret (false, re-pair to upgrade).
                "secureChannel": device.channel_public_key.is_some(),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "desktopId": state.config.desktop_id,
        "devices": devices,
        "legacyAccessAllowed": state.legacy_mobile_access_allowed(),
    })))
}
