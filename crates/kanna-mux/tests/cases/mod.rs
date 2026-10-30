#![allow(dead_code)]

use kanna_mux::json::{encode, num, Value};
use kanna_mux::DEBUG_COMMANDS;
use std::path::PathBuf;

pub type Send = Box<dyn FnMut(Value) -> Value>;

const KANNA_DIFFERS: [(&str, &str); 16] = [
    ("dock.json: move docks a pane, moves the docked pane to the other edge, and undocks it into a tab", "a docked pane is a task's agent, in its own workspace, not a window's"),
    ("dock.json: docking where a pane is docked sends that one to a new tab, not shown", "a docked pane is a task's agent, in its own workspace, not a window's"),
    ("dock.json: moving the docked pane to another window or beside a pane undocks it; dropping on it swaps", "a docked pane is a task's agent, in its own workspace, not a window's"),
    ("dock.json: open with dock makes the window's docked pane, outside its tabs, 2/5 wide on the left", "a docked pane is a task's agent, in its own workspace, not a window's"),
    ("dock.json: switching tabs leaves the docked pane; focus goes to the shown tab and back", "a docked pane is a task's agent, in its own workspace, not a window's"),
    ("dock.json: showing a tab with keep dock leaves focus in the focused docked pane; from a tab's pane focus follows", "a docked pane is a task's agent, in its own workspace, not a window's; owner tests cover keep"),
    ("dock.json: the docked pane closes only with its window; a window keeps it when its last tab closes", "a docked pane is a task's agent, in its own workspace, not a window's"),
    ("dock.json: moving a tab to another window leaves the docked pane where it is", "a docked pane is a task's agent, in its own workspace, not a window's"),
    ("dock.json: a frozen docked pane doesn't move and nothing drops on it; it still resizes, is saved frozen and comes back frozen", "a docked pane is a task's agent, in its own workspace, not a window's"),
    ("dock.json: zoom on a tab's pane leaves the docked pane; zoom on the docked pane fills the window; the zoom follows focus between them", "a docked pane is a task's agent, in its own workspace, not a window's"),
    ("dock.json: zoom on the docked pane of a window with no tabs fills the window, and stays on through a new tab pane and switching tabs", "a docked pane is a task's agent, in its own workspace, not a window's"),
    ("dock.json: arrange arranges the shown tab; the docked pane is not part of a layout", "a docked pane is a task's agent, in its own workspace, not a window's"),
    ("dock.json: the docked pane is saved with its window and comes back; a window with only a docked pane too", "a docked pane is a task's agent, in its own workspace, not a window's"),
    ("dock.json: restoring a dock repairs its edge and width, and refuses an unknown or twice-placed pane", "a docked pane is a task's agent, in its own workspace, not a window's"),
    ("persistence.json: a saved layout comes back: windows, tabs, splits, sizes, names, focus, zoom and what each pane shows; where windows are and how large panes are drawn are the app's, not saved here", "a request that names no window, with several windows, is refused: which window is in front is the app's, not the daemon's"),
    ("workspaces.json: a free window stays free: freeing it changes nothing, and assign needs a window and a workspace that are there", "whether a window shows is each app's, so replies don't say shown"),
];

pub fn cases_directory() -> PathBuf {
    let kmux = std::env::var_os("KMUX_DIR").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../references/kmux"));
    kmux.join("tests/kmux-protocol/cases")
}

pub fn mismatch(expected: &Value, actual: Option<&Value>, path: &str) -> Option<String> {
    let actual = actual.unwrap_or(&Value::Null);
    match expected {
        Value::Array(items) => {
            let Value::Array(got) = actual else { return Some(format!("{path}: expected an array, got {}", encode(actual))) };
            if got.len() != items.len() {
                return Some(format!("{path}: expected {} items, got {}", items.len(), got.len()));
            }
            items.iter().zip(got).enumerate().find_map(|(i, (item, got))| mismatch(item, Some(got), &format!("{path}[{i}]")))
        }
        Value::Object(fields) => {
            let Value::Object(got) = actual else { return Some(format!("{path}: expected an object, got {}", encode(actual))) };
            fields
                .iter()
                .filter(|(key, value)| !(key.as_str() == "title" && is_tab(path) && numbered(value)))
                .filter(|(key, _)| !(key.as_str() == "key" && (path.starts_with("windows[") || path.ends_with("state"))))
                .find_map(|(key, value)| mismatch(value, got.get(key), &if path.is_empty() { key.clone() } else { format!("{path}.{key}") }))
        }
        _ => (!kanna_mux::json::same(expected, actual)).then(|| format!("{path}: expected {}, got {}", encode(expected), encode(actual))),
    }
}

fn content_id(text: &str) -> bool {
    text.split_once('-').is_some_and(|(prefix, n)| ["term", "file", "page", "ios", "diff"].contains(&prefix) && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn learn(expected: &Value, actual: &Value, ids: &mut Vec<(String, String)>) {
    match (expected, actual) {
        (Value::String(kmux), Value::String(ours)) if content_id(kmux) && content_id(ours) => {
            if !ids.iter().any(|(k, o)| k == kmux || o == ours) {
                ids.push((kmux.clone(), ours.clone()));
            }
        }
        (Value::Array(want), Value::Array(got)) => want.iter().zip(got).for_each(|(w, g)| learn(w, g, ids)),
        (Value::Object(want), Value::Object(got)) => want.iter().for_each(|(key, w)| {
            if let Some(g) = got.get(key) {
                learn(w, g, ids);
            }
        }),
        _ => {}
    }
}

fn translate(value: &Value, ids: &[(String, String)], to_ours: bool) -> Value {
    match value {
        Value::String(text) => ids
            .iter()
            .find(|(kmux, ours)| if to_ours { kmux == text } else { ours == text })
            .map(|(kmux, ours)| Value::String(if to_ours { ours.clone() } else { kmux.clone() }))
            .unwrap_or_else(|| value.clone()),
        Value::Array(items) => Value::Array(items.iter().map(|item| translate(item, ids, to_ours)).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(key, item)| (key.clone(), translate(item, ids, to_ours))).collect()),
        _ => value.clone(),
    }
}

fn is_tab(path: &str) -> bool {
    path.rsplit_once("tabs[").is_some_and(|(_, index)| index.strip_suffix(']').is_some_and(|i| i.parse::<usize>().is_ok()))
}

fn numbered(title: &Value) -> bool {
    title.as_str().and_then(|t| t.strip_prefix("Tab ")).is_some_and(|n| n.parse::<usize>().is_ok())
}

fn listed(reply: &Value, key: &str) -> Vec<String> {
    reply[key].as_array().map(|t| t.iter().filter_map(|t| t.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub fn run_cases(label: &str, make: &dyn Fn() -> Send) -> usize {
    let mut files: Vec<PathBuf> = std::fs::read_dir(cases_directory())
        .unwrap_or_else(|e| panic!("kmux is the reference model for Kanna's mux core, and its protocol cases aren't at {} ({e}); they come from references/kmux, or KMUX_DIR", cases_directory().display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no cases in {}", cases_directory().display());
    let capabilities = make()(serde_json::json!({ "id": 0, "cmd": "capabilities" }));
    let types = listed(&capabilities, "paneTypes");
    let commands = listed(&capabilities, "commands");

    let (mut passed, mut failed, mut skipped) = (0, vec![], vec![]);
    for file in &files {
        let name = file.file_name().unwrap().to_string_lossy();
        let cases: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        let Value::Array(cases) = cases else { panic!("{name} is not an array of cases") };
        for case in &cases {
            let title = format!("{name}: {}", case["name"].as_str().unwrap_or("?"));
            let Some(steps) = case["steps"].as_array() else { continue };
            if let Some((_, why)) = KANNA_DIFFERS.iter().find(|(differs, _)| *differs == title) {
                skipped.push(format!("{title} (Kanna differs: {why})"));
                continue;
            }
            let mut missing: Vec<String> = steps
                .iter()
                .filter_map(|step| {
                    let send = &step["send"];
                    let cmd = send["cmd"].as_str()?;
                    if !commands.iter().any(|c| c == cmd) && !DEBUG_COMMANDS.contains(&cmd) && cmd != "nope" {
                        return Some(cmd.to_string());
                    }
                    let kind = send["args"]["type"].as_str()?;
                    (cmd == "open" && ["term", "web", "ios", "md"].contains(&kind) && !types.iter().any(|t| t == kind)).then(|| format!("{kind} panes"))
                })
                .collect();
            if !missing.is_empty() {
                missing.sort();
                missing.dedup();
                skipped.push(format!("{title} (needs {})", missing.join(", ")));
                continue;
            }
            let mut send = make();
            let mut problem = None;
            let mut ids: Vec<(String, String)> = vec![];
            for (index, step) in steps.iter().enumerate() {
                let Value::Object(mut request) = translate(&step["send"], &ids, true) else { continue };
                request.insert("id".into(), num((index + 1) as f64));
                let reply = send(Value::Object(request));
                let mut expected = step.get("expect").cloned().unwrap_or(serde_json::json!({ "ok": true }));
                learn(&expected, &reply, &mut ids);
                if !ids.is_empty() {
                    if let Some(counters) = expected.get_mut("state").and_then(|s| s.get_mut("counters")).and_then(Value::as_object_mut) {
                        counters.remove("pane");
                    }
                }
                let reply = translate(&reply, &ids, false);
                if let Some(why) = mismatch(&expected, Some(&reply), "") {
                    problem = Some(format!("{title}\n  step {}: {why}\n  reply: {}", index + 1, encode(&reply)));
                    break;
                }
            }
            match problem {
                Some(problem) => failed.push(problem),
                None => passed += 1,
            }
        }
    }
    println!("{label}: {passed} cases passed, {} failed, {} skipped", failed.len(), skipped.len());
    for skip in &skipped {
        println!("  skipped {skip}");
    }
    let unmatched: Vec<&str> = KANNA_DIFFERS.iter().map(|(title, _)| *title).filter(|title| !skipped.iter().any(|s| s.starts_with(title))).collect();
    assert!(unmatched.is_empty(), "kmux has no such cases any more: {}", unmatched.join("; "));
    assert!(failed.is_empty(), "{} cases failed:\n{}", failed.len(), failed.join("\n"));
    passed
}
