mod common;

use common::{is, wait_for, Instance};
use serde_json::{json, Value};

const PROBE: &str = r#"python3 -c 'import os,sys,tty; tty.setraw(0); e=lambda a: a.encode().decode("unicode_escape").encode(); os.write(1,e(sys.argv[1])+b"ready"); r=b""
while r.count(e(sys.argv[2])) < int(sys.argv[3]): r+=os.read(0,64)
os.write(1,b"\r\ngot "+r.replace(b"\033",b"^").replace(b"\007",b"~")+b"\r\n")'"#;

fn probe(send: &str, until: &str, count: usize) -> String {
    format!("{PROBE} '{send}' '{until}' {count}; exec cat")
}

fn settings() -> Value {
    let palette: Vec<String> = (0..256).map(|i| format!("#{:02x}{:02x}40", i, 255 - i)).collect();
    json!({"foreground": "#e5d4b0", "background": "#123456", "cursor": "#2ec4b6", "palette": palette, "scheme": "dark",
           "cursorStyle": "bar", "cursorBlink": false, "graphemes": true})
}

#[test]
fn colour_queries_are_answered_from_the_apps_theme() {
    let mut instance = Instance::start();
    let mut app = instance.app();
    app.tell("settings", settings());
    drop(app);
    wait_for("the app to go", || instance.log().contains("the app went away"));
    let pane = instance.open("colours", Some(&probe("\\033]11;?\\007\\033]4;1;?\\007\\033[?996n", "n", 1)));
    wait_for("the replies", || instance.screen(&pane).contains("got "));
    let screen = instance.screen(&pane);
    assert!(screen.contains("got ^]11;rgb:1212/3434/5656~^]4;1;rgb:0101/fefe/4040~^[?997;1n"), "{screen}");

    instance.restart(libc::SIGTERM);
    let again = instance.open("again", Some(&probe("\\033]10;?\\007", "\\007", 1)));
    wait_for("the reply after a restart", || instance.screen(&again).contains("got "));
    let screen = instance.screen(&again);
    assert!(screen.contains("got ^]10;rgb:e5e5/d4d4/b0b0~"), "{screen}");
}

#[test]
fn a_new_colour_scheme_is_reported_to_programs_that_asked() {
    let instance = Instance::start();
    let pane = instance.open("scheme", Some(&probe("\\033[?2031h", "n", 2)));
    wait_for("the probe", || instance.screen(&pane).contains("ready"));
    let mut app = instance.app();
    app.tell("settings", settings());
    let mut light = settings();
    light["scheme"] = json!("light");
    app.tell("settings", light);
    wait_for("the report", || instance.screen(&pane).contains("got "));
    let screen = instance.screen(&pane);
    assert!(screen.contains("got ^[?997;1n^[?997;2n"), "{screen}");
}

#[test]
fn the_shells_folder_is_the_panes_cwd() {
    let instance = Instance::start();
    let pane = instance.open("cwd", Some("printf '\\033]7;file://localhost/private/tmp\\007'; exec cat"));
    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane}));
    app.until("the screen", |m| is(m, "snapshot", &pane));
    let _answering = app.answering();
    wait_for("the cwd", || instance.pane("cwd")["cwd"] == "/private/tmp");
    let listed = instance.call("list", json!({}));
    let found = listed["panes"].as_array().unwrap().iter().any(|p| p["name"] == "cwd" && p["cwd"] == "/private/tmp");
    assert!(found, "{listed}");
}

#[test]
fn a_new_terminal_in_a_task_window_starts_in_the_tasks_folder() {
    let instance = Instance::start();
    let _answering = instance.app().answering();
    let repo = instance.dir.join("home/.kanna-v4/repos/local/task");
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    std::fs::write(repo.join("sub/.keep"), "").unwrap();
    let repo = repo.canonicalize().unwrap();
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e"]).args(args).current_dir(&repo).status().unwrap().success());
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "first"]);
    let added = instance.call("backlog", json!({"action": "new", "name": "task", "repo": repo, "agent": "cd sub; printf '\\033]7;file://localhost/usr\\007'; exec cat #"}));
    assert_eq!(added["ok"], true, "{added}");
    let folder = repo.join(".kanna-worktrees").join(added["task"]["id"].as_str().unwrap());
    wait_for("the task, started by itself on the Shelf", || instance.pane("task")["cwd"] == "/usr");
    let agent = instance.pane("task")["id"].clone();
    let workspace = instance.call("task", json!({"action": "find", "task": agent}))["workspace"].clone();
    let window = instance.call("open", json!({"type": "term", "window": "new", "from": agent, "cmd": "exec cat"}))["window"].clone();
    for at in [json!({"split": "right", "workspace": workspace}), json!({"tab": true, "workspace": workspace}), json!({"tab": true, "window": window})] {
        let mut args = json!({"type": "term", "cmd": "pwd; exec cat"});
        args.as_object_mut().unwrap().extend(at.as_object().unwrap().clone());
        let beside = instance.call("open", args);
        let beside = beside["pane"]["id"].as_str().unwrap_or_else(|| panic!("{beside}")).to_string();
        wait_for("the task's folder", || instance.screen(&beside).replace('\n', "").contains(&*folder.to_string_lossy()));
    }
}
