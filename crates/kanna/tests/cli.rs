mod support;

use serde_json::json;
use support::{cli, serve, wait_for, Daemon, Run};

fn kanna_at(dir: &std::path::Path, socket: Option<&std::path::Path>, args: &[&str]) -> Run {
    let mut command = cli(dir);
    if let Some(socket) = socket {
        command.env("KANNA_SOCKET", socket);
    }
    Run::from(args, command.args(args).output().unwrap())
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from("/tmp").join(format!("kcli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("home")).unwrap();
    dir.canonicalize().unwrap()
}

fn layout(daemon: &Daemon, window: usize, tab: usize) -> String {
    let list = daemon.kanna(&["workspace", "list", "--all", "--json"]).json();
    let node = &list["windows"][window]["tabs"][tab]["layout"];
    kanna::output::expression(node, true)
}

#[test]
fn a_code_pane_opens_a_file_at_a_line_and_moves_between_lines() {
    let daemon = Daemon::start();
    let _app = daemon.app();
    daemon.kanna(&["open", "--name", "a", "--cmd", "exec /bin/sh"]).ok();
    let source = daemon.dir.join("main.rs");
    std::fs::write(&source, "fn main() {}\n").unwrap();
    let opened = daemon.kanna(&["open", "code", "main.rs:12", "--name", "src", "--tab"]).ok();
    assert_eq!(opened, "opened file-1 (src) in win-1, tab-2: running");
    let listed = daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["panes"][1].clone();
    assert_eq!((listed["type"].clone(), listed["path"].clone(), listed["line"].clone()), (json!("code"), json!(source.to_string_lossy()), json!(12)));
    daemon.kanna(&["navigate", "src", &format!("{}:40", source.display())]).ok();
    assert_eq!(daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["panes"][1]["line"], json!(40));
    daemon.kanna(&["navigate", "src", "--back"]).ok();
    assert_eq!(daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["panes"][1]["line"], json!(12));
    daemon.kanna(&["focus", "a"]).ok();
    assert_eq!(daemon.kanna(&["open", "code", "main.rs:30"]).ok(), "opened file-1 (src) in win-1, tab-2: running");
    assert!(daemon.kanna(&["workspace", "list", "--all"]).ok().contains("\nFree windows\nwin-1\n  focus: src\n"), "{}", daemon.kanna(&["workspace", "list", "--all"]).ok());
    assert_eq!(daemon.kanna(&["open", "code", "main.rs"]).ok(), "opened file-1 (src) in win-1, tab-2: running");
    let panes = daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["panes"].clone();
    assert_eq!((panes.as_array().unwrap().len(), panes[1]["line"].clone()), (2, json!(30)));
    assert_eq!(daemon.kanna(&["open", "code", "main.rs", "--tab"]).ok(), "opened file-2 in win-1, tab-3: running");
    assert_eq!(daemon.kanna(&["close", "file-2"]).ok(), "closed file-2");
    assert_eq!(daemon.kanna(&["close", "src"]).ok(), "closed file-1");
    daemon.kanna(&["open", "code"]).fails(2, "open: code panes need a file: kanna open code FILE[:LINE]");
    daemon.kanna(&["raw", r#"{"cmd":"open","args":{"type":"code"}}"#]).fails(2, "code panes need a path");
    daemon.kanna(&["raw", r#"{"cmd":"open","args":{"type":"md","path":"/a.md","line":3}}"#]).fails(2, "line is only for code panes");
}

#[test]
fn a_diff_pane_runs_git_diff_with_the_arguments_it_was_given_in_the_folder_it_was_opened_from() {
    let daemon = Daemon::start();
    let _app = daemon.app();
    daemon.kanna(&["open", "--name", "a", "--cmd", "exec /bin/sh"]).ok();
    let opened = daemon.kanna(&["open", "diff", "--merge-base", "main", "-U5", "--name", "changes", "--tab", "--", "src"]).ok();
    assert_eq!(opened, "opened diff-1 (changes) in win-1, tab-2: running");
    let listed = daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["panes"][1].clone();
    assert_eq!(listed["type"], "diff");
    assert_eq!(listed["path"], json!(daemon.dir.to_string_lossy()));
    assert_eq!(listed["args"], json!(["--merge-base", "main", "-U5", "--", "src"]));
    assert_eq!(listed["view"], "unified");
    daemon.kanna(&["navigate", "changes", "diff", "--staged"]).ok();
    assert_eq!(daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["panes"][1]["args"], json!(["--staged"]));
    daemon.kanna(&["navigate", "changes", "diff"]).ok();
    assert_eq!(daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["panes"][1]["args"], json!([]));
    daemon.kanna(&["navigate", "changes", "--back"]).fails(2, "a diff pane keeps no history");
    assert_eq!(daemon.kanna(&["open", "diff", "--cwd", "/tmp", "--tab"]).ok(), "opened diff-2 in win-1, tab-3: running");
    assert_eq!(daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["panes"][2]["path"], "/tmp");
    daemon.kanna(&["raw", r#"{"cmd":"open","args":{"type":"diff"}}"#]).fails(2, "diff panes need a path: the folder git runs in");
    daemon.kanna(&["raw", r#"{"cmd":"open","args":{"type":"diff","path":"/tmp","args":"--staged"}}"#]).fails(2, "args must be a list of strings");
    daemon.kanna(&["raw", r#"{"cmd":"open","args":{"type":"diff","path":"/tmp","view":"sideways"}}"#]).fails(2, "view must be unified or split");
    daemon.kanna(&["raw", r#"{"cmd":"open","args":{"type":"md","path":"/a.md","view":"split"}}"#]).fails(2, "args and view are only for diff panes");
}

#[test]
fn the_daemon_diffs_a_subscribed_folder_and_diffs_again_when_the_worktree_changes() {
    let daemon = Daemon::start();
    let app = daemon.app();
    let repo = daemon.dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(status.status.success(), "{status:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "first"]);
    std::fs::write(repo.join("a.txt"), "one\n2\n").unwrap();
    let folder = repo.to_string_lossy().to_string();
    app.send(json!({"id": 1, "cmd": "diff.subscribe", "args": {"view": "diff-1", "path": folder, "args": []}}));
    let read = || app.heard.lock().unwrap().iter().rev().find(|m| !m["diff"].is_null()).cloned().unwrap_or(json!({}));
    wait_for("the first diff", || read()["serial"] == 1);
    let first = read();
    assert_eq!(first["content"].as_str().map(|id| id.starts_with("diff-")), Some(true));
    assert_eq!(first["diff"]["command"], "git diff");
    assert_eq!(first["diff"]["files"][0]["path"], "a.txt");
    assert_eq!(first["diff"]["files"][0]["hunks"][0]["lines"], json!([[" ", "one", 1, 1], ["-", "two", 2, null], ["+", "2", null, 2]]));
    std::fs::write(repo.join("b.txt"), "new\n").unwrap();
    wait_for("the untracked file", || read()["diff"]["files"].as_array().is_some_and(|f| f.len() == 2));
    assert_eq!(read()["diff"]["files"][1]["untracked"], true);
    std::fs::create_dir_all(repo.join("target")).unwrap();
    std::fs::write(repo.join(".git/info/exclude"), "target/\n").unwrap();
    wait_for("the exclude", || read()["serial"] == 2 || read()["serial"] == 3);
    let serial = read()["serial"].clone();
    std::fs::write(repo.join("target/out"), "built\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(read()["serial"], serial);

    app.send(json!({"id": 2, "cmd": "diff.subscribe", "args": {"view": "diff-2", "path": folder, "args": []}}));
    let views = || daemon.kanna(&["raw", r#"{"cmd":"diffs"}"#]).json()["diffs"].clone();
    wait_for("a second view of the diff", || views()[0]["views"].as_array().is_some_and(|v| v.len() == 2));
    assert_eq!(views().as_array().map(Vec::len), Some(1));
    app.send(json!({"cmd": "diff.unsubscribe", "args": {"view": "diff-2"}}));
    wait_for("the second view gone", || views()[0]["views"].as_array().is_some_and(|v| v.len() == 1));

    app.send(json!({"id": 3, "cmd": "diff.subscribe", "args": {"view": "diff-1", "path": folder, "args": ["--staged"]}}));
    wait_for("the staged diff", || read()["diff"]["command"] == "git diff --staged");
    assert_eq!(read()["diff"]["files"], json!([]));
    assert_eq!(views()[0]["args"], json!(["--staged"]));
    app.send(json!({"id": 4, "cmd": "diff.subscribe", "args": {"view": "diff-3", "args": []}}));
    wait_for("the refusal", || app.heard.lock().unwrap().iter().any(|m| m["id"] == 4 && m["error"]["message"] == "a diff needs the folder it's in (path)"));
    app.quit();
    wait_for("the watcher stopped with its app", || views() == json!([]));
}

#[test]
fn the_daemon_reads_a_subscribed_file_and_sends_it_again_when_it_changes() {
    let daemon = Daemon::start();
    let app = daemon.app();
    let dir = daemon.dir.join("docs");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.md");
    std::fs::write(&file, "# One\n").unwrap();
    let path = file.to_string_lossy().to_string();
    app.send(json!({"id": 1, "cmd": "file.subscribe", "args": {"view": "file-1", "path": path, "kind": "md"}}));
    let read = || app.heard.lock().unwrap().iter().rev().find(|m| m.get("serial").is_some_and(|s| s.as_u64() > Some(0))).cloned().unwrap_or(json!({}));
    wait_for("the first text", || read()["text"] == "# One\n");
    assert_eq!(read()["content"].as_str().map(|id| id.starts_with("file-")), Some(true));
    assert_eq!(read()["path"], path);
    std::fs::write(&file, "# Two\n").unwrap();
    wait_for("the text written in place", || read()["text"] == "# Two\n");
    std::fs::write(dir.join(".a.md.tmp"), "# Three\n").unwrap();
    std::fs::rename(dir.join(".a.md.tmp"), &file).unwrap();
    wait_for("the text renamed over the file", || read()["text"] == "# Three\n");
    let serial = read()["serial"].clone();
    std::fs::remove_file(&file).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert_eq!(read()["serial"], serial, "a deleted file keeps its text");
    std::fs::write(&file, "# Four\n").unwrap();
    wait_for("the recreated file", || read()["text"] == "# Four\n");

    let files = || daemon.kanna(&["raw", r#"{"cmd":"files"}"#]).json()["files"].clone();
    app.send(json!({"id": 2, "cmd": "file.subscribe", "args": {"view": "file-2", "path": path, "kind": "md"}}));
    wait_for("a second view of the file", || files()[0]["views"].as_array().is_some_and(|v| v.len() == 2));
    assert_eq!(files().as_array().map(Vec::len), Some(1));
    app.send(json!({"cmd": "file.unsubscribe", "args": {"view": "file-2"}}));
    wait_for("the second view gone", || files()[0]["views"].as_array().is_some_and(|v| v.len() == 1));

    let missing = dir.join("later/b.rs").to_string_lossy().to_string();
    app.send(json!({"id": 3, "cmd": "file.subscribe", "args": {"view": "file-3", "path": missing, "kind": "code"}}));
    let of = |view_path: &str| app.heard.lock().unwrap().iter().rev().find(|m| m["path"] == view_path && m.get("serial").is_some_and(|s| s.as_u64() > Some(0))).cloned().unwrap_or(json!({}));
    wait_for("the missing file's error", || of(&missing)["error"]["code"] == "missing");
    assert_eq!(of(&missing)["error"]["message"], format!("No such file: {missing}"));
    std::fs::create_dir_all(dir.join("later")).unwrap();
    std::fs::write(&missing, "fn main() {}\n").unwrap();
    wait_for("the file once it's there", || of(&missing)["text"] == "fn main() {}\n");
    app.send(json!({"id": 4, "cmd": "file.subscribe", "args": {"view": "file-4", "kind": "md"}}));
    wait_for("the refusal", || app.heard.lock().unwrap().iter().any(|m| m["id"] == 4 && m["error"]["message"] == "a file needs its path"));
    app.quit();
    wait_for("the watchers stopped with their app", || files() == json!([]));
}

#[test]
fn every_command_works_through_the_daemon_and_the_app() {
    let daemon = Daemon::start();
    let _app = daemon.app();
    assert_eq!(daemon.kanna(&["open", "--name", "a", "--cmd", "exec /bin/sh"]).ok(), "opened term-1 (a) in win-1, tab-1: running");
    assert_eq!(daemon.kanna(&["open", "--name", "b", "--split", "right", "--size", "1/3"]).ok(), "opened term-2 (b) in win-1, tab-1: running");
    assert_eq!(layout(&daemon, 0, 0), "a:2/3 | b:1/3");
    assert_eq!(daemon.kanna(&["workspace", "list", "--all"]).ok(), "Windows\n*win-1  free\n\nFree windows\nwin-1\n  focus: b\n  *tab-1 \"a\"  a:2/3 | b:1/3");
    assert_eq!(daemon.kanna_as("term-2", &["workspace", "list"]).ok(), "*win-1  free\n  focus: b\n  *tab-1 \"a\"  a:2/3 | b:1/3");
    daemon.kanna(&["workspace", "list"]).fails(2, "outside a Kanna terminal there's no workspace of yours: add --all");
    daemon.kanna_as("term-99", &["workspace", "list"]).fails(4, "no pane \"term-99\"");

    assert_eq!(daemon.kanna(&["focus", "a"]).ok(), "focused a (key window win-1)");
    assert_eq!(daemon.kanna(&["zoom", "b"]).ok(), "b is zoomed (run again to unzoom)");
    assert_eq!(daemon.kanna_as("a", &["workspace", "list"]).ok().lines().nth(1), Some("  focus: b, zoomed"));
    assert_eq!(daemon.kanna(&["zoom", "b"]).ok(), "b is no longer zoomed");
    assert_eq!(daemon.kanna(&["resize", "b", "1/4"]).ok(), "a:3/4 | b:1/4");
    assert_eq!(daemon.kanna(&["move", "b", "--to", "a", "--side", "bottom"]).ok(), "a:1/2 / b:1/2");
    assert_eq!(daemon.kanna(&["move", "b", "--to", "a"]).ok(), "b:1/2 / a:1/2");
    let arranged = daemon.kanna(&["arrange", r#"{"split":"row","children":[{"pane":"a","size":"2/3"},{"pane":"b"}]}"#]).ok();
    assert_eq!(arranged, "a:2/3 | b:1/3");

    assert_eq!(daemon.kanna(&["open", "--tab", "--name", "c"]).ok(), "opened term-3 (c) in win-1, tab-2: running");
    assert_eq!(daemon.kanna(&["rename-tab", "tab-2", "my", "servers"]).ok(), "tab-2 is now \"my servers\"");
    assert_eq!(daemon.kanna(&["rename-pane", "c", "logs"]).ok(), "term-3 is now \"logs\"");
    assert!(daemon.kanna(&["workspace", "list", "--all"]).ok().contains("\"my servers\"  logs"), "{}", daemon.kanna(&["workspace", "list", "--all"]).ok());
    daemon.kanna(&["rename-pane", "logs", "a"]).fails(1, "a pane named \"a\" already exists");
    daemon.kanna(&["rename-pane", "logs", "term-1"]).fails(1, "a pane named \"term-1\" already exists");
    assert_eq!(daemon.kanna(&["rename-pane", "logs", ""]).ok(), "term-3 has no name");
    assert_eq!(daemon.kanna(&["rename-pane", "term-3", "c"]).ok(), "term-3 is now \"c\"");
    assert_eq!(daemon.kanna(&["move-tab", "tab-2", "--index", "0"]).ok(), "win-1: tab-2, tab-1");
    assert_eq!(daemon.kanna(&["focus", "--tab", "tab-1"]).ok(), "switched to tab-1 (key window win-1)");
    let opened = daemon.kanna(&["open", "--window", "new", "--name", "d", "--no-wait"]).ok();
    assert!(opened.starts_with("opened term-4 (d) in win-2, tab-3: "), "{opened}");
    assert_eq!(daemon.kanna(&["focus", "--window", "win-1"]).ok(), "key window is now win-1");
    assert_eq!(daemon.kanna(&["move", "c", "--window", "win-2"]).ok(), "c");
    assert_eq!(daemon.kanna(&["move", "d", "--to", "c", "--side", "left"]).ok(), "d:1/2 | c:1/2");
    assert_eq!(daemon.kanna(&["move", "c", "--tab", "new", "--window", "win-2"]).ok(), "c");
    assert_eq!(daemon.kanna(&["move-tab", "tab-5", "--window", "new"]).ok(), "win-3: tab-5");

    daemon.kanna(&["send", "a", "echo", "SENT-$((40+2))"]).ok();
    wait_for("the sent line", || daemon.kanna(&["read", "a"]).ok().contains("SENT-42"));
    assert!(daemon.kanna(&["read", "a", "--lines", "3"]).ok().lines().count() <= 3);
    daemon.kanna(&["send", "a", "--", "--json"]).ok();
    wait_for("--json typed", || daemon.kanna(&["read", "a"]).ok().contains("--json"));
    assert_eq!(daemon.kanna(&["restart", "a"]).ok(), "term-5 (a) (exec /bin/sh) is running");

    assert_eq!(daemon.kanna(&["focus", "b"]).ok(), "focused b (key window win-1)");
    let page = daemon.dir.join("page.html");
    std::fs::write(&page, "<p>hi</p>").unwrap();
    let url = format!("file://{}", page.display());
    assert_eq!(daemon.kanna(&["open", "web", &url, "--name", "site", "--history", "--split", "down"]).ok(), "opened page-1 (site) in win-1, tab-1: running");
    assert_eq!(daemon.kanna(&["navigate", "site", "file:///tmp/other.html"]).ok(), "page-1 (site) (file:///tmp/other.html) is running");
    assert_eq!(daemon.kanna(&["navigate", "site", "--back"]).ok(), format!("page-1 (site) ({url}) is running"));
    assert_eq!(daemon.kanna(&["navigate", "site", "--forward"]).ok(), "page-1 (site) (file:///tmp/other.html) is running");
    let doc = daemon.dir.join("doc.md");
    std::fs::write(&doc, "# hi").unwrap();
    let opened = daemon.kanna(&["open", "md", "doc.md", "--name", "doc", "--tab"]).ok();
    assert_eq!(opened, "opened file-1 (doc) in win-1, tab-6: running");
    assert_eq!(daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["panes"][5]["path"], json!(doc.to_string_lossy()));
    assert!(daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["panes"][5].get("markup").is_none());
    assert_eq!(daemon.kanna(&["markup", "doc"]).ok(), "comments on in file-1 (doc)");
    assert_eq!(daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["panes"][5]["markup"], json!(true));
    assert_eq!(daemon.kanna(&["markup", "doc", "--to", "term-5"]).ok(), "comments on in file-1 (doc), sent to term-5");
    assert_eq!(daemon.kanna(&["markup", "doc", "--off"]).ok(), "comments off in file-1 (doc)");

    let capabilities = daemon.kanna(&["capabilities"]).ok();
    assert!(capabilities.starts_with("pane types: term, web, ios, md, code, diff\ncommands: agents, arrange, backlog, capabilities, close, comment, desk, diffs, files, focus, list, machine, markup, move, move-tab, navigate, open, projects, quiz, read, rename-pane, rename-tab, resize, restart, screenshot, send, send-key, status, task, toast, tour, window, zoom"), "{capabilities}");
    assert!(capabilities.ends_with("Kanna.app: attached"), "{capabilities}");
    assert_eq!(daemon.kanna(&["toast", "Build finished", "it took", "3m"]).ok(), "raised toast-1");
    assert_eq!(daemon.kanna(&["toast", "Messages wait", "--pane", "a", "--key", "k1"]).ok(), "raised toast-2");
    assert_eq!(daemon.kanna(&["toast", "Messages wait", "--pane", "a", "--key", "k1"]).ok(), "already raised a toast with key k1");
    daemon.kanna(&["toast", "About nothing", "--pane", "nope"]).fails(4, "no pane \"nope\"");
    daemon.kanna(&["toast"]).fails(2, "toast: missing TITLE");
    let raw = daemon.kanna(&["raw", r#"{"cmd":"debug.anything","args":{"pane":"a"}}"#]).json();
    assert_eq!(raw, json!({ "cmd": "debug.anything", "args": { "pane": "term-5" } }));
    let shot = daemon.kanna(&["screenshot", "a", "-o", "shot.png", "--force", "--json"]).json();
    assert_eq!(shot, json!({ "cmd": "screenshot", "args": { "pane": "term-5", "path": daemon.dir.join("shot.png").to_string_lossy(), "force": true } }));
    assert_eq!(daemon.kanna(&["screenshot", "--window", "win-1", "--json"]).json()["args"], json!({ "window": "win-1", "dir": std::env::temp_dir().to_string_lossy() }));
    daemon.kanna(&["screenshot", "a", "--window", "win-1"]).fails(2, "give PANE or --window WINDOW, not both");
    assert_eq!(daemon.kanna(&["raw", r#"{"cmd":"list"}"#]).json()["panes"].as_array().unwrap().len(), 6);

    assert_eq!(daemon.kanna(&["close", "--window", "win-3"]).ok(), "closed win-3 (panes term-3)");
    assert_eq!(daemon.kanna(&["close", "--tab", "tab-6"]).ok(), "closed tab-6 (panes file-1)");
    assert_eq!(daemon.kanna(&["close", "site"]).ok(), "closed page-1");
    let reply = daemon.kanna(&["--json", "close", "d"]).json();
    assert_eq!(reply, json!({ "closed": ["term-4"] }));
    assert_eq!(layout(&daemon, 0, 0), "a:2/3 | b:1/3");

    daemon.kanna(&["open", "--window", "new", "--dock", "--name", "agent", "--no-wait"]).fails(2, "a docked pane is a task's agent, in its own workspace, not in a window");
    let opened = daemon.kanna(&["open", "--dock", "--name", "agent", "--no-wait"]).ok();
    assert!(opened.starts_with("opened term-6 (agent) docked left in ws-1 (on the Shelf)"), "{opened}");
    let notes = daemon.kanna_as("term-6", &["open", "--name", "notes", "--no-wait"]).ok();
    assert!(notes.starts_with("opened term-7 (notes) in ws-1 (on the Shelf), tab-"), "{notes}");
    let listed = daemon.kanna(&["task", "list"]).ok();
    assert_eq!(listed, "TASK    NAME   WHERE  STATE    AGENT\nterm-6  agent  Shelf  running         parked just now", "{listed}");
    assert_eq!(daemon.kanna(&["task", "list", "AGE"]).ok(), listed);
    assert_eq!(daemon.kanna(&["task", "list", "agent", "notes"]).ok(), "no task matches \"agent notes\"");
    assert_eq!(daemon.kanna(&["--json", "task", "list", "nothing"]).json()["tasks"], json!([]));
    assert_eq!(daemon.kanna(&["task", "pull", "notes"]).ok(), "pulled term-6 onto d1", "a task's pane names its task, and the app is told its agent");
    assert_eq!(daemon.kanna(&["task", "park", "ws-1"]).ok(), "parked term-6; d1 is an empty Desk");
    daemon.kanna(&["task", "pull"]).fails(2, "task: pull needs a TASK (see `kanna task list`)");
    daemon.kanna(&["task", "pull", "a"]).fails(4, "a is not in a task: it's in no task's workspace");

    daemon.kanna(&["open", "--window", "new", "--name", "side", "--no-wait"]).ok();
    assert_eq!(daemon.kanna(&["window", "assign", "win-4", "agent"]).ok(), "assigned win-4 to ws-1 (on the Shelf)");
    let more = daemon.kanna(&["open", "--window", "win-4", "--tab", "--name", "more", "--no-wait"]).ok();
    assert!(more.starts_with("opened term-9 (more) in ws-1's win-4 (on the Shelf), tab-"), "{more}");
    assert_eq!(daemon.kanna(&["focus", "more"]).ok(), "focused more in ws-1's win-4 (on the Shelf, key window)");
    assert_eq!(daemon.kanna(&["zoom", "agent"]).ok(), "agent is zoomed in ws-1 (on the Shelf; run again to unzoom)");
    let mine = daemon.kanna_as("agent", &["workspace", "list"]).ok();
    assert!(mine.starts_with("ws-1, on the Shelf\n  dock: term-6 (agent)  left 2/5\n  focus: agent, zoomed\n  *tab-7 \"notes\"  notes\n  win-4\n"), "{mine}");
    assert_eq!(daemon.kanna_as("more", &["workspace", "list"]).ok(), mine, "a pane in its window sees the same");
    assert_eq!(daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["workspaces"][0]["id"], "ws-1");
    assert_eq!(daemon.kanna(&["window", "free", "win-4"]).ok(), "win-4 is free (out of ws-1)");
    assert_eq!(daemon.kanna(&["window", "free", "win-4"]).ok(), "win-4 is free");
    daemon.kanna(&["window", "assign", "win-4", "tab-99"]).fails(4, "no workspace \"tab-99\"");
    daemon.kanna(&["window", "move", "win-4"]).fails(2, "window: unknown action \"move\": assign or free");
}

#[test]
fn errors_exit_as_kmux_says() {
    let daemon = Daemon::start();
    let _app = daemon.app();
    daemon.kanna(&["open", "--name", "a"]).ok();
    daemon.kanna(&["close", "nope"]).fails(4, "no pane \"nope\" [not_found]\nRun `kanna workspace list --all` to see pane names and IDs");
    daemon.kanna(&["open", "--name", "a"]).fails(1, "[");
    daemon.kanna(&["zoom"]).fails(2, "zoom: missing PANE");
    let err = daemon.kanna(&["resize", "a", "2"]).fails(2, "more: kanna help resize");
    assert!(err.contains("\nusage: kanna resize PANE FRACTION\n"), "{err}");
    daemon.kanna(&["raw", r#"{"cmd":"nope"}"#]).fails(5, "unknown command \"nope\" [bad_request]");
    daemon.kanna(&["raw", "{"]).fails(2, "raw: REQUEST is not valid JSON");
    daemon.kanna(&["raw", "{}"]).fails(2, "raw: REQUEST needs a \"cmd\"");
    daemon.kanna(&["read", "nope"]).fails(4, "not_found");
    daemon.kanna(&["workspce"]).fails(2, "unknown command \"workspce\". Did you mean: workspace? Run `kanna --help` for all commands.");
    daemon.kanna(&["zoom", "a", "b"]).fails(2, "zoom: unexpected argument \"b\"");
    daemon.kanna(&["--bogus"]).fails(2, "unknown option \"--bogus\"\n\nkanna — control the Kanna");
    daemon.kanna(&["--instance", "a/b", "workspace", "list", "--all"]).fails(2, "bad instance name \"a/b\"");
    daemon.kanna(&["--mux", "tmux", "workspace", "list", "--all"]).fails(5, "the tmux backend isn't supported yet");
}

const BYTES: &str = r#"stty raw -echo; printf '\033[?1hREADY\r\n'; while :; do printf '%s ' "$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \n')"; done"#;

#[test]
fn send_key_presses_keys_as_the_program_expects_them_without_the_app() {
    let daemon = Daemon::start();
    daemon.kanna(&["open", "--name", "bytes", "--cmd", BYTES]).ok();
    wait_for("the program", || daemon.kanna(&["read", "bytes"]).ok().contains("READY"));
    assert_eq!(daemon.kanna(&["send-key", "bytes", "up", "ctrl+c", "return", "shift+tab", "A", "esc"]).ok(), "");
    assert!(daemon.kanna(&["read", "bytes"]).ok().contains("1b 4f 41 03 0d 1b 5b 5a 41 1b"), "{}", daemon.kanna(&["read", "bytes"]).ok());
    daemon.kanna(&["send-key", "bytes", "cmd+c"]).fails(2, "cmd+c only means something to Kanna.app, not to the program in bytes (term-1)");
    daemon.kanna(&["send-key", "bytes", "up", "hello"]).fails(2, "\"hello\" isn't a key; to type text, use kanna send");
    daemon.kanna(&["send-key", "bytes", "hyper+a"]).fails(2, "hyper+a: modifiers are ctrl, alt and shift");
    daemon.kanna(&["send-key", "nope", "return"]).fails(4, "not_found");
    daemon.kanna(&["raw", r#"{"cmd":"send-key","args":{"pane":"bytes","keys":[]}}"#]).fails(2, "missing keys");
    daemon.kanna(&["open", "--name", "shell"]).ok();
    daemon.kanna(&["send", "shell", "sleep 30"]).ok();
    std::thread::sleep(std::time::Duration::from_millis(300));
    daemon.kanna(&["send-key", "shell", "ctrl+c"]).ok();
    daemon.kanna(&["send", "shell", "echo AFTER-$((6*7))"]).ok();
    wait_for("the interrupted shell", || daemon.kanna(&["read", "shell"]).ok().contains("AFTER-42"));
    assert!(daemon.kanna(&["read", "shell"]).ok().contains("sleep 30\n^C\n"));
}

#[test]
fn without_the_app_terminals_and_the_layout_work_and_debug_needs_it() {
    let daemon = Daemon::start();
    assert_eq!(daemon.kanna(&["open", "--name", "a"]).ok(), "opened term-1 (a) in win-1, tab-1: running");
    assert_eq!(daemon.kanna(&["workspace", "list", "--all"]).ok(), "Windows\n*win-1  free\n\nFree windows\nwin-1\n  focus: a\n  *tab-1 \"a\"  a");
    daemon.kanna(&["send", "a", "echo", "ALONE"]).ok();
    wait_for("the sent line", || daemon.kanna(&["read", "a"]).ok().contains("ALONE"));
    assert_eq!(daemon.kanna(&["restart", "a"]).ok(), "term-2 (a) is running");
    assert!(daemon.kanna(&["capabilities"]).ok().ends_with("Kanna.app: not attached (terminals and the layout work without it; the CLI starts it for commands that need it)"));
    daemon.kanna(&["open", "--name", "b", "--split", "right"]).ok();
    for args in [
        &["open", "--tab"][..],
        &["open", "md", "/tmp/x.md"],
        &["focus", "a"],
        &["resize", "a", "1/3"],
        &["rename-tab", "tab-1", "x"],
        &["close", "--tab", "tab-2"],
    ] {
        daemon.kanna(args).ok();
    }
    for args in [&["raw", r#"{"cmd":"debug.app"}"#][..], &["screenshot", "win-1"]] {
        daemon.kanna(args).fails(6, "[no_app] (KANNA_SOCKET is set, so kanna doesn't start Kanna.app for it)");
    }
    daemon.kanna(&["send", "nope", "x"]).fails(4, "not_found");
    assert_eq!(daemon.kanna(&["close", "b"]).ok(), "closed term-3");
    let app = daemon.app();
    assert_eq!(layout(&daemon, 0, 0), "a");
    app.quit();
}

#[test]
fn help_never_starts_anything_and_marks_what_the_running_kanna_lacks() {
    let dir = scratch("help");
    let nowhere = dir.join("nothing.sock");
    let overview = kanna_at(&dir, Some(&nowhere), &[]).ok();
    assert!(overview.starts_with("kanna — control the Kanna terminal multiplexer (windows → tabs → split panes).\n\nusage: kanna [--mux kanna|kmux] [--instance NAME] [--host MACHINE] [--bg] [--fresh] COMMAND [ARGS] [--json] [-- TEXT]\n       kanna PATH                           (the Desk on folder PATH's project, opening one if none is open)\n\nPanes:\n  open "), "{overview}");
    assert!(overview.contains("Kanna isn't running, so help can't check which commands it supports."), "{overview}");
    assert!(overview.contains(&format!("Instance: default of {} at {}", kanna_build::Build::current().app_name(), nowhere.display())), "{overview}");
    assert!(overview.contains("6 needs Kanna.app, which isn't attached and wasn't started."), "{overview}");
    assert_eq!(kanna_at(&dir, Some(&nowhere), &["help"]).ok(), overview);
    assert_eq!(kanna_at(&dir, Some(&nowhere), &["--version"]).ok(), format!("kanna {} ({})", kanna_build::VERSION, kanna_build::Build::current().app_name()));
    let open = kanna_at(&dir, Some(&nowhere), &["help", "open"]).ok();
    assert!(open.starts_with("kanna open — Open a terminal"), "{open}");
    assert_eq!(kanna_at(&dir, Some(&nowhere), &["open", "--help"]).ok(), open);
    assert_eq!(kanna_at(&dir, Some(&nowhere), &["open", "-h"]).ok(), open);
    assert!(kanna_at(&dir, Some(&nowhere), &["help", "read"]).ok().contains("usage: kanna read PANE [--lines N]"));
    assert!(kanna_at(&dir, Some(&nowhere), &["help", "instances"]).ok().starts_with("kanna instances — List the running Kanna instances."));
    assert!(kanna_at(&dir, Some(&nowhere), &["instances", "--help"]).ok().starts_with("kanna instances — "));
    assert!(kanna_at(&dir, Some(&nowhere), &["raw", "--help"]).ok().starts_with("usage: kanna raw REQUEST"));
    kanna_at(&dir, Some(&nowhere), &["help", "nope"]).fails(2, "unknown command \"nope\"");
    let commands = kanna_at(&dir, Some(&nowhere), &["commands"]).ok();
    assert_eq!(commands.lines().count(), 30);
    assert!(commands.starts_with("open          Open a terminal"), "{commands}");
    let described = kanna_at(&dir, Some(&nowhere), &["commands", "--json"]).json();
    assert_eq!(described["running"], false);
    assert_eq!(described["commands"].as_array().unwrap().len(), 30);
    assert_eq!(described["exitCodes"]["6"], "needs Kanna.app, which isn't attached and wasn't started");
    assert!(described["globalOptions"].as_array().unwrap().iter().any(|o| o["flag"] == "--"));

    let socket = dir.join("old.sock");
    let _requests = serve(&socket, |request| match request["cmd"].as_str() {
        Some("capabilities") => json!({ "ok": true, "mux": "kmux", "paneTypes": ["term"], "commands": ["capabilities", "list", "open"] }),
        Some(cmd) if ["list", "open"].contains(&cmd) => json!({ "ok": true }),
        Some(cmd) => json!({ "ok": false, "error": { "code": "bad_request", "message": format!("unknown command \"{cmd}\"") } }),
        None => json!({ "ok": false }),
    });
    let overview = kanna_at(&dir, Some(&socket), &[]).ok();
    assert!(overview.contains("  zoom          Toggle zoom: the pane fills its tab until you zoom again or focus another pane.  [not in the running Kanna]"), "{overview}");
    assert!(overview.contains("Commands marked [not in the running Kanna] aren't in the running Kanna yet."), "{overview}");
    let help = kanna_at(&dir, Some(&socket), &["help", "zoom"]).ok();
    assert!(help.contains("The running Kanna (default) doesn't support zoom yet."), "{help}");
    let described = kanna_at(&dir, Some(&socket), &["commands", "--json"]).json();
    assert_eq!(described["running"], true);
    let zoom = described["commands"].as_array().unwrap().iter().find(|c| c["name"] == "zoom").unwrap().clone();
    assert_eq!(zoom["supported"], false);
    let diagrams = described["commands"].as_array().unwrap().iter().find(|c| c["name"] == "diagrams").unwrap().clone();
    assert_eq!(diagrams["supported"], true);
    assert!(!overview.contains("  diagrams      Check that a markdown file's mermaid diagrams draw in Kanna, without opening anything.  [not"), "{overview}");
    kanna_at(&dir, Some(&socket), &["zoom", "a"]).fails(5, "the running Kanna doesn't support `zoom` yet. It supports: capabilities, list, open");
    let _ = std::fs::remove_dir_all(&dir);
}

const GOOD: &str = "# Plan\n\n```mermaid\nflowchart LR\n    A --> B\n```\n\n```mermaid\nsequenceDiagram\n    A->>B: hi\n```\n";

const BAD: &str = "# Plan\n\n```mermaid\nflowchart LR\n    A --> B\n```\n\nText.\n\n```mermaid\nerDiagram\n    A ||--o{ B : has\n```\n\n```mermaid\nflowchart LR\n    A --> --> B\n```\n";

const ER: &str = "bad.md:10: unsupported diagram type: er (Kanna draws flowchart, sequence, state, class and gitGraph)";

#[test]
fn diagrams_check_needs_no_kanna_and_exits_1_when_one_does_not_draw() {
    let dir = scratch("diagrams");
    let nowhere = dir.join("nothing.sock");
    std::fs::write(dir.join("good.md"), GOOD).unwrap();
    std::fs::write(dir.join("bad.md"), BAD).unwrap();
    assert_eq!(kanna_at(&dir, Some(&nowhere), &["diagrams", "check", "good.md"]).ok(), "2 diagrams, all draw");
    let run = kanna_at(&dir, Some(&nowhere), &["diagrams", "check", "good.md", "bad.md"]);
    let out = run.out.clone();
    run.fails(1, "kanna: 2 of 5 diagrams don't draw");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], ER);
    assert!(lines[1].starts_with("bad.md:15: diagram error: "), "{out}");
    assert_eq!(lines.len(), 2, "{out}");
    let run = kanna_at(&dir, Some(&nowhere), &["diagrams", "check", "bad.md", "--json"]);
    let reply: serde_json::Value = serde_json::from_str(run.out.trim()).unwrap();
    run.fails(1, "2 of 3 diagrams don't draw");
    assert_eq!((reply["diagrams"].clone(), reply["failures"][0]["line"].clone(), reply["failures"][0]["type"].clone()), (json!(3), json!(10), json!("er")));
    kanna_at(&dir, Some(&nowhere), &["diagrams", "check", "missing.md"]).fails(4, "diagrams: can't read missing.md");
    kanna_at(&dir, Some(&nowhere), &["diagrams", "check"]).fails(2, "diagrams: missing FILE.md");
    kanna_at(&dir, Some(&nowhere), &["diagrams", "lint", "a.md"]).fails(2, "unknown action \"lint\" (check)");
    assert!(kanna_at(&dir, Some(&nowhere), &["help", "diagrams"]).ok().starts_with("kanna diagrams — Check that"));
    assert!(!nowhere.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_md_says_which_diagrams_do_not_draw_and_still_opens() {
    let daemon = Daemon::start();
    let _app = daemon.app();
    daemon.kanna(&["open", "--name", "a", "--cmd", "exec /bin/sh"]).ok();
    std::fs::write(daemon.dir.join("good.md"), GOOD).unwrap();
    std::fs::write(daemon.dir.join("bad.md"), BAD).unwrap();
    let run = daemon.kanna(&["open", "md", "good.md", "--tab"]);
    assert_eq!(run.err, "");
    assert_eq!(run.ok(), "opened file-1 in win-1, tab-2: running");
    let run = daemon.kanna(&["open", "md", "bad.md", "--tab"]);
    let err = run.err.clone();
    assert_eq!(run.ok(), "opened file-2 in win-1, tab-3: running");
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(lines[0], ER);
    assert!(lines[1].starts_with("bad.md:15: diagram error: "), "{err}");
    let reply = daemon.kanna(&["open", "md", "bad.md", "--tab", "--json"]).json();
    assert_eq!(reply["pane"]["id"], "file-3");
    let failures = reply["diagrams"].as_array().unwrap();
    assert_eq!(failures.iter().map(|f| f["line"].clone()).collect::<Vec<_>>(), [json!(10), json!(15)]);
    assert_eq!(failures[0]["message"], ER.split_once(": ").unwrap().1);
    assert_eq!(daemon.kanna(&["open", "md", "good.md", "--tab", "--json"]).json()["diagrams"], json!([]));
}

#[test]
fn a_running_kanna_supports_every_command() {
    let daemon = Daemon::start();
    let overview = daemon.kanna(&[]).ok();
    assert!(overview.contains("The running Kanna supports every command."), "{overview}");
    assert!(!overview.contains("[not in the running"), "{overview}");
}

#[test]
fn instances_are_folders_that_answer() {
    let dir = scratch("instances");
    let support = if cfg!(target_os = "macos") { dir.join("home/Library/Application Support") } else { dir.join("home/.local/share") };
    let build = kanna_build::Build::current();
    let mine = build.channel.instance_folder(&build.default_instance());
    assert!(kanna_at(&dir, None, &["instances"]).ok().starts_with("No Kanna instances are running."));
    let mut listeners = vec![];
    for folder in [mine.as_str(), "kanna-dev-work", "kanna-dev-gone", "kanna-prod", "kanna-staging"] {
        std::fs::create_dir_all(support.join(folder)).unwrap();
    }
    std::fs::write(support.join("kanna-dev-work").join(kanna_build::RECORD), json!({ "channel": "dev", "worktree": dir.join("wt") }).to_string()).unwrap();
    for folder in [mine.as_str(), "kanna-dev-work", "kanna-prod"] {
        let socket = support.join(folder).join("kanna.sock");
        listeners.push(serve(&socket, |request| match request["cmd"].as_str() {
            Some("list") => json!({ "ok": true, "windows": [{ "id": "win-1" }], "panes": [{ "id": "p1" }, { "id": "p2" }] }),
            _ => json!({ "ok": false, "error": { "code": "bad_request", "message": "no" } }),
        }));
    }
    std::os::unix::net::UnixListener::bind(support.join("kanna-dev-gone/kanna.sock")).map(drop).unwrap();
    let text = kanna_at(&dir, None, &["instances"]).ok();
    let lines: Vec<&str> = text.lines().collect();
    let name = build.default_instance();
    assert!(lines[0].starts_with("  default ") && lines[0].contains(" production  1 window, 2 panes"), "{text}");
    assert!(lines.iter().any(|l| l.starts_with(&format!("* {name} ")) && l.contains(" dev ")), "{text}");
    let work = lines.iter().find(|l| l.starts_with("  work ")).unwrap();
    assert!(work.ends_with(&format!("1 window, 2 panes  {} (gone)", dir.join("wt").display())), "{text}");
    assert!(!text.contains("gone "), "{text}");
    assert!(text.ends_with(&format!("* the instance this shell's kanna commands talk to ({}). Pick another with --instance NAME or KANNA_INSTANCE.", support.join(&mine).join("kanna.sock").display())), "{text}");
    let text = kanna_at(&dir, None, &["--instance", "work", "instances"]).ok();
    assert!(text.contains("\n* work "), "{text}");
    let mut command = cli(&dir);
    command.env("KANNA_INSTANCE", "work");
    let described = Run::from(&["instances", "--json"], command.args(["instances", "--json"]).output().unwrap()).json();
    assert_eq!(described["current"]["name"], "work");
    let row = described["instances"].as_array().unwrap().iter().find(|r| r["name"] == "work").unwrap().clone();
    assert_eq!(row, json!({ "name": "work", "channel": "dev", "worktree": dir.join("wt"), "socket": support.join("kanna-dev-work/kanna.sock"), "current": true, "windows": 1, "panes": 2 }));
    assert_eq!(kanna_at(&dir, None, &["--instance", "work", "workspace", "list", "--all", "--json"]).json()["panes"].as_array().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn instances_stop_quits_the_app_and_waits_for_the_daemon_to_go() {
    let daemon = Daemon::start();
    daemon.kanna(&["open", "--name", "a", "--cmd", "exec sleep 600"]).ok();
    let app = daemon.app();
    let shown = format!("default ({})", daemon.socket().display());
    assert_eq!(daemon.kanna(&["instances", "stop"]).ok(), format!("stopped {shown}"));
    assert!(!daemon.socket().exists() && !daemon.dir.join("state/keeper.sock").exists());
    let _ = app.thread.join();
    assert_eq!(daemon.kanna(&["instances", "stop"]).ok(), format!("{shown} isn't running"));
    assert_eq!(daemon.kanna(&["instances", "stop", "--json"]).json()["stopped"], false);
}

#[test]
fn prune_deletes_dev_instances_whose_worktree_is_gone() {
    let dir = scratch("prune");
    let support = if cfg!(target_os = "macos") { dir.join("home/Library/Application Support") } else { dir.join("home/.local/share") };
    std::fs::create_dir_all(dir.join("here")).unwrap();
    let record = |folder: &str, worktree: Option<std::path::PathBuf>| {
        std::fs::create_dir_all(support.join(folder)).unwrap();
        std::fs::write(support.join(folder).join(kanna_build::RECORD), json!({ "channel": "dev", "worktree": worktree }).to_string()).unwrap();
    };
    record("kanna-dev-gone", Some(dir.join("gone")));
    record("kanna-dev-here", Some(dir.join("here")));
    record("kanna-dev", None);
    record("kanna-dev-running", Some(dir.join("gone2")));
    let _listener = serve(&support.join("kanna-dev-running/kanna.sock"), |_| json!({ "ok": true }));
    record("kanna-dev-kept", Some(dir.join("gone3")));
    std::fs::write(support.join("kanna-dev-kept/keeper.pid"), std::process::id().to_string()).unwrap();
    let text = kanna_at(&dir, None, &["instances", "prune"]).ok();
    assert!(text.contains(&format!("deleted {}", support.join("kanna-dev-gone").display())), "{text}");
    assert!(text.contains("kanna-dev-running: its worktree") && text.contains("but it's running"), "{text}");
    assert!(text.contains(&format!("its keeper (pid {}) still holds terminals", std::process::id())), "{text}");
    assert!(!support.join("kanna-dev-gone").exists());
    for kept in ["kanna-dev-here", "kanna-dev", "kanna-dev-running", "kanna-dev-kept"] {
        assert!(support.join(kept).exists(), "{kept}");
    }
    std::fs::remove_dir_all(support.join("kanna-dev-kept")).unwrap();
    std::fs::remove_dir_all(support.join("kanna-dev-running")).unwrap();
    assert!(kanna_at(&dir, None, &["instances", "prune"]).ok().starts_with("Nothing to prune"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_daemon_names_its_instance_and_tells_its_terminals() {
    let daemon = Daemon::start();
    let capabilities = daemon.kanna(&["capabilities", "--json"]).json();
    assert_eq!(capabilities["instance"], "default");
    assert_eq!(capabilities["mux"], "kmux");
    assert_eq!(capabilities["app"], false);
    daemon.kanna(&["open", "--name", "env", "--cmd", "echo \"I=$KANNA_INSTANCE T=$KANNA_TERMINAL P=${KANNA_PANE-none} S=$KANNA_SOCKET\"; exec sleep 60"]).ok();
    let expected = "I=default T=term-1 P=none S=/";
    std::thread::sleep(std::time::Duration::from_secs(2));
    let screen = daemon.kanna(&["read", "env"]).ok();
    assert!(screen.contains(expected), "{screen}");
}

#[test]
fn the_kmux_backend_is_kmux_through_kannas_cli() {
    let dir = scratch("kmux");
    let socket = dir.join("kmux.sock");
    let requests = serve(&socket, |request| match request["cmd"].as_str() {
        Some("capabilities") => json!({ "ok": true, "mux": "kmux", "paneTypes": ["term"], "commands": ["capabilities", "list"] }),
        Some("list") => json!({ "ok": true, "windows": [], "panes": [] }),
        _ => json!({ "ok": false, "error": { "code": "bad_request", "message": "unknown command" } }),
    });
    let run = |args: &[&str]| Run::from(args, cli(&dir).env("KMUX_SOCKET", &socket).args(args).output().unwrap());
    assert_eq!(run(&["--mux", "kmux", "workspace", "list", "--all"]).ok(), "no windows (open one with `kanna open`)");
    let overview = run(&["--mux", "kmux"]).ok();
    assert!(overview.contains("Backend: kmux (--mux kmux), not Kanna."), "{overview}");
    assert!(overview.contains("[not in the running kmux]"), "{overview}");
    assert!(requests.try_iter().any(|r| r["cmd"] == "list"));
    let mut command = cli(&dir);
    command.env("KANNA_MUX", "kmux").env("KMUX_SOCKET", &socket);
    assert_eq!(Run::from(&["workspace", "list", "--all"], command.args(["workspace", "list", "--all"]).output().unwrap()).ok(), "no windows (open one with `kanna open`)");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_for_a_command_opens_a_desk_on_its_project_without_a_task() {
    let daemon = Daemon::start();
    let _app = daemon.app();
    std::fs::create_dir_all(daemon.dir.join("project")).unwrap();
    let project = daemon.dir.join("project").to_string_lossy().to_string();
    assert_eq!(daemon.kanna(&["project"]).ok(), format!("opened d1 on {project}"));
    assert_eq!(daemon.kanna(&["project"]).ok(), format!("brought forward d1 on {project}"));
    let list = daemon.kanna(&["workspace", "list", "--all", "--json"]).json();
    assert_eq!((list["panes"].clone(), list["windows"].clone()), (json!([]), json!([])), "a Desk is the app's, and one on a project docks nothing: {list}");
    daemon.kanna(&["nowhere"]).fails(2, "unknown command \"nowhere\"");
}

#[test]
fn a_backlog_task_is_renamed_and_starts_with_its_new_name() {
    let daemon = Daemon::start();
    let _app = daemon.app();
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e"]).args(args).current_dir(&daemon.dir).status().unwrap().success());
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "first"]);
    let added = daemon.kanna(&["--json", "task", "new", "Old name", "say hi", "--agent", "/bin/cat"]).json();
    let id = added["task"]["id"].as_str().unwrap().to_string();
    assert!(id.len() == 13 && id.starts_with("task-") && id[5..].bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "{id}");
    let short = &id[5..9];
    assert_eq!(daemon.kanna(&["task", "name", short, "New name"]).ok(), format!("named task-{short} \"New name\""), "a start of its ID, without task-, and shown as its first 4 hex digits");
    let listed = daemon.kanna(&["--json", "task", "list"]).json()["backlog"][0].clone();
    assert_eq!((listed["name"].clone(), listed["message"].clone()), (json!("New name"), json!("say hi")));
    daemon.kanna(&["task", "name", "t9", "x"]).fails(4, "t9");
    daemon.kanna(&["task", "name", &id, " "]).fails(2, "a task needs a name");
    let started = daemon.kanna(&["--json", "task", "start", &format!("task-{short}")]).json();
    assert_eq!(started["task"], json!(id), "{started}");
    let folder = daemon.kanna(&["--json", "task", "list"]).json()["backlog"][0]["folder"].clone();
    assert!(folder.as_str().is_some_and(|f| f.ends_with(&format!(".kanna-worktrees/{id}"))), "its worktree is named after it: {folder}");
    let pane = started["pane"].as_str().unwrap().to_string();
    wait_for("the started task's pane", || daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["panes"].as_array().unwrap().iter().any(|p| p["id"] == pane.as_str() && p["name"] == "New name"));
    assert_eq!(daemon.kanna(&["task", "name", &id, "Newer name"]).ok(), format!("named task-{short} \"Newer name\""));
    assert!(daemon.kanna(&["workspace", "list", "--all", "--json"]).json()["panes"].as_array().unwrap().iter().any(|p| p["id"] == pane.as_str() && p["name"] == "Newer name"));
    assert_eq!(daemon.kanna(&["--json", "task", "list"]).json()["backlog"][0]["name"], json!("Newer name"));
}
