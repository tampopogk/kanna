mod support;

use kanna_mux::json::{json, Value};
use support::{make_core, send};

fn state(pane: &str, tab: &str, window: &str) -> Value {
    json!({
        "kmux": "state", "version": 1, "key": window, "counters": {},
        "windows": [{ "id": window, "active": tab, "tabCount": 1, "tabs": [{ "id": tab, "title": "Tab 1", "layout": { "pane": pane } }] }],
        "panes": [{ "id": pane, "type": "term" }]
    })
}

#[test]
fn ids_too_big_to_count_from_are_bad_state() {
    let huge = "18446744073709551615";
    for (pane, tab, window) in [
        (format!("p{huge}"), "t1".to_string(), "w1".to_string()),
        ("p1".into(), format!("t{huge}"), "w1".into()),
        ("p1".into(), "t1".into(), format!("w{huge}")),
        ("p1000000000".into(), "t1".into(), "w1".into()),
    ] {
        let (core, _) = make_core();
        let reply = send(&core, json!({ "cmd": "debug.restoreState", "args": { "state": state(&pane, &tab, &window) } }));
        assert_eq!(reply["ok"], false, "{pane} {tab} {window}: {reply}");
        assert_eq!(reply["error"]["code"], "bad_request");
        let opened = send(&core, json!({ "cmd": "open", "args": { "type": "term", "tab": true } }));
        assert_eq!(opened["ok"], true, "{opened}");
    }
}

#[test]
fn the_largest_ids_still_restore_and_count_on() {
    let (core, _) = make_core();
    let reply = send(&core, json!({ "cmd": "debug.restoreState", "args": { "state": state("p999999999", "t999999999", "w999999999") } }));
    assert_eq!(reply["ok"], true, "{reply}");
    let opened = send(&core, json!({ "cmd": "open", "args": { "type": "term", "tab": true } }));
    assert_eq!(opened["pane"]["id"], "p1000000000", "{opened}");
    assert_eq!(opened["tab"], "tab-1000000000", "{opened}");
}

#[derive(Default)]
struct ExitsOnStop {
    core: std::cell::RefCell<Option<kanna_mux::WeakCore>>,
}

impl kanna_mux::PaneHost for ExitsOnStop {
    fn start(&self, _: &kanna_mux::Pane) {}
    fn stop(&self, pane: &kanna_mux::Pane) {
        if let Some(core) = self.core.borrow().as_ref().and_then(kanna_mux::WeakCore::upgrade) {
            core.update(&pane.id, kanna_mux::PaneState::Exited, Some(0), None);
        }
    }
    fn send(&self, _: &kanna_mux::Pane, _: &str, done: kanna_mux::Responder) {
        done.reply(Ok(Default::default()));
    }
}

#[test]
fn an_old_pane_stopping_at_once_leaves_the_restored_pane_with_its_id_alone() {
    let core = kanna_mux::Core::default();
    let host = std::rc::Rc::new(ExitsOnStop::default());
    *host.core.borrow_mut() = Some(core.weak());
    core.set_host(Some(host));
    let opened = send(&core, json!({ "cmd": "open", "args": { "type": "term", "wait": false } }));
    assert_eq!(opened["pane"]["id"], "p1", "{opened}");
    let restored = send(&core, json!({ "cmd": "debug.restoreState", "args": { "state": state("p1", "t1", "w1") } }));
    assert_eq!(restored["ok"], true, "{restored}");
    let list = send(&core, json!({ "cmd": "list" }));
    assert_eq!(list["panes"][0]["state"], "starting", "{list}");
    assert_eq!(list["panes"][0]["exitCode"], Value::Null, "{list}");
}

#[test]
fn a_bad_layout_changes_nothing() {
    let (core, host) = make_core();
    assert_eq!(send(&core, json!({ "cmd": "open", "args": { "type": "term" } }))["ok"], true);
    let mut bad = state("p1", "t1", "w1");
    bad["windows"][0]["tabs"][0]["layout"] = json!({ "pane": "p9" });
    let reply = send(&core, json!({ "cmd": "debug.restoreState", "args": { "state": bad } }));
    assert_eq!(reply["error"]["code"], "bad_request", "{reply}");
    assert!(host.stopped.borrow().is_empty());
    let list = send(&core, json!({ "cmd": "list" }));
    assert_eq!(list["panes"][0]["state"], "running", "{list}");
}
