use crate::app_link::AppLink;
use crate::db::Db;
use crate::Event;
use kanna_mux::json::{json, string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError, Responder};
use kanna_net::{Config, Failure, Host, Node};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;

pub const KEY: &str = "device.key";

const CONFIRM_WAIT: Duration = Duration::from_secs(120);

const CONFIRM: &str = "app.fleet.confirm";

pub struct Fleet {
    runtime: tokio::runtime::Runtime,
    node: Result<Node, String>,
}

struct DaemonHost(Mutex<Sender<Event>>);

impl Host for DaemonHost {
    fn request(&self, _from: String, request: Value, reply: Box<dyn FnOnce(Value) + Send>) {
        let _ = self.0.lock().unwrap_or_else(|p| p.into_inner()).send(Event::Request(request, reply));
    }

    fn confirm(&self, question: Value, answer: Box<dyn FnOnce(Result<bool, String>) + Send>) {
        let _ = self.0.lock().unwrap_or_else(|p| p.into_inner()).send(Event::FleetConfirm { question, answer });
    }

    fn save(&self, state: Value) {
        let _ = self.0.lock().unwrap_or_else(|p| p.into_inner()).send(Event::FleetSave(state));
    }
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

fn port() -> Option<u16> {
    std::env::var("KANNA_FLEET_PORT").ok().and_then(|p| p.parse().ok())
}

impl Fleet {
    pub fn start(state_dir: &Path, db: &Db, events: Sender<Event>) -> Result<Fleet, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("kanna.fleet").enable_all().build().map_err(|e| format!("can't start the fleet's runtime: {e}"))?;
        let node = kanna_net::read_key(&state_dir.join(KEY)).and_then(|secret| {
            let saved = db.fleet().unwrap_or_else(|_| json!({}));
            let mdns = std::env::var("KANNA_FLEET_MDNS").map_or(true, |v| v != "0");
            let config = Config { secret, saved, name: hostname(), capabilities: capabilities(), port: port(), mdns };
            runtime.block_on(Node::start(config, Box::new(DaemonHost(Mutex::new(events)))))
        });
        Ok(Fleet { runtime, node })
    }

    pub fn describe(&self) -> String {
        match &self.node {
            Ok(node) => format!("fleet endpoint {} on UDP port {}", node.key(), node.port()),
            Err(reason) => format!("no fleet endpoint: {reason}"),
        }
    }

    pub fn install(self: &std::rc::Rc<Self>, core: &CoreHandle) {
        let this = std::rc::Rc::downgrade(self);
        core.add_command(
            "fleet",
            std::rc::Rc::new(move |args, responder| match this.upgrade() {
                Some(fleet) => fleet.handle(args, responder),
                None => responder.reply(Err(KmuxError::new("stopping", "the daemon is stopping"))),
            }),
        );
    }

    fn handle(&self, args: Value, responder: Responder) {
        let node = match &self.node {
            Ok(node) => node.clone(),
            Err(reason) => return responder.reply(Err(KmuxError::new("unavailable", format!("this daemon has no fleet endpoint: {reason}")))),
        };
        let action = string(&args, "action").unwrap_or("status").to_string();
        let host = string(&args, "host").map(String::from);
        let needs_host = |host: Option<String>| host.ok_or_else(|| KmuxError::new("bad_request", format!("fleet {action} needs a machine: its name or key")));
        match action.as_str() {
            "status" => responder.reply(object(node.status())),
            "name" => responder.reply(string(&args, "name").ok_or_else(|| KmuxError::new("bad_request", "fleet name needs a NAME")).and_then(|name| node.rename(name).map_err(error)).and_then(object)),
            "offer" => responder.reply(node.offer().map_err(error).and_then(object)),
            "addr" => {
                let result = needs_host(host).and_then(|host| {
                    let addr = string(&args, "addr").ok_or_else(|| KmuxError::new("bad_request", "fleet addr needs a HOST or HOST:PORT"))?;
                    node.addr(&host, addr).map_err(error)
                });
                responder.reply(result.and_then(object))
            }
            "wait" => self.spawn(responder, async move { node.wait_offer().await }),
            "pair" => {
                let Some(code) = string(&args, "code").map(String::from) else { return responder.reply(Err(KmuxError::new("bad_request", "fleet pair CODE: the code the other machine shows"))) };
                let peer = string(&args, "peer").map(String::from);
                self.spawn(responder, async move { node.pair(&code, peer.as_deref()).await })
            }
            "revoke" | "ping" | "sync" => match needs_host(host) {
                Ok(host) => self.spawn(responder, async move {
                    match action.as_str() {
                        "revoke" => node.revoke(&host).await,
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
            other => responder.reply(Err(KmuxError::new("bad_request", format!("unknown fleet action \"{other}\" (status, name, offer, wait, pair, revoke, addr, ping, sync or call)")))),
        }
    }

    fn spawn(&self, responder: Responder, work: impl std::future::Future<Output = Result<Value, Failure>> + Send + 'static) {
        self.runtime.spawn(async move { responder.reply(work.await.map_err(error).and_then(object)) });
    }

    pub fn confirm(&self, app: &AppLink, question: Value, answer: Box<dyn FnOnce(Result<bool, String>) + Send>) {
        if !app.attached() {
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

    pub fn stop(&self) {
        if let Ok(node) = &self.node {
            let _ = self.runtime.block_on(tokio::time::timeout(Duration::from_secs(1), node.close()));
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
