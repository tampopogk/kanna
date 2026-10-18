mod common;

use common::{wait_for, Instance};
use serde_json::{json, Value};
use std::path::PathBuf;

fn mock_agent() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/mock-agent/mock-agent").canonicalize().unwrap().display().to_string()
}

fn status(instance: &Instance, name: &str) -> Value {
    instance.pane(name)["status"].clone()
}

#[track_caller]
fn becomes(instance: &Instance, name: &str, wanted: &str) {
    wait_for(&format!("{name} to be {wanted}"), || status(instance, name) == wanted);
}

#[test]
fn an_agent_goes_working_needs_you_idle_and_exited_in_list_and_on_the_app_link() {
    let instance = Instance::start();
    let mut app = instance.app();
    let (notices, heard) = std::sync::mpsc::channel::<Value>();
    std::thread::spawn(move || {
        while let Some(message) = app.next() {
            if message["cmd"] == "status" && notices.send(message["args"].clone()).is_err() {
                return;
            }
        }
    });
    instance.open("agent", Some(&mock_agent()));
    let pane = instance.pane("agent")["id"].as_str().unwrap().to_string();
    becomes(&instance, "agent", "idle");
    instance.call("send", json!({"pane": "agent", "text": "turn"}));
    becomes(&instance, "agent", "working");
    becomes(&instance, "agent", "needs-you");
    let explained = instance.call("status", json!({"action": "explain", "pane": "agent"}));
    assert_eq!(explained["agent"], "mock", "{explained}");
    assert_eq!(explained["rule"], "mock/needs-you/proceed");
    assert_eq!(explained["matched"], "Do you want to proceed?");
    assert_eq!(explained["rules"]["source"], "bundled");
    instance.call("send", json!({"pane": "agent", "text": "1"}));
    becomes(&instance, "agent", "working");
    becomes(&instance, "agent", "idle");
    instance.call("send", json!({"pane": "agent", "text": "exit"}));
    becomes(&instance, "agent", "exited");
    let mut seen = vec![];
    while seen.last().map(String::as_str) != Some("exited") {
        let notice = heard.recv_timeout(std::time::Duration::from_secs(10)).expect("a status notice");
        if notice["pane"] == pane.as_str() {
            seen.push(notice["status"].as_str().unwrap_or("none").to_string());
        }
    }
    seen.dedup();
    assert_eq!(seen, ["idle", "working", "needs-you", "working", "idle", "exited"]);
}

#[test]
fn an_agent_s_own_report_wins_over_the_rules_until_it_clears() {
    let instance = Instance::start();
    instance.open("agent", Some(&mock_agent()));
    becomes(&instance, "agent", "idle");
    let explained = instance.call("status", json!({"action": "explain", "pane": "agent"}));
    assert_eq!(explained["source"], "rules", "{explained}");
    instance.call("send", json!({"pane": "agent", "text": "report state=working:app=mock:msg=VGhpbmtpbmc="}));
    becomes(&instance, "agent", "working");
    let explained = instance.call("status", json!({"action": "explain", "pane": "agent"}));
    assert_eq!(explained["source"], "osc7501", "{explained}");
    assert_eq!(explained["judged"], "idle", "the screen alone says idle: {explained}");
    assert_eq!(explained["reported"]["state"], "working");
    assert_eq!(explained["reported"]["app"], "mock");
    assert_eq!(explained["reported"]["message"], "Thinking");
    instance.call("send", json!({"pane": "agent", "text": "report state=blocked:kind=permission"}));
    becomes(&instance, "agent", "needs-you");
    instance.call("send", json!({"pane": "agent", "text": "report state=working:id=sub"}));
    instance.call("send", json!({"pane": "agent", "text": "report state=done"}));
    becomes(&instance, "agent", "idle");
    let explained = instance.call("status", json!({"action": "explain", "pane": "agent"}));
    assert_eq!(explained["records"].as_array().map(Vec::len), Some(2), "{explained}");
    instance.call("send", json!({"pane": "agent", "text": "report state=working"}));
    becomes(&instance, "agent", "working");
    instance.call("send", json!({"pane": "agent", "text": "report state=clear"}));
    becomes(&instance, "agent", "idle");
    let explained = instance.call("status", json!({"action": "explain", "pane": "agent"}));
    assert_eq!(explained["source"], "rules", "{explained}");
    assert_eq!(explained["records"], json!([]));
}

#[test]
fn other_terminals_show_only_exited() {
    let instance = Instance::start();
    instance.open("shell", None);
    instance.open("done", Some("exit 3"));
    wait_for("done to exit", || instance.pane("done")["state"] == "exited");
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert_eq!(status(&instance, "shell"), Value::Null);
    assert_eq!(status(&instance, "done"), "exited");
    let explained = instance.call("status", json!({"action": "explain", "pane": "shell"}));
    assert_eq!(explained["agent"], Value::Null, "{explained}");
}

#[test]
fn a_local_override_wins_and_reloads_when_it_changes() {
    let dir = std::env::temp_dir().join(format!("kanna-status-override-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let instance = Instance::start_with(&[("KANNA_AGENT_RULES_DIR", dir.to_str().unwrap())]);
    instance.open("agent", Some(&mock_agent()));
    becomes(&instance, "agent", "idle");
    instance.call("send", json!({"pane": "agent", "text": "hello"}));
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(status(&instance, "agent"), "idle");
    let rules = r#"{"schema":1,"agent":"mock","version":"0.1","engine":1,"rules":[{"id":"mine","status":"needs-you","priority":1,"when":{"anyLine":{"contains":"echo: hello"}}}]}"#;
    std::fs::write(dir.join("mock.json"), rules).unwrap();
    becomes(&instance, "agent", "needs-you");
    let explained = instance.call("status", json!({"action": "explain", "pane": "agent"}));
    assert!(explained["rules"]["source"].as_str().unwrap().starts_with("override "), "{explained}");
    std::fs::remove_file(dir.join("mock.json")).unwrap();
    becomes(&instance, "agent", "idle");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_saved_screen_is_explained_with_an_agent_s_rules() {
    let instance = Instance::start();
    let file = instance.dir.join("screen.txt");
    std::fs::write(&file, "some output\nDo you want to proceed?\n❯ 1. Yes\n  2. No\n").unwrap();
    let explained = instance.call("status", json!({"action": "explain", "agent": "mock", "file": file}));
    assert_eq!(explained["judged"], "needs-you", "{explained}");
    let case = instance.dir.join("case.json");
    std::fs::write(&case, json!({"rows": ["⠋ Working (3s • esc to interrupt)", ""], "title": "x", "expected": "working", "source": "t"}).to_string()).unwrap();
    let explained = instance.call("status", json!({"action": "explain", "agent": "mock", "file": case}));
    assert_eq!(explained["judged"], "working", "{explained}");
    let unknown = instance.call("status", json!({"action": "explain", "agent": "nobody", "screen": {"text": "x"}}));
    assert_eq!(unknown["ok"], false);
    let update = instance.call("status", json!({"action": "update"}));
    assert_eq!(update["problems"][0], "no rules site is set", "{update}");
    assert!(update["rules"].as_array().unwrap().iter().any(|r| r["agent"] == "claude"));
}
