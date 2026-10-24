use kanna_net::{Config, Host, Node, SecretKey, KINDS};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Machine {
    yes: Arc<Mutex<bool>>,
    asked: Arc<Mutex<Vec<Value>>>,
    saved: Arc<Mutex<Value>>,
    served: Arc<Mutex<Vec<(String, Value)>>>,
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
}

async fn machine(name: &str, saved: Value) -> (Node, Machine) {
    machine_on(name, saved, 0).await
}

async fn machine_on(name: &str, saved: Value, port: u16) -> (Node, Machine) {
    machine_with(name, saved, port, SecretKey::generate()).await
}

async fn machine_with(name: &str, saved: Value, port: u16, secret: SecretKey) -> (Node, Machine) {
    let host = Machine::default();
    *host.yes.lock().unwrap() = true;
    let config = Config { secret, saved, name: name.into(), capabilities: json!({ "os": std::env::consts::OS }), port: Some(port), mdns: false };
    (Node::start(config, Box::new(host.clone())).await.unwrap(), host)
}

fn peer(node: &Node) -> String {
    format!("{}@{}:{}", node.key(), "127.0.0.1", node.port())
}

async fn pair(inviter: &Node, newcomer: &Node) -> Result<Value, kanna_net::Failure> {
    let offer = newcomer.offer()?;
    let code = offer["code"].as_str().unwrap().to_string();
    let waiting = {
        let newcomer = newcomer.clone();
        tokio::spawn(async move { newcomer.wait_offer().await })
    };
    let paired = inviter.pair(&code, Some(&peer(newcomer))).await;
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
    let reply = vm.call("mbp", json!({ "id": 1, "cmd": "backlog", "args": {} })).await.unwrap();
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
async fn pairing_needs_the_code_and_a_yes_and_a_reload_keeps_the_fleet() {
    let (mbp, mbp_host) = machine("mbp", json!({})).await;
    let (vm, vm_host) = machine("linux-vm", json!({})).await;

    vm.offer().unwrap();
    let wrong = mbp.pair("12345", Some(&peer(&vm))).await.unwrap_err();
    assert_eq!(wrong.code, "pake_confirmation_failed");
    assert_eq!(mbp.status()["paired"], false);

    *mbp_host.yes.lock().unwrap() = false;
    let refused = pair(&mbp, &vm).await.unwrap_err();
    assert_eq!(refused.code, "gesture_required");
    assert_eq!(vm.status()["paired"], false);
    assert_eq!(mbp.status()["paired"], false);

    *mbp_host.yes.lock().unwrap() = true;
    pair(&mbp, &vm).await.unwrap();
    let again = vm.offer().unwrap_err();
    assert_eq!(again.code, "already_paired");

    let saved = vm_host.saved.lock().unwrap().clone();
    let (reloaded, _) = machine("ignored", saved.clone()).await;
    assert_eq!(reloaded.status()["name"], "linux-vm");
    assert_eq!(reloaded.status()["fleet"], vm.status()["fleet"]);
    let mut forged = saved;
    forged["entries"][0]["signature"] = json!("00".repeat(64));
    let (broken, _) = machine("x", forged).await;
    assert!(broken.broken().unwrap().contains("bad_signature"));
    assert_eq!(broken.call("mbp", json!({})).await.unwrap_err().code, "broken");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_third_machine_learns_the_fleet_and_a_revocation_reaches_it() {
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
    assert_eq!(again.port(), port);
    mbp.call("linux-vm", json!({ "id": 2, "cmd": "list" })).await.unwrap();
}
