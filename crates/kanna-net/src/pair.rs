use crate::log::{sign, verify, PeerLog, DOMAIN};
use crate::{canonical, hex, unhex, Refused};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use iroh::SecretKey;
use serde_json::{json, Value};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::time::{Duration, Instant};

pub const OFFER_LIFETIME: Duration = Duration::from_secs(300);

pub const ATTEMPTS: u32 = 3;

pub struct Offer {
    pub code: String,
    pub session: String,
    pub expires: Instant,
    pub attempts: u32,
    pub used: bool,
}

impl Offer {
    pub fn new(now: Instant) -> Offer {
        let mut bytes = [0u8; 20];
        getrandom::getrandom(&mut bytes).expect("the OS has no random numbers");
        let number = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 1_000_000;
        Offer { code: format!("{number:06}"), session: hex(&bytes[4..]), expires: now + OFFER_LIFETIME, attempts: 0, used: false }
    }

    pub fn begin(&mut self, now: Instant) -> Result<(), Refused> {
        if self.used {
            return Err(Refused::new("code_used"));
        }
        if now >= self.expires {
            return Err(Refused::new("code_expired"));
        }
        if self.attempts >= ATTEMPTS {
            return Err(Refused::new("rate_limited"));
        }
        self.attempts += 1;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Role {
    A,
    B,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Role::A => "A",
            Role::B => "B",
        }
    }

    fn other(self) -> Role {
        match self {
            Role::A => Role::B,
            Role::B => Role::A,
        }
    }
}

pub fn hello_a(descriptor: &Value, anchor: &Value) -> Value {
    json!({ "domain": DOMAIN, "device": descriptor, "anchor": anchor })
}

pub fn hello_b(descriptor: &Value, session: &str) -> Value {
    json!({ "domain": DOMAIN, "device": descriptor, "session": session })
}

fn derive(shared: &[u8], label: &str) -> [u8; 32] {
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(None, shared).expand(format!("{DOMAIN}/pairing/{label}").as_bytes(), &mut key).expect("32 bytes is a valid HKDF length");
    key
}

fn tag(shared: &[u8], label: &str, message: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(&derive(shared, label)).expect("HMAC takes any key length");
    mac.update(message);
    hex(&mac.finalize().into_bytes())
}

fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

pub struct Party {
    role: Role,
    pub hello_a: Value,
    pub hello_b: Value,
    pake: Option<Spake2<Ed25519Group>>,
    pub outbound: Vec<u8>,
    shared: Vec<u8>,
    transcript: Vec<u8>,
}

impl Party {
    pub fn new(role: Role, code: &str, hello_a: Value, hello_b: Value) -> Party {
        let (password, a, b) = (Password::new(code.as_bytes()), Identity::new(&canonical(&hello_a)), Identity::new(&canonical(&hello_b)));
        let (pake, outbound) = match role {
            Role::A => Spake2::<Ed25519Group>::start_a(&password, &a, &b),
            Role::B => Spake2::<Ed25519Group>::start_b(&password, &a, &b),
        };
        Party { role, hello_a, hello_b, pake: Some(pake), outbound, shared: vec![], transcript: vec![] }
    }

    pub fn finish(&mut self, secret: &SecretKey, incoming: &[u8]) -> Result<Value, Refused> {
        let pake = self.pake.take().ok_or_else(|| Refused::new("bad_pake_message"))?;
        self.shared = pake.finish(incoming).map_err(|_| Refused::new("bad_pake_message"))?;
        let (a, b) = match self.role {
            Role::A => (hex(&self.outbound), hex(incoming)),
            Role::B => (hex(incoming), hex(&self.outbound)),
        };
        self.transcript = canonical(&json!({ "domain": DOMAIN, "a": self.hello_a, "b": self.hello_b, "message_a": a, "message_b": b }));
        Ok(json!({ "mac": tag(&self.shared, &format!("confirm-{}", self.role.name()), &self.transcript), "signature": sign(secret, &self.transcript) }))
    }

    pub fn confirm(&self, proof: &Value) -> Result<(), Refused> {
        let other = self.role.other();
        let expected = tag(&self.shared, &format!("confirm-{}", other.name()), &self.transcript);
        if !same(proof["mac"].as_str().unwrap_or_default(), &expected) {
            return Err(Refused::new("pake_confirmation_failed"));
        }
        let hello = if other == Role::B { &self.hello_b } else { &self.hello_a };
        verify(hello["device"]["key"].as_str().unwrap_or_default(), proof["signature"].as_str().unwrap_or_default(), &self.transcript)
    }

    pub fn seal(&self, payload: &Value) -> Value {
        json!({ "payload": payload, "mac": tag(&self.shared, "handoff", &canonical(payload)) })
    }

    pub fn open(&self, wire: &Value, own_key: &str) -> Result<PeerLog, Refused> {
        if !same(wire["mac"].as_str().unwrap_or_default(), &tag(&self.shared, "handoff", &canonical(&wire["payload"]))) {
            return Err(Refused::new("handoff_tampered"));
        }
        let anchor = &self.hello_a["anchor"];
        if wire["payload"]["anchor"] != *anchor {
            return Err(Refused::new("wrong_fleet"));
        }
        let mut log = PeerLog::new(anchor.clone());
        log.sync(&wire["payload"])?;
        let trusted = log.view().trusted();
        if !trusted.contains(self.hello_a["device"]["key"].as_str().unwrap_or_default()) || !trusted.contains(own_key) {
            return Err(Refused::new("untrusted_endpoint"));
        }
        Ok(log)
    }
}

pub fn pake_hex(bytes: &[u8]) -> String {
    hex(bytes)
}

pub fn pake_bytes(value: &Value) -> Result<Vec<u8>, Refused> {
    value.as_str().and_then(unhex).ok_or_else(|| Refused::new("bad_pake_message"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::tests::Device;

    type Relay<'a> = &'a dyn Fn(&str, Value) -> Value;

    fn pass(_: &str, value: Value) -> Value {
        value
    }

    fn pair(inviter: &mut Device, newcomer: &mut Device, offer: &mut Offer, code: &str, gesture: bool, now: Instant, relay: Relay) -> Result<Value, Refused> {
        offer.begin(now)?;
        if newcomer.log.is_some() {
            return Err(Refused::new("already_paired"));
        }
        let ha = hello_a(&inviter.descriptor(), inviter.log.as_ref().unwrap().anchor());
        let hb = hello_b(&newcomer.descriptor(), &offer.session);
        let mut a = Party::new(Role::A, code, ha.clone(), relay("hello_b", hb.clone()));
        let mut b = Party::new(Role::B, &offer.code, relay("hello_a", ha), hb);
        let to_a = relay("pake_b", json!(pake_hex(&b.outbound)));
        let to_b = relay("pake_a", json!(pake_hex(&a.outbound)));
        let proof_a = a.finish(&inviter.secret, &pake_bytes(&to_a)?)?;
        let proof_b = b.finish(&newcomer.secret, &pake_bytes(&to_b)?)?;
        a.confirm(&relay("confirm_b", proof_b))?;
        b.confirm(&relay("confirm_a", proof_a))?;
        offer.used = true;
        if !gesture {
            return Err(Refused::new("gesture_required"));
        }
        let entry = inviter.entry("add", a.hello_b["device"].clone());
        let mut staged = inviter.log.clone().unwrap();
        staged.merge(std::slice::from_ref(&entry))?;
        let wire = relay("handoff", a.seal(&staged.export()));
        let received = b.open(&wire, &newcomer.key())?;
        inviter.log = Some(staged);
        newcomer.log = Some(received);
        Ok(entry)
    }

    struct Setup {
        a: Device,
        b: Device,
        offer: Offer,
        now: Instant,
    }

    fn setup() -> Setup {
        let mut a = Device::new("mbp");
        a.bootstrap();
        let now = Instant::now();
        Setup { a, b: Device::new("linux-vm"), offer: Offer::new(now), now }
    }

    fn wrong(code: &str) -> String {
        if code == "000000" { "000001".into() } else { "000000".into() }
    }

    impl Setup {
        fn refused(&mut self, reason: &str, code: Option<&str>, gesture: bool, at: Option<Instant>, relay: Relay) {
            let before = self.a.log.as_ref().unwrap().export();
            let code = code.map(String::from).unwrap_or_else(|| self.offer.code.clone());
            let result = pair(&mut self.a, &mut self.b, &mut self.offer, &code, gesture, at.unwrap_or(self.now), relay);
            assert_eq!(result.err().map(|r| r.code), Some(reason));
            assert_eq!(before, self.a.log.as_ref().unwrap().export());
            assert!(self.b.log.is_none());
        }
    }

    #[test]
    fn the_real_pake_pairs_three_devices() {
        let mut s = setup();
        let code = s.offer.code.clone();
        pair(&mut s.a, &mut s.b, &mut s.offer, &code, true, s.now, &pass).unwrap();
        let mut c = Device::new("build-mac");
        let mut offer = Offer::new(s.now);
        let code = offer.code.clone();
        pair(&mut s.a, &mut c, &mut offer, &code, true, s.now, &pass).unwrap();
        let export = s.a.log.as_ref().unwrap().export();
        s.b.log.as_mut().unwrap().sync(&export).unwrap();
        let all: std::collections::BTreeSet<String> = [s.a.key(), s.b.key(), c.key()].into();
        for d in [&s.a, &s.b, &c] {
            assert_eq!(d.log.as_ref().unwrap().view().trusted(), all);
        }
    }

    #[test]
    fn a_wrong_code_adds_neither_side() {
        let mut s = setup();
        let code = wrong(&s.offer.code);
        s.refused("pake_confirmation_failed", Some(&code), true, None, &pass);
    }

    #[test]
    fn a_directory_cannot_substitute_either_key_or_the_anchor() {
        let forged = Device::new("directory");
        for slot in ["hello_a", "hello_b"] {
            let mut s = setup();
            let key = forged.key();
            s.refused("pake_confirmation_failed", None, true, None, &move |label, mut value| {
                if label == slot {
                    value["device"]["key"] = json!(key);
                }
                value
            });
        }
        let mut s = setup();
        let anchor = forged.descriptor();
        s.refused("pake_confirmation_failed", None, true, None, &move |label, mut value| {
            if label == "hello_a" {
                value["anchor"] = anchor.clone();
            }
            value
        });
    }

    #[test]
    fn confirmation_proves_the_device_key() {
        let mut s = setup();
        let impostor = Device::new("impostor");
        s.refused("bad_signature", None, true, None, &move |label, mut value| {
            if label == "confirm_b" {
                value["signature"] = json!(sign(&impostor.secret, b"unrelated transcript"));
            }
            value
        });
    }

    #[test]
    fn a_replayed_confirmation_fails_in_a_fresh_session() {
        let mut s = setup();
        let captured = std::cell::RefCell::new(serde_json::Map::new());
        s.refused("gesture_required", None, false, None, &|label, value| {
            captured.borrow_mut().insert(label.into(), value.clone());
            value
        });
        s.offer = Offer::new(s.now);
        let captured = captured.into_inner();
        s.refused("pake_confirmation_failed", None, true, None, &move |label, value| if label == "confirm_b" { captured[label].clone() } else { value });
    }

    #[test]
    fn a_reflected_confirmation_fails() {
        let mut s = setup();
        let captured = std::cell::RefCell::new(Value::Null);
        s.refused("pake_confirmation_failed", None, true, None, &|label, value| match label {
            "confirm_b" => {
                *captured.borrow_mut() = value.clone();
                value
            }
            "confirm_a" => captured.borrow().clone(),
            _ => value,
        });
    }

    #[test]
    fn the_log_handoff_is_authenticated() {
        let mut s = setup();
        s.refused("handoff_tampered", None, true, None, &|label, mut value| {
            if label == "handoff" {
                value["payload"]["entries"] = json!([]);
            }
            value
        });
    }

    #[test]
    fn the_app_gesture_is_required() {
        let mut s = setup();
        s.refused("gesture_required", None, false, None, &pass);
        assert!(s.offer.used);
    }

    #[test]
    fn a_code_works_once() {
        let mut s = setup();
        let code = s.offer.code.clone();
        pair(&mut s.a, &mut s.b, &mut s.offer, &code, true, s.now, &pass).unwrap();
        let mut c = Device::new("c");
        assert_eq!(pair(&mut s.a, &mut c, &mut s.offer, &code, true, s.now, &pass).err().map(|r| r.code), Some("code_used"));
    }

    #[test]
    fn a_code_expires_after_five_minutes() {
        let mut s = setup();
        let late = s.now + OFFER_LIFETIME;
        s.refused("code_expired", None, true, Some(late), &pass);
    }

    #[test]
    fn three_wrong_codes_close_the_offer() {
        let mut s = setup();
        let code = wrong(&s.offer.code);
        for _ in 0..ATTEMPTS {
            s.refused("pake_confirmation_failed", Some(&code), true, None, &pass);
        }
        s.refused("rate_limited", None, true, None, &pass);
    }

    #[test]
    fn a_name_coming_back_with_a_new_key_is_refused() {
        let mut s = setup();
        let code = s.offer.code.clone();
        pair(&mut s.a, &mut s.b, &mut s.offer, &code, true, s.now, &pass).unwrap();
        let mut replacement = Device::new("linux-vm");
        let mut offer = Offer::new(s.now);
        let code = offer.code.clone();
        assert_eq!(pair(&mut s.a, &mut replacement, &mut offer, &code, true, s.now, &pass).err().map(|r| r.code), Some("identity_mismatch"));
        assert!(replacement.log.is_none());
    }

    #[test]
    fn a_surviving_third_device_pairs_a_fresh_key() {
        let (mut a, mut b, mut c) = crate::log::tests::fleet();
        let (a_key, b_key) = (a.key(), b.key());
        a.revoke(&b_key, 0);
        b.revoke(&a_key, 0);
        for d in [&a, &b] {
            let export = d.log.as_ref().unwrap().export();
            c.log().sync(&export).unwrap();
        }
        let mut replacement = Device::new("mbp-repaired");
        let now = Instant::now();
        let mut offer = Offer::new(now);
        let code = offer.code.clone();
        pair(&mut c, &mut replacement, &mut offer, &code, true, now, &pass).unwrap();
        let view = replacement.log.as_ref().unwrap().view();
        assert_eq!(view.trusted(), [c.key(), replacement.key()].into());
        assert_eq!(view.revoked, [a_key, b_key].into());
    }
}
