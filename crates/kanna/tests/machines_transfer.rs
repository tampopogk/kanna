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

/// A clone of the team's notes repo in this machine's own home folder, on a machine with one runner.
fn clone(daemon: &Daemon) -> PathBuf {
    let repo = daemon.dir.join("home/.kanna-v4/repos/team/notes");
    std::fs::create_dir_all(repo.join(".kanna")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "# Notes\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    git(&repo, &["remote", "add", "origin", ORIGIN]);
    std::fs::write(daemon.dir.join("home/.kanna-v4/config.json"), json!({ "version": 1, "shelf": { "runners": 1 } }).to_string()).unwrap();
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

/// Runs a task on mbp to the end of its first turn, with work left uncommitted and a tab open.
fn started_task(mbp: &Daemon, repo: &Path, agent: &str) -> (String, String, String) {
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
    within(30, "its first turn to end, so a Shelf runner stops it", || shelf_task(mbp, &task)["running"] == false);
    (task, folder, session)
}

fn resumed(daemon: &Daemon, task: &str, session: &str) {
    daemon.kanna(&["task", "start", task]).ok();
    let pane = shelf_task(daemon, task)["held"].as_str().unwrap().to_string();
    within(30, "it to carry on here", || screen(daemon, &pane).contains(&format!("resumed {session}")));
}

/// Checks that a moved task carries on on this machine from the handoff its agent wrote in `session`.
fn carries_on_from_its_handoff(daemon: &Daemon, task: &str, folder: &str, session: &str) {
    within(30, "the task to start from its handoff", || shelf_task(daemon, task)["folder"].as_str() == Some(folder) && shelf_task(daemon, task)["session"].is_string());
    let handoff = std::fs::read_to_string(Path::new(folder).join(".kanna/tmp/handoff.md")).unwrap();
    assert!(handoff.contains(&format!("mock handoff of session {session}")), "the handoff the agent wrote in its own session: {handoff}");
    let entry = shelf_task(daemon, task);
    assert_ne!(entry["session"].as_str(), Some(session), "a new session: {entry}");
    assert_eq!(entry["message"], "work 1", "the task keeps its first message");
    let log = std::fs::read_to_string(daemon.dir.join("daemon.log")).unwrap_or_default();
    assert!(log.contains(&format!("{task} carries on here from its handoff")), "{log}");
}

#[test]
fn a_claude_task_and_a_codex_task_move_to_another_machine_and_carry_on_there_from_a_handoff() {
    let (mbp, vm) = (machine("mbp"), machine("vm"));
    pair(&mbp, &vm);
    let (here, there) = (clone(&mbp), clone(&vm));

    let claude = mock(&mbp, "claude");
    let (task, folder, session) = started_task(&mbp, &here, &claude);
    resumed(&mbp, &task, &session);
    let moved = mbp.kanna(&["task", "move", &task, "vm", "--json"]).json();
    let there_folder = moved["folder"].as_str().unwrap_or_else(|| panic!("{moved}")).to_string();
    assert!(there_folder.starts_with(&there.to_string_lossy().into_owned()), "vm uses its own clone's path: {there_folder}");
    assert_eq!(git(Path::new(&there_folder), &["diff", "--cached", "--name-status"]), "M\tREADME.md\nA\tdraft.md", "the uncommitted work comes back staged");
    assert_eq!(git(Path::new(&there_folder), &["log", "-1", "--format=%s"]), "first", "the work-in-progress commit is undone");
    carries_on_from_its_handoff(&vm, &task, &there_folder, &session);
    let handoff = std::fs::read_to_string(Path::new(&there_folder).join(".kanna/tmp/handoff.md")).unwrap();
    assert!(handoff.contains("## The workspace on mbp") && handoff.contains("- the doc `draft.md`"), "the handoff says which tabs were open, at paths in the worktree: {handoff}");
    let panes = ask(&vm, "list", json!({}))["panes"].as_array().unwrap().clone();
    assert!(!panes.iter().any(|p| p["type"] == "md"), "no tab is reopened: {panes:?}");
    assert!(shelf_task(&mbp, &task).is_null(), "the task left mbp's Shelf");
    assert!(!Path::new(&folder).exists(), "and its worktree there");
    within(10, "mbp to see vm own it", || record(&mbp, &task)["owner"].as_str() == Some(key(&vm).as_str()));
    mbp.kanna(&["task", "start", &task]).fails(1, &format!("vm: {task} is running"));
    mbp.kanna(&["task", "send", &task[..9], "after the move"]).ok();
    let held = vm.kanna(&["task", "messages", &task]).ok();
    assert!(held.contains("after the move"), "the done row mbp kept doesn't stop a message reaching vm: {held}");

    let codex = mock(&mbp, "codex");
    let (task, folder, session) = started_task(&mbp, &here, &codex);
    let moved = mbp.kanna(&["task", "move", &task, "vm", "--json"]).json();
    let there_folder = moved["folder"].as_str().unwrap_or_else(|| panic!("{moved}")).to_string();
    carries_on_from_its_handoff(&vm, &task, &there_folder, &session);
    assert!(Path::new(&there_folder).join("draft.md").is_file());

    let session = shelf_task(&vm, &task)["session"].as_str().unwrap().to_string();
    within(30, "the turn on its handoff to end", || shelf_task(&vm, &task)["running"] == false);
    let back = vm.kanna(&["task", "move", &task, "mbp", "--json"]).json();
    let home_again = back["folder"].as_str().unwrap_or_else(|| panic!("{back}")).to_string();
    assert_eq!(home_again, folder, "back on mbp it gets the worktree it had");
    assert_eq!(git(Path::new(&home_again), &["diff", "--cached", "--name-status"]), "M\tREADME.md\nA\tdraft.md");
    carries_on_from_its_handoff(&mbp, &task, &home_again, &session);
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
    let (task, _, session) = started_task(&mbp, &here, &claude);

    let waiting = mbp.kanna(&["task", "move", &task, "vm", "--json"]).json();
    assert_eq!(waiting["waiting"], true, "vm went before it answered: {waiting}");
    vm.exited();
    assert!(!shelf_task(&mbp, &task).is_null(), "mbp keeps the task until vm says it carries on");
    mbp.kanna(&["task", "start", &task]).fails(1, "is moving to vm");
    vm.unset("KANNA_TRANSFER_EXIT");
    vm.restart();
    within(60, "vm to carry the task on once it's back", || shelf_task(&vm, &task)["held"].is_string());
    let there_folder = shelf_task(&vm, &task)["folder"].as_str().unwrap().to_string();
    carries_on_from_its_handoff(&vm, &task, &there_folder, &session);
    within(30, "mbp to let it go", || shelf_task(&mbp, &task).is_null());
    let worktrees: Vec<_> = std::fs::read_dir(there.join(".kanna-worktrees")).unwrap().flatten().collect();
    assert_eq!(worktrees.len(), 1, "the transfer sent again doesn't make a second worktree");
}

#[test]
fn a_task_a_machine_cant_take_or_whose_agent_writes_no_handoff_stays_where_it_was() {
    let (mbp, vm) = (machine("mbp"), machine("vm"));
    pair(&mbp, &vm);
    let here = clone(&mbp);
    let claude = mock(&mbp, "claude");
    let (task, folder, session) = started_task(&mbp, &here, &claude);
    let head = git(Path::new(&folder), &["rev-parse", "HEAD"]);
    resumed(&mbp, &task, &session);
    mbp.kanna(&["task", "move", &task, "vm"]).fails(1, "has no clone");
    assert_eq!(git(Path::new(&folder), &["rev-parse", "HEAD"]), head, "nothing was committed");
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(shelf_task(&mbp, &task)["running"], true, "a machine that refuses before the move leaves the agent running");
    mbp.kanna(&["task", "move", &task, "nowhere"]).fails(4, "no machine \"nowhere\"");

    clone(&vm);
    std::fs::write(mbp.dir.join("home/.mock-handoff-fails"), "").unwrap();
    mbp.kanna(&["task", "move", &task, "vm"]).fails(1, "wrote no handoff");
    assert_eq!(git(Path::new(&folder), &["rev-parse", "HEAD"]), head, "nothing was committed");
    assert!(shelf_task(&vm, &task).is_null());
    assert!(!shelf_task(&mbp, &task).is_null(), "it stays on mbp, stopped");
    mbp.kanna(&["task", "start", &task]).ok();
}

#[test]
fn an_agent_with_a_first_message_stays_while_nothing_says_its_first_turn_is_over() {
    let (mbp, vm) = (machine("mbp"), machine("vm"));
    pair(&mbp, &vm);
    std::fs::write(mbp.dir.join("mock-silent"), "").unwrap();
    let (here, _there) = (clone(&mbp), clone(&vm));
    let claude = mock(&mbp, "claude");
    let task = new_task(&mbp, &here, &["Notes", "work 1", "--agent", &claude, "--on", "mbp"]);
    within(30, "the task to start on mbp", || shelf_task(&mbp, &task)["held"].is_string());
    let pane = shelf_task(&mbp, &task)["held"].as_str().unwrap().to_string();
    let mut session = String::new();
    within(30, "the agent to say its session", || {
        session = screen(&mbp, &pane).lines().find_map(|l| l.trim().strip_prefix("session ").map(String::from)).unwrap_or_default();
        !session.is_empty()
    });
    within(10, "Kanna to know its agent, which reports nothing", || mbp.kanna(&["status", "explain", &pane]).out.contains("no report yet"));
    let entry = shelf_task(&mbp, &task);
    assert_eq!((&entry["status"], &entry["running"]), (&Value::Null, &json!(true)), "{entry}");

    mbp.kanna(&["task", "move", &task, "vm"]).fails(1, "in the middle of a turn");
    assert_eq!(shelf_task(&mbp, &task)["running"], true, "its agent wasn't quit");
    assert!(shelf_task(&vm, &task).is_null());
    let log = std::fs::read_to_string(mbp.dir.join("daemon.log")).unwrap_or_default();
    assert!(!log.contains("so its agent quits"), "{log}");

    mbp.kanna(&["task", "stop", &task]).ok();
    let moved = mbp.kanna(&["task", "move", &task, "vm", "--json"]).json();
    assert!(moved["folder"].is_string(), "stopped, it moves as it is: {moved}");
    assert!(std::fs::read_to_string(Path::new(moved["folder"].as_str().unwrap()).join(".kanna/tmp/handoff.md")).unwrap().contains(&session));
}
