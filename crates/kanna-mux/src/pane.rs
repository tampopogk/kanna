use crate::error::KmuxError;
use crate::json::{num, opt, Map, Value};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaneType {
    Term,
    Web,
    Ios,
    Md,
    Code,
    Diff,
}

impl PaneType {
    pub const ALL: [PaneType; 6] = [PaneType::Term, PaneType::Web, PaneType::Ios, PaneType::Md, PaneType::Code, PaneType::Diff];

    pub fn as_str(self) -> &'static str {
        match self {
            PaneType::Term => "term",
            PaneType::Web => "web",
            PaneType::Ios => "ios",
            PaneType::Md => "md",
            PaneType::Code => "code",
            PaneType::Diff => "diff",
        }
    }

    pub fn parse(name: &str) -> Option<PaneType> {
        PaneType::ALL.into_iter().find(|t| t.as_str() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneState {
    Starting,
    Running,
    Exited,
    Failed,
}

impl PaneState {
    pub fn as_str(self) -> &'static str {
        match self {
            PaneState::Starting => "starting",
            PaneState::Running => "running",
            PaneState::Exited => "exited",
            PaneState::Failed => "failed",
        }
    }

    pub fn parse(name: &str) -> Option<PaneState> {
        [PaneState::Starting, PaneState::Running, PaneState::Exited, PaneState::Failed].into_iter().find(|s| s.as_str() == name)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    pub back: Vec<String>,
    pub forward: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub path: String,
    pub first: u64,
    pub last: u64,
    pub text: String,
}

impl Comment {
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("path".into(), Value::String(self.path.clone()));
        out.insert("first".into(), num(self.first as f64));
        out.insert("last".into(), num(self.last as f64));
        out.insert("text".into(), Value::String(self.text.clone()));
        Value::Object(out)
    }

    pub fn from_json(value: &Value) -> Option<Comment> {
        let line = |key: &str| value.get(key).and_then(Value::as_u64).filter(|l| *l > 0);
        let (first, last) = (line("first")?, line("last")?);
        Some(Comment {
            path: value.get("path")?.as_str()?.to_string(),
            first,
            last: last.max(first),
            text: value.get("text")?.as_str()?.to_string(),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pane {
    pub id: String,
    pub name: Option<String>,
    pub kind: PaneType,
    pub state: PaneState,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub opened_in: Option<String>,
    pub url: Option<String>,
    pub title: Option<String>,
    pub app: Option<String>,
    pub device: Option<String>,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub comment: Option<Comment>,
    pub diff: Option<Vec<String>>,
    pub view: Option<String>,
    pub agent: Option<String>,
    pub markup: bool,
    pub history: Option<History>,
    pub zoom: Option<f64>,
    pub session: Option<String>,
    pub pid: Option<i64>,
    pub status: Option<String>,
    pub hung_up: bool,
    pub terminal: Option<String>,
}

impl Pane {
    pub fn new(id: &str, name: Option<String>, kind: PaneType) -> Pane {
        Pane {
            id: id.into(),
            name,
            kind,
            state: PaneState::Starting,
            exit_code: None,
            error: None,
            command: None,
            cwd: None,
            opened_in: None,
            url: None,
            title: None,
            app: None,
            device: None,
            path: None,
            line: None,
            comment: None,
            diff: None,
            view: None,
            agent: None,
            markup: false,
            history: None,
            zoom: None,
            session: None,
            pid: None,
            status: None,
            hung_up: false,
            terminal: None,
        }
    }

    pub fn status(&self) -> Option<&str> {
        match (self.kind, self.state) {
            (PaneType::Term, PaneState::Exited) => Some("exited"),
            (PaneType::Term, _) => self.status.as_deref(),
            _ => None,
        }
    }

    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("{name} ({})", self.id),
            None => self.id.clone(),
        }
    }

    pub fn tab_name(&self) -> Option<String> {
        let name = match (&self.name, self.kind) {
            (Some(name), _) => Some(name.clone()),
            (None, PaneType::Md | PaneType::Code) => self.path.as_deref().and_then(file_name),
            (None, PaneType::Web) => self.title.clone().filter(|t| !t.trim().is_empty()).or_else(|| self.url.as_deref().and_then(host)),
            (None, PaneType::Diff) => Some(std::iter::once("diff").chain(self.diff.iter().flatten().map(String::as_str)).collect::<Vec<_>>().join(" ")),
            (None, PaneType::Term | PaneType::Ios) => None,
        };
        name.map(|n| n.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()).filter(|n| !n.is_empty())
    }

    pub fn summary(&self) -> Value {
        let mut out = Map::new();
        out.insert("id".into(), Value::String(self.id.clone()));
        out.insert("name".into(), opt(self.name.as_deref()));
        out.insert("type".into(), self.kind.as_str().into());
        out.insert("state".into(), self.state.as_str().into());
        match self.kind {
            PaneType::Term => {
                out.insert("terminal".into(), opt(self.terminal.as_deref()));
                out.insert("cmd".into(), opt(self.command.as_deref()));
                out.insert("cwd".into(), opt(self.cwd.as_deref()));
                if let Some(pid) = self.pid {
                    out.insert("pid".into(), num(pid as f64));
                }
            }
            PaneType::Web => {
                out.insert("url".into(), opt(self.url.as_deref()));
                out.insert("history".into(), Value::Bool(self.history.is_some()));
                if self.markup {
                    out.insert("markup".into(), Value::Bool(true));
                }
                if let Some(agent) = &self.agent {
                    out.insert("agent".into(), Value::String(agent.clone()));
                }
            }
            PaneType::Ios => {
                out.insert("app".into(), opt(self.app.as_deref()));
                out.insert("device".into(), opt(self.device.as_deref()));
            }
            PaneType::Md => {
                out.insert("path".into(), opt(self.path.as_deref()));
                out.insert("agent".into(), opt(self.agent.as_deref()));
                if self.markup {
                    out.insert("markup".into(), Value::Bool(true));
                }
            }
            PaneType::Code => {
                out.insert("path".into(), opt(self.path.as_deref()));
                if let Some(line) = self.line {
                    out.insert("line".into(), num(line as f64));
                }
                if let Some(comment) = &self.comment {
                    out.insert("comment".into(), comment.to_json());
                }
            }
            PaneType::Diff => {
                out.insert("path".into(), opt(self.path.as_deref()));
                out.insert("args".into(), diff_args(self.diff.as_deref()));
                out.insert("view".into(), Value::String(self.view.clone().unwrap_or_else(|| "unified".into())));
            }
        }
        if let Some(code) = self.exit_code {
            out.insert("exitCode".into(), num(code as f64));
        }
        if let Some(status) = self.status() {
            out.insert("status".into(), Value::String(status.into()));
        }
        if let Some(error) = &self.error {
            out.insert("error".into(), Value::String(error.clone()));
        }
        Value::Object(out)
    }
}

fn file_name(path: &str) -> Option<String> {
    path.trim_end_matches('/').rsplit('/').next().filter(|n| !n.is_empty()).map(String::from)
}

fn host(url: &str) -> Option<String> {
    let Some((_, rest)) = url.split_once("://") else { return file_name(url) };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    if host.is_empty() { file_name(rest.split(['?', '#']).next().unwrap_or("")) } else { Some(host.into()) }
}

#[derive(Debug, Default)]
pub struct Panes {
    panes: HashMap<String, Pane>,
    order: Vec<String>,
    counter: usize,
    terminals: usize,
}

pub fn terminal_number(id: &str) -> Option<usize> {
    let n: usize = id.strip_prefix("term-")?.parse().ok()?;
    (n > 0 && n < 1_000_000_000 && format!("term-{n}") == id).then_some(n)
}

impl Panes {
    pub fn get(&self, id: &str) -> Option<&Pane> {
        self.panes.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Pane> {
        self.panes.get_mut(id)
    }

    pub fn pane(&self, reference: &str) -> Option<&Pane> {
        self.panes.get(reference).or_else(|| self.iter().find(|p| p.name.as_deref() == Some(reference) || p.terminal.as_deref() == Some(reference)))
    }

    pub fn need(&self, reference: Option<&str>) -> Result<&Pane, KmuxError> {
        let reference = reference.ok_or_else(|| KmuxError::new("bad_request", "missing pane"))?;
        self.pane(reference).ok_or_else(|| KmuxError::new("not_found", format!("no pane \"{reference}\"")))
    }

    pub fn display(&self, id: &str) -> String {
        self.panes.get(id).and_then(|p| p.name.clone()).unwrap_or_else(|| id.into())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Pane> {
        self.order.iter().filter_map(|id| self.panes.get(id))
    }

    pub fn ids(&self) -> Vec<String> {
        self.order.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn next_id(&self) -> String {
        format!("p{}", self.counter + 1)
    }

    pub fn make(&mut self, kind: PaneType, name: Option<String>) -> &mut Pane {
        self.counter += 1;
        let pane = Pane::new(&format!("p{}", self.counter), name, kind);
        let id = pane.id.clone();
        self.order.push(id.clone());
        self.panes.entry(id).or_insert(pane)
    }

    pub fn forget(&mut self, id: &str) -> Option<Pane> {
        self.order.retain(|p| p != id);
        self.panes.remove(id)
    }

    pub fn counter(&self) -> usize {
        self.counter
    }

    pub fn terminals(&self) -> usize {
        self.terminals
    }

    pub fn new_terminal(&mut self, id: &str) -> Option<String> {
        let pane = self.panes.get_mut(id).filter(|p| p.kind == PaneType::Term)?;
        self.terminals += 1;
        let terminal = format!("term-{}", self.terminals);
        pane.terminal = Some(terminal.clone());
        Some(terminal)
    }

    pub fn install(&mut self, panes: Vec<Pane>, counter: usize, terminals: usize) {
        let used = panes.iter().filter_map(|p| p.terminal.as_deref().and_then(terminal_number)).max().unwrap_or(0);
        self.order = panes.iter().map(|p| p.id.clone()).collect();
        self.panes = panes.into_iter().map(|p| (p.id.clone(), p)).collect();
        self.counter = self.counter.max(counter);
        self.terminals = self.terminals.max(terminals).max(used);
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;

    #[test]
    fn agent_terminals_list_their_status_and_every_exited_terminal_says_so() {
        let mut pane = Pane::new("p1", None, PaneType::Term);
        pane.state = PaneState::Running;
        assert!(pane.summary().get("status").is_none());
        pane.status = Some("working".into());
        assert_eq!(pane.summary()["status"], "working");
        pane.state = PaneState::Exited;
        assert_eq!(pane.summary()["status"], "exited");
        pane.status = None;
        assert_eq!(pane.summary()["status"], "exited");
        let mut web = Pane::new("p2", None, PaneType::Web);
        web.state = PaneState::Exited;
        assert!(web.summary().get("status").is_none());
    }
}

#[cfg(test)]
mod pid_tests {
    use super::*;

    #[test]
    fn a_terminal_lists_its_pid_once_it_has_one() {
        let mut pane = Pane::new("p1", None, PaneType::Term);
        assert!(pane.summary().get("pid").is_none());
        pane.pid = Some(4242);
        assert_eq!(pane.summary().get("pid").and_then(|v| v.as_f64()), Some(4242.0));
    }
}

#[cfg(test)]
mod tab_name_tests {
    use super::*;

    fn pane(kind: PaneType, name: Option<&str>) -> Pane {
        Pane::new("p1", name.map(String::from), kind)
    }

    #[test]
    fn a_pane_names_its_tab_by_its_name_then_its_file_or_page() {
        let mut code = pane(PaneType::Code, None);
        code.path = Some("/repo/crates/kanna-daemon/src/store.rs".into());
        assert_eq!(code.tab_name().as_deref(), Some("store.rs"));
        let mut md = pane(PaneType::Md, None);
        md.path = Some("docs/specs/presenting.md".into());
        assert_eq!(md.tab_name().as_deref(), Some("presenting.md"));
        let mut named = pane(PaneType::Code, Some("diff"));
        named.path = Some("/repo/a.rs".into());
        assert_eq!(named.tab_name().as_deref(), Some("diff"));
        assert_eq!(pane(PaneType::Term, None).tab_name(), None);
        assert_eq!(pane(PaneType::Term, Some("logs")).tab_name().as_deref(), Some("logs"));
        assert_eq!(pane(PaneType::Ios, None).tab_name(), None);
        let mut diff = pane(PaneType::Diff, None);
        assert_eq!(diff.tab_name().as_deref(), Some("diff"));
        diff.diff = Some(vec!["--merge-base".into(), "main".into()]);
        assert_eq!(diff.tab_name().as_deref(), Some("diff --merge-base main"));
    }

    #[test]
    fn a_web_pane_is_its_page_title_or_its_host_until_the_title_loads() {
        let mut web = pane(PaneType::Web, None);
        web.url = Some("https://user@docs.rs:443/serde/latest?x=1".into());
        assert_eq!(web.tab_name().as_deref(), Some("docs.rs"));
        web.title = Some("serde - Rust".into());
        assert_eq!(web.tab_name().as_deref(), Some("serde - Rust"));
        web.title = Some("  ".into());
        assert_eq!(web.tab_name().as_deref(), Some("docs.rs"));
        web.title = Some("\u{1b}]0;owned\u{7}\u{1b}[2Jpage\n".into());
        assert_eq!(web.tab_name().as_deref(), Some("]0;owned[2Jpage"), "no control characters reach a terminal through kanna workspace list");
        web.title = None;
        web.url = Some("http://[::1]:5173/".into());
        assert_eq!(web.tab_name().as_deref(), Some("::1"));
        web.url = Some("file:///Users/me/mockups/tabs.html".into());
        assert_eq!(web.tab_name().as_deref(), Some("tabs.html"));
    }
}

pub fn diff_args(args: Option<&[String]>) -> Value {
    Value::Array(args.unwrap_or_default().iter().cloned().map(Value::String).collect())
}

pub fn read_diff_args(value: Option<&Value>) -> Result<Option<Vec<String>>, KmuxError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(String::from).ok_or_else(|| KmuxError::new("bad_request", "args must be a list of strings")))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(KmuxError::new("bad_request", "args must be a list of strings")),
    }
}

pub fn read_view(value: Option<&Value>) -> Result<Option<String>, KmuxError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(view)) if view == "unified" || view == "split" => Ok(Some(view.clone())),
        Some(_) => Err(KmuxError::new("bad_request", "view must be unified or split")),
    }
}
