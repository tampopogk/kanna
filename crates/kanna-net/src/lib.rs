mod client;
pub mod log;
mod node;
pub mod pair;

pub use iroh::SecretKey;
pub use client::Client;
pub use node::{Config, Failure, Host, Node, DEFAULT_PORT};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub const ALPN: &[u8] = b"kanna/1";

pub const KINDS: &[&str] = &["control", "file", "port", "sync", "job", "transfer"];

#[derive(Clone, Debug, PartialEq)]
pub struct Refused {
    pub code: &'static str,
}

impl Refused {
    pub fn new(code: &'static str) -> Refused {
        Refused { code }
    }

    pub fn message(&self) -> &'static str {
        match self.code {
            "bad_signature" => "a signature doesn't verify",
            "wrong_fleet" => "it belongs to another fleet",
            "untrusted_signer" => "its signer wasn't trusted when it signed",
            "revoked" => "that key is revoked",
            "identity_mismatch" => "that name belongs to another key",
            "unpaired" => "it isn't paired with this machine",
            "untrusted_endpoint" => "that endpoint isn't trusted",
            "pake_confirmation_failed" => "the code didn't match",
            "bad_pake_message" => "the pairing exchange was malformed",
            "handoff_tampered" => "the peer log handed over was altered",
            "gesture_required" => "it wasn't confirmed in Kanna.app",
            "code_used" => "that code was used already",
            "code_expired" => "that code expired",
            "rate_limited" => "too many wrong codes: start pairing again",
            "already_paired" => "this machine is paired already",
            _ => "refused",
        }
    }
}

pub fn canonical(value: &Value) -> Vec<u8> {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut out = Map::new();
                for key in keys {
                    out.insert(key.clone(), sorted(&map[key]));
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_vec(&sorted(value)).unwrap_or_default()
}

pub fn digest(value: &Value) -> String {
    hex(&Sha256::digest(canonical(value)))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok()).collect()
}

pub fn is_key(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub fn read_key(path: &std::path::Path) -> Result<SecretKey, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(text) = std::fs::read_to_string(path) {
        let bytes: [u8; 32] = unhex(text.trim()).and_then(|b| b.try_into().ok()).ok_or_else(|| format!("{} isn't a device key", path.display()))?;
        return Ok(SecretKey::from_bytes(&bytes));
    }
    let secret = SecretKey::generate();
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(|e| format!("can't make {}: {e}", path.display()))?;
    file.write_all(format!("{}\n", hex(&secret.to_bytes())).as_bytes()).map_err(|e| format!("can't write {}: {e}", path.display()))?;
    Ok(secret)
}
