//! Server-side derivation of a request's channel identity.
//!
//! The identity is read only from request extensions — markers the trust
//! boundary inserted after verifying something (`lan_trust`) and the
//! in-process dispatchers attached before the request entered the router
//! (`router::dispatch_*`). Payload fields and headers never reach it, so a
//! caller cannot talk its way into a channel identity; see
//! `crate::mutation_provenance` for what is recorded and why.

use super::lan_trust::{LocalControlCredential, TrustedLanDeviceAccess};
use super::state::{AppState, TunneledHttpInvoke};
use crate::mutation_provenance::{ChannelIdentity, LocalProcessEvidence, PairedDeviceEvidence};
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use std::net::SocketAddr;
use std::sync::Arc;

/// The identity an in-process dispatcher verified before building the
/// request. Private to this module and `router`: the only way to attach one is
/// [`attach_dispatched_channel_identity`], and a client cannot insert an
/// extension.
#[derive(Clone)]
struct DispatchedChannelIdentity(ChannelIdentity);

pub(super) fn attach_dispatched_channel_identity(
    extensions: &mut axum::http::Extensions,
    channel: ChannelIdentity,
) {
    extensions.insert(DispatchedChannelIdentity(channel));
}

/// Extractor for the request's channel identity. Never rejects: absent
/// evidence is [`ChannelIdentity::Unknown`], not a refusal — authorization is
/// the job of the existing access extractors, which this does not replace.
#[derive(Debug, Clone)]
pub(super) struct RequestChannel(pub(super) ChannelIdentity);

impl FromRequestParts<Arc<AppState>> for RequestChannel {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(channel_identity_of(&parts.extensions)))
    }
}

pub(super) fn channel_identity_of(extensions: &axum::http::Extensions) -> ChannelIdentity {
    // A tunnel's `ConnectInfo` is a synthetic loopback address and its device
    // markers were attached by the dispatcher, so only the identity that same
    // dispatcher recorded counts. A tunnel that attached none proved nothing
    // nameable and stays unknown — it is never "local".
    if extensions.get::<TunneledHttpInvoke>().is_some() {
        return extensions
            .get::<DispatchedChannelIdentity>()
            .map(|dispatched| dispatched.0.clone())
            .unwrap_or_default();
    }
    if let Some(device) = extensions.get::<TrustedLanDeviceAccess>() {
        return ChannelIdentity::PairedDevice {
            device_id: device.device_id().to_string(),
            evidence: PairedDeviceEvidence::LanDeviceSecret,
        };
    }
    match extensions.get::<ConnectInfo<SocketAddr>>() {
        Some(ConnectInfo(peer)) if peer.ip().is_loopback() => ChannelIdentity::LocalProcess {
            evidence: if extensions.get::<LocalControlCredential>().is_some() {
                LocalProcessEvidence::LocalControlCredential
            } else {
                LocalProcessEvidence::Loopback
            },
        },
        // No socket peer (an in-process router call) or a non-loopback peer
        // that proved no pairing: nothing verified.
        _ => ChannelIdentity::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutation_provenance::{PeerDesktopEvidence, SecureChannelTransport};

    fn loopback() -> ConnectInfo<SocketAddr> {
        ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50_000)))
    }

    #[test]
    fn a_real_loopback_socket_is_a_local_process() {
        let mut extensions = axum::http::Extensions::new();
        extensions.insert(loopback());
        assert_eq!(
            channel_identity_of(&extensions),
            ChannelIdentity::LocalProcess {
                evidence: LocalProcessEvidence::Loopback
            }
        );
        extensions.insert(LocalControlCredential);
        assert_eq!(
            channel_identity_of(&extensions),
            ChannelIdentity::LocalProcess {
                evidence: LocalProcessEvidence::LocalControlCredential
            }
        );
    }

    #[test]
    fn a_verified_lan_device_is_named_by_its_device_id() {
        let mut extensions = axum::http::Extensions::new();
        extensions.insert(ConnectInfo(SocketAddr::from(([192, 168, 1, 20], 50_000))));
        extensions.insert(TrustedLanDeviceAccess::new("phone-1".into()));
        assert_eq!(
            channel_identity_of(&extensions),
            ChannelIdentity::PairedDevice {
                device_id: "phone-1".into(),
                evidence: PairedDeviceEvidence::LanDeviceSecret,
            }
        );
    }

    #[test]
    fn a_remote_socket_without_pairing_or_a_missing_peer_is_unknown() {
        let mut extensions = axum::http::Extensions::new();
        assert_eq!(channel_identity_of(&extensions), ChannelIdentity::Unknown);
        extensions.insert(ConnectInfo(SocketAddr::from(([192, 168, 1, 20], 50_000))));
        assert_eq!(channel_identity_of(&extensions), ChannelIdentity::Unknown);
    }

    /// The synthetic loopback address every in-process dispatch carries must
    /// never read as a local process, and a device marker on a tunnel counts
    /// only through the identity its dispatcher recorded.
    #[test]
    fn a_tunnel_is_only_what_its_dispatcher_recorded() {
        let mut extensions = axum::http::Extensions::new();
        extensions.insert(loopback());
        extensions.insert(TunneledHttpInvoke);
        extensions.insert(LocalControlCredential);
        extensions.insert(TrustedLanDeviceAccess::new("phone-1".into()));
        assert_eq!(channel_identity_of(&extensions), ChannelIdentity::Unknown);

        let sealed_peer = ChannelIdentity::PeerDesktop {
            desktop_id: "desk-2".into(),
            evidence: PeerDesktopEvidence::SecureChannel {
                transport: SecureChannelTransport::Relay,
            },
            account_uid: None,
        };
        attach_dispatched_channel_identity(&mut extensions, sealed_peer.clone());
        assert_eq!(channel_identity_of(&extensions), sealed_peer);
    }
}
