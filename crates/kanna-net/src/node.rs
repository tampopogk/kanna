use crate::log::PeerLog;
use crate::pair::{hello_a, hello_b, pake_bytes, pake_hex, Offer, Party, Role};
use crate::{bonjour, phone, protocol};
use crate::{is_key, relay_mode, relay_url, Refused, Relay, ALPN, KINDS};
use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::StreamExt;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, watch};

pub const DEFAULT_PORT: u16 = 7470;

const SERVICE: &str = "kanna";

const PAIRING: &str = "pair";

const APP_HELLO: &str = "app.hello";

pub const PEER: &str = "peer";

const REPLICA: &str = "replica";

pub(crate) const MAX_HEADER: u64 = 64 << 10;

pub(crate) const MAX_LINE: u64 = 16 << 20;

const MAX_TRANSFER: u64 = 4 << 30;

pub(crate) const DIAL: Duration = Duration::from_secs(10);

const PAIRING_SESSION: Duration = Duration::from_secs(180);

const LOOKING: Duration = Duration::from_secs(6);

const BEFORE_THE_CODE: &[&str] = &["unreachable", "pake_confirmation_failed", "bad_pake_message", "no_offer", "code_used", "code_expired", "rate_limited", "already_paired"];

#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

impl Failure {
    pub fn new(code: &str, message: impl Into<String>) -> Failure {
        Failure { code: code.into(), message: message.into() }
    }

    pub(crate) fn from_reply(reply: &Value) -> Failure {
        let error = &reply["error"];
        Failure::new(error["code"].as_str().unwrap_or("failed"), error["message"].as_str().unwrap_or("failed"))
    }
}

impl From<Refused> for Failure {
    fn from(refused: Refused) -> Failure {
        Failure::new(refused.code, refused.message())
    }
}

pub(crate) fn refusal(failure: &Failure) -> Value {
    json!({ "ok": false, "error": { "code": failure.code, "message": failure.message } })
}

pub trait Host: Send + Sync + 'static {
    fn request(&self, from: String, request: Value, reply: Box<dyn FnOnce(Value) + Send>);
    fn confirm(&self, question: Value, answer: Box<dyn FnOnce(Result<bool, String>) + Send>);
    fn save(&self, state: Value);
    fn app(&self, _from: String, _hello: Value, _stream: std::os::unix::net::UnixStream) {}
    fn inbox(&self) -> Option<std::path::PathBuf> {
        None
    }
    fn job(&self, _from: String, _stream: std::os::unix::net::UnixStream) {}
    fn phone(&self, _from: String, _key: String, _stream: std::os::unix::net::UnixStream) {}
}

pub struct Config {
    pub secret: SecretKey,
    pub saved: Value,
    pub name: String,
    pub capabilities: Value,
    pub port: Option<u16>,
    pub mdns: bool,
    pub release: Value,
}

#[derive(Clone)]
pub struct Node(pub(crate) Arc<Inner>);

pub(crate) struct Inner {
    endpoint: Endpoint,
    secret: SecretKey,
    pub(crate) host: Box<dyn Host>,
    capabilities: Value,
    pub(crate) release: Value,
    state: Mutex<State>,
    outcome: watch::Sender<Option<Result<Value, Failure>>>,
    revoked: watch::Sender<u64>,
    bonjour: Mutex<Option<bonjour::Published>>,
}

pub(crate) struct State {
    pub(crate) name: String,
    port: u16,
    relay: Option<Relay>,
    log: Option<PeerLog>,
    broken: Option<String>,
    addrs: BTreeMap<String, Vec<String>>,
    offer: Option<Offer>,
    phone_offer: Option<Offer>,
    nearby: BTreeMap<String, Nearby>,
    conns: HashMap<String, Connection>,
    pub(crate) releases: HashMap<String, Value>,
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
    let builder = || Endpoint::builder(presets::Minimal).secret_key(secret.clone()).alpns(vec![ALPN.to_vec()]).relay_mode(relay_mode());
    if port != 0 {
        if let Ok(fixed) = builder().bind_addr(SocketAddr::from(([0, 0, 0, 0], port))) {
            if let Ok(endpoint) = fixed.bind().await {
                return Ok(endpoint);
            }
        }
    }
    builder().bind().await.map_err(|e| format!("can't open the endpoint: {e}"))
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
        let relay = saved["relay"].as_str().and_then(|url| Relay::new(url, saved["relay_token"].as_str()).ok());
        if let Some(relay) = &relay {
            endpoint.insert_relay(relay.url.clone(), relay.config()).await;
        }
        let state = State { name, port, relay, log, broken, addrs, offer: None, phone_offer: None, nearby: BTreeMap::new(), conns: HashMap::new(), releases: HashMap::new() };
        let node = Node(Arc::new(Inner { endpoint, secret: config.secret, host, capabilities: config.capabilities, release: config.release, state: Mutex::new(state), outcome: watch::channel(None).0, revoked: watch::channel(0).0, bonjour: Mutex::new(None) }));
        if config.mdns {
            node.discover();
            let published = phone::instance_of(&node.key()).and_then(|name| bonjour::publish(&name, port));
            *node.0.bonjour.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = published;
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

    pub(crate) fn state(&self) -> std::sync::MutexGuard<'_, State> {
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

    fn snapshot(state: &State) -> Value {
        let mut saved = json!({ "name": state.name, "port": state.port, "addrs": state.addrs });
        if let Some(relay) = &state.relay {
            saved["relay"] = json!(relay.url.to_string());
            saved["relay_token"] = json!(relay.token);
        }
        if let Some(log) = &state.log {
            saved["anchor"] = log.anchor().clone();
            saved["entries"] = json!(log.entries());
        }
        saved
    }

    fn save(&self) {
        let state = self.state();
        if state.broken.is_none() {
            self.0.host.save(Node::snapshot(&state));
        }
    }

    fn descriptor(&self) -> Value {
        let addrs: Vec<String> = self.0.endpoint.addr().ip_addrs().map(|a| a.to_string()).collect();
        let state = self.state();
        json!({ "name": state.name, "key": self.key(), "capabilities": self.0.capabilities, "relay": state.relay.as_ref().map(|r| r.url.to_string()), "addrs": addrs })
    }

    pub(crate) fn usable(&self) -> Result<(), Failure> {
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
                for key in keys.iter().filter(|key| !view.is_phone(key)) {
                    let conn = state.conns.get(key).filter(|c| c.close_reason().is_none());
                    let selected = conn.and_then(|c| c.paths().iter().find(|p| p.is_selected()).map(|p| (p.is_ip(), p.rtt().as_secs_f64() * 1000.0, matches!(p.remote_addr(), iroh::TransportAddr::Ip(socket) if crate::client::local(socket.ip())))));
                    let path = selected.map(|(ip, _, _)| if ip { "direct" } else { "relay" });
                    let mut addrs: Vec<String> = conn.map(|c| c.paths().iter().filter_map(|p| match p.remote_addr() { iroh::TransportAddr::Ip(socket) => Some(socket.to_string()), _ => None }).collect()).unwrap_or_default();
                    addrs.extend(strings(&view.descriptors.get(key).map(|d| d["addrs"].clone()).unwrap_or_default()));
                    addrs.extend(state.addrs.get(key).cloned().unwrap_or_default());
                    let mut seen = std::collections::HashSet::new();
                    addrs.retain(|a| seen.insert(a.clone()));
                    let relay = state.relay.as_ref().map(|r| r.url.to_string()).or_else(|| view.descriptors.get(key).and_then(|d| d["relay"].as_str().map(String::from)));
                    peers.push(json!({
                        "name": name,
                        "key": key,
                        "me": *key == me,
                        "capabilities": view.descriptors.get(key).map(|d| d["capabilities"].clone()).unwrap_or(Value::Null),
                        "trusted": view.trusted().contains(key),
                        "revoked": view.revoked.contains(key),
                        "conflict": keys.len() > 1,
                        "path": path,
                        "lan": selected.is_some_and(|(ip, _, lan)| ip && lan),
                        "rtt_ms": selected.map(|(_, rtt, _)| rtt),
                        "addrs": addrs,
                        "relay": relay,
                        "release": state.releases.get(key).cloned().unwrap_or(if *key == me { self.0.release.clone() } else { Value::Null }),
                    }));
                }
            }
        }
        let phones: Vec<Value> = state.log.as_ref().map(|log| {
            let view = log.view();
            view.descriptors.iter().filter(|(key, _)| view.is_phone(key)).map(|(key, d)| json!({ "name": d["name"], "key": key, "revoked": view.revoked.contains(key) })).collect()
        }).unwrap_or_default();
        let nearby: Vec<Value> = state.nearby.iter().map(|(key, n)| json!({ "key": key, "addrs": n.addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>(), "pairing": n.pairing })).collect();
        let offer = state.offer.as_ref().filter(|o| !o.used && Instant::now() < o.expires).map(|o| json!({ "code": o.code, "expires_in": o.expires.saturating_duration_since(Instant::now()).as_secs() }));
        let addrs: Vec<String> = self.0.endpoint.addr().ip_addrs().map(|a| a.to_string()).collect();
        json!({
            "name": state.name,
            "key": me,
            "port": state.port,
            "release": self.0.release,
            "addrs": addrs,
            "relay": state.relay.as_ref().map(|r| r.url.to_string()),
            "relay_token": state.relay.as_ref().is_some_and(|r| r.token.is_some()),
            "home_relay": self.0.endpoint.addr().relay_urls().next().map(|r| r.to_string()),
            "paired": state.log.is_some(),
            "log": state.log.as_ref().map(|l| l.id().to_string()),
            "broken": state.broken,
            "peers": peers,
            "phones": phones,
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

    pub fn log(&self) -> Option<Value> {
        self.state().log.as_ref().map(PeerLog::export)
    }

    pub fn relay_token(&self) -> Option<String> {
        self.state().relay.as_ref().and_then(|r| r.token.clone())
    }

    pub async fn set_relay(&self, url: Option<&str>, token: Option<&str>) -> Result<Value, Failure> {
        let relay = url.map(|url| Relay::new(url, token)).transpose()?;
        let old = std::mem::replace(&mut self.state().relay, relay.clone());
        if let Some(old) = old.filter(|old| Some(&old.url) != relay.as_ref().map(|r| &r.url)) {
            self.0.endpoint.remove_relay(&old.url).await;
        }
        if let Some(relay) = &relay {
            self.0.endpoint.insert_relay(relay.url.clone(), relay.config()).await;
        }
        self.save();
        Ok(json!({ "relay": relay.as_ref().map(|r| r.url.to_string()), "token": relay.is_some_and(|r| r.token.is_some()) }))
    }

    pub fn offer(&self) -> Result<Value, Failure> {
        self.usable()?;
        let mut state = self.state();
        if state.log.is_some() {
            return Err(Refused::new("already_paired").into());
        }
        let offer = Offer::new(Instant::now());
        let addrs: Vec<String> = self.0.endpoint.addr().ip_addrs().map(|a| a.to_string()).collect();
        let reply = json!({ "code": offer.code, "key": self.key(), "port": state.port, "addrs": addrs, "expires_in": offer.expires.saturating_duration_since(Instant::now()).as_secs(), "name": state.name });
        state.offer = Some(offer);
        drop(state);
        self.0.outcome.send_replace(None);
        self.0.endpoint.set_user_data_for_address_lookup(PAIRING.parse().ok());
        Ok(reply)
    }

    pub fn offer_phone(&self) -> Result<Value, Failure> {
        self.usable()?;
        let addrs: Vec<String> = self.0.endpoint.addr().ip_addrs().map(|a| a.to_string()).collect();
        let offer = Offer::for_phone(Instant::now());
        let shown = json!({ "code": offer.code, "key": self.key(), "addrs": addrs, "port": self.port(), "expires_in": crate::pair::OFFER_LIFETIME.as_secs() });
        self.state().phone_offer = Some(offer);
        self.0.outcome.send_replace(None);
        Ok(shown)
    }

    pub async fn wait_phone(&self) -> Result<Value, Failure> {
        let mut outcome = self.0.outcome.subscribe();
        let result = match tokio::time::timeout(crate::pair::OFFER_LIFETIME, outcome.wait_for(Option::is_some)).await {
            Ok(Ok(result)) => result.clone().unwrap_or_else(|| Err(Failure::new("failed", "pairing ended"))),
            _ => Err(Refused::new("code_expired").into()),
        };
        self.state().phone_offer = None;
        result
    }

    pub async fn wait_offer(&self) -> Result<Value, Failure> {
        let deadline = match &self.state().offer {
            Some(offer) => offer.expires,
            None => return Err(Failure::new("no_offer", "no pairing is open here: run kanna machine pair")),
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

    pub(crate) fn resolve(&self, host: &str) -> Result<String, Failure> {
        if is_key(host) {
            return Ok(host.to_string());
        }
        let state = self.state();
        let view = state.log.as_ref().map(|l| l.view());
        match view.as_ref().and_then(|v| v.names.get(host)) {
            Some(keys) if keys.len() == 1 => Ok(keys.iter().next().cloned().unwrap_or_default()),
            Some(_) => Err(Refused::new("identity_mismatch").into()),
            None => Err(Failure::new("not_found", format!("no machine \"{host}\" among your machines (kanna machine list lists them)"))),
        }
    }

    pub(crate) fn forget(&self, key: &str) {
        self.state().conns.remove(key);
    }

    pub(crate) fn name_of(&self, key: &str) -> String {
        self.state().log.as_ref().and_then(|l| l.view().name_of(key)).unwrap_or_else(|| key.chars().take(10).collect())
    }

    fn admit(&self, key: &str, cert: Option<&Value>) -> Result<String, Failure> {
        let state = self.state();
        let unpaired = || Failure::new("unpaired", "it doesn't know this machine's key (pair the two with kanna machine pair)");
        let Some(log) = &state.log else { return Err(unpaired()) };
        let view = log.view();
        let device = cert.and_then(|c| c["body"]["device"].as_str()).unwrap_or(key);
        if view.revoked.contains(key) || view.revoked.contains(device) {
            return Err(Failure::new("revoked", "this machine's key is revoked from your machines"));
        }
        if log.accept_endpoint(key, cert).is_err() {
            return Err(unpaired());
        }
        Ok(view.name_of(device).unwrap_or_default())
    }

    pub fn certify(&self, app_key: &str) -> Result<Value, Failure> {
        self.usable()?;
        if !is_key(app_key) {
            return Err(Failure::new("bad_request", format!("{app_key} isn't an endpoint key")));
        }
        if self.state().log.is_none() {
            return Err(Failure::new("unpaired", "this machine isn't paired"));
        }
        Ok(crate::log::certificate(&self.0.secret, app_key))
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
        if kind == "phone" {
            return self.serve_phone(remote, header, send, recv).await;
        }
        if kind == "pair" {
            match header["joining"] == true {
                true => {
                    let _ = tokio::time::timeout(PAIRING_SESSION, self.join(&remote, &mut send, &mut recv)).await;
                }
                false => {
                    let _ = tokio::time::timeout(PAIRING_SESSION, self.pair_b(&remote, &header, &mut send, &mut recv)).await;
                }
            }
            let _ = send.finish();
            return;
        }
        if !KINDS.contains(&kind.as_str()) {
            let _ = write_line(&mut send, &refusal(&Failure::new("unknown_stream", format!("no stream kind \"{kind}\"")))).await;
            let _ = send.finish();
            return;
        }
        let cert = header.get("cert").filter(|c| c.is_object()).cloned();
        let mut revoked = self.0.revoked.subscribe();
        let from = match self.admit(&remote, cert.as_ref()) {
            Ok(name) => name,
            Err(failure) => {
                let _ = write_line(&mut send, &refusal(&failure)).await;
                let _ = send.finish();
                return;
            }
        };
        let mine = self.protocol();
        if cert.is_none() && protocol::of(&header) != Some(mine) {
            let refused = json!({ "ok": false, "error": { "code": "other_protocol", "message": format!("{} speaks protocol {mine}", self.state().name) }, "release": self.0.release });
            let _ = write_line(&mut send, &refused).await;
            let _ = send.finish();
            return;
        };
        let name = self.state().name.clone();
        let target = match header.get("port").filter(|_| kind == "port") {
            Some(port) => match loopback(port, &name).await {
                Ok(tcp) => Some(tcp),
                Err(failure) => {
                    let _ = write_line(&mut send, &refusal(&failure)).await;
                    let _ = send.finish();
                    return;
                }
            },
            None => None,
        };
        if write_line(&mut send, &json!({ "ok": true, "kind": kind, "name": name, "protocol": mine, "release": self.0.release })).await.is_err() {
            return;
        }
        if let Some(tcp) = target {
            let (node, cert) = (self.clone(), cert.clone());
            let watched = async move {
                revoked.mark_changed();
                while revoked.changed().await.is_ok() {
                    if node.admit(&remote, cert.as_ref()).is_err() {
                        return;
                    }
                }
                std::future::pending::<()>().await
            };
            tokio::select! {
                () = splice(tcp, send, recv) => {}
                () = watched => {}
            }
            return;
        }
        if kind == "sync" && cert.is_some() {
            let _ = write_line(&mut send, &refusal(&Failure::new("not_a_daemon", "only a machine's daemon syncs"))).await;
            let _ = send.finish();
            return;
        }
        if kind == "transfer" {
            let reply = match cert {
                Some(_) => refusal(&Failure::new("not_a_daemon", "only a machine's daemon sends a task")),
                None => self.take_transfer(&remote, &from, &mut recv).await,
            };
            let _ = write_line(&mut send, &reply).await;
            let _ = send.finish();
            return;
        }
        if kind == "job" {
            if cert.is_some() {
                let _ = write_line(&mut send, &refusal(&Failure::new("not_a_daemon", "only a machine's daemon sends a job"))).await;
                let _ = send.finish();
                return;
            }
            let Ok((ours, theirs)) = std::os::unix::net::UnixStream::pair() else { return };
            self.0.host.job(from, theirs);
            let (node, remote) = (self.clone(), remote.clone());
            let watched = async move {
                while revoked.changed().await.is_ok() {
                    if node.admit(&remote, None).is_err() {
                        return;
                    }
                }
                std::future::pending::<()>().await
            };
            tokio::select! {
                () = splice_unix(ours, send, recv) => {}
                () = watched => {}
            }
            return;
        }
        if kind == "control" || kind == "sync" {
            while let Ok(mut request) = read_line(&mut recv, MAX_LINE).await {
                if let Err(failure) = self.admit(&remote, cert.as_ref()) {
                    let id = request.get("id").cloned().unwrap_or(Value::Null);
                    let _ = write_line(&mut send, &json!({ "id": id, "ok": false, "error": { "code": failure.code, "message": failure.message } })).await;
                    break;
                }
                if kind == "control" && request["cmd"] == APP_HELLO {
                    return self.bridge(remote, cert, from, request, send, recv).await;
                }
                let replica = request["cmd"] == REPLICA;
                if kind == "sync" && !replica {
                    let id = request.get("id").cloned().unwrap_or(Value::Null);
                    if write_line(&mut send, &json!({ "id": id, "ok": false, "error": { "code": "bad_request", "message": "a sync stream carries only replica requests" } })).await.is_err() {
                        return;
                    }
                    continue;
                }
                if let Some(args) = request.get_mut("args").and_then(Value::as_object_mut) {
                    if kind == "sync" {
                        args.insert(PEER.into(), json!(remote));
                    } else if replica {
                        args.remove(PEER);
                    }
                }
                let reply = self.control(&from, request).await;
                if write_line(&mut send, &reply).await.is_err() {
                    return;
                }
            }
        }
        let _ = send.finish();
    }

    fn admit_phone(&self, key: &str) -> Result<String, Failure> {
        let state = self.state();
        let Some(log) = &state.log else { return Err(Refused::new("unpaired").into()) };
        let view = log.view();
        if view.revoked.contains(key) {
            return Err(Refused::new("revoked").into());
        }
        if !view.phones().contains(key) {
            return Err(Failure::new("not_a_phone", "only a phone opens a phone stream"));
        }
        Ok(view.name_of(key).unwrap_or_default())
    }

    async fn serve_phone(&self, remote: String, header: Value, mut send: SendStream, recv: BufReader<RecvStream>) {
        let from = match self.admit_phone(&remote) {
            Ok(name) => name,
            Err(failure) => {
                let _ = write_line(&mut send, &refusal(&failure)).await;
                let _ = send.finish();
                return;
            }
        };
        let theirs = header["phone"].as_u64().unwrap_or(0);
        if theirs + 1 < phone::VERSION || theirs > phone::VERSION {
            let update = if theirs < phone::VERSION { "this phone's app" } else { "Kanna on this machine" };
            let failure = Failure::new("other_phone_version", format!("{} speaks phone stream {}: update {update}", self.state().name, phone::VERSION));
            let _ = write_line(&mut send, &refusal(&failure)).await;
            let _ = send.finish();
            return;
        }
        let name = self.state().name.clone();
        if write_line(&mut send, &json!({ "ok": true, "kind": "phone", "name": name, "key": self.key(), "phone": phone::VERSION })).await.is_err() {
            return;
        }
        let Ok((ours, theirs)) = std::os::unix::net::UnixStream::pair() else { return };
        let mut revoked = self.0.revoked.subscribe();
        self.0.host.phone(from, remote.clone(), theirs);
        let node = self.clone();
        let watched = async move {
            while revoked.changed().await.is_ok() {
                if node.admit_phone(&remote).is_err() {
                    return;
                }
            }
            std::future::pending::<()>().await
        };
        tokio::select! {
            () = splice_unix(ours, send, recv) => {}
            () = watched => {}
        }
    }

    async fn join(&self, remote: &str, send: &mut SendStream, recv: &mut BufReader<RecvStream>) {
        let mine = self.descriptor();
        let staged = self.state().log.clone().unwrap_or_else(|| PeerLog::new(mine.clone()));
        let code = {
            let mut state = self.state();
            match state.phone_offer.as_mut() {
                None => Err(Failure::new("no_offer", "that machine isn't waiting for a phone")),
                Some(offer) => offer.begin(Instant::now()).map(|()| offer.code.clone()).map_err(Failure::from),
            }
        };
        let code = match code {
            Ok(code) => code,
            Err(failure) => {
                let _ = write_line(send, &refusal(&failure)).await;
                return;
            }
        };
        let ha = hello_a(&mine, staged.anchor());
        if write_line(send, &json!({ "ok": true, "hello": ha })).await.is_err() {
            return;
        }
        let Ok(message) = read_line(recv, MAX_LINE).await else { return };
        let hb = message["hello"].clone();
        let device = hb["device"].clone();
        let name = phone::name_of(remote);
        if device["key"] != remote || device["role"] != "phone" || device["name"] != name.as_str() {
            let _ = write_line(send, &refusal(&Refused::new("pake_confirmation_failed").into())).await;
            return;
        }
        let mut party = Party::new(Role::A, &code, ha, hb);
        let proof = match pake_bytes(&message["pake"]).and_then(|pake| party.finish(&self.0.secret, &pake)) {
            Ok(proof) => proof,
            Err(refused) => {
                let _ = write_line(send, &refusal(&refused.into())).await;
                return;
            }
        };
        if write_line(send, &json!({ "ok": true, "pake": pake_hex(&party.outbound), "proof": proof })).await.is_err() {
            return;
        }
        let Ok(message) = read_line(recv, MAX_LINE).await else { return };
        if let Err(refused) = party.confirm(&message["proof"]) {
            let _ = write_line(send, &refusal(&refused.into())).await;
            return;
        }
        let claimed = match self.state().phone_offer.as_mut() {
            Some(offer) if !offer.used => {
                offer.used = true;
                true
            }
            _ => false,
        };
        if !claimed {
            let _ = write_line(send, &refusal(&Refused::new("code_used").into())).await;
            return;
        }
        let mut staged = staged;
        let entry = staged.entry(&self.0.secret, "add", device.clone(), None, unix_time());
        if let Err(refused) = staged.merge(&[entry]) {
            let failure = Failure::from(refused);
            let _ = write_line(send, &refusal(&failure)).await;
            return self.settle(Err(failure));
        }
        let (tx, rx) = oneshot::channel();
        self.0.host.confirm(json!({ "name": name, "key": remote, "addrs": [], "capabilities": {}, "inviter": mine["name"], "phone": true }), Box::new(move |answer| {
            let _ = tx.send(answer);
        }));
        let refused = match rx.await {
            Ok(Ok(true)) => None,
            Ok(Ok(false)) => Some(format!("adding {name} wasn't confirmed in Kanna.app")),
            Ok(Err(reason)) => Some(format!("adding {name} needs a yes in Kanna.app: {reason}")),
            Err(_) => Some("Kanna.app didn't answer".to_string()),
        };
        if let Some(reason) = refused {
            let failure = Failure::new("gesture_required", reason);
            let _ = write_line(send, &refusal(&failure)).await;
            return self.settle(Err(failure));
        }
        let merged = {
            let mut state = self.state();
            match state.log.as_mut() {
                Some(log) => log.merge(&staged.entries()).map(|_| ()),
                None => {
                    state.log = Some(staged.clone());
                    Ok(())
                }
            }
        };
        if let Err(refused) = merged {
            let failure = Failure::from(refused);
            let _ = write_line(send, &refusal(&failure)).await;
            return self.settle(Err(failure));
        }
        self.save();
        let mut handoff = staged.export();
        if let Some(relay) = self.state().relay.as_ref() {
            handoff["relay"] = json!(relay.url.to_string());
            handoff["relay_token"] = json!(relay.token);
        }
        if write_line(send, &json!({ "ok": true, "handoff": party.seal(&handoff) })).await.is_err() {
            return self.settle(Err(Failure::new("unreachable", "lost the phone")));
        }
        let _ = read_line(recv, MAX_LINE).await;
        self.settle(Ok(json!({ "name": name, "key": remote })));
        let others: Vec<String> = self.state().log.as_ref().map(|l| l.view().machines().into_iter().filter(|k| *k != self.key()).collect()).unwrap_or_default();
        for other in others {
            let _ = self.sync(&other).await;
        }
    }

    async fn bridge(&self, remote: String, cert: Option<Value>, from: String, hello: Value, mut send: SendStream, mut recv: BufReader<RecvStream>) {
        let Ok((ours, theirs)) = std::os::unix::net::UnixStream::pair() else { return };
        let Ok(ours) = ours.set_nonblocking(true).and_then(|()| tokio::net::UnixStream::from_std(ours)) else { return };
        self.0.host.app(from, hello, theirs);
        let (mut from_daemon, mut to_daemon) = ours.into_split();
        let mut revoked = self.0.revoked.subscribe();
        let admitted = {
            let (node, remote, cert) = (self.clone(), remote.clone(), cert.clone());
            move || node.admit(&remote, cert.as_ref()).is_ok()
        };
        let still = admitted.clone();
        let inbound = async move {
            let mut line = Vec::new();
            loop {
                line.clear();
                match (&mut recv).take(MAX_LINE + 1).read_until(b'\n', &mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if line.len() as u64 > MAX_LINE || !still() || to_daemon.write_all(&line).await.is_err() {
                    break;
                }
            }
        };
        let outbound = async {
            let _ = tokio::io::copy(&mut from_daemon, &mut send).await;
        };
        let watched = async move {
            while revoked.changed().await.is_ok() {
                if !admitted() {
                    return;
                }
            }
            std::future::pending::<()>().await
        };
        tokio::select! {
            () = inbound => {}
            () = outbound => {}
            () = watched => {}
        }
        let _ = send.finish();
    }

    async fn take_transfer(&self, remote: &str, from: &str, recv: &mut BufReader<RecvStream>) -> Value {
        let header = match read_line(recv, MAX_LINE).await {
            Ok(header) => header,
            Err(e) => return refusal(&Failure::new("bad_request", format!("no transfer header: {e}"))),
        };
        let id = header["id"].as_str().unwrap_or_default().to_string();
        if id.is_empty() || id.len() > 96 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return refusal(&Failure::new("bad_request", "a transfer needs an ID of letters, digits and dashes"));
        }
        let Some(size) = header["size"].as_u64().filter(|size| *size <= MAX_TRANSFER) else {
            return refusal(&Failure::new("too_big", format!("a transfer carries at most {} MB", MAX_TRANSFER >> 20)));
        };
        let Some(inbox) = self.0.host.inbox() else { return refusal(&Failure::new("unavailable", "this machine takes no transfers")) };
        let file = inbox.join(format!("{id}.in"));
        if let Err(e) = receive_file(recv, &inbox, &file, size).await {
            return refusal(&Failure::new("unreachable", format!("lost the transfer on the way: {e}")));
        }
        let mut args = header["args"].as_object().cloned().unwrap_or_default();
        args.insert("action".into(), json!("transfer"));
        args.insert("id".into(), json!(id));
        args.insert("file".into(), json!(file.to_string_lossy()));
        args.insert(PEER.into(), json!(remote));
        self.control(from, json!({ "id": 1, "cmd": REPLICA, "args": args })).await
    }

    pub async fn transfer(&self, key: &str, id: &str, args: Value, path: &std::path::Path) -> Result<Value, Failure> {
        self.usable()?;
        let name = self.name_of(key);
        let size = tokio::fs::metadata(path).await.map_err(|e| Failure::new("failed", format!("can't read {}: {e}", path.display())))?.len();
        let (mut send, mut recv, _) = self.open_to(key, "transfer", json!({})).await?;
        let lost = |e: String| Failure::new("unreachable", format!("lost {name}: {e}"));
        write_line(&mut send, &json!({ "id": id, "size": size, "args": args })).await.map_err(lost)?;
        let mut file = tokio::fs::File::open(path).await.map_err(|e| Failure::new("failed", format!("can't read {}: {e}", path.display())))?;
        tokio::io::copy(&mut file, &mut send).await.map_err(|e| lost(e.to_string()))?;
        let _ = send.finish();
        read_line(&mut recv, MAX_LINE).await.map_err(lost)
    }

    async fn control(&self, from: &str, request: Value) -> Value {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        if request["cmd"] == "machine.sync" {
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
        self.0.revoked.send_modify(|n| *n += 1);
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
        let theirs = state.log.as_ref().and_then(|l| l.view().descriptors.get(key).and_then(|d| d["relay"].as_str().and_then(|r| relay_url(r).ok())));
        for relay in state.relay.iter().map(|r| &r.url).chain(theirs.iter()) {
            addr = addr.with_relay_url(relay.clone());
        }
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
        if self.state().log.as_ref().is_some_and(|l| l.view().revoked.contains(key)) {
            return Err(Failure::new("revoked", format!("{} is revoked from your machines", self.name_of(key))));
        }
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
            Err(_) => return Err(Failure::new("unreachable", format!("can't reach {name} within {}s (is its daemon running, and on this LAN? kanna machine addr {name} HOST gives an address)", DIAL.as_secs()))),
        };
        self.state().conns.insert(key.to_string(), conn.clone());
        if let Ok((mut send, mut recv, _)) = self.open(&conn, "control", &name).await {
            let _ = self.sync_over(&mut send, &mut recv).await;
            let _ = send.finish();
        }
        Ok((conn, true))
    }

    pub(crate) fn protocol(&self) -> u32 {
        protocol::of(&self.0.release).unwrap_or(0)
    }

    async fn open(&self, conn: &Connection, kind: &str, name: &str) -> Result<(SendStream, BufReader<RecvStream>, Value), Failure> {
        self.open_with(conn, kind, name, json!({})).await
    }

    async fn open_with(&self, conn: &Connection, kind: &str, name: &str, extra: Value) -> Result<(SendStream, BufReader<RecvStream>, Value), Failure> {
        let lost = |e: String| Failure::new("unreachable", format!("lost {name}: {e}"));
        let (mut send, recv) = conn.open_bi().await.map_err(|e| lost(e.to_string()))?;
        let mut recv = BufReader::new(recv);
        let mut header = json!({ "kind": kind, "protocol": self.protocol() });
        if let (Some(header), Some(extra)) = (header.as_object_mut(), extra.as_object()) {
            header.extend(extra.clone());
        }
        write_line(&mut send, &header).await.map_err(lost)?;
        let ack = read_line(&mut recv, MAX_HEADER).await.map_err(lost)?;
        let key = conn.remote_id().to_string();
        if ack["release"].is_object() {
            self.state().releases.insert(key, ack["release"].clone());
        }
        if ack["ok"] != true {
            let failure = Failure::from_reply(&ack);
            if failure.code == "other_protocol" {
                return Err(Failure::new("other_protocol", protocol::other_protocol(name, &ack["release"], self.protocol())));
            }
            return Err(Failure::new(&failure.code, format!("{name} refused this machine: {}", failure.message)));
        }
        if protocol::of(&ack) != Some(self.protocol()) {
            let _ = send.finish();
            return Err(Failure::new("other_protocol", protocol::other_protocol(name, &ack["release"], self.protocol())));
        }
        Ok((send, recv, ack))
    }

    pub(crate) async fn open_to(&self, key: &str, kind: &str, extra: Value) -> Result<(SendStream, BufReader<RecvStream>, Value), Failure> {
        let name = self.name_of(key);
        let (mut conn, mut fresh) = self.connect(key).await?;
        loop {
            match self.open_with(&conn, kind, &name, extra.clone()).await {
                Ok(opened) => return Ok(opened),
                Err(failure) => {
                    self.forget(key);
                    if fresh || failure.code != "unreachable" {
                        return Err(failure);
                    }
                    (conn, fresh) = self.connect(key).await?;
                }
            }
        }
    }

    async fn control_stream(&self, host: &str) -> Result<(SendStream, BufReader<RecvStream>, String), Failure> {
        self.stream(host, "control").await
    }

    async fn stream(&self, host: &str, kind: &str) -> Result<(SendStream, BufReader<RecvStream>, String), Failure> {
        self.usable()?;
        let key = self.resolve(host)?;
        if key == self.key() {
            return Err(Failure::new("bad_request", format!("{host} is this machine")));
        }
        let (send, recv, _) = self.open_to(&key, kind, json!({})).await?;
        Ok((send, recv, self.name_of(&key)))
    }

    async fn sync_over(&self, send: &mut SendStream, recv: &mut BufReader<RecvStream>) -> Result<(), Failure> {
        let Some(log) = self.state().log.as_ref().map(PeerLog::export) else { return Ok(()) };
        let lost = |e: String| Failure::new("unreachable", format!("lost the peer while swapping logs: {e}"));
        write_line(send, &json!({ "id": 0, "cmd": "machine.sync", "args": { "log": log } })).await.map_err(lost)?;
        let reply = read_line(recv, MAX_LINE).await.map_err(lost)?;
        if reply["ok"] != true {
            return Err(Failure::from_reply(&reply));
        }
        self.merge(&reply["log"])
    }

    pub async fn call(&self, host: &str, request: Value) -> Result<Value, Failure> {
        let (mut send, mut recv, name) = self.control_stream(host).await?;
        let lost = |e: String| Failure::new("unreachable", format!("lost {name}: {e}"));
        write_line(&mut send, &request).await.map_err(lost)?;
        let reply = read_line(&mut recv, MAX_LINE).await.map_err(lost)?;
        let _ = send.finish();
        Ok(reply)
    }

    pub async fn exchange(&self, host: &str, request: Value) -> Result<Value, Failure> {
        let (mut send, mut recv, name) = self.stream(host, "sync").await?;
        let lost = |e: String| Failure::new("unreachable", format!("lost {name}: {e}"));
        write_line(&mut send, &request).await.map_err(lost)?;
        let reply = read_line(&mut recv, MAX_LINE).await.map_err(lost)?;
        let _ = send.finish();
        Ok(reply)
    }

    pub async fn job(&self, host: &str) -> Result<(std::os::unix::net::UnixStream, String), Failure> {
        let (send, recv, name) = self.stream(host, "job").await?;
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().map_err(|e| Failure::new("failed", e.to_string()))?;
        tokio::spawn(splice_unix(ours, send, recv));
        Ok((theirs, name))
    }

    pub async fn sync(&self, host: &str) -> Result<Value, Failure> {
        let (mut send, mut recv, name) = self.control_stream(host).await?;
        let synced = self.sync_over(&mut send, &mut recv).await;
        let _ = send.finish();
        synced.map(|()| json!({ "name": name }))
    }

    pub async fn ping(&self, host: &str) -> Result<Value, Failure> {
        self.usable()?;
        let key = self.resolve(host)?;
        let name = self.name_of(&key);
        let started = Instant::now();
        let (mut conn, fresh) = self.connect(&key).await?;
        if !fresh && conn.open_bi().await.is_err() {
            self.state().conns.remove(&key);
            conn = self.connect(&key).await?.0;
        }
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
            return Err(Failure::new("bad_request", "this machine can't revoke itself: revoke it from another of your machines"));
        }
        let (name, is_phone) = {
            let mut state = self.state();
            let Some(log) = state.log.as_mut() else { return Err(Failure::new("unpaired", "this machine isn't paired")) };
            let name = log.view().name_of(&key).ok_or_else(|| Failure::new("not_found", format!("{host} isn't one of your machines")))?;
            let is_phone = log.view().is_phone(&key);
            let entry = log.entry(&self.0.secret, "revoke", json!({ "key": key }), None, unix_time());
            log.merge(&[entry])?;
            (name, is_phone)
        };
        self.drop_revoked();
        self.save();
        let others: Vec<String> = self.state().log.as_ref().map(|l| l.view().machines().into_iter().filter(|k| *k != self.key()).collect()).unwrap_or_default();
        let mut told = vec![];
        for other in others {
            if self.sync(&other).await.is_ok() {
                told.push(self.name_of(&other));
            }
        }
        let relay_token = is_phone && self.relay_token().is_some();
        Ok(json!({ "name": name, "key": key, "told": told, "relay_token": relay_token }))
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
                    Err(Failure::new("not_found", "no machine on this LAN is waiting to pair: run kanna machine pair on it first (or give its --peer KEY@HOST, which it prints)"))
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
                Ok(Err(failure)) if BEFORE_THE_CODE.contains(&failure.code.as_str()) => last = failure,
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
        if !valid_name(&name) {
            return refuse(&mut send, Failure::new("bad_name", "the machine waiting to pair sent a name that isn't one (up to 32 lowercase letters, digits and -)")).await;
        }
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
        {
            let mut state = self.state();
            match state.log.as_mut() {
                Some(log) => {
                    log.merge(&staged.entries())?;
                }
                None => state.log = Some(staged.clone()),
            }
        }
        self.save();
        write_line(&mut send, &json!({ "handoff": party.seal(&staged.export()) })).await.map_err(lost)?;
        let done = read_line(&mut recv, MAX_LINE).await.map_err(lost)?;
        let _ = send.finish();
        if done["ok"] != true {
            return Err(Failure::from_reply(&done));
        }
        self.state().conns.insert(key.clone(), conn);
        let others: Vec<String> = self.state().log.as_ref().map(|l| l.view().machines().into_iter().filter(|k| *k != self.key() && *k != key).collect()).unwrap_or_default();
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
        let claimed = match self.state().offer.as_mut() {
            Some(offer) if !offer.used => {
                offer.used = true;
                true
            }
            _ => false,
        };
        if !claimed {
            let _ = write_line(send, &refusal(&Refused::new("code_used").into())).await;
            return;
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
        let id = log.id().to_string();
        self.state().log.get_or_insert(log);
        self.save();
        let _ = write_line(send, &json!({ "ok": true })).await;
        self.settle(Ok(json!({ "name": inviter, "key": remote, "log": id })));
    }
}

async fn loopback(port: &Value, name: &str) -> Result<tokio::net::TcpStream, Failure> {
    let port = port.as_u64().and_then(|p| u16::try_from(p).ok()).filter(|p| *p > 0).ok_or_else(|| Failure::new("bad_request", format!("{port} isn't a port")))?;
    for ip in [std::net::IpAddr::from([127, 0, 0, 1]), std::net::IpAddr::from(std::net::Ipv6Addr::LOCALHOST)] {
        if let Ok(Ok(tcp)) = tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect((ip, port))).await {
            let _ = tcp.set_nodelay(true);
            return Ok(tcp);
        }
    }
    Err(Failure::new("nothing_listening", format!("nothing listens on port {port} on {name}")))
}

pub(crate) async fn splice(tcp: tokio::net::TcpStream, mut send: SendStream, mut recv: BufReader<RecvStream>) {
    let (mut from_tcp, mut to_tcp) = tcp.into_split();
    let inbound = async move {
        let _ = tokio::io::copy(&mut recv, &mut to_tcp).await;
        let _ = to_tcp.shutdown().await;
    };
    let outbound = async move {
        let _ = tokio::io::copy(&mut from_tcp, &mut send).await;
        let _ = send.finish();
    };
    tokio::join!(inbound, outbound);
}

pub(crate) async fn splice_unix(unix: std::os::unix::net::UnixStream, mut send: SendStream, mut recv: BufReader<RecvStream>) {
    let Ok(unix) = unix.set_nonblocking(true).and_then(|()| tokio::net::UnixStream::from_std(unix)) else { return };
    let (mut from_unix, mut to_unix) = unix.into_split();
    let inbound = async move {
        let _ = tokio::io::copy(&mut recv, &mut to_unix).await;
        let _ = to_unix.shutdown().await;
    };
    let outbound = async move {
        let _ = tokio::io::copy(&mut from_unix, &mut send).await;
        let _ = send.finish();
    };
    tokio::join!(inbound, outbound);
}

async fn refuse(send: &mut SendStream, failure: Failure) -> Result<Value, Failure> {
    let _ = write_line(send, &refusal(&failure)).await;
    let _ = send.finish();
    Err(failure)
}

fn unix_time() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

async fn receive_file<R: tokio::io::AsyncRead + Unpin>(recv: &mut R, inbox: &std::path::Path, file: &std::path::Path, size: u64) -> Result<(), String> {
    tokio::fs::create_dir_all(inbox).await.map_err(|e| e.to_string())?;
    let part = file.with_extension("part");
    let mut out = tokio::fs::File::create(&part).await.map_err(|e| e.to_string())?;
    let copied = tokio::io::copy(&mut (&mut *recv).take(size), &mut out).await.map_err(|e| e.to_string())?;
    if copied != size {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(format!("{copied} of {size} bytes came"));
    }
    out.sync_all().await.map_err(|e| e.to_string())?;
    tokio::fs::rename(&part, file).await.map_err(|e| e.to_string())
}

pub(crate) async fn read_line<R: tokio::io::AsyncBufRead + Unpin>(recv: &mut R, max: u64) -> Result<Value, String> {
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

pub(crate) async fn write_line(send: &mut SendStream, value: &Value) -> Result<(), String> {
    let mut line = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    line.push(b'\n');
    send.write_all(&line).await.map_err(|e| e.to_string())
}
