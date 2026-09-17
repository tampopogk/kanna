//! Server-side state for the Kanna secure channel: the desktop's channel
//! identity, the legacy-access switch, the pending typed-code pairing
//! confirmation, and the request-dispatch context a sealed pairing-only
//! session carries.
//!
//! Authority model, in one place:
//! - A sealed session whose initiator static key matches a `TrustedDevice`
//!   dispatches with that device's LAN-paired authority (the
//!   `TrustedLanDeviceAccess` marker), over LAN and relay alike.
//! - A sealed session whose static key is unknown gets *pairing-only*
//!   authority: it may claim a pairing code and poll for its confirmation,
//!   and nothing else. The KSP layer refuses every other frame and the
//!   dispatch layer inserts no task authority, so even a routing mistake
//!   cannot hand an unpaired phone a task.
//! - Legacy access (plaintext claims, bearer-secret LAN requests, relay
//!   account-only invokes and plaintext relay tunnels) stays available while
//!   `mobile_legacy_access` is not `refused`. While it is allowed, the
//!   relay-level authority those paths confer still exists; this is an
//!   explicit migration limitation, not a protected state.

use crate::pairing::{self, ClaimedPairingSession, PairingClaimError, PairingClaimResponse};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify};

/// Settings-table key. `refused` turns every legacy mobile path off; any
/// other value (including absence) keeps them on. Default-on is the
/// deliberate shipping order: the desktop updates first, phones follow,
/// the owner flips this once paired phones report the new runtime.
pub(crate) const MOBILE_LEGACY_ACCESS_SETTING: &str = "mobile_legacy_access";
pub(crate) const MOBILE_LEGACY_ACCESS_REFUSED: &str = "refused";

/// Settings-table key for the desktop-to-desktop equivalent. `refused`
/// turns off every legacy sibling path - the relay-attested `invoke`, the
/// bearer-secret LAN machine-invoke listener and its relay-attested CA
/// bootstrap, the Firestore transfer key and the sidecar's mDNS pairing -
/// leaving only sealed peer sessions to pinned, human-paired siblings.
/// Kept separate from the mobile switch because phones and desktops upgrade
/// on different schedules.
pub(crate) const DESKTOP_PEER_LEGACY_ACCESS_SETTING: &str = "desktop_peer_legacy_access";
pub(crate) const DESKTOP_PEER_LEGACY_ACCESS_REFUSED: &str = "refused";

pub(crate) const PAIRING_CONFIRMATION_TTL: Duration = Duration::from_secs(180);

/// How long a decided confirmation stays collectable by the phone that
/// claimed, measured from the decision, so a person confirming near the
/// end of the window does not strand a phone whose poll lands a moment
/// later.
const PAIRING_DECISION_COLLECTION_GRACE: Duration = Duration::from_secs(60);

/// Hex form of a handshake hash, the token the desktop UI echoes back so a
/// confirmation names exactly the claim it rendered.
pub(crate) fn encode_handshake_hash(hash: &[u8; 32]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn decode_handshake_hash(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 {
        return None;
    }
    let mut hash = [0u8; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(hash)
}

/// Where a sealed session arrived. Relay-origin sessions are additionally
/// subject to the account's cloud entitlement, which the relay can no
/// longer check per request because it cannot read the requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamOrigin {
    Lan,
    RelayTunnel,
}

/// Which population a sealed endpoint admits: phones (the mobile domain and
/// identity) or sibling desktops (the peer domain and identity). A relay
/// KSP tunnel admits phones only; the peer endpoints admit siblings only.
/// Deciding this per endpoint, rather than trying both identities on every
/// first frame, keeps a phone's handshake and a sibling's handshake from
/// ever being interpreted against the wrong trust store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SealedPopulation {
    Mobile,
    Peer,
}

/// Extension inserted on requests dispatched from a sealed *peer* session
/// whose static key is not (yet) a paired sibling. Only the peer pairing
/// claim handler reads it. A distinct type from `SealedPairingContext` so
/// the phone claim route can never be reached by a desktop and vice versa,
/// whatever the route allowlists say.
#[derive(Debug, Clone)]
pub(crate) struct SealedPeerPairingContext {
    pub(crate) remote_static: [u8; 32],
    pub(crate) handshake_hash: [u8; 32],
    pub(crate) origin: StreamOrigin,
    /// The desktop id the initiator declared in its hello - a claim the
    /// pairing handler checks against the pairing string's audience, not
    /// an authentication.
    pub(crate) declared_desktop_id: Option<String>,
}

impl SealedPeerPairingContext {
    pub(crate) fn encoded_remote_static(&self) -> String {
        kanna_secure_channel::encode_key(&self.remote_static)
    }
}

/// Extension inserted on requests dispatched from a sealed *pairing-only*
/// session. Only the pairing claim and confirmation handlers read it; every
/// other route sees a tunneled request with no authority marker.
#[derive(Debug, Clone)]
pub(crate) struct SealedPairingContext {
    pub(crate) remote_static: [u8; 32],
    pub(crate) handshake_hash: [u8; 32],
    pub(crate) origin: StreamOrigin,
}

impl SealedPairingContext {
    pub(crate) fn encoded_remote_static(&self) -> String {
        kanna_secure_channel::encode_key(&self.remote_static)
    }
}

/// A typed-code pairing that verified its code and now waits for the person
/// at the desktop to confirm the short authentication string.
#[derive(Debug, Clone)]
pub(crate) struct PendingPairingConfirmation {
    pub(crate) device_id: String,
    pub(crate) device_name: String,
    /// The phone's static key as the sealed session authenticated it. This
    /// is what confirmation persists - never a key the request body named.
    pub(crate) channel_public_key: String,
    /// The desktop's SAS for *this* handshake; the phone shows its own.
    pub(crate) sas: String,
    /// Binds the confirmation to the exact session that claimed: a phone
    /// that reconnected (new handshake) cannot collect a confirmation the
    /// person gave to the old one, and the old session's disappearance
    /// invalidates the pending entry.
    pub(crate) handshake_hash: [u8; 32],
    pub(crate) session: ClaimedPairingSession,
    pub(crate) expires_at: Instant,
    pub(crate) expires_at_unix_ms: u64,
    pub(crate) decision: Option<PairingConfirmationDecision>,
}

#[derive(Debug, Clone)]
pub(crate) enum PairingConfirmationDecision {
    Confirmed(Box<PairingClaimResponse>),
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PairingConfirmationOutcome {
    Pending,
    Confirmed(Box<PairingClaimResponse>),
    Rejected,
    /// No confirmation is pending for this session (expired, abandoned or
    /// never started).
    Gone,
}

/// At most one *undecided* entry exists at a time (one ceremony at a time,
/// like the pairing session itself); decided entries stay until the phone
/// that claimed collects them or they expire, so a new ceremony cannot
/// erase a confirmation the person already gave.
#[derive(Default)]
pub(crate) struct PairingConfirmationState {
    entries: Mutex<Vec<PendingPairingConfirmation>>,
    changed: Notify,
}

impl PairingConfirmationState {
    /// Starts a confirmation, replacing the undecided one if any. A decided
    /// entry is already persisted and is kept for its phone to collect.
    pub(crate) async fn begin(&self, confirmation: PendingPairingConfirmation) {
        let mut entries = self.entries.lock().await;
        expire_in_place(&mut entries);
        entries.retain(|entry| entry.decision.is_some());
        entries.push(confirmation);
        drop(entries);
        self.changed.notify_waiters();
    }

    /// The entry the desktop UI shows, if one is live and undecided.
    pub(crate) async fn current(&self) -> Option<PendingPairingConfirmation> {
        let mut entries = self.entries.lock().await;
        expire_in_place(&mut entries);
        entries
            .iter()
            .find(|entry| entry.decision.is_none())
            .cloned()
    }

    /// The person confirmed the SAS they were shown: persist the device
    /// with the key the handshake authenticated and hand the response to
    /// the waiting phone. `handshake_hash` and `device_id` are what the UI
    /// rendered; a click on a render the pending claim has since replaced
    /// is `Stale`, never a confirmation of the newcomer.
    pub(crate) async fn confirm(
        &self,
        config: &crate::config::Config,
        persistence_mutation: &Mutex<()>,
        handshake_hash: &[u8; 32],
        device_id: &str,
    ) -> Result<PairingClaimResponse, PairingConfirmationError> {
        let mut entries = self.entries.lock().await;
        expire_in_place(&mut entries);
        let entry = undecided_entry_mut(&mut entries, handshake_hash, device_id)?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| PairingConfirmationError::Persistence(error.to_string()))?
            .as_millis() as u64;
        let _mutation = persistence_mutation.lock().await;
        let response = pairing::register_trusted_device_at(
            config,
            &entry.session,
            &entry.device_id,
            &entry.device_name,
            Some(&entry.channel_public_key),
            now_ms,
        )
        .map_err(|error| match error {
            PairingClaimError::Persistence(message) => {
                PairingConfirmationError::Persistence(message)
            }
            other => PairingConfirmationError::Persistence(other.to_string()),
        })?;
        entry.decision = Some(PairingConfirmationDecision::Confirmed(Box::new(
            response.clone(),
        )));
        entry.expires_at = entry
            .expires_at
            .max(Instant::now() + PAIRING_DECISION_COLLECTION_GRACE);
        drop(entries);
        self.changed.notify_waiters();
        Ok(response)
    }

    /// The person said the strings differ. Bound to the rendered claim
    /// exactly like `confirm`.
    pub(crate) async fn reject(
        &self,
        handshake_hash: &[u8; 32],
        device_id: &str,
    ) -> Result<(), PairingConfirmationError> {
        let mut entries = self.entries.lock().await;
        expire_in_place(&mut entries);
        let entry = undecided_entry_mut(&mut entries, handshake_hash, device_id)?;
        entry.decision = Some(PairingConfirmationDecision::Rejected);
        entry.expires_at = entry
            .expires_at
            .max(Instant::now() + PAIRING_DECISION_COLLECTION_GRACE);
        drop(entries);
        self.changed.notify_waiters();
        Ok(())
    }

    /// The sealed session that claimed has gone away: whatever it was
    /// waiting for is void. A confirmation the person already gave was
    /// persisted at confirm time and stays; only an undecided entry is
    /// dropped, so a phone that reconnects starts a fresh ceremony.
    pub(crate) async fn abandon(&self, handshake_hash: &[u8; 32]) {
        let mut entries = self.entries.lock().await;
        let before = entries.len();
        entries
            .retain(|entry| !(&entry.handshake_hash == handshake_hash && entry.decision.is_none()));
        if entries.len() != before {
            drop(entries);
            self.changed.notify_waiters();
        }
    }

    /// Long-poll for the decision on the confirmation bound to
    /// `handshake_hash`, up to `timeout`. A confirmed response is handed
    /// out once and the entry cleared.
    pub(crate) async fn wait_decision(
        &self,
        handshake_hash: &[u8; 32],
        timeout: Duration,
    ) -> PairingConfirmationOutcome {
        let deadline = Instant::now() + timeout;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            // Register interest before inspecting state so a decision
            // landing between the two is not missed.
            notified.as_mut().enable();
            {
                let mut entries = self.entries.lock().await;
                expire_in_place(&mut entries);
                let Some(index) = entries
                    .iter()
                    .position(|entry| &entry.handshake_hash == handshake_hash)
                else {
                    return PairingConfirmationOutcome::Gone;
                };
                match &entries[index].decision {
                    None => {}
                    Some(PairingConfirmationDecision::Confirmed(response)) => {
                        let response = response.clone();
                        entries.remove(index);
                        return PairingConfirmationOutcome::Confirmed(response);
                    }
                    Some(PairingConfirmationDecision::Rejected) => {
                        entries.remove(index);
                        return PairingConfirmationOutcome::Rejected;
                    }
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return PairingConfirmationOutcome::Pending;
            }
            if tokio::time::timeout(remaining, notified).await.is_err() {
                return PairingConfirmationOutcome::Pending;
            }
        }
    }
}

fn expire_in_place(entries: &mut Vec<PendingPairingConfirmation>) {
    let now = Instant::now();
    entries.retain(|entry| now < entry.expires_at);
}

/// The one undecided entry, provided it is the claim the caller rendered.
fn undecided_entry_mut<'a>(
    entries: &'a mut [PendingPairingConfirmation],
    handshake_hash: &[u8; 32],
    device_id: &str,
) -> Result<&'a mut PendingPairingConfirmation, PairingConfirmationError> {
    let Some(entry) = entries.iter_mut().find(|entry| entry.decision.is_none()) else {
        return Err(PairingConfirmationError::NothingPending);
    };
    if &entry.handshake_hash != handshake_hash || entry.device_id != device_id {
        return Err(PairingConfirmationError::Stale);
    }
    Ok(entry)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PairingConfirmationError {
    NothingPending,
    /// The claim pending now is not the one the caller rendered: a later
    /// claim replaced it between the render and the click.
    Stale,
    Persistence(String),
}

impl std::fmt::Display for PairingConfirmationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingPending => formatter.write_str("no pairing confirmation is pending"),
            Self::Stale => formatter.write_str(
                "the pairing request changed since it was shown; check the code again before confirming",
            ),
            Self::Persistence(message) => formatter.write_str(message),
        }
    }
}

/// Reads the legacy switch from the already-open settings connection. A read
/// that fails answers "refused": the failure mode of a broken settings read
/// must not be plaintext authority.
pub(crate) fn legacy_mobile_access_allowed(db: &crate::db::Db) -> bool {
    match db.get_setting(MOBILE_LEGACY_ACCESS_SETTING) {
        Ok(Some(value)) => value.trim() != MOBILE_LEGACY_ACCESS_REFUSED,
        Ok(None) => true,
        Err(error) => {
            log::warn!("failed to read {MOBILE_LEGACY_ACCESS_SETTING}: {error}; refusing legacy mobile access");
            false
        }
    }
}

/// Reads the desktop-peer legacy switch with the same fail-closed stance as
/// `legacy_mobile_access_allowed`.
pub(crate) fn legacy_peer_access_allowed(db: &crate::db::Db) -> bool {
    match db.get_setting(DESKTOP_PEER_LEGACY_ACCESS_SETTING) {
        Ok(Some(value)) => value.trim() != DESKTOP_PEER_LEGACY_ACCESS_REFUSED,
        Ok(None) => true,
        Err(error) => {
            log::warn!("failed to read {DESKTOP_PEER_LEGACY_ACCESS_SETTING}: {error}; refusing legacy desktop-to-desktop access");
            false
        }
    }
}

/// Shared holder so the identity is loaded once per process and its failure
/// is reported (not hidden) everywhere it is needed.
pub(crate) type SecureChannelIdentity =
    Arc<std::sync::OnceLock<Result<Arc<kanna_secure_channel::Keypair>, String>>>;
