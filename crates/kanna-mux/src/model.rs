use crate::error::KmuxError;
use crate::fraction;
use crate::json::{opt, string, trim, Map, Value};
use crate::layout::{Edge, LayoutHost, Placement, SavedLayout, Scope, SplitSide, Spot};
use crate::pane::Panes;
pub use crate::tabs::{Tab, Tabs};
use crate::workspace::{Where, Workspace, MAC_APP};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Row,
    Column,
}

impl Axis {
    pub fn as_str(self) -> &'static str {
        match self {
            Axis::Row => "row",
            Axis::Column => "column",
        }
    }

    pub fn parse(name: &str) -> Option<Axis> {
        match name {
            "row" => Some(Axis::Row),
            "column" => Some(Axis::Column),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Child {
    pub node: Node,
    pub size: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    pub axis: Axis,
    pub kids: Vec<Child>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Pane(String),
    Split(Split),
}

pub const DOCK_WIDTH: f64 = 2.0 / 5.0;
const WINDOW_ASPECT: f64 = 1.6;

#[derive(Clone, Debug, PartialEq)]
pub struct Dock {
    pub pane: String,
    pub edge: Edge,
    pub width: f64,
    pub frozen: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub id: String,
    pub tabs: Tabs,
    pub lead: Option<Dock>,
    pub workspace: Option<String>,
}

impl Window {
    pub(crate) fn new(id: String) -> Window {
        Window { id, tabs: Tabs::default(), lead: None, workspace: None }
    }

    pub fn leads(&self, pane: &str) -> bool {
        self.lead.as_ref().is_some_and(|p| p.pane == pane)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holder {
    Window(usize),
    Workspace(usize),
}

#[derive(Clone, Debug)]
pub struct Location {
    pub holder: Holder,
    pub tab: usize,
    pub parent: Option<Vec<usize>>,
    pub index: usize,
}

pub struct Model {
    pub(crate) windows: Vec<Window>,
    pub(crate) workspaces: Vec<Workspace>,
    pub(crate) counters: (usize, usize, usize),
    pub(crate) orphans: Vec<Tabs>,
}

impl Default for Model {
    fn default() -> Model {
        Model { windows: vec![], workspaces: vec![], counters: (0, 0, 0), orphans: vec![] }
    }
}

pub fn pane_ids(node: Option<&Node>) -> Vec<String> {
    fn collect(node: &Node, out: &mut Vec<String>) {
        match node {
            Node::Pane(id) => out.push(id.clone()),
            Node::Split(split) => split.kids.iter().for_each(|kid| collect(&kid.node, out)),
        }
    }
    let mut out = vec![];
    if let Some(node) = node {
        collect(node, &mut out);
    }
    out
}

pub(crate) fn split_at<'a>(root: &'a mut Node, path: &[usize]) -> &'a mut Split {
    let mut node = root;
    for &i in path {
        node = match node {
            Node::Split(split) => &mut split.kids[i].node,
            Node::Pane(_) => unreachable!("a path goes through splits"),
        };
    }
    match node {
        Node::Split(split) => split,
        Node::Pane(_) => unreachable!("a path ends at a split"),
    }
}

pub fn normalize(node: Node) -> Node {
    match node {
        Node::Split(mut split) => {
            split.kids = split.kids.into_iter().map(|kid| Child { node: normalize(kid.node), size: kid.size }).collect();
            if split.kids.len() == 1 { split.kids.pop().unwrap().node } else { Node::Split(split) }
        }
        pane => pane,
    }
}

pub(crate) fn fraction(value: &Value) -> Result<f64, KmuxError> {
    fraction::parse(value).ok_or_else(|| KmuxError::new("bad_request", format!("\"{}\" is not a fraction", describe(value))))
}

fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => format!("bool({b})"),
        Value::Number(n) => format!("number({})", swift_double(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => format!("string({})", swift_quoted(s)),
        Value::Array(items) => format!("array([{}])", items.iter().map(|item| format!("KmuxCore.JSON.{}", describe(item))).collect::<Vec<_>>().join(", ")),
        Value::Object(fields) if fields.is_empty() => "object([:])".into(),
        Value::Object(fields) => format!(
            "object([{}])",
            fields.iter().map(|(key, item)| format!("{}: KmuxCore.JSON.{}", swift_quoted(key), describe(item))).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn swift_double(value: f64) -> String {
    let size = value.abs();
    if !value.is_finite() || size == 0.0 || (1e-4..=9007199254740992.0).contains(&size) {
        return format!("{value:?}");
    }
    let text = format!("{value:e}");
    let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
    let (sign, digits) = match exponent.strip_prefix('-') {
        Some(digits) => ('-', digits),
        None => ('+', exponent),
    };
    format!("{mantissa}e{sign}{digits:0>2}")
}

fn swift_quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if c.is_ascii_control() => out.push_str(&format!("\\u{{{:X}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn not_found(message: String) -> KmuxError {
    KmuxError::new("not_found", message)
}

pub(crate) fn bad_request(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

fn layout_invalid(message: impl Into<String>) -> KmuxError {
    KmuxError::new("layout_invalid", message)
}

fn docked(message: impl std::fmt::Display) -> KmuxError {
    KmuxError::new("docked", format!("docked: {message}"))
}

fn frozen(message: impl std::fmt::Display) -> KmuxError {
    KmuxError::new("frozen", format!("frozen: {message}"))
}

pub(crate) fn reply<const N: usize>(fields: [(&str, Value); N]) -> Map {
    fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn number(id: &str) -> usize {
    id.strip_prefix("win-").and_then(|n| n.parse().ok()).unwrap_or(usize::MAX)
}

impl Model {
    pub fn new() -> Model {
        Model::default()
    }

    pub fn retitle(&mut self, panes: &Panes) -> bool {
        let mut changed = false;
        let tabs = self.windows.iter_mut().map(|w| &mut w.tabs).chain(self.workspaces.iter_mut().map(|s| &mut s.tabs));
        for tab in tabs.flat_map(|t| t.tabs.iter_mut()) {
            changed |= tab.retitle(panes);
        }
        changed
    }

    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    pub fn window(&self, id: &str) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn workspace(&self, id: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|s| s.id == id)
    }

    pub(crate) fn window_index(&self, id: &str) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }

    pub(crate) fn ws_index(&self, id: &str) -> Option<usize> {
        self.workspaces.iter().position(|s| s.id == id)
    }

    pub fn sorted_windows(&self) -> Vec<usize> {
        let mut all: Vec<usize> = (0..self.windows.len()).collect();
        all.sort_by_key(|&w| number(&self.windows[w].id));
        all
    }

    pub fn tabs(&self, h: Holder) -> &Tabs {
        match h {
            Holder::Window(w) => &self.windows[w].tabs,
            Holder::Workspace(s) => &self.workspaces[s].tabs,
        }
    }

    pub(crate) fn tabs_mut(&mut self, h: Holder) -> &mut Tabs {
        match h {
            Holder::Window(w) => &mut self.windows[w].tabs,
            Holder::Workspace(s) => &mut self.workspaces[s].tabs,
        }
    }

    fn holders(&self) -> Vec<Holder> {
        (0..self.windows.len()).map(Holder::Window).chain((0..self.workspaces.len()).map(Holder::Workspace)).collect()
    }

    pub fn locate(&self, id: &str) -> Option<Location> {
        self.holders().into_iter().find_map(|holder| self.tabs(holder).locate(id).map(|(tab, parent, index)| Location { holder, tab, parent, index }))
    }

    pub(crate) fn tab_index(&self, id: &str) -> Option<(Holder, usize)> {
        self.holders().into_iter().find_map(|h| self.tabs(h).position(id).map(|t| (h, t)))
    }

    pub fn tab(&self, id: &str) -> Option<&Tab> {
        self.tab_index(id).map(|(h, t)| &self.tabs(h).tabs[t])
    }

    pub(crate) fn dock_ws(&self, pane: &str) -> Option<usize> {
        self.workspaces.iter().position(|s| s.dock.pane == pane)
    }

    pub fn lead_window(&self, id: &str) -> Option<usize> {
        self.windows.iter().position(|w| w.leads(id))
    }

    pub(crate) fn holder_of(&self, id: &str) -> Option<Holder> {
        self.locate(id).map(|l| l.holder).or_else(|| self.dock_ws(id).map(Holder::Workspace)).or_else(|| self.lead_window(id).map(Holder::Window))
    }

    pub(crate) fn ws_of(&self, h: Holder) -> Option<usize> {
        match h {
            Holder::Workspace(s) => Some(s),
            Holder::Window(w) => self.windows[w].workspace.as_deref().and_then(|id| self.ws_index(id)),
        }
    }

    pub(crate) fn window_of(&self, h: Holder) -> Option<usize> {
        match h {
            Holder::Window(w) => Some(w),
            Holder::Workspace(_) => None,
        }
    }

    pub(crate) fn side(&self, h: Holder) -> Option<String> {
        match h {
            Holder::Window(w) => self.windows[w].lead.as_ref().map(|l| l.pane.clone()),
            Holder::Workspace(s) => Some(self.workspaces[s].dock.pane.clone()),
        }
    }

    pub fn placed(&self) -> Vec<String> {
        let windows = self.windows.iter().flat_map(|w| w.tabs.panes().into_iter().chain(w.lead.iter().map(|l| l.pane.clone())));
        windows.chain(self.workspaces.iter().flat_map(|s| std::iter::once(s.dock.pane.clone()).chain(s.tabs.panes()))).collect()
    }

    pub fn workspace_panes(&self, s: usize) -> Vec<String> {
        let ws = &self.workspaces[s];
        let own = self.windows.iter().filter(|w| w.workspace.as_deref() == Some(ws.id.as_str())).flat_map(|w| w.tabs.panes());
        ws.tabs.panes().into_iter().chain(own).chain(std::iter::once(ws.dock.pane.clone())).collect()
    }

    pub fn workspace_windows(&self, id: &str) -> Vec<&Window> {
        self.windows.iter().filter(|w| w.workspace.as_deref() == Some(id)).collect()
    }

    pub fn rename_pane(&mut self, from: &str, to: &str) {
        for window in &mut self.windows {
            window.tabs.rename(from, to);
            if let Some(lead) = window.lead.as_mut().filter(|lead| lead.pane == from) {
                lead.pane = to.into();
            }
        }
        for ws in &mut self.workspaces {
            ws.tabs.rename(from, to);
            if ws.dock.pane == from {
                ws.dock.pane = to.into();
            }
        }
        for tabs in &mut self.orphans {
            tabs.rename(from, to);
        }
    }

    fn wide(&self, h: Holder, t: usize, pane: &str) -> bool {
        fn find(node: &Node, pane: &str, w: f64, h: f64) -> Option<(f64, f64)> {
            match node {
                Node::Pane(id) => (id == pane).then_some((w, h)),
                Node::Split(split) => split.kids.iter().find_map(|kid| match split.axis {
                    Axis::Row => find(&kid.node, pane, w * kid.size, h),
                    Axis::Column => find(&kid.node, pane, w, h * kid.size),
                }),
            }
        }
        let width = if self.side(h).is_some() { WINDOW_ASPECT * (1.0 - DOCK_WIDTH) } else { WINDOW_ASPECT };
        self.tabs(h).tabs[t].root.as_ref().and_then(|root| find(root, pane, width, 1.0)).is_none_or(|(w, h)| w >= h)
    }

    pub(crate) fn new_workspace(&mut self, dock: Dock, place: Where) -> Workspace {
        self.counters.2 += 1;
        Workspace::new(format!("ws-{}", self.counters.2), dock, place)
    }

    pub(crate) fn make_window(&mut self) -> usize {
        self.counters.1 += 1;
        self.windows.push(Window::new(format!("win-{}", self.counters.1)));
        self.windows.len() - 1
    }

    fn window_for(&mut self, ws: Option<usize>) -> usize {
        let workspace = ws.filter(|&s| self.workspaces[s].is_task()).map(|s| self.workspaces[s].id.clone());
        let w = self.make_window();
        self.windows[w].workspace = workspace;
        w
    }

    pub(crate) fn make_tab(&mut self, h: Holder, activate: bool) -> usize {
        self.counters.0 += 1;
        let id = format!("tab-{}", self.counters.0);
        let tabs = self.tabs_mut(h);
        tabs.count += 1;
        let tab = Tab::new(id, tabs.count);
        if activate || tabs.active.is_none() {
            tabs.active = Some(tab.id.clone());
        }
        tabs.tabs.push(tab);
        tabs.tabs.len() - 1
    }

    pub(crate) fn frozen_dock(&self, pane: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|s| s.dock.pane == pane && s.dock.frozen)
    }

    fn refuse_frozen_pane(&self, pane: &str, panes: &Panes) -> Result<(), KmuxError> {
        match self.frozen_dock(pane) {
            Some(ws) => Err(frozen(format!("{} is frozen in {}'s dock; nothing drops on it", panes.get(pane).map_or_else(|| pane.to_string(), |p| p.label()), ws.id))),
            None => Ok(()),
        }
    }

    fn dock_view(&self, s: usize, panes: &Panes) -> Value {
        let dock = &self.workspaces[s].dock;
        let mut view = reply([("pane", panes.display(&dock.pane).into()), ("edge", dock.edge.as_str().into()), ("width", fraction::format(dock.width).into())]);
        if dock.frozen {
            view.insert("frozen".into(), Value::Bool(true));
        }
        Value::Object(view)
    }

    pub(crate) fn detach(&mut self, id: &str) {
        if let Some(w) = self.lead_window(id) {
            self.windows[w].lead = None;
            return;
        }
        if let Some(s) = self.dock_ws(id) {
            self.dissolve(s);
            return;
        }
        if let Some(h) = self.locate(id).map(|l| l.holder) {
            self.tabs_mut(h).remove(id);
            return;
        }
        for tabs in &mut self.orphans {
            if tabs.remove(id) {
                return;
            }
        }
    }

    fn dissolve(&mut self, s: usize) {
        let ws = self.workspaces.remove(s);
        for window in &mut self.windows {
            if window.workspace.as_deref() == Some(ws.id.as_str()) {
                window.workspace = None;
            }
        }
        self.orphans.push(ws.tabs);
    }

    fn swap(&mut self, a: &str, b: &str) {
        let (la, lb, da, db) = (self.locate(a), self.locate(b), self.dock_ws(a), self.dock_ws(b));
        if (la.is_none() && da.is_none()) || (lb.is_none() && db.is_none()) {
            return;
        }
        let put = |model: &mut Model, location: Option<Location>, dock: Option<usize>, id: &str| match (location, dock) {
            (Some(l), _) => model.tabs_mut(l.holder).set_at(l.tab, l.parent.as_deref(), l.index, id),
            (None, Some(s)) => model.workspaces[s].dock.pane = id.into(),
            (None, None) => {}
        };
        put(self, la, da, b);
        put(self, lb, db, a);
    }

    pub(crate) fn drop_empty(&mut self) {
        let gone: Vec<usize> = (0..self.windows.len()).filter(|&w| self.windows[w].workspace.as_deref().is_some_and(|id| self.ws_index(id).is_none())).collect();
        for w in gone {
            self.windows[w].workspace = None;
        }
        for h in self.holders() {
            let side = self.side(h);
            let tabs = self.tabs_mut(h);
            tabs.tidy();
            tabs.settle_focus(side.as_ref());
        }
        self.windows.retain(|w| !w.tabs.tabs.is_empty() || w.lead.is_some());
        for mut tabs in std::mem::take(&mut self.orphans) {
            tabs.tidy();
            if tabs.tabs.is_empty() {
                continue;
            }
            let w = self.make_window();
            self.windows[w].tabs = Tabs { count: tabs.tabs.len(), zoom: false, ..tabs };
            self.windows[w].tabs.settle_focus(None);
        }
    }

    pub(crate) fn target_window(&mut self, reference: Option<&str>) -> Result<usize, KmuxError> {
        match reference {
            None => match self.windows.len() {
                0 => Ok(self.make_window()),
                1 => Ok(0),
                _ => Err(bad_request("there are several windows: say which with --window WINDOW (or --window new)")),
            },
            Some("new") => Ok(self.make_window()),
            Some(id) => self.window_index(id).ok_or_else(|| not_found(format!("no window \"{id}\""))),
        }
    }

    fn workspace_holder(&mut self, reference: &str) -> Result<Holder, KmuxError> {
        let s = self.ws_index(reference).ok_or_else(|| not_found(format!("no workspace \"{reference}\"")))?;
        Ok(if self.workspaces[s].is_task() { Holder::Workspace(s) } else { Holder::Window(self.make_window()) })
    }

    fn need_tab(&self, reference: Option<&str>) -> Result<(Holder, usize), KmuxError> {
        reference.and_then(|r| self.tab_index(r)).ok_or_else(|| not_found(format!("no tab \"{}\"", reference.unwrap_or(""))))
    }

    pub(crate) fn focus_pane(&mut self, id: &str) -> Option<Holder> {
        let h = self.holder_of(id)?;
        self.tabs_mut(h).focus(id);
        Some(h)
    }

    pub(crate) fn spot_of(&self, h: Holder) -> (Option<String>, Value) {
        let window = self.window_of(h).map(|w| self.windows[w].id.clone());
        let workspace = self.ws_of(h).map_or(Value::Null, |s| self.workspaces[s].summary());
        (window, workspace)
    }

    fn at(&self, h: Holder) -> Map {
        let (window, workspace) = self.spot_of(h);
        let mut out = reply([("window", opt(window.as_deref()))]);
        if !workspace.is_null() {
            out.insert("workspace".into(), workspace);
        }
        out
    }

    pub fn tree(&self, node: Option<&Node>, size: Option<f64>, panes: &Panes) -> Value {
        let Some(node) = node else { return Value::Null };
        let mut out = Map::new();
        match node {
            Node::Pane(id) => {
                out.insert("pane".into(), panes.display(id).into());
            }
            Node::Split(split) => {
                out.insert("split".into(), split.axis.as_str().into());
                out.insert("children".into(), Value::Array(split.kids.iter().map(|kid| self.tree(Some(&kid.node), Some(kid.size), panes)).collect()));
            }
        }
        if let Some(size) = size {
            if size < 1.0 - 1e-6 {
                out.insert("size".into(), fraction::format(size).into());
            }
        }
        Value::Object(out)
    }

    fn tab_tree(&self, tab: &str, panes: &Panes) -> Value {
        self.tab(tab).map_or(Value::Null, |t| self.tree(t.root.as_ref(), None, panes))
    }

    fn focus(&mut self, args: &Value, panes: &Panes) -> Result<Map, KmuxError> {
        if let Some(reference) = string(args, "pane") {
            let id = panes.need(Some(reference))?.id.clone();
            return match self.focus_pane(&id) {
                Some(h) => Ok(self.at(h)),
                None => Ok(reply([("window", Value::Null)])),
            };
        }
        if let Some(reference) = string(args, "tab") {
            let (h, t) = self.tab_index(reference).ok_or_else(|| not_found(format!("no tab \"{reference}\"")))?;
            let side = self.side(h);
            let tabs = self.tabs_mut(h);
            if string(args, "keep") == Some("dock") && tabs.focused.is_some() && tabs.focused == side {
                tabs.active = Some(tabs.tabs[t].id.clone());
            } else {
                tabs.show(t);
            }
            return Ok(self.at(h));
        }
        if args.get("window").is_some() {
            let w = self.target_window(string(args, "window"))?;
            return Ok(self.at(Holder::Window(w)));
        }
        Err(bad_request("focus needs a pane, tab or window"))
    }

    fn zoom(&mut self, args: &Value, panes: &Panes) -> Result<Map, KmuxError> {
        let id = panes.need(string(args, "pane"))?.id.clone();
        let h = self.holder_of(&id).ok_or_else(|| not_found(format!("no pane \"{id}\"")))?;
        let was = self.tabs(h).zoomed() == Some(&id);
        self.focus_pane(&id);
        self.tabs_mut(h).zoom = !was;
        let mut out = self.at(h);
        out.insert("zoomed".into(), Value::Bool(!was));
        Ok(out)
    }

    fn rename_tab(&mut self, args: &Value) -> Result<Map, KmuxError> {
        let reference = string(args, "tab").unwrap_or("");
        let (h, t) = self.tab_index(reference).ok_or_else(|| not_found(format!("no tab \"{reference}\"")))?;
        let title = trim(string(args, "title").unwrap_or(""));
        if title.is_empty() {
            return Err(bad_request("a tab title cannot be empty"));
        }
        let tab = &mut self.tabs_mut(h).tabs[t];
        tab.rename(title);
        Ok(reply([("tab", tab.id.clone().into()), ("title", title.into())]))
    }

    fn moved_reply(&self, id: &str, panes: &Panes) -> Map {
        if let Some(s) = self.dock_ws(id) {
            let mut out = self.at(Holder::Workspace(s));
            out.insert("dock".into(), self.dock_view(s, panes));
            return out;
        }
        let Some(location) = self.locate(id) else { return Map::new() };
        let tab = &self.tabs(location.holder).tabs[location.tab];
        let mut out = self.at(location.holder);
        out.insert("tab".into(), tab.id.clone().into());
        out.insert("layout".into(), self.tree(tab.root.as_ref(), None, panes));
        out
    }

    fn move_pane(&mut self, args: &Value, panes: &Panes) -> Result<Map, KmuxError> {
        let id = panes.need(string(args, "pane"))?.id.clone();
        if self.lead_window(&id).is_some() {
            return Err(KmuxError::new("lead", format!("{} is its window's lead, which stays where it is", panes.display(&id))));
        }
        if self.frozen_dock(&id).is_some() {
            return Err(frozen(format!("{} is frozen in its dock and doesn't move", panes.get(&id).map_or_else(|| id.clone(), |p| p.label()))));
        }
        let from = self.holder_of(&id).ok_or_else(|| not_found(format!("no pane \"{id}\"")))?;
        let unmoved = self.at(from);
        let from_tab = self.locate(&id).map(|l| self.tabs(l.holder).tabs[l.tab].id.clone());
        if let Some(edge) = Edge::read(args.get("dock"))? {
            if args.get("to").is_some() || args.get("tab").is_some() || args.get("window").is_some() {
                return Err(bad_request("move --dock takes no to, tab or window: a task's agent stays in its workspace"));
            }
            let Some(s) = self.dock_ws(&id).filter(|&s| self.workspaces[s].is_task()) else {
                return Err(KmuxError::new("not_a_task", format!("{} isn't a task, and only a task's agent docks: start one with `kanna task new` and `kanna task start`", panes.display(&id))));
            };
            self.workspaces[s].dock.edge = edge;
            self.focus_pane(&id);
            return Ok(self.moved_reply(&id, panes));
        }
        if let Some(reference) = string(args, "to") {
            let target = panes.need(Some(reference))?;
            let (target, label) = (target.id.clone(), target.label());
            let side = string(args, "side").unwrap_or("swap");
            if !["left", "right", "top", "bottom", "swap"].contains(&side) {
                return Err(bad_request("side must be left, right, top, bottom or swap"));
            }
            if target == id {
                return Ok(unmoved);
            }
            self.refuse_frozen_pane(&target, panes)?;
            if self.lead_window(&target).is_some() {
                return Err(KmuxError::new("lead", format!("{label} is its window's lead; nothing drops on it")));
            }
            if side != "swap" && self.dock_ws(&target).is_some() {
                return Err(docked(format!("{label} is docked and holds one pane; drop on its middle to swap")));
            }
            if side == "swap" {
                self.swap(&id, &target);
                let tabs = self.tabs_mut(from);
                if tabs.focused.as_deref() == Some(&id) {
                    tabs.focused = Some(target);
                }
            } else {
                self.detach(&id);
                let location = self.locate(&target).ok_or_else(|| not_found(format!("no pane \"{reference}\"")))?;
                let axis = if side == "left" || side == "right" { Axis::Row } else { Axis::Column };
                self.tabs_mut(location.holder).insert(Node::Pane(id.clone()), &target, location.tab, axis, side == "right" || side == "bottom", 0.5);
            }
        } else {
            let tab = string(args, "tab");
            let destination = if let Some(reference) = tab.filter(|r| *r != "new") {
                let (h, t) = self.need_tab(Some(reference))?;
                self.tabs(h).tabs[t].id.clone()
            } else if tab == Some("new") || args.get("window").is_some() || args.get("workspace").is_some() {
                let h = match string(args, "window").filter(|_| args.get("window").is_some()) {
                    None if args.get("workspace").is_some() => self.workspace_holder(string(args, "workspace").unwrap_or_default())?,
                    Some("new") => Holder::Window(self.window_for(self.ws_of(from))),
                    Some(window) => Holder::Window(self.target_window(Some(window))?),
                    None if self.dock_ws(&id).is_some() => {
                        self.detach(&id);
                        Holder::Window(self.make_window())
                    }
                    None => from,
                };
                let alone = self.locate(&id).is_some_and(|l| l.holder == h && l.parent.is_none());
                if tab.is_none() && alone {
                    return Ok(unmoved);
                }
                let t = self.make_tab(h, true);
                self.tabs(h).tabs[t].id.clone()
            } else {
                return Err(bad_request("move needs to + side, tab, or window"));
            };
            if Some(&destination) == from_tab.as_ref() {
                return Ok(unmoved);
            }
            self.detach(&id);
            let (h, t) = self.tab_index(&destination).unwrap();
            self.tabs_mut(h).append(&id, t);
        }
        self.focus_pane(&id);
        self.drop_empty();
        Ok(self.moved_reply(&id, panes))
    }

    fn move_tab(&mut self, args: &Value) -> Result<Map, KmuxError> {
        let (source, t) = self.need_tab(string(args, "tab"))?;
        let tab = self.tabs(source).tabs[t].id.clone();
        let destination = match args.get("window") {
            None if args.get("workspace").is_some() => self.workspace_holder(string(args, "workspace").unwrap_or_default())?,
            Some(window) if window.as_str() == Some("new") => Holder::Window(self.window_for(self.ws_of(source))),
            Some(window) => Holder::Window(self.target_window(window.as_str())?),
            None => source,
        };
        let mut index = None;
        if let Some(value) = args.get("index") {
            match value.as_f64() {
                Some(n) if n >= 0.0 && n.round() == n => index = Some(n as usize),
                _ => return Err(bad_request("index must be a whole number ≥ 0")),
            }
        }
        let moving = self.tabs_mut(source).tabs.remove(t);
        let mut index = index.unwrap_or(self.tabs(destination).tabs.len() + usize::from(source == destination));
        if source == destination && index > t {
            index -= 1;
        }
        let tabs = self.tabs_mut(destination);
        tabs.tabs.insert(index.min(tabs.tabs.len()), moving);
        let d = tabs.position(&tab).unwrap();
        tabs.show(d);
        self.drop_empty();
        let (h, _) = self.tab_index(&tab).unwrap();
        let tabs = self.tabs(h).tabs.iter().map(|t| Value::String(t.id.clone())).collect();
        let mut out = self.at(h);
        out.insert("tabs".into(), Value::Array(tabs));
        Ok(out)
    }

    fn set_divider(&mut self, tab: &str, path: &[usize], index: usize, share: f64) -> Result<(), KmuxError> {
        let (h, t) = self.tab_index(tab).ok_or_else(|| not_found(format!("no tab \"{tab}\"")))?;
        let mut node = self.tabs_mut(h).tabs[t].root.as_mut().ok_or_else(|| not_found(format!("tab \"{tab}\" is empty")))?;
        for &i in path {
            node = match node {
                Node::Split(split) if i < split.kids.len() => &mut split.kids[i].node,
                _ => return Err(bad_request("no split at that path")),
            };
        }
        let Node::Split(split) = node else { return Err(bad_request("no split at that path")) };
        if index + 1 >= split.kids.len() {
            return Err(bad_request("no divider at that index"));
        }
        let pair = split.kids[index].size + split.kids[index + 1].size;
        if !(share > 0.0 && share < pair) {
            return Err(bad_request("the share must leave both panes some space"));
        }
        split.kids[index].size = share;
        split.kids[index + 1].size = pair - share;
        Ok(())
    }

    fn resize(&mut self, args: &Value, panes: &Panes) -> Result<Map, KmuxError> {
        if let Some(tab) = string(args, "tab") {
            let path: Vec<usize> = args.get("path").and_then(Value::as_array).map_or(vec![], |p| p.iter().filter_map(Value::as_u64).map(|i| i as usize).collect());
            let index = args.get("index").and_then(Value::as_u64).ok_or_else(|| bad_request("a divider needs its index"))? as usize;
            let share = args.get("share").and_then(Value::as_f64).ok_or_else(|| bad_request("a divider needs a share"))?;
            self.set_divider(tab, &path, index, share)?;
            let root = self.tab(tab).and_then(|t| t.root.as_ref());
            return Ok(reply([("tab", tab.into()), ("layout", self.tree(root, None, panes))]));
        }
        let pane = panes.need(string(args, "pane"))?;
        let size = fraction(args.get("size").unwrap_or(&Value::Null))?;
        if !(size > 0.0 && size < 1.0) {
            return Err(bad_request("size must be a fraction between 0 and 1"));
        }
        if let Some(s) = self.dock_ws(&pane.id) {
            self.workspaces[s].dock.width = size;
            let mut out = self.at(Holder::Workspace(s));
            out.insert("dock".into(), self.dock_view(s, panes));
            return Ok(out);
        }
        if let Some(w) = self.lead_window(&pane.id) {
            if let Some(lead) = self.windows[w].lead.as_mut() {
                lead.width = size;
            }
            return Ok(reply([("window", self.windows[w].id.clone().into()), ("width", fraction::format(size).into())]));
        }
        let location = self.locate(&pane.id).ok_or_else(|| not_found(format!("no pane \"{}\"", pane.id)))?;
        let Some(path) = &location.parent else {
            return Err(bad_request(format!("{} fills its tab, so there is nothing to resize against", pane.label())));
        };
        let tab = &mut self.tabs_mut(location.holder).tabs[location.tab];
        let parent = split_at(tab.root.as_mut().unwrap(), path);
        let old = parent.kids[location.index].size;
        let others = (parent.kids.len() - 1) as f64;
        for (i, kid) in parent.kids.iter_mut().enumerate() {
            if i != location.index {
                kid.size = if old < 1.0 - 1e-9 { kid.size * (1.0 - size) / (1.0 - old) } else { (1.0 - size) / others };
            }
        }
        parent.kids[location.index].size = size;
        let tab = &self.tabs(location.holder).tabs[location.tab];
        Ok(reply([("layout", self.tree(tab.root.as_ref(), None, panes))]))
    }

    fn arrange(&mut self, args: &Value, panes: &Panes) -> Result<Map, KmuxError> {
        fn build(node: &Value, used: &mut Vec<String>, panes: &Panes, outside: &[String]) -> Result<Node, KmuxError> {
            if let Some(reference) = string(node, "pane") {
                let pane = panes.need(Some(reference))?;
                let id = pane.id.clone();
                if outside.contains(&id) {
                    return Err(docked(format!(
                        "{} is docked, outside the tabs; undock it first (move {} --tab new)",
                        pane.label(),
                        pane.name.as_deref().unwrap_or(&pane.id)
                    )));
                }
                if used.contains(&id) {
                    return Err(layout_invalid(format!("\"{reference}\" appears more than once")));
                }
                used.push(id.clone());
                return Ok(Node::Pane(id));
            }
            let axis = string(node, "split").and_then(Axis::parse);
            let children = node.get("children").and_then(Value::as_array).filter(|c| !c.is_empty());
            let (Some(axis), Some(children)) = (axis, children) else { return Err(layout_invalid("malformed layout tree")) };
            let sizes = children
                .iter()
                .map(|child| match child.get("size") {
                    None | Some(Value::Null) => Ok(None),
                    Some(value) => {
                        let size = fraction(value)?;
                        if !(size > 0.0 && size <= 1.0) {
                            return Err(layout_invalid("sizes must be fractions between 0 and 1"));
                        }
                        Ok(Some(size))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            let given: f64 = sizes.iter().flatten().sum();
            let unsized_count = sizes.iter().filter(|s| s.is_none()).count();
            if given > 1.0 + 1e-9 {
                return Err(layout_invalid(format!("sizes in a {} add up to {}, more than 1", axis.as_str(), fraction::format(given))));
            }
            if unsized_count > 0 && given >= 1.0 - 1e-9 {
                return Err(layout_invalid(format!("no space left for the unsized panes in a {}", axis.as_str())));
            }
            let fill = if unsized_count > 0 { (1.0 - given) / unsized_count as f64 } else { 0.0 };
            let scale = if unsized_count > 0 { 1.0 } else { 1.0 / given };
            let kids = children
                .iter()
                .zip(sizes)
                .map(|(child, size)| Ok(Child { node: build(child, used, panes, outside)?, size: size.unwrap_or(fill) * scale }))
                .collect::<Result<Vec<_>, KmuxError>>()?;
            Ok(Node::Split(Split { axis, kids }))
        }
        let mut used = vec![];
        let outside: Vec<String> = self.workspaces.iter().map(|s| s.dock.pane.clone()).collect();
        let root = normalize(build(args.get("layout").unwrap_or(&Value::Null), &mut used, panes, &outside)?);
        let from = string(args, "from").filter(|_| args.get("window").is_none());
        let caller = from.and_then(|from| panes.need(Some(from)).ok()).and_then(|p| self.holder_of(&p.id));
        let h = match (caller, string(args, "workspace"), from) {
            (_, Some(workspace), _) => self.workspace_holder(workspace)?,
            (Some(h), None, _) => h,
            (None, None, Some(from)) => return Err(not_found(format!("{from} is in no window or workspace here: say which with --window WINDOW"))),
            (None, None, None) => Holder::Window(self.target_window(string(args, "window"))?),
        };
        let t = match self.tabs(h).active_index() {
            Some(t) => t,
            None => self.make_tab(h, true),
        };
        let tab = self.tabs(h).tabs[t].id.clone();
        for id in &used {
            self.detach(id);
        }
        let (h, t) = self.tab_index(&tab).unwrap();
        let leftovers = self.tabs(h).tabs[t].panes();
        self.tabs_mut(h).tabs[t].root = Some(root);
        if !leftovers.is_empty() {
            let rest = self.make_tab(h, false);
            let share = 1.0 / leftovers.len() as f64;
            let kids = leftovers.into_iter().map(|id| Child { node: Node::Pane(id), size: share }).collect();
            let rest = &mut self.tabs_mut(h).tabs[rest];
            rest.rename("unarranged");
            rest.root = Some(normalize(Node::Split(Split { axis: Axis::Row, kids })));
        }
        let tabs = self.tabs_mut(h);
        tabs.active = Some(tab.clone());
        if !tabs.focused.as_ref().is_some_and(|f| used.contains(f)) {
            tabs.focused = used.first().cloned();
        }
        let mut out = self.at(h);
        self.drop_empty();
        out.insert("layout".into(), self.tab_tree(&tab, panes));
        Ok(out)
    }

    fn window_command(&mut self, args: &Value, panes: &Panes) -> Result<Map, KmuxError> {
        let reference = string(args, "window").ok_or_else(|| bad_request("window needs a window"))?;
        let w = self.window_index(reference).ok_or_else(|| not_found(format!("no window \"{reference}\"")))?;
        if let Some(lead) = &self.windows[w].lead {
            return Err(KmuxError::new("lead", format!("{reference} has a lead, {}, and stays its own", panes.display(&lead.pane))));
        }
        let was = self.windows[w].workspace.as_deref().and_then(|id| self.ws_index(id)).map(|s| self.workspaces[s].summary());
        match string(args, "action").unwrap_or("") {
            "assign" => {
                let task = string(args, "workspace").ok_or_else(|| bad_request("assign needs a workspace: a task's ID or its agent's pane"))?;
                let s = self.find_workspace(task, panes).filter(|&s| self.workspaces[s].is_task()).ok_or_else(|| not_found(format!("no workspace \"{task}\"")))?;
                self.windows[w].workspace = Some(self.workspaces[s].id.clone());
            }
            "free" => self.windows[w].workspace = None,
            other => return Err(bad_request(format!("unknown window action \"{other}\" (assign or free)"))),
        }
        self.drop_empty();
        let mut out = self.at(Holder::Window(w));
        out.insert("was".into(), was.unwrap_or(Value::Null));
        Ok(out)
    }

    pub(crate) fn find_workspace(&self, reference: &str, panes: &Panes) -> Option<usize> {
        if let Some(s) = self.ws_index(reference) {
            return Some(s);
        }
        let id = panes.need(Some(reference)).ok()?.id.clone();
        self.dock_ws(&id).or_else(|| self.holder_of(&id).and_then(|h| self.ws_of(h)))
    }

    pub(crate) fn export_node(node: &Node) -> Value {
        match node {
            Node::Pane(id) => Value::Object(reply([("pane", id.clone().into())])),
            Node::Split(split) => {
                let children = split
                    .kids
                    .iter()
                    .map(|kid| {
                        let mut fields = match Model::export_node(&kid.node) {
                            Value::Object(fields) => fields,
                            _ => Map::new(),
                        };
                        fields.insert("size".into(), crate::json::num(kid.size));
                        Value::Object(fields)
                    })
                    .collect();
                Value::Object(reply([("split", split.axis.as_str().into()), ("children", Value::Array(children))]))
            }
        }
    }

    fn list_window(&self, w: usize, panes: &Panes) -> Value {
        let window = &self.windows[w];
        let tabs = &window.tabs;
        let rows = tabs
            .tabs
            .iter()
            .map(|tab| {
                Value::Object(reply([
                    ("id", tab.id.clone().into()),
                    ("title", tab.title.clone().into()),
                    ("active", Value::Bool(Some(&tab.id) == tabs.active.as_ref())),
                    ("layout", self.tree(tab.root.as_ref(), None, panes)),
                ]))
            })
            .collect();
        let mut out = reply([
            ("id", window.id.clone().into()),
            ("focused", tabs.focused.as_ref().map_or(Value::Null, |p| panes.display(p).into())),
            ("zoomed", tabs.zoomed().map_or(Value::Null, |p| panes.display(p).into())),
            ("tabs", Value::Array(rows)),
        ]);
        if let Some(lead) = &window.lead {
            out.insert("lead".into(), Value::Object(reply([("pane", panes.display(&lead.pane).into()), ("width", fraction::format(lead.width).into())])));
        }
        if let Some(ws) = &window.workspace {
            out.insert("workspace".into(), ws.clone().into());
        }
        Value::Object(out)
    }
}

impl LayoutHost for Model {
    fn commands(&self) -> &'static [&'static str] {
        &["focus", "zoom", "rename-tab", "move", "move-tab", "resize", "arrange", "task", "window"]
    }

    fn features(&self) -> &'static [&'static str] {
        &["windows", "tabs", "fractionalSizing", "zoom", "dock"]
    }

    fn rename(&mut self, from: &str, to: &str) {
        self.rename_pane(from, to);
    }

    fn place(&mut self, pane: &str, at: &Placement, _panes: &Panes) -> Result<Option<String>, KmuxError> {
        if at.lead {
            return self.lead(pane, at.window.as_deref()).map(Some);
        }
        if at.planner {
            let s = self.shelve(pane, Edge::Left, Where::Planner(at.app.clone().unwrap_or_else(|| MAC_APP.into())), false);
            self.workspaces[s].parked_at = None;
            return Ok(None);
        }
        if let Some(edge) = at.dock {
            let place = if at.backlog { Where::Backlog } else { Where::Shelf(at.app.clone().unwrap_or_else(|| MAC_APP.into())) };
            self.shelve(pane, edge, place, at.frozen);
            return Ok(None);
        }
        let caller = at.opener.as_deref().or(at.from.as_deref()).and_then(|c| self.holder_of(c).map(|h| (c, h)));
        let planning = |model: &Model, h: Holder| matches!(h, Holder::Workspace(s) if !model.workspaces[s].is_task());
        let (mut h, mut beside, mut new_tab) = (None, None, at.new_tab);
        match (at.window.as_deref(), caller) {
            _ if at.window.is_none() && at.workspace.is_some() => h = Some(self.workspace_holder(at.workspace.as_deref().unwrap_or_default())?),
            (Some("new"), Some((_, from))) => h = Some(Holder::Window(self.window_for(self.ws_of(from)))),
            (Some(window), _) => h = Some(Holder::Window(self.target_window(Some(window))?)),
            (None, Some((_, from))) if planning(self, from) => h = Some(Holder::Window(self.make_window())),
            (None, Some((c, from))) => {
                h = Some(from);
                let side = self.side(from).as_deref() == Some(c);
                if side && at.from.is_some() {
                    new_tab = true;
                } else if at.from.as_deref() == Some(c) {
                    beside = self.locate(c).map(|l| (l.tab, c.to_string()));
                }
            }
            (None, None) => {
                if let Some(c) = at.opener.as_deref().or(at.from.as_deref()) {
                    return Err(not_found(format!("{c} is in no window or workspace, so there's nowhere beside it: say where with --window WINDOW (or --window new)")));
                }
            }
        }
        let h = match h {
            Some(h) => h,
            None => Holder::Window(self.target_window(None)?),
        };
        let t = match &beside {
            Some((t, _)) => *t,
            None if new_tab || self.tabs(h).tabs.is_empty() => self.make_tab(h, true),
            None => self.tabs(h).active_index().unwrap(),
        };
        let wide = {
            let tabs = self.tabs(h);
            let ids = tabs.tabs[t].panes();
            (!ids.is_empty()).then(|| match &beside {
                Some((_, from)) => from.clone(),
                None => tabs.focused.clone().filter(|f| ids.contains(f)).unwrap_or_else(|| ids.last().unwrap().clone()),
            })
        };
        let axis = |model: &Model, target: &str| match at.split {
            SplitSide::Right => Axis::Row,
            SplitSide::Down => Axis::Column,
            SplitSide::Auto if model.wide(h, t, target) => Axis::Row,
            SplitSide::Auto => Axis::Column,
        };
        match wide {
            None => self.tabs_mut(h).tabs[t].root = Some(Node::Pane(pane.into())),
            Some(target) => {
                let axis = axis(self, &target);
                self.tabs_mut(h).insert(Node::Pane(pane.into()), &target, t, axis, true, at.size);
            }
        }
        let tabs = self.tabs_mut(h);
        if beside.is_some() {
            tabs.active = Some(tabs.tabs[t].id.clone());
        }
        tabs.focused = Some(pane.into());
        if let Some(active) = tabs.active_index() {
            tabs.tabs[active].last_focus = Some(pane.into());
        }
        Ok(self.window_of(h).map(|w| self.windows[w].id.clone()))
    }

    fn location(&self, pane: &str) -> Option<Spot> {
        if let Some(w) = self.lead_window(pane) {
            return Some(Spot { window: Some(self.windows[w].id.clone()), tab: None, dock: None, workspace: Value::Null });
        }
        if let Some(s) = self.dock_ws(pane) {
            let (window, workspace) = self.spot_of(Holder::Workspace(s));
            return Some(Spot { window, tab: None, dock: Some(self.workspaces[s].dock.edge).filter(|_| self.workspaces[s].is_task()), workspace });
        }
        let location = self.locate(pane)?;
        let (window, workspace) = self.spot_of(location.holder);
        Some(Spot { window, tab: Some(self.tabs(location.holder).tabs[location.tab].id.clone()), dock: None, workspace })
    }

    fn panes_in(&mut self, scope: &Scope) -> Result<Vec<String>, KmuxError> {
        match scope {
            Scope::Tab(reference) => {
                let (h, t) = self.tab_index(reference).ok_or_else(|| not_found(format!("no tab \"{reference}\"")))?;
                Ok(self.tabs(h).tabs[t].panes())
            }
            Scope::Window(reference) => {
                let w = self.target_window(reference.as_deref())?;
                let window = &self.windows[w];
                Ok(window.tabs.panes().into_iter().chain(window.lead.iter().map(|d| d.pane.clone())).collect())
            }
            Scope::Task(task) => self.task_panes(task),
        }
    }

    fn remove(&mut self, pane: &str) {
        self.detach(pane);
    }

    fn close_task(&mut self, task: &str) {
        Model::close_task(self, task);
    }

    fn tidy(&mut self) {
        self.drop_empty();
    }

    fn command(&mut self, cmd: &str, args: &Value, panes: &Panes) -> Result<Map, KmuxError> {
        match cmd {
            "focus" => self.focus(args, panes),
            "zoom" => self.zoom(args, panes),
            "rename-tab" => self.rename_tab(args),
            "move" => self.move_pane(args, panes),
            "move-tab" => self.move_tab(args),
            "resize" => self.resize(args, panes),
            "arrange" => self.arrange(args, panes),
            "task" => self.task(args, panes),
            "window" => self.window_command(args, panes),
            _ => Err(bad_request(format!("unknown command \"{cmd}\""))),
        }
    }

    fn windows(&self, panes: &Panes) -> Value {
        Value::Array(self.sorted_windows().into_iter().map(|w| self.list_window(w, panes)).collect())
    }

    fn hidden_windows(&self, _panes: &Panes) -> Value {
        Value::Array(vec![])
    }

    fn workspaces(&self, panes: &Panes) -> Value {
        Value::Array(
            self.workspaces
                .iter()
                .enumerate()
                .map(|(s, ws)| {
                    let mut out = ws.summary();
                    out["dock"] = self.dock_view(s, panes);
                    out["dock"]["id"] = ws.dock.pane.clone().into();
                    out["tabs"] = Value::Array(
                        ws.tabs
                            .tabs
                            .iter()
                            .map(|t| crate::json::json!({ "id": t.id, "title": t.title, "active": ws.tabs.active.as_ref() == Some(&t.id), "layout": self.tree(t.root.as_ref(), None, panes), "panes": t.panes().iter().map(|p| panes.display(p)).collect::<Vec<_>>() }))
                            .collect(),
                    );
                    out["focused"] = ws.tabs.focused.as_ref().map_or(Value::Null, |p| panes.display(p).into());
                    out["zoomed"] = ws.tabs.zoomed().map_or(Value::Null, |p| panes.display(p).into());
                    out["windows"] = Value::Array(self.workspace_windows(&ws.id).iter().map(|w| Value::String(w.id.clone())).collect());
                    out["panes"] = Value::Array(self.workspace_panes(s).iter().map(|p| Value::String(p.clone())).collect());
                    out
                })
                .collect(),
        )
    }

    fn export_state(&self) -> Map {
        self.export_v1()
    }

    fn read_state(&self, state: &Value, panes: &HashSet<String>) -> Result<SavedLayout, KmuxError> {
        crate::saved::read(state, panes)
    }

    fn install_state(&mut self, saved: SavedLayout) {
        crate::saved::install(self, saved);
    }

    fn retitle(&mut self, panes: &Panes) -> bool {
        Model::retitle(self, panes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_are_described_as_swift_prints_them() {
        assert_eq!(describe(&json!(null)), "null");
        assert_eq!(describe(&json!(true)), "bool(true)");
        assert_eq!(describe(&json!(100)), "number(100.0)");
        assert_eq!(describe(&json!(-1e20)), "number(-1e+20)");
        assert_eq!(describe(&json!(-0.00001)), "number(-1e-05)");
        assert_eq!(describe(&json!(9.5e15)), "number(9.5e+15)");
        assert_eq!(describe(&json!(9007199254740992.0)), "number(9007199254740992.0)");
        assert_eq!(describe(&json!(1.25e-7)), "number(1.25e-07)");
        assert_eq!(describe(&json!(0.0001)), "number(0.0001)");
        assert_eq!(describe(&json!("a'b\u{1b}é\u{7f}\t\0\\\"")), "string(\"a\\'b\\u{1B}é\\u{7F}\\t\\0\\\\\\\"\")");
        assert_eq!(describe(&json!([])), "array([])");
        assert_eq!(describe(&json!({})), "object([:])");
        assert_eq!(
            describe(&json!([[null], { "k": "v" }, 1, false])),
            "array([KmuxCore.JSON.array([KmuxCore.JSON.null]), KmuxCore.JSON.object([\"k\": KmuxCore.JSON.string(\"v\")]), KmuxCore.JSON.number(1.0), KmuxCore.JSON.bool(false)])"
        );
    }
}
