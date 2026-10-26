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
    mbp.kanna(&["--host", "vm", "open", "--name", "far", "--no-wait"]).ok();
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
    let stream = runtime.block_on(client.link(key, &[format!("127.0.0.1:{port}")], cert)).unwrap();
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

    let (_, refused) = link(&runtime, &client, &vm_key, port, &cert, json!({ "pid": 1, "client": "mbp", "protocols": [7, 9], "build": "another" }));
    assert_eq!((refused["ok"].clone(), refused["error"]["code"].clone(), refused["mine"].clone()), (json!(false), json!("other_protocol"), json!([1, 1])), "{refused}");
    assert_eq!(vm.kanna(&["machine", "--json"]).json()["name"], "vm", "the daemon keeps running");

    vm.kanna(&["open", "--name", "far", "--no-wait", "--cmd", "/bin/cat"]).ok();
    let (mut far, hello) = link(&runtime, &client, &vm_key, port, &cert, json!({ "pid": 1, "client": "mbp", "protocols": [1, 3], "build": "another", "commit": "abc" }));
    assert_eq!((hello["ok"].clone(), hello["protocol"].clone(), hello["daemon"]["machine"].clone()), (json!(true), json!(1), json!("vm")), "{hello}");
    let terminal = hello["panes"].as_array().unwrap().iter().find(|p| p["name"] == "far").unwrap()["id"].as_str().unwrap().to_string();
    far.send(json!({ "cmd": "attach", "args": { "terminal": terminal } }));
    far.next(|m| m["cmd"] == "snapshot" && m["args"]["terminal"] == terminal.as_str());
    far.send(json!({ "cmd": "input", "args": { "terminal": terminal, "data": "dHlwZWQgZmFyCg==" } }));
    wait_for("the typed line on vm", || vm.kanna(&["read", "far"]).ok().contains("typed far"));
    far.next(|m| m["cmd"] == "output");
    far.send(json!({ "id": 5, "cmd": "machine", "args": { "action": "certify", "key": client.key() } }));
    assert_eq!(far.next(|m| m["id"] == 5)["error"]["code"], "local_only", "a remote app can't have an endpoint certified");
    let called = ask(&mbp, "machine", json!({ "action": "call", "host": "vm", "cmd": "machine", "args": { "action": "certify", "key": client.key() } }));
    assert!(called.to_string().contains("local_only"), "another machine can't have an endpoint certified: {called}");
    let here = UnixStream::connect(vm.socket()).unwrap();
    here.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    let mut here = Link { lines: BufReader::new(here.try_clone().unwrap()).lines(), stream: here };
    here.send(json!({ "id": 0, "cmd": "app.hello", "args": { "pid": 2 } }));
    assert_eq!(here.next(|m| m["id"] == 0)["ok"], true);
    assert!(far.lines.by_ref().all(|line| line.is_ok()), "the machine's own app takes over from the remote one");
    here.send(json!({ "cmd": "attach", "args": { "terminal": terminal } }));
    here.next(|m| m["cmd"] == "snapshot" && m["args"]["terminal"] == terminal.as_str());
    drop(here);

    let stranger = runtime.block_on(kanna_net::Client::start(kanna_net::SecretKey::generate())).unwrap();
    let unpaired = runtime.block_on(stranger.link(&vm_key, &[format!("127.0.0.1:{port}")], &cert)).unwrap_err();
    assert_eq!(unpaired.code, "unpaired");
    app.quit();
}

#[test]
fn an_update_moves_a_machine_to_another_commit_and_its_terminals_carry_on() {
    let (a, b) = ("a".repeat(40), "b".repeat(40));
    let (mbp, vm) = (Daemon::start(), Daemon::installed(&a));
    let app = mbp.app();
    mbp.kanna(&["machine", "name", "mbp"]).ok();
    vm.kanna(&["machine", "name", "vm"]).ok();
    pair(&mbp, &vm);
    let pid_file = vm.dir.join("shell.pid");
    vm.kanna(&["open", "--name", "kept", "--no-wait", "--cmd", &format!("echo $$ > {}; exec sleep 600", pid_file.display())]).ok();
    wait_for("the terminal's shell", || std::fs::read_to_string(&pid_file).is_ok_and(|p| p.ends_with('\n')));
    let shell: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
    let daemon_pid = std::fs::read_to_string(vm.dir.join("state/daemon.pid")).unwrap();

    let release = |commit: &str| json!({ "commit": commit, "version": "0.1.0", "channel": "dev", "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "protocols": [1, 1] });
    let stage = vm.dir.join("stage");
    Daemon::build(&stage.join("kanna"), &release(&b));
    let tarball = vm.dir.join("Kanna-test.tar.gz");
    assert!(std::process::Command::new("tar").arg("-czf").arg(&tarball).arg("-C").arg(&stage).arg("kanna").status().unwrap().success());

    let updated = mbp.kanna(&["machine", "update", "--tarball", tarball.to_str().unwrap()]).ok();
    assert!(updated.contains("vm             updated from aaaaaaaa: sent "), "{updated}");
    assert_eq!(std::fs::read_link(vm.dir.join("opt/current")).unwrap().to_str(), Some(b.as_str()));
    assert_eq!(std::fs::read_link(vm.dir.join("opt/previous")).unwrap().to_str(), Some(a.as_str()));
    let log = std::fs::read_to_string(vm.dir.join("daemon.log")).unwrap();
    assert!(log.contains(&format!("restarting as {}", vm.dir.join("opt/current/kanna-daemon").display())), "{log}");
    assert_eq!(std::fs::read_to_string(vm.dir.join("state/daemon.pid")).unwrap(), daemon_pid, "the daemon restarts in place, under the same pid");
    assert_eq!(unsafe { libc::kill(shell, 0) }, 0, "the terminal's shell kept running");
    assert!(vm.kanna(&["workspace", "list", "--all"]).ok().contains("kept"));
    let status = mbp.kanna(&["machine", "list"]).ok();
    assert!(status.contains("Kanna 0.1.0 (bbbbbbbb)"), "{status}");

    let again = mbp.kanna(&["machine", "update", "vm", "--tarball", tarball.to_str().unwrap()]).ok();
    assert!(again.contains("vm             already runs it"), "{again}");
    mbp.kanna(&["machine", "update", "mbp", "--tarball", tarball.to_str().unwrap()]).fails(2, "can't update itself");
    let mac = vm.kanna(&["machine", "update", "mbp", "--tarball", tarball.to_str().unwrap()]);
    assert!(mac.out.contains("mbp            failed: mbp refused this machine: mbp's Kanna isn't installed in /opt/kanna"), "{}", mac.out);
    mac.fails(1, "not every machine took the update");
    app.quit();
}
