use super::{caller, usage, Call, Command, Flow, Invocation};
use crate::args::Argv;
use crate::output::{expression, label, place_named, place_of, tilde};
use crate::target::Session;
use kanna_client::{exit, fail, Failure};
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "workspace",
    group: "Tabs and windows",
    summary: "Show your workspace as it will be seen: its dock, tabs and panes, which tab is shown, the focus and the zoom.",
    usage: "kanna workspace list [--all]",
    options: &[("--all", "Every window, workspace and pane, not just your own.")],
    notes: "Your workspace is the one your terminal ($KANNA_TERMINAL) is in, on a Desk or on the Shelf: its ID (ws-3), its task, where it is, its docked pane, the focused pane (and whether it's zoomed), its tabs with `*` on the shown one, and its own windows. From a pane in a window that's no workspace's, it's that window. Layouts are written as `a:2/3 | b` (side by side) and `a / b` (stacked).\nWith --all: every window and what it shows, every workspace, free windows' tabs, and panes in no window. With Kanna.app closed there are no windows, and the terminals, which keep running, are listed alone. --json prints the reply: your own windows, workspace and panes, or with --all all of them.",
    examples: &[
        ("kanna workspace list", "See your workspace as the user will when they pull your task."),
        ("kanna workspace list --all", "See every window, workspace and pane."),
        ("kanna workspace list --all --json", "The same, as JSON."),
    ],
    parse,
    show,
    flow: Flow { wire: Some(|_| "list"), offline: Some(offline), after: Some(after), ..Flow::PLAIN },
};

fn after(request: &Value, reply: &mut Value, _: &mut Session, _: &mut Call) -> Result<(), Failure> {
    if let Some(pane) = request["pane"].as_str() {
        *reply = scoped(reply, pane).ok_or_else(|| fail(exit::NOT_FOUND, format!("no pane \"{pane}\": your terminal isn't one of this Kanna's")))?;
    }
    Ok(())
}

fn offline(request: &Value, _: &Invocation) -> Option<Result<(), Failure>> {
    (request["all"] != true && request["pane"].is_null()).then(|| Err(usage("workspace", "outside a Kanna terminal there's no workspace of yours: add --all")))
}

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let action = args.positional().ok_or_else(|| usage("workspace", "say list"))?;
    if action != "list" {
        return Err(usage("workspace", &format!("unknown action \"{action}\": list")));
    }
    if args.flag("--all") {
        return Ok(json!({ "action": "list", "all": true }));
    }
    Ok(match caller() {
        Some(pane) => json!({ "action": "list", "pane": pane }),
        None => json!({ "action": "list" }),
    })
}

fn workspace_block(ws: &Value, reply: &Value) -> Vec<String> {
    let empty = vec![];
    let empty_desks = vec![];
    let desks = reply["desks"].as_array().unwrap_or(&empty_desks);
    let planning = desks.iter().find(|d| d["planner"] == ws["id"]).and_then(|d| d["id"].as_str());
    let shown = desks.iter().find(|d| d["workspace"] == ws["id"] && d["deskView"] != true).and_then(|d| d["id"].as_str());
    let place = place_named(&ws["where"], planning) + &shown.map(|d| format!(", shown in {d}")).unwrap_or_default();
    let mut out = vec![format!("{}, {place}", workspace_name(ws))];
    let dock = &ws["dock"];
    if let Some(pane) = dock["id"].as_str().or(dock["pane"].as_str()) {
        out.push(format!(
            "  dock: {}  {} {}{}",
            named_pane(reply, pane),
            dock["edge"].as_str().unwrap_or("?"),
            dock["width"].as_str().unwrap_or("?"),
            if dock["frozen"] == true { "  frozen" } else { "" }
        ));
    }
    out.extend(focus_line(ws, "  "));
    out.extend(tab_lines(&ws["tabs"], "  "));
    let background: Vec<String> = ws["background"].as_array().unwrap_or(&empty).iter().filter_map(Value::as_str).map(|p| named_pane(reply, p)).collect();
    if !background.is_empty() {
        out.push(format!("  background: {}", background.join(", ")));
    }
    for own in ws["windows"].as_array().unwrap_or(&empty).iter().filter_map(Value::as_str) {
        let window = reply["windows"].as_array().and_then(|l| l.iter().find(|w| w["id"] == own)).cloned();
        out.push(format!("  {own}"));
        if let Some(window) = window {
            out.extend(focus_line(&window, "    "));
            out.extend(tab_lines(&window["tabs"], "    "));
        }
    }
    out
}

fn tab_lines(tabs: &Value, indent: &str) -> Vec<String> {
    tabs.as_array().map_or(vec![], |tabs| {
        tabs.iter()
            .map(|tab| format!("{indent}{}{} \"{}\"  {}", if tab["active"] == true { "*" } else { " " }, tab["id"].as_str().unwrap_or("?"), tab["title"].as_str().unwrap_or(""), expression(&tab["layout"], true)))
            .collect()
    })
}

fn focus_line(holder: &Value, indent: &str) -> Option<String> {
    let focused = holder["focused"].as_str()?;
    let zoomed = if holder["zoomed"].as_str() == Some(focused) { ", zoomed" } else { "" };
    Some(format!("{indent}focus: {focused}{zoomed}"))
}

fn window_line(window: &Value, reply: &Value) -> String {
    let empty = vec![];
    let workspaces = reply["workspaces"].as_array().unwrap_or(&empty);
    let ws = |id: &Value| workspaces.iter().find(|w| w["id"] == *id).map_or_else(|| id.as_str().unwrap_or("?").to_string(), workspace_name);
    let mut line = format!("{}{}", if window["key"] == true { "*" } else { " " }, window["id"].as_str().unwrap_or("?"));
    let lead = window["lead"]["pane"].as_str().map(|p| format!("led by {}", named_pane(reply, p)));
    let what = if window["workspace"].is_string() { format!("{}'s window", ws(&window["workspace"])) } else { "free".to_string() };
    let parts: Vec<String> = std::iter::once(what).chain(lead).collect();
    line += &format!("  {}", parts.join(", "));
    line
}

fn workspace_name(ws: &Value) -> String {
    let id = ws["id"].as_str().unwrap_or("?");
    match (ws["task"].as_str(), ws["name"].as_str()) {
        (Some(task), Some(name)) => format!("{id} ({task}, {name})"),
        (Some(task), None) => format!("{id} ({task})"),
        _ => id.to_string(),
    }
}

fn named_pane(reply: &Value, shown: &str) -> String {
    reply["panes"].as_array().and_then(|p| p.iter().find(|p| p["id"] == shown || p["name"] == shown)).map_or_else(|| shown.to_string(), label)
}

fn everything(reply: &Value) -> String {
    let empty = vec![];
    let windows = reply["windows"].as_array().unwrap_or(&empty);
    let desks = reply["desks"].as_array().unwrap_or(&empty);
    let panes = reply["panes"].as_array().unwrap_or(&empty);
    if windows.is_empty() && desks.is_empty() && panes.is_empty() {
        return "no windows (open one with `kanna open`)".into();
    }
    let mut sections: Vec<String> = Vec::new();
    if !windows.is_empty() || !desks.is_empty() {
        let lines = desks.iter().map(|d| desk_line(d, reply)).chain(windows.iter().map(|w| window_line(w, reply)));
        sections.push(std::iter::once("Windows".to_string()).chain(lines).collect::<Vec<_>>().join("\n"));
    }
    let mut workspaces: Vec<&Value> = reply["workspaces"].as_array().unwrap_or(&empty).iter().collect();
    workspaces.sort_by_key(|w| match place_of(&w["where"]) {
        "on the Desk" => 0,
        "on the Shelf" => 1,
        _ => 2,
    });
    if !workspaces.is_empty() {
        let blocks: Vec<String> = workspaces.iter().map(|ws| workspace_block(ws, reply).join("\n")).collect();
        sections.push(format!("Workspaces\n{}", blocks.join("\n")));
    }
    let free: Vec<String> = windows
        .iter()
        .filter(|w| w["workspace"].is_null() && w["tabs"].as_array().is_some_and(|t| !t.is_empty()))
        .map(|w| std::iter::once(w["id"].as_str().unwrap_or("?").to_string()).chain(focus_line(w, "  ")).chain(tab_lines(&w["tabs"], "  ")).collect::<Vec<_>>().join("\n"))
        .collect();
    if !free.is_empty() {
        sections.push(format!("Free windows\n{}", free.join("\n")));
    }
    let mut placed: Vec<String> = workspaces.iter().flat_map(|w| [&w["panes"], &w["background"]]).flat_map(|l| l.as_array().cloned().unwrap_or_default()).filter_map(|p| p.as_str().map(String::from)).collect();
    for window in windows.iter().chain(reply["hiddenWindows"].as_array().unwrap_or(&empty)) {
        placed.extend(window["tabs"].as_array().unwrap_or(&empty).iter().flat_map(|t| layout_panes(&t["layout"])));
        placed.extend(window["lead"]["pane"].as_str().map(String::from));
    }
    let unplaced: Vec<String> = panes.iter().filter(|p| !placed.iter().any(|q| p["id"] == q.as_str() || p["name"] == q.as_str())).map(label).collect();
    if !unplaced.is_empty() {
        sections.push(format!("Unplaced panes\n{}", unplaced.join("\n")));
    }
    sections.join("\n\n")
}

fn own(reply: &Value) -> String {
    let empty = vec![];
    let pane = reply["panes"].as_array().and_then(|p| p.iter().find(|p| p["id"] == reply["pane"])).map_or_else(|| reply["pane"].as_str().unwrap_or("?").to_string(), label);
    let mut out = Vec::new();
    match reply["at"].as_str() {
        Some("workspace") => {
            for ws in reply["workspaces"].as_array().unwrap_or(&empty) {
                out.extend(workspace_block(ws, reply));
            }
        }
        Some("window") => {
            for window in ["windows", "hiddenWindows"].iter().flat_map(|k| reply[*k].as_array().cloned().unwrap_or_default()) {
                out.push(window_line(&window, reply));
                out.extend(focus_line(&window, "  "));
                out.extend(tab_lines(&window["tabs"], "  "));
            }
            for ws in reply["workspaces"].as_array().unwrap_or(&empty) {
                out.push(String::new());
                out.extend(workspace_block(ws, reply));
            }
        }
        _ => out.push(format!("{pane} isn't in a window or a workspace")),
    }
    out.join("\n")
}

pub fn show(request: &Value, reply: &Value) -> String {
    match request["action"].as_str() {
        Some("list") if request["all"] == true => everything(reply),
        Some("list") => own(reply),
        _ => String::new(),
    }
}

fn layout_panes(node: &Value) -> Vec<String> {
    match node["pane"].as_str() {
        Some(pane) => vec![pane.to_string()],
        None => node["children"].as_array().map_or(vec![], |kids| kids.iter().flat_map(layout_panes).collect()),
    }
}

pub fn scoped(reply: &Value, pane: &str) -> Option<Value> {
    let empty = vec![];
    let all = |key: &str| reply[key].as_array().unwrap_or(&empty).clone();
    let (windows, hidden, workspaces, panes) = (all("windows"), all("hiddenWindows"), all("workspaces"), all("panes"));
    let row = panes.iter().find(|p| p["id"] == pane || p["name"] == pane)?;
    let id = row["id"].as_str().unwrap_or(pane).to_string();
    let shown = row["name"].as_str().unwrap_or(&id).to_string();
    let named = |v: &Value| v == shown.as_str() || v == id.as_str();
    let has = |list: &Value, p: &str| list.as_array().is_some_and(|l| l.iter().any(|x| x == p));
    let mut out = json!({ "pane": id, "at": null, "windows": [], "hiddenWindows": [], "workspaces": [], "panes": [row], "desks": reply["desks"].clone() });
    if let Some(ws) = workspaces.iter().find(|w| has(&w["panes"], &id) || has(&w["background"], &id)) {
        let ours = |w: &&Value| w["workspace"] == ws["id"];
        out["at"] = json!("workspace");
        out["workspaces"] = json!([ws]);
        out["windows"] = Value::Array(windows.iter().filter(ours).cloned().collect());
        out["hiddenWindows"] = Value::Array(hidden.iter().filter(ours).cloned().collect());
        out["panes"] = Value::Array(panes.iter().filter(|p| p["id"].as_str().is_some_and(|p| has(&ws["panes"], p) || has(&ws["background"], p))).cloned().collect());
        return Some(out);
    }
    let holds = |w: &Value| named(&w["lead"]["pane"]) || named(&w["dock"]["pane"]) || w["tabs"].as_array().is_some_and(|t| t.iter().any(|t| layout_panes(&t["layout"]).iter().any(|p| named(&json!(p)))));
    let found = windows.iter().find(|w| holds(w)).map(|w| (w, "windows")).or_else(|| hidden.iter().find(|w| holds(w)).map(|w| (w, "hiddenWindows")));
    if let Some((window, key)) = found {
        out["at"] = json!("window");
        out[key] = json!([window]);
        let shows: Vec<Value> = workspaces.iter().filter(|w| w["id"] == window["shows"]).cloned().collect();
        let mut inside: Vec<String> = window["tabs"].as_array().unwrap_or(&empty).iter().flat_map(|t| layout_panes(&t["layout"])).collect();
        inside.extend(window["lead"]["pane"].as_str().map(String::from));
        inside.extend(shows.iter().flat_map(|w| w["panes"].as_array().cloned().unwrap_or_default()).filter_map(|p| p.as_str().map(String::from)));
        out["workspaces"] = Value::Array(shows);
        out["panes"] = Value::Array(panes.iter().filter(|p| inside.iter().any(|i| p["id"] == i.as_str() || p["name"] == i.as_str())).cloned().collect());
    }
    Some(out)
}

fn desk_line(desk: &Value, reply: &Value) -> String {
    let empty = vec![];
    let workspaces = reply["workspaces"].as_array().unwrap_or(&empty);
    let id = desk["id"].as_str().unwrap_or("?");
    let project = desk["project"].as_str().map(|p| format!(" on {}", tilde(p))).unwrap_or_default();
    let shows = match workspaces.iter().find(|w| w["id"] == desk["workspace"]) {
        Some(ws) if desk["deskView"] == true => format!(", the Desk view: {}", workspace_name(ws)),
        Some(ws) => format!(", shows {}", workspace_name(ws)),
        None => String::new(),
    };
    format!(" {id}  Desk{project}{shows}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_list_lists_unplaced_panes_and_says_when_there_is_nothing() {
        let all = json!({ "action": "list", "all": true });
        let list = show(&all, &json!({ "windows": [], "panes": [{ "id": "p1", "type": "term", "state": "running" }] }));
        assert_eq!(list, "Unplaced panes\np1");
        assert_eq!(show(&all, &json!({ "windows": [], "panes": [] })), "no windows (open one with `kanna open`)");
        let alone = scoped(&json!({ "windows": [], "panes": [{ "id": "p1", "name": "logs", "type": "term" }] }), "p1").unwrap();
        assert_eq!(show(&json!({ "action": "list", "pane": "p1" }), &alone), "p1 (logs) isn't in a window or a workspace");
    }
}

#[cfg(test)]
mod workspace_tests {
    use super::*;
    use crate::commands::{focus, open, window, zoom};
    use serde_json::json;

    fn reply() -> Value {
        json!({
            "windows": [
                { "id": "win-19", "focused": "p157", "zoomed": null, "lead": { "pane": "p157", "width": "46%" }, "workspace": null, "tabs": [] },
                { "id": "win-40", "focused": "p9", "zoomed": null, "workspace": "ws-3", "tabs": [{ "id": "tab-30", "title": "Tab 1", "active": true, "layout": { "pane": "p9" } }] },
                { "id": "win-45", "focused": "notes", "zoomed": null, "workspace": null, "tabs": [{ "id": "tab-247", "title": "notes", "active": true, "layout": { "pane": "notes" } }] }
            ],
            "hiddenWindows": [],
            "workspaces": [
                { "id": "ws-3", "task": "task-22a4", "name": "Skills show diffs", "where": { "desk": "mac" }, "dock": { "pane": "Skills show diffs", "id": "p401", "edge": "left", "width": "2/5", "frozen": true },
                  "focused": "p406", "zoomed": null, "tabs": [{ "id": "tab-246", "title": "diff --merge-base main", "active": true, "layout": { "pane": "p406" }, "panes": ["p406"] }], "windows": ["win-40"], "panes": ["p406", "p9", "p401"] },
                { "id": "ws-1", "task": "task-1dfb", "name": "Fix login", "where": { "desk": "mac" }, "dock": { "pane": "Fix login", "id": "p407", "edge": "left", "width": "2/5", "frozen": true },
                  "focused": "diff", "zoomed": "diff",
                  "tabs": [{ "id": "tab-211", "title": "diff", "active": true, "layout": { "pane": "diff" }, "panes": ["diff"] }, { "id": "tab-212", "title": "login.md", "active": false, "layout": { "split": "row", "children": [{ "pane": "p5", "size": "1/2" }, { "pane": "p6", "size": "1/2" }] }, "panes": ["p5", "p6"] }],
                  "windows": [], "panes": ["p400", "p5", "p6", "p407"] },
                { "id": "ws-4", "where": "shelf", "dock": { "pane": "p185", "id": "p185", "edge": "left", "width": "2/5" }, "focused": "p185", "zoomed": null, "tabs": [], "windows": [], "panes": ["p185"] },
                { "id": "ws-5", "where": { "planner": "mac" }, "dock": { "pane": "p280", "id": "p280", "edge": "left", "width": "2/5" }, "focused": "p280", "zoomed": null, "tabs": [], "windows": [], "panes": ["p280"] }
            ],
            "panes": [
                { "id": "p157", "name": null, "type": "term" }, { "id": "p185", "name": null, "type": "term" }, { "id": "p280", "name": null, "type": "term" },
                { "id": "p400", "name": "diff", "type": "term" }, { "id": "p401", "name": "Skills show diffs", "type": "term" }, { "id": "p406", "name": null, "type": "term" },
                { "id": "p407", "name": "Fix login", "type": "term" }, { "id": "p5", "name": null, "type": "md" }, { "id": "p6", "name": null, "type": "term" },
                { "id": "p9", "name": null, "type": "term" }, { "id": "p411", "name": "notes", "type": "md" }, { "id": "p500", "name": "logs", "type": "term" }
            ]
        })
    }

    fn mine(pane: &str) -> String {
        show(&json!({ "action": "list", "pane": pane }), &scoped(&reply(), pane).unwrap())
    }

    #[test]
    fn your_workspace_is_shown_as_it_will_be_seen() {
        assert_eq!(
            mine("p6"),
            "ws-1 (task-1dfb, Fix login), on the Desk\n  dock: p407 (Fix login)  left 2/5  frozen\n  focus: diff, zoomed\n  *tab-211 \"diff\"  diff\n   tab-212 \"login.md\"  p5:1/2 | p6:1/2"
        );
        assert_eq!(mine("Fix login"), mine("p6"), "its agent sees the same");
        assert_eq!(
            mine("p9"),
            "ws-3 (task-22a4, Skills show diffs), on the Desk\n  dock: p401 (Skills show diffs)  left 2/5  frozen\n  focus: p406\n  *tab-246 \"diff --merge-base main\"  p406\n  win-40\n    focus: p9\n    *tab-30 \"Tab 1\"  p9"
        );
        assert_eq!(mine("p185"), "ws-4, on the Shelf\n  dock: p185  left 2/5\n  focus: p185", "a workspace with no task");
        assert_eq!(mine("p280"), "ws-5, a Desk's planner\n  dock: p280  left 2/5\n  focus: p280");
    }

    #[test]
    fn from_a_window_that_is_no_workspaces_it_is_that_window() {
        assert_eq!(mine("notes"), " win-45  free\n  focus: notes\n  *tab-247 \"notes\"  notes");
        assert_eq!(mine("p157"), " win-19  free, led by p157\n  focus: p157");
        assert_eq!(mine("p500"), "p500 (logs) isn't in a window or a workspace");
        assert!(scoped(&reply(), "p999").is_none());
    }

    #[test]
    fn your_workspace_as_json_is_the_reply_cut_to_it() {
        let ws = scoped(&reply(), "p9").unwrap();
        assert_eq!((ws["pane"].clone(), ws["at"].clone()), (json!("p9"), json!("workspace")));
        assert_eq!(ws["workspaces"].as_array().map(|w| w.iter().map(|w| w["id"].clone()).collect::<Vec<_>>()), Some(vec![json!("ws-3")]));
        assert_eq!(ws["windows"].as_array().map(|w| w.iter().map(|w| w["id"].clone()).collect::<Vec<_>>()), Some(vec![json!("win-40")]));
        assert_eq!(ws["panes"].as_array().map(|p| p.iter().map(|p| p["id"].clone()).collect::<Vec<_>>()), Some(vec![json!("p401"), json!("p406"), json!("p9")]));
    }

    #[test]
    fn all_is_every_window_then_every_workspace_then_free_windows_and_panes_in_none() {
        let text = show(&json!({ "action": "list", "all": true }), &reply());
        assert!(text.starts_with("Windows\n win-19  free, led by p157\n win-40  ws-3 (task-22a4, Skills show diffs)'s window\n win-45  free\n\nWorkspaces\nws-3 (task-22a4, Skills show diffs), on the Desk\n"), "{text}");
        assert!(text.contains("\nws-1 (task-1dfb, Fix login), on the Desk\n"), "{text}");
        assert!(text.contains("\nws-4, on the Shelf\n  dock: p185  left 2/5\n  focus: p185\nws-5, a Desk's planner\n  dock: p280  left 2/5\n  focus: p280\n\nFree windows\nwin-45\n  focus: notes\n  *tab-247 \"notes\"  notes\n\nUnplaced panes\np500 (logs)"), "{text}");
    }

    #[test]
    fn a_desk_is_listed_as_a_window_and_names_its_planner_and_the_workspace_it_shows() {
        let mut listed = reply();
        listed["desks"] = json!([{ "id": "d1", "desk": true, "workspace": "ws-1", "planner": "ws-5", "deskView": false, "project": "/work/app" }, { "id": "d2", "desk": true, "workspace": "ws-5", "planner": "ws-5", "deskView": true, "project": null }]);
        let text = show(&json!({ "action": "list", "all": true }), &listed);
        assert!(text.starts_with("Windows\n d1  Desk on /work/app, shows ws-1 (task-1dfb, Fix login)\n d2  Desk, the Desk view: ws-5\n win-19  free"), "{text}");
        assert!(text.contains("\nws-1 (task-1dfb, Fix login), on the Desk, shown in d1\n"), "{text}");
        assert!(text.contains("\nws-5, d1's planner\n"), "{text}");
        let planner = json!({ "pane": { "id": "file-1", "state": "running" }, "window": null, "tab": "tab-9", "workspace": { "id": "ws-5", "where": { "planner": "mac" }, "desk": "d2" } });
        assert_eq!(open::show(&json!({}), &planner), "opened file-1 in ws-5 (d2's planner), tab-9: running");
        assert_eq!(focus::show(&json!({ "window": "d2" }), &json!({ "window": null, "front": "d2", "workspace": { "id": "ws-5", "where": { "planner": "mac" }, "desk": "d2" } })), "brought d2 forward");
    }

    #[test]
    fn replies_name_the_workspace_then_its_task_and_where_it_is() {
        let windowless = json!({ "pane": { "id": "p5", "state": "running" }, "window": null, "tab": "tab-9", "workspace": { "id": "ws-3", "task": "task-1dfb", "where": { "desk": "mac" } } });
        assert_eq!(open::show(&json!({}), &windowless), "opened p5 in ws-3 (task-1dfb, on the Desk), tab-9: running");
        let windowed = json!({ "pane": { "id": "p5", "state": "running" }, "window": "win-31", "tab": "tab-9", "workspace": { "id": "ws-3", "task": "task-1dfb", "where": { "desk": "mac" } } });
        assert_eq!(open::show(&json!({}), &windowed), "opened p5 in ws-3's win-31 (task-1dfb, on the Desk), tab-9: running");
        assert_eq!(focus::show(&json!({ "pane": "spec" }), &windowed), "focused spec in ws-3's win-31 (task-1dfb, on the Desk)");
        assert_eq!(focus::show(&json!({ "pane": "spec" }), &json!({ "window": "win-31", "front": "win-31", "workspace": null })), "focused spec in win-31 (in front)");
        assert_eq!(focus::show(&json!({ "pane": "spec" }), &json!({ "window": "win-31", "front": "win-31", "behind": true, "workspace": null })), "focused spec in win-31 (behind the app you're in)");
        assert_eq!(focus::show(&json!({ "window": "win-31" }), &json!({ "window": "win-31", "front": "win-31", "behind": true, "workspace": null })), "put win-31 behind the app you're in");
        assert_eq!(zoom::show(&json!({ "pane": "spec" }), &json!({ "zoomed": true, "workspace": windowless["workspace"] })), "spec is zoomed in ws-3 (task-1dfb, on the Desk; run again to unzoom)");
        let free = json!({ "pane": { "id": "p5", "state": "running" }, "window": "win-4", "tab": "tab-9", "workspace": null });
        assert_eq!(open::show(&json!({}), &free), "opened p5 in win-4, tab-9: running");
        assert_eq!(window::show(&json!({ "action": "assign", "window": "win-41" }), &json!({ "window": "win-41", "workspace": windowless["workspace"] })), "assigned win-41 to ws-3 (task-1dfb, on the Desk)");
    }
}
