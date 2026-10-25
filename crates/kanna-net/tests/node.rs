use kanna_net::{Config, Host, Install, Node, SecretKey, Tarball, KINDS};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Machine {
    yes: Arc<Mutex<bool>>,
    asked: Arc<Mutex<Vec<Value>>>,
    saved: Arc<Mutex<Value>>,
    served: Arc<Mutex<Vec<(String, Value)>>>,
    restarts: Arc<tokio::sync::Notify>,
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

    fn restart(&self) {
        self.restarts.notify_one();
    }
}

async fn machine(name: &str, saved: Value) -> (Node, Machine) {
    machine_on(name, saved, 0).await
}

async fn machine_on(name: &str, saved: Value, port: u16) -> (Node, Machine) {
    machine_with(name, saved, port, SecretKey::generate()).await
}

async fn machine_with(name: &str, saved: Value, port: u16, secret: SecretKey) -> (Node, Machine) {
    let release = json!({ "os": std::env::consts::OS, "protocols": [1, 1] });
    server(name, saved, port, secret, release, None).await
}

async fn server(name: &str, saved: Value, port: u16, secret: SecretKey, release: Value, install: Option<Install>) -> (Node, Machine) {
    let host = Machine::default();
    *host.yes.lock().unwrap() = true;
    let capabilities = json!({ "os": release["os"], "arch": release["arch"] });
    let config = Config { secret, saved, name: name.into(), capabilities, port: Some(port), mdns: false, release, install };
    (Node::start(config, Box::new(host.clone())).await.unwrap(), host)
}

fn linux(commit: &str, protocols: [u32; 2]) -> Value {
    json!({ "commit": commit, "version": "4.0.0", "channel": "production", "os": "linux", "arch": "x86_64", "protocols": protocols })
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kanna-net-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn files(dir: &Path, release: &Value, daemon_exit: i32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir.join("kanna-skills")).unwrap();
    for file in ["kanna", "kanna-keeper", "kanna-daemon"] {
        std::fs::write(dir.join(file), format!("#!/bin/sh\nexit {daemon_exit}\n")).unwrap();
        std::fs::set_permissions(dir.join(file), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(dir.join("release.json"), release.to_string()).unwrap();
}

fn installed(prefix: &Path, commit: &str) -> Install {
    files(&prefix.join(commit), &linux(commit, [1, 1]), 0);
    std::os::unix::fs::symlink(commit, prefix.join("current")).unwrap();
    Install::detect(&prefix.join("current/kanna-daemon")).unwrap()
}

async fn tarball(dir: &Path, release: &Value, daemon_exit: i32) -> Tarball {
    let stage = dir.join("stage");
    let _ = std::fs::remove_dir_all(&stage);
    files(&stage.join("kanna"), release, daemon_exit);
    let path = dir.join(format!("Kanna-{}.tar.gz", &release["commit"].as_str().unwrap()[..8]));
    assert!(std::process::Command::new("tar").arg("-czf").arg(&path).arg("-C").arg(&stage).arg("kanna").status().unwrap().success());
    Tarball::open(&path).await.unwrap()
}

fn link(prefix: &Path, name: &str) -> String {
    std::fs::read_link(prefix.join(name)).unwrap().to_string_lossy().into_owned()
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

#[tokio::test(flavor = "multi_thread")]
async fn an_update_moves_a_machine_to_the_tarballs_commit_and_it_comes_back_there() {
    let (a, b) = ("a".repeat(40), "b".repeat(40));
    let dir = folder("update");
    let prefix = dir.join("opt");
    std::fs::create_dir_all(&prefix).unwrap();
    std::fs::create_dir_all(prefix.join("c".repeat(40))).unwrap();
    let install = installed(&prefix, &a);
    let (mbp, _) = machine("mbp", json!({})).await;
    let secret = SecretKey::generate();
    let (vm, vm_host) = server("linux-vm", json!({}), 0, secret.clone(), linux(&a, [1, 1]), Some(install.clone())).await;
    pair(&mbp, &vm).await.unwrap();
    let restarted = {
        let (vm_host, prefix, b) = (vm_host.clone(), prefix.clone(), b.clone());
        tokio::spawn(async move {
            vm_host.restarts.notified().await;
            let (port, saved) = (vm.port(), vm_host.saved.lock().unwrap().clone());
            vm.close().await;
            drop(vm);
            server("ignored", saved, port, secret, linux(&b, [1, 1]), Install::detect(&prefix.join("current/kanna-daemon"))).await.0
        })
    };
    let update = tarball(&dir, &linux(&b, [1, 1]), 0).await;
    let done = mbp.update(vec![], update).await.unwrap();
    let machine = &done["machines"][0];
    assert_eq!(machine["ok"], true, "{done}");
    assert_eq!((machine["from"].as_str(), machine["to"].as_str()), (Some(a.as_str()), Some(b.as_str())));
    assert_eq!((link(&prefix, "current"), link(&prefix, "previous")), (b.clone(), a.clone()));
    assert!(prefix.join(&b).join("kanna-skills").is_dir());
    assert!(!prefix.join("c".repeat(40)).exists(), "an install that is neither current nor previous is removed");
    let vm = restarted.await.unwrap();
    assert_eq!(mbp.status()["peers"].as_array().unwrap().iter().find(|p| p["name"] == "linux-vm").unwrap()["release"]["commit"], b);

    let again = mbp.update(vec!["linux-vm".into()], tarball(&dir, &linux(&b, [1, 1]), 0).await).await.unwrap();
    assert_eq!(again["machines"][0]["already"], true, "{again}");
    drop(vm);
}

#[tokio::test(flavor = "multi_thread")]
async fn another_protocol_is_refused_with_its_fix_but_still_takes_an_update() {
    let (a, b) = ("a".repeat(40), "b".repeat(40));
    let dir = folder("protocol");
    let prefix = dir.join("opt");
    std::fs::create_dir_all(&prefix).unwrap();
    let install = installed(&prefix, &a);
    let (mbp, _) = machine("mbp", json!({})).await;
    let (vm, vm_host) = server("linux-vm", json!({}), 0, SecretKey::generate(), linux(&a, [2, 3]), Some(install)).await;
    pair(&mbp, &vm).await.unwrap();
    let refused = mbp.call("linux-vm", json!({ "id": 1, "cmd": "list" })).await.unwrap_err();
    assert_eq!(refused.code, "other_protocol");
    assert_eq!(refused.message, "linux-vm runs Kanna 4.0.0 (aaaaaaaa), protocol 2–3, and this machine speaks 1; this machine is older: update it, or run kanna fleet update on linux-vm");
    assert!(vm_host.served.lock().unwrap().is_empty());

    let broken = mbp.update(vec![], tarball(&dir, &linux(&b, [1, 1]), 1).await).await.unwrap();
    assert_eq!(broken["machines"][0]["error"]["code"], "install_failed", "{broken}");
    assert_eq!(link(&prefix, "current"), a, "a build that doesn't run here is never switched to");
    assert!(!prefix.join(&b).exists());

    let mut other = linux(&b, [1, 1]);
    other["arch"] = json!("aarch64");
    let skipped = mbp.update(vec![], tarball(&dir, &other, 0).await).await.unwrap();
    assert_eq!(skipped["machines"][0]["skipped"], "linux x86_64: this tarball is for linux aarch64", "{skipped}");

    let (mac, _) = machine("mac", json!({})).await;
    pair(&mbp, &mac).await.unwrap();
    let named = mbp.update(vec!["mac".into()], tarball(&dir, &linux(&b, [1, 1]), 0).await).await.unwrap();
    assert_eq!(named["machines"][0]["ok"], false);
    assert!(named["machines"][0]["skipped"].as_str().unwrap().contains("Kanna.app updates it"), "{named}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_update_to_a_build_of_another_protocol_still_sees_it_come_back() {
    let (a, b) = ("a".repeat(40), "b".repeat(40));
    let dir = folder("across");
    let prefix = dir.join("opt");
    std::fs::create_dir_all(&prefix).unwrap();
    let install = installed(&prefix, &a);
    let (mbp, _) = machine("mbp", json!({})).await;
    let secret = SecretKey::generate();
    let (vm, vm_host) = server("linux-vm", json!({}), 0, secret.clone(), linux(&a, [1, 1]), Some(install)).await;
    pair(&mbp, &vm).await.unwrap();
    let restarted = {
        let (vm_host, prefix, b) = (vm_host.clone(), prefix.clone(), b.clone());
        tokio::spawn(async move {
            vm_host.restarts.notified().await;
            let (port, saved) = (vm.port(), vm_host.saved.lock().unwrap().clone());
            vm.close().await;
            drop(vm);
            server("ignored", saved, port, secret, linux(&b, [2, 2]), Install::detect(&prefix.join("current/kanna-daemon"))).await.0
        })
    };
    let done = mbp.update(vec![], tarball(&dir, &linux(&b, [2, 2]), 0).await).await.unwrap();
    assert_eq!(done["machines"][0]["ok"], true, "{done}");
    let _vm = restarted.await.unwrap();
    assert_eq!(mbp.call("linux-vm", json!({ "id": 1, "cmd": "list" })).await.unwrap_err().code, "other_protocol");
}
