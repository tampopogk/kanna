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

fn git(dir: &Path, args: &[&str]) {
    assert!(Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e"]).args(args).current_dir(dir).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success(), "git {args:?}");
}

/// A clone of the team's notes repo where this machine's Kanna keeps its projects, with this many runners.
fn clone(daemon: &Daemon, runners: usize) -> PathBuf {
    let repo = daemon.dir.join("home/.kanna-v4/repos/team/notes");
    std::fs::create_dir_all(repo.join(".kanna")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(&repo, &["remote", "add", "origin", ORIGIN]);
    let repo = repo.canonicalize().unwrap();
    set_runners(&repo, runners);
    repo
}

fn set_runners(repo: &Path, runners: usize) {
    std::fs::write(repo.join(".kanna/config.json"), json!({ "version": 1, "shelf": { "runners": runners } }).to_string()).unwrap();
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

/// The tasks a machine has started, from its own Shelf.
fn started(daemon: &Daemon) -> Vec<String> {
    let listed = ask(daemon, "tasks", json!({ "action": "list" }));
    listed["shelf"].as_array().unwrap().iter().filter(|t| t["folder"].is_string()).map(|t| t["id"].as_str().unwrap().to_string()).collect()
}

fn holds(daemon: &Daemon, task: &str) -> bool {
    ask(daemon, "tasks", json!({ "action": "list" }))["shelf"].as_array().unwrap().iter().any(|t| t["id"] == task)
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
fn two_machines_share_one_repo_s_shelf_and_each_task_runs_once_oldest_first() {
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
    assert!(!holds(&mbp, &first) && !holds(&mbp, &second), "a task granted away leaves its owner's Shelf");
    assert!(holds(&mbp, &blocked) && holds(&mbp, &elsewhere), "the blocked task and the one for mini stay with mbp");
    assert_eq!(worktrees(&here), Vec::<String>::new(), "mbp, with no runners, ran nothing");
    assert_eq!(worktrees(&there).len(), 2);

    let listed = mbp.kanna(&["task", "list"]).ok();
    assert!(listed.lines().any(|l| l.starts_with(&first[..9]) && l.contains("on vm")), "mbp lists the task vm runs: {listed}");
    within(10, "mbp to see that vm owns and started the first task", || record(&mbp, &first)["state"] == "started");
    assert_eq!(record(&mbp, &first)["owner"].as_str(), Some(key(&vm).as_str()));

    within(30, "the first task's turn to end", || ask(&vm, "tasks", json!({ "action": "list" }))["shelf"].as_array().unwrap().iter().any(|t| t["id"] == first.as_str() && t["running"] == false));
    vm.kanna(&["task", "done", &first, "--keep"]).ok();
    within(30, "vm to run the task that waited for the first", || started(&vm).contains(&blocked));
    std::thread::sleep(Duration::from_secs(4));
    assert!(holds(&mbp, &elsewhere) && !started(&vm).contains(&elsewhere), "a task for mini runs nowhere else");
    mbp.kanna(&["task", "start", &elsewhere]).fails(1, "runs on mini, not mbp");
    assert_eq!(worktrees(&here), Vec::<String>::new());

    let birth = record(&mbp, &elsewhere)["birth"].as_str().unwrap().to_string();
    let later = json!({ "device": "mini-key", "ms": 32503680000000i64, "n": 0, "task": elsewhere, "birth": birth, "field": "name", "value": "Renamed on mini" });
    assert_eq!(ask(&mbp, "replica", json!({ "action": "ops", "peer": "mini-key", "ops": [later] }))["ok"], true);
    let named = ask(&mbp, "tasks", json!({ "action": "list" }))["shelf"].as_array().unwrap().iter().find(|t| t["id"] == elsewhere.as_str()).unwrap()["name"].clone();
    assert_eq!(named, "Renamed on mini", "a later edit from another machine reaches the task waiting here");

    let forged = ask(&mbp, "machine", json!({ "action": "call", "host": "vm", "cmd": "replica", "args": { "action": "grant", "task": first, "birth": "1.0.x", "epoch": 99, "peer": key(&mbp) } }));
    assert!(forged.to_string().contains("replica is for another machine's daemon, over a sync stream"), "a control stream can't speak for a machine: {forged}");
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

fn quick(name: &str) -> Daemon {
    let daemon = Daemon::start_with(&[("KANNA_TAKEOVER_AFTER", "5")]);
    daemon.kanna(&["machine", "name", name]).ok();
    daemon
}

#[test]
fn an_owner_away_past_the_grace_has_its_task_run_elsewhere_and_gives_it_up_when_back() {
    let (mut mbp, vm) = (quick("mbp"), quick("vm"));
    pair(&mbp, &vm);
    let (here, there) = (clone(&mbp, 0), clone(&vm, 0));
    let codex = agent(&mbp);
    let (mbp_key, vm_key) = (key(&mbp), key(&vm));
    let task = new_task(&mbp, &here, &["Taken", "work 1", "--agent", &codex]);
    within(15, "vm to know mbp's task", || record(&vm, &task)["state"] == "waiting");

    mbp.stop();
    set_runners(&there, 1);
    within(60, "vm to run the task of mbp, away past the grace", || started(&vm).contains(&task));
    assert!(log_says(&vm, &format!("{task}'s owner mbp has been away 5s, so it runs here")));
    assert_eq!(worktrees(&there), vec![format!("{task}-vm")], "a taken copy's branch names the machine");
    let taken = record(&vm, &task);
    assert_eq!(taken["owner"].as_str(), Some(vm_key.as_str()));
    assert_eq!(taken[format!("run.{vm_key}")]["from"].as_str(), Some(mbp_key.as_str()), "{taken}");

    set_runners(&here, 1);
    mbp.restart();
    within(30, "mbp, back, to give the task up", || !holds(&mbp, &task));
    assert!(log_says(&mbp, &format!("{task} ran on vm while this machine was away, so it leaves this Shelf")));
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(worktrees(&here), Vec::<String>::new(), "mbp doesn't run what vm took");
    let listed = mbp.kanna(&["task", "list"]).ok();
    assert!(listed.lines().any(|l| l.starts_with(&task[..9]) && l.contains("on vm") && l.contains("started")), "{listed}");
}

#[test]
fn a_task_both_machines_ran_while_apart_is_listed_as_ran_twice_on_both() {
    let (mut mbp, mut vm) = (quick("mbp"), quick("vm"));
    pair(&mbp, &vm);
    let (here, there) = (clone(&mbp, 0), clone(&vm, 0));
    let codex = agent(&mbp);
    let task = new_task(&mbp, &here, &["Twice", "work 1", "--agent", &codex]);
    within(15, "vm to know mbp's task", || record(&vm, &task)["state"] == "waiting");

    mbp.stop();
    set_runners(&there, 1);
    within(60, "vm to run the task of mbp, away past the grace", || started(&vm).contains(&task));
    vm.stop();
    set_runners(&here, 1);
    mbp.restart();
    within(60, "mbp, back while vm is away, to run its own copy", || started(&mbp).contains(&task));
    assert_eq!(worktrees(&here), vec![task.clone()]);

    vm.restart();
    for daemon in [&mbp, &vm] {
        within(30, "both to see the task ran on both", || {
            let listed = daemon.kanna(&["task", "list"]).ok();
            listed.lines().any(|l| l.starts_with(&task[..9]) && l.contains("ran twice: ") && l.contains("mbp") && l.contains("vm"))
        });
    }
    assert!(holds(&mbp, &task) && holds(&vm, &task), "each keeps its copy until the user ends one");
}

fn phone(daemon: &Daemon, request: &str, mut args: Value) -> Value {
    args["request"] = json!(request);
    ask(daemon, "phone", args)
}

fn listed(tasks: &Value, id: &str) -> Value {
    tasks["tasks"].as_array().unwrap().iter().find(|t| t["id"] == id).cloned().unwrap_or(Value::Null)
}

#[test]
fn a_phone_reads_and_steers_tasks_on_other_machines_through_its_planner_machine() {
    let (studio, mut vm) = (quick("studio"), quick("vm"));
    pair(&studio, &vm);
    let (here, there) = (clone(&studio, 0), clone(&vm, 0));
    let (codex, far) = (agent(&studio), agent(&vm));
    ask(&studio, "projects", json!({ "action": "touch", "folder": here }));
    ask(&vm, "projects", json!({ "action": "touch", "folder": there }));
    let repo = phone(&studio, "hello", json!({}))["projects"][0]["repo"].as_str().unwrap().to_string();

    let tasks: Vec<String> = ["One", "Two", "Three"].iter().map(|name| new_task(&vm, &there, &[name, "the brief", "--agent", &far])).collect();
    let mine = new_task(&studio, &here, &["Mine", "mine", "--agent", &codex]);
    let spare = new_task(&vm, &there, &["Spare", "spare", "--agent", &far]);
    within(15, "studio to know vm's tasks", || record(&studio, &spare)["state"] == "waiting");

    let shelf = phone(&studio, "tasks.list", json!({ "repo": repo }));
    assert_eq!(listed(&shelf, &tasks[0])["machine"], "vm", "{shelf}");
    assert!(listed(&shelf, &tasks[0])["unreachable"].is_null(), "live from the machine that has it: {shelf}");
    assert_eq!(listed(&shelf, &tasks[0])["status"], "waiting");
    let desks: Vec<&str> = shelf["desks"].as_array().unwrap().iter().filter_map(|d| d["machine"].as_str()).collect();
    assert_eq!(desks, ["studio", "vm"], "both are Macs, neither open: {shelf}");
    assert_eq!(phone(&studio, "task.read", json!({ "task": spare }))["task"]["message"], "spare");

    assert_eq!(phone(&studio, "task.send", json!({ "task": spare, "id": "n1", "text": "and this" }))["state"], "waiting");
    within(15, "the note to reach studio's replica", || record(&studio, &spare)["notes"][0]["text"] == "and this");
    assert_eq!(phone(&studio, "task.read", json!({ "task": spare }))["thread"][0]["text"], "and this");

    for task in &tasks {
        let picked = phone(&studio, "task.pick", json!({ "task": task, "desk": "studio" }));
        assert_eq!(picked["ok"], true, "{picked}");
    }
    within(15, "the claims to reach studio", || tasks.iter().all(|task| record(&studio, task)["claim"] == "studio"));
    assert_eq!(listed(&phone(&studio, "tasks.list", json!({ "repo": repo })), &tasks[0])["desk"], "studio");

    let full = phone(&studio, "task.pick", json!({ "task": mine, "desk": "studio" }));
    assert_eq!(full["error"]["code"], "task_limit", "three on other machines fill a Desk here: {full}");
    let made_room = phone(&studio, "task.pick", json!({ "task": mine, "desk": "studio", "room": tasks[2] }));
    assert_eq!(made_room["ok"], true, "{made_room}");
    within(15, "the shelve to reach studio", || record(&studio, &tasks[2])["claim"].is_null());

    let moved = spare.clone();
    set_runners(&there, 0);
    vm.stop();
    let last = phone(&studio, "tasks.list", json!({ "repo": repo }));
    assert_eq!(listed(&last, &moved)["unreachable"], true, "as last known: {last}");
    assert_eq!(listed(&last, &moved)["name"], "Spare");
    assert_eq!(phone(&studio, "task.read", json!({ "task": moved }))["task"]["message"], "spare", "the record, with vm away");

    set_runners(&here, 1);
    within(60, "studio to take vm's task over and run it", || started(&studio).contains(&moved));
    let pane = ask(&studio, "tasks", json!({ "action": "list" }))["shelf"].as_array().unwrap().iter().find(|t| t["id"] == moved).and_then(|t| t["held"].as_str().map(String::from)).unwrap();
    within(15, "a note granted away to go with the first message", || studio.kanna(&["read", &pane]).ok().replace(['\n', '\r'], "").contains("[message from phone] and this"));
}
