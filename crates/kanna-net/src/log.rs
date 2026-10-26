use crate::{canonical, digest, hex, unhex, Refused};
use iroh::{PublicKey, SecretKey, Signature};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub const DOMAIN: &str = "kanna/fleet/1";

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub names: BTreeMap<String, BTreeSet<String>>,
    pub revoked: BTreeSet<String>,
    pub descriptors: BTreeMap<String, Value>,
}

impl View {
    pub fn trusted(&self) -> BTreeSet<String> {
        self.names.values().filter(|keys| keys.len() == 1).filter_map(|keys| keys.iter().next()).filter(|key| !self.revoked.contains(*key)).cloned().collect()
    }

    pub fn resolve(&self, name: &str, key: &str) -> Result<(), Refused> {
        match self.names.get(name) {
            None => Err(Refused::new("unpaired")),
            Some(keys) if keys.len() != 1 || !keys.contains(key) => Err(Refused::new("identity_mismatch")),
            Some(_) if self.revoked.contains(key) => Err(Refused::new("revoked")),
            Some(_) => Ok(()),
        }
    }

    pub fn name_of(&self, key: &str) -> Option<String> {
        self.names.iter().find(|(_, keys)| keys.len() == 1 && keys.contains(key)).map(|(name, _)| name.clone())
    }

    pub fn key_of(&self, name: &str) -> Option<String> {
        match self.names.get(name) {
            Some(keys) if keys.len() == 1 => keys.iter().next().cloned(),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PeerLog {
    anchor: Value,
    id: String,
    events: BTreeMap<String, Value>,
    pending: BTreeMap<String, Value>,
}

pub fn verify(key: &str, signature: &str, message: &[u8]) -> Result<(), Refused> {
    let bad = || Refused::new("bad_signature");
    let key: [u8; 32] = unhex(key).and_then(|k| k.try_into().ok()).ok_or_else(bad)?;
    let signature: [u8; 64] = unhex(signature).and_then(|s| s.try_into().ok()).ok_or_else(bad)?;
    let key = PublicKey::from_bytes(&key).map_err(|_| bad())?;
    key.verify(message, &Signature::from_bytes(&signature)).map_err(|_| bad())
}

pub fn sign(secret: &SecretKey, message: &[u8]) -> String {
    hex(&secret.sign(message).to_bytes())
}

pub fn certificate(secret: &SecretKey, app_key: &str) -> Value {
    let body = json!({ "domain": format!("{DOMAIN}/app"), "key": app_key, "device": secret.public().to_string() });
    let signature = sign(secret, &canonical(&body));
    json!({ "body": body, "signature": signature })
}

impl PeerLog {
    pub fn new(anchor: Value) -> PeerLog {
        let id = digest(&anchor);
        PeerLog { anchor, id, events: BTreeMap::new(), pending: BTreeMap::new() }
    }

    pub fn anchor(&self) -> &Value {
        &self.anchor
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    pub fn view(&self) -> View {
        self.view_of(self.events.keys())
    }

    fn view_of<'a>(&'a self, ids: impl Iterator<Item = &'a String>) -> View {
        let mut view = View { names: BTreeMap::new(), revoked: BTreeSet::new(), descriptors: BTreeMap::new() };
        let anchor_key = self.anchor["key"].as_str().unwrap_or_default().to_string();
        view.names.entry(self.anchor["name"].as_str().unwrap_or_default().to_string()).or_default().insert(anchor_key.clone());
        view.descriptors.insert(anchor_key.clone(), self.anchor.clone());
        let ids: Vec<&String> = ids.collect();
        let mut seen: BTreeMap<String, HashSet<String>> = BTreeMap::new();
        for id in &ids {
            let body = &self.events[*id]["body"];
            if body["kind"] == "revoke" {
                let key = body["payload"]["key"].as_str().unwrap_or_default().to_string();
                view.revoked.insert(key.clone());
                seen.entry(key).or_default().extend(self.ancestors(&body["parents"]));
            }
        }
        let mut signers: HashSet<String> = [anchor_key].into();
        let mut adds: Vec<&String> = ids.iter().copied().filter(|id| self.events[*id]["body"]["kind"] == "add").collect();
        loop {
            let before = adds.len();
            adds.retain(|id| {
                let body = &self.events[*id]["body"];
                let signer = body["signer"].as_str().unwrap_or_default();
                let unseen = view.revoked.contains(signer) && !seen.get(signer).is_some_and(|s| s.contains(*id));
                if !signers.contains(signer) || unseen {
                    return true;
                }
                let payload = &body["payload"];
                let key = payload["key"].as_str().unwrap_or_default().to_string();
                signers.insert(key.clone());
                view.names.entry(payload["name"].as_str().unwrap_or_default().to_string()).or_default().insert(key.clone());
                let merged = match view.descriptors.get(&key) {
                    Some(known) => merge_descriptor(known, payload),
                    None => payload.clone(),
                };
                view.descriptors.insert(key, merged);
                false
            });
            if adds.len() == before {
                break;
            }
        }
        view
    }

    fn ancestors(&self, parents: &Value) -> HashSet<String> {
        let mut found = HashSet::new();
        let mut todo: Vec<String> = strings(parents);
        while let Some(id) = todo.pop() {
            if found.insert(id.clone()) {
                todo.extend(strings(&self.events[&id]["body"]["parents"]));
            }
        }
        found
    }

    fn authorize(&self, entry: &Value) -> Result<(), Refused> {
        let body = &entry["body"];
        let ancestors = self.ancestors(&body["parents"]);
        let past = self.view_of(ancestors.iter());
        if !past.trusted().contains(body["signer"].as_str().unwrap_or_default()) {
            return Err(Refused::new("untrusted_signer"));
        }
        let payload = &body["payload"];
        let key = payload["key"].as_str().unwrap_or_default();
        match body["kind"].as_str() {
            Some("add") => {
                if past.revoked.contains(key) {
                    return Err(Refused::new("revoked"));
                }
                let name = payload["name"].as_str().unwrap_or_default();
                if name.is_empty() || key.len() != 64 {
                    return Err(Refused::new("bad_entry"));
                }
                if past.names.get(name).is_some_and(|keys| keys.len() != 1 || !keys.contains(key)) {
                    return Err(Refused::new("identity_mismatch"));
                }
                Ok(())
            }
            Some("revoke") => Ok(()),
            _ => Err(Refused::new("unknown_entry")),
        }
    }

    pub fn merge(&mut self, entries: &[Value]) -> Result<usize, Refused> {
        let mut stage = self.clone();
        for entry in entries {
            let body = &entry["body"];
            verify(body["signer"].as_str().unwrap_or_default(), entry["signature"].as_str().unwrap_or_default(), &canonical(body))?;
            if body["domain"] != DOMAIN || body["fleet"] != self.id {
                return Err(Refused::new("wrong_log"));
            }
            if !body["parents"].is_array() {
                return Err(Refused::new("bad_entry"));
            }
            let id = digest(entry);
            if !stage.events.contains_key(&id) {
                stage.pending.insert(id, entry.clone());
            }
        }
        let before = stage.events.len();
        loop {
            let ready: Vec<String> = stage.pending.iter().filter(|(_, e)| strings(&e["body"]["parents"]).iter().all(|p| stage.events.contains_key(p))).map(|(id, _)| id.clone()).collect();
            if ready.is_empty() {
                break;
            }
            for id in ready {
                let entry = stage.pending.remove(&id).unwrap_or_default();
                stage.authorize(&entry)?;
                stage.events.insert(id, entry);
            }
        }
        let added = stage.events.len() - before;
        *self = stage;
        Ok(added)
    }

    pub fn entries(&self) -> Vec<Value> {
        self.events.values().chain(self.pending.values()).cloned().collect()
    }

    pub fn export(&self) -> Value {
        json!({ "anchor": self.anchor, "entries": self.entries() })
    }

    pub fn sync(&mut self, envelope: &Value) -> Result<usize, Refused> {
        if envelope["anchor"] != self.anchor {
            return Err(Refused::new("wrong_log"));
        }
        self.merge(envelope["entries"].as_array().map(Vec::as_slice).unwrap_or_default())
    }

    pub fn frontier(&self) -> Vec<String> {
        let referenced: HashSet<String> = self.events.values().flat_map(|e| strings(&e["body"]["parents"])).collect();
        self.events.keys().filter(|id| !referenced.contains(*id)).cloned().collect()
    }

    pub fn entry(&self, secret: &SecretKey, kind: &str, payload: Value, parents: Option<Vec<String>>, claimed_time: i64) -> Value {
        let mut parents = parents.unwrap_or_else(|| self.frontier());
        parents.sort();
        let body = json!({
            "domain": DOMAIN,
            "fleet": self.id,
            "kind": kind,
            "signer": secret.public().to_string(),
            "parents": parents,
            "payload": payload,
            "claimed_time": claimed_time,
        });
        let signature = sign(secret, &canonical(&body));
        json!({ "body": body, "signature": signature })
    }

    pub fn accept_endpoint(&self, key: &str, certificate: Option<&Value>) -> Result<(), Refused> {
        let view = self.view();
        let untrusted = || Refused::new("untrusted_endpoint");
        if view.revoked.contains(key) {
            return Err(untrusted());
        }
        if view.trusted().contains(key) {
            return Ok(());
        }
        let certificate = certificate.ok_or_else(untrusted)?;
        let body = &certificate["body"];
        verify(body["device"].as_str().unwrap_or_default(), certificate["signature"].as_str().unwrap_or_default(), &canonical(body))?;
        if body["domain"] != format!("{DOMAIN}/app").as_str() || body["key"] != key || !view.trusted().contains(body["device"].as_str().unwrap_or_default()) {
            return Err(untrusted());
        }
        Ok(())
    }
}

fn merge_descriptor(known: &Value, newer: &Value) -> Value {
    let mut merged = newer.clone();
    let mut addrs = strings(&known["addrs"]);
    for addr in strings(&newer["addrs"]) {
        if !addrs.contains(&addr) {
            addrs.push(addr);
        }
    }
    merged["addrs"] = json!(addrs);
    merged
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub struct Device {
        pub name: String,
        pub secret: SecretKey,
        pub log: Option<PeerLog>,
    }

    impl Device {
        pub fn new(name: &str) -> Device {
            Device { name: name.into(), secret: SecretKey::generate(), log: None }
        }

        pub fn key(&self) -> String {
            self.secret.public().to_string()
        }

        pub fn descriptor(&self) -> Value {
            json!({ "name": self.name, "key": self.key(), "capabilities": {}, "relay": null, "addrs": [] })
        }

        pub fn bootstrap(&mut self) {
            self.log = Some(PeerLog::new(self.descriptor()));
        }

        pub fn log(&mut self) -> &mut PeerLog {
            self.log.as_mut().unwrap()
        }

        pub fn entry(&self, kind: &str, payload: Value) -> Value {
            self.log.as_ref().unwrap().entry(&self.secret, kind, payload, None, 0)
        }

        pub fn revoke(&mut self, key: &str, claimed_time: i64) -> Value {
            let entry = self.log.as_ref().unwrap().entry(&self.secret, "revoke", json!({ "key": key }), None, claimed_time);
            self.log().merge(std::slice::from_ref(&entry)).unwrap();
            entry
        }

        pub fn add(&mut self, other: &Device) {
            let entry = self.entry("add", other.descriptor());
            self.log().merge(&[entry]).unwrap();
        }
    }

    pub fn machines() -> (Device, Device, Device) {
        let (mut a, mut b, mut c) = (Device::new("mbp"), Device::new("linux-vm"), Device::new("build-mac"));
        a.bootstrap();
        a.add(&b);
        a.add(&c);
        for d in [&mut b, &mut c] {
            let mut log = PeerLog::new(a.log.as_ref().unwrap().anchor().clone());
            log.sync(&a.log.as_ref().unwrap().export()).unwrap();
            d.log = Some(log);
        }
        (a, b, c)
    }

    fn code(result: Result<usize, Refused>) -> &'static str {
        result.err().map(|r| r.code).unwrap_or("ok")
    }

    #[test]
    fn three_devices_trust_each_other() {
        let (a, b, c) = machines();
        let all: BTreeSet<String> = [a.key(), b.key(), c.key()].into();
        for d in [&a, &b, &c] {
            assert_eq!(d.log.as_ref().unwrap().view().trusted(), all);
        }
    }

    #[test]
    fn a_forged_signature_and_an_unsigned_peer_are_refused() {
        let (a, _, mut c) = machines();
        let attacker = Device::new("directory");
        let mut entry = a.entry("add", attacker.descriptor());
        entry["signature"] = json!(sign(&attacker.secret, &canonical(&entry["body"])));
        let before = c.log().export();
        assert_eq!(code(c.log().merge(&[entry])), "bad_signature");
        assert_eq!(before, c.log().export());
        assert_eq!(c.log().accept_endpoint(&attacker.key(), None).unwrap_err().code, "untrusted_endpoint");
    }

    #[test]
    fn a_valid_signature_from_an_untrusted_signer_is_refused() {
        let (a, _, mut c) = machines();
        let mut attacker = Device::new("directory");
        attacker.log = a.log.clone();
        for (kind, payload) in [("add", attacker.descriptor()), ("revoke", json!({ "key": a.key() }))] {
            let entry = attacker.entry(kind, payload);
            assert_eq!(code(c.log().merge(&[entry])), "untrusted_signer", "{kind}");
        }
    }

    #[test]
    fn tampered_metadata_is_refused_on_sync() {
        let (mut a, _, mut c) = machines();
        let mut entry = a.log().entries()[0].clone();
        entry["body"]["payload"]["addrs"] = json!(["attacker.invalid:1234"]);
        assert_eq!(code(c.log().merge(&[entry])), "bad_signature");
    }

    #[test]
    fn a_name_with_another_key_is_not_a_key_rotation() {
        let (mut a, b, mut c) = machines();
        let replacement = Device::new(&b.name);
        let entry = a.entry("add", replacement.descriptor());
        assert_eq!(code(a.log().merge(&[entry])), "identity_mismatch");
        assert_eq!(c.log().view().resolve(&b.name, &replacement.key()).unwrap_err().code, "identity_mismatch");
    }

    #[test]
    fn another_logs_entries_are_refused() {
        let (mut a, _, _) = machines();
        let mut other = Device::new("other-root");
        other.bootstrap();
        let entries = a.log().entries();
        assert_eq!(code(other.log().merge(&entries)), "wrong_log");
    }

    fn permutations(items: &[Value]) -> Vec<Vec<Value>> {
        if items.len() <= 1 {
            return vec![items.to_vec()];
        }
        let mut all = vec![];
        for i in 0..items.len() {
            let mut rest = items.to_vec();
            let first = rest.remove(i);
            for mut tail in permutations(&rest) {
                tail.insert(0, first.clone());
                all.push(tail);
            }
        }
        all
    }

    #[test]
    fn mutual_revocation_converges_in_all_24_delivery_orders() {
        let (mut a, mut b, c) = machines();
        let (a_key, b_key) = (a.key(), b.key());
        a.revoke(&b_key, i64::MAX);
        b.revoke(&a_key, i64::MIN);
        let mut entries: BTreeMap<String, Value> = BTreeMap::new();
        for d in [&a, &b] {
            for e in d.log.as_ref().unwrap().entries() {
                entries.insert(digest(&e), e);
            }
        }
        assert_eq!(entries.len(), 4);
        let entries: Vec<Value> = entries.into_values().collect();
        let orders = permutations(&entries);
        assert_eq!(orders.len(), 24);
        for order in orders {
            for observer in [&a, &b, &c] {
                let mut log = observer.log.clone().unwrap();
                let mut seen = log.view().revoked;
                for entry in &order {
                    log.merge(std::slice::from_ref(entry)).unwrap();
                    assert!(seen.is_subset(&log.view().revoked));
                    seen.extend(log.view().revoked);
                    assert!(seen.is_disjoint(&log.view().trusted()));
                }
                let twice: Vec<Value> = entries.iter().chain(entries.iter()).cloned().collect();
                log.merge(&twice).unwrap();
                assert_eq!(log.pending(), 0);
                assert_eq!(log.view().revoked, [a_key.clone(), b_key.clone()].into());
                assert_eq!(log.view().trusted(), [c.key()].into());
            }
        }
    }

    #[test]
    fn a_revoked_author_cannot_sign_after_it_knows_of_its_revocation() {
        let (mut a, _, mut c) = machines();
        c.revoke(&a.key(), 0);
        let export = c.log().export();
        a.log().sync(&export).unwrap();
        let entry = a.entry("revoke", json!({ "key": c.key() }));
        assert_eq!(code(a.log().merge(&[entry])), "untrusted_signer");
    }

    #[test]
    fn a_revoked_key_never_returns_by_name_replay_or_stale_branch() {
        let (mut a, mut b, mut c) = machines();
        let past: Vec<String> = a.log().frontier();
        c.revoke(&b.key(), 0);
        let export = c.log().export();
        a.log().sync(&export).unwrap();
        b.log().sync(&export).unwrap();
        let mut target = c.log.clone().unwrap();
        for (signer, expected) in [(&b, "untrusted_signer"), (&c, "revoked")] {
            for name in [b.name.as_str(), "renamed-stolen-device"] {
                let mut payload = b.descriptor();
                payload["name"] = json!(name);
                let entry = signer.entry("add", payload.clone());
                assert_eq!(code(target.merge(&[entry])), expected);
                let stale = signer.log.as_ref().unwrap().entry(&signer.secret, "add", payload, Some(past.clone()), i64::MAX);
                target.merge(&[stale]).unwrap();
                assert!(!target.view().trusted().contains(&b.key()));
            }
        }
        let export = target.export();
        for d in [&mut a, &mut b] {
            d.log().sync(&export).unwrap();
        }
        for d in [&a, &b] {
            assert!(!d.log.as_ref().unwrap().view().trusted().contains(&b.key()));
            assert_eq!(d.log.as_ref().unwrap().accept_endpoint(&b.key(), None).unwrap_err().code, "untrusted_endpoint");
        }
    }

    #[test]
    fn what_a_revoked_key_adds_on_a_branch_its_revoker_never_saw_counts_for_nothing() {
        let (mut a, mut b, mut c) = machines();
        let past = b.log().frontier();
        let (x, y) = (Device::new("x"), Device::new("y"));
        let stale = b.log.as_ref().unwrap().entry(&b.secret, "add", x.descriptor(), Some(past), 0);
        let mut forked = b.log.clone().unwrap();
        forked.merge(std::slice::from_ref(&stale)).unwrap();
        let laundered = forked.entry(&x.secret, "add", y.descriptor(), None, 0);
        c.revoke(&b.key(), 0);
        let export = c.log().export();
        a.log().sync(&export).unwrap();
        a.log().merge(&[stale, laundered]).unwrap();
        let trusted = a.log().view().trusted();
        assert!(!trusted.contains(&x.key()) && !trusted.contains(&y.key()), "{trusted:?}");
        assert!(trusted.contains(&c.key()));
        assert_eq!(a.log().accept_endpoint(&y.key(), None).unwrap_err().code, "untrusted_endpoint");
    }

    #[test]
    fn an_offline_revocation_takes_effect_when_delivered() {
        let (mut a, b, mut c) = machines();
        a.revoke(&b.key(), 0);
        assert!(c.log().view().trusted().contains(&b.key()));
        let export = a.log().export();
        c.log().sync(&export).unwrap();
        assert!(!c.log().view().trusted().contains(&b.key()));
    }

    #[test]
    fn missing_history_never_authorizes() {
        let (mut a, mut b, _) = machines();
        let revoke = b.revoke(&a.key(), 0);
        let mut log = PeerLog::new(a.log().anchor().clone());
        log.merge(&[revoke]).unwrap();
        assert_eq!(log.pending(), 1);
        assert!(log.view().revoked.is_empty());
        log.sync(&b.log().export()).unwrap();
        assert_eq!(log.pending(), 0);
        assert_eq!(log.view().revoked, [a.key()].into());
    }

    #[test]
    fn a_certificate_is_bound_to_its_endpoint_and_dies_with_its_device() {
        let (_, b, mut c) = machines();
        let app = Device::new("app");
        let cert = certificate(&b.secret, &app.key());
        c.log().accept_endpoint(&app.key(), Some(&cert)).unwrap();
        assert_eq!(c.log().accept_endpoint(&Device::new("impostor").key(), Some(&cert)).unwrap_err().code, "untrusted_endpoint");
        c.revoke(&b.key(), 0);
        assert_eq!(c.log().accept_endpoint(&app.key(), Some(&cert)).unwrap_err().code, "untrusted_endpoint");
    }

    #[test]
    fn a_certificate_cannot_launder_a_revoked_key() {
        let (a, b, mut c) = machines();
        c.revoke(&b.key(), 0);
        let cert = certificate(&a.secret, &b.key());
        assert_eq!(c.log().accept_endpoint(&b.key(), Some(&cert)).unwrap_err().code, "untrusted_endpoint");
    }

    #[test]
    fn concurrent_adds_of_one_name_fail_closed_in_both_orders() {
        let (a, b, c) = machines();
        let (x, y) = (Device::new("duplicate"), Device::new("duplicate"));
        let (left, right) = (a.entry("add", x.descriptor()), b.entry("add", y.descriptor()));
        for order in [[left.clone(), right.clone()], [right, left]] {
            let mut log = c.log.clone().unwrap();
            for entry in &order {
                log.merge(std::slice::from_ref(entry)).unwrap();
            }
            for key in [x.key(), y.key()] {
                assert!(!log.view().trusted().contains(&key));
                assert_eq!(log.view().resolve("duplicate", &key).unwrap_err().code, "identity_mismatch");
            }
        }
    }

    #[test]
    fn a_reload_verifies_every_signature_and_keeps_tombstones() {
        let (mut a, b, mut c) = machines();
        c.revoke(&b.key(), 0);
        let saved = c.log().export();
        let mut loaded = PeerLog::new(saved["anchor"].clone());
        loaded.sync(&saved).unwrap();
        assert_eq!(loaded.view(), c.log().view());
        loaded.sync(&a.log().export()).unwrap();
        assert!(!loaded.view().trusted().contains(&b.key()));
        let mut forged = saved.clone();
        forged["entries"][0]["signature"] = json!("00".repeat(64));
        assert_eq!(code(PeerLog::new(saved["anchor"].clone()).sync(&forged)), "bad_signature");
    }
}
