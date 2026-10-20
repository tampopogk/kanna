//! Human-readable output for replies.

use serde_json::{json, Value};

pub fn join(list: &Value) -> String {
    list.as_array().map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

fn label(pane: &Value) -> String {
    let id = pane["id"].as_str().unwrap_or("?");
    match pane["name"].as_str() {
        Some(name) => format!("{id} ({name})"),
        None => id.to_string(),
    }
}

fn state(pane: &Value) -> String {
    let state = pane["state"].as_str().unwrap_or("?");
    match (pane["exitCode"].as_i64(), pane["error"].as_str()) {
        (Some(code), _) => format!("{state} ({code})"),
        (_, Some(error)) => format!("{state}: {error}"),
        _ => state.to_string(),
    }
}

/// The first line of `text`, cut to fit a table column.
fn brief(text: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    if line.chars().count() > 60 { format!("{}…", line.chars().take(59).collect::<String>()) } else { line.to_string() }
}

/// Where a reply's window is: `win-1`, or `ws-1's win-2` in a workspace, with
/// `(hidden)` after a hidden one.
fn place(reply: &Value) -> String {
    let window = reply["window"].as_str().unwrap_or("?");
    let workspace = &reply["workspace"];
    match workspace["id"].as_str() {
        Some(id) if workspace["hidden"] == true => format!("{id}'s {window} (hidden)"),
        Some(id) => format!("{id}'s {window}"),
        None => window.to_string(),
    }
}

pub fn opened(request: &Value, reply: &Value) -> String {
    let place = place(reply);
    if let Some(edge) = reply["dock"].as_str() {
        let frozen = if request["frozen"] == true { ", frozen," } else { "" };
        return format!("opened {} docked {edge}{frozen} in {place}: {}", label(&reply["pane"]), state(&reply["pane"]));
    }
    format!("opened {} in {place}, {}: {}", label(&reply["pane"]), reply["tab"].as_str().unwrap_or("?"), state(&reply["pane"]))
}

fn workspace_label(workspace: &Value) -> String {
    let id = workspace["id"].as_str().unwrap_or("?");
    match workspace["name"].as_str() {
        Some(name) => format!("{id} ({name})"),
        None => id.to_string(),
    }
}

pub fn scoped(reply: &Value, pane: &str) -> Option<Value> {
    let empty = vec![];
    let panes = reply["panes"].as_array().unwrap_or(&empty);
    let row = panes.iter().find(|p| p["id"] == pane || p["name"] == pane)?;
    let id = row["id"].as_str().unwrap_or(pane);
    let shown = row["name"].as_str().unwrap_or(id);
    let named = |v: &Value| v == shown || v == id;
    fn holds(node: &Value, named: &dyn Fn(&Value) -> bool) -> bool {
        named(&node["pane"]) || node["children"].as_array().is_some_and(|kids| kids.iter().any(|k| holds(k, named)))
    }
    let windows = reply["windows"].as_array().unwrap_or(&empty);
    let mine = windows.iter().find(|w| named(&w["dock"]["pane"]) || w["tabs"].as_array().is_some_and(|tabs| tabs.iter().any(|t| holds(&t["layout"], &named))));
    let Some(mine) = mine else { return Some(json!({ "pane": id, "windows": [], "workspaces": [], "panes": [row] })) };
    let workspaces: Vec<Value> = reply["workspaces"].as_array().unwrap_or(&empty).iter().filter(|w| !mine["workspace"].is_null() && w["id"] == mine["workspace"]).cloned().collect();
    let ours: Vec<Value> = windows.iter().filter(|w| w["id"] == mine["id"] || (!mine["workspace"].is_null() && w["workspace"] == mine["workspace"])).cloned().collect();
    fn names(node: &Value, out: &mut Vec<Value>) {
        out.extend(node.get("pane").cloned());
        node["children"].as_array().into_iter().flatten().for_each(|kid| names(kid, out));
    }
    let mut inside = vec![];
    for window in &ours {
        inside.extend(window["dock"].get("pane").cloned());
        window["tabs"].as_array().into_iter().flatten().for_each(|tab| names(&tab["layout"], &mut inside));
    }
    let kept: Vec<Value> = panes.iter().filter(|p| inside.iter().any(|n| *n == p["id"] || *n == p["name"]) || p["id"] == id).cloned().collect();
    Some(json!({ "pane": id, "windows": ours, "workspaces": workspaces, "panes": kept }))
}

pub fn workspace(args: &Value, reply: &Value) -> String {
    if args["action"] == "list" {
        return match (reply["pane"].as_str(), reply["windows"].as_array().is_some_and(|w| w.is_empty())) {
            (Some(pane), true) => format!("{pane} isn't in a window"),
            _ => list(reply),
        };
    }
    let windows = join(&reply["windows"]);
    let label = workspace_label(&reply["workspace"]);
    match args["action"].as_str() {
        Some("new") => {
            let id = reply["workspace"]["id"].as_str().unwrap_or("?");
            format!("made {label}: put windows in it with `kmux window assign WINDOW {id}`")
        }
        Some("close") => match join(&reply["closed"]) {
            none if none.is_empty() => format!("closed {} (it had no panes)", reply["workspace"].as_str().unwrap_or("?")),
            panes => format!("closed {} (panes {panes})", reply["workspace"].as_str().unwrap_or("?")),
        },
        action if windows.is_empty() => format!("{label} has no windows to {}", action.unwrap_or("?")),
        Some("hide") => format!("hid {label}: {windows} off screen, their panes still running"),
        _ => format!("showed {label}: {windows}"),
    }
}

pub fn window(args: &Value, reply: &Value) -> String {
    let window = reply["window"].as_str().unwrap_or("?");
    if args["action"] == "assign" {
        let id = reply["workspace"]["id"].as_str().unwrap_or("?");
        return if reply["shown"] == true { format!("assigned {window} to {id}") } else { format!("assigned {window} to {id} (hidden: it opens again when {id} is shown)") };
    }
    match (reply["was"]["id"].as_str(), reply["was"]["hidden"] == true) {
        (Some(was), true) => format!("{window} is free (out of {was}, and open again)"),
        (Some(was), false) => format!("{window} is free (out of {was})"),
        (None, _) => format!("{window} is free"),
    }
}

pub fn moved(reply: &Value) -> String {
    let dock = &reply["dock"];
    if dock.is_object() {
        return format!(
            "{} is docked {} in {} (width {})",
            dock["pane"].as_str().unwrap_or("?"),
            dock["edge"].as_str().unwrap_or("?"),
            reply["window"].as_str().unwrap_or("?"),
            dock["width"].as_str().unwrap_or("?")
        );
    }
    layout(reply)
}

pub fn closed(args: &Value, reply: &Value) -> String {
    let panes = join(&reply["closed"]);
    match (args["window"].as_str(), args["tab"].as_str()) {
        (Some(window), _) => format!("closed {window} (panes {panes})"),
        (_, Some(tab)) => format!("closed {tab} (panes {panes})"),
        _ => format!("closed {panes}"),
    }
}

/// What a pane shows: its command, its URL, or its app on a device.
fn what(pane: &Value) -> Option<String> {
    if let Some(app) = pane["app"].as_str() {
        return Some(match pane["device"].as_str() {
            Some(device) => format!("{app} on {device}"),
            None => app.into(),
        });
    }
    if let Some(path) = pane["path"].as_str() {
        return Some(match std::env::var("HOME") {
            Ok(home) if !home.is_empty() && path.starts_with(&format!("{home}/")) => format!("~{}", &path[home.len()..]),
            _ => path.into(),
        });
    }
    pane["cmd"].as_str().or(pane["url"].as_str()).map(String::from)
}

pub fn pane_state(reply: &Value) -> String {
    let pane = &reply["pane"];
    let what = what(pane).map(|c| format!(" ({c})")).unwrap_or_default();
    format!("{}{what} is {}", label(pane), state(pane))
}

pub fn layout(reply: &Value) -> String {
    expression(&reply["layout"], true)
}

/// A layout tree as a layout expression: `a:2/3 | b`, `a / b`, with
/// parentheses for nested splits (the kanna layout syntax).
pub fn expression(node: &Value, top: bool) -> String {
    if let Some(pane) = node["pane"].as_str() {
        return sized(pane.to_string(), node);
    }
    let separator = if node["split"] == "column" { " / " } else { " | " };
    let inner = node["children"].as_array().map(|kids| kids.iter().map(|k| expression(k, false)).collect::<Vec<_>>().join(separator)).unwrap_or_default();
    if top { inner } else { sized(format!("({inner})"), node) }
}

fn sized(text: String, node: &Value) -> String {
    match node["size"].as_str() {
        Some(size) => format!("{text}:{size}"),
        None => text,
    }
}

fn window_lines(window: &Value, reply: &Value, out: &mut Vec<String>) {
    let mut line = format!("{}{}", if window["key"] == true { "*" } else { " " }, window["id"].as_str().unwrap_or("?"));
    if let Some(focused) = window["focused"].as_str() {
        line += &format!("  focused: {focused}");
    }
    if let Some(zoomed) = window["zoomed"].as_str() {
        line += &format!("  zoomed: {zoomed}");
    }
    out.push(line);
    let dock = &window["dock"];
    if let Some(name) = dock["pane"].as_str() {
        let pane = reply["panes"].as_array().and_then(|panes| panes.iter().find(|p| p["name"] == name || p["id"] == name));
        out.push(format!(
            "  dock: {}  {} {}{}",
            pane.map(label).unwrap_or_else(|| name.to_string()),
            dock["edge"].as_str().unwrap_or("?"),
            dock["width"].as_str().unwrap_or("?"),
            if dock["frozen"] == true { "  frozen" } else { "" }
        ));
    }
    for tab in window["tabs"].as_array().into_iter().flatten() {
        out.push(format!(
            "  {}{} \"{}\"  {}",
            if tab["active"] == true { "*" } else { " " },
            tab["id"].as_str().unwrap_or("?"),
            tab["title"].as_str().unwrap_or(""),
            expression(&tab["layout"], true)
        ));
    }
}

pub fn list(reply: &Value) -> String {
    let empty = vec![];
    let windows = reply["windows"].as_array().unwrap_or(&empty);
    let workspaces = reply["workspaces"].as_array().unwrap_or(&empty);
    if windows.is_empty() && workspaces.is_empty() {
        return "no windows (open one with `kmux open`)".into();
    }
    let mut out = Vec::new();
    for window in windows.iter().filter(|w| w["workspace"].is_null()) {
        window_lines(window, reply, &mut out);
    }
    for workspace in workspaces {
        let mine: Vec<&Value> = windows.iter().filter(|w| w["workspace"] == workspace["id"]).collect();
        out.push(format!(
            "workspace {}{}{}",
            workspace_label(workspace),
            if workspace["hidden"] == true { "  hidden" } else { "" },
            if mine.is_empty() { "  no windows" } else { "" }
        ));
        for window in mine {
            window_lines(window, reply, &mut out);
        }
    }
    out.push(String::new());
    let mut rows = vec![["PANE".to_string(), "NAME".into(), "TYPE".into(), "STATE".into(), "COMMAND".into()]];
    for pane in reply["panes"].as_array().unwrap_or(&empty) {
        rows.push([
            pane["id"].as_str().unwrap_or("?").into(),
            pane["name"].as_str().unwrap_or("-").into(),
            pane["type"].as_str().unwrap_or("?").into(),
            brief(&state(pane)),
            what(pane).unwrap_or_else(|| "(shell)".into()),
        ]);
    }
    let widths: Vec<usize> = (0..5).map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0)).collect();
    for row in rows {
        out.push(row.iter().zip(&widths).map(|(cell, w)| format!("{cell:w$}")).collect::<Vec<_>>().join("  ").trim_end().to_string());
    }
    out.join("\n")
}
