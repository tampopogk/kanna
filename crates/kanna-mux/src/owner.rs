use crate::error::KmuxError;
use crate::json::{encode, json, num, string, Map, Value};
use crate::layout::{read_pane_table, serve, LayoutHost, Placement, SplitSide};
use crate::model::{Frame, Model, Node};
use crate::pane::{Pane, PaneType, Panes};
use crate::saved::LAYOUT_VERSION;
use crate::state::{self, number_in};
use crate::state_file::{Contents, StateFile};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub struct LayoutOwner {
    model: Model,
    wide: Arc<Mutex<HashMap<String, bool>>>,
    panes: Panes,
    saved: Option<Value>,
    file: Option<StateFile>,
    last_saved: Option<String>,
}

impl Default for LayoutOwner {
    fn default() -> LayoutOwner {
        let wide: Arc<Mutex<HashMap<String, bool>>> = Arc::default();
        let mut model = Model::new();
        let shapes = wide.clone();
        model.pane_is_wide = Box::new(move |pane| shapes.lock().map_or(true, |w| w.get(pane).copied().unwrap_or(true)));
        LayoutOwner { model, wide, panes: Panes::default(), saved: None, file: None, last_saved: None }
    }
}

impl LayoutOwner {
    pub fn new() -> LayoutOwner {
        LayoutOwner::default()
    }

    pub fn open(file: Option<StateFile>, fresh: bool) -> (LayoutOwner, Option<String>) {
        let mut owner = LayoutOwner { file, ..LayoutOwner::default() };
        let Some(file) = owner.file.clone().filter(|_| !fresh) else { return (owner, None) };
        let problem = match file.load() {
            Contents::None => None,
            Contents::Refused(why) => Some(why),
            Contents::State(state) => match state::import_versions(&state, &[1, 2, LAYOUT_VERSION], "Kanna reads") {
                Ok(_) => {
                    owner.saved = Some(state);
                    None
                }
                Err(error) => Some(error.message),
            },
        };
        let problem = problem.map(|why| format!("not restoring the saved layout: {why}; moved to {}", file.set_aside().unwrap_or_else(|| "nowhere".into())));
        (owner, problem)
    }

    pub fn handle(&mut self, request: &Value) -> Value {
        let result = match string(request, "call").unwrap_or("") {
            "serve" => self.serve(request.get("request").unwrap_or(&Value::Null)),
            "attach" => self.attach(request.get("panes").unwrap_or(&Value::Null)),
            "view" => Ok(self.view()),
            "hello" => Ok([("panes".to_string(), self.hello())].into_iter().collect()),
            "pane" => self.record(request.get("pane").unwrap_or(&Value::Null)),
            "export" => Ok(match self.export() {
                Value::Object(state) => state,
                _ => Map::new(),
            }),
            "focused" => Ok(changed(self.model.set_focused(string(request, "pane").unwrap_or("")))),
            "divider" => self.divider(request),
            "resolve" => self.panes.need(string(request, "pane")).map(|pane| [("id".to_string(), Value::String(pane.id.clone()))].into_iter().collect()),
            "frame" => self.frame(request),
            "shapes" => self.shapes(request),
            "save" => self.save().map(|saved| [("saved".to_string(), Value::Bool(saved))].into_iter().collect()),
            other => Err(KmuxError::new("bad_request", format!("unknown call \"{other}\""))),
        };
        match result {
            Ok(mut reply) => {
                reply.insert("ok".into(), Value::Bool(true));
                Value::Object(reply)
            }
            Err(error) => json!({ "ok": false, "error": { "code": error.code, "message": error.message } }),
        }
    }

    pub fn serve(&mut self, request: &Value) -> Result<Map, KmuxError> {
        if let Some(table) = request.get("panes") {
            let panes = read_pane_table(table)?;
            self.panes.install(panes, 0, 0);
        }
        if string(request, "op") == Some("place") {
            if let [pane] = read_pane_table(&Value::Array(vec![request.get("pane").cloned().unwrap_or(Value::Null)]))?.as_slice() {
                self.learn(pane.clone());
            }
        }
        serve(&mut self.model, request, &self.panes)
    }

    pub fn sync(&mut self, table: &Value) -> Result<bool, KmuxError> {
        let panes = read_pane_table(table)?;
        self.panes.install(panes, 0, 0);
        Ok(self.model.retitle(&self.panes))
    }

    pub fn hello(&self) -> Value {
        let panes = match &self.saved {
            Some(saved) => state::import_versions(saved, &[1, 2, LAYOUT_VERSION], "Kanna reads").map(|(panes, _)| panes).unwrap_or_default(),
            None => {
                let placed: HashSet<String> = self.placed().into_iter().collect();
                self.panes.iter().filter(|p| placed.contains(&p.id)).cloned().collect()
            }
        };
        Value::Array(panes.iter().filter(|p| p.kind != PaneType::Term).map(|p| Value::Object(state::pane_record(p))).collect())
    }

    fn divider(&mut self, request: &Value) -> Result<Map, KmuxError> {
        let tab = string(request, "tab").ok_or_else(|| KmuxError::new("bad_request", "divider needs a tab"))?;
        let path: Vec<usize> = request.get("path").and_then(Value::as_array).map_or(vec![], |p| p.iter().filter_map(Value::as_u64).map(|i| i as usize).collect());
        let index = request.get("index").and_then(Value::as_u64).ok_or_else(|| KmuxError::new("bad_request", "divider needs an index"))? as usize;
        let share = request.get("share").and_then(Value::as_f64).ok_or_else(|| KmuxError::new("bad_request", "divider needs a share"))?;
        self.model.set_divider(tab, &path, index, share)?;
        Ok(Map::new())
    }

    fn frame(&mut self, request: &Value) -> Result<Map, KmuxError> {
        let window = string(request, "window").unwrap_or("");
        let frame = request.get("frame").unwrap_or(&Value::Null);
        let field = |name: &str| frame.get(name).and_then(Value::as_f64).ok_or_else(|| KmuxError::new("bad_request", "a frame has x, y, w and h"));
        let frame = Frame { x: field("x")?, y: field("y")?, w: field("w")?, h: field("h")? };
        Ok(changed(self.model.set_frame(window, frame)))
    }

    fn shapes(&mut self, request: &Value) -> Result<Map, KmuxError> {
        let shapes: HashMap<String, bool> = request.get("panes").and_then(Value::as_object).map_or_else(HashMap::new, |panes| {
            panes.iter().filter_map(|(id, wide)| wide.as_bool().map(|wide| (id.clone(), wide))).collect()
        });
        if let Ok(mut wide) = self.wide.lock() {
            *wide = shapes;
        }
        Ok(Map::new())
    }

    fn record(&mut self, record: &Value) -> Result<Map, KmuxError> {
        let [pane] = read_pane_table(&Value::Array(vec![record.clone()]))?.try_into().map_err(|_| KmuxError::new("bad_request", "one pane"))?;
        if let Some(known) = self.panes.get_mut(&pane.id) {
            *known = pane;
        }
        Ok(changed(self.model.retitle(&self.panes)))
    }

    fn learn(&mut self, pane: Pane) {
        let mut panes: Vec<Pane> = self.panes.iter().filter(|p| p.id != pane.id).cloned().collect();
        panes.push(pane);
        self.panes.install(panes, 0, 0);
    }

    pub fn attach(&mut self, live: &Value) -> Result<Map, KmuxError> {
        let live = read_pane_table(live)?;
        let ids: HashSet<String> = live.iter().map(|p| p.id.clone()).collect();
        self.panes.install(live.clone(), 0, 0);
        let mut problem = None;
        if let Some(saved) = self.saved.take() {
            let pruned = prune_state(&saved, &ids);
            match self.model.read_state(&pruned, &ids) {
                Ok(layout) => self.model.install_state(layout),
                Err(error) => {
                    let aside = self.file.as_ref().and_then(StateFile::set_aside).unwrap_or_else(|| "nowhere".into());
                    problem = Some(format!("not restoring the saved layout: {}; moved to {aside}", error.message));
                }
            }
        }
        let mut dropped = vec![];
        for pane in self.placed() {
            if !ids.contains(&pane) {
                self.model.remove(&pane);
                dropped.push(pane);
            }
        }
        self.model.tidy();
        let placed: HashSet<String> = self.placed().into_iter().collect();
        let mut added = vec![];
        for pane in live.iter().filter(|p| !placed.contains(&p.id)) {
            self.model.place(&pane.id, &Placement { window: None, split: SplitSide::Auto, size: 0.5, new_tab: false, dock: None, frozen: false, from: None, opener: None, shelf: false, backlog: false, lead: false }, &self.panes)?;
            added.push(pane.id.clone());
        }
        self.model.retitle(&self.panes);
        let mut reply = Map::new();
        reply.insert("dropped".into(), dropped.into());
        reply.insert("placed".into(), added.into());
        reply.insert("problem".into(), problem.map_or(Value::Null, Value::String));
        Ok(reply)
    }

    fn placed(&self) -> Vec<String> {
        self.model.placed()
    }

    pub fn view(&self) -> Map {
        let model = &self.model;
        let windows: Vec<Value> = model
            .visible_windows()
            .into_iter()
            .map(|w| {
                let window = &model.windows()[w];
                let shown = model.shown_tabs(w);
                let tab = shown.active_tab();
                let dock = model.shows(w).map(|s| &s.dock);
                let beside = |pane: &String| dock.or(window.lead.as_ref()).is_some_and(|d| &d.pane == pane);
                let zoomed = shown.zoomed().cloned().filter(|z| beside(z) || tab.is_some_and(|t| t.panes().contains(z)));
                let root = match (&zoomed, tab) {
                    (Some(pane), _) if beside(pane) => Value::Null,
                    (Some(pane), _) => self.node(&Node::Pane(pane.clone())),
                    (None, Some(tab)) => tab.root.as_ref().map_or(Value::Null, |root| self.node(root)),
                    (None, None) => Value::Null,
                };
                let dock_view = dock.map_or(Value::Null, |d| {
                    let mut node = self.node(&Node::Pane(d.pane.clone()));
                    node["edge"] = d.edge.as_str().into();
                    node["width"] = num(d.width);
                    if d.frozen {
                        node["frozen"] = Value::Bool(true);
                    }
                    node["name"] = self.panes.get(&d.pane).and_then(|p| p.name.clone()).map_or(Value::Null, Value::String);
                    node
                });
                let title = tab.map(|t| t.title.clone()).or_else(|| dock.or(window.lead.as_ref()).map(|d| self.panes.display(&d.pane)));
                let frame = window.frame.map_or(Value::Null, |f| json!({ "x": f.x, "y": f.y, "w": f.w, "h": f.h }));
                let tabs: Vec<Value> = shown.tabs.iter().map(|t| json!({ "id": t.id, "title": t.title, "panes": t.panes() })).collect();
                json!({
                    "id": window.id,
                    "frame": frame,
                    "tabs": tabs,
                    "tab": tab.map_or(Value::Null, |t| t.id.clone().into()),
                    "title": title.map_or(Value::Null, Value::String),
                    "focused": shown.focused.clone().map_or(Value::Null, Value::String),
                    "zoomed": zoomed.map_or(Value::Null, Value::String),
                    "root": root,
                    "dock": dock_view,
                    "desk": window.desk || dock.is_some(),
                    "project": window.project.clone().map_or(Value::Null, Value::String),
                    "lead": window.lead.as_ref().map_or(Value::Null, |p| json!({ "pane": p.pane, "type": "term", "edge": "left", "width": p.width, "name": self.panes.get(&p.pane).and_then(|p| p.name.clone()) })),
                    "workspace": window.workspace.clone().map_or(Value::Null, Value::String),
                    "shows": model.shows(w).map_or(Value::Null, |s| s.id.clone().into()),
                })
            })
            .collect();
        let shelf: Vec<Value> = (0..model.workspaces().len())
            .filter(|&s| model.workspaces()[s].place == crate::workspace::Where::Shelf)
            .map(|s| {
                let ws = &model.workspaces()[s];
                let task = &ws.dock.pane;
                json!({
                    "task": task,
                    "workspace": ws.id,
                    "type": self.panes.get(task).map_or("term", |p| p.kind.as_str()),
                    "parkedAt": ws.parked_at.map_or(Value::Null, num),
                    "edge": ws.dock.edge.as_str(),
                    "width": num(ws.dock.width),
                    "tabs": ws.tabs.tabs.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
                    "name": self.panes.get(task).and_then(|p| p.name.clone()).map_or(Value::Null, Value::String),
                    "panes": model.workspace_panes(s),
                })
            })
            .collect();
        let held: Vec<String> = model.held().flat_map(|s| model.workspace_panes(s)).collect();
        let mut view = Map::new();
        view.insert("shelf".into(), Value::Array(shelf));
        view.insert("held".into(), held.into());
        view.insert("key".into(), model.key_window().map_or(Value::Null, |w| w.id.clone().into()));
        view.insert("windows".into(), Value::Array(windows));
        view
    }

    fn node(&self, node: &Node) -> Value {
        match node {
            Node::Pane(id) => json!({ "pane": id, "type": self.panes.get(id).map_or("term", |p| p.kind.as_str()) }),
            Node::Split(split) => {
                let children: Vec<Value> = split
                    .kids
                    .iter()
                    .map(|kid| {
                        let mut child = self.node(&kid.node);
                        child["size"] = num(kid.size);
                        child
                    })
                    .collect();
                json!({ "split": split.axis.as_str(), "children": children })
            }
        }
    }

    pub fn export(&self) -> Value {
        let mut out = self.model.export_v3();
        let mut counters = match out.remove("counters") {
            Some(Value::Object(counters)) => counters,
            _ => Map::new(),
        };
        let placed: HashSet<String> = self.placed().into_iter().collect();
        let mut panes = Panes::default();
        panes.install(self.panes.iter().filter(|p| placed.contains(&p.id)).cloned().collect(), 0, 0);
        let counter = panes.iter().filter_map(|p| number_in(&p.id, "p")).max().unwrap_or(0);
        counters.insert("pane".into(), num(counter as f64));
        out.insert("kmux".into(), "state".into());
        out.insert("version".into(), num(LAYOUT_VERSION as f64));
        out.insert("counters".into(), Value::Object(counters));
        out.insert("panes".into(), state::export_panes(&panes));
        Value::Object(out)
    }

    pub fn save(&mut self) -> Result<bool, KmuxError> {
        let Some(file) = &self.file else { return Ok(false) };
        if self.saved.is_some() {
            return Ok(false);
        }
        let state = self.export();
        let encoded = encode(&state);
        if self.last_saved.as_ref() == Some(&encoded) {
            return Ok(false);
        }
        file.write(&state)?;
        self.last_saved = Some(encoded);
        Ok(true)
    }
}

fn changed(changed: bool) -> Map {
    [("changed".to_string(), Value::Bool(changed))].into_iter().collect()
}

pub fn prune_state(state: &Value, live: &HashSet<String>) -> Value {
    fn prune_windows(windows: &mut [Value], live: &HashSet<String>) {
        for window in windows.iter_mut() {
            for key in ["dock", "lead", "planner"] {
                if window.get(key).and_then(|d| d.get("pane")).and_then(Value::as_str).is_some_and(|pane| !live.contains(pane)) {
                    window[key] = Value::Null;
                }
            }
            if let Some(tabs) = window.get_mut("tabs").and_then(Value::as_array_mut) {
                for tab in tabs.iter_mut() {
                    if let Some(layout) = tab.get_mut("layout") {
                        *layout = prune(layout, live).unwrap_or(Value::Null);
                    }
                }
            }
            if let Some(own) = window.get_mut("windows").and_then(Value::as_array_mut) {
                prune_windows(own, live);
            }
        }
    }
    let mut state = state.clone();
    for list in ["windows", "parked", "shelf", "workspaces"] {
        if let Some(windows) = state.get_mut(list).and_then(Value::as_array_mut) {
            prune_windows(windows, live);
        }
    }
    state
}

fn prune(node: &Value, live: &HashSet<String>) -> Option<Value> {
    if node.is_null() {
        return None;
    }
    if let Some(pane) = node.get("pane").filter(|p| !p.is_null()) {
        return pane.as_str().filter(|id| live.contains(*id)).map(|_| node.clone());
    }
    let Some(children) = node.get("children").and_then(Value::as_array) else { return Some(node.clone()) };
    let kept: Vec<Value> = children.iter().filter_map(|child| prune(child, live)).collect();
    if kept.is_empty() {
        return None;
    }
    let mut node = node.clone();
    node["children"] = Value::Array(kept);
    Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::json;

    fn table(ids: &[&str]) -> Value {
        Value::Array(ids.iter().map(|id| json!({ "id": id, "name": null, "type": "term" })).collect())
    }

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kmo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        dir
    }

    fn place(owner: &mut LayoutOwner, id: &str, at: Value) {
        let reply = owner.handle(&json!({ "call": "serve", "request": { "op": "place", "pane": { "id": id, "name": null, "type": "term" }, "at": at } }));
        assert_eq!(reply["ok"], true, "{reply}");
    }

    fn open(owner: &mut LayoutOwner, pane: Value, at: Value) {
        let reply = owner.handle(&json!({ "call": "serve", "request": { "op": "place", "pane": pane, "at": at } }));
        assert_eq!(reply["ok"], true, "{reply}");
    }

    fn titles(owner: &LayoutOwner) -> Vec<String> {
        owner.model.windows().iter().flat_map(|w| w.tabs.tabs.iter().map(|t| t.title.clone())).chain(owner.model.workspaces().iter().flat_map(|s| s.tabs.tabs.iter().map(|t| t.title.clone()))).collect()
    }

    fn command(owner: &mut LayoutOwner, cmd: &str, args: Value) -> Value {
        owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": cmd, "args": args } }))
    }

    #[test]
    fn a_tab_is_named_after_the_pane_it_was_made_with() {
        let mut owner = LayoutOwner::new();
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        open(&mut owner, json!({ "id": "p1", "name": null, "type": "term" }), json!({}));
        open(&mut owner, json!({ "id": "p2", "name": null, "type": "code", "path": "/repo/crates/kanna-daemon/src/store.rs" }), json!({ "tab": true }));
        open(&mut owner, json!({ "id": "p3", "name": null, "type": "md", "path": "docs/specs/presenting.md" }), json!({ "tab": true }));
        open(&mut owner, json!({ "id": "p4", "name": null, "type": "web", "url": "http://localhost:5173/app" }), json!({ "tab": true }));
        open(&mut owner, json!({ "id": "p5", "name": "diff", "type": "term" }), json!({ "tab": true }));
        open(&mut owner, json!({ "id": "p6", "name": "spec", "type": "md", "path": "docs/specs/windows.md" }), json!({ "tab": true }));
        open(&mut owner, json!({ "id": "p7", "name": null, "type": "md", "path": "a/SKILL.md" }), json!({ "tab": true }));
        open(&mut owner, json!({ "id": "p8", "name": null, "type": "md", "path": "b/SKILL.md" }), json!({ "tab": true }));
        open(&mut owner, json!({ "id": "p9", "name": null, "type": "term" }), json!({ "tab": true }));
        assert_eq!(titles(&owner), ["Tab 1", "store.rs", "presenting.md", "localhost", "diff", "spec", "SKILL.md", "SKILL.md", "Tab 9"]);
        let view = Value::Object(owner.view());
        assert_eq!(view["windows"][0]["title"], "Tab 9", "the window is titled after its active tab");
        assert_eq!(view["windows"][0]["tabs"][1]["title"], "store.rs");
    }

    #[test]
    fn a_tab_follows_its_web_page_title_and_its_terminal_renamed() {
        let mut owner = LayoutOwner::new();
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        open(&mut owner, json!({ "id": "p1", "name": null, "type": "web", "url": "https://docs.rs/serde" }), json!({}));
        open(&mut owner, json!({ "id": "p2", "name": null, "type": "term" }), json!({ "tab": true }));
        assert_eq!(titles(&owner), ["docs.rs", "Tab 2"]);
        let reply = owner.handle(&json!({ "call": "pane", "pane": { "id": "p1", "name": null, "type": "web", "url": "https://docs.rs/serde", "title": "serde - Rust" } }));
        assert_eq!(reply["changed"], true);
        assert_eq!(titles(&owner), ["serde - Rust", "Tab 2"]);
        owner.handle(&json!({ "call": "pane", "pane": { "id": "p1", "name": null, "type": "web", "url": "https://github.com/serde-rs", "title": "serde-rs · GitHub" } }));
        assert_eq!(titles(&owner), ["serde-rs · GitHub", "Tab 2"]);
        owner.handle(&json!({ "call": "pane", "pane": { "id": "p1", "name": null, "type": "web", "url": "https://example.com/" } }));
        assert_eq!(titles(&owner), ["example.com", "Tab 2"], "the host until the next title loads");
        owner.handle(&json!({ "call": "serve", "request": { "op": "windows", "panes": [{ "id": "p1", "name": null, "type": "web", "url": "https://example.com/" }, { "id": "p2", "name": "server", "type": "term" }] } }));
        assert_eq!(titles(&owner), ["example.com", "server"], "rename-pane reaches the layout as a windows step with the new names");
        let unchanged = owner.handle(&json!({ "call": "pane", "pane": { "id": "p2", "name": "server", "type": "term" } }));
        assert_eq!(unchanged["changed"], false);
    }

    #[test]
    fn a_renamed_tab_keeps_its_name_after_a_relaunch_and_an_older_layout_takes_its_panes_names() {
        let dir = folder("renamed");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        open(&mut owner, json!({ "id": "p1", "name": null, "type": "web", "url": "https://docs.rs/" }), json!({}));
        open(&mut owner, json!({ "id": "p2", "name": null, "type": "code", "path": "/r/a.rs" }), json!({ "tab": true }));
        assert_eq!(command(&mut owner, "rename-tab", json!({ "tab": "t1", "title": "reading" }))["title"], "reading");
        owner.handle(&json!({ "call": "pane", "pane": { "id": "p1", "name": null, "type": "web", "url": "https://docs.rs/", "title": "Docs.rs" } }));
        assert_eq!(titles(&owner), ["reading", "a.rs"]);
        owner.save().unwrap();
        assert_eq!(owner.export()["windows"][0]["tabs"][0]["renamed"], true);

        let panes = json!([{ "id": "p1", "name": "docs", "type": "web", "url": "https://docs.rs/" }, { "id": "p2", "name": "main", "type": "code", "path": "/r/a.rs" }]);
        let (mut again, _) = LayoutOwner::open(Some(file.clone()), false);
        again.handle(&json!({ "call": "attach", "panes": panes }));
        assert_eq!(titles(&again), ["reading", "main"]);
        again.handle(&json!({ "call": "pane", "pane": { "id": "p2", "name": null, "type": "term" } }));
        assert_eq!(titles(&again), ["reading", "Tab 2"], "a restored tab keeps its number");
        again.handle(&json!({ "call": "pane", "pane": { "id": "p2", "name": "main", "type": "code", "path": "/r/a.rs" } }));

        let mut old = again.export();
        old["windows"][0]["tabs"][0].as_object_mut().unwrap().remove("renamed");
        old["windows"][0]["tabs"][1]["title"] = "Tab 2".into();
        file.write(&old).unwrap();
        let (mut older, _) = LayoutOwner::open(Some(file), false);
        older.handle(&json!({ "call": "attach", "panes": panes }));
        assert_eq!(titles(&older), ["docs", "main"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_parked_tasks_tabs_are_named_after_their_panes() {
        let mut owner = LayoutOwner::new();
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        open(&mut owner, json!({ "id": "p1", "name": null, "type": "term" }), json!({ "dock": "left" }));
        command(&mut owner, "task", json!({ "action": "park" }));
        open(&mut owner, json!({ "id": "p2", "name": null, "type": "code", "path": "/r/store.rs" }), json!({ "tab": true, "from": "p1" }));
        open(&mut owner, json!({ "id": "p3", "name": null, "type": "term" }), json!({ "tab": true, "from": "p1" }));
        assert_eq!(owner.view()["shelf"][0]["tabs"], json!(["store.rs", "Tab 2"]));
    }

    #[test]
    fn a_split_tab_keeps_its_first_panes_name_until_that_pane_closes() {
        let mut owner = LayoutOwner::new();
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        open(&mut owner, json!({ "id": "p1", "name": null, "type": "term" }), json!({}));
        open(&mut owner, json!({ "id": "p2", "name": null, "type": "code", "path": "/r/store.rs" }), json!({ "tab": true }));
        open(&mut owner, json!({ "id": "p3", "name": null, "type": "md", "path": "/r/plan.md" }), json!({ "split": "right" }));
        open(&mut owner, json!({ "id": "p4", "name": null, "type": "term" }), json!({ "split": "down" }));
        assert_eq!(titles(&owner), ["Tab 1", "store.rs"]);
        owner.handle(&json!({ "call": "serve", "request": { "op": "remove", "ids": ["p2"], "tidy": true } }));
        assert_eq!(titles(&owner), ["Tab 1", "plan.md"]);
        owner.handle(&json!({ "call": "serve", "request": { "op": "remove", "ids": ["p3"], "tidy": true } }));
        assert_eq!(titles(&owner), ["Tab 1", "Tab 2"]);
    }

    #[test]
    fn an_open_from_a_terminal_goes_beside_it_in_its_window_not_the_key_one() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({}));
        place(&mut owner, "p2", json!({ "window": "new" }));
        assert_eq!(owner.view()["key"], "w2");
        place(&mut owner, "p3", json!({ "from": "p1" }));
        let view = Value::Object(owner.view());
        assert_eq!(view["key"], "w1");
        assert_eq!(view["windows"][0]["tabs"][0]["panes"], json!(["p1", "p3"]));
        assert_eq!(view["windows"][0]["focused"], "p3");
        assert_eq!(view["windows"][1]["tabs"][0]["panes"], json!(["p2"]));
        place(&mut owner, "p4", json!({ "window": "w2" }));
        place(&mut owner, "p5", json!({ "tab": true, "opener": "p1" }));
        let view = Value::Object(owner.view());
        assert_eq!((view["key"].clone(), view["windows"][0]["tabs"][1]["panes"].clone()), (json!("w1"), json!(["p5"])), "a tab opened from a pane goes in that pane's window");
        place(&mut owner, "p6", json!({ "window": "w2", "tab": true, "opener": "p1" }));
        assert_eq!(owner.view()["windows"][1]["tabs"][1]["panes"], json!(["p6"]), "a window it's asked for wins");
    }

    #[test]
    fn a_saved_layout_comes_back_without_the_panes_that_are_gone() {
        let dir = folder("back");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, problem) = LayoutOwner::open(Some(file.clone()), false);
        assert_eq!(problem, None);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({}));
        place(&mut owner, "p2", json!({ "split": "right", "size": 0.25 }));
        place(&mut owner, "p3", json!({ "split": "down" }));
        assert_eq!(owner.handle(&json!({ "call": "save" }))["saved"], true);
        assert_eq!(owner.handle(&json!({ "call": "save" }))["saved"], false);
        let mode = std::fs::metadata(&file.path).map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o777).unwrap();
        assert_eq!(mode, 0o600);

        let (mut again, problem) = LayoutOwner::open(Some(file), false);
        assert_eq!(problem, None);
        let reply = again.handle(&json!({ "call": "attach", "panes": table(&["p1", "p3", "p4"]) }));
        assert_eq!(reply["placed"], json!(["p4"]), "{reply}");
        let view = again.view();
        assert_eq!(
            Value::Object(view),
            json!({ "key": "w1", "shelf": [], "held": [], "windows": [{ "id": "w1", "frame": null, "desk": false, "project": null, "lead": null, "tabs": [{ "id": "t1", "title": "Tab 1", "panes": ["p1", "p3", "p4"] }], "tab": "t1", "title": "Tab 1",
                "focused": "p4", "zoomed": null, "workspace": null, "shows": null, "root": { "split": "row", "children": [
                { "pane": "p1", "type": "term", "size": 0.75 },
                { "pane": "p3", "type": "term", "size": 0.125 },
                { "pane": "p4", "type": "term", "size": 0.125 }
            ] }, "dock": null }] })
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_records_of_web_markdown_and_ios_panes_are_saved_and_handed_back_in_the_hello() {
        let dir = folder("records");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let site = json!({ "id": "p1", "name": "site", "type": "web", "url": "http://a.test/", "history": true });
        owner.handle(&json!({ "call": "serve", "request": { "op": "place", "pane": site, "at": {} } }));
        owner.handle(&json!({ "call": "serve", "request": { "op": "place", "pane": { "id": "p2", "name": null, "type": "term" }, "at": { "split": "right" } } }));
        owner.handle(&json!({ "call": "pane", "pane": { "id": "p1", "name": "site", "type": "web", "url": "http://b.test/", "history": true } }));
        owner.handle(&json!({ "call": "pane", "pane": { "id": "p9", "name": "stray", "type": "md", "path": "/x.md" } }));
        assert_eq!(owner.handle(&json!({ "call": "hello" }))["panes"], json!([{ "id": "p1", "name": "site", "type": "web", "url": "http://b.test/", "history": true }]));
        owner.save().unwrap();

        let (again, _) = LayoutOwner::open(Some(file), false);
        assert_eq!(again.hello(), json!([{ "id": "p1", "name": "site", "type": "web", "url": "http://b.test/", "history": true }]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_app_moves_dividers_records_focus_and_frames_and_chooses_auto_splits_by_shape() {
        let mut owner = LayoutOwner::new();
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({}));
        owner.handle(&json!({ "call": "shapes", "panes": { "p1": false } }));
        place(&mut owner, "p2", json!({}));
        owner.handle(&json!({ "call": "shapes", "panes": { "p1": true, "p2": true } }));
        place(&mut owner, "p3", json!({}));
        let root = |owner: &LayoutOwner| owner.view()["windows"][0]["root"].clone();
        assert_eq!(root(&owner)["split"], "column");
        assert_eq!(root(&owner)["children"][1]["split"], "row");
        let reply = owner.handle(&json!({ "call": "divider", "tab": "t1", "path": [], "index": 0, "share": 0.25 }));
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(root(&owner)["children"][0]["size"], 0.25);
        assert_eq!(root(&owner)["children"][1]["size"], 0.75);
        assert_eq!(owner.handle(&json!({ "call": "divider", "tab": "t1", "path": [1], "index": 0, "share": 2.0 }))["ok"], false);
        assert_eq!(owner.handle(&json!({ "call": "divider", "tab": "t1", "path": [0], "index": 0, "share": 0.5 }))["ok"], false);
        assert_eq!(owner.handle(&json!({ "call": "focused", "pane": "p1" }))["changed"], true);
        assert_eq!(owner.handle(&json!({ "call": "focused", "pane": "p1" }))["changed"], false);
        assert_eq!(owner.view()["windows"][0]["focused"], "p1");
        assert_eq!(owner.handle(&json!({ "call": "frame", "window": "w1", "frame": { "x": 10, "y": 20, "w": 800, "h": 600 } }))["changed"], true);
        assert_eq!(owner.view()["windows"][0]["frame"], json!({ "x": 10.0, "y": 20.0, "w": 800.0, "h": 600.0 }));
        assert_eq!(owner.export()["windows"][0]["frame"], json!({ "x": 10, "y": 20, "w": 800, "h": 600 }));
    }

    #[test]
    fn zoom_is_the_windows_mode_through_tabs_and_panes_and_after_a_relaunch() {
        let dir = folder("tab-zoom");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({}));
        place(&mut owner, "p2", json!({ "split": "right" }));
        place(&mut owner, "p3", json!({ "tab": true }));
        let command = |owner: &mut LayoutOwner, cmd: &str, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": cmd, "args": args } }));
        let window = |owner: &LayoutOwner| owner.view()["windows"][0].clone();
        command(&mut owner, "zoom", json!({ "pane": "p1" }));
        assert_eq!(window(&owner)["zoomed"], "p1");
        assert_eq!(window(&owner)["root"], json!({ "pane": "p1", "type": "term" }));
        command(&mut owner, "focus", json!({ "pane": "p2" }));
        assert_eq!(window(&owner)["zoomed"], "p2");
        command(&mut owner, "focus", json!({ "tab": "t2" }));
        assert_eq!(window(&owner)["zoomed"], "p3", "switching tabs keeps zoom on");
        command(&mut owner, "focus", json!({ "tab": "t1" }));
        assert_eq!(window(&owner)["zoomed"], "p2");
        place(&mut owner, "p4", json!({ "split": "down" }));
        assert_eq!(window(&owner)["zoomed"], "p4", "a new pane is focused, so it's the one shown");
        owner.save().unwrap();

        let (mut again, _) = LayoutOwner::open(Some(file), false);
        again.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2", "p3", "p4"]) }));
        assert_eq!(window(&again)["zoomed"], "p4");
        command(&mut again, "zoom", json!({ "pane": "p4" }));
        assert_eq!(window(&again)["zoomed"], Value::Null);
        assert_eq!(window(&again)["focused"], "p4");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_task_parks_onto_the_shelf_and_pulls_back_as_it_was_across_a_relaunch() {
        let dir = folder("desk");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({ "dock": "left" }));
        place(&mut owner, "p2", json!({}));
        place(&mut owner, "p3", json!({ "tab": true }));
        let task = |owner: &mut LayoutOwner, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "task", "args": args } }));
        let window = |owner: &LayoutOwner| owner.view()["windows"][0].clone();
        owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "resize", "args": { "pane": "p1", "size": 0.3 } } }));
        let before = window(&owner);
        assert_eq!(before["desk"], true);

        let parked = task(&mut owner, json!({ "action": "park" }));
        assert_eq!(parked["task"], "p1", "{parked}");
        assert_eq!(parked["moves"], json!([{ "task": "p1", "from": "desk", "to": "shelf", "window": "w1" }]));
        let empty = window(&owner);
        assert_eq!((empty["desk"].clone(), empty["dock"].clone(), empty["tabs"].clone()), (json!(true), Value::Null, json!([])));
        assert_eq!(owner.view()["shelf"][0]["task"], "p1");
        assert_eq!((owner.view()["shelf"][0]["workspace"].clone(), owner.export()["workspaces"][0]["where"].clone()), (json!("ws-1"), json!("shelf")));
        assert_eq!(owner.view()["shelf"][0]["tabs"], json!(["Tab 1", "Tab 2"]));
        assert_eq!((owner.view()["shelf"][0]["edge"].clone(), owner.view()["shelf"][0]["width"].clone()), (json!("left"), json!(0.3)), "a parked task keeps its dock's edge and width");
        assert_eq!(owner.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2", "p3"]) }))["placed"], json!([]));
        owner.save().unwrap();

        let (mut again, _) = LayoutOwner::open(Some(file), false);
        assert_eq!(again.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2", "p3"]) }))["placed"], json!([]));
        assert_eq!(again.view()["shelf"][0]["task"], "p1");
        let listed = task(&mut again, json!({ "action": "list" }));
        assert_eq!(listed["tasks"][0]["id"], "p1", "{listed}");
        assert_eq!(listed["tasks"][0]["window"], Value::Null);
        let pulled = task(&mut again, json!({ "action": "pull", "task": "p1" }));
        assert_eq!((pulled["window"].clone(), pulled["moved"].clone()), (json!("w1"), json!(true)), "{pulled}");
        assert_eq!(pulled["moves"], json!([{ "task": "p1", "from": "shelf", "to": "desk", "window": "w1" }]));
        let after = window(&again);
        for field in ["dock", "tabs", "tab", "root", "focused"] {
            assert_eq!(after[field], before[field], "{field}");
        }
        assert_eq!(again.view()["shelf"], json!([]));
        assert_eq!(again.export()["workspaces"][0]["where"], json!({ "desk": "w1" }));
        assert_eq!(task(&mut again, json!({ "action": "pull", "task": "p1" }))["moved"], false);
        assert_eq!(task(&mut again, json!({ "action": "pull", "task": "p1" }))["moves"], Value::Null, "a task already on its Desk doesn't move");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_task_held_in_the_backlog_keeps_its_panes_off_the_shelf_across_a_relaunch_and_comes_back_as_it_was() {
        let dir = folder("held");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({ "dock": "left" }));
        place(&mut owner, "p2", json!({}));
        let task = |owner: &mut LayoutOwner, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "task", "args": args } }));
        let before = owner.view()["windows"][0].clone();

        assert_eq!(task(&mut owner, json!({ "action": "hold", "task": "p1" }))["task"], "p1");
        let empty = owner.view()["windows"][0].clone();
        assert_eq!((empty["desk"].clone(), empty["dock"].clone(), empty["tabs"].clone()), (json!(true), Value::Null, json!([])));
        assert_eq!(owner.view()["shelf"], json!([]));
        assert_eq!(owner.export()["workspaces"][0]["where"], "backlog");
        assert_eq!(task(&mut owner, json!({ "action": "list" }))["tasks"], json!([]));
        assert!(task(&mut owner, json!({ "action": "pull", "task": "p1" }))["error"].is_object());
        assert_eq!(owner.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2"]) }))["placed"], json!([]));
        owner.save().unwrap();

        let (mut again, _) = LayoutOwner::open(Some(file), false);
        assert_eq!(again.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2"]) }))["placed"], json!([]));
        assert_eq!(again.view()["shelf"], json!([]));
        assert_eq!(task(&mut again, json!({ "action": "release", "task": "p1" }))["task"], "p1");
        assert_eq!(again.view()["shelf"][0]["task"], "p1");
        assert!(task(&mut again, json!({ "action": "release", "task": "p1" }))["error"].is_object());
        task(&mut again, json!({ "action": "pull", "task": "p1" }));
        let after = again.view()["windows"][0].clone();
        for field in ["dock", "tabs", "tab", "root", "focused"] {
            assert_eq!(after[field], before[field], "{field}");
        }

        task(&mut again, json!({ "action": "hold", "task": "p1" }));
        again.handle(&json!({ "call": "serve", "request": { "op": "remove", "ids": ["p1"], "tidy": true } }));
        assert_eq!(again.view()["windows"].as_array().unwrap().iter().filter(|w| w["tabs"].as_array().is_some_and(|t| !t.is_empty())).count(), 1, "a held task's tabs outlive its agent in a window of their own");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_task_held_in_the_backlog_opens_focuses_and_zooms_in_its_own_hidden_window_whatever_the_placement() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({ "dock": "left" }));
        place(&mut owner, "p2", json!({}));
        let command = |owner: &mut LayoutOwner, cmd: &str, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": cmd, "args": args } }));
        command(&mut owner, "task", json!({ "action": "hold", "task": "p1" }));
        let shown = owner.view()["windows"][0].clone();
        place(&mut owner, "p3", json!({ "from": "p1" }));
        place(&mut owner, "p4", json!({ "tab": true, "split": "right", "opener": "p3" }));
        assert_eq!(owner.view()["held"], json!(["p2", "p3", "p4", "p1"]), "the app keeps what a held task shows");
        owner.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2", "p3", "p4"]) }));
        assert_eq!(command(&mut owner, "focus", json!({ "pane": "p3" }))["ok"], true);
        assert_eq!(command(&mut owner, "zoom", json!({ "pane": "p3" }))["zoomed"], true);
        assert_eq!(owner.view()["windows"][0], shown, "the Desk the user sees doesn't change");
        command(&mut owner, "task", json!({ "action": "release", "task": "p1" }));
        command(&mut owner, "task", json!({ "action": "pull", "task": "p1" }));
        let pulled = owner.view()["windows"][0].clone();
        assert_eq!((pulled["focused"].clone(), pulled["zoomed"].clone(), pulled["tabs"].as_array().map(Vec::len)), (json!("p3"), json!("p3"), Some(3)), "{pulled}");

        command(&mut owner, "task", json!({ "action": "hold", "task": "p1" }));
        let tab = pulled["tabs"][2]["id"].as_str().unwrap_or("t3").to_string();
        assert_eq!(command(&mut owner, "zoom", json!({ "pane": "p3" }))["zoomed"], false);
        assert_eq!(command(&mut owner, "focus", json!({ "tab": tab }))["ok"], true);
        command(&mut owner, "task", json!({ "action": "release", "task": "p1" }));
        command(&mut owner, "task", json!({ "action": "pull", "task": "p1" }));
        let again = owner.view()["windows"][0].clone();
        assert_eq!((again["focused"].clone(), again["zoomed"].clone()), (json!("p4"), Value::Null), "{again}");

        command(&mut owner, "task", json!({ "action": "hold", "task": "p1" }));
        assert_eq!(command(&mut owner, "zoom", json!({ "pane": "p1" }))["zoomed"], true, "an agent zooms its own pane while hidden");
        command(&mut owner, "task", json!({ "action": "release", "task": "p1" }));
        command(&mut owner, "task", json!({ "action": "pull", "task": "p1" }));
        let agent = owner.view()["windows"][0].clone();
        assert_eq!((agent["focused"].clone(), agent["zoomed"].clone()), (json!("p1"), json!("p1")), "{agent}");
    }

    #[test]
    fn a_desk_opens_on_a_project_with_nothing_in_it_and_keeps_its_project_across_a_relaunch() {
        let dir = folder("project-desk");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let task = |owner: &mut LayoutOwner, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "task", "args": args } }));
        let opened = task(&mut owner, json!({ "action": "desk", "project": "/work/app" }));
        assert_eq!((opened["window"].clone(), opened["project"].clone()), (json!("w1"), json!("/work/app")), "{opened}");
        assert!(task(&mut owner, json!({ "action": "desk", "project": "work/app" }))["error"].is_object());
        let window = owner.view()["windows"][0].clone();
        assert_eq!((window["desk"].clone(), window["dock"].clone(), window["tabs"].clone(), window["project"].clone()), (json!(true), Value::Null, json!([]), json!("/work/app")), "{window}");
        assert_eq!(owner.view()["key"], "w1");
        owner.save().unwrap();

        let (mut again, _) = LayoutOwner::open(Some(file), false);
        again.handle(&json!({ "call": "attach", "panes": [] }));
        assert_eq!(again.view()["windows"][0]["project"], "/work/app", "{:?}", again.view());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_desk_on_a_project_already_open_is_brought_forward_instead_of_opening_another() {
        let dir = folder("desk-reuse").canonicalize().unwrap();
        let at = |name: &str| dir.join(name).to_string_lossy().into_owned();
        for repo in ["app", "other", "third", "fourth", "fifth"] {
            std::fs::create_dir_all(dir.join(repo).join(".git")).unwrap();
        }
        std::fs::create_dir_all(dir.join("app/.git/worktrees/wt")).unwrap();
        std::fs::create_dir_all(dir.join("app/sub")).unwrap();
        std::fs::create_dir_all(dir.join("wt/deep")).unwrap();
        std::fs::create_dir_all(dir.join("fourth/sub")).unwrap();
        std::fs::write(dir.join("wt/.git"), format!("gitdir: {}\n", at("app/.git/worktrees/wt"))).unwrap();
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let task = |owner: &mut LayoutOwner, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "task", "args": args } }));
        let desk = |owner: &mut LayoutOwner, project: &str| {
            let reply = task(owner, json!({ "action": "desk", "project": project }));
            (reply["window"].as_str().unwrap_or("?").to_string(), reply["new"].clone())
        };
        let put = |owner: &mut LayoutOwner, id: &str, cwd: &str, at: Value| {
            let reply = owner.handle(&json!({ "call": "serve", "request": { "op": "place", "pane": { "id": id, "name": null, "type": "term", "cwd": cwd }, "at": at } }));
            assert_eq!(reply["ok"], true, "{reply}");
        };
        let key = |owner: &LayoutOwner| owner.view()["key"].clone();

        assert_eq!(desk(&mut owner, &at("app")), ("w1".into(), json!(true)));
        let again = task(&mut owner, json!({ "action": "desk", "project": at("app") }));
        assert_eq!((again["window"].clone(), again["new"].clone(), again["project"].clone()), (json!("w1"), json!(false), json!(at("app"))), "{again}");
        assert_eq!(desk(&mut owner, &at("app/sub")), ("w1".into(), json!(false)), "a subfolder is in the project");
        assert_eq!(desk(&mut owner, &at("wt/deep")), ("w1".into(), json!(false)), "a worktree's project is its main repo");
        assert_eq!(desk(&mut owner, &at("other")), ("w2".into(), json!(true)));
        assert_eq!(key(&owner), "w2");
        assert_eq!(desk(&mut owner, &at("app")), ("w1".into(), json!(false)));
        assert_eq!(key(&owner), "w1");
        assert_eq!(owner.view()["windows"].as_array().unwrap().len(), 2);

        put(&mut owner, "p1", &at("third"), json!({ "window": "new" }));
        assert_eq!(desk(&mut owner, &at("third")), ("w4".into(), json!(true)), "a plain terminal window isn't a Desk");

        task(&mut owner, json!({ "action": "desk" }));
        put(&mut owner, "p2", &at("fourth/sub"), json!({ "lead": true, "window": "w5" }));
        task(&mut owner, json!({ "action": "desk", "project": at("app") }));
        assert_eq!(desk(&mut owner, &at("fourth")), ("w5".into(), json!(false)), "a Desk with no project is on its planning terminal's");
        assert_eq!(owner.view()["windows"][4]["focused"], "p2");
        put(&mut owner, "p3", &at("fifth"), json!({ "dock": "left", "shelf": true }));
        task(&mut owner, json!({ "action": "pull", "task": "p3", "window": "w5" }));
        task(&mut owner, json!({ "action": "desk", "project": at("app") }));
        assert_eq!(desk(&mut owner, &at("fifth")), ("w5".into(), json!(false)), "a Desk is on its docked task's project");
        assert_eq!(owner.view()["windows"][4]["focused"], "p3");
        assert_eq!(desk(&mut owner, &at("fourth")).1, json!(true), "and no longer on its planning terminal's");

        let fresh = task(&mut owner, json!({ "action": "desk", "project": at("app"), "new": true }));
        assert_eq!((fresh["window"].clone(), fresh["new"].clone()), (json!("w7"), json!(true)), "{fresh}");
        assert_eq!(desk(&mut owner, &at("other")).0, "w2");
        assert_eq!(desk(&mut owner, &at("app")).0, "w7", "of two Desks on a project, the one used last");
        owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "focus", "args": { "window": "w1" } } }));
        assert_eq!(desk(&mut owner, &at("app")).0, "w1", "the key one first");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pull_onto_a_desk_with_a_task_says_both_moved() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let task = |owner: &mut LayoutOwner, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "task", "args": args } }));
        task(&mut owner, json!({ "action": "desk" }));
        place(&mut owner, "p1", json!({ "dock": "left", "shelf": true }));
        place(&mut owner, "p2", json!({ "dock": "left", "shelf": true }));
        task(&mut owner, json!({ "action": "pull", "task": "p1", "window": "w1" }));
        let swapped = task(&mut owner, json!({ "action": "pull", "task": "p2", "window": "w1" }));
        assert_eq!(swapped["moves"], json!([{ "task": "p1", "from": "desk", "to": "shelf", "window": "w1" }, { "task": "p2", "from": "shelf", "to": "desk", "window": "w1" }]), "{swapped}");
        let places: Vec<(Value, Value)> = owner.export()["workspaces"].as_array().unwrap().iter().map(|s| (s["id"].clone(), s["where"].clone())).collect();
        assert_eq!(places, [(json!("ws-1"), json!("shelf")), (json!("ws-2"), json!({ "desk": "w1" }))]);
    }

    #[test]
    fn a_planning_agent_shows_while_no_task_does_is_no_task_and_survives_a_relaunch() {
        let dir = folder("lead");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let task = |owner: &mut LayoutOwner, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "task", "args": args } }));
        let window = |owner: &LayoutOwner| owner.view()["windows"][0].clone();
        task(&mut owner, json!({ "action": "desk", "project": "/work/app" }));
        place(&mut owner, "p1", json!({ "lead": true, "window": "w1" }));
        let reply = owner.handle(&json!({ "call": "serve", "request": { "op": "place", "pane": { "id": "p9", "name": null, "type": "term" }, "at": { "lead": true, "window": "w1" } } }));
        assert_eq!(reply["error"]["code"], "led", "{reply}");
        assert_eq!(window(&owner)["lead"]["pane"], "p1");
        assert_eq!(window(&owner)["focused"], "p1");
        assert_eq!(task(&mut owner, json!({ "action": "list" }))["tasks"], json!([]), "a planning agent isn't a task");
        let moved = owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "move", "args": { "pane": "p1", "dock": "left" } } }));
        assert_eq!(moved["error"]["code"], "lead", "{moved}");

        place(&mut owner, "p2", json!({ "dock": "left", "shelf": true }));
        task(&mut owner, json!({ "action": "pull", "task": "p2", "window": "w1" }));
        assert_eq!((window(&owner)["dock"]["pane"].clone(), window(&owner)["lead"]["pane"].clone(), window(&owner)["focused"].clone()), (json!("p2"), json!("p1"), json!("p2")));
        task(&mut owner, json!({ "action": "park" }));
        assert_eq!((window(&owner)["dock"].clone(), window(&owner)["lead"]["pane"].clone(), window(&owner)["focused"].clone()), (Value::Null, json!("p1"), json!("p1")));
        owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "resize", "args": { "pane": "p1", "size": 0.3 } } }));
        assert_eq!(owner.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2"]) }))["placed"], json!([]));
        owner.save().unwrap();

        let (mut again, _) = LayoutOwner::open(Some(file.clone()), false);
        assert_eq!(again.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2"]) }))["placed"], json!([]));
        assert_eq!(window(&again)["lead"], json!({ "pane": "p1", "type": "term", "edge": "left", "width": 0.3, "name": null }));
        assert_eq!(again.view()["shelf"][0]["task"], "p2");

        let (mut gone, _) = LayoutOwner::open(Some(file), false);
        gone.handle(&json!({ "call": "attach", "panes": table(&["p2"]) }));
        assert_eq!(window(&gone)["lead"], Value::Null);
        assert_eq!(window(&gone)["project"], "/work/app");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plain_window_s_lead_is_docked_beside_its_tabs_takes_what_it_opens_and_is_no_task() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let serve = |owner: &mut LayoutOwner, request: Value| owner.handle(&json!({ "call": "serve", "request": request }));
        let task = |owner: &mut LayoutOwner, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "task", "args": args } }));
        place(&mut owner, "p1", json!({}));
        place(&mut owner, "p2", json!({ "lead": true, "window": "new" }));
        let led = |owner: &LayoutOwner| owner.view()["windows"][1].clone();
        assert_eq!((led(&owner)["id"].clone(), led(&owner)["lead"]["pane"].clone(), led(&owner)["desk"].clone(), led(&owner)["focused"].clone(), led(&owner)["title"].clone()), (json!("w2"), json!("p2"), json!(false), json!("p2"), json!("p2")));
        assert_eq!(owner.view()["key"], "w2");

        place(&mut owner, "p3", json!({ "from": "p2" }));
        place(&mut owner, "p4", json!({ "from": "p2" }));
        let tabs = led(&owner)["tabs"].clone();
        assert_eq!((tabs[0]["panes"].clone(), tabs[1]["panes"].clone(), led(&owner)["lead"]["pane"].clone(), led(&owner)["focused"].clone()), (json!(["p3"]), json!(["p4"]), json!("p2"), json!("p4")), "{tabs}");

        assert_eq!(task(&mut owner, json!({ "action": "list" }))["tasks"], json!([]), "a lead isn't a task");
        assert_eq!(serve(&mut owner, json!({ "op": "command", "cmd": "move", "args": { "pane": "p2", "tab": true } }))["error"]["code"], "lead");
        assert_eq!(serve(&mut owner, json!({ "op": "command", "cmd": "move", "args": { "pane": "p3", "to": "p2" } }))["error"]["code"], "lead");
        let docked = serve(&mut owner, json!({ "op": "place", "pane": { "id": "p5", "name": null, "type": "term" }, "at": { "dock": "left", "window": "w2" } }));
        assert_eq!(docked["error"]["code"], "led", "{docked}");
        assert_eq!(task(&mut owner, json!({ "action": "pull", "task": "p2", "window": "w2" }))["ok"], json!(false));

        let closing = serve(&mut owner, json!({ "op": "panes-in", "window": "w2" }));
        assert_eq!(closing["panes"], json!(["p3", "p4", "p2"]), "{closing}");
    }

    #[test]
    fn a_task_parks_and_pulls_with_its_zoom_and_the_shelf_keeps_its_own_across_a_relaunch() {
        let dir = folder("shelf-zoom");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let command = |owner: &mut LayoutOwner, cmd: &str, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": cmd, "args": args } }));
        let window = |owner: &LayoutOwner| owner.view()["windows"][0].clone();
        let shown = |owner: &LayoutOwner| (window(owner)["focused"].clone(), window(owner)["zoomed"].clone());
        command(&mut owner, "task", json!({ "action": "desk", "project": "/work/app" }));
        place(&mut owner, "p1", json!({ "lead": true, "window": "w1" }));
        place(&mut owner, "p2", json!({ "dock": "left", "shelf": true }));
        command(&mut owner, "task", json!({ "action": "pull", "task": "p2", "window": "w1" }));
        place(&mut owner, "p3", json!({}));
        place(&mut owner, "p4", json!({ "split": "right" }));
        command(&mut owner, "focus", json!({ "pane": "p3" }));
        assert_eq!(command(&mut owner, "zoom", json!({ "pane": "p3" }))["zoomed"], true);
        assert_eq!(shown(&owner), (json!("p3"), json!("p3")));
        assert_eq!(window(&owner)["root"], json!({ "pane": "p3", "type": "term" }));

        command(&mut owner, "task", json!({ "action": "park" }));
        assert_eq!(shown(&owner), (json!("p1"), Value::Null), "the Shelf view isn't zoomed");
        assert_eq!(command(&mut owner, "zoom", json!({ "pane": "p1" }))["zoomed"], true);
        assert_eq!(shown(&owner), (json!("p1"), json!("p1")));
        let listed = owner.handle(&json!({ "call": "serve", "request": { "op": "windows" } }));
        assert_eq!(listed["windows"][0]["zoomed"], "p1", "{listed}");

        command(&mut owner, "task", json!({ "action": "pull", "task": "p2", "window": "w1" }));
        assert_eq!(shown(&owner), (json!("p3"), json!("p3")), "the task comes back zoomed where it was");
        command(&mut owner, "task", json!({ "action": "park" }));
        assert_eq!(shown(&owner), (json!("p1"), json!("p1")), "the Shelf view comes back zoomed");
        command(&mut owner, "task", json!({ "action": "pull", "task": "p2", "window": "w1" }));
        owner.save().unwrap();

        let (mut again, _) = LayoutOwner::open(Some(file), false);
        again.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2", "p3", "p4"]) }));
        assert_eq!(shown(&again), (json!("p3"), json!("p3")));
        command(&mut again, "task", json!({ "action": "park" }));
        assert_eq!(shown(&again), (json!("p1"), json!("p1")), "the Shelf's zoom survives a relaunch behind a task");
        assert_eq!(command(&mut again, "zoom", json!({ "pane": "p1" }))["zoomed"], false);
        command(&mut again, "task", json!({ "action": "pull", "task": "p2", "window": "w1" }));
        assert_eq!(shown(&again), (json!("p3"), json!("p3")));
        assert_eq!(command(&mut again, "zoom", json!({ "pane": "p3" }))["zoomed"], false);
        command(&mut again, "task", json!({ "action": "park" }));
        assert_eq!(shown(&again), (json!("p1"), Value::Null));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_desk_kanna_opens_with_nothing_has_no_folder_until_it_is_given_one() {
        let mut owner = LayoutOwner::new();
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let task = |owner: &mut LayoutOwner, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "task", "args": args } }));
        let opened = task(&mut owner, json!({ "action": "desk" }));
        assert_eq!((opened["window"].clone(), opened["project"].clone()), (json!("w1"), Value::Null), "{opened}");
        assert_eq!(owner.view()["windows"][0]["desk"], true);
        assert!(task(&mut owner, json!({ "action": "desk", "project": "work/app" }))["error"].is_object());
    }

    #[test]
    fn a_parked_task_whose_agent_is_gone_comes_back_as_a_plain_window() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({ "dock": "left" }));
        place(&mut owner, "p2", json!({}));
        owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "task", "args": { "action": "park" } } }));
        let reply = owner.handle(&json!({ "call": "attach", "panes": table(&["p2"]) }));
        assert_eq!(reply["dropped"], json!(["p1"]), "{reply}");
        assert_eq!((owner.view()["shelf"].clone(), owner.export()["workspaces"].clone()), (json!([]), json!([])));
        let windows = owner.view()["windows"].clone();
        assert!(windows.as_array().unwrap().iter().any(|w| w["root"] == json!({ "pane": "p2", "type": "term" })), "{windows}");
    }

    #[test]
    fn a_docked_pane_is_drawn_beside_the_tabs_saved_and_dropped_when_its_pane_is_gone() {
        let dir = folder("dock");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({}));
        place(&mut owner, "p2", json!({ "dock": "right" }));
        let window = |owner: &LayoutOwner| owner.view()["windows"][0].clone();
        assert_eq!(window(&owner)["dock"], json!({ "pane": "p2", "name": null, "type": "term", "edge": "right", "width": 0.4 }));
        assert_eq!(window(&owner)["root"], json!({ "pane": "p1", "type": "term" }));
        assert_eq!(window(&owner)["focused"], "p2");
        assert_eq!(owner.handle(&json!({ "call": "focused", "pane": "p1" }))["changed"], true);
        assert_eq!(owner.handle(&json!({ "call": "focused", "pane": "p2" }))["changed"], true);
        let zoom = owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "zoom", "args": { "pane": "p2" } } }));
        assert_eq!(zoom["zoomed"], true, "{zoom}");
        assert_eq!(window(&owner)["root"], Value::Null);
        assert_eq!(window(&owner)["zoomed"], "p2");
        assert_eq!(owner.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2"]) }))["placed"], json!([]));
        owner.save().unwrap();
        assert_eq!(owner.export()["workspaces"][0]["dock"], json!({ "pane": "p2", "edge": "right", "width": 0.4 }));

        let (mut again, _) = LayoutOwner::open(Some(file.clone()), false);
        again.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2"]) }));
        assert_eq!(window(&again)["dock"]["pane"], "p2");
        assert_eq!(window(&again)["zoomed"], "p2");

        let (mut gone, problem) = LayoutOwner::open(Some(file), false);
        let reply = gone.handle(&json!({ "call": "attach", "panes": table(&["p1"]) }));
        assert_eq!(problem, None);
        assert_eq!(reply["problem"], Value::Null, "{reply}");
        assert_eq!((window(&gone)["dock"].clone(), window(&gone)["desk"].clone(), window(&gone)["focused"].clone()), (Value::Null, json!(false), json!("p1")), "a Desk without its task and with no planning agent is a plain window");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_window_with_only_a_docked_pane_zooms_it_and_keeps_the_zoom_across_a_relaunch() {
        let dir = folder("dock-zoom");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({ "dock": "left" }));
        let window = |owner: &LayoutOwner| owner.view()["windows"][0].clone();
        let zoom = |owner: &mut LayoutOwner| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "zoom", "args": { "pane": "p1" } } }));
        assert_eq!(window(&owner)["tabs"], json!([]));
        assert_eq!(zoom(&mut owner)["zoomed"], true);
        assert_eq!(window(&owner)["zoomed"], "p1");
        assert_eq!(window(&owner)["root"], Value::Null);
        owner.save().unwrap();

        let (mut again, _) = LayoutOwner::open(Some(file), false);
        again.handle(&json!({ "call": "attach", "panes": table(&["p1"]) }));
        assert_eq!(window(&again)["zoomed"], "p1");
        assert_eq!(zoom(&mut again)["zoomed"], false);
        assert_eq!(window(&again)["zoomed"], Value::Null);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zoomed_tab_pane_beside_a_docked_pane_is_drawn_alone_and_unzooms_on_a_second_zoom() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({ "dock": "left" }));
        place(&mut owner, "p2", json!({}));
        place(&mut owner, "p3", json!({ "split": "right" }));
        let window = |owner: &LayoutOwner| owner.view()["windows"][0].clone();
        let split = window(&owner)["root"].clone();
        assert_eq!(split["split"], "row", "{split}");
        let zoom = |owner: &mut LayoutOwner| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "zoom", "args": { "pane": "p3" } } }));
        assert_eq!(zoom(&mut owner)["zoomed"], true);
        assert_eq!(window(&owner)["zoomed"], "p3");
        assert_eq!(window(&owner)["root"], json!({ "pane": "p3", "type": "term" }));
        assert_eq!(window(&owner)["dock"]["pane"], "p1");
        assert_eq!(window(&owner)["focused"], "p3");
        assert_eq!(zoom(&mut owner)["zoomed"], false);
        assert_eq!(window(&owner)["zoomed"], Value::Null);
        assert_eq!(window(&owner)["root"], split);
    }

    #[test]
    fn the_shelf_holds_no_tabs_so_a_pane_opened_onto_it_gets_a_window_of_its_own() {
        let mut owner = LayoutOwner::new();
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let task = |owner: &mut LayoutOwner, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "task", "args": args } }));
        task(&mut owner, json!({ "action": "desk", "project": "/work/app" }));
        place(&mut owner, "p1", json!({ "lead": true, "window": "w1" }));
        place(&mut owner, "p2", json!({}));
        place(&mut owner, "p3", json!({ "window": "w1", "tab": true }));
        let windows = owner.view()["windows"].clone();
        assert_eq!(windows[0]["tabs"], json!([]), "{windows}");
        assert_eq!((windows[1]["desk"].clone(), windows[1]["root"]["pane"].clone()), (json!(false), json!("p2")), "{windows}");
        assert_eq!(windows[2]["root"]["pane"], "p3", "{windows}");
        place(&mut owner, "p4", json!({ "window": "w1", "dock": "left" }));
        place(&mut owner, "p5", json!({ "window": "w1", "tab": true }));
        assert_eq!(owner.view()["windows"][0]["tabs"][0]["panes"], json!(["p5"]), "a Desk with a task holds its tabs");
        owner.handle(&json!({ "call": "serve", "request": { "op": "remove", "ids": ["p4"], "tidy": true } }));
        let windows = owner.view()["windows"].clone();
        assert_eq!((windows[0]["tabs"].clone(), windows[0]["lead"]["pane"].clone()), (json!([]), json!("p1")), "{windows}");
        assert_eq!(windows[3]["root"]["pane"], "p5", "the task's tabs go to a window of their own when its agent closes: {windows}");
    }

    #[test]
    fn a_window_with_only_a_docked_pane_is_titled_after_it() {
        let mut owner = LayoutOwner::new();
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let reply = owner.handle(&json!({ "call": "serve", "request": { "op": "place", "pane": { "id": "p1", "name": "agent", "type": "term" }, "at": { "dock": "left" } } }));
        assert_eq!(reply, json!({ "ok": true, "window": "w1", "tab": null, "dock": "left", "workspace": { "id": "ws-1", "where": { "desk": "w1" }, "hidden": false } }));
        let view = owner.view();
        assert_eq!(view["windows"][0]["title"], "agent");
        assert_eq!(view["windows"][0]["tabs"], json!([]));
        assert_eq!(view["windows"][0]["root"], Value::Null);
        assert_eq!(owner.handle(&json!({ "call": "serve", "request": { "op": "place", "pane": { "id": "p2", "name": null, "type": "term" }, "at": { "dock": "top" } } }))["ok"], false);
    }

    #[test]
    fn a_file_it_cannot_trust_is_set_aside() {
        let dir = folder("bad");
        let path = dir.join("layout.json");
        std::fs::write(&path, "{\"kmux\":\"state\",\"version\":99,\"panes\":[],\"windows\":[]}").unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        let (_, problem) = LayoutOwner::open(Some(StateFile::new(&path.to_string_lossy())), false);
        assert!(problem.as_deref().is_some_and(|p| p.contains("unsupported state version")), "{problem:?}");
        assert!(dir.join("layout.json.bad").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_saved_before_the_saved_layout_is_installed() {
        let dir = folder("early");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": table(&["p1"]) }));
        owner.save().unwrap();
        let before = std::fs::read_to_string(&file.path).unwrap();
        let (mut early, _) = LayoutOwner::open(Some(file.clone()), false);
        assert!(!early.save().unwrap());
        assert_eq!(std::fs::read_to_string(&file.path).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_move_to_a_window_makes_a_new_focused_tab_there_and_to_its_own_window_splits_it_out() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let mv = |owner: &mut LayoutOwner, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "move", "args": args } }));
        let tabs = |owner: &LayoutOwner, w: usize| {
            let view = Value::Object(owner.view());
            let window = &view["windows"][w];
            (window["tabs"].as_array().unwrap().iter().map(|t| t["panes"].clone()).collect::<Vec<_>>(), window["tab"].clone(), window["focused"].clone())
        };
        place(&mut owner, "p1", json!({}));
        place(&mut owner, "p2", json!({ "from": "p1" }));
        place(&mut owner, "p3", json!({ "window": "new" }));
        place(&mut owner, "p4", json!({ "from": "p3" }));
        assert_eq!(mv(&mut owner, json!({ "pane": "p2", "window": "w2" }))["ok"], true);
        assert_eq!(tabs(&owner, 1), (vec![json!(["p3", "p4"]), json!(["p2"])], json!("t3"), json!("p2")), "a new tab at the end of the window, focused, not a split of its active tab");
        assert_eq!(tabs(&owner, 0).0, vec![json!(["p1"])], "the pane left behind keeps the whole tab");
        assert_eq!(owner.view()["key"], "w2");
        let before = Value::Object(owner.view());
        assert_eq!(mv(&mut owner, json!({ "pane": "p2", "window": "w2" }))["ok"], true);
        assert_eq!(Value::Object(owner.view()), before, "a pane alone in its tab, moved to its own window, stays as it is");
        assert_eq!(mv(&mut owner, json!({ "pane": "p4", "window": "w2" }))["ok"], true);
        assert_eq!(tabs(&owner, 1), (vec![json!(["p3"]), json!(["p2"]), json!(["p4"])], json!("t4"), json!("p4")), "a pane in a split, moved to its own window, leaves it for a new tab");
        assert_eq!(mv(&mut owner, json!({ "pane": "p4", "tab": "t2" }))["ok"], true);
        assert_eq!(tabs(&owner, 1).0, vec![json!(["p3", "p4"]), json!(["p2"])], "--tab still joins that tab");
        assert_eq!(mv(&mut owner, json!({ "pane": "p1", "to": "p2", "side": "left" }))["ok"], true);
        assert_eq!(tabs(&owner, 0).0, vec![json!(["p3", "p4"]), json!(["p1", "p2"])], "--to still goes beside the pane");
    }

    fn serve_place(owner: &mut LayoutOwner, id: &str, at: Value) -> Value {
        owner.handle(&json!({ "call": "serve", "request": { "op": "place", "pane": { "id": id, "name": null, "type": "term" }, "at": at } }))
    }

    fn ids(view: &Map) -> Vec<Value> {
        view["windows"].as_array().unwrap().iter().map(|w| w["id"].clone()).collect()
    }

    #[test]
    fn a_tasks_open_lands_in_its_own_workspace_after_another_task_is_pulled_onto_its_old_desk() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        command(&mut owner, "task", json!({ "action": "desk" }));
        place(&mut owner, "p1", json!({ "dock": "left", "shelf": true }));
        place(&mut owner, "p2", json!({ "dock": "left", "shelf": true }));
        command(&mut owner, "task", json!({ "action": "pull", "task": "ws-1", "window": "w1" }));
        place(&mut owner, "p3", json!({ "from": "p1" }));
        command(&mut owner, "task", json!({ "action": "pull", "task": "p2", "window": "w1" }));
        let opened = serve_place(&mut owner, "p4", json!({ "from": "p1" }));
        assert_eq!((opened["window"].clone(), opened["workspace"].clone()), (Value::Null, json!({ "id": "ws-1", "where": "shelf", "hidden": true })), "{opened}");
        assert_eq!(owner.view()["windows"][0]["tabs"], json!([]), "ws-2's Desk part on w1 doesn't get p4");
        assert_eq!(owner.view()["shelf"][0]["panes"], json!(["p3", "p4", "p1"]));
        let focused = command(&mut owner, "focus", json!({ "pane": "p3" }));
        assert_eq!((focused["window"].clone(), focused["workspace"]["where"].clone()), (Value::Null, json!("shelf")), "{focused}");
        assert_eq!(owner.view()["key"], "w1", "focusing a hidden pane leaves the key window");
        command(&mut owner, "task", json!({ "action": "pull", "task": "p1", "window": "w1" }));
        let shown = owner.view()["windows"][0].clone();
        assert_eq!((shown["dock"]["pane"].clone(), shown["focused"].clone(), shown["tabs"].as_array().map(Vec::len)), (json!("p1"), json!("p3"), Some(2)), "{shown}");
        let opened = serve_place(&mut owner, "p5", json!({ "from": "p3" }));
        assert_eq!((opened["window"].clone(), opened["workspace"]["where"].clone()), (json!("w1"), json!({ "desk": "w1" })), "{opened}");
    }

    #[test]
    fn a_workspaces_window_is_hidden_on_park_and_opens_at_its_frame_on_pull() {
        let dir = folder("own-window");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        command(&mut owner, "task", json!({ "action": "desk" }));
        place(&mut owner, "p1", json!({ "dock": "left", "shelf": true }));
        command(&mut owner, "task", json!({ "action": "pull", "task": "p1", "window": "w1" }));
        let opened = serve_place(&mut owner, "p2", json!({ "window": "new", "opener": "p1" }));
        assert_eq!((opened["window"].clone(), opened["workspace"]["id"].clone()), (json!("w2"), json!("ws-1")), "open --window new from a task's pane makes its window: {opened}");
        assert_eq!(owner.view()["windows"][1]["workspace"], "ws-1");
        owner.handle(&json!({ "call": "frame", "window": "w2", "frame": { "x": 40, "y": 50, "w": 600, "h": 400 } }));
        place(&mut owner, "p3", json!({ "window": "new" }));
        assert_eq!(owner.view()["windows"][2]["workspace"], Value::Null, "a window made without a task's pane is free");

        command(&mut owner, "task", json!({ "action": "park", "window": "w1" }));
        assert_eq!(ids(&owner.view()), [json!("w1"), json!("w3")], "a parked task's window closes with it");
        assert_eq!(owner.view()["shelf"][0]["panes"], json!(["p2", "p1"]), "and its panes keep running");
        let hidden = serve_place(&mut owner, "p4", json!({ "window": "w2", "tab": true }));
        assert_eq!((hidden["window"].clone(), hidden["workspace"]["where"].clone()), (json!("w2"), json!("shelf")), "{hidden}");
        assert_eq!(command(&mut owner, "focus", json!({ "window": "w2" }))["error"]["code"], "hidden");
        owner.save().unwrap();
        let saved = owner.export();
        assert_eq!((saved["version"].clone(), saved["workspaces"][0]["id"].clone(), saved["workspaces"][0]["where"].clone(), saved["workspaces"][0]["windows"][0]["id"].clone()), (json!(3), json!("ws-1"), json!("shelf"), json!("w2")));

        let (mut again, _) = LayoutOwner::open(Some(file), false);
        again.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2", "p3", "p4"]) }));
        assert_eq!(ids(&again.view()), [json!("w1"), json!("w3")]);
        command(&mut again, "task", json!({ "action": "pull", "task": "ws-1", "window": "w1" }));
        let view = again.view();
        assert_eq!(ids(&view), [json!("w1"), json!("w2"), json!("w3")]);
        assert_eq!((view["windows"][1]["frame"].clone(), view["windows"][1]["tabs"].as_array().map(Vec::len)), (json!({ "x": 40.0, "y": 50.0, "w": 600.0, "h": 400.0 }), Some(2)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_window_is_assigned_to_a_task_or_freed_and_a_desk_a_lead_or_a_task_not_started_is_refused() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        command(&mut owner, "task", json!({ "action": "desk" }));
        place(&mut owner, "p1", json!({ "dock": "left", "shelf": true }));
        place(&mut owner, "p2", json!({ "window": "new" }));
        place(&mut owner, "p3", json!({ "lead": true, "window": "new" }));
        let window = |owner: &mut LayoutOwner, args: Value| command(owner, "window", args);
        let assigned = window(&mut owner, json!({ "action": "assign", "window": "w2", "workspace": "ws-1" }));
        assert_eq!((assigned["window"].clone(), assigned["workspace"].clone(), assigned["shown"].clone()), (json!("w2"), json!({ "id": "ws-1", "where": "shelf", "hidden": true }), json!(false)), "{assigned}");
        assert_eq!(ids(&owner.view()), [json!("w1"), json!("w3")], "a window assigned to a task on the Shelf hides with it");
        assert_eq!(owner.view()["shelf"][0]["panes"], json!(["p2", "p1"]));
        let freed = window(&mut owner, json!({ "action": "free", "window": "w2" }));
        assert_eq!((freed["workspace"].clone(), freed["was"]["id"].clone(), freed["shown"].clone()), (Value::Null, json!("ws-1"), json!(true)), "{freed}");
        assert_eq!((ids(&owner.view()), owner.view()["key"].clone()), (vec![json!("w1"), json!("w2"), json!("w3")], json!("w2")), "a freed window opens");
        assert_eq!(window(&mut owner, json!({ "action": "assign", "window": "w1", "workspace": "ws-1" }))["error"]["code"], "desk");
        assert_eq!(window(&mut owner, json!({ "action": "free", "window": "w1" }))["error"]["code"], "desk");
        assert_eq!(window(&mut owner, json!({ "action": "assign", "window": "w3", "workspace": "ws-1" }))["error"]["code"], "lead");
        let missing = window(&mut owner, json!({ "action": "assign", "window": "w2", "workspace": "ws-9" }));
        assert_eq!((missing["error"]["code"].clone(), missing["error"]["message"].clone()), (json!("not_found"), json!("no workspace \"ws-9\"")));
        command(&mut owner, "task", json!({ "action": "pull", "task": "ws-1", "window": "w1" }));
        assert_eq!(window(&mut owner, json!({ "action": "assign", "window": "w2", "workspace": "p1" }))["shown"], true, "a window assigned to a task on a Desk stays");
        command(&mut owner, "task", json!({ "action": "hold", "task": "ws-1" }));
        assert_eq!(owner.view()["held"], json!(["p2", "p1"]), "a task moved to the backlog keeps its windows' panes");
        assert_eq!(ids(&owner.view()), [json!("w1"), json!("w3")]);
    }

    #[test]
    fn a_workspace_has_an_id_of_its_own_and_goes_when_its_agent_does() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({ "dock": "left" }));
        place(&mut owner, "p2", json!({}));
        place(&mut owner, "p3", json!({ "window": "new", "opener": "p2" }));
        assert_eq!(owner.view()["windows"][1]["workspace"], "ws-1", "a docked pane's workspace has an ID of its own");
        assert_eq!(command(&mut owner, "task", json!({ "action": "list" }))["tasks"][0]["workspace"], "ws-1");
        owner.handle(&json!({ "call": "serve", "request": { "op": "remove", "ids": ["p1"], "tidy": true } }));
        let view = owner.view();
        assert_eq!((view["windows"][0]["dock"].clone(), view["windows"][0]["tabs"][0]["panes"].clone(), view["windows"][1]["workspace"].clone()), (Value::Null, json!(["p2"]), Value::Null), "its tabs stay as a plain window and its window is free: {view:?}");
        place(&mut owner, "p4", json!({ "dock": "left", "shelf": true }));
        assert_eq!(owner.export()["workspaces"][0]["id"], "ws-2", "IDs aren't reused");
    }

    #[test]
    fn a_version_1_layout_loads_into_workspaces_and_saves_as_version_3() {
        let dir = folder("v1");
        let path = dir.join("layout.json");
        let v1 = json!({ "kmux": "state", "version": 1, "key": "w1", "counters": { "pane": 6, "tab": 4, "window": 2 },
            "panes": [{ "id": "p1", "type": "term" }, { "id": "p2", "type": "term" }, { "id": "p3", "type": "term" }, { "id": "p4", "type": "term" }, { "id": "p5", "type": "term" }, { "id": "p6", "type": "term" }],
            "windows": [
                { "id": "w1", "desk": true, "project": "/work/app", "planner": { "pane": "p6", "width": 0.3 }, "shelfZoom": true, "active": "t1", "focused": "p2", "zoomed": null,
                  "dock": { "pane": "p1", "edge": "left", "width": 0.4, "frozen": true }, "tabs": [{ "id": "t1", "layout": { "pane": "p2" } }] },
                { "id": "w2", "active": "t2", "tabs": [{ "id": "t2", "layout": { "pane": "p3" } }] }],
            "parked": [
                { "id": "p4", "desk": true, "parkedAt": 100, "dock": { "pane": "p4", "edge": "left", "width": 0.4 }, "tabs": [{ "id": "t3", "layout": { "pane": "p5" } }], "active": "t3" }] });
        std::fs::write(&path, crate::json::encode(&v1)).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        let file = StateFile::new(&path.to_string_lossy());
        let (mut owner, problem) = LayoutOwner::open(Some(file.clone()), false);
        assert_eq!(problem, None);
        assert_eq!(owner.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2", "p3", "p4", "p5", "p6"]) }))["placed"], json!([]));
        let view = owner.view();
        assert_eq!((view["windows"][0]["dock"]["pane"].clone(), view["windows"][0]["lead"]["pane"].clone(), view["windows"][0]["focused"].clone()), (json!("p1"), json!("p6"), json!("p2")), "{view:?}");
        assert_eq!((view["shelf"][0]["task"].clone(), view["shelf"][0]["workspace"].clone()), (json!("p4"), json!("ws-2")));
        owner.save().unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], 3);
        assert_eq!(saved["windows"][0]["shows"], "ws-1");
        assert_eq!(saved["windows"][0]["lead"]["pane"], "p6");
        assert_eq!(saved["workspaces"].as_array().map(|w| w.iter().map(|s| (s["id"].clone(), s["where"].clone())).collect::<Vec<_>>()), Some(vec![(json!("ws-1"), json!({ "desk": "w1" })), (json!("ws-2"), json!("shelf"))]));
        command(&mut owner, "task", json!({ "action": "park" }));
        assert_eq!(owner.view()["windows"][0]["zoomed"], "p6", "the Shelf view keeps its zoom");
        owner.save().unwrap();

        let (mut again, _) = LayoutOwner::open(Some(file), false);
        again.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2", "p3", "p4", "p5", "p6"]) }));
        assert_eq!(again.view()["shelf"].as_array().map(|s| s.iter().map(|t| t["workspace"].clone()).collect::<Vec<_>>()), Some(vec![json!("ws-1"), json!("ws-2")]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_version_2_layout_s_task_named_workspaces_get_ids_of_their_own_and_their_windows_follow() {
        let dir = folder("v2");
        let path = dir.join("layout.json");
        let v2 = json!({ "kmux": "state", "version": 2, "key": "w1", "counters": { "pane": 4, "tab": 2, "window": 3 },
            "panes": [{ "id": "p1", "type": "term" }, { "id": "p2", "type": "term" }, { "id": "p3", "type": "term" }, { "id": "p4", "type": "term" }],
            "windows": [{ "id": "w1", "desk": true, "lead": { "pane": "p4", "width": 0.4 }, "shows": "task-1dfb12c2", "tabs": [] }],
            "workspaces": [
                { "id": "task-1dfb12c2", "task": "task-1dfb12c2", "where": { "desk": "w1" }, "dock": { "pane": "p1", "edge": "left", "width": 0.4, "frozen": true },
                  "tabs": [{ "id": "t1", "layout": { "pane": "p2" } }], "active": "t1", "focused": "p2",
                  "windows": [{ "id": "w3", "tabs": [{ "id": "t2", "layout": { "pane": "p3" } }], "active": "t2" }] }] });
        std::fs::write(&path, crate::json::encode(&v2)).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        let (mut owner, problem) = LayoutOwner::open(Some(StateFile::new(&path.to_string_lossy())), false);
        assert_eq!(problem, None);
        assert_eq!(owner.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2", "p3", "p4"]) }))["placed"], json!([]));
        let saved = owner.export();
        assert_eq!((saved["workspaces"][0]["id"].clone(), saved["workspaces"][0].get("task").cloned(), saved["workspaces"][0]["windows"][0]["id"].clone()), (json!("ws-1"), None, json!("w3")), "{saved}");
        assert_eq!((saved["windows"][0]["shows"].clone(), saved["counters"]["workspace"].clone()), (json!("ws-1"), json!(1)));
        assert_eq!(owner.view()["windows"][1]["workspace"], "ws-1", "its window follows");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "needs KANNA_REAL_LAYOUT, a copy of a real layout.json"]
    fn a_real_layout_s_workspaces_get_ids_of_their_own_with_their_tabs_and_windows() {
        let dir = folder("real-layout");
        let path = dir.join("layout.json");
        std::fs::copy(std::env::var("KANNA_REAL_LAYOUT").unwrap(), &path).unwrap();
        let before: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let live: Vec<Value> = before["panes"].as_array().unwrap().iter().map(|p| json!({ "id": p["id"], "name": p.get("name").cloned().unwrap_or(Value::Null), "type": p.get("type").cloned().unwrap_or(json!("term")) })).collect();
        let (mut owner, problem) = LayoutOwner::open(Some(StateFile::new(&path.to_string_lossy())), false);
        assert_eq!(problem, None);
        owner.handle(&json!({ "call": "attach", "panes": live }));
        owner.save().unwrap();
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut renamed = vec![];
        for old in before["workspaces"].as_array().unwrap() {
            let new = after["workspaces"].as_array().unwrap().iter().find(|w| w["dock"]["pane"] == old["dock"]["pane"]).unwrap_or_else(|| panic!("{} lost", old["id"]));
            assert!(new["id"].as_str().is_some_and(|id| id.starts_with("ws-")) && new.get("task").is_none(), "{new}");
            let tabs = |ws: &Value| ws["tabs"].as_array().unwrap().iter().map(|t| (t["id"].clone(), t["layout"].clone())).collect::<Vec<_>>();
            assert_eq!(tabs(new), tabs(old), "{}'s tabs", old["id"]);
            let windows = |ws: &Value| ws["windows"].as_array().unwrap().iter().map(|w| w["id"].clone()).collect::<Vec<_>>();
            assert_eq!(windows(new), windows(old), "{}'s windows", old["id"]);
            renamed.push(format!("{} is {}", old["id"].as_str().unwrap(), new["id"].as_str().unwrap()));
        }
        println!("{} workspaces: {}", renamed.len(), renamed.join(", "));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_drops_empty_splits() {
        let live: HashSet<String> = ["p2".to_string()].into_iter().collect();
        let node = json!({ "split": "row", "children": [{ "pane": "p1" }, { "split": "column", "children": [{ "pane": "p2" }, { "pane": "p3" }] }] });
        assert_eq!(prune(&node, &live), Some(json!({ "split": "row", "children": [{ "split": "column", "children": [{ "pane": "p2" }] }] })));
        assert_eq!(prune(&json!({ "pane": "p9" }), &live), None);
    }
}
