use crate::app_link::AppLink;
use crate::db::Db;
use crate::replica::{Peer, Roster};
use crate::Event;
use kanna_mux::json::{json, string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError, Responder};
use kanna_net::{Config, Failure, Host, Node};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub const KEY: &str = "device.key";

const CONFIRM_WAIT: Duration = Duration::from_secs(120);

const CONFIRM: &str = "app.machine.confirm";

pub type Withdraw = std::sync::Arc<dyn Fn(&str) -> Option<bool> + Send + Sync>;

const EXCHANGE_WAIT: Duration = Duration::from_secs(20);

pub struct Machines {
    runtime: tokio::runtime::Runtime,
    node: Result<Node, String>,
    withdraw: std::sync::OnceLock<Withdraw>,
}

struct DaemonHost(Mutex<crate::runtime::Events>, std::sync::Arc<crate::wire::Wire>, PathBuf, PathBuf);

impl Host for DaemonHost {
    fn request(&self, from: String, request: Value, reply: Box<dyn FnOnce(Value) + Send>) {
        if let Some(refused) = refuse_remote(&request) {
            return reply(refused);
        }
        let event = match request.get("cmd").and_then(Value::as_str) == Some(crate::accounts::COMMAND) {
            true => Event::PeerRequest { from, request, reply },
            false => Event::Request(request, reply),
        };
        let _ = self.0.lock().unwrap_or_else(|p| p.into_inner()).send(event);
    }

    fn confirm(&self, question: Value, answer: Box<dyn FnOnce(Result<bool, String>) + Send>) {
        let _ = self.0.lock().unwrap_or_else(|p| p.into_inner()).send(Event::MachineConfirm { question, answer });
    }

    fn save(&self, state: Value) {
        let _ = self.0.lock().unwrap_or_else(|p| p.into_inner()).send(Event::MachinesSave(state));
    }

    fn app(&self, from: String, hello: Value, stream: std::os::unix::net::UnixStream) {
        let events = self.0.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let wire = self.1.clone();
        let Ok(reader) = stream.try_clone() else { return };
        let _ = std::thread::Builder::new().name("kanna.app.read".into()).spawn(move || crate::app_link::serve(events, wire, hello, std::io::BufReader::new(reader), stream, Some(from)));
    }

    fn inbox(&self) -> Option<PathBuf> {
        Some(self.2.clone())
    }

    fn phone(&self, from: String, _key: String, stream: std::os::unix::net::UnixStream) {
        let events = self.0.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let _ = std::thread::Builder::new().name("kanna.phone.read".into()).spawn(move || crate::phone::serve(events, stream, from));
    }

    fn job(&self, from: String, stream: std::os::unix::net::UnixStream) {
        let state_dir = self.3.clone();
        let _ = std::thread::Builder::new().name("kanna.build.relay".into()).spawn(move || crate::builds::relay(&state_dir, &from, stream));
    }
}

#[derive(Clone)]
pub struct Caller {
    handle: tokio::runtime::Handle,
    node: Node,
}

const CALL_WAIT: Duration = Duration::from_secs(60);

impl Caller {
    pub fn key(&self) -> String {
        self.node.key()
    }

    pub fn status(&self) -> Value {
        self.node.status()
    }

    pub fn log(&self) -> Option<Value> {
        self.node.log()
    }

    pub fn relay_token(&self) -> Option<String> {
        self.node.relay_token()
    }

    pub fn phone_offer(&self) -> Result<Value, KmuxError> {
        self.node.offer_phone().map_err(error)
    }

    pub fn phone_wait(&self, responder: Responder) {
        let node = self.node.clone();
        self.handle.spawn(async move { responder.reply(node.wait_phone().await.map_err(error).and_then(object)) });
    }

    pub fn call(&self, host: &str, cmd: &str, args: Value) -> Result<Value, KmuxError> {
        let request = json!({ "id": 1, "cmd": cmd, "args": args });
        let reply = self.handle.block_on(async { tokio::time::timeout(CALL_WAIT, self.node.call(host, request)).await });
        match reply {
            Ok(Ok(reply)) if reply["ok"] == true => Ok(reply),
            Ok(Ok(reply)) => Err(KmuxError::new(reply["error"]["code"].as_str().unwrap_or("failed"), reply["error"]["message"].as_str().unwrap_or("failed"))),
            Ok(Err(failure)) => Err(error(failure)),
            Err(_) => Err(KmuxError::new("unreachable", format!("{host} didn't answer in time"))),
        }
    }
}

impl Caller {
    pub fn job(&self, host: &str) -> Result<(std::os::unix::net::UnixStream, String), Failure> {
        let opened = self.handle.block_on(async { tokio::time::timeout(CALL_WAIT, self.node.job(host)).await });
        opened.unwrap_or_else(|_| Err(Failure::new("unreachable", format!("{host} didn't answer in time"))))
    }
}

pub fn refuse_remote(request: &Value) -> Option<Value> {
    let certify = request.get("cmd").and_then(Value::as_str) == Some("machine") && request.pointer("/args/action").and_then(Value::as_str) == Some("certify");
    certify.then(|| json!({ "id": request.get("id").cloned().unwrap_or(Value::Null), "ok": false, "error": { "code": "local_only", "message": "only this machine's own Kanna.app can have its endpoint certified" } }))
}

fn hostname() -> String {
    let mut buffer = [0u8; 256];
    let found = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0;
    let raw = if found { String::from_utf8_lossy(&buffer[..buffer.iter().position(|b| *b == 0).unwrap_or(0)]).into_owned() } else { String::new() };
    let first = raw.split('.').next().unwrap_or_default().to_lowercase();
    let name: String = first.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>().trim_matches('-').chars().take(32).collect();
    if name.is_empty() { "machine".into() } else { name }
}

fn capabilities() -> Value {
    let cores = std::thread::available_parallelism().map_or(0, |n| n.get());
    json!({ "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "cores": cores })
}

pub fn release() -> Value {
    let channel = kanna_build::Build::current().channel.id();
    json!({ "commit": kanna_build::COMMIT, "version": kanna_build::VERSION, "channel": channel, "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "protocol": kanna_build::PROTOCOL })
}

fn port() -> Option<u16> {
    std::env::var("KANNA_MACHINE_PORT").ok().and_then(|p| p.parse().ok())
}

impl Machines {
    pub fn start(state_dir: &Path, db: &Db, events: crate::runtime::Events, wire: std::sync::Arc<crate::wire::Wire>) -> Result<Machines, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("kanna.machines").enable_all().build().map_err(|e| format!("can't start the machines' runtime: {e}"))?;
        let node = kanna_net::read_key(&state_dir.join(KEY)).and_then(|secret| {
            let saved = db.machines().unwrap_or_else(|_| json!({}));
            let mdns = std::env::var("KANNA_MACHINE_MDNS").map_or(true, |v| v != "0");
            let config = Config { secret, saved, name: hostname(), capabilities: capabilities(), port: port(), mdns, release: release() };
            runtime.block_on(Node::start(config, Box::new(DaemonHost(Mutex::new(events), wire, crate::transfer::inbox(state_dir), state_dir.to_path_buf()))))
        });
        Ok(Machines { runtime, node, withdraw: std::sync::OnceLock::new() })
    }

    pub fn before_revoke(&self, withdraw: Withdraw) {
        let _ = self.withdraw.set(withdraw);
    }

    pub fn caller(&self) -> Option<Caller> {
        self.node.as_ref().ok().map(|node| Caller { handle: self.runtime.handle().clone(), node: node.clone() })
    }

    pub fn describe(&self) -> String {
        match &self.node {
            Ok(node) => format!("endpoint {} on UDP port {}", node.key(), node.port()),
            Err(reason) => format!("no endpoint: {reason}"),
        }
    }

    pub fn install(self: &std::rc::Rc<Self>, core: &CoreHandle) {
        let this = std::rc::Rc::downgrade(self);
        core.add_command(
            "machine",
            std::rc::Rc::new(move |args, responder| match this.upgrade() {
                Some(machines) => machines.handle(args, responder),
                None => responder.reply(Err(KmuxError::new("stopping", "the daemon is stopping"))),
            }),
        );
    }

    fn handle(&self, args: Value, responder: Responder) {
        let node = match &self.node {
            Ok(node) => node.clone(),
            Err(reason) => return responder.reply(Err(KmuxError::new("unavailable", format!("this daemon has no endpoint: {reason}")))),
        };
        let action = string(&args, "action").unwrap_or("status").to_string();
        let host = string(&args, "host").map(String::from);
        let needs_host = |host: Option<String>| host.ok_or_else(|| KmuxError::new("bad_request", format!("machine {action} needs a machine: its name or key")));
        match action.as_str() {
            "status" => responder.reply(object(node.status())),
            "certify" => responder.reply(string(&args, "key").ok_or_else(|| KmuxError::new("bad_request", "machine certify needs the app's endpoint KEY")).and_then(|key| node.certify(key).map_err(error)).and_then(|cert| object(json!({ "cert": cert })))),
            "name" => responder.reply(string(&args, "name").ok_or_else(|| KmuxError::new("bad_request", "machine name needs a NAME")).and_then(|name| node.rename(name).map_err(error)).and_then(object)),
            "offer" => responder.reply(node.offer().map_err(error).and_then(object)),
            "addr" => {
                let result = needs_host(host).and_then(|host| {
                    let addr = string(&args, "addr").ok_or_else(|| KmuxError::new("bad_request", "machine addr needs a HOST or HOST:PORT"))?;
                    node.addr(&host, addr).map_err(error)
                });
                responder.reply(result.and_then(object))
            }
            "wait" => self.spawn(responder, async move { node.wait_offer().await }),
            "relay" => {
                let url = string(&args, "url").map(String::from);
                let token = string(&args, "token").map(String::from);
                self.spawn(responder, async move { node.set_relay(url.as_deref(), token.as_deref()).await })
            }
            "pair" => {
                let Some(code) = string(&args, "code").map(String::from) else { return responder.reply(Err(KmuxError::new("bad_request", "machine pair CODE: the code the other machine shows"))) };
                let peer = string(&args, "peer").map(String::from);
                self.spawn(responder, async move { node.pair(&code, peer.as_deref()).await })
            }
            "revoke" => match needs_host(host) {
                Ok(host) => {
                    let withdraw = self.withdraw.get().cloned();
                    self.spawn(responder, async move {
                        let withdrawn = match withdraw {
                            Some(withdraw) => {
                                let host = host.clone();
                                tokio::task::spawn_blocking(move || withdraw(&host)).await.ok().flatten()
                            }
                            None => None,
                        };
                        let mut revoked = node.revoke(&host).await?;
                        if let Some(withdrawn) = withdrawn {
                            revoked["accounts"] = json!(withdrawn);
                        }
                        Ok(revoked)
                    })
                }
                Err(e) => responder.reply(Err(e)),
            },
            "ping" | "sync" => match needs_host(host) {
                Ok(host) => self.spawn(responder, async move {
                    match action.as_str() {
                        "ping" => node.ping(&host).await,
                        _ => node.sync(&host).await,
                    }
                }),
                Err(e) => responder.reply(Err(e)),
            },
            "call" => {
                let host = match needs_host(host) {
                    Ok(host) => host,
                    Err(e) => return responder.reply(Err(e)),
                };
                let request = json!({ "id": 1, "cmd": args.get("cmd").cloned().unwrap_or(Value::Null), "args": args.get("args").cloned().unwrap_or_else(|| json!({})) });
                self.runtime.spawn(async move {
                    let reply = node.call(&host, request).await;
                    responder.reply(match reply {
                        Ok(reply) if reply["ok"] == true => {
                            let mut fields = reply.as_object().cloned().unwrap_or_default();
                            fields.remove("id");
                            fields.remove("ok");
                            Ok(fields)
                        }
                        Ok(reply) => Err(KmuxError::new(reply["error"]["code"].as_str().unwrap_or("failed"), format!("{host}: {}", reply["error"]["message"].as_str().unwrap_or("failed")))),
                        Err(failure) => Err(error(failure)),
                    })
                });
            }
            other => responder.reply(Err(KmuxError::new("bad_request", format!("unknown machine action \"{other}\" (status, name, offer, wait, pair, revoke, addr, relay, ping, sync, certify, update or call)")))),
        }
    }

    fn spawn(&self, responder: Responder, work: impl std::future::Future<Output = Result<Value, Failure>> + Send + 'static) {
        self.runtime.spawn(async move { responder.reply(work.await.map_err(error).and_then(object)) });
    }

    pub fn confirm(&self, app: &AppLink, question: Value, answer: Box<dyn FnOnce(Result<bool, String>) + Send>) {
        if !app.attached_here() {
            return answer(Err("Kanna.app isn't open on this machine".into()));
        }
        let (responder, reply) = kanna_mux::responder();
        app.ask_within(CONFIRM, question, CONFIRM_WAIT, responder);
        self.runtime.spawn(async move {
            answer(match reply.await {
                Ok(fields) => Ok(fields.get("confirmed") == Some(&Value::Bool(true))),
                Err(e) => Err(e.message),
            })
        });
    }

    pub fn roster(&self) -> Roster {
        let Ok(node) = &self.node else { return Roster { me: Peer { key: String::new(), name: hostname() }, peers: vec![] } };
        let status = node.status();
        let me = Peer { key: node.key(), name: status["name"].as_str().unwrap_or_default().to_string() };
        let peers = status["peers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["me"] != true && p["trusted"] == true && p["revoked"] != true && p["conflict"] != true)
            .map(|p| Peer { key: p["key"].as_str().unwrap_or_default().to_string(), name: p["name"].as_str().unwrap_or_default().to_string() })
            .collect();
        Roster { me, peers }
    }

    pub fn exchange(&self, key: String, request: Value, done: Box<dyn FnOnce(Result<Value, String>) + Send>) {
        let Ok(node) = &self.node else { return done(Err("this daemon has no endpoint".into())) };
        let node = node.clone();
        self.runtime.spawn(async move {
            let reply = tokio::time::timeout(EXCHANGE_WAIT, node.exchange(&key, request)).await;
            done(match reply {
                Ok(Ok(reply)) => Ok(reply),
                Ok(Err(failure)) => Err(format!("{} [{}]", failure.message, failure.code)),
                Err(_) => Err(format!("no answer in {}s [unreachable]", EXCHANGE_WAIT.as_secs())),
            })
        });
    }

    pub fn transfer(&self, key: String, id: String, args: Value, path: PathBuf, done: Box<dyn FnOnce(Result<Value, String>) + Send>) {
        let Ok(node) = &self.node else { return done(Err("this daemon has no endpoint [unavailable]".into())) };
        let node = node.clone();
        self.runtime.spawn(async move {
            done(node.transfer(&key, &id, args, &path).await.map_err(|failure| format!("{} [{}]", failure.message, failure.code)))
        });
    }

    pub fn machine(&self) -> Value {
        match &self.node {
            Ok(node) => json!({ "name": node.status()["name"], "capabilities": capabilities(), "relay_token": node.relay_token() }),
            Err(_) => json!({ "name": hostname(), "capabilities": capabilities() }),
        }
    }

    pub fn stop(&self) {
        if let Ok(node) = &self.node {
            let _ = self.runtime.block_on(async { tokio::time::timeout(Duration::from_secs(1), node.close()).await });
        }
    }
}

fn error(failure: Failure) -> KmuxError {
    KmuxError::new(failure.code, failure.message)
}

fn object(value: Value) -> Result<Map, KmuxError> {
    match value {
        Value::Object(map) => Ok(map),
        other => Ok(Map::from_iter([("value".to_string(), other)])),
    }
}
