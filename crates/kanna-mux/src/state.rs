use crate::error::KmuxError;
use crate::json::{encode, is_true, json, num, opt, string, Map, Value};
use crate::pane::{content_prefix, diff_args, CONTENT_PREFIXES, read_diff_args, read_view, Comment, History, Pane, PaneType, Panes, StopMark, Tour};
use std::collections::{HashMap, HashSet};

pub const STATE_VERSION: u32 = 2;

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
            if let Some(comment) = &pane.comment {
                out.insert("comment".into(), comment.to_json());
            }
        }
        PaneType::Diff => {
            out.insert("path".into(), opt(pane.path.as_deref()));
            out.insert("args".into(), diff_args(pane.diff.as_deref()));
            out.insert("view".into(), Value::String(pane.view.clone().unwrap_or_else(|| "unified".into())));
        }
        PaneType::Ios => {
            out.insert("app".into(), opt(pane.app.as_deref()));
            out.insert("device".into(), opt(pane.device.as_deref()));
        }
    }
    if let Some(tour) = &pane.tour {
        out.insert("tour".into(), tour.to_json());
    }
    if let Some(stop) = &pane.stop {
        out.insert("tour".into(), stop.to_json());
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
            pane.agent = field("agent");
            pane.markup = is_true(item, "markup");
            pane.history = Some(History::default());
        }
        PaneType::Code => {
            pane.path = field("path");
            pane.line = item.get("line").and_then(Value::as_u64).filter(|l| *l > 0);
            pane.comment = item.get("comment").and_then(Comment::from_json);
            pane.history = Some(History::default());
        }
        PaneType::Diff => {
            pane.path = field("path");
            pane.diff = read_diff_args(item.get("args")).ok().flatten();
            pane.view = read_view(item.get("view")).ok().flatten();
        }
        PaneType::Ios => {
            pane.app = field("app");
            pane.device = field("device");
        }
    }
    match kind {
        PaneType::Term => pane.tour = item.get("tour").and_then(Tour::from_json),
        PaneType::Code | PaneType::Md => pane.stop = item.get("tour").and_then(StopMark::from_json),
        _ => {}
    }
    pane
}

pub(crate) fn import_panes(state: &Value) -> Result<(Vec<Pane>, usize), KmuxError> {
    import_versions(state, &[1], "this kmux reads")
}

pub(crate) fn upgrade(state: &Value) -> Value {
    if state.get("version").and_then(Value::as_f64) != Some(1.0) {
        return state.clone();
    }
    let kinds: HashMap<&str, &str> = state
        .get("panes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| Some((string(p, "id")?, content_prefix(string(p, "type").and_then(PaneType::parse)?))))
        .collect();
    let id = |value: &Value| -> Value {
        let Some(text) = value.as_str() else { return value.clone() };
        let renamed = |prefix: &str, to: &str| number_in(text, prefix).map(|n| format!("{to}-{n}"));
        let found = match text.chars().next() {
            Some('p') => kinds.get(text).and_then(|kind| renamed("p", kind)),
            Some('t') => renamed("t", "tab"),
            Some('w') => renamed("w", "win"),
            Some('s') => renamed("s", "ws"),
            _ => None,
        };
        found.map(Value::String).unwrap_or_else(|| value.clone())
    };
    fn walk(value: &Value, key: Option<&str>, id: &dyn Fn(&Value) -> Value) -> Value {
        match value {
            Value::String(_) if matches!(key, Some("id" | "key" | "active" | "focused" | "zoomed" | "workspace" | "pane" | "lastFocus")) => id(value),
            Value::Array(items) => Value::Array(items.iter().map(|item| walk(item, key, id)).collect()),
            Value::Object(fields) => Value::Object(fields.iter().map(|(k, item)| (k.clone(), walk(item, Some(k), id))).collect()),
            _ => value.clone(),
        }
    }
    let mut out = walk(state, None, &id);
    let counters = state.get("counters");
    let count = |key: &str| counters.and_then(|c| c.get(key)).cloned().unwrap_or(Value::Null);
    let pane = count("pane");
    out["version"] = num(STATE_VERSION as f64);
    out["counters"] = json!({ "term": pane, "page": pane, "file": pane, "ios": pane, "diff": pane, "tab": count("tab"), "window": count("window"), "workspace": count("workspace") });
    out
}

pub(crate) fn import_content(state: &Value) -> Result<(Vec<Pane>, HashMap<String, usize>), KmuxError> {
    if !state.is_object() {
        return Err(bad("not an object"));
    }
    if state.get("version").and_then(Value::as_f64) != Some(STATE_VERSION as f64) {
        return Err(KmuxError::new("bad_request", format!("unsupported state version {} (this kmux reads 1 and {STATE_VERSION})", quoted(state.get("version")))));
    }
    let saved = state.get("counters");
    let mut counters: HashMap<String, usize> =
        CONTENT_PREFIXES.iter().map(|prefix| (prefix.to_string(), whole(saved.and_then(|c| c.get(*prefix))).unwrap_or(0))).collect();
    for item in state.get("panes").and_then(Value::as_array).into_iter().flatten() {
        let (Some(id), Some(kind)) = (string(item, "id"), string(item, "type").and_then(PaneType::parse)) else { continue };
        let Some((prefix, _)) = id.split_once('-').filter(|(prefix, n)| !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_lowercase()) && n.parse::<usize>().is_ok_and(|n| n > 0)) else {
            return Err(bad(&format!("pane {}", quoted(item.get("id")))));
        };
        let kind_prefix = content_prefix(kind);
        let n = number_in(id, &format!("{kind_prefix}-")).filter(|_| prefix == kind_prefix);
        let Some(n) = n else { return Err(bad(&format!("pane {id} is a {} pane, so its ID is {kind_prefix}-N", kind.as_str()))) };
        let at = counters.entry(kind_prefix.to_string()).or_insert(0);
        *at = (*at).max(n);
    }
    let (panes, _) = import_versions(state, &[STATE_VERSION], "this kmux reads")?;
    Ok((panes, counters))
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
