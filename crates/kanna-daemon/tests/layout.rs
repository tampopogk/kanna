mod common;

use common::{App, Instance};
use serde_json::json;

#[test]
fn with_no_app_the_daemon_places_panes_and_layout_commands_work() {
    let instance = Instance::start();
    let opened = instance.call("open", json!({"type": "term", "name": "plain"}));
    assert_eq!((opened["window"].clone(), opened["tab"].clone()), (json!("w1"), json!("t1")), "{opened}");

    for (cmd, args) in [
        ("open", json!({"type": "term", "name": "split", "split": "right"})),
        ("open", json!({"type": "term", "name": "tabbed", "tab": true})),
        ("focus", json!({"pane": "plain"})),
        ("arrange", json!({"layout": {"split": "row", "children": [{"pane": "plain"}, {"pane": "split"}]}})),
        ("close", json!({"tab": "t2"})),
    ] {
        let reply = instance.call(cmd, args.clone());
        assert_eq!(reply["ok"], true, "{cmd} {args}: {reply}");
    }
    let list = instance.call("list", json!({}));
    assert_eq!(list["windows"][0]["focused"], "plain", "{list}");
    assert_eq!(list["windows"][0]["tabs"][0]["layout"], json!({"split": "row", "children": [{"pane": "plain", "size": "1/2"}, {"pane": "split", "size": "1/2"}]}), "{list}");
    let doc = instance.call("open", json!({"type": "md", "name": "doc", "path": "/tmp/x.md"}));
    assert_eq!(doc["pane"]["state"], "running", "{doc}");
    let doc = instance.pane("doc");
    assert_eq!(doc["state"], "running", "{doc}");
    assert_eq!(instance.call("debug.app", json!({}))["error"]["code"], "no_app");
}

#[test]
fn an_app_that_attaches_later_gets_the_layout_as_it_was_left_and_every_change() {
    let instance = Instance::start();
    instance.open("early", Some("cat"));
    let mut app = App::connect(&instance.socket());
    assert_eq!(app.view["windows"][0]["root"], json!({"pane": "p1", "type": "term"}), "{}", app.view);
    let opened = instance.call("open", json!({"type": "term", "name": "right", "cmd": "cat", "split": "right", "size": "1/3"}));
    assert_eq!((opened["window"].clone(), opened["tab"].clone()), (json!("w1"), json!("t1")), "{opened}");
    let view = app.until("the split", |m| m["cmd"] == "layout" && m["args"]["windows"][0]["root"]["split"] == "row");
    assert_eq!(view["args"]["windows"][0]["root"]["children"][1]["pane"], "p2", "{view}");
    drop(app);
    assert_eq!(instance.call("close", json!({"pane": "right"}))["ok"], true);
    let list = instance.call("list", json!({}));
    assert_eq!(list["windows"][0]["tabs"][0]["layout"], json!({"pane": "early"}), "{list}");
}
