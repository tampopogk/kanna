//! This desktop's stable *secure-channel identity*: the X25519 static key a
//! paired phone pins (from the pairing QR, or from a typed-code pairing it
//! confirmed with a short authentication string) and that every Kanna
//! secure-channel handshake authenticates the desktop with.
//!
//! Distinct from every other identity in this crate on purpose: the Ed25519
//! anonymous-push identity signs pairing certificates the relay verifies, the
//! LAN TLS identity authenticates desktop-to-desktop machine invokes, and the
//! task-transfer identity seals transfer payloads. A key that authenticates a
//! phone's sessions is never reused for any of those, so compromising or
//! rotating one does not silently touch another.
//!
//! Generated once from the operating-system CSPRNG and persisted `0600`
//! through `secure_file`; it must not regenerate on an ordinary restart,
//! because every paired phone would then see "desktop identity changed" and
//! need to re-pair. It regenerates only when the file is missing. A file
//! that exists but fails to parse, is a symlink, or is readable by another
//! account fails closed (no sealed sessions, no `channelPublicKey` on
//! status) rather than being silently replaced - the same stance
//! `lan_tls_identity` takes.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use kanna_secure_channel::Keypair;
use serde::{Deserialize, Serialize};
use std::path::Path;

const IDENTITY_VERSION: u8 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedIdentity {
    version: u8,
    /// Unpadded base64url of the 32-byte private scalar.
    private_key: String,
    /// Unpadded base64url of the 32-byte public key; recomputed on load and
    /// compared, so a hand-edited file cannot advertise a key it does not
    /// hold.
    public_key: String,
}

pub fn load_or_create(path: &Path) -> Result<Keypair, String> {
    match try_load(path)? {
        Some(identity) => Ok(identity),
        None => {
            let identity = Keypair::generate()
                .map_err(|error| format!("failed to generate secure-channel identity: {error}"))?;
            save(path, &identity)?;
            Ok(identity)
        }
    }
}

fn try_load(path: &Path) -> Result<Option<Keypair>, String> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to stat secure-channel identity {}: {error}",
                path.display()
            ))
        }
    };
    if !metadata.file_type().is_file() {
        return Err(format!(
            "secure-channel identity {} is not a regular file",
            path.display()
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "secure-channel identity {} must not grant group or other permissions",
            path.display()
        ));
    }
    let content = std::fs::read_to_string(path).map_err(|error| {
        format!(
            "failed to read secure-channel identity {}: {error}",
            path.display()
        )
    })?;
    let persisted: PersistedIdentity = serde_json::from_str(&content).map_err(|error| {
        format!(
            "failed to parse secure-channel identity {}: {error}",
            path.display()
        )
    })?;
    if persisted.version != IDENTITY_VERSION {
        return Err(format!(
            "secure-channel identity {} has unsupported version {}",
            path.display(),
            persisted.version
        ));
    }
    let private: [u8; 32] = URL_SAFE_NO_PAD
        .decode(persisted.private_key.trim())
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| {
            format!(
                "secure-channel identity {} private key is not 32 base64url bytes",
                path.display()
            )
        })?;
    let identity = Keypair::from_private(private)
        .map_err(|error| format!("secure-channel identity {}: {error}", path.display()))?;
    if identity.encoded_public_key() != persisted.public_key.trim() {
        return Err(format!(
            "secure-channel identity {} public key does not match its private key",
            path.display()
        ));
    }
    Ok(Some(identity))
}

fn save(path: &Path, identity: &Keypair) -> Result<(), String> {
    let persisted = PersistedIdentity {
        version: IDENTITY_VERSION,
        private_key: URL_SAFE_NO_PAD.encode(identity.private_key()),
        public_key: identity.encoded_public_key(),
    };
    let body = serde_json::to_string_pretty(&persisted)
        .map_err(|error| format!("failed to serialize secure-channel identity: {error}"))?;
    crate::secure_file::atomic_write_0600(path, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_path() -> std::path::PathBuf {
        crate::test_paths::unique_test_path("secure-channel-identity").join("identity.json")
    }

    #[test]
    fn generates_once_and_reloads_the_same_identity() {
        let path = temp_path();
        let first = load_or_create(&path).expect("create");
        let second = load_or_create(&path).expect("reload");
        assert_eq!(first.public_key(), second.public_key());
        assert_eq!(first.private_key(), second.private_key());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_loose_permission_file_fails_closed() {
        let path = temp_path();
        load_or_create(&path).expect("create");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let error = load_or_create(&path).expect_err("must refuse");
        assert!(error.contains("group or other permissions"), "{error}");
    }

    #[test]
    fn a_corrupt_file_fails_closed_instead_of_regenerating() {
        let path = temp_path();
        load_or_create(&path).expect("create");
        std::fs::write(&path, "{ not json").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(load_or_create(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn a_public_key_that_does_not_match_its_private_key_is_refused() {
        let path = temp_path();
        let identity = load_or_create(&path).expect("create");
        let other = Keypair::generate().unwrap();
        let forged = PersistedIdentity {
            version: IDENTITY_VERSION,
            private_key: URL_SAFE_NO_PAD.encode(identity.private_key()),
            public_key: other.encoded_public_key(),
        };
        crate::secure_file::atomic_write_0600(&path, &serde_json::to_string(&forged).unwrap())
            .unwrap();
        let error = load_or_create(&path).expect_err("must refuse");
        assert!(error.contains("does not match"), "{error}");
    }
}
