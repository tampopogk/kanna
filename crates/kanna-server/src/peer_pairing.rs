//! The desktop-to-desktop pairing ceremony: one desktop (the *issuer*)
//! shows a one-time pairing string, a person pastes it into another (the
//! *claimant*), and both end up pinning each other's peer channel key.
//!
//! The string is the trust anchor, exactly as the phone's `KANNA2` QR is:
//!
//! ```text
//! KANNA-PEER:<issuer desktop id>:<code>:<base32 issuer peer key>:<base32 secret>
//! ```
//!
//! The claimant pins the issuer's key *from the string* and opens a sealed
//! `peer_pairing` session against exactly that key; the issuer verifies the
//! code and the secret - which only ever existed on the issuer's screen and
//! inside the sealed claim - and pins the claimant's *handshake* static key.
//! Neither key ever comes from the relay, Firestore or Bonjour, and a
//! compromised relay that swapped the target learns nothing: the claim is
//! encrypted to the key the person copied, and a responder without that
//! key's private half cannot read it. No short authentication string is
//! needed because the key came from the screen (the foundation's QR
//! argument); a typed-code + SAS variant is deliberately not offered - a
//! typed code over the relay pins nothing, and both ends here have a
//! clipboard.
//!
//! The string is a five-minute, five-attempt secret. Stolen before use it
//! lets the thief pair as a peer of the issuer until unpaired - the same
//! class as the phone QR, mitigated the same way (TTL, attempt cap, and the
//! Machines list showing every paired peer).

use crate::pairing::{base32_encode, constant_time_eq, generate_pairing_code};
use serde::{Deserialize, Serialize};
use std::io::Read;

pub(crate) const PEER_PAIRING_TTL_MS: u64 = 5 * 60 * 1000;
pub(crate) const PEER_PAIRING_MAX_ATTEMPTS: u8 = 5;
const PAIRING_STRING_PREFIX: &str = "KANNA-PEER";

/// The offer currently on this desktop's screen.
#[derive(Debug, Clone)]
pub(crate) struct ActivePeerPairingOffer {
    pub(crate) code: String,
    pub(crate) secret: String,
    pub(crate) pairing_string: String,
    pub(crate) expires_at_unix_ms: u64,
    pub(crate) failed_claims: u8,
}

/// What the desktop UI renders for an offer.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PeerPairingOfferView {
    pub(crate) desktop_id: String,
    pub(crate) desktop_name: String,
    pub(crate) code: String,
    pub(crate) pairing_string: String,
    pub(crate) expires_at_unix_ms: u64,
}

/// A parsed pairing string: everything the claimant pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedPairingString {
    pub(crate) desktop_id: String,
    pub(crate) code: String,
    pub(crate) channel_public_key: [u8; 32],
    pub(crate) secret: String,
}

/// The sidecar's transfer identity, exchanged inside the sealed claim.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PeerTransferIdentity {
    pub(crate) peer_id: String,
    pub(crate) public_key: String,
}

/// The sealed claim the claimant sends to the issuer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PeerPairingClaim {
    pub(crate) code: String,
    pub(crate) secret: String,
    pub(crate) desktop_id: String,
    pub(crate) desktop_name: String,
    pub(crate) environment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transfer_identity: Option<PeerTransferIdentity>,
}

/// The issuer's sealed answer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PeerPairingClaimResponse {
    pub(crate) desktop_id: String,
    pub(crate) desktop_name: String,
    pub(crate) environment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transfer_identity: Option<PeerTransferIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PeerClaimError {
    NoActiveOffer,
    Expired,
    InvalidCode,
    RateLimited,
    EnvironmentMismatch,
    SelfPairing,
    InvalidRequest(String),
}

impl std::fmt::Display for PeerClaimError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoActiveOffer => formatter.write_str("no peer pairing offer is active"),
            Self::Expired => formatter.write_str("the peer pairing offer expired"),
            Self::InvalidCode => formatter.write_str("invalid peer pairing code or secret"),
            Self::RateLimited => formatter.write_str("too many failed peer pairing attempts"),
            Self::EnvironmentMismatch => {
                formatter.write_str("the desktops run in different Kanna environments")
            }
            Self::SelfPairing => formatter.write_str("a desktop cannot pair with itself"),
            Self::InvalidRequest(detail) => {
                write!(formatter, "invalid peer pairing claim: {detail}")
            }
        }
    }
}

pub(crate) fn desktop_id_is_pairable(desktop_id: &str) -> bool {
    !desktop_id.is_empty()
        && desktop_id.len() <= 256
        && desktop_id.chars().all(|character| {
            !character.is_control() && !character.is_whitespace() && character != ':'
        })
}

/// Mints an offer for this desktop's peer key.
pub(crate) fn create_offer(
    desktop_id: &str,
    channel_public_key: &[u8; 32],
    now_ms: u64,
) -> Result<ActivePeerPairingOffer, String> {
    if !desktop_id_is_pairable(desktop_id) {
        return Err("desktop id cannot be carried in a pairing string".to_string());
    }
    let code = generate_pairing_code()?;
    let secret = generate_secret()?;
    let pairing_string = format!(
        "{PAIRING_STRING_PREFIX}:{desktop_id}:{code}:{}:{secret}",
        base32_encode(channel_public_key)
    );
    Ok(ActivePeerPairingOffer {
        code,
        secret,
        pairing_string,
        expires_at_unix_ms: now_ms + PEER_PAIRING_TTL_MS,
        failed_claims: 0,
    })
}

fn generate_secret() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .map_err(|error| format!("failed to open /dev/urandom: {error}"))?
        .read_exact(&mut bytes)
        .map_err(|error| format!("failed to read random bytes: {error}"))?;
    Ok(base32_encode(&bytes))
}

pub(crate) fn parse_pairing_string(text: &str) -> Result<ParsedPairingString, String> {
    let text = text.trim();
    let mut parts = text.split(':');
    let prefix = parts.next().unwrap_or_default();
    if prefix != PAIRING_STRING_PREFIX {
        return Err("not a Kanna desktop pairing string".to_string());
    }
    let desktop_id = parts.next().unwrap_or_default().trim();
    let code = parts.next().unwrap_or_default().trim();
    let key = parts.next().unwrap_or_default().trim();
    let secret = parts.next().unwrap_or_default().trim();
    if parts.next().is_some() {
        return Err("pairing string has too many parts".to_string());
    }
    if !desktop_id_is_pairable(desktop_id) {
        return Err("pairing string names an invalid desktop id".to_string());
    }
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("pairing string code is malformed".to_string());
    }
    let key_bytes =
        base32_decode(key).ok_or_else(|| "pairing string key is malformed".to_string())?;
    let channel_public_key: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| "pairing string key must be 32 bytes".to_string())?;
    if secret.is_empty() || base32_decode(secret).is_none() {
        return Err("pairing string secret is malformed".to_string());
    }
    Ok(ParsedPairingString {
        desktop_id: desktop_id.to_string(),
        code: code.to_ascii_uppercase(),
        channel_public_key,
        secret: secret.to_string(),
    })
}

/// RFC 4648 base32 (uppercase, no padding) - the inverse of
/// `pairing::base32_encode`, tolerant of lowercase input.
pub(crate) fn base32_decode(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for character in text.bytes() {
        let value = ALPHABET
            .iter()
            .position(|&candidate| candidate == character.to_ascii_uppercase())?;
        buffer = (buffer << 5) | value as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    // Leftover bits are padding and must be zero.
    if bits >= 5 || (buffer & ((1 << bits) - 1)) != 0 {
        return None;
    }
    Some(out)
}

/// Verifies a sealed claim against the live offer, consuming the offer on
/// success and counting a failure otherwise. `local_desktop_id` and
/// `local_environment` are the issuer's; `declared_desktop_id` is what the
/// claimant's handshake hello announced, which must agree with the claim.
pub(crate) fn verify_claim(
    offer: &mut Option<ActivePeerPairingOffer>,
    claim: &PeerPairingClaim,
    declared_desktop_id: Option<&str>,
    local_desktop_id: &str,
    local_environment: &str,
    now_ms: u64,
) -> Result<(), PeerClaimError> {
    if !desktop_id_is_pairable(&claim.desktop_id) {
        return Err(PeerClaimError::InvalidRequest("desktop id".into()));
    }
    if claim.desktop_name.trim().is_empty() || claim.desktop_name.len() > 256 {
        return Err(PeerClaimError::InvalidRequest("desktop name".into()));
    }
    if declared_desktop_id.is_some_and(|declared| declared != claim.desktop_id) {
        return Err(PeerClaimError::InvalidRequest(
            "desktop id does not match the handshake".into(),
        ));
    }
    if claim.desktop_id == local_desktop_id {
        return Err(PeerClaimError::SelfPairing);
    }
    if claim.environment != local_environment {
        return Err(PeerClaimError::EnvironmentMismatch);
    }
    let Some(active) = offer.as_mut() else {
        return Err(PeerClaimError::NoActiveOffer);
    };
    if now_ms >= active.expires_at_unix_ms {
        *offer = None;
        return Err(PeerClaimError::Expired);
    }
    if active.failed_claims >= PEER_PAIRING_MAX_ATTEMPTS {
        *offer = None;
        return Err(PeerClaimError::RateLimited);
    }
    let code_ok = constant_time_eq(
        claim.code.trim().to_ascii_uppercase().as_bytes(),
        active.code.as_bytes(),
    );
    let secret_ok = constant_time_eq(
        claim.secret.trim().to_ascii_uppercase().as_bytes(),
        active.secret.as_bytes(),
    );
    if !(code_ok && secret_ok) {
        active.failed_claims += 1;
        if active.failed_claims >= PEER_PAIRING_MAX_ATTEMPTS {
            *offer = None;
            return Err(PeerClaimError::RateLimited);
        }
        return Err(PeerClaimError::InvalidCode);
    }
    // Consumed: a second claim with the same string finds no offer.
    *offer = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(code: &str, secret: &str) -> PeerPairingClaim {
        PeerPairingClaim {
            code: code.into(),
            secret: secret.into(),
            desktop_id: "desktop-a".into(),
            desktop_name: "A".into(),
            environment: "development".into(),
            transfer_identity: None,
        }
    }

    #[test]
    fn a_pairing_string_round_trips_and_pins_the_key() {
        let key = [7u8; 32];
        let offer = create_offer("desktop-b", &key, 1_000).unwrap();
        assert!(offer.pairing_string.starts_with("KANNA-PEER:desktop-b:"));
        let parsed = parse_pairing_string(&offer.pairing_string).unwrap();
        assert_eq!(parsed.desktop_id, "desktop-b");
        assert_eq!(parsed.code, offer.code);
        assert_eq!(parsed.channel_public_key, key);
        assert_eq!(parsed.secret, offer.secret);
        assert_eq!(offer.expires_at_unix_ms, 1_000 + PEER_PAIRING_TTL_MS);
    }

    #[test]
    fn malformed_strings_are_refused() {
        assert!(parse_pairing_string("KANNA2:D:ABC123:KEY:SECRET").is_err());
        assert!(parse_pairing_string("KANNA-PEER:desktop:ABC123").is_err());
        assert!(parse_pairing_string("KANNA-PEER:desk top:ABC123:AAAA:BBBB").is_err());
        assert!(parse_pairing_string("KANNA-PEER:d:ABC12:AAAA:BBBB").is_err());
        let short_key = format!("KANNA-PEER:d:ABC123:{}:BBBB", base32_encode(&[1u8; 16]));
        assert!(parse_pairing_string(&short_key).is_err());
        let ok = format!(
            "KANNA-PEER:d:ABC123:{}:{}",
            base32_encode(&[1u8; 32]),
            base32_encode(&[2u8; 16])
        );
        assert!(parse_pairing_string(&ok).is_ok());
        assert!(parse_pairing_string(&format!("{ok}:extra")).is_err());
    }

    #[test]
    fn base32_decode_inverts_encode() {
        for length in 0..40 {
            let bytes: Vec<u8> = (0..length).map(|index| (index * 37 % 251) as u8).collect();
            assert_eq!(base32_decode(&base32_encode(&bytes)).unwrap(), bytes);
        }
        assert!(base32_decode("1").is_none());
    }

    #[test]
    fn a_claim_needs_the_code_and_the_secret_and_consumes_the_offer() {
        let key = [3u8; 32];
        let mut offer = Some(create_offer("desktop-b", &key, 1_000).unwrap());
        let (code, secret) = {
            let active = offer.as_ref().unwrap();
            (active.code.clone(), active.secret.clone())
        };
        assert_eq!(
            verify_claim(
                &mut offer,
                &claim(&code, "WRONG"),
                None,
                "desktop-b",
                "development",
                2_000
            ),
            Err(PeerClaimError::InvalidCode)
        );
        assert_eq!(offer.as_ref().unwrap().failed_claims, 1);
        assert_eq!(
            verify_claim(
                &mut offer,
                &claim("000000", &secret),
                None,
                "desktop-b",
                "development",
                2_000
            ),
            Err(PeerClaimError::InvalidCode)
        );
        assert_eq!(
            verify_claim(
                &mut offer,
                &claim(&code, &secret),
                Some("desktop-a"),
                "desktop-b",
                "development",
                2_000
            ),
            Ok(())
        );
        assert!(offer.is_none(), "consumed");
        assert_eq!(
            verify_claim(
                &mut offer,
                &claim(&code, &secret),
                None,
                "desktop-b",
                "development",
                2_000
            ),
            Err(PeerClaimError::NoActiveOffer)
        );
    }

    #[test]
    fn attempts_expiry_environment_and_identity_are_enforced() {
        let key = [3u8; 32];
        let mut offer = Some(create_offer("desktop-b", &key, 1_000).unwrap());
        let (code, secret) = {
            let active = offer.as_ref().unwrap();
            (active.code.clone(), active.secret.clone())
        };
        for _ in 0..PEER_PAIRING_MAX_ATTEMPTS - 1 {
            assert_eq!(
                verify_claim(
                    &mut offer,
                    &claim("BADBAD", &secret),
                    None,
                    "desktop-b",
                    "development",
                    2_000
                ),
                Err(PeerClaimError::InvalidCode)
            );
        }
        assert_eq!(
            verify_claim(
                &mut offer,
                &claim("BADBAD", &secret),
                None,
                "desktop-b",
                "development",
                2_000
            ),
            Err(PeerClaimError::RateLimited)
        );
        assert!(offer.is_none());

        let mut offer = Some(create_offer("desktop-b", &key, 1_000).unwrap());
        assert_eq!(
            verify_claim(
                &mut offer,
                &claim(&code, &secret),
                None,
                "desktop-b",
                "development",
                1_000 + PEER_PAIRING_TTL_MS
            ),
            Err(PeerClaimError::Expired)
        );

        let mut offer = Some(create_offer("desktop-b", &key, 1_000).unwrap());
        let (code, secret) = {
            let active = offer.as_ref().unwrap();
            (active.code.clone(), active.secret.clone())
        };
        let mut staging = claim(&code, &secret);
        staging.environment = "staging".into();
        assert_eq!(
            verify_claim(
                &mut offer,
                &staging,
                None,
                "desktop-b",
                "development",
                2_000
            ),
            Err(PeerClaimError::EnvironmentMismatch)
        );
        let mut me = claim(&code, &secret);
        me.desktop_id = "desktop-b".into();
        assert_eq!(
            verify_claim(&mut offer, &me, None, "desktop-b", "development", 2_000),
            Err(PeerClaimError::SelfPairing)
        );
        assert!(matches!(
            verify_claim(
                &mut offer,
                &claim(&code, &secret),
                Some("desktop-z"),
                "desktop-b",
                "development",
                2_000
            ),
            Err(PeerClaimError::InvalidRequest(_))
        ));
        assert!(
            offer.is_some(),
            "a refused claim that never reached the code check spends nothing"
        );
    }
}
