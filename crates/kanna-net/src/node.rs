use crate::log::PeerLog;
use crate::pair::{hello_a, hello_b, pake_bytes, pake_hex, Offer, Party, Role};
use crate::{is_key, Refused, ALPN, KINDS};
use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::StreamExt;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::{oneshot, watch};

pub const DEFAULT_PORT: u16 = 7470;

const SERVICE: &str = "kanna";

const PAIRING: &str = "pair";

const MAX_HEADER: u64 = 64 << 10;

const MAX_LINE: u64 = 16 << 20;

const DIAL: Duration = Duration::from_secs(10);

const PAIRING_SESSION: Duration = Duration::from_secs(180);

const LOOKING: Duration = Duration::from_secs(6);

#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

impl Failure {
    pub fn new(code: &str, message: impl Into<String>) -> Failure {
        Failure { code: code.into(), message: message.into() }
    }

    fn from_reply(reply: &Value) -> Failure {
        let error = &reply["error"];
        Failure::new(error["code"].as_str().unwrap_or("failed"), error["message"].as_str().unwrap_or("failed"))
    }
}

impl From<Refused> for Failure {
    fn from(refused: Refused) -> Failure {
        Failure::new(refused.code, refused.message())
    }
}

fn refusal(failure: &Failure) -> Value {
    json!({ "ok": false, "error": { "code": failure.code, "message": failure.message } })
}

pub trait Host: Send + Sync + 'static {
    fn request(&self, from: String, request: Value, reply: Box<dyn FnOnce(Value) + Send>);
    fn confirm(&self, question: Value, answer: Box<dyn FnOnce(Result<bool, String>) + Send>);
    fn save(&self, state: Value);
}

pub struct Config {
    pub secret: SecretKey,
    pub saved: Value,
    pub name: String,
    pub capabilities: Value,
    pub port: Option<u16>,
    pub mdns: bool,
}

#[derive(Clone)]
pub struct Node(Arc<Inner>);

struct Inner {
    endpoint: Endpoint,
    secret: SecretKey,
    host: Box<dyn Host>,
    capabilities: Value,
    state: Mutex<State>,
    outcome: watch::Sender<Option<Result<Value, Failure>>>,
}

struct State {
    name: String,
    port: u16,
    log: Option<PeerLog>,
    broken: Option<String>,
    addrs: BTreeMap<String, Vec<String>>,
    offer: Option<Offer>,
    nearby: BTreeMap<String, Nearby>,
    conns: HashMap<String, Connection>,
}

#[derive(Clone, Default)]
struct Nearby {
    addrs: Vec<SocketAddr>,
    pairing: bool,
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32 && !is_key(name) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

async fn bind(secret: &SecretKey, port: u16) -> Result<Endpoint, String> {
    let builder = || Endpoint::builder(presets::Minimal).secret_key(secret.clone()).alpns(vec![ALPN.to_vec()]).relay_mode(RelayMode::Disabled);
    if port != 0 {
        if let Ok(fixed) = builder().bind_addr(SocketAddr::from(([0, 0, 0, 0], port))) {
            if let Ok(endpoint) = fixed.bind().await {
                return Ok(endpoint);
            }
        }
    }
    builder().bind().await.map_err(|e| format!("can't open the fleet endpoint: {e}"))
}

impl Node {
    pub async fn start(config: Config, host: Box<dyn Host>) -> Result<Node, String> {
        let saved = &config.saved;
        let wanted = config.port.or_else(|| saved["port"].as_u64().map(|p| p as u16)).unwrap_or(DEFAULT_PORT);
        let endpoint = bind(&config.secret, wanted).await?;
        let port = endpoint.bound_sockets().iter().find(|a| a.is_ipv4()).or(endpoint.bound_sockets().first()).map_or(0, |a| a.port());
        let (log, broken) = match saved.get("anchor").filter(|a| a.is_object()) {
            None => (None, None),
            Some(anchor) => {
                let mut log = PeerLog::new(anchor.clone());
                match log.merge(saved["entries"].as_array().map(Vec::as_slice).unwrap_or_default()) {
                    Ok(_) => (Some(log), None),
                    Err(refused) => (None, Some(format!("the peer log in kanna.db doesn't verify ({}: {})", refused.code, refused.message()))),
                }
            }
        };
        let name = saved["name"].as_str().filter(|n| valid_name(n)).map(String::from).unwrap_or(config.name);
        let addrs = saved["addrs"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), strings(v))).collect()).unwrap_or_default();
        let state = State { name, port, log, broken, addrs, offer: None, nearby: BTreeMap::new(), conns: HashMap::new() };
        let node = Node(Arc::new(Inner { endpoint, secret: config.secret, host, capabilities: config.capabilities, state: Mutex::new(state), outcome: watch::channel(None).0 }));
        if config.mdns {
            node.discover();
        }
        node.save();
        let accepting = node.clone();
        tokio::spawn(async move { accepting.accept().await });
        Ok(node)
    }

    pub fn key(&self) -> String {
        self.0.endpoint.id().to_string()
    }

    pub fn broken(&self) -> Option<String> {
        self.state().broken.clone()
    }

    pub fn port(&self) -> u16 {
        self.state().port
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub async fn close(&self) {
        self.0.endpoint.close().await;
    }

    fn discover(&self) {
        let mdns = match MdnsAddressLookup::builder().service_name(SERVICE).build(self.0.endpoint.id()) {
            Ok(mdns) => mdns,
            Err(_) => return,
        };
        if let Ok(lookup) = self.0.endpoint.address_lookup() {
            lookup.add(mdns.clone());
        }
        let node = self.clone();
        tokio::spawn(async move {
            let mut events = mdns.subscribe().await;
            while let Some(event) = events.next().await {
                let mut state = node.state();
                match event {
                    DiscoveryEvent::Discovered { endpoint_info, .. } => {
                        let pairing = endpoint_info.user_data().is_some_and(|d| d.as_ref() == PAIRING);
                        state.nearby.insert(endpoint_info.endpoint_id.to_string(), Nearby { addrs: endpoint_info.ip_addrs().cloned().collect(), pairing });
                    }
                    DiscoveryEvent::Expired { endpoint_id } => {
                        state.nearby.remove(&endpoint_id.to_string());
                    }
                    _ => {}
                }
            }
        });
    }

    fn snapshot(&self) -> Value {
        let state = self.state();
        let mut saved = json!({ "name": state.name, "port": state.port, "addrs": state.addrs });
        if let Some(log) = &state.log {
            saved["anchor"] = log.anchor().clone();
            saved["entries"] = json!(log.entries());
        }
        saved
    }

    fn save(&self) {
        if self.state().broken.is_some() {
            return;
        }
        self.0.host.save(self.snapshot());
    }

    fn descriptor(&self) -> Value {
        let addrs: Vec<String> = self.0.endpoint.addr().ip_addrs().map(|a| a.to_string()).collect();
        json!({ "name": self.state().name, "key": self.key(), "capabilities": self.0.capabilities, "relay": null, "addrs": addrs })
    }

    fn usable(&self) -> Result<(), Failure> {
        match self.broken() {
            Some(reason) => Err(Failure::new("broken", reason)),
            None => Ok(()),
        }
    }

    pub fn status(&self) -> Value {
        let state = self.state();
        let me = self.key();
        let mut peers = vec![];
        if let Some(log) = &state.log {
            let view = log.view();
            for (name, keys) in &view.names {
                for key in keys {
                    let conn = state.conns.get(key).filter(|c| c.close_reason().is_none());
                    let path = conn.and_then(|c| c.paths().iter().find(|p| p.is_selected()).map(|p| if p.is_ip() { "direct" } else { "relay" }));
                    let mut addrs = strings(&view.descriptors.get(key).map(|d| d["addrs"].clone()).unwrap_or_default());
                    addrs.extend(state.addrs.get(key).cloned().unwrap_or_default());
                    peers.push(json!({
                        "name": name,
                        "key": key,
                        "me": *key == me,
                        "trusted": view.trusted().contains(key),
                        "revoked": view.revoked.contains(key),
                        "conflict": keys.len() > 1,
                        "path": path,
                        "addrs": addrs,
                    }));
                }
            }
        }
        let nearby: Vec<Value> = state.nearby.iter().map(|(key, n)| json!({ "key": key, "addrs": n.addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>(), "pairing": n.pairing })).collect();
        let offer = state.offer.as_ref().filter(|o| !o.used && Instant::now() < o.expires).map(|o| json!({ "code": o.code, "expires_in": o.expires.saturating_duration_since(Instant::now()).as_secs() }));
        let addrs: Vec<String> = self.0.endpoint.addr().ip_addrs().map(|a| a.to_string()).collect();
        json!({
            "name": state.name,
            "key": me,
            "port": state.port,
            "addrs": addrs,
            "paired": state.log.is_some(),
            "fleet": state.log.as_ref().map(|l| l.fleet().to_string()),
            "broken": state.broken,
            "peers": peers,
            "nearby": nearby,
            "offer": offer,
        })
    }

    pub fn rename(&self, name: &str) -> Result<Value, Failure> {
        if !valid_name(name) {
            return Err(Failure::new("bad_request", format!("\"{name}\" isn't a machine name: use up to 32 lowercase letters, digits and -")));
        }
        {
            let mut state = self.state();
            let me = self.key();
            if state.log.as_ref().is_some_and(|log| log.view().descriptors.contains_key(&me)) {
                return Err(Failure::new("paired", "this machine is paired under its name already; renaming a paired machine isn't supported yet"));
            }
            state.name = name.to_string();
        }
        self.save();
        Ok(json!({ "name": name }))
    }

    pub fn offer(&self) -> Result<Value, Failure> {
        self.usable()?;
        let mut state = self.state();
        if state.log.is_some() {
            return Err(Refused::new("already_paired").into());
        }
        let offer = Offer::new(Instant::now());
        let reply = json!({ "code": offer.code, "key": self.key(), "port": state.port, "expires_in": offer.expires.saturating_duration_since(Instant::now()).as_secs(), "name": state.name });
        state.offer = Some(offer);
        drop(state);
        self.0.outcome.send_replace(None);
        self.0.endpoint.set_user_data_for_address_lookup(PAIRING.parse().ok());
        Ok(reply)
    }

    pub async fn wait_offer(&self) -> Result<Value, Failure> {
        let deadline = match &self.state().offer {
            Some(offer) => offer.expires,
            None => return Err(Failure::new("no_offer", "no pairing is open here: run kanna fleet pair")),
        };
        let mut outcome = self.0.outcome.subscribe();
        let result = tokio::time::timeout_at(deadline.into(), outcome.wait_for(Option::is_some)).await;
        let outcome = match result {
            Ok(Ok(value)) => value.clone().unwrap_or_else(|| Err(Refused::new("code_expired").into())),
            _ => Err(Refused::new("code_expired").into()),
        };
        self.close_offer();
        outcome
    }

    fn close_offer(&self) {
        self.state().offer = None;
        self.0.endpoint.set_user_data_for_address_lookup(None);
    }

    fn settle(&self, outcome: Result<Value, Failure>) {
        self.0.outcome.send_replace(Some(outcome));
    }

    pub fn addr(&self, host: &str, addr: &str) -> Result<Value, Failure> {
        let key = self.resolve(host)?;
        let addr = if addr.parse::<SocketAddr>().is_ok() || addr.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok() && !addr.ends_with(']')) { addr.to_string() } else { format!("{addr}:{DEFAULT_PORT}") };
        let mut state = self.state();
        let known = state.addrs.entry(key.clone()).or_default();
        if !known.contains(&addr) {
            known.push(addr.clone());
        }
        drop(state);
        self.save();
        Ok(json!({ "key": key, "addr": addr }))
    }

    fn resolve(&self, host: &str) -> Result<String, Failure> {
        if is_key(host) {
            return Ok(host.to_string());
        }
        let state = self.state();
        let view = state.log.as_ref().map(|l| l.view());
        match view.as_ref().and_then(|v| v.names.get(host)) {
            Some(keys) if keys.len() == 1 => Ok(keys.iter().next().cloned().unwrap_or_default()),
            Some(_) => Err(Refused::new("identity_mismatch").into()),
            None => Err(Failure::new("not_found", format!("no machine \"{host}\" in your fleet (kanna fleet lists them)"))),
        }
    }

    fn name_of(&self, key: &str) -> String {
        self.state().log.as_ref().and_then(|l| l.view().name_of(key)).unwrap_or_else(|| key.chars().take(10).collect())
    }

    fn admit(&self, key: &str) -> Result<String, Failure> {
        let state = self.state();
        let Some(log) = &state.log else { return Err(Failure::new("unpaired", "this machine isn't paired with yours: pair them with kanna fleet pair")) };
        let view = log.view();
        if view.revoked.contains(key) {
            return Err(Failure::new("revoked", "this machine's key is revoked in the fleet"));
        }
        if !view.trusted().contains(key) {
            return Err(Failure::new("unpaired", "this machine isn't paired with yours: pair them with kanna fleet pair"));
        }
        Ok(view.name_of(key).unwrap_or_default())
    }

    async fn accept(&self) {
        while let Some(incoming) = self.0.endpoint.accept().await {
            let node = self.clone();
            tokio::spawn(async move {
                let Ok(conn) = incoming.await else { return };
                let remote = conn.remote_id().to_string();
                while let Ok((send, recv)) = conn.accept_bi().await {
                    let (node, remote) = (node.clone(), remote.clone());
                    tokio::spawn(async move { node.serve(remote, send, recv).await });
                }
            });
        }
    }

    async fn serve(&self, remote: String, mut send: SendStream, recv: RecvStream) {
        let mut recv = BufReader::new(recv);
        let Ok(header) = read_line(&mut recv, MAX_HEADER).await else { return };
        let kind = header["kind"].as_str().unwrap_or_default().to_string();
        if kind == "pair" {
            let _ = tokio::time::timeout(PAIRING_SESSION, self.pair_b(&remote, &header, &mut send, &mut recv)).await;
            let _ = send.finish();
            return;
        }
        if !KINDS.contains(&kind.as_str()) {
            let _ = write_line(&mut send, &refusal(&Failure::new("unknown_stream", format!("no stream kind \"{kind}\"")))).await;
            let _ = send.finish();
            return;
        }
        let from = match self.admit(&remote) {
            Ok(name) => name,
            Err(failure) => {
                let _ = write_line(&mut send, &refusal(&failure)).await;
                let _ = send.finish();
                return;
            }
        };
        if write_line(&mut send, &json!({ "ok": true, "kind": kind, "name": self.state().name })).await.is_err() {
            return;
        }
        if kind == "control" {
            while let Ok(request) = read_line(&mut recv, MAX_LINE).await {
                let reply = self.control(&from, request).await;
                if write_line(&mut send, &reply).await.is_err() {
                    return;
                }
            }
        }
        let _ = send.finish();
    }

    async fn control(&self, from: &str, request: Value) -> Value {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        if request["cmd"] == "fleet.sync" {
            return match self.merge(&request["args"]["log"]) {
                Ok(()) => json!({ "id": id, "ok": true, "log": self.state().log.as_ref().map(PeerLog::export) }),
                Err(failure) => json!({ "id": id, "ok": false, "error": { "code": failure.code, "message": failure.message } }),
            };
        }
        let (tx, rx) = oneshot::channel();
        self.0.host.request(from.to_string(), request, Box::new(move |reply| {
            let _ = tx.send(reply);
        }));
        rx.await.unwrap_or_else(|_| json!({ "id": id, "ok": false, "error": { "code": "failed", "message": "the daemon dropped the request" } }))
    }

    fn merge(&self, envelope: &Value) -> Result<(), Failure> {
        if !envelope.is_object() {
            return Ok(());
        }
        let added = {
            let mut state = self.state();
            let Some(log) = state.log.as_mut() else { return Ok(()) };
            log.sync(envelope)?
        };
        if added > 0 {
            self.drop_revoked();
            self.save();
        }
        Ok(())
    }

    fn drop_revoked(&self) {
        let mut state = self.state();
        let revoked = state.log.as_ref().map(|l| l.view().revoked).unwrap_or_default();
        let gone: Vec<String> = state.conns.keys().filter(|k| revoked.contains(*k)).cloned().collect();
        for key in gone {
            if let Some(conn) = state.conns.remove(&key) {
                conn.close(1u32.into(), b"revoked");
            }
        }
    }

    fn endpoint_addr(&self, key: &str) -> Result<EndpointAddr, Failure> {
        let id: EndpointId = key.parse().map_err(|_| Failure::new("bad_request", format!("{key} isn't a machine's key")))?;
        let state = self.state();
        let mut addrs: Vec<String> = state.log.as_ref().and_then(|l| l.view().descriptors.get(key).map(|d| strings(&d["addrs"]))).unwrap_or_default();
        addrs.extend(state.addrs.get(key).cloned().unwrap_or_default());
        let mut direct: Vec<SocketAddr> = addrs.iter().filter_map(|a| a.parse().ok()).collect();
        direct.extend(state.nearby.get(key).map(|n| n.addrs.clone()).unwrap_or_default());
        let mut addr = EndpointAddr::new(id);
        for socket in direct {
            addr = addr.with_ip_addr(socket);
        }
        Ok(addr)
    }

    async fn resolve_hosts(&self, key: &str) -> Vec<SocketAddr> {
        let named: Vec<String> = self.state().addrs.get(key).cloned().unwrap_or_default().into_iter().filter(|a| a.parse::<SocketAddr>().is_err()).collect();
        let mut found = vec![];
        for host in named {
            if let Ok(Ok(addrs)) = tokio::time::timeout(Duration::from_secs(3), tokio::net::lookup_host(host)).await {
                found.extend(addrs);
            }
        }
        found
    }

    async fn connect(&self, key: &str) -> Result<(Connection, bool), Failure> {
        if let Some(conn) = self.state().conns.get(key).filter(|c| c.close_reason().is_none()).cloned() {
            return Ok((conn, false));
        }
        let mut addr = self.endpoint_addr(key)?;
        for socket in self.resolve_hosts(key).await {
            addr = addr.with_ip_addr(socket);
        }
        let name = self.name_of(key);
        let conn = match tokio::time::timeout(DIAL, self.0.endpoint.connect(addr, ALPN)).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => return Err(Failure::new("unreachable", format!("can't reach {name}: {e}"))),
            Err(_) => return Err(Failure::new("unreachable", format!("can't reach {name} within {}s (is its daemon running, and on this LAN? kanna fleet addr {name} HOST gives an address)", DIAL.as_secs()))),
        };
        self.state().conns.insert(key.to_string(), conn.clone());
        Ok((conn, true))
    }

    async fn open(&self, conn: &Connection, kind: &str, name: &str) -> Result<(SendStream, BufReader<RecvStream>, Value), Failure> {
        let lost = |e: String| Failure::new("unreachable", format!("lost {name}: {e}"));
        let (mut send, recv) = conn.open_bi().await.map_err(|e| lost(e.to_string()))?;
        let mut recv = BufReader::new(recv);
        write_line(&mut send, &json!({ "kind": kind })).await.map_err(lost)?;
        let ack = read_line(&mut recv, MAX_HEADER).await.map_err(lost)?;
        if ack["ok"] != true {
            let failure = Failure::from_reply(&ack);
            return Err(Failure::new(&failure.code, format!("{name} refused this machine: {}", failure.message)));
        }
        Ok((send, recv, ack))
    }

    async fn control_stream(&self, host: &str) -> Result<(SendStream, BufReader<RecvStream>, String), Failure> {
        self.usable()?;
        let key = self.resolve(host)?;
        if key == self.key() {
            return Err(Failure::new("bad_request", format!("{host} is this machine")));
        }
        let name = self.name_of(&key);
        let (conn, fresh) = self.connect(&key).await?;
        let (mut send, mut recv, _) = match self.open(&conn, "control", &name).await {
            Ok(opened) => opened,
            Err(failure) => {
                self.state().conns.remove(&key);
                return Err(failure);
            }
        };
        if fresh {
            self.sync_over(&mut send, &mut recv).await;
        }
        Ok((send, recv, name))
    }

    async fn sync_over(&self, send: &mut SendStream, recv: &mut BufReader<RecvStream>) {
        let Some(log) = self.state().log.as_ref().map(PeerLog::export) else { return };
        if write_line(send, &json!({ "id": 0, "cmd": "fleet.sync", "args": { "log": log } })).await.is_err() {
            return;
        }
        if let Ok(reply) = read_line(recv, MAX_LINE).await {
            let _ = self.merge(&reply["log"]);
        }
    }

    pub async fn call(&self, host: &str, request: Value) -> Result<Value, Failure> {
        let (mut send, mut recv, name) = self.control_stream(host).await?;
        let lost = |e: String| Failure::new("unreachable", format!("lost {name}: {e}"));
        write_line(&mut send, &request).await.map_err(lost)?;
        let reply = read_line(&mut recv, MAX_LINE).await.map_err(lost)?;
        let _ = send.finish();
        Ok(reply)
    }

    pub async fn sync(&self, host: &str) -> Result<Value, Failure> {
        let (mut send, mut recv, name) = self.control_stream(host).await?;
        self.sync_over(&mut send, &mut recv).await;
        let _ = send.finish();
        Ok(json!({ "name": name }))
    }

    pub async fn ping(&self, host: &str) -> Result<Value, Failure> {
        self.usable()?;
        let key = self.resolve(host)?;
        let name = self.name_of(&key);
        let started = Instant::now();
        let (conn, _) = self.connect(&key).await?;
        let mut streams = serde_json::Map::new();
        for kind in KINDS {
            let opened = Instant::now();
            let result = match self.open(&conn, kind, &name).await {
                Ok((mut send, _, _)) => {
                    let _ = send.finish();
                    json!({ "ok": true, "ms": opened.elapsed().as_secs_f64() * 1000.0 })
                }
                Err(failure) => json!({ "ok": false, "error": { "code": failure.code, "message": failure.message } }),
            };
            streams.insert(kind.to_string(), result);
        }
        let selected = conn.paths().iter().find(|p| p.is_selected()).map(|p| (if p.is_ip() { "direct" } else { "relay" }, p.rtt().as_secs_f64() * 1000.0, p.remote_addr().to_string()));
        Ok(json!({
            "name": name,
            "key": key,
            "path": selected.as_ref().map(|s| s.0),
            "rtt_ms": selected.as_ref().map(|s| s.1),
            "remote": selected.as_ref().map(|s| s.2.clone()),
            "ms": started.elapsed().as_secs_f64() * 1000.0,
            "streams": streams,
        }))
    }

    pub async fn revoke(&self, host: &str) -> Result<Value, Failure> {
        self.usable()?;
        let key = self.resolve(host)?;
        if key == self.key() {
            return Err(Failure::new("bad_request", "this machine can't revoke itself: revoke it from another machine in the fleet"));
        }
        let name = {
            let mut state = self.state();
            let Some(log) = state.log.as_mut() else { return Err(Failure::new("unpaired", "this machine has no fleet")) };
            let name = log.view().name_of(&key).ok_or_else(|| Failure::new("not_found", format!("{host} isn't in your fleet")))?;
            let entry = log.entry(&self.0.secret, "revoke", json!({ "key": key }), None, unix_time());
            log.merge(&[entry])?;
            name
        };
        self.drop_revoked();
        self.save();
        let others: Vec<String> = self.state().log.as_ref().map(|l| l.view().trusted().into_iter().filter(|k| *k != self.key()).collect()).unwrap_or_default();
        let mut told = vec![];
        for other in others {
            if self.sync(&other).await.is_ok() {
                told.push(self.name_of(&other));
            }
        }
        Ok(json!({ "name": name, "key": key, "told": told }))
    }

    async fn pairing_targets(&self, peer: Option<&str>) -> Result<Vec<EndpointAddr>, Failure> {
        if let Some(peer) = peer {
            let (key, at) = match peer.split_once('@') {
                Some((key, at)) => (key, Some(at)),
                None => (peer, None),
            };
            let id: EndpointId = key.parse().map_err(|_| Failure::new("bad_request", format!("{key} isn't a machine's key")))?;
            let mut addr = EndpointAddr::new(id);
            if let Some(at) = at {
                let at = if at.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) { at.to_string() } else { format!("{at}:{DEFAULT_PORT}") };
                let found = tokio::net::lookup_host(at.clone()).await.map_err(|e| Failure::new("not_found", format!("can't resolve {at}: {e}")))?;
                for socket in found {
                    addr = addr.with_ip_addr(socket);
                }
            }
            return Ok(vec![addr]);
        }
        let deadline = Instant::now() + LOOKING;
        loop {
            let found: Vec<EndpointAddr> = self
                .state()
                .nearby
                .iter()
                .filter(|(_, n)| n.pairing)
                .filter_map(|(key, n)| {
                    let id: EndpointId = key.parse().ok()?;
                    Some(n.addrs.iter().fold(EndpointAddr::new(id), |a, s| a.with_ip_addr(*s)))
                })
                .collect();
            if !found.is_empty() || Instant::now() >= deadline {
                return if found.is_empty() {
                    Err(Failure::new("not_found", "no machine on this LAN is waiting to pair: run kanna fleet pair on it first (or give its --peer KEY@HOST, which it prints)"))
                } else {
                    Ok(found)
                };
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    pub async fn pair(&self, code: &str, peer: Option<&str>) -> Result<Value, Failure> {
        self.usable()?;
        let targets = self.pairing_targets(peer).await?;
        let mut last = Failure::from(Refused::new("pake_confirmation_failed"));
        for target in targets {
            match tokio::time::timeout(PAIRING_SESSION, self.pair_a(code, target)).await {
                Ok(Ok(paired)) => return Ok(paired),
                Ok(Err(failure)) if failure.code == "pake_confirmation_failed" || failure.code == "unreachable" => last = failure,
                Ok(Err(failure)) => return Err(failure),
                Err(_) => return Err(Failure::new("timeout", "pairing took too long")),
            }
        }
        Err(last)
    }

    async fn pair_a(&self, code: &str, target: EndpointAddr) -> Result<Value, Failure> {
        let key = target.id.to_string();
        let mine = self.descriptor();
        let staged = self.state().log.clone().unwrap_or_else(|| PeerLog::new(mine.clone()));
        let anchor = staged.anchor().clone();
        let conn = match tokio::time::timeout(DIAL, self.0.endpoint.connect(target, ALPN)).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => return Err(Failure::new("unreachable", format!("can't reach the machine waiting to pair: {e}"))),
            Err(_) => return Err(Failure::new("unreachable", "can't reach the machine waiting to pair")),
        };
        let lost = |e: String| Failure::new("unreachable", format!("lost the machine waiting to pair: {e}"));
        let (mut send, recv) = conn.open_bi().await.map_err(|e| lost(e.to_string()))?;
        let mut recv = BufReader::new(recv);
        let ha = hello_a(&mine, &anchor);
        write_line(&mut send, &json!({ "kind": "pair", "hello": ha })).await.map_err(lost)?;
        let reply = read_line(&mut recv, MAX_HEADER).await.map_err(lost)?;
        if reply["ok"] != true {
            return Err(Failure::from_reply(&reply));
        }
        let hb = reply["hello"].clone();
        if hb["device"]["key"] != key.as_str() {
            return Err(Refused::new("pake_confirmation_failed").into());
        }
        let mut party = Party::new(Role::A, code, ha, hb.clone());
        let proof = party.finish(&self.0.secret, &pake_bytes(&reply["pake"])?)?;
        write_line(&mut send, &json!({ "pake": pake_hex(&party.outbound), "proof": proof })).await.map_err(lost)?;
        let reply = read_line(&mut recv, MAX_LINE).await.map_err(lost)?;
        if reply["ok"] != true {
            return Err(Failure::from_reply(&reply));
        }
        party.confirm(&reply["proof"])?;
        let device = hb["device"].clone();
        let name = device["name"].as_str().unwrap_or_default().to_string();
        let mut staged = staged;
        let entry = staged.entry(&self.0.secret, "add", device.clone(), None, unix_time());
        if let Err(refused) = staged.merge(&[entry]) {
            return refuse(&mut send, refused.into()).await;
        }
        let (tx, rx) = oneshot::channel();
        let question = json!({ "name": name, "key": key, "addrs": device["addrs"], "capabilities": device["capabilities"], "inviter": mine["name"] });
        self.0.host.confirm(question, Box::new(move |answer| {
            let _ = tx.send(answer);
        }));
        let answer = match rx.await {
            Ok(Ok(true)) => None,
            Ok(Ok(false)) => Some(format!("adding {name} wasn't confirmed in Kanna.app")),
            Ok(Err(reason)) => Some(format!("adding {name} needs a yes in Kanna.app: {reason}")),
            Err(_) => Some("Kanna.app didn't answer".to_string()),
        };
        if let Some(reason) = answer {
            return refuse(&mut send, Failure::new("gesture_required", reason)).await;
        }
        write_line(&mut send, &json!({ "handoff": party.seal(&staged.export()) })).await.map_err(lost)?;
        let done = read_line(&mut recv, MAX_LINE).await.map_err(lost)?;
        let _ = send.finish();
        if done["ok"] != true {
            return Err(Failure::from_reply(&done));
        }
        {
            let mut state = self.state();
            match state.log.as_mut() {
                Some(log) => {
                    log.merge(&staged.entries())?;
                }
                None => state.log = Some(staged),
            }
            state.conns.insert(key.clone(), conn);
        }
        self.save();
        let others: Vec<String> = self.state().log.as_ref().map(|l| l.view().trusted().into_iter().filter(|k| *k != self.key() && *k != key).collect()).unwrap_or_default();
        for other in others {
            let _ = self.sync(&other).await;
        }
        Ok(json!({ "name": name, "key": key }))
    }

    async fn pair_b(&self, remote: &str, header: &Value, send: &mut SendStream, recv: &mut BufReader<RecvStream>) {
        let ha = header["hello"].clone();
        let mine = self.descriptor();
        let started = {
            let mut state = self.state();
            let paired = state.log.is_some();
            match state.offer.as_mut() {
                None => Err(Failure::new("no_offer", "that machine isn't waiting to pair")),
                Some(_) if paired => Err(Refused::new("already_paired").into()),
                Some(_) if ha["device"]["key"] != remote => Err(Refused::new("pake_confirmation_failed").into()),
                Some(offer) => offer.begin(Instant::now()).map(|()| (offer.code.clone(), offer.session.clone())).map_err(Failure::from),
            }
        };
        let (code, session) = match started {
            Ok(started) => started,
            Err(failure) => {
                let _ = write_line(send, &refusal(&failure)).await;
                return;
            }
        };
        let hb = hello_b(&mine, &session);
        let mut party = Party::new(Role::B, &code, ha, hb.clone());
        if write_line(send, &json!({ "ok": true, "hello": hb, "pake": pake_hex(&party.outbound) })).await.is_err() {
            return;
        }
        let Ok(message) = read_line(recv, MAX_LINE).await else { return };
        let proof = match pake_bytes(&message["pake"]).and_then(|pake| party.finish(&self.0.secret, &pake)).and_then(|proof| party.confirm(&message["proof"]).map(|()| proof)) {
            Ok(proof) => proof,
            Err(refused) => {
                let _ = write_line(send, &refusal(&refused.into())).await;
                return;
            }
        };
        if let Some(offer) = self.state().offer.as_mut() {
            offer.used = true;
        }
        if write_line(send, &json!({ "ok": true, "proof": proof })).await.is_err() {
            return self.settle(Err(Failure::new("unreachable", "lost the machine pairing with this one")));
        }
        let message = match read_line(recv, MAX_LINE).await {
            Ok(message) => message,
            Err(e) => return self.settle(Err(Failure::new("unreachable", format!("lost the machine pairing with this one: {e}")))),
        };
        if message["ok"] == false {
            return self.settle(Err(Failure::from_reply(&message)));
        }
        let log = match party.open(&message["handoff"], &self.key()) {
            Ok(log) => log,
            Err(refused) => {
                let failure = Failure::from(refused);
                let _ = write_line(send, &refusal(&failure)).await;
                return self.settle(Err(failure));
            }
        };
        let inviter = party.hello_a["device"]["name"].as_str().unwrap_or_default().to_string();
        let fleet = log.fleet().to_string();
        self.state().log = Some(log);
        self.save();
        let _ = write_line(send, &json!({ "ok": true })).await;
        self.settle(Ok(json!({ "name": inviter, "key": remote, "fleet": fleet })));
    }
}

async fn refuse(send: &mut SendStream, failure: Failure) -> Result<Value, Failure> {
    let _ = write_line(send, &refusal(&failure)).await;
    let _ = send.finish();
    Err(failure)
}

fn unix_time() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

async fn read_line<R: tokio::io::AsyncBufRead + Unpin>(recv: &mut R, max: u64) -> Result<Value, String> {
    let mut line = Vec::new();
    let read = (&mut *recv).take(max).read_until(b'\n', &mut line).await.map_err(|e| e.to_string())?;
    if read == 0 {
        return Err("the stream ended".into());
    }
    if line.last() != Some(&b'\n') {
        return Err(format!("a line over {max} bytes"));
    }
    serde_json::from_slice(&line).map_err(|e| format!("a line that isn't JSON: {e}"))
}

async fn write_line(send: &mut SendStream, value: &Value) -> Result<(), String> {
    let mut line = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    line.push(b'\n');
    send.write_all(&line).await.map_err(|e| e.to_string())
}
