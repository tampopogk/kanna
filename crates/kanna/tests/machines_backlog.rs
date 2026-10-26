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
    let mut waiting = cli(&offerer.dir).env("KANNA_SOCKET", offerer.socket()).args(["fleet", "pair", "--json"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines = BufReader::new(waiting.stdout.take().unwrap()).lines();
    let offer: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let peer = format!("{}@127.0.0.1:{}", offer["key"].as_str().unwrap(), offer["port"]);
    joiner.kanna(&["fleet", "pair", offer["code"].as_str().unwrap(), "--peer", &peer]).ok();
    assert!(waiting.wait().unwrap().success());
    app.quit();
}

fn machine(name: &str) -> Daemon {
    let daemon = Daemon::start();
    daemon.kanna(&["fleet", "name", name]).ok();
    daemon
}

fn key(daemon: &Daemon) -> String {
    daemon.kanna(&["fleet", "--json"]).json()["key"].as_str().unwrap().to_string()
}

fn ask(daemon: &Daemon, cmd: &str, args: Value) -> Value {
    let stream = UnixStream::connect(daemon.socket()).unwrap();
    writeln!(&stream, "{}", json!({ "id": 1, "cmd": cmd, "args": args })).unwrap();
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    assert!(Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e"]).args(args).current_dir(dir).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success(), "git {args:?}");
}

/// A clone of the team's notes repo on this machine, known to its daemon as a project, with this many runners.
fn clone(daemon: &Daemon, runners: usize) -> PathBuf {
    let repo = daemon.dir.join("notes");
    std::fs::create_dir_all(repo.join(".kanna")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(&repo, &["remote", "add", "origin", ORIGIN]);
    let repo = repo.canonicalize().unwrap();
    set_runners(&repo, runners);
    assert_eq!(ask(daemon, "projects", json!({ "action": "touch", "folder": repo }))["ok"], true);
    repo
}

fn set_runners(repo: &Path, runners: usize) {
    std::fs::write(repo.join(".kanna/config.json"), json!({ "version": 1, "backlog": { "runners": runners } }).to_string()).unwrap();
}

fn agent(daemon: &Daemon) -> String {
    let dir = daemon.dir.join("agents");
    std::fs::create_dir_all(&dir).unwrap();
    let mock = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/mock-agent/mock-agent").canonicalize().unwrap();
    let _ = std::os::unix::fs::symlink(mock, dir.join("codex"));
    dir.join("codex").to_string_lossy().into_owned()
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

/// The tasks a machine has started, from its own backlog.
fn started(daemon: &Daemon) -> Vec<String> {
    let listed = ask(daemon, "backlog", json!({ "action": "list" }));
    listed["tasks"].as_array().unwrap().iter().filter(|t| t["folder"].is_string()).map(|t| t["id"].as_str().unwrap().to_string()).collect()
}

fn holds(daemon: &Daemon, task: &str) -> bool {
    ask(daemon, "backlog", json!({ "action": "list" }))["tasks"].as_array().unwrap().iter().any(|t| t["id"] == task)
}

fn record(daemon: &Daemon, task: &str) -> Value {
    ask(daemon, "replica", json!({ "action": "list" }))["records"].as_array().unwrap().iter().find(|r| r["task"] == task).cloned().unwrap_or(Value::Null)
}

fn worktrees(repo: &Path) -> Vec<String> {
    std::fs::read_dir(repo.join(".kanna-worktrees")).map_or(vec![], |dir| dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
}

fn log_says(daemon: &Daemon, text: &str) -> bool {
    std::fs::read_to_string(daemon.dir.join("daemon.log")).unwrap_or_default().contains(text)
}

#[test]
fn two_machines_share_one_repo_s_backlog_and_each_task_runs_once_oldest_first() {
    let (mbp, vm) = (machine("mbp"), machine("vm"));
    pair(&mbp, &vm);
    let (here, there) = (clone(&mbp, 0), clone(&vm, 1));
    let codex = agent(&mbp);

    let first = new_task(&mbp, &here, &["First", "work 1", "--agent", &codex]);
    let second = new_task(&mbp, &here, &["Second", "work 1", "--agent", &codex]);
    let blocked = new_task(&mbp, &here, &["Blocked", "work 1", "--agent", &codex, "--after", &first]);
    let elsewhere = new_task(&mbp, &here, &["Elsewhere", "work 1", "--agent", &codex, "--on", "mini"]);

    let mut order: Vec<String> = vec![];
    within(60, "vm to run the first two of mbp's tasks", || {
        for task in started(&vm) {
            if !order.contains(&task) {
                order.push(task);
            }
        }
        order.len() >= 2
    });
    assert_eq!(order, vec![first.clone(), second.clone()], "vm takes mbp's tasks oldest first, skipping the blocked one");
    assert!(!holds(&mbp, &first) && !holds(&mbp, &second), "a task granted away leaves its owner's backlog");
    assert!(holds(&mbp, &blocked) && holds(&mbp, &elsewhere), "the blocked task and the one for mini stay with mbp");
    assert_eq!(worktrees(&here), Vec::<String>::new(), "mbp, with no runners, ran nothing");
    assert_eq!(worktrees(&there).len(), 2);

    let listed = mbp.kanna(&["task", "list"]).ok();
    assert!(listed.lines().any(|l| l.starts_with(&first[..9]) && l.contains("on vm")), "mbp lists the task vm runs: {listed}");
    within(10, "mbp to see that vm owns and started the first task", || record(&mbp, &first)["state"] == "started");
    assert_eq!(record(&mbp, &first)["owner"].as_str(), Some(key(&vm).as_str()));

    within(30, "the first task's turn to end", || ask(&vm, "backlog", json!({ "action": "list" }))["tasks"].as_array().unwrap().iter().any(|t| t["id"] == first.as_str() && t["running"] == false));
    vm.kanna(&["task", "done", &first, "--keep"]).ok();
    within(30, "vm to run the task that waited for the first", || started(&vm).contains(&blocked));
    std::thread::sleep(Duration::from_secs(4));
    assert!(holds(&mbp, &elsewhere) && !started(&vm).contains(&elsewhere), "a task for mini runs nowhere else");
    assert_eq!(worktrees(&here), Vec::<String>::new());
}

#[test]
fn two_runners_asking_at_once_get_one_grant_and_a_grant_lost_on_the_way_is_taken_once() {
    let (mbp, mut vm1, vm2) = (machine("mbp"), machine("vm1"), machine("vm2"));
    pair(&mbp, &vm1);
    pair(&mbp, &vm2);
    let (here, one, two) = (clone(&mbp, 0), clone(&vm1, 0), clone(&vm2, 0));
    let codex = agent(&mbp);
    let (key1, key2) = (key(&vm1), key(&vm2));
    let task = new_task(&mbp, &here, &["Contested", "work 1", "--agent", &codex]);
    within(15, "both runners to know the task", || record(&vm1, &task)["state"] == "waiting" && record(&vm2, &task)["state"] == "waiting");
    let birth = record(&mbp, &task)["birth"].as_str().unwrap().to_string();

    let granted = ask(&mbp, "replica", json!({ "action": "ask", "task": task, "birth": birth, "peer": key1 }));
    let refused = ask(&mbp, "replica", json!({ "action": "ask", "task": task, "birth": birth, "peer": key2 }));
    assert_eq!((granted["ok"].clone(), granted["epoch"].clone()), (json!(true), json!(1)), "{granted}");
    assert_eq!((refused["ok"].clone(), refused["error"]["code"].clone()), (json!(false), json!("not_owner")), "{refused}");

    set_runners(&one, 1);
    set_runners(&two, 1);
    within(30, "vm1 to take the grant its ask never heard back and run it", || started(&vm1).contains(&task));
    within(10, "mbp to hear vm1 took it", || ask(&mbp, "replica", json!({ "action": "list" }))["authority"].as_array().unwrap().iter().any(|a| a["task"] == task.as_str() && a["permit"] == "acked"));
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!((worktrees(&here).len(), worktrees(&one).len(), worktrees(&two).len()), (0, 1, 0), "it ran once, on vm1");

    let later = new_task(&mbp, &here, &["Later", "work 1", "--agent", &codex]);
    set_runners(&one, 0);
    set_runners(&two, 0);
    within(15, "vm1 to know the later task", || record(&vm1, &later)["state"] == "waiting");
    vm1.stop();
    let birth = record(&mbp, &later)["birth"].as_str().unwrap().to_string();
    let granted = ask(&mbp, "replica", json!({ "action": "ask", "task": later, "birth": birth, "peer": key1 }));
    assert_eq!(granted["ok"], true, "{granted}");
    set_runners(&two, 1);
    std::thread::sleep(Duration::from_secs(6));
    assert!(!started(&vm2).contains(&later) && !holds(&mbp, &later), "while vm1 is away, its grant waits for it and nobody runs the task");
    set_runners(&one, 1);
    vm1.restart();
    within(40, "vm1, back, to get the grant made while it was away and run it", || started(&vm1).contains(&later));
    assert_eq!((worktrees(&here).len(), worktrees(&two).len()), (0, 0));
}

#[test]
fn an_owner_going_away_mid_claim_leaves_its_task_waiting_until_it_is_back() {
    let (mut mbp, vm) = (machine("mbp"), machine("vm"));
    pair(&mbp, &vm);
    let (here, there) = (clone(&mbp, 0), clone(&vm, 0));
    let codex = agent(&mbp);
    let task = new_task(&mbp, &here, &["Waits", "work 1", "--agent", &codex]);
    within(15, "vm to know mbp's task", || record(&vm, &task)["state"] == "waiting");

    mbp.stop();
    set_runners(&there, 1);
    within(30, "vm to find mbp away", || log_says(&vm, &format!("{task} waits: its owner mbp is away")));
    std::thread::sleep(Duration::from_secs(3));
    assert!(started(&vm).is_empty() && !holds(&vm, &task), "an offline owner's task isn't taken");
    assert_eq!(record(&vm, &task)["state"], "waiting");
    let listed = vm.kanna(&["task", "list"]).ok();
    assert!(listed.lines().any(|l| l.starts_with(&task[..9]) && l.contains("on mbp") && l.contains("waiting")), "{listed}");

    mbp.restart();
    within(60, "vm to get the task once mbp is back", || started(&vm).contains(&task));
    assert!(!holds(&mbp, &task));
    assert_eq!(worktrees(&here), Vec::<String>::new());
}
