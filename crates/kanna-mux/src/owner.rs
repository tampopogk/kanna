use crate::error::KmuxError;
use crate::json::{encode, json, num, string, Map, Value};
use crate::layout::{read_pane_table, serve, LayoutHost, Placement};
use crate::model::{Model, Node};
use crate::pane::{Pane, PaneType, Panes};
use crate::saved::LAYOUT_VERSION;
use crate::state::{self, number_in};
use crate::state_file::{Contents, StateFile};
use std::collections::{HashMap, HashSet};

pub struct LayoutOwner {
    model: Model,
    panes: Panes,
    saved: Option<Value>,
    file: Option<StateFile>,
    last_saved: Option<String>,
}

impl Default for LayoutOwner {
    fn default() -> LayoutOwner {
        LayoutOwner { model: Model::new(), panes: Panes::default(), saved: None, file: None, last_saved: None }
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
                    owner.panes.note_numbers(&content_counters(&state));
                    owner.saved = Some(state);
                    None
                }
                Err(error) => Some(error.message),
            },
        };
        let problem = problem.map(|why| format!("not restoring the saved layout: {why}; moved to {}", file.set_aside().unwrap_or_else(|| "nowhere".into())));
        (owner, problem)
    }

    pub fn restoring(state: Value) -> Result<LayoutOwner, KmuxError> {
        state::import_versions(&state, &[1, 2, LAYOUT_VERSION], "Kanna reads")?;
        let mut owner = LayoutOwner::default();
        owner.panes.note_numbers(&content_counters(&state));
        owner.saved = Some(state);
        Ok(owner)
    }

    pub fn content_numbers(&self) -> HashMap<String, usize> {
        self.panes.numbers()
    }

    pub fn changed_state(&mut self) -> Option<Value> {
        if self.saved.is_some() {
            return None;
        }
        let state = self.export();
        let encoded = encode(&state);
        if self.last_saved.as_ref() == Some(&encoded) {
            return None;
        }
        self.last_saved = Some(encoded);
        Some(state)
    }

    pub fn forget_saved(&mut self) {
        self.last_saved = None;
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
        Value::Array(self.records().into_iter().filter(|p| p.kind != PaneType::Term).map(|p| Value::Object(state::pane_record(&p))).collect())
    }

    pub fn records(&self) -> Vec<Pane> {
        match &self.saved {
            Some(saved) => state::import_versions(saved, &[1, 2, LAYOUT_VERSION], "Kanna reads").map(|(panes, _)| panes).unwrap_or_default(),
            None => {
                let placed: HashSet<String> = self.placed().into_iter().collect();
                self.panes.iter().filter(|p| placed.contains(&p.id)).cloned().collect()
            }
        }
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
            self.model.place(&pane.id, &Placement::default(), &self.panes)?;
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
            .sorted_windows()
            .into_iter()
            .map(|w| {
                let window = &model.windows()[w];
                let mut out = self.shown(&window.tabs, window.lead.as_ref());
                out.insert("id".into(), window.id.clone().into());
                out.insert("lead".into(), window.lead.as_ref().map_or(Value::Null, |d| self.dock(d)));
                out.insert("workspace".into(), window.workspace.clone().map_or(Value::Null, Value::String));
                Value::Object(out)
            })
            .collect();
        let workspaces: Vec<Value> = model
            .workspaces()
            .iter()
            .enumerate()
            .map(|(s, ws)| {
                let mut out = self.shown(&ws.tabs, Some(&ws.dock));
                out.insert("id".into(), ws.id.clone().into());
                out.insert("where".into(), ws.place.to_json());
                out.insert("dock".into(), self.dock(&ws.dock));
                out.insert("agent".into(), self.panes.get(&ws.dock.pane).and_then(|p| p.program()).map_or(Value::Null, Value::String));
                out.insert("parkedAt".into(), ws.parked_at.map_or(Value::Null, num));
                out.insert("windows".into(), model.workspace_windows(&ws.id).iter().map(|w| w.id.clone()).collect::<Vec<_>>().into());
                out.insert("panes".into(), model.workspace_panes(s).into());
                Value::Object(out)
            })
            .collect();
        let mut view = Map::new();
        view.insert("key".into(), model.key_window().map_or(Value::Null, |w| w.id.clone().into()));
        view.insert("windows".into(), Value::Array(windows));
        view.insert("workspaces".into(), Value::Array(workspaces));
        view
    }

    fn shown(&self, tabs: &crate::tabs::Tabs, side: Option<&crate::model::Dock>) -> Map {
        let tab = tabs.active_tab();
        let beside = |pane: &String| side.is_some_and(|d| &d.pane == pane);
        let zoomed = tabs.zoomed().cloned().filter(|z| beside(z) || tab.is_some_and(|t| t.panes().contains(z)));
        let root = match (&zoomed, tab) {
            (Some(pane), _) if beside(pane) => Value::Null,
            (Some(pane), _) => self.node(&Node::Pane(pane.clone())),
            (None, Some(tab)) => tab.root.as_ref().map_or(Value::Null, |root| self.node(root)),
            (None, None) => Value::Null,
        };
        let title = tab.map(|t| t.title.clone()).or_else(|| side.map(|d| self.panes.display(&d.pane)));
        let listed: Vec<Value> = tabs.tabs.iter().map(|t| json!({ "id": t.id, "title": t.title, "panes": t.panes() })).collect();
        let mut out = Map::new();
        out.insert("tabs".into(), Value::Array(listed));
        out.insert("tab".into(), tab.map_or(Value::Null, |t| t.id.clone().into()));
        out.insert("title".into(), title.map_or(Value::Null, Value::String));
        out.insert("focused".into(), tabs.focused.clone().map_or(Value::Null, Value::String));
        out.insert("zoomed".into(), zoomed.map_or(Value::Null, Value::String));
        out.insert("root".into(), root);
        out
    }

    fn dock(&self, dock: &crate::model::Dock) -> Value {
        let mut node = self.node(&Node::Pane(dock.pane.clone()));
        node["edge"] = dock.edge.as_str().into();
        node["width"] = num(dock.width);
        if dock.frozen {
            node["frozen"] = Value::Bool(true);
        }
        node["name"] = self.panes.get(&dock.pane).and_then(|p| p.name.clone()).map_or(Value::Null, Value::String);
        node
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
        for (prefix, n) in self.panes.numbers().into_iter().filter(|(_, n)| *n > 0) {
            counters.insert(prefix, num(n as f64));
        }
        out.insert("kmux".into(), "state".into());
        out.insert("version".into(), num(LAYOUT_VERSION as f64));
        out.insert("counters".into(), Value::Object(counters));
        out.insert("panes".into(), state::export_panes(&panes));
        Value::Object(out)
    }

    pub fn save(&mut self) -> Result<bool, KmuxError> {
        let Some(file) = self.file.clone() else { return Ok(false) };
        let Some(state) = self.changed_state() else { return Ok(false) };
        if let Err(error) = file.write(&state) {
            self.last_saved = None;
            return Err(error);
        }
        Ok(true)
    }
}

fn content_counters(state: &Value) -> HashMap<String, usize> {
    let counters = state.get("counters");
    ["file", "page", "ios", "diff"].into_iter().filter_map(|prefix| Some((prefix.to_string(), counters?.get(prefix)?.as_f64().filter(|n| *n >= 0.0)? as usize))).collect()
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
        assert_eq!(command(&mut owner, "rename-tab", json!({ "tab": "tab-1", "title": "reading" }))["title"], "reading");
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
        open(&mut owner, json!({ "id": "p2", "name": null, "type": "code", "path": "/r/store.rs" }), json!({ "tab": true, "from": "p1" }));
        open(&mut owner, json!({ "id": "p3", "name": null, "type": "term" }), json!({ "tab": true, "from": "p1" }));
        assert_eq!(owner.view()["workspaces"][0]["tabs"].as_array().unwrap().iter().map(|t| t["title"].clone()).collect::<Vec<_>>(), [json!("store.rs"), json!("Tab 2")]);
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
        assert_eq!(owner.view()["key"], "win-2");
        place(&mut owner, "p3", json!({ "from": "p1" }));
        let view = Value::Object(owner.view());
        assert_eq!(view["key"], "win-1");
        assert_eq!(view["windows"][0]["tabs"][0]["panes"], json!(["p1", "p3"]));
        assert_eq!(view["windows"][0]["focused"], "p3");
        assert_eq!(view["windows"][1]["tabs"][0]["panes"], json!(["p2"]));
        place(&mut owner, "p4", json!({ "window": "win-2" }));
        place(&mut owner, "p5", json!({ "tab": true, "opener": "p1" }));
        let view = Value::Object(owner.view());
        assert_eq!((view["key"].clone(), view["windows"][0]["tabs"][1]["panes"].clone()), (json!("win-1"), json!(["p5"])), "a tab opened from a pane goes in that pane's window");
        place(&mut owner, "p6", json!({ "window": "win-2", "tab": true, "opener": "p1" }));
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
            json!({ "key": "win-1", "workspaces": [], "windows": [{ "id": "win-1", "lead": null, "tabs": [{ "id": "tab-1", "title": "Tab 1", "panes": ["p1", "p3", "p4"] }], "tab": "tab-1", "title": "Tab 1",
                "focused": "p4", "zoomed": null, "workspace": null, "root": { "split": "row", "children": [
                { "pane": "p1", "type": "term", "size": 0.75 },
                { "split": "column", "size": 0.25, "children": [{ "pane": "p3", "type": "term", "size": 0.5 }, { "pane": "p4", "type": "term", "size": 0.5 }] }
            ] } }] })
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
    fn auto_splits_go_by_the_shape_the_pane_has_in_the_layout_and_a_divider_moves_two_neighbours() {
        let mut owner = LayoutOwner::new();
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({}));
        place(&mut owner, "p2", json!({ "split": "auto" }));
        place(&mut owner, "p3", json!({ "split": "auto" }));
        let root = owner.view()["windows"][0]["root"].clone();
        assert_eq!(root["split"], "row", "{root}");
        assert_eq!(root["children"][1]["split"], "column", "{root}");
        let divided = command(&mut owner, "resize", json!({ "tab": "tab-1", "path": [], "index": 0, "share": 0.25 }));
        assert_eq!(divided["ok"], true, "{divided}");
        let root = owner.view()["windows"][0]["root"].clone();
        assert_eq!((root["children"][0]["size"].clone(), root["children"][1]["size"].clone()), (json!(0.25), json!(0.75)), "{root}");
        assert_eq!(command(&mut owner, "resize", json!({ "tab": "tab-1", "path": [1], "index": 0, "share": 2.0 }))["ok"], false);
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
        command(&mut owner, "focus", json!({ "tab": "tab-2" }));
        assert_eq!(window(&owner)["zoomed"], "p3", "switching tabs keeps zoom on");
        command(&mut owner, "focus", json!({ "tab": "tab-1" }));
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
    fn a_plain_window_s_lead_is_docked_beside_its_tabs_takes_what_it_opens_and_is_no_task() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let serve = |owner: &mut LayoutOwner, request: Value| owner.handle(&json!({ "call": "serve", "request": request }));
        let task = |owner: &mut LayoutOwner, args: Value| owner.handle(&json!({ "call": "serve", "request": { "op": "command", "cmd": "task", "args": args } }));
        place(&mut owner, "p1", json!({}));
        place(&mut owner, "p2", json!({ "lead": true, "window": "new" }));
        let led = |owner: &LayoutOwner| owner.view()["windows"][1].clone();
        assert_eq!((led(&owner)["id"].clone(), led(&owner)["lead"]["pane"].clone(), led(&owner)["focused"].clone(), led(&owner)["title"].clone()), (json!("win-2"), json!("p2"), json!("p2"), json!("p2")));
        assert_eq!(owner.view()["key"], "win-2");

        place(&mut owner, "p3", json!({ "from": "p2" }));
        place(&mut owner, "p4", json!({ "from": "p2" }));
        let tabs = led(&owner)["tabs"].clone();
        assert_eq!((tabs[0]["panes"].clone(), tabs[1]["panes"].clone(), led(&owner)["lead"]["pane"].clone(), led(&owner)["focused"].clone()), (json!(["p3"]), json!(["p4"]), json!("p2"), json!("p4")), "{tabs}");

        assert_eq!(task(&mut owner, json!({ "action": "list" }))["tasks"], json!([]), "a lead isn't a task");
        assert_eq!(serve(&mut owner, json!({ "op": "command", "cmd": "move", "args": { "pane": "p2", "tab": true } }))["error"]["code"], "lead");
        assert_eq!(serve(&mut owner, json!({ "op": "command", "cmd": "move", "args": { "pane": "p3", "to": "p2" } }))["error"]["code"], "lead");

        let closing = serve(&mut owner, json!({ "op": "panes-in", "window": "win-2" }));
        assert_eq!(closing["panes"], json!(["p3", "p4", "p2"]), "{closing}");
    }

    #[test]
    fn a_task_is_claimed_by_an_app_held_in_the_backlog_and_claimed_again_across_a_relaunch() {
        let dir = folder("claim");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({ "dock": "left" }));
        place(&mut owner, "p2", json!({ "from": "p1" }));
        place(&mut owner, "p3", json!({ "from": "p1" }));
        let workspace = |owner: &LayoutOwner| owner.view()["workspaces"][0].clone();
        let before = workspace(&owner);
        assert_eq!((before["id"].clone(), before["where"].clone(), owner.view()["windows"].clone()), (json!("ws-1"), json!({ "shelf": "mac" }), json!([])), "a task's workspace is in no window");
        assert_eq!(before["tabs"].as_array().map(Vec::len), Some(2));
        let listed = command(&mut owner, "task", json!({ "action": "list" }));
        assert_eq!((listed["tasks"][0]["id"].clone(), listed["tasks"][0]["where"].clone()), (json!("p1"), json!({ "shelf": "mac" })), "{listed}");
        assert_eq!(command(&mut owner, "task", json!({ "action": "list", "app": "ios" }))["tasks"], json!([]), "another app's Shelf is its own");

        let held = command(&mut owner, "task", json!({ "action": "hold", "task": "p1" }));
        assert_eq!((held["task"].clone(), held["was"].clone()), (json!("p1"), json!({ "shelf": "mac" })), "{held}");
        assert_eq!(workspace(&owner)["where"], "backlog");
        assert_eq!(command(&mut owner, "task", json!({ "action": "list" }))["tasks"], json!([]));
        assert_eq!(command(&mut owner, "task", json!({ "action": "claim", "task": "p1" }))["error"]["code"], "bad_request");
        let claimed = command(&mut owner, "task", json!({ "action": "claim", "task": "ws-1", "app": "ios" }));
        assert_eq!((claimed["task"].clone(), claimed["workspace"].clone(), claimed["was"].clone()), (json!("p1"), json!("ws-1"), json!("backlog")), "{claimed}");
        assert_eq!(owner.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2", "p3"]) }))["placed"], json!([]));
        owner.save().unwrap();

        let (mut again, _) = LayoutOwner::open(Some(file), false);
        assert_eq!(again.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2", "p3"]) }))["placed"], json!([]));
        let after = workspace(&again);
        assert_eq!(after["where"], json!({ "shelf": "ios" }));
        for field in ["dock", "tabs", "tab", "root", "focused"] {
            assert_eq!(after[field], before[field], "{field}");
        }
        assert_eq!(command(&mut again, "task", json!({ "action": "pull", "task": "p1" }))["error"]["code"], "bad_request", "pulling is the app's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tasks_workspace_opens_focuses_and_zooms_with_no_window_and_its_tabs_outlive_its_agent() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({ "dock": "left" }));
        place(&mut owner, "p2", json!({ "from": "p1" }));
        place(&mut owner, "p3", json!({ "from": "p2" }));
        let opened = serve_place(&mut owner, "p4", json!({ "workspace": "ws-1", "tab": true }));
        assert_eq!((opened["window"].clone(), opened["workspace"].clone()), (Value::Null, json!({ "id": "ws-1", "where": { "shelf": "mac" } })), "{opened}");
        let workspace = |owner: &LayoutOwner| owner.view()["workspaces"][0].clone();
        assert_eq!(workspace(&owner)["panes"], json!(["p2", "p3", "p4", "p1"]));
        assert_eq!(command(&mut owner, "focus", json!({ "pane": "p3" }))["ok"], true);
        assert_eq!(command(&mut owner, "zoom", json!({ "pane": "p3" }))["zoomed"], true);
        assert_eq!((workspace(&owner)["focused"].clone(), workspace(&owner)["zoomed"].clone(), workspace(&owner)["root"].clone()), (json!("p3"), json!("p3"), json!({ "pane": "p3", "type": "term" })));
        assert_eq!(command(&mut owner, "zoom", json!({ "pane": "p1" }))["zoomed"], true, "an agent zooms its own pane");
        assert_eq!((workspace(&owner)["zoomed"].clone(), workspace(&owner)["root"].clone()), (json!("p1"), Value::Null));
        assert_eq!(workspace(&owner)["dock"], json!({ "pane": "p1", "type": "term", "edge": "left", "width": 0.4, "name": null }));

        owner.handle(&json!({ "call": "serve", "request": { "op": "remove", "ids": ["p1"], "tidy": true } }));
        let view = owner.view();
        assert_eq!((view["workspaces"].clone(), view["windows"].as_array().map(Vec::len)), (json!([]), Some(1)), "its tabs go to a window of their own: {view:?}");
        place(&mut owner, "p5", json!({ "dock": "left" }));
        assert_eq!(owner.export()["workspaces"][0]["id"], "ws-2", "IDs aren't reused");
    }

    #[test]
    fn cycling_tabs_from_the_docked_agent_shows_the_tab_and_keeps_focus_on_the_agent() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({ "dock": "left" }));
        place(&mut owner, "p2", json!({ "from": "p1" }));
        place(&mut owner, "p3", json!({ "from": "p1" }));
        let workspace = |owner: &LayoutOwner| owner.view()["workspaces"][0].clone();
        let tab = |owner: &LayoutOwner, i: usize| workspace(owner)["tabs"][i]["id"].clone();
        let shown = |owner: &LayoutOwner| (workspace(owner)["tab"].clone(), workspace(owner)["focused"].clone());
        let (first, second) = (tab(&owner, 0), tab(&owner, 1));
        command(&mut owner, "focus", json!({ "pane": "p2" }));
        command(&mut owner, "focus", json!({ "pane": "p1" }));
        assert_eq!(shown(&owner), (first.clone(), json!("p1")));
        command(&mut owner, "focus", json!({ "tab": second, "keep": "dock" }));
        assert_eq!(shown(&owner), (second.clone(), json!("p1")), "the next tab is shown and the agent keeps focus");
        assert_eq!((owner.export()["workspaces"][0]["focused"].clone(), owner.export()["workspaces"][0]["active"].clone()), (json!("p1"), second.clone()));
        command(&mut owner, "focus", json!({ "tab": first, "keep": "dock" }));
        assert_eq!(shown(&owner), (first.clone(), json!("p1")), "the previous tab is shown and the agent keeps focus");
        command(&mut owner, "focus", json!({ "pane": "p2" }));
        command(&mut owner, "focus", json!({ "tab": second, "keep": "dock" }));
        assert_eq!(shown(&owner), (second.clone(), json!("p3")), "from a tab's pane, focus follows into the shown tab");
        command(&mut owner, "focus", json!({ "tab": first }));
        assert_eq!(shown(&owner), (first, json!("p2")), "without keep, focus goes into the tab");
    }

    #[test]
    fn a_task_with_only_its_agent_is_titled_after_it_and_keeps_its_zoom_across_a_relaunch() {
        let dir = folder("dock-zoom");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let reply = owner.handle(&json!({ "call": "serve", "request": { "op": "place", "pane": { "id": "p1", "name": "agent", "type": "term" }, "at": { "dock": "left" } } }));
        assert_eq!(reply, json!({ "ok": true, "window": null, "tab": null, "dock": "left", "workspace": { "id": "ws-1", "where": { "shelf": "mac" } } }));
        let workspace = |owner: &LayoutOwner| owner.view()["workspaces"][0].clone();
        assert_eq!((workspace(&owner)["title"].clone(), workspace(&owner)["tabs"].clone(), workspace(&owner)["root"].clone()), (json!("agent"), json!([]), Value::Null));
        assert_eq!(command(&mut owner, "zoom", json!({ "pane": "p1" }))["zoomed"], true);
        owner.save().unwrap();
        assert_eq!(owner.export()["workspaces"][0]["dock"], json!({ "pane": "p1", "edge": "left", "width": 0.4 }));

        let (mut again, _) = LayoutOwner::open(Some(file.clone()), false);
        again.handle(&json!({ "call": "attach", "panes": table(&["p1"]) }));
        assert_eq!(workspace(&again)["zoomed"], "p1");
        assert_eq!(command(&mut again, "zoom", json!({ "pane": "p1" }))["zoomed"], false);

        let (mut gone, problem) = LayoutOwner::open(Some(file), false);
        let reply = gone.handle(&json!({ "call": "attach", "panes": [] }));
        assert_eq!((problem, reply["problem"].clone()), (None, Value::Null), "{reply}");
        assert_eq!(gone.view()["workspaces"], json!([]));
        assert_eq!(owner.handle(&json!({ "call": "serve", "request": { "op": "place", "pane": { "id": "p2", "name": null, "type": "term" }, "at": { "dock": "top" } } }))["ok"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_planner_is_a_workspace_with_no_task_whose_opens_get_windows_of_their_own() {
        let dir = folder("planner");
        let file = StateFile::new(&dir.join("layout.json").to_string_lossy());
        let (mut owner, _) = LayoutOwner::open(Some(file.clone()), false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        let planned = serve_place(&mut owner, "p1", json!({ "planner": true, "app": "mac" }));
        assert_eq!((planned["window"].clone(), planned["workspace"].clone(), planned.get("dock").cloned()), (Value::Null, json!({ "id": "ws-1", "where": { "planner": "mac" } }), None), "{planned}");
        assert_eq!(owner.view()["workspaces"][0]["parkedAt"], Value::Null);
        assert_eq!(command(&mut owner, "task", json!({ "action": "list" }))["tasks"], json!([]), "a planner isn't a task");
        assert_eq!(command(&mut owner, "task", json!({ "action": "hold", "task": "p1" }))["error"]["code"], "not_found");
        assert_eq!(command(&mut owner, "task", json!({ "action": "claim", "task": "ws-1", "app": "mac" }))["error"]["code"], "not_found");
        assert_eq!(command(&mut owner, "move", json!({ "pane": "p1", "dock": "right" }))["error"]["code"], "not_a_task");

        let from = serve_place(&mut owner, "p2", json!({ "from": "p1" }));
        assert_eq!((from["window"].clone(), from.get("workspace").cloned()), (json!("win-1"), None), "{from}");
        let asked = serve_place(&mut owner, "p3", json!({ "workspace": "ws-1" }));
        assert_eq!(asked["window"], "win-2", "{asked}");
        assert_eq!(owner.view()["workspaces"][0]["tabs"], json!([]));
        owner.save().unwrap();

        let (mut again, _) = LayoutOwner::open(Some(file.clone()), false);
        assert_eq!(again.handle(&json!({ "call": "attach", "panes": table(&["p1", "p2", "p3"]) }))["placed"], json!([]));
        assert_eq!(again.view()["workspaces"][0]["where"], json!({ "planner": "mac" }));
        let (mut gone, _) = LayoutOwner::open(Some(file), false);
        gone.handle(&json!({ "call": "attach", "panes": table(&["p2", "p3"]) }));
        assert_eq!(gone.view()["workspaces"], json!([]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_window_is_assigned_to_a_task_or_freed_and_a_lead_or_a_planner_is_refused() {
        let (mut owner, _) = LayoutOwner::open(None, false);
        owner.handle(&json!({ "call": "attach", "panes": [] }));
        place(&mut owner, "p1", json!({ "dock": "left" }));
        let opened = serve_place(&mut owner, "p2", json!({ "window": "new", "opener": "p1" }));
        assert_eq!((opened["window"].clone(), opened["workspace"]["id"].clone()), (json!("win-1"), json!("ws-1")), "open --window new from a task's pane makes its window: {opened}");
        place(&mut owner, "p3", json!({ "window": "new" }));
        place(&mut owner, "p4", json!({ "lead": true, "window": "new" }));
        place(&mut owner, "p5", json!({ "planner": true }));
        let view = owner.view();
        assert_eq!(view["windows"].as_array().unwrap().iter().map(|w| w["workspace"].clone()).collect::<Vec<_>>(), [json!("ws-1"), Value::Null, Value::Null]);
        assert_eq!(view["workspaces"][1]["windows"], json!(["win-1"]));

        let window = |owner: &mut LayoutOwner, args: Value| command(owner, "window", args);
        let assigned = window(&mut owner, json!({ "action": "assign", "window": "win-2", "workspace": "p1" }));
        assert_eq!((assigned["window"].clone(), assigned["workspace"].clone()), (json!("win-2"), json!({ "id": "ws-1", "where": { "shelf": "mac" } })), "{assigned}");
        let freed = window(&mut owner, json!({ "action": "free", "window": "win-2" }));
        assert_eq!((freed["workspace"].clone(), freed["was"]["id"].clone()), (Value::Null, json!("ws-1")), "{freed}");
        assert_eq!(window(&mut owner, json!({ "action": "assign", "window": "win-3", "workspace": "ws-1" }))["error"]["code"], "lead");
        assert_eq!(window(&mut owner, json!({ "action": "assign", "window": "win-2", "workspace": "ws-2" }))["error"]["code"], "not_found", "a planner has no windows");
        let missing = window(&mut owner, json!({ "action": "assign", "window": "win-2", "workspace": "ws-9" }));
        assert_eq!((missing["error"]["code"].clone(), missing["error"]["message"].clone()), (json!("not_found"), json!("no workspace \"ws-9\"")));
        command(&mut owner, "task", json!({ "action": "hold", "task": "ws-1" }));
        assert_eq!(owner.view()["windows"][0]["workspace"], "ws-1", "a held task keeps its windows; each app decides what it shows");
        owner.handle(&json!({ "call": "serve", "request": { "op": "remove", "ids": ["p1"], "tidy": true } }));
        assert_eq!(owner.view()["windows"][0]["workspace"], Value::Null, "a task's window is freed when it goes");
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
        assert_eq!(ids(&view), [json!("win-2")], "a Desk window is the app's now");
        let workspaces: Vec<(Value, Value, Value)> = view["workspaces"].as_array().unwrap().iter().map(|s| (s["id"].clone(), s["dock"]["pane"].clone(), s["where"].clone())).collect();
        assert_eq!(workspaces, [(json!("ws-1"), json!("p1"), json!({ "shelf": "mac" })), (json!("ws-2"), json!("p4"), json!({ "shelf": "mac" })), (json!("ws-3"), json!("p6"), json!({ "planner": "mac" }))], "its lead is a planner");
        assert_eq!((view["workspaces"][0]["focused"].clone(), view["workspaces"][2]["zoomed"].clone()), (json!("p2"), json!("p6")));
        owner.save().unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], 3);
        assert_eq!(saved["workspaces"][2]["where"], json!({ "planner": "mac" }));
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
        assert_eq!((saved["workspaces"][0]["id"].clone(), saved["workspaces"][0].get("task").cloned(), saved["workspaces"][0]["where"].clone(), saved["workspaces"][0]["windows"][0]["id"].clone()), (json!("ws-1"), None, json!({ "shelf": "mac" }), json!("win-3")), "{saved}");
        assert_eq!((saved["workspaces"][1]["where"].clone(), saved["counters"]["workspace"].clone()), (json!({ "planner": "mac" }), json!(2)));
        assert_eq!(owner.view()["windows"][0]["workspace"], "ws-1", "its window follows");
        let _ = std::fs::remove_dir_all(&dir);
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
        assert_eq!(mv(&mut owner, json!({ "pane": "p2", "window": "win-2" }))["ok"], true);
        assert_eq!(tabs(&owner, 1), (vec![json!(["p3", "p4"]), json!(["p2"])], json!("tab-3"), json!("p2")), "a new tab at the end of the window, focused, not a split of its active tab");
        assert_eq!(tabs(&owner, 0).0, vec![json!(["p1"])], "the pane left behind keeps the whole tab");
        assert_eq!(owner.view()["key"], "win-2");
        let before = Value::Object(owner.view());
        assert_eq!(mv(&mut owner, json!({ "pane": "p2", "window": "win-2" }))["ok"], true);
        assert_eq!(Value::Object(owner.view()), before, "a pane alone in its tab, moved to its own window, stays as it is");
        assert_eq!(mv(&mut owner, json!({ "pane": "p4", "window": "win-2" }))["ok"], true);
        assert_eq!(tabs(&owner, 1), (vec![json!(["p3"]), json!(["p2"]), json!(["p4"])], json!("tab-4"), json!("p4")), "a pane in a split, moved to its own window, leaves it for a new tab");
        assert_eq!(mv(&mut owner, json!({ "pane": "p4", "tab": "tab-2" }))["ok"], true);
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
