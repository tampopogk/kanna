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
    mbp.kanna(&["fleet", "name", "mbp"]).ok();
    vm.kanna(&["fleet", "name", "vm"]).ok();

    let mut waiting = cli(&vm.dir).env("KANNA_SOCKET", vm.socket()).args(["fleet", "pair", "--json"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines = BufReader::new(waiting.stdout.take().unwrap()).lines();
    let offer: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let code = offer["code"].as_str().unwrap();
    assert_eq!(code.len(), 6);
    let peer = format!("{}@127.0.0.1:{}", offer["key"].as_str().unwrap(), offer["port"]);

    let wrong = if code == "000000" { "000001" } else { "000000" };
    mbp.kanna(&["fleet", "pair", wrong, "--peer", &peer]).fails(1, "pake_confirmation_failed");
    assert_eq!(mbp.kanna(&["fleet", "pair", code, "--peer", &peer]).ok(), "added vm to your fleet");
    let paired: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(paired["name"], "mbp");
    assert!(waiting.wait().unwrap().success());
    let asked: Vec<Value> = app.heard.lock().unwrap().iter().filter(|m| m["cmd"] == "app.fleet.confirm").cloned().collect();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["args"]["name"], "vm");

    let here = vm.kanna(&["task", "list"]).ok();
    assert_eq!(mbp.kanna(&["--host", "vm", "task", "list"]).ok(), here);
    mbp.kanna(&["--host", "vm", "open", "--name", "far", "--no-wait"]).ok();
    assert!(vm.kanna(&["workspace", "list", "--all"]).ok().contains("far"));
    let listed = cli(&mbp.dir).env("KANNA_SOCKET", mbp.socket()).env("KANNA_HOST", "vm").args(["workspace", "list", "--all"]).output().unwrap();
    assert!(String::from_utf8_lossy(&listed.stdout).contains("far"));

    let status = mbp.kanna(&["fleet"]).ok();
    assert!(status.starts_with("mbp (this machine)"), "{status}");
    assert!(status.contains("vm ") && status.contains("direct"), "{status}");
    let ping = mbp.kanna(&["fleet", "ping", "vm"]).ok();
    assert!(ping.starts_with("vm: direct"), "{ping}");
    assert!(ping.contains("control ok, file ok, port ok, sync ok, job ok, transfer ok") || ping.contains("streams: "), "{ping}");
    for kind in ["control", "file", "port", "sync", "job", "transfer"] {
        assert!(ping.contains(&format!("{kind} ok")), "{ping}");
    }

    stranger.kanna(&["fleet", "addr", offer["key"].as_str().unwrap(), &format!("127.0.0.1:{}", offer["port"])]).ok();
    stranger.kanna(&["--host", offer["key"].as_str().unwrap(), "task", "list"]).fails(1, "refused this machine: it doesn't know this machine's key (pair the two with kanna fleet pair) [unpaired]");
    stranger.kanna(&["--host", "vm", "task", "list"]).fails(4, "no machine \"vm\" in your fleet");
    assert!(!vm.kanna(&["workspace", "list", "--all"]).ok().contains("stranger"));

    assert!(mbp.kanna(&["fleet", "revoke", "vm"]).ok().starts_with("revoked vm for good"));
    vm.kanna(&["--host", "mbp", "task", "list"]).fails(1, "[revoked]");
    mbp.kanna(&["--host", "vm", "task", "list"]).fails(1, "vm is revoked in your fleet [revoked]");
    app.quit();
}

fn pair(mbp: &Daemon, vm: &Daemon) -> u64 {
    mbp.kanna(&["fleet", "name", "mbp"]).ok();
    vm.kanna(&["fleet", "name", "vm"]).ok();
    let mut waiting = cli(&vm.dir).env("KANNA_SOCKET", vm.socket()).args(["fleet", "pair", "--json"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines = BufReader::new(waiting.stdout.take().unwrap()).lines();
    let offer: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let peer = format!("{}@127.0.0.1:{}", offer["key"].as_str().unwrap(), offer["port"]);
    mbp.kanna(&["fleet", "pair", offer["code"].as_str().unwrap(), "--peer", &peer]).ok();
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
    let stream = runtime.block_on(client.link(key, &[format!("127.0.0.1:{port}")], cert)).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    let mut link = Link { lines: BufReader::new(stream.try_clone().unwrap()).lines(), stream };
    link.send(json!({ "id": 0, "cmd": "app.hello", "args": hello }));
    let reply = link.next(|m| m["id"] == 0);
    (link, reply)
}

#[test]
fn an_app_on_another_machine_attaches_over_the_fleet_and_another_protocol_leaves_the_daemon_running() {
    let (mbp, vm) = (Daemon::start(), Daemon::start());
    let app = mbp.app();
    let port = pair(&mbp, &vm);
    let vm_key = vm.kanna(&["fleet", "--json"]).json()["key"].as_str().unwrap().to_string();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = runtime.block_on(kanna_net::Client::start(kanna_net::SecretKey::generate())).unwrap();
    let certified = ask(&mbp, "fleet", json!({ "action": "certify", "key": client.key() }));
    assert_eq!(certified["ok"], true, "{certified}");
    let cert = certified["cert"].clone();

    let (_, refused) = link(&runtime, &client, &vm_key, port, &cert, json!({ "pid": 1, "client": "mbp", "protocols": [7, 9], "build": "another" }));
    assert_eq!((refused["ok"].clone(), refused["error"]["code"].clone(), refused["mine"].clone()), (json!(false), json!("other_protocol"), json!([1, 1])), "{refused}");
    assert_eq!(vm.kanna(&["fleet", "--json"]).json()["name"], "vm", "the daemon keeps running");

    vm.kanna(&["open", "--name", "far", "--no-wait", "--cmd", "/bin/cat"]).ok();
    let (mut far, hello) = link(&runtime, &client, &vm_key, port, &cert, json!({ "pid": 1, "client": "mbp", "protocols": [1, 3], "build": "another", "commit": "abc" }));
    assert_eq!((hello["ok"].clone(), hello["protocol"].clone(), hello["daemon"]["machine"].clone()), (json!(true), json!(1), json!("vm")), "{hello}");
    let terminal = hello["panes"].as_array().unwrap().iter().find(|p| p["name"] == "far").unwrap()["id"].as_str().unwrap().to_string();
    far.send(json!({ "cmd": "attach", "args": { "terminal": terminal } }));
    far.next(|m| m["cmd"] == "snapshot" && m["args"]["terminal"] == terminal.as_str());
    far.send(json!({ "cmd": "input", "args": { "terminal": terminal, "data": "dHlwZWQgZmFyCg==" } }));
    wait_for("the typed line on vm", || vm.kanna(&["read", "far"]).ok().contains("typed far"));
    far.next(|m| m["cmd"] == "output");
    let here = vm.app();
    assert!(far.lines.by_ref().all(|line| line.is_ok()), "the machine's own app takes over from the remote one");
    here.quit();

    let stranger = runtime.block_on(kanna_net::Client::start(kanna_net::SecretKey::generate())).unwrap();
    let unpaired = runtime.block_on(stranger.link(&vm_key, &[format!("127.0.0.1:{port}")], &cert)).unwrap_err();
    assert_eq!(unpaired.code, "unpaired");
    app.quit();
}
