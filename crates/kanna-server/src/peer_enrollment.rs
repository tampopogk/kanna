//! Automatic E2EE peer trust between desktops signed into one account: the
//! ceremony becomes verification rather than the entry fee.
//!
//! The pairing string (`peer_pairing`) pins a key a *person* carried between
//! two screens, so no relay was ever in a position to substitute it. That is
//! the strongest claim available and it stays exactly as it was. But it was
//! also the only way a `PeerDesktop` record could be born, which made a
//! manual ceremony mandatory before two of one person's own Macs could talk
//! at all - and, since the sealed channel is the only route for sibling
//! terminal views, made "not paired" look like a product failure.
//!
//! So this module adds the second way a record is born, the one WhatsApp and
//! Signal use: **server-assisted introduction with trust on first use.** Each
//! desktop announces its peer channel *public* key on its own relay control
//! socket - a socket the relay only binds to a desktop id after that desktop
//! proved its desktop secret - and the relay hands that key out with presence
//! (`AppState::list_active_relay_desktop_presence`). A desktop that wants to
//! reach an unpinned sibling of the same account reads the sibling's key from
//! there, runs the ordinary `kanna-ksc-peer` handshake against exactly that
//! key, and claims enrollment over the resulting sealed session. Both sides
//! end up with a real pin from one exchange, exactly as the ceremony leaves
//! them.
//!
//! What is traded, stated plainly: **at first contact the relay is the
//! introducer, and a compromised relay could lie about both keys and insert
//! itself once.** It cannot do so afterwards, and it never gets a second
//! chance:
//!
//! - The result is a *pin*, not a lookup. [`try_enroll`] refuses outright
//!   while any record for that desktop id exists, and the responder refuses
//!   a claim for a desktop id it already pins under a different key. Nothing
//!   here ever re-trusts a changed key; a rotation is `peer_identity_mismatch`
//!   and a person resolves it (verify with a pairing string, or unpair and
//!   let it re-enroll).
//! - A changed key is loud: `PeerDesktop::identity_mismatch_at_unix_ms` is
//!   recorded and surfaced in Preferences → Machines.
//! - Provenance is visible: `PeerProvenance::Account` never claims to be
//!   `Verified`, and the ceremony upgrades a record that is.
//! - The introducer must be *live*. The relay persists nothing; it can only
//!   list a key for a socket authenticated right now. A Firestore document
//!   was rejected for exactly this reason - a one-time account or rules
//!   compromise could plant a key there that outlives it.
//!
//! What this does **not** change: the sealed handshake, the sealed-only
//! routing rule, the authority a session carries, or the loopback-authority
//! doctrine. Only how a pin is born.

use crate::http_api::AppState;
use crate::peer_channel::{dial_peer_with_key, PeerDialError, PeerHello};
use crate::peer_pairing::{PeerPairingClaimResponse, PeerTransferIdentity};
use crate::peer_trust::{PeerDesktop, PeerProvenance, PeerTrustStore};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The route the enrollment claim is made on. Shaped like the pairing claim
/// and carried inside the same sealed pairing-only session; the difference
/// is what authorizes it - relay presence under one account rather than a
/// one-time secret off a screen.
pub(crate) const ACCOUNT_ENROLL_PATH: &str = "/v1/peers/account-enroll";

/// How long a refused enrollment is remembered, so a burst of invokes to an
/// unreachable or ineligible sibling does not become a burst of dials.
const NEGATIVE_CACHE: Duration = Duration::from_secs(30);

/// The claim the initiator sends inside the sealed session. Everything in it
/// is a claim rather than an authentication: the responder checks the
/// declared id against its own handshake hello, and the *key* against what
/// the relay lists for that id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PeerAccountEnrollClaim {
    pub(crate) desktop_id: String,
    pub(crate) desktop_name: String,
    pub(crate) environment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transfer_identity: Option<PeerTransferIdentity>,
}

/// Why an enrollment attempt did not produce a pin. Each maps to a sentence
/// a person can act on, because the alternative - the single
/// "pair it from Preferences → Machines" line - is what made a signed-in
/// owner think pairing was mandatory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PeerEnrollError {
    /// Already pinned: enrollment is not attempted, ever, while a record
    /// exists. The caller uses the record it has.
    AlreadyPinned,
    /// This desktop is signed out, so there is no account to be introduced
    /// within. The ceremony remains this machine's path.
    NotSignedIn,
    /// The relay is unreachable or this desktop is not routed through it.
    RelayUnavailable(String),
    /// The relay does not report that desktop as connected.
    SiblingOffline,
    /// The relay lists it, but with no announced key: an older Kanna, or one
    /// whose peer channel identity failed to load.
    SiblingAnnouncesNoKey,
    /// The relay itself predates key presence.
    RelayTooOld,
    /// The sibling refused the claim, or already pins a different key for
    /// this desktop. A person resolves this one.
    SiblingRefused(String),
    /// An attempt for this target is already running, or one just failed.
    Throttled,
    /// The handshake or the sealed exchange failed.
    Failed(String),
}

impl std::fmt::Display for PeerEnrollError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyPinned => formatter.write_str("that machine is already paired"),
            Self::NotSignedIn => formatter.write_str(
                "this desktop is signed out, so it cannot pair automatically; sign in, or pair from Preferences → Machines",
            ),
            Self::RelayUnavailable(detail) => {
                write!(formatter, "the Kanna relay is unavailable: {detail}")
            }
            Self::SiblingOffline => {
                formatter.write_str("that machine is not connected to your account right now")
            }
            Self::SiblingAnnouncesNoKey => formatter
                .write_str("that machine runs an older Kanna and cannot pair automatically yet"),
            Self::RelayTooOld => formatter
                .write_str("the relay does not support automatic pairing yet (relay upgrade required)"),
            Self::SiblingRefused(detail) => write!(formatter, "that machine refused: {detail}"),
            Self::Throttled => {
                formatter.write_str("an automatic pairing attempt with that machine just failed")
            }
            Self::Failed(detail) => write!(formatter, "automatic pairing failed: {detail}"),
        }
    }
}

/// Serializes attempts per target and remembers a recent failure, so the
/// eager background enrollment `list_cloud_desktops` starts and the lazy one
/// `dial_peer` starts cannot become a dial storm. Same shape as
/// `AppState::begin_lan_bootstrap_attempt`, kept here because it is this
/// module's own concern.
#[derive(Default)]
pub(crate) struct EnrollmentAttempts {
    inner: std::sync::Mutex<EnrollmentAttemptState>,
}

#[derive(Default)]
struct EnrollmentAttemptState {
    in_flight: std::collections::HashSet<String>,
    recent_failures: std::collections::HashMap<String, Instant>,
}

/// Releases the in-flight slot when the attempt ends, however it ends.
struct AttemptGuard<'a> {
    attempts: &'a EnrollmentAttempts,
    desktop_id: String,
}

impl Drop for AttemptGuard<'_> {
    fn drop(&mut self) {
        let mut state = self
            .attempts
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.in_flight.remove(&self.desktop_id);
    }
}

impl EnrollmentAttempts {
    fn begin(&self, desktop_id: &str) -> Option<AttemptGuard<'_>> {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(failed_at) = state.recent_failures.get(desktop_id) {
            if failed_at.elapsed() < NEGATIVE_CACHE {
                return None;
            }
            state.recent_failures.remove(desktop_id);
        }
        if !state.in_flight.insert(desktop_id.to_string()) {
            return None;
        }
        Some(AttemptGuard {
            attempts: self,
            desktop_id: desktop_id.to_string(),
        })
    }

    fn record_failure(&self, desktop_id: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recent_failures
            .insert(desktop_id.to_string(), Instant::now());
    }

    fn clear_failure(&self, desktop_id: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recent_failures
            .remove(desktop_id);
    }
}

/// Pins `desktop_id` automatically, if both desktops are signed into one
/// account and the relay can introduce them.
///
/// The order of the checks is the security property. Nothing is dialed until
/// this desktop is signed in and the relay lists the target *with a key*, and
/// the key the handshake runs against is that announced key and nothing else:
/// a sibling that answers the dial but does not hold it cannot complete the
/// handshake at all. Together with the responder's own mirror-image check
/// (`http_api::peers::claim_account_enrollment`), one exchange leaves a real
/// pin on both sides.
///
/// Returns a boxed future rather than being an `async fn`, because this is
/// the one genuinely cyclic edge in the sealed peer stack: enrollment
/// synchronizes the transfer proxies, proxy synchronization can fetch a
/// sibling's transfer identity over a sealed session, and that dials - which
/// may enroll. Erasing the future's type here keeps that a runtime path
/// rather than a future that definitionally contains itself.
pub(crate) fn try_enroll<'a>(
    state: &'a Arc<AppState>,
    desktop_id: &'a str,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<PeerDesktop, PeerEnrollError>> + Send + 'a>,
> {
    Box::pin(try_enroll_inner(state, desktop_id))
}

async fn try_enroll_inner(
    state: &Arc<AppState>,
    desktop_id: &str,
) -> Result<PeerDesktop, PeerEnrollError> {
    if desktop_id == state.config().desktop_id {
        return Err(PeerEnrollError::AlreadyPinned);
    }
    if !crate::peer_pairing::desktop_id_is_pairable(desktop_id) {
        return Err(PeerEnrollError::Failed("invalid desktop id".into()));
    }
    // A record - of either provenance - is final. Enrollment is how a pin is
    // born, never how one is replaced.
    match state.paired_peer(desktop_id) {
        Ok(Some(_)) => return Err(PeerEnrollError::AlreadyPinned),
        Ok(None) => {}
        Err(error) => return Err(PeerEnrollError::Failed(error)),
    }
    let Some(account_uid) = state.authenticated_account_uid() else {
        return Err(PeerEnrollError::NotSignedIn);
    };
    if !state.desktop_routing_available() {
        return Err(PeerEnrollError::RelayUnavailable(
            state.desktop_routing_unavailable_reason(),
        ));
    }
    let Some(_guard) = state.peer_enrollment_attempts().begin(desktop_id) else {
        return Err(PeerEnrollError::Throttled);
    };
    let outcome = enroll_once(state, desktop_id, &account_uid).await;
    match &outcome {
        Ok(_) => state.peer_enrollment_attempts().clear_failure(desktop_id),
        // `AlreadyPinned` is a race with the other side's exchange, not a
        // failure worth throttling the next call over.
        Err(PeerEnrollError::AlreadyPinned) => {}
        Err(_) => state.peer_enrollment_attempts().record_failure(desktop_id),
    }
    outcome
}

async fn enroll_once(
    state: &Arc<AppState>,
    desktop_id: &str,
    account_uid: &str,
) -> Result<PeerDesktop, PeerEnrollError> {
    let announced = announced_key_for(state, desktop_id).await?;
    let pinned_key = kanna_secure_channel::decode_key(&announced).map_err(|error| {
        PeerEnrollError::Failed(format!("the relay announced an unusable key: {error}"))
    })?;
    let config = state.config();
    let local_transfer_identity = crate::http_api::peers::local_transfer_identity(state).await;
    // The pairing intent is deliberate: it asks the sibling for pairing-only
    // authority, which is exactly what an enrollment claim needs, and leaves
    // `kanna-secure-channel` untouched. Success here plus the responder
    // hello naming `desktop_id` is the proof that the far side holds the
    // announced key.
    let sealed = dial_peer_with_key(state, desktop_id, pinned_key, PeerHello::Pairing)
        .await
        .map_err(enroll_dial_error)?;
    let (mut writer, mut reader) = sealed.split();
    let claim = PeerAccountEnrollClaim {
        desktop_id: config.desktop_id.clone(),
        desktop_name: config.desktop_name.clone(),
        environment: config.environment.clone(),
        transfer_identity: local_transfer_identity,
    };
    let exchange = async {
        writer
            .send(
                serde_json::json!({ "type": "auth", "capabilities": [] })
                    .to_string()
                    .as_bytes(),
            )
            .await?;
        crate::http_api::peers::expect_sealed_frame(&mut reader, "auth_ok").await?;
        writer
            .send(
                serde_json::json!({
                    "type": "request",
                    "id": 1,
                    "method": "POST",
                    "path": ACCOUNT_ENROLL_PATH,
                    "body": claim,
                })
                .to_string()
                .as_bytes(),
            )
            .await?;
        crate::http_api::peers::expect_sealed_frame(&mut reader, "response").await
    };
    let response = match tokio::time::timeout(Duration::from_secs(20), exchange).await {
        Ok(Ok(response)) => {
            writer.close("enrollment complete").await;
            response
        }
        Ok(Err(error)) => {
            writer.close("enrollment failed").await;
            return Err(PeerEnrollError::Failed(error));
        }
        Err(_) => {
            writer.close("enrollment timed out").await;
            return Err(PeerEnrollError::Failed(
                "the other desktop did not answer the enrollment claim".into(),
            ));
        }
    };
    let status = response
        .get("status")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if status != 200 {
        let reason = response
            .get("body")
            .and_then(|body| body.get("error"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("the other desktop refused the enrollment claim");
        return Err(PeerEnrollError::SiblingRefused(reason.to_string()));
    }
    let body: PeerPairingClaimResponse = response
        .get("body")
        .cloned()
        .ok_or_else(|| PeerEnrollError::Failed("empty enrollment reply".into()))
        .and_then(|body| {
            serde_json::from_value(body)
                .map_err(|error| PeerEnrollError::Failed(format!("malformed reply: {error}")))
        })?;
    if body.desktop_id != desktop_id {
        return Err(PeerEnrollError::Failed(
            "the enrollment reply named a different desktop".into(),
        ));
    }
    if body.environment != config.environment {
        return Err(PeerEnrollError::Failed(
            "the desktops run in different Kanna environments".into(),
        ));
    }
    let now_ms = crate::peer_trust::unix_time_ms().map_err(PeerEnrollError::Failed)?;
    let peer = PeerDesktop {
        desktop_id: desktop_id.to_string(),
        display_name: body.desktop_name.trim().to_string(),
        channel_public_key: announced,
        transfer_peer_id: body
            .transfer_identity
            .as_ref()
            .map(|identity| identity.peer_id.clone()),
        transfer_public_key: body
            .transfer_identity
            .as_ref()
            .map(|identity| identity.public_key.clone()),
        environment: config.environment.clone(),
        // Always account-bound: an automatic pin exists because of the
        // account, so `retain_account` must drop it when that account goes.
        account_uid: Some(account_uid.to_string()),
        provenance: PeerProvenance::Account,
        // The relay listed this id with exactly `announced` under
        // `account_uid`: that listing is the evidence.
        account_verified_at_unix_ms: Some(now_ms),
        identity_mismatch_at_unix_ms: None,
        paired_at_unix_ms: now_ms,
        last_seen_unix_ms: Some(now_ms),
    };
    persist(state, peer.clone())?;
    state.publish_state_changed(kanna_agent_protocol::StateChangeScope::Settings);
    log::info!("[peer] enrolled {desktop_id} automatically (same account, relay-introduced)");
    state.peer_sessions().close(desktop_id).await;
    let proxies = state.peer_transfer_proxies();
    let sync_state = Arc::clone(state);
    tokio::spawn(async move { proxies.sync_from_store(&sync_state).await });
    Ok(peer)
}

/// The key the relay lists for `desktop_id` right now.
async fn announced_key_for(
    state: &Arc<AppState>,
    desktop_id: &str,
) -> Result<String, PeerEnrollError> {
    let presence = state
        .list_active_relay_desktop_presence()
        .await
        .map_err(PeerEnrollError::RelayUnavailable)?;
    // A relay that predates key presence lists every desktop without one,
    // which is indistinguishable per-entry from a sibling running an older
    // Kanna - so the whole-listing shape is what tells them apart.
    let relay_announces_keys = presence
        .iter()
        .any(|entry| entry.peer_channel_public_key.is_some());
    let Some(entry) = presence.iter().find(|entry| entry.desktop_id == desktop_id) else {
        return Err(PeerEnrollError::SiblingOffline);
    };
    match entry.peer_channel_public_key.clone() {
        Some(key) => Ok(key),
        None if relay_announces_keys => Err(PeerEnrollError::SiblingAnnouncesNoKey),
        None => Err(PeerEnrollError::RelayTooOld),
    }
}

/// A dial failure during enrollment. A handshake that fails against the
/// announced key means the far side does not hold it - the relay named a key
/// nobody answers for - which is an introduction failure, never a pin
/// mismatch: there is no pin yet.
fn enroll_dial_error(error: PeerDialError) -> PeerEnrollError {
    match error {
        PeerDialError::Unreachable(detail) => PeerEnrollError::Failed(format!(
            "could not reach that machine to pair automatically: {detail}"
        )),
        other => PeerEnrollError::Failed(other.to_string()),
    }
}

fn persist(state: &Arc<AppState>, peer: PeerDesktop) -> Result<(), PeerEnrollError> {
    let path = state
        .config()
        .peer_trust_store_path()
        .ok_or_else(|| PeerEnrollError::Failed("no pairing store configured".into()))?;
    let _guard = crate::peer_trust::persistence_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut store = PeerTrustStore::load(&path).map_err(PeerEnrollError::Failed)?;
    // The other side may have enrolled us in the meantime; its record and
    // this one are identical, and `upsert` is idempotent.
    store
        .upsert(peer)
        .map_err(PeerEnrollError::SiblingRefused)?;
    store.save(&path).map_err(PeerEnrollError::Failed)
}

/// Records that a handshake against `desktop_id`'s pin met a different key,
/// so Preferences → Machines can say so instead of the session merely
/// failing over and over. Best-effort: a store that cannot be written is
/// already reported by the dial error itself.
pub(crate) fn note_identity_mismatch(state: &Arc<AppState>, desktop_id: &str) {
    let (Some(path), Ok(now_ms)) = (
        state.config().peer_trust_store_path(),
        crate::peer_trust::unix_time_ms(),
    ) else {
        return;
    };
    let changed = {
        let _guard = crate::peer_trust::persistence_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Ok(mut store) = PeerTrustStore::load(&path) else {
            return;
        };
        if !store.record_identity_mismatch(desktop_id, now_ms) {
            return;
        }
        store.save(&path).is_ok()
    };
    if changed {
        log::warn!(
            "[peer] the pinned identity of {desktop_id} no longer matches; \
             verify it with a pairing string or unpair it"
        );
        state.publish_state_changed(kanna_agent_protocol::StateChangeScope::Settings);
    }
}

/// Clears that flag after a handshake against the pin succeeded again.
pub(crate) fn clear_identity_mismatch(state: &Arc<AppState>, desktop_id: &str) {
    let Some(path) = state.config().peer_trust_store_path() else {
        return;
    };
    let changed = {
        let _guard = crate::peer_trust::persistence_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Ok(mut store) = PeerTrustStore::load(&path) else {
            return;
        };
        if !store.clear_identity_mismatch(desktop_id) {
            return;
        }
        store.save(&path).is_ok()
    };
    if changed {
        state.publish_state_changed(kanna_agent_protocol::StateChangeScope::Settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_attempt_per_target_and_a_failure_is_remembered() {
        let attempts = EnrollmentAttempts::default();
        let guard = attempts.begin("desktop-b").expect("first attempt");
        assert!(
            attempts.begin("desktop-b").is_none(),
            "a second concurrent attempt must not start"
        );
        assert!(attempts.begin("desktop-c").is_some());
        drop(guard);
        assert!(
            attempts.begin("desktop-b").is_some(),
            "the slot is released"
        );

        attempts.record_failure("desktop-b");
        assert!(
            attempts.begin("desktop-b").is_none(),
            "a just-failed target is not retried immediately"
        );
        attempts.clear_failure("desktop-b");
        assert!(attempts.begin("desktop-b").is_some());
    }
}
