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
    assert_eq!(explained["reported"]["state"], "blocked", "{explained}");
    assert_eq!(explained["reported"]["kind"], "permission", "{explained}");
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
fn an_agent_s_report_is_its_status_until_it_clears() {
    let instance = Instance::start();
    instance.open("agent", Some(&mock_agent()));
    becomes(&instance, "agent", "idle");
    instance.call("send", json!({"pane": "agent", "text": "report state=working:app=mock:msg=VGhpbmtpbmc="}));
    becomes(&instance, "agent", "working");
    let explained = instance.call("status", json!({"action": "explain", "pane": "agent"}));
    assert_eq!(explained["reported"]["state"], "working", "{explained}");
    assert_eq!(explained["reported"]["app"], "mock");
    assert_eq!(explained["reported"]["message"], "Thinking");
    instance.call("send", json!({"pane": "agent", "text": "report state=blocked:kind=permission"}));
    becomes(&instance, "agent", "needs-you");
    instance.call("send", json!({"pane": "agent", "text": "report state=working:id=sub"}));
    instance.call("send", json!({"pane": "agent", "text": "report state=done"}));
    becomes(&instance, "agent", "idle");
    let explained = instance.call("status", json!({"action": "explain", "pane": "agent"}));
    assert_eq!(explained["records"].as_array().map(Vec::len), Some(2), "{explained}");
    instance.call("send", json!({"pane": "agent", "text": "report state=clear"}));
    wait_for("the status to clear", || status(&instance, "agent") == Value::Null);
    let explained = instance.call("status", json!({"action": "explain", "pane": "agent"}));
    assert_eq!(explained["reported"], Value::Null, "{explained}");
    assert_eq!(explained["records"], json!([]));
}

#[test]
fn an_agent_that_doesn_t_report_has_no_status() {
    let instance = Instance::start();
    instance.open("agent", Some(&format!("KANNA_MOCK_REPORTS=0 {}", mock_agent())));
    wait_for("the mock to start", || instance.call("read", json!({"pane": "agent"}))["text"].as_str().is_some_and(|t| t.contains("mock-agent:")));
    instance.call("send", json!({"pane": "agent", "text": "turn"}));
    std::thread::sleep(std::time::Duration::from_millis(2500));
    assert_eq!(status(&instance, "agent"), Value::Null, "the screen isn't read");
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

fn kept_reports(instance: &Instance) -> Vec<(String, Value)> {
    let db = rusqlite::Connection::open_with_flags(instance.dir.join("state/kanna.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut statement = db.prepare("select terminal, doc from status_reports order by terminal").unwrap();
    let rows = statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap();
    rows.map(|r| r.unwrap()).map(|(t, doc)| (t, serde_json::from_str(&doc).unwrap())).collect()
}

fn kept_state(instance: &Instance, terminal: &str) -> Option<String> {
    let kept = kept_reports(instance);
    let (_, doc) = kept.iter().find(|(t, _)| t == terminal)?;
    doc["records"].as_array()?.iter().find(|r| r["id"] == "").and_then(|r| r["state"].as_str()).map(String::from)
}

#[test]
fn an_agent_s_last_report_outlives_a_daemon_restart_and_one_sent_while_it_was_down_wins() {
    let mut instance = Instance::start();
    instance.open("agent", Some(&mock_agent()));
    instance.open("shell", None);
    let terminal = instance.pane("agent")["terminal"].as_str().unwrap().to_string();
    becomes(&instance, "agent", "idle");
    instance.call("send", json!({"pane": "agent", "text": "report state=idle:app=mock:msg=V2FpdGluZw=="}));
    wait_for("the report to be kept", || kept_reports(&instance).iter().any(|(t, doc)| *t == terminal && doc["records"][0]["message"] == "Waiting"));
    assert_eq!(kept_reports(&instance).len(), 1, "a plain terminal with no report keeps none");
    instance.restart(libc::SIGKILL);
    becomes(&instance, "agent", "idle");
    let explained = instance.call("status", json!({"action": "explain", "pane": "agent"}));
    assert_eq!(explained["reported"]["message"], "Waiting", "{explained}");
    assert_eq!(status(&instance, "shell"), Value::Null, "a plain terminal has nothing to restore");
    instance.restart(libc::SIGTERM);
    becomes(&instance, "agent", "idle");
    instance.call("send", json!({"pane": "agent", "text": "later 1.5 state=working"}));
    wait_for("the mock to take it", || instance.screen("agent").contains("later: state=working"));
    instance.stop(libc::SIGTERM);
    assert_eq!(kept_state(&instance, &terminal).as_deref(), Some("idle"));
    std::thread::sleep(std::time::Duration::from_millis(2500));
    instance.start_again();
    becomes(&instance, "agent", "working");
    wait_for("the newer report to be kept", || kept_state(&instance, &terminal).as_deref() == Some("working"));
    instance.call("send", json!({"pane": "agent", "text": "exit"}));
    becomes(&instance, "agent", "exited");
    wait_for("the ended terminal's report to go", || kept_reports(&instance).is_empty());
}
