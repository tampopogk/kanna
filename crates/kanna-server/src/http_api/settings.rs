use super::lan_trust::DesktopLocalAccess;
use super::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use kanna_agent_protocol::StateChangeScope;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(crate) const CLOUD_TRANSFER_IDENTITY_SETTING: &str = "cloud_transfer_identity_v1";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SettingResponse {
    key: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PutSettingRequest {
    value: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CloudTransferIdentity {
    pub(crate) peer_id: String,
    pub(crate) display_name: String,
    pub(crate) public_key: String,
    pub(crate) protocol_version: u16,
    pub(crate) accepting_transfers: bool,
}

pub(super) async fn put_cloud_transfer_identity(
    _desktop: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
    Json(identity): Json<CloudTransferIdentity>,
) -> Result<Json<SettingResponse>, (axum::http::StatusCode, String)> {
    validate_cloud_transfer_identity(&identity)?;
    let value = serde_json::to_string(&identity).map_err(|error| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to encode cloud transfer identity: {error}"),
        )
    })?;
    state
        .with_settings_db(|db| db.set_setting(CLOUD_TRANSFER_IDENTITY_SETTING, &value))
        .map_err(|error| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {error}"),
            )
        })?;
    state.publish_state_changed(StateChangeScope::Settings);
    Ok(Json(SettingResponse {
        key: CLOUD_TRANSFER_IDENTITY_SETTING.into(),
        value,
    }))
}

fn validate_cloud_transfer_identity(
    identity: &CloudTransferIdentity,
) -> Result<(), (axum::http::StatusCode, String)> {
    for (field, value, maximum) in [
        ("peerId", identity.peer_id.as_str(), 256),
        ("displayName", identity.display_name.as_str(), 256),
        ("publicKey", identity.public_key.as_str(), 4096),
    ] {
        if value.trim().is_empty() || value.chars().count() > maximum {
            return Err((
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                format!("{field} must be nonblank and at most {maximum} characters"),
            ));
        }
    }
    if identity.protocol_version == 0 {
        return Err((
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "protocolVersion must be positive".into(),
        ));
    }
    Ok(())
}

pub(super) async fn get_setting(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Result<Json<SettingResponse>, (axum::http::StatusCode, String)> {
    let value = state
        .with_settings_db(|db| db.get_setting(&key))
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {e}"),
            )
        })?
        .ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                format!("setting not found: {key}"),
            )
        })?;
    Ok(Json(SettingResponse { key, value }))
}

/// Settings are this desktop's own controls, so writing one is
/// `DesktopLocalAccess` - the class `docs/specs/secure-channel.md` §5 and §10
/// already promise ("`DesktopLocalAccess` routes (pairing controls, settings)
/// stay refused" for a paired phone, "never `DesktopLocalAccess` (pairing
/// controls, settings, the peer list)" for a paired sibling desktop). The
/// promise was only ever kept for `put_cloud_transfer_identity`: these two
/// handlers carried no authority extractor at all, so they sat on the
/// `require_http_access` floor and any tunneled caller that cleared it - a
/// relay-authenticated invoke, a paired phone's sealed session, a paired
/// sibling's sealed peer session - could write `mobile_legacy_access` or
/// `terminalEditorCommand` on this machine. The first is the switch that
/// decides whether this desktop still accepts the pre-E2EE mobile paths
/// (the desktop-to-desktop one is gone); the second is the command line
/// `terminal_editor::editor_choices` resolves to the executable the daemon
/// spawns the next time the person here opens a file in a terminal editor.
pub(super) async fn put_setting(
    _desktop: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    Json(payload): Json<PutSettingRequest>,
) -> Result<Json<SettingResponse>, (axum::http::StatusCode, String)> {
    reject_reserved_setting_mutation(&key)?;
    state
        .with_settings_db(|db| db.set_setting(&key, &payload.value))
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {e}"),
            )
        })?;
    state.publish_state_changed(StateChangeScope::Settings);
    Ok(Json(SettingResponse {
        key,
        value: payload.value,
    }))
}

/// Deleting a setting is a mutation like [`put_setting`], and reverting one of
/// the switches above to its default is worth exactly as much to a remote
/// caller as setting it, so it takes the same authority.
pub(super) async fn delete_setting(
    _desktop: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    reject_reserved_setting_mutation(&key)?;
    state
        .with_settings_db(|db| db.delete_setting(&key))
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {e}"),
            )
        })?;
    state.publish_state_changed(StateChangeScope::Settings);
    Ok(Json(serde_json::json!({ "key": key })))
}

fn reject_reserved_setting_mutation(key: &str) -> Result<(), (axum::http::StatusCode, String)> {
    if key == CLOUD_TRANSFER_IDENTITY_SETTING {
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            "cloud transfer identity must use the desktop-local identity endpoint".into(),
        ));
    }
    Ok(())
}
