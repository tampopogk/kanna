mod support;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use support::{cli, Daemon, Run};

const ORIGIN: &str = "https://example.com/team/notes.git";

fn pair(joiner: &Daemon, offerer: &Daemon) {
    let app = joiner.app();
    let mut waiting = cli(&offerer.dir).env("KANNA_SOCKET", offerer.socket()).args(["machine", "pair", "--json"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines = BufReader::new(waiting.stdout.take().unwrap()).lines();
    let offer: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let peer = format!("{}@127.0.0.1:{}", offer["key"].as_str().unwrap(), offer["port"]);
    joiner.kanna(&["machine", "pair", offer["code"].as_str().unwrap(), "--peer", &peer]).ok();
    assert!(waiting.wait().unwrap().success());
    app.quit();
}

fn machine(name: &str) -> Daemon {
    let daemon = Daemon::start();
    daemon.kanna(&["machine", "name", name]).ok();
    daemon
}

fn key(daemon: &Daemon) -> String {
    daemon.kanna(&["machine", "--json"]).json()["key"].as_str().unwrap().to_string()
}

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

/// A clone of the team's notes repo in this machine's own home folder, with one runner.
fn clone(daemon: &Daemon) -> PathBuf {
    let repo = daemon.dir.join("home/.kanna-v4/repos/team/notes");
    std::fs::create_dir_all(repo.join(".kanna")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "# Notes\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    git(&repo, &["remote", "add", "origin", ORIGIN]);
    std::fs::write(repo.join(".kanna/config.json"), json!({ "version": 1, "shelf": { "runners": 1 } }).to_string()).unwrap();
    daemon.kanna_in(&repo, &["task", "list"]).ok();
    repo.canonicalize().unwrap()
}

fn mock(daemon: &Daemon, name: &str) -> String {
    let dir = daemon.dir.join("agents");
    std::fs::create_dir_all(&dir).unwrap();
    let mock = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/mock-agent/mock-agent").canonicalize().unwrap();
    let _ = std::os::unix::fs::symlink(mock, dir.join(name));
    dir.join(name).to_string_lossy().into_owned()
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

fn shelf_task(daemon: &Daemon, task: &str) -> Value {
    ask(daemon, "tasks", json!({ "action": "list" }))["shelf"].as_array().unwrap().iter().find(|t| t["id"] == task).cloned().unwrap_or(Value::Null)
}

fn screen(daemon: &Daemon, pane: &str) -> String {
    ask(daemon, "read", json!({ "pane": pane }))["text"].as_str().unwrap_or_default().to_string()
}

fn record(daemon: &Daemon, task: &str) -> Value {
    ask(daemon, "replica", json!({ "action": "list" }))["records"].as_array().unwrap().iter().find(|r| r["task"] == task).cloned().unwrap_or(Value::Null)
}

fn claude_project(folder: &str) -> String {
    folder.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Runs a task on mbp until its turn ends and Kanna stops it, with work left uncommitted and a tab open.
fn stopped_task(mbp: &Daemon, repo: &Path, agent: &str) -> (String, String, String) {
    let task = new_task(mbp, repo, &["Notes", "work 1", "--agent", agent, "--on", "mbp"]);
    within(30, "the task to start on mbp", || shelf_task(mbp, &task)["held"].is_string());
    let entry = shelf_task(mbp, &task);
    let (folder, pane) = (entry["folder"].as_str().unwrap().to_string(), entry["held"].as_str().unwrap().to_string());
    let mut session = String::new();
    within(30, "the agent to say its session", || {
        session = screen(mbp, &pane).lines().find_map(|l| l.trim().strip_prefix("session ").map(String::from)).unwrap_or_default();
        !session.is_empty()
    });
    std::fs::write(Path::new(&folder).join("draft.md"), "half done\n").unwrap();
    std::fs::write(Path::new(&folder).join("README.md"), "# Notes\n\nchanged\n").unwrap();
    let opened = ask(mbp, "open", json!({ "type": "md", "path": format!("{folder}/draft.md"), "from": pane, "tab": true }));
    assert_eq!(opened["ok"], true, "{opened}");
    (task, folder, session)
}

#[test]
fn a_stopped_claude_task_and_a_stopped_codex_task_move_to_another_machine_and_carry_on_there() {
    let (mbp, vm) = (machine("mbp"), machine("vm"));
    pair(&mbp, &vm);
    let (here, there) = (clone(&mbp), clone(&vm));

    let claude = mock(&mbp, "claude");
    mock(&vm, "claude");
    let (task, folder, session) = stopped_task(&mbp, &here, &claude);
    let transcript = mbp.dir.join("home/.claude/projects").join(claude_project(&folder)).join(format!("{session}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, format!("{{\"sessionId\":\"{session}\",\"cwd\":\"{folder}\"}}\n")).unwrap();

    let moved = mbp.kanna(&["task", "move", &task, "vm", "--json"]).json();
    let there_folder = moved["folder"].as_str().unwrap_or_else(|| panic!("{moved}")).to_string();
    assert!(there_folder.starts_with(&there.to_string_lossy().into_owned()), "vm uses its own clone's path: {there_folder}");
    assert_eq!(git(Path::new(&there_folder), &["diff", "--cached", "--name-status"]), "M\tREADME.md\nA\tdraft.md", "the uncommitted work comes back staged");
    assert_eq!(git(Path::new(&there_folder), &["log", "-1", "--format=%s"]), "first", "the work-in-progress commit is undone");
    let installed = vm.dir.join("home/.claude/projects").join(claude_project(&there_folder)).join(format!("{session}.jsonl"));
    assert_eq!(std::fs::read(&installed).unwrap(), std::fs::read(&transcript).unwrap(), "the transcript is where vm's claude looks for it");

    let entry = shelf_task(&vm, &task);
    let pane = entry["held"].as_str().unwrap().to_string();
    within(30, "vm's agent to resume the same session", || screen(&vm, &pane).contains(&format!("resumed {session}")));
    let panes = ask(&vm, "list", json!({}))["panes"].as_array().unwrap().clone();
    assert!(panes.iter().any(|p| p["type"] == "md" && p["path"] == format!("{there_folder}/draft.md")), "its tab is open again at vm's path: {panes:?}");
    assert!(shelf_task(&mbp, &task).is_null(), "the task left mbp's Shelf");
    assert!(!Path::new(&folder).exists(), "and its worktree there");
    within(10, "mbp to see vm own it", || record(&mbp, &task)["owner"].as_str() == Some(key(&vm).as_str()));
    mbp.kanna(&["task", "start", &task]).fails(4, "no task");

    let codex = mock(&mbp, "codex");
    mock(&vm, "codex");
    let (task, folder, session) = stopped_task(&mbp, &here, &codex);
    let rollout = mbp.dir.join("home/.codex/sessions/2026/11/02").join(format!("rollout-2026-11-02T10-00-00-{session}.jsonl"));
    std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
    std::fs::write(&rollout, format!("{}\n", json!({ "type": "session_meta", "payload": { "id": session, "cwd": folder } }))).unwrap();
    within(30, "the codex task's turn to end", || shelf_task(&mbp, &task)["running"] == false);
    let moved = mbp.kanna(&["task", "move", &task, "vm", "--json"]).json();
    let there_folder = moved["folder"].as_str().unwrap_or_else(|| panic!("{moved}")).to_string();
    assert_eq!(std::fs::read(vm.dir.join("home/.codex/sessions/2026/11/02").join(rollout.file_name().unwrap())).unwrap(), std::fs::read(&rollout).unwrap());
    let pane = shelf_task(&vm, &task)["held"].as_str().unwrap().to_string();
    within(30, "vm's codex to resume the same session", || screen(&vm, &pane).contains(&format!("resumed {session}")));
    assert!(Path::new(&there_folder).join("draft.md").is_file());

    let back = vm.kanna(&["task", "move", &task, "mbp", "--json"]).json();
    let home_again = back["folder"].as_str().unwrap_or_else(|| panic!("{back}")).to_string();
    assert_eq!(home_again, folder, "back on mbp it gets the worktree it had");
    assert_eq!(git(Path::new(&home_again), &["diff", "--cached", "--name-status"]), "M\tREADME.md\nA\tdraft.md");
    let pane = shelf_task(&mbp, &task)["held"].as_str().unwrap().to_string();
    within(30, "mbp's codex to resume it again", || screen(&mbp, &pane).contains(&format!("resumed {session}")));
    within(10, "vm to see mbp own it again", || record(&vm, &task)["owner"].as_str() == Some(key(&mbp).as_str()));
}

#[test]
fn a_receiver_that_stops_before_it_answers_carries_the_task_on_once_it_is_back() {
    let mbp = machine("mbp");
    let mut vm = Daemon::start_with(&[("KANNA_TRANSFER_EXIT", "installed")]);
    vm.kanna(&["machine", "name", "vm"]).ok();
    pair(&mbp, &vm);
    let (here, there) = (clone(&mbp), clone(&vm));
    let claude = mock(&mbp, "claude");
    mock(&vm, "claude");
    let (task, folder, session) = stopped_task(&mbp, &here, &claude);
    let transcript = mbp.dir.join("home/.claude/projects").join(claude_project(&folder)).join(format!("{session}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, "{}\n").unwrap();

    let waiting = mbp.kanna(&["task", "move", &task, "vm", "--json"]).json();
    assert_eq!(waiting["waiting"], true, "vm went before it answered: {waiting}");
    vm.exited();
    assert!(!shelf_task(&mbp, &task).is_null(), "mbp keeps the task until vm says it carries on");
    mbp.kanna(&["task", "start", &task]).fails(1, "is moving to vm");
    vm.unset("KANNA_TRANSFER_EXIT");
    vm.restart();
    within(60, "vm to carry the task on once it's back", || shelf_task(&vm, &task)["held"].is_string());
    let pane = shelf_task(&vm, &task)["held"].as_str().unwrap().to_string();
    within(30, "vm's agent to resume the session", || screen(&vm, &pane).contains(&format!("resumed {session}")));
    within(30, "mbp to let it go", || shelf_task(&mbp, &task).is_null());
    let worktrees: Vec<_> = std::fs::read_dir(there.join(".kanna-worktrees")).unwrap().flatten().collect();
    assert_eq!(worktrees.len(), 1, "the transfer sent again doesn't make a second worktree");
}

#[test]
fn a_task_a_machine_cant_take_stays_where_it_was_as_it_was() {
    let (mbp, vm) = (machine("mbp"), machine("vm"));
    pair(&mbp, &vm);
    let here = clone(&mbp);
    let claude = mock(&mbp, "claude");
    let (task, folder, session) = stopped_task(&mbp, &here, &claude);
    let transcript = mbp.dir.join("home/.claude/projects").join(claude_project(&folder)).join(format!("{session}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, "{}\n").unwrap();
    within(30, "the task's turn to end", || shelf_task(&mbp, &task)["running"] == false);
    let head = git(Path::new(&folder), &["rev-parse", "HEAD"]);

    mbp.kanna(&["task", "move", &task, "vm"]).fails(1, "has no clone");
    assert_eq!(git(Path::new(&folder), &["rev-parse", "HEAD"]), head, "nothing was committed");
    mbp.kanna(&["task", "move", &task, "nowhere"]).fails(4, "no machine \"nowhere\"");
    mbp.kanna(&["task", "start", &task]).ok();
    let pane = shelf_task(&mbp, &task)["held"].as_str().unwrap().to_string();
    within(30, "it to carry on here", || screen(&mbp, &pane).contains(&format!("resumed {session}")));
    mbp.kanna(&["task", "move", &task, "vm"]).fails(1, "has no clone");
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(shelf_task(&mbp, &task)["running"], true, "a machine that refuses before the move leaves the agent running");
}

fn moved_with_handoff(vm_env: &[(&str, &str)], break_vm: impl Fn(&Daemon)) -> (Daemon, Daemon, String, String) {
    let mbp = machine("mbp");
    let vm = Daemon::start_with(vm_env);
    vm.kanna(&["machine", "name", "vm"]).ok();
    pair(&mbp, &vm);
    let (here, _) = (clone(&mbp), clone(&vm));
    let claude = mock(&mbp, "claude");
    let (task, folder, session) = stopped_task(&mbp, &here, &claude);
    let transcript = mbp.dir.join("home/.claude/projects").join(claude_project(&folder)).join(format!("{session}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, "{}\n").unwrap();
    break_vm(&vm);
    let moved = mbp.kanna(&["task", "move", &task, "vm", "--json"]).json();
    assert!(moved["folder"].is_string(), "{moved}");
    (mbp, vm, task, session)
}

fn carries_on_from_its_handoff(vm: &Daemon, task: &str, session: &str) {
    let pane = shelf_task(vm, task)["held"].as_str().unwrap_or_else(|| panic!("{}", shelf_task(vm, task))).to_string();
    let mut text = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let cmd = ask(vm, "list", json!({}))["panes"].as_array().unwrap().iter().find(|p| p["id"] == pane.as_str()).map_or(String::new(), |p| p["cmd"].as_str().unwrap_or_default().to_string());
        text = format!("{}\n{cmd}", screen(vm, &pane));
        if text.contains("mock handoff of session") && text.contains(session) && !text.contains(&format!("resumed {session}")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let log = std::fs::read_to_string(vm.dir.join("daemon.log")).unwrap_or_default();
    let restarted = log.contains(&format!("{task}'s claude session didn't resume after the move, so it starts again from its handoff"));
    assert!(restarted || (text.contains("mock handoff of session") && !text.contains(&format!("resumed {session}"))), "vm's agent starts from the handoff: {text}\n{}", log.lines().filter(|l| l.contains(task)).collect::<Vec<_>>().join("\n"));
    let entry = shelf_task(vm, task);
    assert_ne!(entry["session"].as_str(), Some(session), "a new session: {entry}");
    assert_eq!(entry["message"], "work 1", "the task keeps its first message");
}

#[test]
fn a_session_that_cant_be_put_in_place_carries_on_from_the_handoff_its_agent_wrote() {
    let (_mbp, vm, task, session) = moved_with_handoff(&[], |vm| {
        std::fs::create_dir_all(vm.dir.join("home/.claude")).unwrap();
        std::fs::write(vm.dir.join("home/.claude/projects"), "not a folder").unwrap();
    });
    carries_on_from_its_handoff(&vm, &task, &session);
}

#[test]
fn a_session_that_doesnt_resume_carries_on_from_the_handoff_its_agent_wrote() {
    let (_mbp, vm, task, session) = moved_with_handoff(&[], |vm| std::fs::write(vm.dir.join("home/.mock-resume-fails"), "").unwrap());
    carries_on_from_its_handoff(&vm, &task, &session);
}
