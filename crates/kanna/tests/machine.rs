mod support;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::Stdio;
use support::{cli, wait_for, Daemon};

#[test]
fn a_paired_machine_runs_any_command_on_another_and_an_unpaired_or_revoked_one_is_refused() {
    let (mbp, vm, stranger) = (Daemon::start(), Daemon::start(), Daemon::start());
    let app = mbp.app();
    mbp.kanna(&["machine", "name", "mbp"]).ok();
    vm.kanna(&["machine", "name", "vm"]).ok();

    let mut waiting = cli(&vm.dir).env("KANNA_SOCKET", vm.socket()).args(["machine", "pair", "--json"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines = BufReader::new(waiting.stdout.take().unwrap()).lines();
    let offer: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let code = offer["code"].as_str().unwrap();
    assert_eq!(code.len(), 6);
    let peer = format!("{}@127.0.0.1:{}", offer["key"].as_str().unwrap(), offer["port"]);

    let wrong = if code == "000000" { "000001" } else { "000000" };
    mbp.kanna(&["machine", "pair", wrong, "--peer", &peer]).fails(1, "pake_confirmation_failed");
    assert_eq!(mbp.kanna(&["machine", "pair", code, "--peer", &peer]).ok(), "added vm to your machines");
    let paired: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(paired["name"], "mbp");
    assert!(waiting.wait().unwrap().success());
    let asked: Vec<Value> = app.heard.lock().unwrap().iter().filter(|m| m["cmd"] == "app.machine.confirm").cloned().collect();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["args"]["name"], "vm");

    let here = vm.kanna(&["task", "list"]).ok();
    assert_eq!(mbp.kanna(&["--host", "vm", "task", "list"]).ok(), here);
    mbp.kanna(&["--host", "vm", "open", "--window", "new", "--name", "far", "--no-wait"]).ok();
    assert!(vm.kanna(&["workspace", "list", "--all"]).ok().contains("far"));
    let listed = cli(&mbp.dir).env("KANNA_SOCKET", mbp.socket()).env("KANNA_HOST", "vm").args(["workspace", "list", "--all"]).output().unwrap();
    assert!(String::from_utf8_lossy(&listed.stdout).contains("far"));

    let status = mbp.kanna(&["machine", "list"]).ok();
    assert!(status.starts_with("mbp (this machine)"), "{status}");
    assert!(status.contains("vm ") && status.contains("direct"), "{status}");
    let ping = mbp.kanna(&["machine", "ping", "vm"]).ok();
    assert!(ping.starts_with("vm: direct"), "{ping}");
    assert!(ping.contains("control ok, file ok, port ok, sync ok, job ok, transfer ok") || ping.contains("streams: "), "{ping}");
    for kind in ["control", "file", "port", "sync", "job", "transfer"] {
        assert!(ping.contains(&format!("{kind} ok")), "{ping}");
    }

    stranger.kanna(&["machine", "addr", offer["key"].as_str().unwrap(), &format!("127.0.0.1:{}", offer["port"])]).ok();
    stranger.kanna(&["--host", offer["key"].as_str().unwrap(), "task", "list"]).fails(1, "refused this machine: it doesn't know this machine's key (pair the two with kanna machine pair) [unpaired]");
    stranger.kanna(&["--host", "vm", "task", "list"]).fails(4, "no machine \"vm\" among your machines");
    assert!(!vm.kanna(&["workspace", "list", "--all"]).ok().contains("stranger"));

    assert!(mbp.kanna(&["machine", "revoke", "vm"]).ok().starts_with("revoked vm for good"));
    vm.kanna(&["--host", "mbp", "task", "list"]).fails(1, "[revoked]");
    mbp.kanna(&["--host", "vm", "task", "list"]).fails(1, "vm is revoked from your machines [revoked]");
    app.quit();
}

const FAKE_CODEX: &str = r#"#!/usr/bin/env python3
import json, os, sys
home = os.environ["CODEX_HOME"]
for line in sys.stdin:
    message = json.loads(line)
    if message.get("id") == 1:
        print(json.dumps({"id": 1, "result": {}}), flush=True)
    if message.get("id") == 2:
        path = os.path.join(home, "auth.json")
        login = json.load(open(path))
        login["tokens"]["access_token"] += "r"
        json.dump(login, open(path, "w"))
        print(json.dumps({"id": 2, "result": {"account": {"type": "chatgpt"}}}), flush=True)
"#;

fn jwt() -> String {
    let encode = |text: &str| {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let bytes = text.as_bytes();
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
            for i in 0..=chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
        }
        out
    };
    format!("{}.{}.sig", encode(r#"{"alg":"RS256"}"#), encode(r#"{"exp":4102444800,"https://api.openai.com/auth":{"chatgpt_account_id":"workspace-1","chatgpt_plan_type":"pro"}}"#))
}

fn received(daemon: &Daemon) -> Vec<Value> {
    daemon.kanna(&["accounts", "--json"]).json()["received"].as_array().cloned().unwrap_or_default()
}

#[test]
fn accounts_added_on_one_machine_reach_another_and_come_off_with_removal_and_revocation() {
    use std::os::unix::fs::PermissionsExt;
    let (mbp, vm) = (Daemon::start(), Daemon::start());
    let app = mbp.app();
    let codex = mbp.dir.join("codex");
    std::fs::write(&codex, FAKE_CODEX).unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    let login = json!({ "tokens": { "access_token": jwt(), "refresh_token": "rt-kept-on-the-holder", "id_token": "id", "account_id": "workspace-1" } }).to_string();
    let added = ask(&mbp, "accounts", json!({ "action": "add", "kind": "claude-api-key", "secret": "sk-ant-api03-for-every-machine" }));
    assert_eq!(added["ok"], true, "{added}");
    let held = ask(&mbp, "accounts", json!({ "action": "add", "kind": "codex-chatgpt", "secret": login, "program": codex }));
    assert_eq!(held["account"]["holder"], mbp.kanna(&["machine", "--json"]).json()["key"], "{held}");
    assert_eq!(ask(&mbp, "accounts", json!({ "action": "supply", "version": 9, "accounts": [] }))["error"]["code"], "refused", "only another machine supplies accounts");

    pair(&mbp, &vm);
    wait_for("the accounts reach the machine that joined", || received(&vm).len() == 2);
    let listed = vm.kanna(&["accounts"]).ok();
    assert!(listed.contains("Anthropic API key, from mbp") && listed.contains("Codex, ChatGPT login, held by mbp, from mbp"), "{listed}");
    let kept = vm.dir.join("state/agent-secrets/received");
    assert_eq!(std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777, 0o600);
    let text = std::fs::read_to_string(&kept).unwrap();
    assert!(text.contains("sk-ant-api03-for-every-machine"));
    assert!(!text.contains("rt-kept-on-the-holder") && !text.contains(&jwt()), "a worker gets no login and keeps no access token");
    assert!(!vm.kanna(&["accounts", "--json"]).ok().contains("sk-ant-api03"), "accounts never lists a secret");

    let first = ask(&vm, "accounts", json!({ "action": "token" }));
    assert_eq!(first["tokens"]["accessToken"], jwt(), "{first}");
    assert_eq!(first["tokens"]["chatgptAccountId"], "workspace-1");
    let renewed = ask(&vm, "accounts", json!({ "action": "token", "previous": jwt() }));
    assert_eq!(renewed["tokens"]["accessToken"], format!("{}r", jwt()), "the holder refreshed its login for the renewal: {renewed}");
    let again = ask(&vm, "accounts", json!({ "action": "token", "previous": jwt() }));
    assert_eq!(again["tokens"]["accessToken"], format!("{}r", jwt()), "a renewal of a token already replaced gets the new one without another refresh");

    mbp.kanna(&["accounts", "remove", "codex"]).ok();
    wait_for("the removal reaches the other machine", || received(&vm).len() == 1);
    assert_eq!(ask(&vm, "accounts", json!({ "action": "token" }))["error"]["code"], "not_found");

    let revoked = mbp.kanna(&["machine", "revoke", "vm"]).ok();
    assert!(revoked.contains("it dropped the agent accounts it was given"), "{revoked}");
    assert!(received(&vm).is_empty());
    assert!(!kept.exists(), "nothing left on the revoked machine");
    app.quit();
}

fn pair(mbp: &Daemon, vm: &Daemon) -> u64 {
    mbp.kanna(&["machine", "name", "mbp"]).ok();
    vm.kanna(&["machine", "name", "vm"]).ok();
    let mut waiting = cli(&vm.dir).env("KANNA_SOCKET", vm.socket()).args(["machine", "pair", "--json"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines = BufReader::new(waiting.stdout.take().unwrap()).lines();
    let offer: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let peer = format!("{}@127.0.0.1:{}", offer["key"].as_str().unwrap(), offer["port"]);
    mbp.kanna(&["machine", "pair", offer["code"].as_str().unwrap(), "--peer", &peer]).ok();
    assert!(waiting.wait().unwrap().success());
    offer["port"].as_u64().unwrap()
}

fn ask(daemon: &Daemon, cmd: &str, args: Value) -> Value {
    let stream = UnixStream::connect(daemon.socket()).unwrap();
    writeln!(&stream, "{}", json!({ "id": 1, "cmd": cmd, "args": args })).unwrap();
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

struct Link {
    lines: std::io::Lines<BufReader<UnixStream>>,
    stream: UnixStream,
}

impl Link {
    fn send(&self, message: Value) {
        writeln!(&self.stream, "{message}").unwrap();
    }

    fn next(&mut self, what: impl Fn(&Value) -> bool) -> Value {
        loop {
            let message: Value = serde_json::from_str(&self.lines.next().expect("the link closed").unwrap()).unwrap();
            if what(&message) {
                return message;
            }
        }
    }
}

fn link(runtime: &tokio::runtime::Runtime, client: &kanna_net::Client, key: &str, port: u64, cert: &Value, hello: Value) -> (Link, Value) {
    let stream = runtime.block_on(client.link(key, &[format!("127.0.0.1:{port}")], None, cert)).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    let mut link = Link { lines: BufReader::new(stream.try_clone().unwrap()).lines(), stream };
    link.send(json!({ "id": 0, "cmd": "app.hello", "args": hello }));
    let reply = link.next(|m| m["id"] == 0);
    (link, reply)
}

#[test]
fn an_app_on_another_machine_attaches_over_the_network_and_another_protocol_leaves_the_daemon_running() {
    let (mbp, vm) = (Daemon::start(), Daemon::start());
    let app = mbp.app();
    let port = pair(&mbp, &vm);
    let vm_key = vm.kanna(&["machine", "--json"]).json()["key"].as_str().unwrap().to_string();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = runtime.block_on(kanna_net::Client::start(kanna_net::SecretKey::generate())).unwrap();
    let certified = ask(&mbp, "machine", json!({ "action": "certify", "key": client.key() }));
    assert_eq!(certified["ok"], true, "{certified}");
    let cert = certified["cert"].clone();

    let (_, refused) = link(&runtime, &client, &vm_key, port, &cert, json!({ "pid": 1, "client": "mbp", "protocol": 9, "build": "another" }));
    assert_eq!((refused["ok"].clone(), refused["error"]["code"].clone(), refused["protocol"].clone()), (json!(false), json!("other_protocol"), json!(kanna_build::PROTOCOL)), "{refused}");
    assert_eq!(vm.kanna(&["machine", "--json"]).json()["name"], "vm", "the daemon keeps running");

    vm.kanna(&["open", "--window", "new", "--name", "far", "--no-wait", "--cmd", "/bin/cat"]).ok();
    let (mut far, hello) = link(&runtime, &client, &vm_key, port, &cert, json!({ "pid": 1, "client": "mbp", "protocol": kanna_build::PROTOCOL, "build": "another", "commit": "abc" }));
    assert_eq!((hello["ok"].clone(), hello["protocol"].clone(), hello["daemon"]["machine"].clone()), (json!(true), json!(kanna_build::PROTOCOL), json!("vm")), "{hello}");
    let terminal = hello["panes"].as_array().unwrap().iter().find(|p| p["name"] == "far").unwrap()["terminal"].as_str().unwrap().to_string();
    far.send(json!({ "cmd": "attach", "args": { "terminal": terminal } }));
    far.next(|m| m["cmd"] == "snapshot" && m["args"]["terminal"] == terminal.as_str());
    far.send(json!({ "cmd": "input", "args": { "terminal": terminal, "data": "dHlwZWQgZmFyCg==" } }));
    wait_for("the typed line on vm", || vm.kanna(&["read", "far"]).ok().contains("typed far"));
    far.next(|m| m["cmd"] == "output");
    far.send(json!({ "id": 5, "cmd": "machine", "args": { "action": "certify", "key": client.key() } }));
    assert_eq!(far.next(|m| m["id"] == 5)["error"]["code"], "local_only", "a remote app can't have an endpoint certified");
    far.send(json!({ "id": 6, "cmd": "replica", "args": { "action": "grant", "task": "task-00000001", "birth": "1.0.x", "epoch": 9, "peer": client.key() } }));
    assert!(far.next(|m| m["id"] == 6).to_string().contains("replica is for another machine's daemon"), "an app can't speak for a machine");
    let called = ask(&mbp, "machine", json!({ "action": "call", "host": "vm", "cmd": "machine", "args": { "action": "certify", "key": client.key() } }));
    assert!(called.to_string().contains("local_only"), "another machine can't have an endpoint certified: {called}");
    let here = UnixStream::connect(vm.socket()).unwrap();
    here.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    let mut here = Link { lines: BufReader::new(here.try_clone().unwrap()).lines(), stream: here };
    here.send(json!({ "id": 0, "cmd": "app.hello", "args": { "pid": 2, "protocol": kanna_build::PROTOCOL } }));
    assert_eq!(here.next(|m| m["id"] == 0)["ok"], true, "the machine's own app attaches beside the remote one");
    here.send(json!({ "cmd": "attach", "args": { "terminal": terminal } }));
    here.next(|m| m["cmd"] == "snapshot" && m["args"]["terminal"] == terminal.as_str());

    let (mut second, reply) = link(&runtime, &client, &vm_key, port, &cert, json!({ "pid": 3, "client": "studio", "protocol": kanna_build::PROTOCOL }));
    assert_eq!(reply["ok"], true, "any number of remote apps: {reply}");
    second.send(json!({ "cmd": "attach", "args": { "terminal": terminal } }));
    second.next(|m| m["cmd"] == "snapshot" && m["args"]["terminal"] == terminal.as_str());

    let typed = |data: &str| json!({ "cmd": "input", "args": { "terminal": terminal, "data": data } });
    let output = |m: &Value| m["cmd"] == "output" && m["args"]["terminal"] == terminal.as_str();
    let taken = json!({ "cmd": "taken", "args": { "terminal": terminal } });
    far.send(taken.clone());
    far.send(typed("ZnJvbSBmYXIK"));
    wait_for("the remote app's line on vm", || vm.kanna(&["read", "far"]).ok().contains("from far"));
    for app in [&mut here, &mut far, &mut second] {
        app.next(&output);
        app.send(taken.clone());
    }
    here.send(typed("ZnJvbSBoZXJlCg=="));
    wait_for("the local app's line on vm", || vm.kanna(&["read", "far"]).ok().contains("from here"));
    for app in [&mut here, &mut far, &mut second] {
        app.next(&output);
        app.send(taken.clone());
    }

    let grid = |m: &Value, cols: u64, rows: u64| m["cmd"] == "grid" && m["args"]["cols"] == cols && m["args"]["rows"] == rows;
    let size = |cols: u64, rows: u64| json!({ "cmd": "resize", "args": { "terminal": terminal, "cols": cols, "rows": rows, "width": cols * 10, "height": rows * 20 } });
    here.send(size(60, 20));
    here.next(|m| grid(m, 60, 20));
    far.send(size(100, 30));
    far.next(|m| grid(m, 60, 20));
    far.send(typed("Cg=="));
    for app in [&mut here, &mut far, &mut second] {
        app.next(|m| grid(m, 100, 30));
    }
    here.send(json!({ "cmd": "used", "args": { "terminal": terminal } }));
    for app in [&mut here, &mut far, &mut second] {
        app.next(|m| grid(m, 60, 20));
    }

    let socket = vm.socket();
    let asking = std::thread::spawn(move || {
        let stream = UnixStream::connect(socket).unwrap();
        writeln!(&stream, "{}", json!({ "id": 1, "cmd": "debug.app", "args": {} })).unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    });
    let asked = here.next(|m| m["cmd"] == "debug.app");
    here.send(json!({ "id": asked["id"], "ok": true, "answered": "here" }));
    assert_eq!(asking.join().unwrap()["answered"], "here", "a request from vm's CLI goes to vm's own app");
    far.send(json!({ "id": 7, "cmd": "capabilities", "args": {} }));
    far.next(|m| {
        assert_ne!(m["cmd"], "debug.app", "a remote app never gets a request from vm's CLI");
        m["id"] == 7
    });

    drop(here);
    far.next(|m| grid(m, 100, 30));
    far.send(typed("c3RpbGwgZmFyCg=="));
    wait_for("the remote app's line after the local one left", || vm.kanna(&["read", "far"]).ok().contains("still far"));
    assert_eq!(ask(&vm, "debug.app", json!({}))["error"]["code"], "no_app", "with vm's own app gone, the CLI starts it again; a remote app doesn't stand in");

    let stranger = runtime.block_on(kanna_net::Client::start(kanna_net::SecretKey::generate())).unwrap();
    let unpaired = runtime.block_on(stranger.link(&vm_key, &[format!("127.0.0.1:{port}")], None, &cert)).unwrap_err();
    assert_eq!(unpaired.code, "unpaired");
    app.quit();
}
