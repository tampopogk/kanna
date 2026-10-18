use crate::error::KmuxError;
use crate::json::{encode, is_true, num, opt, string, Map, Value};
use crate::pane::{content_prefix, diff_args, read_diff_args, read_view, Comment, History, Pane, PaneType, Panes};
use std::collections::HashSet;

pub const STATE_VERSION: u32 = 1;

pub(crate) fn bad(message: &str) -> KmuxError {
    KmuxError::new("bad_request", format!("bad state: {message}"))
}

pub(crate) fn quoted(value: Option<&Value>) -> String {
    encode(value.unwrap_or(&Value::Null))
}

pub(crate) fn number_in(id: &str, prefix: &str) -> Option<usize> {
    let n: usize = id.strip_prefix(prefix)?.parse().ok()?;
    (n > 0 && n < 1_000_000_000 && format!("{prefix}{n}") == id).then_some(n)
}

pub(crate) fn whole(value: Option<&Value>) -> Option<usize> {
    let n = value?.as_f64()?;
    (n.round() == n && n >= 0.0 && n < 1e9).then_some(n as usize)
}

pub fn pane_record(pane: &Pane) -> Map {
    let mut out = Map::new();
    out.insert("id".into(), pane.id.clone().into());
    out.insert("name".into(), opt(pane.name.as_deref()));
    out.insert("type".into(), pane.kind.as_str().into());
    match pane.kind {
        PaneType::Term => {
            out.insert("terminal".into(), opt(pane.terminal.as_deref()));
            out.insert("cmd".into(), opt(pane.command.as_deref()));
            out.insert("cwd".into(), opt(pane.cwd.as_deref()));
            out.insert("session".into(), opt(pane.session.as_deref()));
        }
        PaneType::Web => {
            out.insert("url".into(), opt(pane.url.as_deref()));
            out.insert("history".into(), Value::Bool(pane.history.is_some()));
            if let Some(title) = &pane.title {
                out.insert("title".into(), Value::String(title.clone()));
            }
            if pane.markup {
                out.insert("markup".into(), Value::Bool(true));
            }
            if let Some(agent) = &pane.agent {
                out.insert("agent".into(), Value::String(agent.clone()));
            }
        }
        PaneType::Md => {
            out.insert("path".into(), opt(pane.path.as_deref()));
            out.insert("zoom".into(), num(pane.zoom.unwrap_or(1.0)));
            if pane.markup {
                out.insert("markup".into(), Value::Bool(true));
            }
            if let Some(agent) = &pane.agent {
                out.insert("agent".into(), Value::String(agent.clone()));
            }
        }
        PaneType::Code => {
            out.insert("path".into(), opt(pane.path.as_deref()));
            if let Some(line) = pane.line {
                out.insert("line".into(), num(line as f64));
            }
            out.insert("zoom".into(), num(pane.zoom.unwrap_or(1.0)));
            if let Some(comment) = &pane.comment {
                out.insert("comment".into(), comment.to_json());
            }
        }
        PaneType::Diff => {
            out.insert("path".into(), opt(pane.path.as_deref()));
            out.insert("args".into(), diff_args(pane.diff.as_deref()));
            out.insert("view".into(), Value::String(pane.view.clone().unwrap_or_else(|| "unified".into())));
            out.insert("zoom".into(), num(pane.zoom.unwrap_or(1.0)));
        }
        PaneType::Ios => {
            out.insert("app".into(), opt(pane.app.as_deref()));
            out.insert("device".into(), opt(pane.device.as_deref()));
        }
    }
    out
}

pub(crate) fn export_panes(panes: &Panes) -> Value {
    Value::Array(panes.iter().map(|pane| Value::Object(pane_record(pane))).collect())
}

pub fn read_record(item: &Value, id: &str, kind: PaneType) -> Pane {
    let mut pane = Pane::new(id, string(item, "name").map(String::from), kind);
    let field = |key: &str| string(item, key).map(String::from);
    match kind {
        PaneType::Term => {
            pane.command = field("cmd");
            pane.cwd = field("cwd");
            pane.session = field("session");
            pane.terminal = field("terminal");
        }
        PaneType::Web => {
            pane.url = field("url");
            pane.title = field("title");
            pane.agent = field("agent");
            pane.markup = is_true(item, "markup");
            if is_true(item, "history") {
                pane.history = Some(History::default());
            }
        }
        PaneType::Md => {
            pane.path = field("path");
            pane.zoom = item.get("zoom").and_then(Value::as_f64).filter(|z| *z > 0.0);
            pane.agent = field("agent");
            pane.markup = is_true(item, "markup");
            pane.history = Some(History::default());
        }
        PaneType::Code => {
            pane.path = field("path");
            pane.line = item.get("line").and_then(Value::as_u64).filter(|l| *l > 0);
            pane.comment = item.get("comment").and_then(Comment::from_json);
            pane.zoom = item.get("zoom").and_then(Value::as_f64).filter(|z| *z > 0.0);
            pane.history = Some(History::default());
        }
        PaneType::Diff => {
            pane.path = field("path");
            pane.diff = read_diff_args(item.get("args")).ok().flatten();
            pane.view = read_view(item.get("view")).ok().flatten();
            pane.zoom = item.get("zoom").and_then(Value::as_f64).filter(|z| *z > 0.0);
        }
        PaneType::Ios => {
            pane.app = field("app");
            pane.device = field("device");
        }
    }
    pane
}

pub(crate) fn import_panes(state: &Value) -> Result<(Vec<Pane>, usize), KmuxError> {
    import_versions(state, &[STATE_VERSION], "this kmux reads")
}

pub(crate) fn import_versions(state: &Value, versions: &[u32], reader: &str) -> Result<(Vec<Pane>, usize), KmuxError> {
    if !state.is_object() {
        return Err(bad("not an object"));
    }
    if !versions.iter().any(|v| state.get("version").and_then(Value::as_f64) == Some(*v as f64)) {
        let read = versions.iter().map(u32::to_string).collect::<Vec<_>>().join(" and ");
        return Err(KmuxError::new("bad_request", format!("unsupported state version {} ({reader} {read})", quoted(state.get("version")))));
    }
    let (Some(saved_panes), Some(_)) = (state.get("panes").and_then(Value::as_array), state.get("windows").and_then(Value::as_array)) else {
        return Err(bad("needs panes and windows"));
    };
    let mut counter = whole(state.get("counters").and_then(|c| c.get("pane"))).unwrap_or(0);

    let mut panes: Vec<Pane> = vec![];
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for item in saved_panes {
        let Some(id) = string(item, "id").filter(|_| item.is_object()) else { return Err(bad(&format!("pane {}", quoted(item.get("id"))))) };
        let Some(kind) = string(item, "type").and_then(PaneType::parse) else {
            return Err(bad(&format!("pane {id} has unknown type {}", quoted(item.get("type")))));
        };
        let n = match number_in(id, "p") {
            Some(n) => n,
            None if number_in(id, &format!("{}-", content_prefix(kind))).is_some() => 0,
            None => return Err(bad(&format!("pane {}", quoted(item.get("id"))))),
        };
        if !ids.insert(id.to_string()) {
            return Err(bad(&format!("pane {}", quoted(item.get("id")))));
        }
        let name = string(item, "name").map(String::from);
        if let Some(name) = &name {
            if name.is_empty() || !names.insert(name.clone()) {
                return Err(bad(&format!("pane name {} is used twice", quoted(Some(&Value::String(name.clone()))))));
            }
        }
        let needed = match kind {
            PaneType::Web => Some("url"),
            PaneType::Md | PaneType::Code | PaneType::Diff => Some("path"),
            PaneType::Ios => Some("app"),
            PaneType::Term => None,
        };
        if let Some(needed) = needed {
            if string(item, needed).map_or(true, str::is_empty) {
                return Err(bad(&format!("{} pane {id} has no {needed}", kind.as_str())));
            }
        }
        panes.push(read_record(item, id, kind));
        counter = counter.max(n);
    }
    Ok((panes, counter))
}
