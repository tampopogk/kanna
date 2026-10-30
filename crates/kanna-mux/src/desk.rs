use crate::error::KmuxError;
use crate::json::{num, Map, Value};
use crate::layout::Edge;
use crate::model::{bad_request, not_found, reply, Dock, Model, DOCK_WIDTH};
use crate::pane::Panes;
use crate::workspace::{Where, Workspace};
use std::time::{SystemTime, UNIX_EPOCH};

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64().floor())
}

impl Model {
    pub fn held(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.workspaces.len()).filter(|&s| self.workspaces[s].place == Where::Backlog)
    }

    pub(crate) fn shelve(&mut self, pane: &str, edge: Edge, place: Where, frozen: bool) -> usize {
        let mut ws = self.new_workspace(Dock { pane: pane.into(), edge, width: DOCK_WIDTH, frozen }, place);
        ws.parked_at = Some(now());
        ws.tabs.focused = Some(pane.into());
        self.workspaces.insert(0, ws);
        0
    }

    pub(crate) fn task_panes(&mut self, task: &str) -> Result<Vec<String>, KmuxError> {
        let s = self.dock_ws(task).or_else(|| self.ws_index(task)).ok_or_else(|| not_found(format!("no task \"{task}\"")))?;
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

    fn task_workspace(&self, reference: &str, panes: &Panes) -> Result<usize, KmuxError> {
        let s = self.find_workspace(reference, panes).ok_or_else(|| match panes.need(Some(reference)) {
            Err(error) => error,
            Ok(_) => not_found(format!("{reference} is not in a task: it's in no task's workspace")),
        })?;
        if self.workspaces[s].is_task() { Ok(s) } else { Err(not_found(format!("{reference} is a Desk's planner, not a task"))) }
    }

    fn agent(&self, s: usize) -> String {
        self.workspaces[s].dock.pane.clone()
    }

    pub(crate) fn task(&mut self, args: &Value, panes: &Panes) -> Result<Map, KmuxError> {
        let string = |key: &str| args.get(key).and_then(Value::as_str);
        match string("action").unwrap_or("") {
            "list" => Ok(self.task_list(string("app"), panes)),
            "hold" => {
                let reference = string("task").ok_or_else(|| bad_request("hold needs a task"))?;
                let s = self.task_workspace(reference, panes)?;
                let was = self.workspaces[s].place.to_json();
                self.workspaces[s].place = Where::Backlog;
                Ok(reply([("task", self.agent(s).into()), ("was", was)]))
            }
            "find" => {
                let reference = string("task").ok_or_else(|| bad_request("find needs a task"))?;
                let s = self.task_workspace(reference, panes)?;
                Ok(reply([("task", self.agent(s).into()), ("workspace", self.workspaces[s].id.clone().into()), ("where", self.workspaces[s].place.to_json())]))
            }
            "claim" => {
                let reference = string("task").ok_or_else(|| bad_request("claim needs a task"))?;
                let app = string("app").ok_or_else(|| bad_request("claim needs the app that has it out"))?;
                let s = self.task_workspace(reference, panes)?;
                let was = self.workspaces[s].place.to_json();
                if self.workspaces[s].place != Where::Shelf(app.into()) {
                    let mut ws = self.workspaces.remove(s);
                    ws.place = Where::Shelf(app.into());
                    ws.parked_at = Some(now());
                    self.workspaces.insert(0, ws);
                }
                Ok(reply([("task", self.agent(0).into()), ("workspace", self.workspaces[0].id.clone().into()), ("was", was)]))
            }
            other => Err(bad_request(format!("unknown task action \"{other}\" (list, find, hold or claim)"))),
        }
    }

    pub fn task_list(&self, app: Option<&str>, panes: &Panes) -> Map {
        let entry = |ws: &Workspace| {
            Value::Object(reply([
                ("id", ws.dock.pane.clone().into()),
                ("workspace", ws.id.clone().into()),
                ("name", panes.get(&ws.dock.pane).and_then(|p| p.name.clone()).map_or(Value::Null, Value::String)),
                ("agent", panes.get(&ws.dock.pane).and_then(|p| p.program()).map_or(Value::Null, Value::String)),
                ("where", ws.place.to_json()),
                ("leftAt", ws.parked_at.map_or(Value::Null, num)),
                ("tabs", num(ws.tabs.tabs.len() as f64)),
            ]))
        };
        let out = |ws: &&Workspace| matches!(&ws.place, Where::Shelf(by) if app.is_none_or(|app| app == by));
        reply([("tasks", Value::Array(self.workspaces.iter().filter(out).map(entry).collect()))])
    }
}
