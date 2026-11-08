mod support;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use support::{cli, Daemon, Run};

fn ask(daemon: &Daemon, cmd: &str, args: Value) -> Value {
    let stream = UnixStream::connect(daemon.socket()).unwrap();
    writeln!(&stream, "{}", json!({ "id": 1, "cmd": cmd, "args": args })).unwrap();
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
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

fn shelf_task(daemon: &Daemon, task: &str) -> Value {
    listed(daemon)["shelf"].as_array().unwrap().iter().find(|t| t["id"] == task).cloned().unwrap_or(Value::Null)
}

fn screen(daemon: &Daemon, pane: &str) -> String {
    ask(daemon, "read", json!({ "pane": pane }))["text"].as_str().unwrap_or_default().to_string()
}

fn session_on(daemon: &Daemon, pane: &str, word: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let shown = screen(daemon, pane);
        if let Some(session) = shown.lines().find_map(|l| l.trim().strip_prefix(&format!("{word} ")).map(String::from)) {
            return session;
        }
        assert!(Instant::now() < deadline, "the agent in {pane} didn't say `{word}`:\n{shown}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn key(daemon: &Daemon) -> String {
    daemon.kanna(&["machine", "--json"]).json()["key"].as_str().unwrap_or_default().to_string()
}

#[test]
fn a_kanna_moves_to_another_instance_with_its_waiting_started_and_desk_tasks() {
    let mut old = Daemon::start();
    let home = old.dir.join("home");
    let mut new = Daemon::start_with(&[("HOME", home.to_str().unwrap())]);
    let to = new.dir.join("state");
    let to = to.to_str().unwrap();
    let repo = repo(&old);
    let claude = mock(&old);

    let started = new_task(&old, &repo, &["Started", "work 1", "--agent", &claude]);
    let pane = old.kanna(&["--json", "task", "start", &started]).json()["pane"].as_str().unwrap().to_string();
    let started_session = session_on(&old, &pane, "session");
    let started_folder = shelf_task(&old, &started)["folder"].as_str().unwrap().to_string();
    std::fs::write(Path::new(&started_folder).join("draft.md"), "half done\n").unwrap();

    let desk = new_task(&old, &repo, &["Desk", "work 2", "--agent", &claude]);
    let desk_pane = old.kanna(&["--json", "task", "pick", &desk]).json()["pane"].as_str().unwrap().to_string();
    let desk_session = session_on(&old, &desk_pane, "session");

    let waiting = new_task(&old, &repo, &["Waiting", "work 3", "--agent", &claude, "--after", &started]);
    old.kanna(&["task", "send", &waiting, "a note for later"]).ok();
    let machine = key(&old);
    assert!(!machine.is_empty());

    let plan = old.kanna(&["instances", "move", to, "--dry-run", "--json"]).json();
    assert_eq!(plan["tasks"], json!({ "waiting": 1, "started": 1, "desk": 1 }), "{plan}");
    assert_eq!(plan["refused"], json!([{ "code": "running", "message": "state's Kanna is running: quit it (or stop its served daemon) first; its terminals stay" }]), "the receiving Kanna runs: {plan}");
    let mut quitting: Vec<&str> = plan["quit"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    quitting.sort();
    assert_eq!(quitting, ["Desk", "Started"], "{plan}");
    old.kanna(&["instances", "move", to]).fails(1, "state's Kanna is running");
    assert_eq!(listed(&old)["desk"].as_array().map(Vec::len), Some(1), "a refused move changes nothing");

    new.stop();
    let plan = old.kanna(&["instances", "move", to, "--dry-run", "--json"]).json();
    assert_eq!(plan["refused"], json!([]), "{plan}");
    let text = old.kanna(&["instances", "move", to, "--dry-run"]).ok();
    assert!(text.contains("1 task waiting, 1 started, 1 from the Desk onto the Shelf") && text.contains("Nothing stops it"), "{text}");

    let blocker = Path::new(to).join("kanna.db.moving");
    std::fs::create_dir_all(blocker.join("in-the-way")).unwrap();
    old.kanna(&["instances", "move", to]).fails(1, "nothing moved");
    assert!(old.exited().success(), "a failed move restarts the old Kanna");
    std::fs::remove_dir_all(&blocker).unwrap();
    old.restart();
    assert_eq!(key(&old), machine, "a failed move leaves the old Kanna the same machine");
    assert!(!Path::new(to).join("device.key.moving").exists(), "the receiver got nothing");
    within(30, "the quit agents to start again", || shelf_task(&old, &started)["held"].is_string());
    let again = shelf_task(&old, &started)["held"].as_str().unwrap().to_string();
    assert_eq!(session_on(&old, &again, "resumed"), started_session, "the started task resumed in the old Kanna");

    let moved = old.kanna(&["instances", "move", to, "--json"]).json();
    assert_eq!(moved["moved"], json!({ "waiting": 1, "started": 1, "desk": 1 }), "{moved}");
    assert!(old.exited().success(), "the old Kanna stops once its work has moved");
    let log = std::fs::read_to_string(old.dir.join("daemon.log")).unwrap();
    assert!(log.contains("moved to") && log.contains("1 waiting, 1 started, 1 from a Desk"), "{log}");

    let shared = new.socket().parent().unwrap().join("mock-conversations");
    std::fs::create_dir_all(&shared).unwrap();
    for entry in std::fs::read_dir(old.socket().parent().unwrap().join("mock-conversations")).unwrap().flatten() {
        std::fs::copy(entry.path(), shared.join(entry.file_name())).unwrap();
    }
    new.restart();
    assert_eq!(key(&new), machine, "the new Kanna is the same machine");
    assert_eq!(listed(&new)["desk"].as_array().map(Vec::len).unwrap_or(0), 0, "nothing on its Desk");
    for (task, folder, session) in [(&started, Some(started_folder.as_str()), &started_session), (&desk, None, &desk_session)] {
        within(30, &format!("{task} to start again"), || shelf_task(&new, task)["held"].is_string());
        let entry = shelf_task(&new, task);
        if let Some(folder) = folder {
            assert_eq!(entry["folder"], folder, "the same worktree: {entry}");
        }
        assert_eq!(session_on(&new, entry["held"].as_str().unwrap(), "resumed"), *session, "the same conversation, resumed: {entry}");
    }
    assert_eq!(git(Path::new(&started_folder), &["status", "--porcelain"]), "?? draft.md", "the worktree is as it was");
    let entry = shelf_task(&new, &waiting);
    assert_eq!((entry["message"].clone(), entry["after"].clone(), entry["held"].clone(), entry["folder"].clone()), (json!("work 3"), json!([started]), Value::Null, Value::Null), "the waiting task as it was: {entry}");
    let messages = new.kanna(&["task", "messages", &waiting]).ok();
    assert!(messages.contains("a note for later"), "{messages}");
    let events = ask(&new, "tasks", json!({ "action": "events", "task": desk }));
    let moved_event = events["events"].as_array().unwrap().iter().find(|e| e["what"] == "moved").cloned().unwrap_or(Value::Null);
    assert_eq!((moved_event["from"].clone(), moved_event["to"].clone()), (json!("desk"), json!("shelf")), "{events}");

    old.restart();
    assert_ne!(key(&old), machine, "the old Kanna has a key of its own now");
    assert_eq!(listed(&old)["shelf"].as_array().map(Vec::len).unwrap_or(0), 0, "and an empty Shelf");
    let kept: Vec<_> = std::fs::read_dir(old.dir.join("state")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("kanna.db.moved-")).collect();
    assert!(!kept.is_empty(), "its records are kept beside");
}
