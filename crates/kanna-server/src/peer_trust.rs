//! The sibling desktops a person paired with this one, and what was pinned.
//!
//! Distinct from every other trust store in this crate on purpose:
//! `pairing::PairingStore` holds phones, `machine_trust::MachineTrustStore`
//! holds the *automatic*, relay-bootstrapped, account-derived LAN bearer
//! secrets that the legacy desktop-to-desktop path uses. A record here is
//! neither automatic nor account-derived: it exists because a person pasted
//! one desktop's pairing string into the other, and it pins two public keys
//! that no relay, Firestore document or Bonjour record can substitute -
//! the sibling's peer *channel* key (what every `kanna-ksc-peer` handshake
//! authenticates) and its task-transfer key (what the sidecar seals
//! transfer payloads to). Public values only, but written owner-only and
//! atomically through `secure_file` all the same: a store another account
//! could rewrite is a store that could pin an impostor.
//!
//! Records are bound to the environment they were minted in (a staging
//! desktop must not become a peer of a development one just because both
//! sit on one LAN) and to the account this desktop was signed into at
//! pairing time, exactly like `machine_trust`: an explicit sign-out or an
//! account change purges the records of the old account
//! (`retain_account`), so a machine handed to another account does not keep
//! sibling authority the first account established. A pairing made while
//! signed out carries no account and survives sign-in - it is LAN trust the
//! person established by hand, and the relay never routes it anyway.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub const PEER_TRUST_STORE_VERSION: u8 = 1;

/// Guards every load-modify-save cycle against this store's own file - the
/// same process-wide static `machine_trust` uses, for the same reason.
pub(crate) fn persistence_mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PeerDesktop {
    pub desktop_id: String,
    pub display_name: String,
    /// Unpadded base64url X25519 key the sibling authenticates its
    /// `kanna-ksc-peer` handshakes with. On the side that pasted the
    /// pairing string this is the key from the string; on the side that
    /// issued it, the key the sealed claim's handshake authenticated.
    pub channel_public_key: String,
    /// The sibling's task-transfer sidecar identity, exchanged inside the
    /// sealed pairing claim (never read from Firestore or mDNS). `None`
    /// until the sibling's sidecar has reported one; fetched over the
    /// sealed session later in that case, and pinned on first sight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_peer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_public_key: Option<String>,
    pub environment: String,
    /// The account this desktop was signed into when it pinned the peer,
    /// or `None` for a pairing made while signed out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_uid: Option<String>,
    pub paired_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_unix_ms: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerTrustStore {
    #[serde(default)]
    pub version: u8,
    #[serde(default)]
    pub peers: Vec<PeerDesktop>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferIdentityPin {
    /// Nothing was pinned before; the given identity is now pinned.
    Pinned,
    /// The pinned identity matches.
    Unchanged,
    /// A different identity is already pinned: refused, nothing changed.
    Conflict,
}

impl PeerTrustStore {
    /// Loads the store. A missing file is an empty store; a file that is
    /// not a regular owner-only file, or that does not parse, is an error -
    /// the store is never silently reset, because a reset is exactly what
    /// an attacker who cannot read it would want.
    pub fn load(path: &Path) -> Result<Self, String> {
        use kanna_runtime_defaults::secure_file::{load_owner_only, OwnerOnlyReadError};

        let content = match load_owner_only(path) {
            Ok(content) => content,
            Err(OwnerOnlyReadError::NotFound) => return Ok(Self::default()),
            Err(error) => {
                return Err(format!(
                    "peer trust store {} is unusable: {error}",
                    path.display()
                ))
            }
        };
        let store: Self = serde_json::from_str(&content).map_err(|error| {
            format!(
                "failed to parse peer trust store {}: {error}",
                path.display()
            )
        })?;
        if store.version != PEER_TRUST_STORE_VERSION {
            return Err(format!(
                "peer trust store {} has unsupported version {}",
                path.display(),
                store.version
            ));
        }
        Ok(store)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut persisted = Self {
            version: PEER_TRUST_STORE_VERSION,
            peers: self.peers.clone(),
        };
        persisted
            .peers
            .sort_by(|left, right| left.desktop_id.cmp(&right.desktop_id));
        let body = serde_json::to_string_pretty(&persisted)
            .map_err(|error| format!("failed to serialize peer trust store: {error}"))?;
        crate::secure_file::atomic_write_0600(path, &body)
    }

    /// The peer whose channel key is `channel_public_key`, in `environment`.
    pub fn peer_by_channel_key(
        &self,
        channel_public_key: &str,
        environment: &str,
    ) -> Option<&PeerDesktop> {
        self.peers.iter().find(|peer| {
            peer.channel_public_key == channel_public_key && peer.environment == environment
        })
    }

    pub fn peer_by_desktop_id(&self, desktop_id: &str, environment: &str) -> Option<&PeerDesktop> {
        self.peers
            .iter()
            .find(|peer| peer.desktop_id == desktop_id && peer.environment == environment)
    }

    /// Records (or replaces) a peer. One record per desktop id: re-pairing a
    /// sibling whose key rotated replaces the old pin, which is the only
    /// way a rotated key ever becomes trusted. A key that another desktop
    /// id already holds is refused - two siblings cannot share an identity.
    pub fn upsert(&mut self, peer: PeerDesktop) -> Result<(), String> {
        if self.peers.iter().any(|existing| {
            existing.channel_public_key == peer.channel_public_key
                && existing.desktop_id != peer.desktop_id
        }) {
            return Err(format!(
                "peer channel key is already pinned for another desktop ({})",
                peer.desktop_id
            ));
        }
        self.peers
            .retain(|existing| existing.desktop_id != peer.desktop_id);
        self.peers.push(peer);
        Ok(())
    }

    pub fn remove(&mut self, desktop_id: &str) -> bool {
        let before = self.peers.len();
        self.peers.retain(|peer| peer.desktop_id != desktop_id);
        self.peers.len() != before
    }

    /// Pins a sibling's transfer identity the first time it is seen over a
    /// sealed session; a later, different identity is a conflict and is
    /// refused (re-pair to accept a rotated sidecar key).
    pub fn pin_transfer_identity(
        &mut self,
        desktop_id: &str,
        transfer_peer_id: &str,
        transfer_public_key: &str,
    ) -> Result<TransferIdentityPin, String> {
        let peer = self
            .peers
            .iter_mut()
            .find(|peer| peer.desktop_id == desktop_id)
            .ok_or_else(|| format!("desktop {desktop_id} is not a paired peer"))?;
        match (&peer.transfer_peer_id, &peer.transfer_public_key) {
            (Some(pinned_id), Some(pinned_key)) => {
                if pinned_id == transfer_peer_id && pinned_key == transfer_public_key {
                    Ok(TransferIdentityPin::Unchanged)
                } else {
                    Ok(TransferIdentityPin::Conflict)
                }
            }
            _ => {
                peer.transfer_peer_id = Some(transfer_peer_id.to_string());
                peer.transfer_public_key = Some(transfer_public_key.to_string());
                Ok(TransferIdentityPin::Pinned)
            }
        }
    }

    pub fn touch(&mut self, desktop_id: &str, now_ms: u64) {
        if let Some(peer) = self
            .peers
            .iter_mut()
            .find(|peer| peer.desktop_id == desktop_id)
        {
            peer.last_seen_unix_ms = Some(now_ms);
        }
    }

    /// Keeps only the records the current account may use: records made
    /// under `current_account_uid`, plus records made while signed out.
    /// Returns the desktop ids that were dropped so their live sessions
    /// can be closed.
    pub fn retain_account(&mut self, current_account_uid: Option<&str>) -> Vec<String> {
        let mut dropped = Vec::new();
        self.peers.retain(|peer| {
            let keep = match (&peer.account_uid, current_account_uid) {
                (None, _) => true,
                (Some(recorded), Some(current)) => recorded == current,
                (Some(_), None) => false,
            };
            if !keep {
                dropped.push(peer.desktop_id.clone());
            }
            keep
        });
        dropped
    }
}

pub(crate) fn unix_time_ms() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .map_err(|error| format!("system clock error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn peer(desktop_id: &str, key: &str, account: Option<&str>) -> PeerDesktop {
        PeerDesktop {
            desktop_id: desktop_id.into(),
            display_name: format!("{desktop_id} Mac"),
            channel_public_key: key.into(),
            transfer_peer_id: None,
            transfer_public_key: None,
            environment: "development".into(),
            account_uid: account.map(str::to_string),
            paired_at_unix_ms: 1,
            last_seen_unix_ms: None,
        }
    }

    fn store_path() -> std::path::PathBuf {
        crate::test_paths::unique_test_path("peer-trust").join("peer-desktops.json")
    }

    #[test]
    fn round_trips_owner_only_and_refuses_a_loosened_file() {
        let path = store_path();
        let mut store = PeerTrustStore::default();
        store
            .upsert(peer("desktop-b", "key-b", Some("uid-1")))
            .unwrap();
        store.save(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let loaded = PeerTrustStore::load(&path).unwrap();
        assert_eq!(loaded.peers.len(), 1);
        assert!(loaded.peer_by_channel_key("key-b", "development").is_some());
        assert!(loaded.peer_by_channel_key("key-b", "staging").is_none());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let error = PeerTrustStore::load(&path).unwrap_err();
        assert!(error.contains("group or other"), "{error}");
    }

    #[test]
    fn a_missing_store_is_empty_and_a_corrupt_one_is_an_error() {
        let path = store_path();
        assert!(PeerTrustStore::load(&path).unwrap().peers.is_empty());
        crate::secure_file::atomic_write_0600(&path, "{ nope").unwrap();
        assert!(PeerTrustStore::load(&path).is_err());
    }

    #[test]
    fn one_record_per_desktop_and_no_shared_keys() {
        let mut store = PeerTrustStore::default();
        store.upsert(peer("desktop-b", "key-1", None)).unwrap();
        store.upsert(peer("desktop-b", "key-2", None)).unwrap();
        assert_eq!(store.peers.len(), 1);
        assert_eq!(store.peers[0].channel_public_key, "key-2");
        let error = store.upsert(peer("desktop-c", "key-2", None)).unwrap_err();
        assert!(error.contains("already pinned"), "{error}");
        assert!(store.remove("desktop-b"));
        assert!(!store.remove("desktop-b"));
    }

    #[test]
    fn transfer_identity_pins_once_and_refuses_a_change() {
        let mut store = PeerTrustStore::default();
        store.upsert(peer("desktop-b", "key-b", None)).unwrap();
        assert_eq!(
            store
                .pin_transfer_identity("desktop-b", "peer-b", "tkey-b")
                .unwrap(),
            TransferIdentityPin::Pinned
        );
        assert_eq!(
            store
                .pin_transfer_identity("desktop-b", "peer-b", "tkey-b")
                .unwrap(),
            TransferIdentityPin::Unchanged
        );
        assert_eq!(
            store
                .pin_transfer_identity("desktop-b", "peer-b", "tkey-rotated")
                .unwrap(),
            TransferIdentityPin::Conflict
        );
        assert_eq!(
            store.peers[0].transfer_public_key.as_deref(),
            Some("tkey-b"),
            "a conflict changes nothing"
        );
        assert!(store.pin_transfer_identity("desktop-x", "p", "k").is_err());
    }

    #[test]
    fn account_reconciliation_keeps_current_and_signed_out_pairings() {
        let mut store = PeerTrustStore::default();
        store
            .upsert(peer("desktop-a", "ka", Some("uid-1")))
            .unwrap();
        store
            .upsert(peer("desktop-b", "kb", Some("uid-2")))
            .unwrap();
        store.upsert(peer("desktop-c", "kc", None)).unwrap();
        assert_eq!(store.retain_account(Some("uid-1")), vec!["desktop-b"]);
        assert_eq!(store.peers.len(), 2);
        assert_eq!(store.retain_account(None), vec!["desktop-a"]);
        assert_eq!(store.peers.len(), 1);
        assert_eq!(store.peers[0].desktop_id, "desktop-c");
    }
}
