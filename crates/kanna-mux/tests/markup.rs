mod support;

use kanna_mux::json::json;
use support::{make_core, send};

#[test]
fn turning_comments_on_and_off_updates_the_running_pane_in_place() {
    let (core, host) = make_core();
    let agent = send(&core, json!({ "cmd": "open", "args": { "type": "term", "name": "agent" } }));
    let agent = agent["pane"]["id"].as_str().unwrap().to_string();
    let opened = send(&core, json!({ "cmd": "open", "args": { "type": "md", "name": "doc", "path": "/tmp/doc.md" } }));
    let doc = opened["pane"]["id"].as_str().unwrap().to_string();
    let on = send(&core, json!({ "cmd": "markup", "args": { "pane": "doc", "agent": "agent" } }));
    assert_eq!((on["pane"]["id"].clone(), on["pane"]["markup"].clone(), on["pane"]["agent"].clone()), (json!(doc), json!(true), json!(agent)), "{on}");
    let off = send(&core, json!({ "cmd": "markup", "args": { "pane": "doc", "off": true } }));
    assert!(off["pane"]["markup"].is_null(), "{off}");
    assert!(host.stopped.borrow().is_empty(), "{:?}", host.stopped.borrow());
    assert_eq!(*host.updated.borrow(), vec![format!("{doc} markup=true agent={agent}"), format!("{doc} markup=false agent={agent}")]);
}
