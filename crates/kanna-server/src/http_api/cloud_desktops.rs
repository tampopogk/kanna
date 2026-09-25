use super::lan_trust::DesktopLocalAccess;
use super::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MachineDescriptor {
    id: String,
    name: Option<String>,
    is_local: bool,
    /// How this desktop reaches the machine: `local` (itself), `e2ee` (a
    /// pinned sibling over its sealed peer session), or `legacy` (an
    /// unpinned sibling over the relay-attested or bearer-secret path -
    /// which the sibling itself refuses from 0.4.0 on, so in practice only
    /// a desktop older than that).
    encryption: &'static str,
    /// For an `e2ee` machine, how its pin was born: `verified` (the pairing
    /// string a person carried) or `account` (automatic same-account
    /// enrollment). `None` for every other value of `encryption`.
    #[serde(skip_serializing_if = "Option::is_none")]
    provenance: Option<&'static str>,
    /// A handshake against that machine's pin met a different key and has
    /// not succeeded since.
    identity_changed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MachineListResponse {
    current_machine_id: String,
    relay_available: bool,
    machines: Vec<MachineDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MachineInvokeRequest {
    method: String,
    path: String,
    #[serde(default)]
    body: serde_json::Value,
}

pub(super) async fn list_cloud_desktops(
    _access: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
) -> Json<MachineListResponse> {
    let current_id = state.config().desktop_id.clone();
    let mut ids = vec![current_id.clone()];
    let mut enrollable: Vec<String> = Vec::new();
    let (relay_available, error) = if state.desktop_routing_available() {
        match state.list_active_relay_desktop_presence().await {
            Ok(active) => {
                enrollable.extend(
                    active
                        .iter()
                        .filter(|entry| {
                            entry.desktop_id != current_id
                                && entry.peer_channel_public_key.is_some()
                        })
                        .map(|entry| entry.desktop_id.clone()),
                );
                ids.extend(active.into_iter().map(|entry| entry.desktop_id));
                (true, None)
            }
            Err(error) => (false, Some(error)),
        }
    } else {
        (false, Some(state.desktop_routing_unavailable_reason()))
    };
    // A trusted discovered LAN peer must not disappear from machine
    // discovery merely because relay happens to be unavailable - added
    // unconditionally, alongside relay presence rather than instead of it,
    // and never affecting `relay_available`/`error`, which report relay's
    // own dimension exactly as before.
    // No longer gated on the legacy switch here: `eligible_lan_desktop_ids`
    // now consults it itself, and also counts a pinned peer, whose sealed
    // session needs no legacy route at all. Gating it a second time would
    // hide those pinned peers in exactly the configuration - legacy refused -
    // where they are the only reachable LAN machines left.
    ids.extend(super::invoke_desktop::eligible_lan_desktop_ids(&state));
    // A paired sibling with a LAN candidate is reachable without the relay.
    let paired: Vec<crate::peer_trust::PeerDesktop> = state
        .peer_trust_store()
        .map(|store| store.peers)
        .unwrap_or_default()
        .into_iter()
        .filter(|peer| peer.environment == state.config().environment)
        .collect();
    ids.extend(
        paired
            .iter()
            .filter(|peer| state.lan_api_candidate_for(&peer.desktop_id).is_some())
            .map(|peer| peer.desktop_id.clone()),
    );
    // Same-account siblings the relay can introduce are enrolled eagerly, in
    // the background, so both directions are pinned from one exchange and
    // this list reports `e2ee` on its next refresh without anyone having to
    // open a session first. `try_enroll` is a no-op for anyone already
    // pinned and is guarded against concurrent attempts per target, so this
    // is idempotent however often the list is refreshed.
    for desktop_id in enrollable {
        if matches!(state.paired_peer(&desktop_id), Ok(None)) {
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                if let Err(error) = crate::peer_enrollment::try_enroll(&state, &desktop_id).await {
                    log::debug!("[peer] {desktop_id} was not enrolled automatically: {error}");
                }
            });
        }
    }
    ids.sort();
    ids.dedup();
    let machines = ids
        .into_iter()
        .map(|id| {
            let is_local = id == current_id;
            let peer = paired.iter().find(|peer| peer.desktop_id == id);
            MachineDescriptor {
                name: if is_local {
                    Some(state.config().desktop_name.clone())
                } else {
                    peer.map(|peer| peer.display_name.clone())
                },
                encryption: if is_local {
                    "local"
                } else if peer.is_some() {
                    "e2ee"
                } else {
                    "legacy"
                },
                provenance: peer.map(|peer| peer.provenance.as_str()),
                identity_changed: peer
                    .is_some_and(|peer| peer.identity_mismatch_at_unix_ms.is_some()),
                id,
                is_local,
            }
        })
        .collect();
    Json(MachineListResponse {
        current_machine_id: current_id,
        relay_available,
        machines,
        error,
    })
}

/// `HttpInvokeResponse`'s fields plus which transport actually served this
/// call. A separate response shape rather than a new field on
/// `HttpInvokeResponse` itself: that struct has a dozen other construction
/// sites across this crate that have nothing to do with desktop-to-desktop
/// routing, and none of them need to grow a route to reason about.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MachineInvokeHttpResponse {
    status: u16,
    body: Option<serde_json::Value>,
    error: Option<String>,
    /// "local" | "lan" | "relay" - see `invoke_desktop::RouteProvenance`.
    route: &'static str,
}

pub(super) async fn invoke_cloud_desktop(
    _access: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
    Path(desktop_id): Path<String>,
    Json(request): Json<MachineInvokeRequest>,
) -> Result<Json<MachineInvokeHttpResponse>, (axum::http::StatusCode, String)> {
    validate_invoke_request(&desktop_id, &request)?;
    let routed = super::invoke_desktop::invoke_desktop(
        state,
        desktop_id,
        request.method,
        request.path,
        request.body,
    )
    .await
    .map_err(|error| (axum::http::StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(MachineInvokeHttpResponse {
        status: routed.response.status,
        body: routed.response.body,
        error: routed.response.error,
        route: routed.route.as_str(),
    }))
}

fn validate_invoke_request(
    desktop_id: &str,
    request: &MachineInvokeRequest,
) -> Result<(), (axum::http::StatusCode, String)> {
    if desktop_id.trim().is_empty() || desktop_id.chars().any(char::is_control) {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "machine id must not be empty or contain control characters".to_string(),
        ));
    }
    if !matches!(request.method.as_str(), "GET" | "POST" | "PATCH") {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "remote method must be GET, POST, or PATCH".to_string(),
        ));
    }
    if !request.path.starts_with("/v1/")
        || request.path.contains("://")
        || request.path.chars().any(char::is_control)
        || request.path.starts_with("/v1/cloud/desktops")
    {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "remote path must be a non-recursive /v1/ API path".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, State};

    /// A genuinely isolated per-desktop config, deliberately not
    /// `test_state_with_seed` - that shared helper places `pairing_store_path`
    /// directly under one shared temp root with only the *filename* made
    /// unique, so `Config::machine_trust_store_path`/`lan_tls_identity_path`
    /// (both derived from `pairing_store_path`'s *parent* directory) resolve
    /// to the exact same path for every desktop that uses it - fine for
    /// this file's existing single-desktop tests, but a real collision for
    /// a two-desktop LAN scenario (confirmed directly: an earlier version of
    /// this test using `test_state_with_seed` for both desktops had the
    /// source's later `.save()` silently overwrite the target's earlier
    /// inbound grant, since both resolved to one shared `machine-trust.json`,
    /// exactly the class of collision `crate::test_paths`'s own doc
    /// comments warn about. Mirrors `invoke_desktop.rs`'s own
    /// `lan_e2e_test_config` pattern, which does not share this bug.
    fn isolated_lan_test_config(desktop_id: &str) -> crate::config::Config {
        let dir = crate::test_paths::unique_test_dir(&format!("cloud-desktops-lan-{desktop_id}"));
        // The legacy desktop-to-desktop switch fails closed without a
        // settings database; the legacy LAN route under test needs one.
        let db_path = crate::db::Db::test_db_path(&format!("cloud-desktops-lan-{desktop_id}"));
        let _ = crate::db::Db::open_for_tests(&db_path).expect("open test db");
        crate::config::Config {
            relay_url: String::new(),
            device_token: "device-token".to_string(),
            firebase_project_id: "kanna-local".to_string(),
            firebase_auth_emulator_url: None,
            firebase_firestore_emulator_host: None,
            daemon_dir: dir.join("daemon").to_string_lossy().into_owned(),
            db_path,
            kanna_cli_path: None,
            desktop_id: desktop_id.to_string(),
            desktop_secret: Some("desktop-secret".to_string()),
            desktop_name: format!("{desktop_id} Mac"),
            version: "test-version".to_string(),
            environment: "development".to_string(),
            lan_host: "127.0.0.1".to_string(),
            lan_port: 48120,
            transfer_port: 4455,
            lan_routing_port: 4460,
            activity_event_debounce_seconds: 300,
            pairing_store_path: dir.join("pairings.json").to_string_lossy().into_owned(),
        }
    }

    /// Real CLI/MCP route provenance, at the exact HTTP/JSON contract those
    /// surfaces parse - `crates/kanna-cli/src/commands/tool.rs`'s
    /// `machine_status_with_route` and `crates/kanna-mcp/src/main.rs`'s
    /// equivalent both read `MachineInvokeHttpResponse.route` off this
    /// handler's real JSON response, not off `invoke_desktop` the Rust
    /// function directly. Every existing real-LAN-dial test in
    /// `invoke_desktop.rs` calls that function directly, one layer below
    /// the actual HTTP handler CLI/MCP talk to - this proves the boundary
    /// those two crates actually depend on, through the real axum handler,
    /// with a real pinned-TLS dial to an explicitly-seeded candidate (not
    /// real mDNS discovery, which remains blocked on this host - see the
    /// task checkpoint).
    #[tokio::test]
    async fn invoke_cloud_desktop_reports_lan_route_provenance_in_its_real_http_response() {
        let target_config = isolated_lan_test_config("desktop-cli-mcp-target");
        let target_state = Arc::new(AppState::new(target_config));
        target_state.set_authenticated_account_uid(Some("uid-1".to_string()));
        let target_identity_path = target_state.config().lan_tls_identity_path().unwrap();
        let target_identity = crate::lan_tls_identity::load_or_create(
            &target_identity_path,
            &target_state.config().desktop_id,
            &target_state.config().environment,
        )
        .expect("create target identity");

        let now_ms = crate::machine_trust::unix_time_ms().unwrap();
        let target_store_path = target_state.config().machine_trust_store_path().unwrap();
        {
            let mut store = crate::machine_trust::MachineTrustStore::default();
            let hash = crate::pairing::hash_device_secret("the-bearer-secret");
            store.accept_inbound(
                "desktop-cli-mcp-source",
                &hash,
                "uid-1",
                "development",
                &target_state.config().desktop_id,
                now_ms,
            );
            store.save(&target_store_path).expect("seed target trust");
        }

        let listener_addr =
            super::super::lan_listener::spawn_for_test(Arc::clone(&target_state)).await;
        let candidate = std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            listener_addr.port(),
        );

        let source_config = isolated_lan_test_config("desktop-cli-mcp-source");
        let source_state = Arc::new(AppState::new(source_config));
        source_state.set_authenticated_account_uid(Some("uid-1".to_string()));
        source_state.set_lan_candidate("desktop-cli-mcp-target".to_string(), candidate);
        let source_store_path = source_state.config().machine_trust_store_path().unwrap();
        {
            let mut store = crate::machine_trust::MachineTrustStore::default();
            store
                .pending_or_create(
                    "desktop-cli-mcp-target",
                    "uid-1",
                    "development",
                    &source_state.config().desktop_id,
                    || Ok("the-bearer-secret".to_string()),
                    now_ms,
                )
                .expect("prepare pending");
            store
                .confirm_outbound(
                    "desktop-cli-mcp-target",
                    "the-bearer-secret",
                    &source_state.config().desktop_id,
                    Some(target_identity.ca_certificate_pem.clone()),
                    now_ms + 1000,
                )
                .expect("confirm outbound grant");
            store.save(&source_store_path).expect("seed source trust");
        }

        let response = invoke_cloud_desktop(
            DesktopLocalAccess,
            State(Arc::clone(&source_state)),
            Path("desktop-cli-mcp-target".to_string()),
            Json(MachineInvokeRequest {
                method: "GET".to_string(),
                path: "/v1/status".to_string(),
                body: serde_json::Value::Null,
            }),
        )
        .await
        .expect("the real HTTP handler must complete")
        .0;

        assert_eq!(
            response.route, "lan",
            "CLI/MCP's exact route-provenance contract must report \"lan\" for a real LAN \
             dial's actual HTTP response: {response:?}"
        );
        // The target runs this same build, so its own listener refuses the
        // legacy bearer path (2026-09-20). The contract under test is the
        // route provenance and the definiteness of the answer, both of
        // which hold; only the status moved from 200 to 401.
        assert_eq!(response.status, 401, "{response:?}");
    }

    #[test]
    fn validates_catalog_http_requests_and_rejects_recursive_proxying() {
        let request = MachineInvokeRequest {
            method: "GET".to_string(),
            path: "/v1/tasks/recent".to_string(),
            body: serde_json::Value::Null,
        };
        assert!(validate_invoke_request("desktop-two", &request).is_ok());

        let recursive = MachineInvokeRequest {
            path: "/v1/cloud/desktops/desktop-two/invoke".to_string(),
            ..request
        };
        assert!(validate_invoke_request("desktop-two", &recursive).is_err());
    }

    /// Finding #2's LAN-peer-enumeration completion: a trusted discovered
    /// LAN peer must not vanish from machine discovery merely because relay
    /// happens to be unavailable. `desktop_routing_available()` defaults to
    /// `false` on a freshly-constructed `AppState` (no relay setup here at
    /// all), matching a genuine outage; the LAN peer must still be listed,
    /// and `relay_available`/`error` must still truthfully report the
    /// outage as its own separate dimension.
    #[tokio::test]
    async fn a_trusted_lan_peer_is_listed_even_while_relay_is_unavailable() {
        let state =
            crate::http_api::test_state_with_seed("desktop-observer", "Observer Mac", |_db| {});
        state.set_authenticated_account_uid(Some("uid-1".to_string()));
        state.set_lan_candidate(
            "desktop-lan-peer".to_string(),
            "127.0.0.1:1".parse().unwrap(),
        );
        // A pin, which is what "trusted" means for a LAN peer now: it is the
        // only trust that is still a route, so it is the only one whose
        // machine may be claimed reachable while relay is down.
        {
            let path = state.config().peer_trust_store_path().unwrap();
            let mut store = crate::peer_trust::PeerTrustStore::load(&path).unwrap();
            store
                .upsert(crate::peer_trust::PeerDesktop {
                    desktop_id: "desktop-lan-peer".into(),
                    display_name: "LAN Peer".into(),
                    channel_public_key: kanna_secure_channel::Keypair::generate()
                        .unwrap()
                        .encoded_public_key(),
                    transfer_peer_id: None,
                    transfer_public_key: None,
                    environment: state.config().environment.clone(),
                    account_uid: Some("uid-1".into()),
                    provenance: crate::peer_trust::PeerProvenance::Verified,
                    account_verified_at_unix_ms: Some(1),
                    identity_mismatch_at_unix_ms: None,
                    paired_at_unix_ms: 1,
                    last_seen_unix_ms: None,
                })
                .unwrap();
            store.save(&path).unwrap();
        }

        let response = list_cloud_desktops(DesktopLocalAccess, State(Arc::clone(&state))).await;

        assert!(
            !response.relay_available,
            "relay must still truthfully report unavailable"
        );
        assert!(
            response.error.is_some(),
            "the relay outage must still be reported as its own error"
        );
        assert!(
            response.machines.iter().any(|m| m.id == "desktop-lan-peer"),
            "a trusted discovered LAN peer must still be listed: {:?}",
            response.machines
        );
    }

    /// The negative case: a discovered candidate with no established trust
    /// grant is not a machine this desktop can reach, and must not appear.
    #[tokio::test]
    async fn an_untrusted_discovered_candidate_is_not_listed() {
        let state =
            crate::http_api::test_state_with_seed("desktop-observer-2", "Observer Mac", |_db| {});
        state.set_authenticated_account_uid(Some("uid-1".to_string()));
        state.set_lan_candidate(
            "desktop-untrusted-peer".to_string(),
            "127.0.0.1:1".parse().unwrap(),
        );

        let response = list_cloud_desktops(DesktopLocalAccess, State(Arc::clone(&state))).await;

        assert!(
            !response
                .machines
                .iter()
                .any(|m| m.id == "desktop-untrusted-peer"),
            "a merely-discovered, never-trusted candidate must not be listed: {:?}",
            response.machines
        );
    }
}
