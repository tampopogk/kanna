use kanna_net::{Relay, Client, Config, Host, Node, SecretKey, KINDS};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Machine {
    yes: Arc<Mutex<bool>>,
    asked: Arc<Mutex<Vec<Value>>>,
    saved: Arc<Mutex<Value>>,
    served: Arc<Mutex<Vec<(String, Value)>>>,
    apps: Arc<Mutex<Vec<(String, Value, UnixStream)>>>,
}

impl Host for Machine {
    fn request(&self, from: String, request: Value, reply: Box<dyn FnOnce(Value) + Send>) {
        self.served.lock().unwrap().push((from.clone(), request.clone()));
        reply(json!({ "id": request["id"], "ok": true, "cmd": request["cmd"], "from": from }))
    }

    fn confirm(&self, question: Value, answer: Box<dyn FnOnce(Result<bool, String>) + Send>) {
        self.asked.lock().unwrap().push(question);
        answer(Ok(*self.yes.lock().unwrap()))
    }

    fn save(&self, state: Value) {
        *self.saved.lock().unwrap() = state;
    }

    fn app(&self, from: String, hello: Value, stream: UnixStream) {
        self.apps.lock().unwrap().push((from, hello, stream));
    }
}

async fn machine(name: &str, saved: Value) -> (Node, Machine) {
    machine_on(name, saved, 0).await
}

async fn machine_on(name: &str, saved: Value, port: u16) -> (Node, Machine) {
    machine_with(name, saved, port, SecretKey::generate()).await
}

async fn machine_with(name: &str, saved: Value, port: u16, secret: SecretKey) -> (Node, Machine) {
    let release = json!({ "os": std::env::consts::OS, "protocol": 4 });
    server(name, saved, port, secret, release).await
}

async fn server(name: &str, saved: Value, port: u16, secret: SecretKey, release: Value) -> (Node, Machine) {
    let host = Machine::default();
    *host.yes.lock().unwrap() = true;
    let capabilities = json!({ "os": release["os"], "arch": release["arch"] });
    let config = Config { secret, saved, name: name.into(), capabilities, port: Some(port), mdns: false, release };
    (Node::start(config, Box::new(host.clone())).await.unwrap(), host)
}

fn linux(commit: &str, protocol: u32) -> Value {
    json!({ "commit": commit, "version": "4.0.0", "channel": "production", "os": "linux", "arch": "x86_64", "protocol": protocol })
}

fn peer(node: &Node) -> String {
    format!("{}@{}:{}", node.key(), "127.0.0.1", node.port())
}

async fn pair(inviter: &Node, newcomer: &Node) -> Result<Value, kanna_net::Failure> {
    let offer = newcomer.offer(None)?;
    let code = offer["code"].as_str().unwrap().to_string();
    let waiting = {
        let newcomer = newcomer.clone();
        tokio::spawn(async move { newcomer.wait_offer().await })
    };
    let paired = inviter.pair(&code, Some(&peer(newcomer)), false).await;
    let waited = waiting.await.unwrap();
    if let Ok(paired) = &paired {
        assert_eq!(waited.unwrap()["name"], inviter.status()["name"]);
        assert_eq!(paired["name"], newcomer.status()["name"]);
    }
    paired
}

fn names(node: &Node) -> Vec<(String, bool)> {
    node.status()["peers"].as_array().unwrap().iter().map(|p| (p["name"].as_str().unwrap().to_string(), p["trusted"] == true)).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paired_machine_takes_commands_and_an_unpaired_or_revoked_one_is_refused() {
    let (mbp, mbp_host) = machine("mbp", json!({})).await;
    let (vm, vm_host) = machine("linux-vm", json!({})).await;
    let (stranger, _) = machine("stranger", json!({})).await;

    let paired = pair(&mbp, &vm).await.unwrap();
    assert_eq!(paired["key"], vm.key());
    assert_eq!(mbp_host.asked.lock().unwrap()[0]["name"], "linux-vm");
    assert_eq!(names(&mbp), [("linux-vm".to_string(), true), ("mbp".to_string(), true)]);
    assert_eq!(names(&vm), names(&mbp));
    assert_eq!(vm_host.saved.lock().unwrap()["anchor"]["name"], "mbp");

    let reply = mbp.call("linux-vm", json!({ "id": 7, "cmd": "list", "args": {} })).await.unwrap();
    assert_eq!(reply, json!({ "id": 7, "ok": true, "cmd": "list", "from": "mbp" }));
    let reply = vm.call("mbp", json!({ "id": 1, "cmd": "tasks", "args": {} })).await.unwrap();
    assert_eq!(reply["from"], "linux-vm");

    let ping = mbp.ping("linux-vm").await.unwrap();
    assert_eq!(ping["path"], "direct");
    for kind in KINDS {
        assert_eq!(ping["streams"][kind]["ok"], true, "{kind}: {ping}");
    }

    stranger.addr(&vm.key(), &format!("127.0.0.1:{}", vm.port())).unwrap();
    let refused = stranger.call(&vm.key(), json!({ "id": 1, "cmd": "list" })).await.unwrap_err();
    assert_eq!(refused.code, "unpaired", "{refused:?}");
    let ping = stranger.ping(&vm.key()).await.unwrap();
    for kind in KINDS {
        assert_eq!(ping["streams"][kind]["error"]["code"], "unpaired", "{kind}");
    }
    assert!(vm_host.served.lock().unwrap().iter().all(|(from, _)| from == "mbp"));

    let revoked = mbp.revoke("linux-vm").await.unwrap();
    assert_eq!(revoked["name"], "linux-vm");
    assert_eq!(revoked["told"], json!([]));
    let refused = vm.call("mbp", json!({ "id": 2, "cmd": "list" })).await.unwrap_err();
    assert_eq!(refused.code, "revoked", "{refused:?}");
    assert_eq!(names(&mbp), [("linux-vm".to_string(), false), ("mbp".to_string(), true)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_needs_the_code_and_a_yes_and_a_reload_keeps_the_log() {
    let (mbp, mbp_host) = machine("mbp", json!({})).await;
    let (vm, vm_host) = machine("linux-vm", json!({})).await;

    vm.offer(None).unwrap();
    let wrong = mbp.pair("12345", Some(&peer(&vm)), false).await.unwrap_err();
    assert_eq!(wrong.code, "pake_confirmation_failed");
    assert_eq!(mbp.status()["paired"], false);

    *mbp_host.yes.lock().unwrap() = false;
    let refused = pair(&mbp, &vm).await.unwrap_err();
    assert_eq!(refused.code, "gesture_required");
    assert_eq!(vm.status()["paired"], false);
    assert_eq!(mbp.status()["paired"], false);

    *mbp_host.yes.lock().unwrap() = true;
    pair(&mbp, &vm).await.unwrap();
    let again = vm.offer(None).unwrap_err();
    assert_eq!(again.code, "already_paired");

    let saved = vm_host.saved.lock().unwrap().clone();
    let (reloaded, _) = machine("ignored", saved.clone()).await;
    assert_eq!(reloaded.status()["name"], "linux-vm");
    assert_eq!(reloaded.status()["log"], vm.status()["log"]);
    let mut forged = saved;
    forged["entries"][0]["signature"] = json!("00".repeat(64));
    let (broken, _) = machine("x", forged).await;
    assert!(broken.broken().unwrap().contains("bad_signature"));
    assert_eq!(broken.call("mbp", json!({})).await.unwrap_err().code, "broken");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_third_machine_learns_the_log_and_a_revocation_reaches_it() {
    let (mbp, _) = machine("mbp", json!({})).await;
    let (vm, _) = machine("linux-vm", json!({})).await;
    let (mini, _) = machine("mini", json!({})).await;
    pair(&mbp, &vm).await.unwrap();
    pair(&mbp, &mini).await.unwrap();
    assert_eq!(names(&vm).len(), 3, "the vm hears of the mini when the mbp pairs it");
    vm.call("mini", json!({ "id": 1, "cmd": "list" })).await.unwrap();
    let revoked = mbp.revoke("mini").await.unwrap();
    assert_eq!(revoked["told"], json!(["linux-vm"]));
    assert_eq!(mini.call("linux-vm", json!({ "id": 1, "cmd": "list" })).await.unwrap_err().code, "revoked");
}

#[tokio::test(flavor = "multi_thread")]
async fn machines_on_fixed_ports_reach_each_other_at_a_given_address() {
    let (mbp, _) = machine_on("mbp", json!({}), 47471).await;
    let (vm, _) = machine_on("linux-vm", json!({}), 47472).await;
    assert_eq!((mbp.port(), vm.port()), (47471, 47472));
    pair(&mbp, &vm).await.unwrap();
    mbp.call("linux-vm", json!({ "id": 1, "cmd": "list" })).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_that_restarted_is_dialed_again() {
    let (mbp, _) = machine("mbp", json!({})).await;
    let secret = SecretKey::generate();
    let (vm, vm_host) = machine_with("linux-vm", json!({}), 0, secret.clone()).await;
    pair(&mbp, &vm).await.unwrap();
    mbp.call("linux-vm", json!({ "id": 1, "cmd": "list" })).await.unwrap();
    let (port, saved) = (vm.port(), vm_host.saved.lock().unwrap().clone());
    vm.close().await;
    drop(vm);
    let (again, _) = machine_with("ignored", saved, port, secret).await;
    mbp.addr("linux-vm", &format!("127.0.0.1:{}", again.port())).unwrap();
    mbp.call("linux-vm", json!({ "id": 2, "cmd": "list" })).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_machine_reached_first_by_a_ping_still_learns_a_revocation() {
    let (mbp, _) = machine("mbp", json!({})).await;
    let secret = SecretKey::generate();
    let (vm, vm_host) = machine_with("linux-vm", json!({}), 0, secret.clone()).await;
    let (mini, _) = machine("mini", json!({})).await;
    pair(&mbp, &vm).await.unwrap();
    pair(&mbp, &mini).await.unwrap();
    let stale = vm_host.saved.lock().unwrap().clone();
    vm.close().await;
    drop(vm);
    mbp.revoke("mini").await.unwrap();
    let (vm, _) = machine_with("ignored", stale, 0, secret).await;
    assert!(names(&vm).contains(&("mini".to_string(), true)), "the stale copy still trusts mini");
    mbp.addr("linux-vm", &format!("127.0.0.1:{}", vm.port())).unwrap();
    mini.addr("linux-vm", &format!("127.0.0.1:{}", vm.port())).unwrap();
    mbp.ping("linux-vm").await.unwrap();
    assert!(names(&vm).contains(&("mini".to_string(), false)), "{:?}", names(&vm));
    assert_eq!(mini.call("linux-vm", json!({ "id": 1, "cmd": "list" })).await.unwrap_err().code, "revoked");
}

fn line(stream: &UnixStream) -> Option<Value> {
    let mut text = String::new();
    BufReader::new(stream).read_line(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

async fn hello(client: &Client, node: &Node, cert: &Value) -> Result<UnixStream, kanna_net::Failure> {
    let link = client.link(&node.key(), &[format!("127.0.0.1:{}", node.port())], None, cert).await?;
    (&link).write_all(b"{\"id\":0,\"cmd\":\"app.hello\",\"args\":{\"client\":\"mbp\"}}\n").unwrap();
    Ok(link)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_certified_by_a_paired_machine_holds_a_link_until_its_machine_is_revoked() {
    let (mbp, _) = machine("mbp", json!({})).await;
    let (vm, vm_host) = machine("linux-vm", json!({})).await;
    pair(&mbp, &vm).await.unwrap();
    let client = Client::start(SecretKey::generate()).await.unwrap();
    let cert = mbp.certify(&client.key()).unwrap();

    let link = hello(&client, &vm, &cert).await.unwrap();
    let served = loop {
        if let Some(app) = vm_host.apps.lock().unwrap().pop() {
            break app;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    let (from, hello_line, daemon_end) = served;
    assert_eq!((from.as_str(), &hello_line["args"]["client"]), ("mbp", &json!("mbp")));
    (&daemon_end).write_all(b"{\"id\":0,\"ok\":true}\n").unwrap();
    let reply = tokio::task::spawn_blocking({
        let link = link.try_clone().unwrap();
        move || line(&link)
    });
    assert_eq!(reply.await.unwrap().unwrap()["ok"], true);
    (&link).write_all(b"{\"cmd\":\"input\"}\n").unwrap();
    let got = tokio::task::spawn_blocking({
        let end = daemon_end.try_clone().unwrap();
        move || line(&end)
    });
    assert_eq!(got.await.unwrap().unwrap()["cmd"], "input");
    let path = client.path(&vm.key());
    assert_eq!((path["path"].clone(), path["lan"].clone()), (json!("direct"), json!(true)), "{path}");

    let stranger = Client::start(SecretKey::generate()).await.unwrap();
    assert_eq!(hello(&stranger, &vm, &json!(null)).await.unwrap_err().code, "unpaired");
    let forged = mbp.certify(&client.key()).unwrap();
    assert_eq!(hello(&stranger, &vm, &forged).await.unwrap_err().code, "unpaired", "a certificate is for one endpoint");

    vm.revoke("mbp").await.unwrap();
    let after = tokio::task::spawn_blocking(move || line(&daemon_end));
    assert_eq!(after.await.unwrap(), None, "the bridge closes as soon as its machine is revoked");
    assert_eq!(hello(&client, &vm, &cert).await.unwrap_err().code, "revoked");
}


#[tokio::test(flavor = "multi_thread")]
async fn a_port_on_a_paired_machines_loopback_opens_on_the_apps_loopback_until_its_machine_is_revoked() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mbp, _) = machine("mbp", json!({})).await;
    let (vm, _) = machine("linux-vm", json!({})).await;
    pair(&mbp, &vm).await.unwrap();
    let client = Client::start(SecretKey::generate()).await.unwrap();
    let cert = mbp.certify(&client.key()).unwrap();
    let addrs = [format!("127.0.0.1:{}", vm.port())];

    let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = server.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = server.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = tcp.read(&mut buf).await {
                    if n == 0 || tcp.write_all(&[b"echo:", &buf[..n]].concat()).await.is_err() {
                        break;
                    }
                }
            });
        }
    });

    let local = client.port_open(&vm.key(), &addrs, None, &cert, port).await.unwrap();
    assert_ne!(local, port);
    assert_eq!(client.port_open(&vm.key(), &addrs, None, &cert, port).await.unwrap(), local, "one listener per machine and port");
    assert_eq!(client.ports_of(&vm.key()), [(port, local)]);
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", local)).await.unwrap();
    tcp.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").await.unwrap();
    let mut got = vec![0u8; 5 + 35];
    tcp.read_exact(&mut got).await.unwrap();
    assert_eq!(got, b"echo:GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    let big = vec![b'x'; 4 << 20];
    let (mut read, mut write) = tcp.into_split();
    let writing = tokio::spawn(async move { write.write_all(&big).await.unwrap(); write });
    let mut total = 0;
    let mut buf = vec![0u8; 1 << 16];
    while total < 4 << 20 {
        let n = read.read(&mut buf).await.unwrap();
        assert!(n > 0);
        total += buf[..n].iter().filter(|b| **b == b'x').count();
    }
    let write = writing.await.unwrap();
    let mut tcp = read.reunite(write).unwrap();

    client.port_close(&vm.key());
    assert_eq!(client.ports_of(&vm.key()), []);
    let after = tokio::time::timeout(std::time::Duration::from_secs(10), tcp.read(&mut buf)).await.unwrap().unwrap_or(0);
    assert_eq!(after, 0, "closing a machine's ports ends its open tunnels");
    let mut listening = true;
    for _ in 0..100 {
        listening = tokio::net::TcpStream::connect(("127.0.0.1", local)).await.is_ok();
        if !listening {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!listening, "a closed machine's ports stop listening");

    let halves = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let half_port = halves.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut tcp, _) = halves.accept().await.unwrap();
        let mut got = vec![];
        tcp.read_to_end(&mut got).await.unwrap();
        tcp.write_all(format!("read {} bytes", got.len()).as_bytes()).await.unwrap();
    });
    let half = client.port_open(&vm.key(), &addrs, None, &cert, half_port).await.unwrap();
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", half)).await.unwrap();
    tcp.write_all(b"request body").await.unwrap();
    tcp.shutdown().await.unwrap();
    let mut answer = String::new();
    tokio::time::timeout(std::time::Duration::from_secs(10), tcp.read_to_string(&mut answer)).await.unwrap().unwrap();
    assert_eq!(answer, "read 12 bytes", "a half-closed connection still gets its answer");

    let local = client.port_open(&vm.key(), &addrs, None, &cert, port).await.unwrap();
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", local)).await.unwrap();
    tcp.write_all(b"ping").await.unwrap();
    let mut got = [0u8; 9];
    tcp.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"echo:ping");

    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    let nothing = client.port_open(&vm.key(), &addrs, None, &cert, closed).await.unwrap();
    let mut refused = tokio::net::TcpStream::connect(("127.0.0.1", nothing)).await.unwrap();
    assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(10), refused.read(&mut buf)).await.unwrap().unwrap_or(0), 0, "nothing listening there closes the connection");

    let stranger = Client::start(SecretKey::generate()).await.unwrap();
    let theirs = stranger.port_open(&vm.key(), &addrs, None, &json!(null), port).await.unwrap();
    let mut rejected = tokio::net::TcpStream::connect(("127.0.0.1", theirs)).await.unwrap();
    let _ = rejected.write_all(b"hello").await;
    assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(10), rejected.read(&mut buf)).await.unwrap().unwrap_or(0), 0, "an unpaired app gets nothing");

    vm.revoke("mbp").await.unwrap();
    let after = tokio::time::timeout(std::time::Duration::from_secs(10), tcp.read(&mut buf)).await.unwrap().unwrap_or(0);
    assert_eq!(after, 0, "an open tunnel closes as soon as its machine is revoked");
}

#[tokio::test(flavor = "multi_thread")]
async fn another_protocol_is_refused_with_its_fix() {
    let (mbp, _) = machine("mbp", json!({})).await;
    let (vm, vm_host) = server("linux-vm", json!({}), 0, SecretKey::generate(), linux(&"a".repeat(40), 5)).await;
    pair(&mbp, &vm).await.unwrap();
    let refused = mbp.call("linux-vm", json!({ "id": 1, "cmd": "list" })).await.unwrap_err();
    assert_eq!(refused.code, "other_protocol");
    assert_eq!(refused.message, "linux-vm runs Kanna 4.0.0 (aaaaaaaa), protocol 5, and this machine speaks 4; this machine is older: update it");
    assert!(vm_host.served.lock().unwrap().is_empty());
    let back = vm.call("mbp", json!({ "id": 1, "cmd": "list" })).await.unwrap_err();
    assert_eq!(back.code, "other_protocol");
    assert!(back.message.ends_with("protocol 4, and this machine speaks 5; it's older: update Kanna on mbp"), "{}", back.message);
}

#[derive(Debug)]
struct Token(&'static str);

impl iroh_relay::server::AccessControl for Token {
    async fn on_connect(&self, request: &iroh_relay::server::ClientRequest) -> iroh_relay::server::Access {
        match request.auth_token() {
            Some(token) if token == self.0 => iroh_relay::server::Access::Allow,
            _ => iroh_relay::server::Access::Deny { reason: None },
        }
    }
}

async fn relay(token: Option<&'static str>) -> (iroh_relay::server::Server, String) {
    let mut relay = iroh_relay::server::RelayConfig::new(std::net::SocketAddr::from(([127, 0, 0, 1], 0)));
    if let Some(token) = token {
        relay.access = std::sync::Arc::new(Token(token));
    }
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(relay);
    let server = iroh_relay::server::Server::spawn(config).await.unwrap();
    let url = format!("http://{}/", server.http_addr().unwrap());
    (server, url)
}

async fn reaches_relay(node: &Node, url: &str, within: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + within;
    while node.status()["home_relay"] != json!(url) {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    true
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_given_only_a_key_reaches_a_machine_through_its_relay() {
    let (_server, url) = relay(None).await;
    let (mbp, mbp_host) = machine("mbp", json!({})).await;
    let (vm, vm_host) = machine("linux-vm", json!({})).await;
    pair(&mbp, &vm).await.unwrap();
    assert_eq!(vm.set_relay(Some("ftp://nowhere"), None).await.unwrap_err().code, "bad_request");
    assert_eq!(vm.set_relay(Some(&url), None).await.unwrap()["relay"], json!(url));
    mbp.set_relay(Some(&url), None).await.unwrap();
    assert_eq!(vm_host.saved.lock().unwrap()["relay"], json!(url));
    assert!(reaches_relay(&vm, &url, std::time::Duration::from_secs(10)).await, "the machine never connected to its relay");
    let peers = mbp.status()["peers"].clone();
    let them = peers.as_array().unwrap().iter().find(|p| p["name"] == "linux-vm").unwrap();
    assert_eq!(them["relay"], json!(url), "{them}");

    let client = Client::start(SecretKey::generate()).await.unwrap();
    let cert = mbp.certify(&client.key()).unwrap();
    let open = Relay::new(&url, None).unwrap();
    let link = client.link(&vm.key(), &[], Some(&open), &cert).await.unwrap();
    (&link).write_all(b"{\"id\":0,\"cmd\":\"app.hello\",\"args\":{}}\n").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while vm_host.apps.lock().unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline, "the hello never came through the relay");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let (_guarded, moved) = relay(Some("s3cret")).await;
    vm.set_relay(Some(&moved), Some("wrong")).await.unwrap();
    assert!(!reaches_relay(&vm, &moved, std::time::Duration::from_secs(2)).await, "a relay that wants a token turns away the wrong one");
    assert_eq!(vm.set_relay(Some(&moved), Some("s3cret\n")).await.unwrap()["token"], true);
    mbp.set_relay(Some(&moved), Some("s3cret")).await.unwrap();
    assert!(reaches_relay(&vm, &moved, std::time::Duration::from_secs(10)).await, "the machine never moved to its new relay");
    let status = mbp.status();
    let them = status["peers"].as_array().unwrap().iter().find(|p| p["name"] == "linux-vm").unwrap();
    assert_eq!(them["relay"], json!(moved), "the relay set now, not the one in the peer log: {them}");
    assert_eq!((status["relay_token"].clone(), mbp.relay_token()), (json!(true), Some("s3cret".to_string())), "the status says only that there is a token");
    assert!(!json!(vm_host.saved.lock().unwrap()["entries"]).to_string().contains("s3cret"), "the token never goes in the peer log");
    let fresh = Client::start(SecretKey::generate()).await.unwrap();
    let cert = mbp.certify(&fresh.key()).unwrap();
    let guarded = Relay::new(&moved, Some("s3cret")).unwrap();
    fresh.link(&vm.key(), &[], Some(&guarded), &cert).await.unwrap();

    let saved = mbp_host.saved.lock().unwrap().clone();
    mbp.close().await;
    let (again, _) = machine("mbp", saved).await;
    assert_eq!((again.status()["relay"].clone(), again.relay_token()), (json!(moved), Some("s3cret".to_string())), "the relay and its token are kept across a restart");
    assert_eq!(again.set_relay(None, None).await.unwrap()["relay"], Value::Null);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_vm_enrolled_by_token_through_a_relay_is_hosted_and_admits_no_one() {
    let (_server, url) = relay(None).await;
    let (mbp, mbp_host) = machine("mbp", json!({})).await;
    let (vm, _) = machine("cloud-1", json!({})).await;
    let (other, _) = machine("other", json!({})).await;
    mbp.set_relay(Some(&url), None).await.unwrap();
    vm.set_relay(Some(&url), None).await.unwrap();
    assert!(reaches_relay(&vm, &url, std::time::Duration::from_secs(10)).await, "the VM never connected to its relay");
    assert_eq!(vm.offer(Some("short")).unwrap_err().code, "bad_request");
    let token = "Tk7q2mZp9xL4vR8sW1nB3c";
    let offer = vm.offer(Some(token)).unwrap();
    assert_eq!((offer["code"].clone(), offer["token"].clone(), offer["home_relay"].clone()), (Value::Null, json!(true), json!(url)));
    assert_eq!(vm.status()["offer"]["code"], Value::Null, "the status never shows a token");
    let waiting = {
        let vm = vm.clone();
        tokio::spawn(async move { vm.wait_offer().await })
    };
    assert_eq!(mbp.pair("Tk7q2mZp9xL4vR8sW1nB3d", Some(&vm.key()), true).await.unwrap_err().code, "pake_confirmation_failed");
    let paired = mbp.pair(token, Some(&vm.key()), true).await.unwrap();
    assert_eq!(paired["name"], "cloud-1");
    assert_eq!(waiting.await.unwrap().unwrap()["name"], "mbp");
    let asked = mbp_host.asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!((asked["name"].clone(), asked["hosted"].clone()), (json!("cloud-1"), json!(true)));
    for node in [&mbp, &vm] {
        let status = node.status();
        let peer = status["peers"].as_array().unwrap().iter().find(|p| p["name"] == "cloud-1").unwrap().clone();
        assert_eq!((peer["trusted"].clone(), peer["hosted"].clone()), (json!(true), json!(true)), "{peer}");
        let me = status["peers"].as_array().unwrap().iter().find(|p| p["name"] == "mbp").unwrap().clone();
        assert_eq!(me["hosted"], false);
    }
    mbp.call("cloud-1", json!({ "id": 1, "cmd": "list" })).await.unwrap();

    other.offer(None).unwrap();
    let code = other.status()["offer"]["code"].as_str().unwrap().to_string();
    let refused = vm.pair(&code, Some(&peer(&other)), false).await.unwrap_err();
    assert_eq!(refused.code, "hosted_signer", "{refused:?}");
    assert_eq!(other.status()["paired"], false);
    assert_eq!(vm.revoke("mbp").await.unwrap_err().code, "hosted_signer");
    let through = vm.call("mbp", json!({ "id": 3, "cmd": "machine", "args": { "action": "revoke", "host": "other" } })).await.unwrap();
    assert_eq!(through["error"]["code"], "hosted_signer", "{through}");
    let listed = vm.call("mbp", json!({ "id": 4, "cmd": "machine", "args": { "action": "status" } })).await.unwrap();
    assert_eq!(listed["ok"], true, "{listed}");
    mbp.call("cloud-1", json!({ "id": 2, "cmd": "list" })).await.unwrap();
}
