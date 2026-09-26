//! Who asked for a task mutation, recorded as two facts that are never merged.
//!
//! Every result, stage transition, workflow edit and delivered input carries a
//! **declared role** — the `source`/`trigger`/`origin` label the caller wrote
//! into its payload (`operator`, `manager`, `agent`, `unspecified`) or the
//! server's own label for work it did itself (`auto`, `engine`). Nothing
//! authenticates a declaration: a manager agent can write `operator`, and a
//! declared `operator` is never evidence that a human was present.
//!
//! Beside it sits the **channel identity**: what this server itself verified
//! about the connection the mutation arrived on, derived at the trust boundary
//! (`http_api::lan_trust`, the relay and KSP dispatchers in `http_api::router`)
//! and never read from a payload or a header a caller controls. A paired
//! phone's verified device id, a sibling desktop's verified id, the relay's
//! account attestation, or a real loopback socket are channel identities;
//! anything this server could not verify is [`ChannelIdentity::Unknown`].
//!
//! The two are recorded side by side so an audit can see when they disagree —
//! an `operator` declaration arriving over a sibling desktop's sealed session
//! is two facts, and neither is allowed to overwrite the other.
//!
//! What this is not: an authorization decision. Route access is still decided
//! by the existing extractors (`PrivilegedTaskAccess` and friends); a recorded
//! identity grants nothing, and historical rows that predate this record
//! decode as `Unknown` rather than being reconstructed from their labels.

use serde::{Deserialize, Serialize};

/// What this server verified about the channel a mutation arrived on.
///
/// Serialized as a tagged object (`{"kind": "pairedDevice", "deviceId": ..}`)
/// into SQL columns and event payloads. Only verified identifiers are stored:
/// never a credential, a key, or a claim a caller made about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum ChannelIdentity {
    /// This server's own engine: automatic policy transitions, subscription
    /// wakes, merge handoffs, recovery. Never assigned to a request.
    Server,
    /// A process on this machine that reached the real listener over a
    /// loopback socket (not a tunnel's synthetic loopback address).
    LocalProcess { evidence: LocalProcessEvidence },
    /// A paired phone whose pairing this server verified.
    #[serde(rename_all = "camelCase")]
    PairedDevice {
        device_id: String,
        evidence: PairedDeviceEvidence,
    },
    /// A paired sibling desktop whose identity this server verified.
    #[serde(rename_all = "camelCase")]
    PeerDesktop {
        desktop_id: String,
        evidence: PeerDesktopEvidence,
        /// The account the caller was verified under, when the evidence
        /// carries one: the account the LAN machine-trust secret was
        /// verified under, or the account a sealed session's pin proves the
        /// sibling shares (`crate::account_boundary`). Absent on rows written
        /// before either was recorded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account_uid: Option<String>,
    },
    /// A caller the relay authenticated for this account. The relay attests
    /// the account; `source_desktop_id` is present only when the relay also
    /// attested the calling desktop from its own connection-bound identity.
    #[serde(rename_all = "camelCase")]
    RelayAccount {
        account_uid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_desktop_id: Option<String>,
    },
    /// No verified channel evidence: a legacy record, an in-process dispatch
    /// with no identity attached, a tunnel whose caller proved nothing about
    /// itself, or a request without a socket peer.
    ///
    /// Also what any `kind` this build does not know decodes as, so a payload
    /// carrying an identity from a newer peer still decodes - and the
    /// identity it cannot read is never guessed at.
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum LocalProcessEvidence {
    /// A loopback socket that did not present the local control credential.
    Loopback,
    /// A loopback socket that also presented this desktop's local control
    /// credential.
    LocalControlCredential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PairedDeviceEvidence {
    /// The legacy LAN device id + secret (or the stream cookie derived from
    /// it) verified against the pairing store.
    LanDeviceSecret,
    /// A secure-channel handshake whose static key matched the paired device.
    SecureChannel { transport: SecureChannelTransport },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PeerDesktopEvidence {
    /// The legacy LAN machine-invoke bearer secret, verified by
    /// `machine_trust` under this desktop's current account.
    LanMachineTrust,
    /// A sealed peer session whose static key matched a paired sibling.
    SecureChannel { transport: SecureChannelTransport },
}

/// The socket a sealed session arrived on. The handshake, not the transport,
/// is what verified the identity; the transport is recorded because a relay
/// hop and a LAN hop are different facts to an audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SecureChannelTransport {
    Lan,
    Relay,
}

impl ChannelIdentity {
    /// Encode for a nullable TEXT column. `Unknown` is stored as its JSON too,
    /// so a row written after this record existed is distinguishable from a
    /// legacy NULL only by the column's presence, never by guesswork.
    pub(crate) fn to_column(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|error| {
            log::warn!("failed to encode a channel identity: {error}");
            r#"{"kind":"unknown"}"#.to_string()
        })
    }

    /// Decode a stored column. A NULL (legacy row) or an unreadable value is
    /// `Unknown`: provenance is never reconstructed from anything else.
    pub(crate) fn from_column(stored: Option<&str>) -> Self {
        match stored {
            None => Self::Unknown,
            Some(stored) => serde_json::from_str(stored).unwrap_or_else(|error| {
                log::warn!("failed to parse a stored channel identity: {error}");
                Self::Unknown
            }),
        }
    }

    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({ "kind": "unknown" }))
    }
}

/// One mutation's declared role and verified channel, as a pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MutationProvenance {
    /// The caller-declared (or server-owned) role label, recorded verbatim.
    pub(crate) declared_role: String,
    pub(crate) channel_identity: ChannelIdentity,
}

/// Declared role the server records for results it observed itself (a session
/// that exited, a spawn that failed) rather than one a caller submitted.
pub(crate) const ENGINE_DECLARED_ROLE: &str = "engine";

/// Declared role of `complete-stage`: the operation is an agent's report of its
/// own stage, so the label names the operation's convention. It is not a
/// verified claim that an agent made the call.
pub(crate) const AGENT_DECLARED_ROLE: &str = "agent";

impl MutationProvenance {
    pub(crate) fn new(declared_role: impl Into<String>, channel_identity: ChannelIdentity) -> Self {
        Self {
            declared_role: declared_role.into(),
            channel_identity,
        }
    }

    /// Something this server did itself.
    pub(crate) fn engine() -> Self {
        Self::new(ENGINE_DECLARED_ROLE, ChannelIdentity::Server)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_round_trip_through_their_column_encoding() {
        let identities = [
            ChannelIdentity::Unknown,
            ChannelIdentity::Server,
            ChannelIdentity::LocalProcess {
                evidence: LocalProcessEvidence::LocalControlCredential,
            },
            ChannelIdentity::PairedDevice {
                device_id: "phone-1".into(),
                evidence: PairedDeviceEvidence::SecureChannel {
                    transport: SecureChannelTransport::Relay,
                },
            },
            ChannelIdentity::PeerDesktop {
                desktop_id: "desk-2".into(),
                evidence: PeerDesktopEvidence::LanMachineTrust,
                account_uid: Some("uid-1".into()),
            },
            ChannelIdentity::RelayAccount {
                account_uid: "uid-1".into(),
                source_desktop_id: None,
            },
        ];
        for identity in identities {
            let column = identity.to_column();
            assert_eq!(ChannelIdentity::from_column(Some(&column)), identity);
        }
    }

    #[test]
    fn the_wire_shape_is_a_tagged_camel_case_object() {
        let identity = ChannelIdentity::PairedDevice {
            device_id: "phone-1".into(),
            evidence: PairedDeviceEvidence::SecureChannel {
                transport: SecureChannelTransport::Lan,
            },
        };
        assert_eq!(
            identity.to_json(),
            serde_json::json!({
                "kind": "pairedDevice",
                "deviceId": "phone-1",
                "evidence": { "secureChannel": { "transport": "lan" } },
            })
        );
        assert_eq!(
            ChannelIdentity::LocalProcess {
                evidence: LocalProcessEvidence::Loopback
            }
            .to_json(),
            serde_json::json!({ "kind": "localProcess", "evidence": "loopback" })
        );
    }

    /// A newer peer's identity kind inside a wire payload decodes as
    /// `Unknown` without failing the payload around it.
    #[test]
    fn an_unknown_kind_on_the_wire_decodes_as_unknown_and_keeps_the_payload() {
        let decoded: MutationProvenance = serde_json::from_value(serde_json::json!({
            "declaredRole": "operator",
            "channelIdentity": { "kind": "orgServer", "orgId": "org-1" },
        }))
        .expect("an unknown kind must not fail the payload");
        assert_eq!(decoded.declared_role, "operator");
        assert_eq!(decoded.channel_identity, ChannelIdentity::Unknown);
        // Known kinds still decode exactly, and `Unknown` still encodes as
        // its own kind.
        let known: ChannelIdentity =
            serde_json::from_value(serde_json::json!({ "kind": "server" })).unwrap();
        assert_eq!(known, ChannelIdentity::Server);
        assert_eq!(
            ChannelIdentity::Unknown.to_json(),
            serde_json::json!({ "kind": "unknown" })
        );
    }

    /// A legacy NULL or a value from a newer peer is never guessed at.
    #[test]
    fn missing_or_unreadable_columns_decode_as_unknown() {
        assert_eq!(ChannelIdentity::from_column(None), ChannelIdentity::Unknown);
        assert_eq!(
            ChannelIdentity::from_column(Some(r#"{"kind":"fromTheFuture"}"#)),
            ChannelIdentity::Unknown
        );
        assert_eq!(
            ChannelIdentity::from_column(Some("not json")),
            ChannelIdentity::Unknown
        );
    }
}
