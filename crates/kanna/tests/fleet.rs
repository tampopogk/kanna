mod support;

use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::process::Stdio;
use support::{cli, Daemon};

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
