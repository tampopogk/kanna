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

pub(crate) const PAIRING_CONFIRMATION_TTL: Duration = Duration::from_secs(180);

/// Where a sealed session arrived. Relay-origin sessions are additionally
/// subject to the account's cloud entitlement, which the relay can no
/// longer check per request because it cannot read the requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamOrigin {
    Lan,
    RelayTunnel,
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

#[derive(Default)]
pub(crate) struct PairingConfirmationState {
    pending: Mutex<Option<PendingPairingConfirmation>>,
    changed: Notify,
}

impl PairingConfirmationState {
    /// Starts a confirmation, replacing any earlier one (only one pairing
    /// ceremony runs at a time, like the pairing session itself).
    pub(crate) async fn begin(&self, confirmation: PendingPairingConfirmation) {
        let mut pending = self.pending.lock().await;
        *pending = Some(confirmation);
        drop(pending);
        self.changed.notify_waiters();
    }

    /// The entry the desktop UI shows, if one is live and undecided.
    pub(crate) async fn current(&self) -> Option<PendingPairingConfirmation> {
        let mut pending = self.pending.lock().await;
        expire_in_place(&mut pending);
        pending
            .as_ref()
            .filter(|entry| entry.decision.is_none())
            .cloned()
    }

    /// The person confirmed the SAS: persist the device with the key the
    /// handshake authenticated and hand the response to the waiting phone.
    pub(crate) async fn confirm(
        &self,
        config: &crate::config::Config,
        persistence_mutation: &Mutex<()>,
    ) -> Result<PairingClaimResponse, PairingConfirmationError> {
        let mut pending = self.pending.lock().await;
        expire_in_place(&mut pending);
        let Some(entry) = pending.as_mut().filter(|entry| entry.decision.is_none()) else {
            return Err(PairingConfirmationError::NothingPending);
        };
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
        drop(pending);
        self.changed.notify_waiters();
        Ok(response)
    }

    pub(crate) async fn reject(&self) -> bool {
        let mut pending = self.pending.lock().await;
        expire_in_place(&mut pending);
        let Some(entry) = pending.as_mut().filter(|entry| entry.decision.is_none()) else {
            return false;
        };
        entry.decision = Some(PairingConfirmationDecision::Rejected);
        drop(pending);
        self.changed.notify_waiters();
        true
    }

    /// The sealed session that claimed has gone away: whatever it was
    /// waiting for is void. A confirmation the person already gave was
    /// persisted at confirm time and stays; only an undecided entry is
    /// dropped, so a phone that reconnects starts a fresh ceremony.
    pub(crate) async fn abandon(&self, handshake_hash: &[u8; 32]) {
        let mut pending = self.pending.lock().await;
        if pending.as_ref().is_some_and(|entry| {
            &entry.handshake_hash == handshake_hash && entry.decision.is_none()
        }) {
            *pending = None;
            drop(pending);
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
                let mut pending = self.pending.lock().await;
                expire_in_place(&mut pending);
                match pending.as_ref() {
                    Some(entry) if &entry.handshake_hash == handshake_hash => {
                        match &entry.decision {
                            None => {}
                            Some(PairingConfirmationDecision::Confirmed(response)) => {
                                let response = response.clone();
                                *pending = None;
                                return PairingConfirmationOutcome::Confirmed(response);
                            }
                            Some(PairingConfirmationDecision::Rejected) => {
                                *pending = None;
                                return PairingConfirmationOutcome::Rejected;
                            }
                        }
                    }
                    _ => return PairingConfirmationOutcome::Gone,
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

fn expire_in_place(pending: &mut Option<PendingPairingConfirmation>) {
    if pending
        .as_ref()
        .is_some_and(|entry| entry.decision.is_none() && Instant::now() >= entry.expires_at)
    {
        *pending = None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PairingConfirmationError {
    NothingPending,
    Persistence(String),
}

impl std::fmt::Display for PairingConfirmationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingPending => formatter.write_str("no pairing confirmation is pending"),
            Self::Persistence(message) => formatter.write_str(message),
        }
    }
}

/// Reads the legacy switch. A database that cannot be opened answers
/// "refused": the failure mode of a broken settings read must not be
/// plaintext authority.
pub(crate) fn legacy_mobile_access_allowed(db_path: &str) -> bool {
    match crate::db::Db::open(db_path) {
        Ok(db) => match db.get_setting(MOBILE_LEGACY_ACCESS_SETTING) {
            Ok(Some(value)) => value.trim() != MOBILE_LEGACY_ACCESS_REFUSED,
            Ok(None) => true,
            Err(error) => {
                log::warn!("failed to read {MOBILE_LEGACY_ACCESS_SETTING}: {error}; refusing legacy mobile access");
                false
            }
        },
        Err(error) => {
            log::warn!(
                "failed to open the settings database: {error}; refusing legacy mobile access"
            );
            false
        }
    }
}

/// Shared holder so the identity is loaded once per process and its failure
/// is reported (not hidden) everywhere it is needed.
pub(crate) type SecureChannelIdentity =
    Arc<std::sync::OnceLock<Result<Arc<kanna_secure_channel::Keypair>, String>>>;
