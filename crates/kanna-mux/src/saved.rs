use crate::error::KmuxError;
use crate::json::{num, opt, string, trim, Map, Value};
use crate::layout::{Edge, SavedLayout};
use crate::model::{normalize, pane_ids, reply, Axis, Child, Dock, Model, Node, Split, Tab, Tabs, Window, DOCK_WIDTH};
use crate::state::{bad, number_in, quoted, whole};
use crate::workspace::{Where, Workspace, MAC_APP};
use std::collections::HashSet;

pub const LAYOUT_VERSION: u32 = 3;

struct Restored {
    windows: Vec<Window>,
    workspaces: Vec<Workspace>,
    orphans: Vec<Tabs>,
    counters: (usize, usize, usize),
}

fn tab_json(tab: &Tab) -> Value {
    let mut out = reply([
        ("id", tab.id.clone().into()),
        ("title", tab.title.clone().into()),
        ("lastFocus", opt(tab.last_focus.as_deref())),
        ("layout", tab.root.as_ref().map_or(Value::Null, Model::export_node)),
        ("number", num(tab.number as f64)),
    ]);
    if let Some(namer) = &tab.namer {
        out.insert("namer".into(), namer.clone().into());
    }
    if tab.renamed {
        out.insert("renamed".into(), Value::Bool(true));
    }
    Value::Object(out)
}

fn put_tabs(out: &mut Map, tabs: &Tabs) {
    out.insert("active".into(), opt(tabs.active.as_deref()));
    out.insert("focused".into(), opt(tabs.focused.as_deref()));
    out.insert("zoomed".into(), opt(tabs.zoomed().map(String::as_str)));
    out.insert("tabCount".into(), num(tabs.count as f64));
    out.insert("tabs".into(), Value::Array(tabs.tabs.iter().map(tab_json).collect()));
}

fn dock_json(dock: &Dock) -> Value {
    let mut saved = reply([("pane", dock.pane.clone().into()), ("edge", dock.edge.as_str().into()), ("width", num(dock.width))]);
    if dock.frozen {
        saved.insert("frozen".into(), Value::Bool(true));
    }
    Value::Object(saved)
}

fn window_json(window: &Window, tabs: &Tabs) -> Map {
    let mut out = reply([("id", window.id.clone().into())]);
    put_tabs(&mut out, tabs);
    if let Some(lead) = &window.lead {
        out.insert("lead".into(), Value::Object(reply([("pane", lead.pane.clone().into()), ("width", num(lead.width))])));
    }
    out
}

fn put_workspace(out: &mut Map, ws: &Workspace) {
    out.insert("dock".into(), dock_json(&ws.dock));
    if let Some(at) = ws.parked_at {
        out.insert("parkedAt".into(), num(at));
    }
}

fn counters(model: &Model) -> Value {
    Value::Object(reply([("tab", num(model.counters.0 as f64)), ("window", num(model.counters.1 as f64)), ("workspace", num(model.counters.2 as f64))]))
}

impl Model {
    pub fn export_v1(&self) -> Map {
        let windows = self
            .windows
            .iter()
            .map(|window| {
                let mut out = window_json(window, &window.tabs);
                if let Some(ws) = &window.workspace {
                    out.insert("workspace".into(), ws.clone().into());
                }
                Value::Object(out)
            })
            .collect();
        let parked: Vec<Value> = self
            .workspaces
            .iter()
            .map(|ws| {
                let mut out = reply([("id", ws.dock.pane.clone().into()), ("workspace", ws.id.clone().into()), ("where", ws.place.to_json())]);
                put_tabs(&mut out, &ws.tabs);
                put_workspace(&mut out, ws);
                Value::Object(out)
            })
            .collect();
        let mut out = reply([("counters", counters(self)), ("windows", Value::Array(windows))]);
        if !parked.is_empty() {
            out.insert("parked".into(), Value::Array(parked));
        }
        out
    }

    pub fn export_v3(&self) -> Map {
        let windows = self
            .windows
            .iter()
            .filter(|window| window.workspace.is_none())
            .map(|window| Value::Object(window_json(window, &window.tabs)))
            .collect();
        let workspaces = self
            .workspaces
            .iter()
            .map(|ws| {
                let mut out = reply([("id", ws.id.clone().into()), ("where", ws.place.to_json())]);
                put_workspace(&mut out, ws);
                put_tabs(&mut out, &ws.tabs);
                out.insert("windows".into(), Value::Array(self.workspace_windows(&ws.id).into_iter().map(|w| Value::Object(window_json(w, &w.tabs))).collect()));
                Value::Object(out)
            })
            .collect();
        reply([("counters", counters(self)), ("windows", Value::Array(windows)), ("workspaces", Value::Array(workspaces))])
    }
}

struct Reader<'a> {
    panes: &'a HashSet<String>,
    placed: HashSet<String>,
    tab_ids: HashSet<String>,
    window_ids: HashSet<String>,
    counters: (usize, usize, usize),
}

fn build(node: &Value, panes: &HashSet<String>, placed: &mut HashSet<String>) -> Result<Node, KmuxError> {
    if !node.is_object() {
        return Err(bad("malformed layout"));
    }
    if let Some(reference) = node.get("pane").filter(|r| !r.is_null()) {
        let id = reference.as_str().filter(|id| panes.contains(*id)).ok_or_else(|| bad(&format!("layout names unknown pane {}", quoted(Some(reference)))))?;
        if !placed.insert(id.to_string()) {
            return Err(bad(&format!("pane {id} is placed twice")));
        }
        return Ok(Node::Pane(id.into()));
    }
    let axis = string(node, "split").and_then(Axis::parse);
    let children = node.get("children").and_then(Value::as_array).filter(|c| !c.is_empty());
    let (Some(axis), Some(children)) = (axis, children) else { return Err(bad("malformed layout")) };
    let sizes: Vec<f64> = children.iter().map(|child| child.get("size").and_then(Value::as_f64).filter(|s| *s > 0.0 && *s <= 1.0).unwrap_or(1.0 / children.len() as f64)).collect();
    let total: f64 = sizes.iter().sum();
    let kids = children.iter().zip(sizes).map(|(child, size)| Ok(Child { node: build(child, panes, placed)?, size: size / total })).collect::<Result<Vec<_>, KmuxError>>()?;
    Ok(Node::Split(Split { axis, kids }))
}

impl Reader<'_> {
    fn tabs(&mut self, item: &Value) -> Result<Tabs, KmuxError> {
        let saved = item.get("tabs").and_then(Value::as_array).ok_or_else(|| bad(&format!("window {}", quoted(item.get("id")))))?;
        let mut tabs = Tabs::default();
        for entry in saved {
            let tab_id = string(entry, "id").filter(|_| entry.is_object());
            let tn = tab_id.and_then(|id| number_in(id, "tab-"));
            let (Some(tab_id), Some(tn)) = (tab_id, tn) else { return Err(bad(&format!("tab {}", quoted(entry.get("id"))))) };
            if !self.tab_ids.insert(tab_id.to_string()) {
                return Err(bad(&format!("tab {}", quoted(entry.get("id")))));
            }
            let root = match entry.get("layout") {
                None | Some(Value::Null) => None,
                Some(layout) => Some(normalize(build(layout, self.panes, &mut self.placed)?)),
            };
            let title = trim(string(entry, "title").unwrap_or(""));
            let number = whole(entry.get("number")).or_else(|| title.strip_prefix("Tab ").and_then(|n| n.parse().ok())).filter(|n| *n > 0).unwrap_or(tabs.tabs.len() + 1);
            let mut tab = Tab::new(tab_id.into(), number);
            if !title.is_empty() {
                tab.title = title.into();
            }
            tab.renamed = !title.is_empty() && entry.get("renamed") == Some(&Value::Bool(true));
            let ids = pane_ids(root.as_ref());
            tab.namer = string(entry, "namer").filter(|n| ids.iter().any(|id| id == n)).map(String::from);
            tab.root = root;
            tab.last_focus = string(entry, "lastFocus").filter(|l| ids.iter().any(|id| id == l)).map(String::from);
            tabs.tabs.push(tab);
            self.counters.0 = self.counters.0.max(tn);
        }
        tabs.tabs.retain(|t| t.root.is_some());
        let active = string(item, "active");
        tabs.active = if tabs.tabs.iter().any(|t| Some(t.id.as_str()) == active) { active.map(String::from) } else { tabs.tabs.first().map(|t| t.id.clone()) };
        tabs.count = whole(item.get("tabCount")).unwrap_or(0).max(tabs.tabs.len());
        Ok(tabs)
    }

    fn side(&mut self, item: &Value, key: &str, owner: &str, what: &str) -> Result<Option<Dock>, KmuxError> {
        let Some(saved) = item.get(key).filter(|d| !d.is_null()) else { return Ok(None) };
        let pane = string(saved, "pane").filter(|p| self.panes.contains(*p)).ok_or_else(|| bad(&format!("window {owner} {what} unknown pane {}", quoted(saved.get("pane")))))?;
        if !self.placed.insert(pane.to_string()) {
            return Err(bad(&format!("pane {pane} is placed twice")));
        }
        let width = saved.get("width").and_then(Value::as_f64).filter(|w| *w > 0.0 && *w < 1.0).unwrap_or(DOCK_WIDTH);
        let edge = string(saved, "edge").and_then(Edge::parse).unwrap_or(Edge::Left);
        Ok(Some(Dock { pane: pane.into(), edge, width, frozen: saved.get("frozen") == Some(&Value::Bool(true)) }))
    }

    fn window(&mut self, item: &Value) -> Result<(Window, Option<Dock>, bool), KmuxError> {
        let id = string(item, "id").filter(|_| item.is_object());
        let n = id.and_then(|id| number_in(id, "win-"));
        let (Some(id), Some(n)) = (id, n) else { return Err(bad(&format!("window {}", quoted(item.get("id"))))) };
        if !self.window_ids.insert(id.to_string()) {
            return Err(bad(&format!("window {}", quoted(item.get("id")))));
        }
        self.counters.1 = self.counters.1.max(n);
        let mut window = Window::new(id.into());
        window.tabs = self.tabs(item)?;
        let dock = self.side(item, "dock", id, "docks")?;
        let lead_key = if item.get("lead").is_some() { "lead" } else { "planner" };
        window.lead = self.side(item, lead_key, id, "is led by")?.map(|lead| Dock { edge: Edge::Left, frozen: false, ..lead });
        let desk = dock.is_some() || item.get("desk").and_then(Value::as_bool).unwrap_or(false);
        Ok((window, dock, desk))
    }
}

fn settle(tabs: &mut Tabs, item: &Value, side: Option<&String>) {
    let shown: Vec<String> = side.cloned().into_iter().chain(tabs.shown()).collect();
    let first = tabs.shown().first().cloned();
    let focused = string(item, "focused").filter(|f| shown.iter().any(|s| s == f)).map(String::from);
    tabs.focused = focused.or_else(|| tabs.active_tab().and_then(|t| t.last_focus.clone())).or(first).or_else(|| side.cloned());
    if let Some(zoomed) = string(item, "zoomed").filter(|z| shown.iter().any(|s| s == z)) {
        tabs.zoom = true;
        tabs.focused = Some(zoomed.into());
    }
}

fn renamed(value: &Value, key: Option<&str>) -> Value {
    match value {
        Value::String(id) if matches!(key, Some("id" | "active" | "windows")) => {
            let legacy = |prefix: &str, to: &str| number_in(id, prefix).map(|n| Value::String(format!("{to}{n}")));
            legacy("t", "tab-").or_else(|| legacy("w", "win-")).unwrap_or_else(|| value.clone())
        }
        Value::Array(items) => Value::Array(items.iter().map(|item| renamed(item, key)).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(k, item)| (k.clone(), renamed(item, Some(k)))).collect()),
        _ => value.clone(),
    }
}

fn read_workspace(id: &str, item: &Value, dock: Dock, tabs: Tabs, place: Where) -> Workspace {
    let mut ws = Workspace::new(id.into(), dock, place);
    ws.parked_at = item.get("parkedAt").and_then(Value::as_f64);
    ws.tabs = tabs;
    let side = Some(ws.dock.pane.clone());
    settle(&mut ws.tabs, item, side.as_ref());
    ws
}

pub fn read(state: &Value, panes: &HashSet<String>) -> Result<SavedLayout, KmuxError> {
    let state = &renamed(state, None);
    let saved = state.get("counters").cloned().unwrap_or(Value::Object(Map::new()));
    let mut reader = Reader { panes, placed: HashSet::new(), tab_ids: HashSet::new(), window_ids: HashSet::new(), counters: (whole(saved.get("tab")).unwrap_or(0), whole(saved.get("window")).unwrap_or(0), whole(saved.get("workspace")).unwrap_or(0)) };
    let saved_windows = state.get("windows").and_then(Value::as_array).ok_or_else(|| bad("needs panes and windows"))?;
    let (mut windows, mut workspaces, mut orphans) = (vec![], vec![], vec![]);
    let mut ws_ids = HashSet::new();
    let mut claim = |id: &str| if ws_ids.insert(id.to_string()) { Ok(()) } else { Err(bad(&format!("workspace {id} is saved twice"))) };
    let mut planners = vec![];
    for item in saved_windows {
        let (mut window, dock, desk) = reader.window(item)?;
        if desk {
            if let Some(lead) = window.lead.take() {
                planners.push((format!("{}-planner", window.id), lead, item.get("shelfZoom") == Some(&Value::Bool(true))));
            }
        }
        let lead = window.lead.as_ref().map(|l| l.pane.clone());
        match dock {
            Some(dock) => {
                let id = string(item, "shows").unwrap_or(&dock.pane).to_string();
                claim(&id)?;
                let tabs = std::mem::take(&mut window.tabs);
                let mut ws = read_workspace(&id, item, dock, tabs, Where::Shelf(MAC_APP.into()));
                ws.parked_at.get_or_insert(0.0);
                workspaces.push(ws);
            }
            None => {
                settle(&mut window.tabs, item, lead.as_ref());
                window.workspace = string(item, "workspace").map(String::from);
            }
        }
        windows.push(window);
    }
    for item in state.get("parked").or_else(|| state.get("shelf")).and_then(Value::as_array).into_iter().flatten() {
        let id = string(item, "id").filter(|_| item.is_object()).ok_or_else(|| bad(&format!("window {}", quoted(item.get("id")))))?;
        let tabs = reader.tabs(item)?;
        let place = match Where::read(item.get("where")) {
            Some(place) => place,
            None if item.get("backlog").and_then(Value::as_bool) == Some(true) => Where::Backlog,
            None => Where::Shelf(MAC_APP.into()),
        };
        let ws_id = string(item, "workspace").unwrap_or(id);
        match reader.side(item, "dock", id, "docks")? {
            Some(dock) => {
                claim(ws_id)?;
                let mut ws = read_workspace(ws_id, item, dock, tabs, place);
                ws.parked_at = Some(ws.parked_at.unwrap_or(0.0));
                workspaces.push(ws);
            }
            None => orphans.push(tabs),
        }
    }
    for item in state.get("workspaces").and_then(Value::as_array).into_iter().flatten() {
        let id = string(item, "id").filter(|id| item.is_object() && !id.is_empty()).ok_or_else(|| bad(&format!("workspace {}", quoted(item.get("id")))))?;
        let tabs = reader.tabs(item)?;
        let place = Where::read(item.get("where")).unwrap_or_else(|| Where::Shelf(MAC_APP.into()));
        let dock = reader.side(item, "dock", id, "docks")?;
        let gone = dock.is_none();
        match dock {
            Some(dock) => {
                claim(id)?;
                workspaces.push(read_workspace(id, item, dock, tabs, place));
            }
            None => orphans.push(tabs),
        }
        for saved in item.get("windows").and_then(Value::as_array).into_iter().flatten() {
            let (mut window, _, _) = reader.window(saved)?;
            settle(&mut window.tabs, saved, None);
            window.lead = None;
            window.workspace = (!gone).then(|| id.to_string());
            windows.push(window);
        }
    }
    for (id, lead, zoom) in planners {
        let mut ws = Workspace::new(id, lead, Where::Planner(MAC_APP.into()));
        ws.tabs.zoom = zoom;
        ws.tabs.focused = Some(ws.dock.pane.clone());
        workspaces.push(ws);
    }
    let ids: HashSet<String> = workspaces.iter().map(|s| s.id.clone()).collect();
    for window in &mut windows {
        if window.workspace.as_ref().is_some_and(|id| !ids.contains(id)) {
            window.workspace = None;
        }
    }
    let mut next = workspaces.iter().filter_map(|s| number_in(&s.id, "ws-")).fold(reader.counters.2, usize::max);
    for ws in workspaces.iter_mut().filter(|s| number_in(&s.id, "ws-").is_none()) {
        next += 1;
        let id = format!("ws-{next}");
        for window in windows.iter_mut().filter(|w| w.workspace.as_deref() == Some(ws.id.as_str())) {
            window.workspace = Some(id.clone());
        }
        ws.id = id;
    }
    reader.counters.2 = next;
    windows.retain(|w: &Window| !w.tabs.tabs.is_empty() || w.lead.is_some());
    let counters = reader.counters;
    Ok(SavedLayout { placed: reader.placed, layout: Box::new(Restored { windows, workspaces, orphans, counters }) })
}

pub fn install(model: &mut Model, saved: SavedLayout) {
    let Ok(restored) = saved.layout.downcast::<Restored>() else { return };
    model.windows = restored.windows;
    model.workspaces = restored.workspaces;
    model.counters = (model.counters.0.max(restored.counters.0), model.counters.1.max(restored.counters.1), model.counters.2.max(restored.counters.2));
    model.orphans.extend(restored.orphans);
    model.drop_empty();
}
