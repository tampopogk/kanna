//! The same-account boundary: which verified callers may control this
//! desktop's tasks, decided by the account the caller was verified under.
//!
//! Task control, input and transfer cross between machines of one account
//! only; nothing crosses an account boundary except artifacts
//! (docs/specs/tasks-sessions-structured-workflows.md §8, §12). Every
//! transport that can carry another machine's request answers the same
//! question here, from evidence the transport itself verified:
//!
//! - **Relay invoke** (`relay::dispatch_relay_http_invoke`): the account is
//!   the one this desktop's relay connection authenticated as when it
//!   received the request. It must still be the account this desktop is
//!   signed in to; a connection that authenticated none carries nothing.
//! - **LAN machine invoke** (`http_api::lan_listener`): the account is the
//!   one `machine_trust` verified the bearer secret under. It must still be
//!   the current account when the request is dispatched.
//! - **Sealed peer session** (`ksp`, LAN or relay): the account is the one
//!   the pinned record *proves* the sibling shares
//!   ([`PeerDesktop::same_account_evidence`]). A record without that proof —
//!   a pairing made while signed out, or a ceremony pin from before the
//!   ceremony checked the sibling's account — is refused by name, with the
//!   re-pairing that restores it; it is never read as same-account.
//!
//! Not here, on purpose: a real loopback process is this machine's own user
//! (process authentication, not an account claim), and a phone paired by QR
//! is a device of this desktop rather than another machine of the account —
//! LAN QR pairing needs no account by owner ruling
//! (docs/specs/accounts-and-billing.md, "What stays free"), and a phone
//! arriving through the relay is already gated by the relay account access
//! that sign-out and an account change reset (`RelayAccess`).
//!
//! A refusal is never a fallback: a caller refused here gets the refusal,
//! not a quieter local answer. And the account a caller was verified under
//! is authority only for *which machines* may act; it is never evidence
//! that a person was present (`crate::mutation_provenance`).

use crate::peer_trust::{PeerDesktop, PeerProvenance};
use axum::http::StatusCode;

/// Why a verified caller may not control this desktop's tasks. Each names
/// what was refused and what restores access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AccountBoundaryRefusal {
    /// This desktop is signed in to no account, or its relay has not yet
    /// confirmed one, so no other machine can be same-account with it.
    SignedOut,
    /// A pinned sibling whose record does not prove it shares this
    /// desktop's account.
    PeerWithoutAccountEvidence {
        desktop_id: String,
        display_name: String,
        paired_while_signed_out: bool,
    },
    /// A pinned sibling proven under an account this desktop is no longer
    /// signed in to (the account-transition purge has not run yet).
    PeerAccountChanged {
        desktop_id: String,
        display_name: String,
    },
    /// A relay invoke on a connection that authenticated no account.
    RelayAccountUnattested,
    /// A relay invoke received on a connection authenticated as an account
    /// this desktop is no longer signed in to.
    RelayAccountChanged,
    /// A LAN machine invoke whose credential was verified under an account
    /// this desktop is no longer signed in to.
    LanCredentialAccountChanged { desktop_id: String },
}

impl AccountBoundaryRefusal {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::SignedOut => "account_signed_out",
            Self::PeerWithoutAccountEvidence { .. } => "peer_account_evidence_missing",
            Self::PeerAccountChanged { .. } => "peer_account_changed",
            Self::RelayAccountUnattested => "relay_account_unattested",
            Self::RelayAccountChanged => "relay_account_changed",
            Self::LanCredentialAccountChanged { .. } => "lan_machine_account_changed",
        }
    }

    /// The text a sealed-session refusal is answered with. That answer
    /// travels in the clear (the refused dialer has no channel to read a
    /// sealed one), so it names neither machine: the refusing desktop logs
    /// the full diagnostic and reports it in its machine list, and the
    /// dialer knows which machine it dialed.
    pub(crate) fn wire_message(&self) -> String {
        match self {
            Self::PeerWithoutAccountEvidence { .. } => format!(
                "{}: the other machine's pairing record for this desktop does not show that both \
                 are signed in to one Kanna account; sign both in to the same account, then \
                 unpair and reconnect, or pair again with a pairing string",
                self.code()
            ),
            Self::PeerAccountChanged { .. } => format!(
                "{}: the other machine is signed in to a different Kanna account than the one \
                 this desktop was paired under; pair again under one account",
                self.code()
            ),
            _ => self.to_string(),
        }
    }

    pub(crate) fn status(&self) -> StatusCode {
        match self {
            Self::RelayAccountUnattested => StatusCode::UNAUTHORIZED,
            _ => StatusCode::FORBIDDEN,
        }
    }
}

impl std::fmt::Display for AccountBoundaryRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: ", self.code())?;
        match self {
            Self::SignedOut => formatter.write_str(
                "this desktop is not signed in to a Kanna account (or its relay has not confirmed \
                 one yet), so no other machine may control its tasks; sign in and retry",
            ),
            Self::PeerWithoutAccountEvidence {
                desktop_id,
                display_name,
                paired_while_signed_out,
            } => {
                let how = if *paired_while_signed_out {
                    "while this desktop was signed out"
                } else {
                    "with a pairing string before pairing checked the other machine's account"
                };
                write!(
                    formatter,
                    "paired machine \"{display_name}\" ({desktop_id}) was paired {how}, so its \
                     record does not show that it is signed in to this desktop's account and it \
                     may not control tasks here. To restore access, sign both machines in to the \
                     same Kanna account, then unpair \"{display_name}\" in Preferences → Machines \
                     and reconnect (machines on one account pair again automatically), or pair \
                     them again with a pairing string"
                )
            }
            Self::PeerAccountChanged {
                desktop_id,
                display_name,
            } => write!(
                formatter,
                "paired machine \"{display_name}\" ({desktop_id}) was paired under a different \
                 account than the one this desktop is signed in to now, so it may not control \
                 tasks here; sign it in to this account and pair it again"
            ),
            Self::RelayAccountUnattested => formatter.write_str(
                "this relay connection has not authenticated an account, so it cannot carry \
                 task control",
            ),
            Self::RelayAccountChanged => formatter.write_str(
                "this request arrived on a relay connection authenticated as an account this \
                 desktop is no longer signed in to; retry once the relay has reconnected",
            ),
            Self::LanCredentialAccountChanged { desktop_id } => write!(
                formatter,
                "the LAN machine credential of {desktop_id} was verified under an account this \
                 desktop is no longer signed in to; retry"
            ),
        }
    }
}

/// Every refusal code, for recognising a sibling's refusal on the wire.
const REFUSAL_CODES: [&str; 6] = [
    "account_signed_out",
    "peer_account_evidence_missing",
    "peer_account_changed",
    "relay_account_unattested",
    "relay_account_changed",
    "lan_machine_account_changed",
];

/// Whether `text` is an account-boundary refusal (the `{code}: ...` shape
/// every refusal renders as).
pub(crate) fn is_refusal_text(text: &str) -> bool {
    REFUSAL_CODES.iter().any(|code| {
        text.strip_prefix(code)
            .is_some_and(|rest| rest.starts_with(':'))
    })
}

/// Whether the pinned sibling `peer` may control this desktop's tasks
/// while this desktop is signed in as `current_account_uid`.
///
/// A record without evidence is refused first, whatever the sign-in state:
/// signing in does not give it evidence, so naming the record and its
/// repair is the actionable answer.
pub(crate) fn peer_standing(
    peer: &PeerDesktop,
    current_account_uid: Option<&str>,
) -> Result<(), AccountBoundaryRefusal> {
    let Some(proven) = peer.same_account_evidence() else {
        return Err(AccountBoundaryRefusal::PeerWithoutAccountEvidence {
            desktop_id: peer.desktop_id.clone(),
            display_name: peer.display_name.clone(),
            paired_while_signed_out: peer.account_uid.is_none(),
        });
    };
    match current_account_uid {
        None => Err(AccountBoundaryRefusal::SignedOut),
        Some(current) if current == proven => Ok(()),
        Some(_) => Err(AccountBoundaryRefusal::PeerAccountChanged {
            desktop_id: peer.desktop_id.clone(),
            display_name: peer.display_name.clone(),
        }),
    }
}

/// The account a relay invoke acts as: the one its connection authenticated,
/// provided this desktop is still signed in to exactly that account.
pub(crate) fn relay_invoke_account(
    connection_account_uid: Option<&str>,
    current_account_uid: Option<&str>,
) -> Result<String, AccountBoundaryRefusal> {
    let Some(connection) = connection_account_uid else {
        return Err(AccountBoundaryRefusal::RelayAccountUnattested);
    };
    match current_account_uid {
        None => Err(AccountBoundaryRefusal::SignedOut),
        Some(current) if current == connection => Ok(connection.to_string()),
        Some(_) => Err(AccountBoundaryRefusal::RelayAccountChanged),
    }
}

/// The account a LAN machine invoke acts as: the one its bearer secret was
/// verified under, provided this desktop is still signed in to it.
pub(crate) fn lan_invoke_account(
    source_desktop_id: &str,
    verified_account_uid: Option<&str>,
    current_account_uid: Option<&str>,
) -> Result<String, AccountBoundaryRefusal> {
    match (verified_account_uid, current_account_uid) {
        (_, None) | (None, _) => Err(AccountBoundaryRefusal::SignedOut),
        (Some(verified), Some(current)) if verified == current => Ok(verified.to_string()),
        (Some(_), Some(_)) => Err(AccountBoundaryRefusal::LanCredentialAccountChanged {
            desktop_id: source_desktop_id.to_string(),
        }),
    }
}

/// A pinned sibling's standing, as the machine list reports it: `sameAccount`
/// or the refusal code, and the refusal's full text naming the record and
/// the action that restores it.
pub(crate) fn peer_standing_report(
    peer: &PeerDesktop,
    current_account_uid: Option<&str>,
) -> (&'static str, Option<String>) {
    match peer_standing(peer, current_account_uid) {
        Ok(()) => ("sameAccount", None),
        Err(refusal) => (refusal.code(), Some(refusal.to_string())),
    }
}

/// Whether the relay, speaking for this desktop's current account, lists
/// `desktop_id` with exactly `channel_public_key` right now — the same
/// evidence automatic enrollment pins on. Returns the account it was listed
/// under. The relay can only withhold this, never substitute a key: the key
/// compared is the one the handshake authenticated or the person carried.
pub(crate) async fn relay_confirms_sibling(
    state: &crate::http_api::AppState,
    desktop_id: &str,
    channel_public_key: &str,
) -> Result<String, String> {
    let Some(account_uid) = state.authenticated_account_uid() else {
        return Err("this desktop is not signed in to a Kanna account".to_string());
    };
    let presence = state
        .list_active_relay_desktop_presence()
        .await
        .map_err(|error| {
            format!("the relay cannot confirm the other machine's account: {error}")
        })?;
    let listed = presence
        .into_iter()
        .find(|entry| entry.desktop_id == desktop_id)
        .and_then(|entry| entry.peer_channel_public_key);
    match listed {
        Some(key) if key == channel_public_key => {
            // The listing is only meaningful for the account it was taken
            // under; an account change while it was in flight voids it.
            if state.authenticated_account_uid().as_deref() == Some(account_uid.as_str()) {
                Ok(account_uid)
            } else {
                Err("this desktop's account changed while pairing".to_string())
            }
        }
        Some(_) => Err(format!(
            "your account lists machine {desktop_id} with a different key"
        )),
        None => Err(format!(
            "machine {desktop_id} is not signed in to this desktop's Kanna account right now"
        )),
    }
}

/// The provenance label a refused record is reported with, for logs.
pub(crate) fn provenance_label(peer: &PeerDesktop) -> &'static str {
    match peer.provenance {
        PeerProvenance::Verified => "pairing string",
        PeerProvenance::Account => "automatic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(account: Option<&str>, provenance: PeerProvenance, stamped: bool) -> PeerDesktop {
        PeerDesktop {
            desktop_id: "desktop-b".into(),
            display_name: "B Mac".into(),
            channel_public_key: "key-b".into(),
            transfer_peer_id: None,
            transfer_public_key: None,
            environment: "development".into(),
            account_uid: account.map(str::to_string),
            provenance,
            account_verified_at_unix_ms: stamped.then_some(1),
            identity_mismatch_at_unix_ms: None,
            paired_at_unix_ms: 1,
            last_seen_unix_ms: None,
        }
    }

    #[test]
    fn a_proven_same_account_peer_is_admitted_only_while_that_account_is_current() {
        let automatic = peer(Some("uid-1"), PeerProvenance::Account, false);
        assert_eq!(peer_standing(&automatic, Some("uid-1")), Ok(()));
        assert_eq!(
            peer_standing(&automatic, None),
            Err(AccountBoundaryRefusal::SignedOut)
        );
        assert_eq!(
            peer_standing(&automatic, Some("uid-2")).unwrap_err().code(),
            "peer_account_changed"
        );
        let ceremony = peer(Some("uid-1"), PeerProvenance::Verified, true);
        assert_eq!(peer_standing(&ceremony, Some("uid-1")), Ok(()));
    }

    /// The legacy records: refused whatever the sign-in state, by name,
    /// with the action that restores access.
    #[test]
    fn a_record_without_account_evidence_is_refused_by_name_with_its_repair() {
        for (record, signed_out_pairing) in [
            (peer(None, PeerProvenance::Verified, false), true),
            (peer(Some("uid-1"), PeerProvenance::Verified, false), false),
        ] {
            for current in [None, Some("uid-1"), Some("uid-2")] {
                let refusal = peer_standing(&record, current).unwrap_err();
                assert_eq!(
                    refusal,
                    AccountBoundaryRefusal::PeerWithoutAccountEvidence {
                        desktop_id: "desktop-b".into(),
                        display_name: "B Mac".into(),
                        paired_while_signed_out: signed_out_pairing,
                    }
                );
                let text = refusal.to_string();
                assert!(
                    text.starts_with("peer_account_evidence_missing: "),
                    "{text}"
                );
                assert!(text.contains("\"B Mac\" (desktop-b)"), "{text}");
                assert!(text.contains("unpair \"B Mac\""), "{text}");
                assert!(text.contains("pairing string"), "{text}");
                assert_eq!(refusal.status(), StatusCode::FORBIDDEN);
            }
        }
        let (status, diagnostic) =
            peer_standing_report(&peer(None, PeerProvenance::Verified, false), Some("uid-1"));
        assert_eq!(status, "peer_account_evidence_missing");
        assert!(diagnostic
            .unwrap()
            .contains("while this desktop was signed out"));
        assert_eq!(
            peer_standing_report(
                &peer(Some("uid-1"), PeerProvenance::Account, false),
                Some("uid-1")
            ),
            ("sameAccount", None)
        );
    }

    #[test]
    fn every_refusal_is_recognised_on_the_wire_and_nothing_else_is() {
        let peer = peer(None, PeerProvenance::Verified, false);
        for refusal in [
            AccountBoundaryRefusal::SignedOut,
            peer_standing(&peer, Some("uid-1")).unwrap_err(),
            AccountBoundaryRefusal::PeerAccountChanged {
                desktop_id: "d".into(),
                display_name: "D".into(),
            },
            AccountBoundaryRefusal::RelayAccountUnattested,
            AccountBoundaryRefusal::RelayAccountChanged,
            AccountBoundaryRefusal::LanCredentialAccountChanged {
                desktop_id: "d".into(),
            },
        ] {
            assert!(REFUSAL_CODES.contains(&refusal.code()));
            assert!(is_refusal_text(&refusal.to_string()), "{refusal}");
        }
        assert!(!is_refusal_text("task not found: t-1"));
        assert!(!is_refusal_text("account_signed_outward: no"));
    }

    #[test]
    fn a_relay_invoke_acts_as_its_connections_account_only_while_it_is_current() {
        assert_eq!(
            relay_invoke_account(Some("uid-1"), Some("uid-1")).as_deref(),
            Ok("uid-1")
        );
        assert_eq!(
            relay_invoke_account(None, Some("uid-1")),
            Err(AccountBoundaryRefusal::RelayAccountUnattested)
        );
        assert_eq!(
            relay_invoke_account(Some("uid-1"), None),
            Err(AccountBoundaryRefusal::SignedOut)
        );
        assert_eq!(
            relay_invoke_account(Some("uid-1"), Some("uid-2")),
            Err(AccountBoundaryRefusal::RelayAccountChanged)
        );
    }

    #[test]
    fn a_lan_invoke_acts_as_the_account_its_secret_was_verified_under() {
        assert_eq!(
            lan_invoke_account("desk-2", Some("uid-1"), Some("uid-1")).as_deref(),
            Ok("uid-1")
        );
        assert_eq!(
            lan_invoke_account("desk-2", Some("uid-1"), None),
            Err(AccountBoundaryRefusal::SignedOut)
        );
        assert_eq!(
            lan_invoke_account("desk-2", None, Some("uid-1")),
            Err(AccountBoundaryRefusal::SignedOut)
        );
        assert_eq!(
            lan_invoke_account("desk-2", Some("uid-1"), Some("uid-2")),
            Err(AccountBoundaryRefusal::LanCredentialAccountChanged {
                desktop_id: "desk-2".into()
            })
        );
    }
}
