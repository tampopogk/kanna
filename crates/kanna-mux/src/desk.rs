use crate::error::KmuxError;
use crate::json::{num, Map, Value};
use crate::layout::Edge;
use crate::model::{bad_request, not_found, reply, Dock, Model, Window, DOCK_WIDTH};
use crate::pane::Panes;
use crate::workspace::{Where, Workspace};
use std::time::{SystemTime, UNIX_EPOCH};

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64().floor())
}

impl Model {
    pub fn shelf(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces.iter().filter(|s| s.place == Where::Shelf)
    }

    pub fn held(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.workspaces.len()).filter(|&s| self.workspaces[s].place == Where::Backlog)
    }

    pub(crate) fn shelve(&mut self, pane: &str, edge: Edge, backlog: bool, frozen: bool) {
        let mut ws = self.new_workspace(Dock { pane: pane.into(), edge, width: DOCK_WIDTH, frozen }, if backlog { Where::Backlog } else { Where::Shelf });
        ws.parked_at = Some(now());
        ws.tabs.focused = Some(pane.into());
        self.workspaces.insert(0, ws);
    }

    pub(crate) fn task_panes(&mut self, task: &str) -> Result<Vec<String>, KmuxError> {
        let s = self.dock_ws(task).or_else(|| self.ws_index(task)).ok_or_else(|| not_found(format!("no task \"{task}\"")))?;
        if let Some(w) = self.workspaces[s].place.desk().and_then(|id| self.window_index(id)) {
            self.windows[w].desk = true;
        }
        let mut ids = self.workspace_panes(s);
        ids.rotate_right(1);
        Ok(ids)
    }

    pub(crate) fn close_task(&mut self, task: &str) {
        let Some(s) = self.dock_ws(task).or_else(|| self.ws_index(task)) else { return };
        let ws = self.workspaces.remove(s);
        self.windows.retain(|w| w.workspace.as_deref() != Some(ws.id.as_str()));
    }

    pub(crate) fn lead(&mut self, pane: &str, window: Option<&str>) -> Result<String, KmuxError> {
        let reference = window.ok_or_else(|| bad_request("a window's lead needs its window"))?;
        let w = self.target_window(Some(reference))?;
        let window = &mut self.windows[w];
        if let Some(lead) = &window.lead {
            return Err(KmuxError::new("led", format!("{} has a lead already: {}", window.id, lead.pane)));
        }
        window.lead = Some(Dock { pane: pane.into(), edge: Edge::Left, width: DOCK_WIDTH, frozen: false });
        window.workspace = None;
        let lead = window.lead.as_ref().map(|l| l.pane.clone());
        window.tabs.settle_focus(lead.as_ref());
        Ok(window.id.clone())
    }

    /// A Desk left with tabs but no task (its agent closed or undocked) becomes a plain window,
    /// or, when it has a lead to show beside the Shelf, hands its tabs to a window of their own.
    pub(crate) fn clear_shelves(&mut self) {
        for w in 0..self.windows.len() {
            if !self.shows_shelf(w) || self.windows[w].tabs.tabs.is_empty() {
                continue;
            }
            if self.windows[w].lead.is_none() {
                self.windows[w].desk = false;
                continue;
            }
            let tabs = std::mem::take(&mut self.windows[w].tabs.tabs);
            let (active, key) = (self.windows[w].tabs.active.take(), self.key.clone());
            let n = self.make_window();
            let window = &mut self.windows[n];
            window.tabs.count = tabs.len();
            window.tabs.tabs = tabs;
            window.tabs.active = active;
            window.tabs.settle_focus(None);
            let lead = self.windows[w].lead.as_ref().map(|l| l.pane.clone());
            self.windows[w].tabs.settle_focus(lead.as_ref());
            self.set_key(key);
        }
    }

    fn is_desk(&self, w: usize) -> bool {
        self.windows[w].desk
    }

    fn park_window(&mut self, w: usize) -> Option<String> {
        let s = self.showing(w)?;
        let mut ws = self.workspaces.remove(s);
        ws.place = Where::Shelf;
        ws.parked_at = Some(now());
        let task = ws.dock.pane.clone();
        self.workspaces.insert(0, ws);
        let lead = self.windows[w].lead.as_ref().map(|l| l.pane.clone());
        self.windows[w].tabs.settle_focus(lead.as_ref());
        Some(task)
    }

    fn task_window(&self, reference: &str, panes: &Panes) -> Result<(usize, Option<usize>), KmuxError> {
        let s = self.find_workspace(reference, panes).ok_or_else(|| match panes.need(Some(reference)) {
            Err(error) => error,
            Ok(_) => not_found(format!("{reference} is not in a task: it's in no task's workspace")),
        })?;
        match &self.workspaces[s].place {
            Where::Desk(window) => Ok((s, self.window_index(window))),
            Where::Shelf => Ok((s, None)),
            Where::Backlog => Err(not_found(format!("{reference} is not on a Desk or the Shelf"))),
        }
    }

    fn desk_on(&self, project: &str, panes: &Panes) -> Option<usize> {
        let root = crate::core::project(Some(project));
        let on = |w: usize| {
            let window = &self.windows[w];
            let mut folders = self.shows(w).map(|s| &s.dock).into_iter().chain(window.lead.iter()).filter_map(|d| panes.get(&d.pane)?.cwd.as_deref());
            let folder = folders.next().or(window.project.as_deref());
            folder.is_some() && crate::core::project(folder) == root
        };
        let rank = |w: &Window| match self.used.iter().position(|u| *u == w.id) {
            _ if self.key.as_ref() == Some(&w.id) => 0,
            Some(at) => at + 1,
            None => usize::MAX,
        };
        (0..self.windows.len()).filter(|&w| self.is_desk(w) && on(w)).min_by_key(|&w| rank(&self.windows[w]))
    }

    fn desk_for(&mut self, window: Option<&str>) -> Result<usize, KmuxError> {
        if let Some(reference) = window.filter(|r| *r != "new") {
            let w = self.window_index(reference).ok_or_else(|| not_found(format!("no window \"{reference}\"")))?;
            return if self.is_desk(w) { Ok(w) } else { Err(bad_request(format!("{reference} is not a Desk: it has no docked pane"))) };
        }
        if window.is_none() {
            if let Some(w) = self.key.clone().and_then(|k| self.window_index(&k)).filter(|w| self.is_desk(*w)) {
                return Ok(w);
            }
        }
        let w = self.make_window();
        self.windows[w].desk = true;
        Ok(w)
    }

    fn agent(&self, s: usize) -> String {
        self.workspaces[s].dock.pane.clone()
    }

    pub(crate) fn task(&mut self, args: &Value, panes: &Panes) -> Result<Map, KmuxError> {
        let string = |key: &str| args.get(key).and_then(Value::as_str);
        match string("action").unwrap_or("") {
            "list" => Ok(self.task_list(panes)),
            "park" => {
                let w = match string("task") {
                    Some(reference) => match self.task_window(reference, panes)? {
                        (s, None) => return Ok(reply([("task", self.agent(s).into()), ("window", Value::Null), ("parked", Value::Bool(false))])),
                        (_, Some(w)) => w,
                    },
                    None => match string("window") {
                        Some(reference) => self.window_index(reference).ok_or_else(|| not_found(format!("no window \"{reference}\"")))?,
                        None => self.key.clone().and_then(|k| self.window_index(&k)).ok_or_else(|| not_found("no window to park".into()))?,
                    },
                };
                let window = self.windows[w].id.clone();
                let task = self.park_window(w);
                if args.get("close").and_then(Value::as_bool) == Some(true) {
                    self.windows.remove(w);
                    if self.key.as_deref() == Some(window.as_str()) {
                        self.set_key(None);
                    }
                } else {
                    self.set_key(Some(window.clone()));
                }
                self.drop_empty();
                match task {
                    Some(task) => Ok(reply([("moves", moves(&[(&task, "desk", "shelf", &window)])), ("task", task.into()), ("window", window.into()), ("parked", Value::Bool(true))])),
                    None if args.get("close").and_then(Value::as_bool) == Some(true) => Ok(reply([("task", Value::Null), ("window", window.into()), ("parked", Value::Bool(false))])),
                    None => Err(not_found(format!("{window} has no task to park"))),
                }
            }
            "pull" => {
                let reference = string("task").ok_or_else(|| bad_request("pull needs a task"))?;
                let (s, at) = self.task_window(reference, panes)?;
                let task = self.agent(s);
                if let Some(w) = at {
                    let window = self.windows[w].id.clone();
                    self.set_key(Some(window.clone()));
                    return Ok(reply([("task", task.into()), ("window", window.into()), ("moved", Value::Bool(false))]));
                }
                let id = self.workspaces[s].id.clone();
                let w = self.desk_for(string("window"))?;
                let window = self.windows[w].id.clone();
                let swapped = self.park_window(w);
                let s = self.ws_index(&id).ok_or_else(|| not_found(format!("no task \"{task}\"")))?;
                self.workspaces[s].place = Where::Desk(window.clone());
                self.workspaces[s].parked_at = None;
                let dock = Some(self.agent(s));
                self.workspaces[s].tabs.settle_focus(dock.as_ref());
                self.set_key(Some(window.clone()));
                self.drop_empty();
                let mut moved = swapped.iter().map(|p| (p.as_str(), "desk", "shelf", window.as_str())).collect::<Vec<_>>();
                moved.push((&task, "shelf", "desk", &window));
                Ok(reply([("moves", moves(&moved)), ("task", task.clone().into()), ("window", window.into()), ("moved", Value::Bool(true))]))
            }
            "hold" => {
                let reference = string("task").ok_or_else(|| bad_request("hold needs a task"))?;
                let (s, at) = self.task_window(reference, panes)?;
                let id = self.workspaces[s].id.clone();
                if let Some(w) = at {
                    self.park_window(w);
                }
                let s = self.ws_index(&id).ok_or_else(|| not_found(format!("no task \"{reference}\"")))?;
                self.workspaces[s].place = Where::Backlog;
                let task = self.agent(s);
                self.drop_empty();
                Ok(reply([("task", task.into())]))
            }
            "release" => {
                let reference = string("task").ok_or_else(|| bad_request("release needs a task"))?;
                let held = self.find_workspace(reference, panes).filter(|&s| self.workspaces[s].place == Where::Backlog);
                let s = held.ok_or_else(|| not_found(format!("no task \"{reference}\" held in the backlog")))?;
                let mut ws = self.workspaces.remove(s);
                ws.place = Where::Shelf;
                ws.parked_at = Some(now());
                let task = ws.dock.pane.clone();
                self.workspaces.insert(0, ws);
                Ok(reply([("task", task.into())]))
            }
            "desk" => {
                let project = match string("project") {
                    Some(p) if p.starts_with('/') => Some(p.to_string()),
                    None => None,
                    Some(_) => return Err(bad_request("desk needs a project: an absolute path to a folder")),
                };
                let fresh = args.get("new").and_then(Value::as_bool) == Some(true);
                let found = project.as_deref().filter(|_| !fresh).and_then(|p| self.desk_on(p, panes));
                let w = match found {
                    Some(w) => {
                        let h = self.shown_holder(w);
                        if let Some(side) = self.side(h) {
                            self.tabs_mut(h).focused = Some(side);
                        }
                        w
                    }
                    None => {
                        let w = self.make_window();
                        self.windows[w].desk = true;
                        self.windows[w].project = project.clone();
                        w
                    }
                };
                let id = self.windows[w].id.clone();
                let project = match found {
                    Some(_) => project.as_deref().and_then(|p| crate::core::project(Some(p))).map(|root| root.to_string_lossy().into_owned()),
                    None => project,
                };
                self.set_key(Some(id.clone()));
                Ok(reply([("window", id.into()), ("project", project.map_or(Value::Null, Value::String)), ("new", Value::Bool(found.is_none()))]))
            }
            other => Err(bad_request(format!("unknown task action \"{other}\" (list, park, pull, hold, release or desk)"))),
        }
    }

    pub fn task_list(&self, panes: &Panes) -> Map {
        let entry = |ws: &Workspace, window: Option<&str>| {
            Value::Object(reply([
                ("id", ws.dock.pane.clone().into()),
                ("workspace", ws.id.clone().into()),
                ("name", panes.get(&ws.dock.pane).and_then(|p| p.name.clone()).map_or(Value::Null, Value::String)),
                ("window", window.map_or(Value::Null, |w| w.into())),
                ("parkedAt", ws.parked_at.map_or(Value::Null, num)),
                ("tabs", num(ws.tabs.tabs.len() as f64)),
            ]))
        };
        let mut tasks: Vec<Value> = vec![];
        for w in 0..self.windows.len() {
            if let Some(s) = self.showing(w) {
                tasks.push(entry(&self.workspaces[s], Some(&self.windows[w].id)));
            }
        }
        tasks.extend(self.shelf().map(|ws| entry(ws, None)));
        reply([("tasks", Value::Array(tasks))])
    }
}

fn moves(moved: &[(&str, &str, &str, &str)]) -> Value {
    Value::Array(moved.iter().map(|(task, from, to, window)| crate::json::json!({ "task": task, "from": from, "to": to, "window": window })).collect())
}
