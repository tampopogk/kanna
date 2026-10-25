mod support;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
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
    let asked: Vec<Value> = app.diffs.lock().unwrap().iter().filter(|m| m["cmd"] == "app.fleet.confirm").cloned().collect();
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

fn pair(mbp: &Daemon, vm: &Daemon) {
    let mut waiting = cli(&vm.dir).env("KANNA_SOCKET", vm.socket()).args(["fleet", "pair", "--json"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines = BufReader::new(waiting.stdout.take().unwrap()).lines();
    let offer: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let peer = format!("{}@127.0.0.1:{}", offer["key"].as_str().unwrap(), offer["port"]);
    mbp.kanna(&["fleet", "pair", offer["code"].as_str().unwrap(), "--peer", &peer]).ok();
    assert!(waiting.wait().unwrap().success());
}

#[test]
fn an_update_moves_a_machine_to_another_commit_and_its_terminals_carry_on() {
    let (a, b) = ("a".repeat(40), "b".repeat(40));
    let (mbp, vm) = (Daemon::start(), Daemon::installed(&a));
    let app = mbp.app();
    mbp.kanna(&["fleet", "name", "mbp"]).ok();
    vm.kanna(&["fleet", "name", "vm"]).ok();
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

    let updated = mbp.kanna(&["fleet", "update", "--tarball", tarball.to_str().unwrap()]).ok();
    assert!(updated.contains("vm             updated from aaaaaaaa: sent "), "{updated}");
    assert_eq!(std::fs::read_link(vm.dir.join("opt/current")).unwrap().to_str(), Some(b.as_str()));
    assert_eq!(std::fs::read_link(vm.dir.join("opt/previous")).unwrap().to_str(), Some(a.as_str()));
    let log = std::fs::read_to_string(vm.dir.join("daemon.log")).unwrap();
    assert!(log.contains(&format!("restarting as {}", vm.dir.join("opt/current/kanna-daemon").display())), "{log}");
    assert_eq!(std::fs::read_to_string(vm.dir.join("state/daemon.pid")).unwrap(), daemon_pid, "the daemon restarts in place, under the same pid");
    assert_eq!(unsafe { libc::kill(shell, 0) }, 0, "the terminal's shell kept running");
    assert!(vm.kanna(&["workspace", "list", "--all"]).ok().contains("kept"));
    let status = mbp.kanna(&["fleet"]).ok();
    assert!(status.contains("Kanna 0.1.0 (bbbbbbbb)"), "{status}");

    let again = mbp.kanna(&["fleet", "update", "vm", "--tarball", tarball.to_str().unwrap()]).ok();
    assert!(again.contains("vm             already runs it"), "{again}");
    mbp.kanna(&["fleet", "update", "mbp", "--tarball", tarball.to_str().unwrap()]).fails(2, "can't update itself");
    let mac = vm.kanna(&["fleet", "update", "mbp", "--tarball", tarball.to_str().unwrap()]);
    assert!(mac.out.contains("mbp            failed: mbp refused this machine: mbp's Kanna isn't installed in /opt/kanna"), "{}", mac.out);
    mac.fails(1, "not every machine took the update");
    app.quit();
}
