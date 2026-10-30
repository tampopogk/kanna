mod support;

use kanna_mux::json::{json, Value};
use kanna_mux::Core;
use support::{make_core, make_remote_core, send};

fn two_panes() -> Core {
    let (core, _) = make_core();
    for name in ["a", "b"] {
        let reply = send(&core, json!({ "cmd": "open", "args": { "type": "term", "name": name } }));
        assert_eq!(reply["ok"], true, "{reply}");
    }
    core
}

#[test]
fn arrange_with_no_space_left_names_the_unsized_panes() {
    let core = two_panes();
    let reply = send(&core, json!({ "cmd": "arrange", "args": { "layout": { "split": "row", "children": [{ "pane": "a", "size": 1 }, { "pane": "b" }] } } }));
    assert_eq!(reply["error"]["code"], "layout_invalid");
    assert_eq!(reply["error"]["message"], "no space left for the unsized panes in a row");
}

#[test]
fn resizing_a_pane_that_had_the_whole_split_shares_the_rest_equally() {
    let core = two_panes();
    assert_eq!(send(&core, json!({ "cmd": "open", "args": { "type": "term", "name": "c" } }))["ok"], true);
    let mut state = send(&core, json!({ "cmd": "debug.saveState" }))["state"].clone();
    state["windows"][0]["tabs"][0]["layout"] =
        json!({ "split": "row", "children": [{ "pane": "p1", "size": 1 }, { "pane": "p2", "size": 1e-300 }, { "pane": "p3", "size": 1e-300 }] });
    let restored = send(&core, json!({ "cmd": "debug.restoreState", "args": { "state": state } }));
    assert_eq!(restored["ok"], true, "{restored}");
    let reply = send(&core, json!({ "cmd": "resize", "args": { "pane": "a", "size": "1/2" } }));
    assert_eq!(reply["ok"], true, "{reply}");
    let sizes: Vec<Value> = reply["layout"]["children"].as_array().unwrap().iter().map(|kid| kid["size"].clone()).collect();
    assert_eq!(sizes, vec![json!("1/2"), json!("1/4"), json!("1/4")], "{reply}");
    let saved = send(&core, json!({ "cmd": "debug.saveState" }));
    let kids = saved["state"]["windows"][0]["tabs"][0]["layout"]["children"].as_array().unwrap().clone();
    assert!(kids.iter().all(|kid| kid["size"].as_f64().is_some_and(f64::is_finite)), "{saved}");
}

#[test]
fn open_docks_only_a_task_s_agent() {
    let (core, _) = make_core();
    let refused = send(&core, json!({ "cmd": "open", "args": { "type": "term", "name": "agent", "dock": "left" } }));
    assert_eq!(refused["error"]["code"], "not_a_task", "{refused}");
    assert_eq!(refused["error"]["message"], "a docked pane is a task's agent, and only a task docks: make one with `kanna task new`, then `kanna task pick` or `kanna task start`");
    assert_eq!(send(&core, json!({ "cmd": "list", "args": { "all": true } }))["panes"], json!([]));
    assert_eq!(send(&core, json!({ "cmd": "open", "args": { "type": "term", "name": "agent", "dock": "left", "task": "task-1" } }))["ok"], true);
}

#[test]
fn move_docks_only_a_task_and_moves_its_agent_to_the_other_edge_of_its_workspace() {
    let (core, _) = make_core();
    for args in [json!({ "type": "term", "name": "a" }), json!({ "type": "term", "name": "agent", "dock": "left", "task": "task-1" })] {
        assert_eq!(send(&core, json!({ "cmd": "open", "args": args }))["ok"], true);
    }
    let refused = send(&core, json!({ "cmd": "move", "args": { "pane": "a", "dock": "left" } }));
    assert_eq!(refused["error"]["code"], "not_a_task", "{refused}");
    assert_eq!(refused["error"]["message"], "a isn't a task, and only a task's agent docks: start one with `kanna task new` and `kanna task start`");
    let windowed = send(&core, json!({ "cmd": "open", "args": { "type": "term", "name": "other", "dock": "left", "task": "task-2", "window": "win-1" } }));
    assert_eq!(windowed["error"]["code"], "bad_request", "{windowed}");

    let flipped = send(&core, json!({ "cmd": "move", "args": { "pane": "agent", "dock": "right" } }));
    assert_eq!(flipped["dock"], json!({ "pane": "agent", "edge": "right", "width": "2/5" }), "{flipped}");
    let elsewhere = send(&core, json!({ "cmd": "move", "args": { "pane": "agent", "dock": "left", "window": "win-1" } }));
    assert_eq!(elsewhere["error"]["code"], "bad_request", "a task's agent stays in its workspace: {elsewhere}");
}

fn desk_task_with_a_tab() -> Core {
    let (core, _) = make_core();
    let task = send(&core, json!({ "cmd": "open", "args": { "type": "term", "name": "agent", "dock": "left", "task": "task-1" } }));
    assert_eq!(task["ok"], true, "{task}");
    let tab = send(&core, json!({ "cmd": "open", "args": { "type": "term", "name": "side", "tab": true, "from": "agent" } }));
    assert_eq!(tab["ok"], true, "{tab}");
    core
}

fn everything(core: &Core) -> Value {
    send(core, json!({ "cmd": "list", "args": { "all": true } }))
}

#[test]
fn closing_a_desk_task_closes_its_tabs_and_leaves_no_window() {
    let core = desk_task_with_a_tab();
    let closed = send(&core, json!({ "cmd": "close", "args": { "task": "agent" } }));
    assert_eq!(closed["closed"], json!(["p1", "p2"]), "{closed}");
    let list = everything(&core);
    assert_eq!((list["windows"].clone(), list["hiddenWindows"].clone(), list["workspaces"].clone()), (json!([]), json!([]), json!([])), "{list}");
}

fn dead_pane_in_a_window() -> Core {
    let (core, _, app) = make_remote_core();
    assert_eq!(send(&core, json!({ "cmd": "open", "args": { "type": "term", "name": "live" } }))["ok"], true);
    let placed = app.owner.borrow_mut().handle(&json!({ "call": "serve", "request": { "op": "place", "pane": { "id": "p9", "type": "diff" }, "at": { "window": "new" } } }));
    assert_eq!(placed["window"], "win-2", "{placed}");
    core
}

#[test]
fn closing_a_window_of_panes_the_daemon_doesnt_know_removes_the_window() {
    let core = dead_pane_in_a_window();
    let closed = send(&core, json!({ "cmd": "close", "args": { "window": "win-2" } }));
    assert_eq!(closed["closed"], json!(["p9"]), "{closed}");
    let list = everything(&core);
    assert_eq!(list["windows"].as_array().unwrap().iter().map(|w| w["id"].clone()).collect::<Vec<_>>(), vec![json!("win-1")], "{list}");
}

#[test]
fn closing_a_tab_of_panes_the_daemon_doesnt_know_removes_the_tab() {
    let core = dead_pane_in_a_window();
    let list = everything(&core);
    let tab = list["windows"][1]["tabs"][0]["id"].clone();
    let closed = send(&core, json!({ "cmd": "close", "args": { "tab": tab } }));
    assert_eq!(closed["closed"], json!(["p9"]), "{closed}");
    assert_eq!(everything(&core)["windows"].as_array().unwrap().len(), 1);
}

#[test]
fn closing_a_task_closes_its_workspace_and_its_windows() {
    let core = desk_task_with_a_tab();
    let window = send(&core, json!({ "cmd": "open", "args": { "type": "term", "name": "own", "window": "new", "from": "agent" } }));
    assert_eq!(window["ok"], true, "{window}");
    let closed = send(&core, json!({ "cmd": "close", "args": { "task": "agent" } }));
    assert_eq!(closed["ok"], true, "{closed}");
    let list = everything(&core);
    assert_eq!((list["windows"].clone(), list["hiddenWindows"].clone(), list["workspaces"].clone(), list["panes"].clone()), (json!([]), json!([]), json!([]), json!([])), "{list}");
}
