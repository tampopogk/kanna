mod support;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use support::{cli, Daemon, Run};

const KANNA: &str = env!("CARGO_BIN_EXE_kanna");

fn ask_at(socket: &Path, cmd: &str, args: Value) -> Value {
    let stream = UnixStream::connect(socket).unwrap();
    writeln!(&stream, "{}", json!({ "id": 1, "cmd": cmd, "args": args })).unwrap();
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn ask(daemon: &Daemon, cmd: &str, args: Value) -> Value {
    ask_at(&daemon.socket(), cmd, args)
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e"]).args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn repo(daemon: &Daemon) -> PathBuf {
    let repo = daemon.dir.join("home/.kanna-v4/repos/local/notes");
    std::fs::create_dir_all(repo.join(".kanna")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "# Notes\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    std::fs::write(repo.join(".kanna/config.json"), json!({ "version": 1, "shelf": { "runners": 0 } }).to_string()).unwrap();
    daemon.kanna_in(&repo, &["task", "list"]).ok();
    repo.canonicalize().unwrap()
}

fn mock(daemon: &Daemon) -> String {
    let dir = daemon.dir.join("agents");
    std::fs::create_dir_all(&dir).unwrap();
    let mock = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/mock-agent/mock-agent").canonicalize().unwrap();
    let _ = std::os::unix::fs::symlink(mock, dir.join("claude"));
    dir.join("claude").to_string_lossy().into_owned()
}

fn new_task(daemon: &Daemon, repo: &Path, args: &[&str]) -> String {
    let mut all = vec!["task", "new"];
    all.extend(args);
    all.push("--json");
    let output = cli(&daemon.dir).current_dir(repo).env("KANNA_SOCKET", daemon.socket()).args(&all).output().unwrap();
    let made: Value = serde_json::from_str(&Run::from(&all, output).ok()).unwrap();
    made["task"]["id"].as_str().unwrap().to_string()
}

fn within(seconds: u64, what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn listed(daemon: &Daemon) -> Value {
    ask(daemon, "tasks", json!({ "action": "list" }))
}

fn find(daemon: &Daemon, place: &str, task: &str) -> Value {
    listed(daemon)[place].as_array().unwrap().iter().find(|t| t["id"] == task).cloned().unwrap_or(Value::Null)
}

fn screen(daemon: &Daemon, pane: &str) -> String {
    ask(daemon, "read", json!({ "pane": pane }))["text"].as_str().unwrap_or_default().to_string()
}

fn shown(daemon: &Daemon, pane: &str, word: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let text = screen(daemon, pane);
        if let Some(found) = text.lines().rev().find_map(|l| l.trim().strip_prefix(&format!("{word} ")).map(String::from)) {
            return found;
        }
        assert!(Instant::now() < deadline, "{pane} didn't show `{word}`:\n{text}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn agent_pid(daemon: &Daemon, pane: &str, tag: &str) -> String {
    daemon.kanna(&["send", pane, &format!("run echo {tag} $PPID")]).ok();
    shown(daemon, pane, tag)
}

fn key(daemon: &Daemon) -> String {
    daemon.kanna(&["machine", "--json"]).json()["key"].as_str().unwrap_or_default().to_string()
}

fn keeper_pid(state: &Path) -> String {
    std::fs::read_to_string(state.join("keeper.pid")).unwrap_or_default().trim().to_string()
}

fn script(daemon: &Daemon, args: &[&str]) -> Run {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/move-instance.sh");
    let output = Command::new(script).args(args).env("KANNA", KANNA).env("KANNA_SOCKET", daemon.socket()).output().unwrap();
    Run::from(args, output)
}

#[test]
fn a_kanna_moves_to_another_instance_with_its_terminals_and_agents_running() {
    let mut old = Daemon::start();
    let home = old.dir.join("home");
    let mut new = Daemon::start_with(&[("HOME", home.to_str().unwrap()), ("KANNA_KEEPER_IDLE_MS", "300")]);
    let to = new.dir.join("state");
    let to_arg = to.to_str().unwrap();
    let repo = repo(&old);
    let claude = mock(&old);

    let started = new_task(&old, &repo, &["Started", "work 1", "--agent", &claude]);
    let pane = old.kanna(&["--json", "task", "start", &started]).json()["pane"].as_str().unwrap().to_string();
    let session = shown(&old, &pane, "session");
    let started_pid = agent_pid(&old, &pane, "BEFORE");
    let folder = find(&old, "shelf", &started)["folder"].as_str().unwrap().to_string();
    std::fs::write(Path::new(&folder).join("draft.md"), "half done\n").unwrap();

    let desk = new_task(&old, &repo, &["Desk", "work 2", "--agent", &claude]);
    let desk_pane = old.kanna(&["--json", "task", "pick", &desk]).json()["pane"].as_str().unwrap().to_string();
    let desk_pid = agent_pid(&old, &desk_pane, "BEFORE");

    let waiting = new_task(&old, &repo, &["Waiting", "work 3", "--agent", &claude, "--after", &started]);
    old.kanna(&["task", "send", &waiting, "a note for later"]).ok();
    let machine = key(&old);
    assert!(!machine.is_empty());

    let plan = ask(&old, "instance", json!({ "action": "move", "to": to_arg, "dry": true }));
    assert_eq!(plan["tasks"], json!({ "waiting": 1, "started": 1, "desk": 1 }), "{plan}");
    let codes: Vec<&str> = plan["refused"].as_array().unwrap().iter().filter_map(|r| r["code"].as_str()).collect();
    assert!(codes.contains(&"running"), "the receiving Kanna runs: {plan}");
    script(&old, &[to_arg]).fails(1, "is running");
    assert_eq!(listed(&old)["desk"].as_array().map(Vec::len), Some(1), "a refused move changes nothing");

    new.stop();
    within(10, "the receiver's idle keeper to stop", || ask(&old, "instance", json!({ "action": "move", "to": to_arg, "dry": true }))["refused"] == json!([]));
    let text = script(&old, &[to_arg, "--dry-run"]).ok();
    assert!(text.contains("tasks: 1 waiting, 1 started, 1 on a Desk") && !text.contains("refused"), "{text}");

    let blocker = to.join("kanna.db.moving");
    std::fs::create_dir_all(blocker.join("in-the-way")).unwrap();
    let failed = ask(&old, "instance", json!({ "action": "move", "to": to_arg }));
    assert_eq!(failed["error"]["code"], "failed", "{failed}");
    assert!(old.exited().success(), "a failed move restarts the old Kanna");
    std::fs::remove_dir_all(&blocker).unwrap();
    old.restart();
    assert_eq!(key(&old), machine, "a failed move leaves the old Kanna the same machine");
    assert!(!to.join("device.key.moving").exists() && !to.join("terminals.moving").exists(), "the receiver got nothing");
    assert_eq!(agent_pid(&old, &pane, "AGAIN"), started_pid, "its agents ran on");

    let keeper = keeper_pid(&old.dir.join("state"));
    let moved = ask(&old, "instance", json!({ "action": "move", "to": to_arg }));
    assert_eq!(moved["moved"], json!({ "waiting": 1, "started": 1, "desk": 1 }), "{moved}");
    assert_eq!(moved["warnings"], json!([]), "{moved}");
    assert!(old.exited().success(), "the old Kanna stops once its work has moved");
    assert_eq!(keeper_pid(&to), keeper, "the same keeper, in the new folder");
    assert!(!old.dir.join("state/keeper.sock").exists());
    assert!(std::fs::symlink_metadata(old.dir.join("kanna.sock")).unwrap().file_type().is_symlink(), "the old socket is a link");

    new.restart();
    assert_eq!(key(&new), machine, "the new Kanna is the same machine");
    assert_eq!(std::fs::read_link(old.dir.join("kanna.sock")).unwrap(), new.dir.join("kanna.sock"), "the link leads to the new Kanna's socket");
    assert_eq!(ask_at(&old.dir.join("kanna.sock"), "machine", json!({ "action": "status" }))["key"], json!(machine), "the old socket reaches the new Kanna");
    assert_eq!(find(&new, "desk", &desk)["id"], json!(desk), "the Desk task is still on the Desk: {}", listed(&new));
    let entry = find(&new, "shelf", &started);
    assert_eq!((entry["held"].clone(), entry["folder"].clone(), entry["session"].clone()), (json!(pane), json!(folder), json!(session)), "{entry}");
    assert_eq!(agent_pid(&new, &pane, "AFTER"), started_pid, "the same agent, still running");
    assert_eq!(agent_pid(&new, &desk_pane, "AFTER"), desk_pid);
    new.kanna(&["send", &pane, &format!("run {KANNA} task list")]).ok();
    assert_eq!(shown(&new, &pane, "RUN_DONE"), "0", "an agent's kanna reaches the new Kanna through the old socket:\n{}", screen(&new, &pane));
    assert_eq!(git(Path::new(&folder), &["status", "--porcelain"]), "?? draft.md", "the worktree is as it was");
    let entry = find(&new, "shelf", &waiting);
    assert_eq!((entry["message"].clone(), entry["after"].clone(), entry["held"].clone(), entry["folder"].clone()), (json!("work 3"), json!([started]), Value::Null, Value::Null), "the waiting task as it was: {entry}");
    let messages = new.kanna(&["task", "messages", &waiting]).ok();
    assert!(messages.contains("a note for later"), "{messages}");

    old.restart();
    assert_eq!(old.socket(), old.dir.join("kanna.own.sock"), "the old Kanna steps around the link");
    assert_ne!(key(&old), machine, "the old Kanna has a key of its own now");
    assert_eq!(listed(&old)["shelf"].as_array().map(Vec::len).unwrap_or(0), 0, "and an empty Shelf");
    assert_eq!(ask_at(&old.dir.join("kanna.sock"), "machine", json!({ "action": "status" }))["key"], json!(machine), "the link still leads to the new Kanna");
    let kept: Vec<String> = std::fs::read_dir(old.dir.join("state")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".moved-")).collect();
    assert!(kept.iter().any(|n| n.starts_with("kanna.db.moved-")) && kept.iter().any(|n| n.starts_with("terminals.moved-")), "its records are kept beside: {kept:?}");
}
