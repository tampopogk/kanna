use serde_json::{json, Value};
use std::ffi::{c_char, CString};
use std::path::{Path, PathBuf};

pub const FENCE: &str = "calltree";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guide {
    None,
    Through,
    Tee,
    End,
}

impl Guide {
    fn name(self) -> &'static str {
        match self {
            Guide::None => "none",
            Guide::Through => "through",
            Guide::Tee => "tee",
            Guide::End => "end",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub process: usize,
    pub depth: usize,
    pub name: String,
    pub path: String,
    pub line: usize,
    pub note: String,
    pub parent: Option<usize>,
    pub number: Option<usize>,
    pub lane_start: bool,
    pub guides: Vec<Guide>,
    pub source_line: usize,
}

impl Call {
    pub fn external(&self) -> bool {
        self.path.starts_with('/') || self.path.starts_with('~')
    }

    pub fn file(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn package(&self) -> String {
        let parts: Vec<&str> = self.path.split('/').collect();
        match parts.iter().position(|p| *p == "src") {
            Some(at) if at > 0 => parts[at - 1].to_string(),
            _ if parts.len() > 1 => parts[parts.len() - 2].to_string(),
            _ => String::new(),
        }
    }

    pub fn location(&self) -> String {
        format!("{}:{}", self.file(), self.line)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Call(Call),
    Hop { label: String, guides: Vec<Guide>, source_line: usize },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub processes: Vec<String>,
    pub rows: Vec<Row>,
    pub errors: Vec<(usize, String)>,
}

enum Line<'a> {
    Blank,
    Process(&'a str),
    Hop(&'a str),
    Call { indent: usize, name: &'a str, path: &'a str, line: usize, note: String },
    Bad(String),
}

fn fields(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        match rest.find("  ") {
            Some(at) => {
                out.push(&rest[..at]);
                rest = rest[at..].trim_start();
            }
            None => {
                out.push(rest);
                break;
            }
        }
    }
    out
}

fn location(field: &str) -> Option<(&str, usize)> {
    let (path, line) = field.rsplit_once(':')?;
    let line: usize = line.parse().ok()?;
    (!path.is_empty() && line > 0 && !path.contains(char::is_whitespace)).then_some((path, line))
}

fn classify(text: &str) -> Line<'_> {
    if text.trim().is_empty() {
        return Line::Blank;
    }
    let indent = text.len() - text.trim_start_matches(' ').len();
    let trimmed = text.trim();
    if let Some(label) = trimmed.strip_prefix("->") {
        return Line::Hop(label.trim());
    }
    let parts = fields(trimmed);
    match parts.get(1).and_then(|f| location(f)) {
        Some((path, line)) => Line::Call { indent, name: parts[0], path, line, note: parts[2..].join(" ") },
        None if indent == 0 => Line::Process(trimmed),
        None => Line::Bad(format!("a call needs NAME  PATH:LINE (two spaces between): {trimmed}")),
    }
}

pub fn quote(text: &str) -> Option<String> {
    match classify(text) {
        Line::Process(name) => Some(name.to_string()),
        Line::Hop(label) => Some(label.to_string()),
        Line::Call { name, path, line, .. } => {
            let call = Call { process: 0, depth: 0, name: name.into(), path: path.into(), line, note: String::new(), parent: None, number: None, lane_start: false, guides: vec![], source_line: 0 };
            Some(call.location())
        }
        Line::Blank | Line::Bad(_) => None,
    }
}

pub fn parse(source: &str) -> Tree {
    let mut tree = Tree::default();
    let mut calls: Vec<Call> = Vec::new();
    let mut order: Vec<Result<usize, (String, usize)>> = Vec::new();
    let mut process: Option<usize> = None;
    let mut lane_open = false;
    let mut last_depth: Option<usize> = None;
    let lines: Vec<&str> = source.lines().collect();
    for (index, text) in lines.iter().enumerate() {
        let at = index + 1;
        match classify(text) {
            Line::Blank => {}
            Line::Bad(message) => tree.errors.push((at, message)),
            Line::Process(name) => {
                let known = tree.processes.iter().position(|p| p == name);
                process = Some(known.unwrap_or_else(|| {
                    tree.processes.push(name.to_string());
                    tree.processes.len() - 1
                }));
                lane_open = true;
            }
            Line::Hop(label) => {
                let next = lines[index + 1..].iter().map(|t| classify(t)).find(|l| !matches!(l, Line::Blank));
                if !matches!(next, Some(Line::Process(_))) {
                    tree.errors.push((at, "a hop needs a process on the next line".into()));
                }
                order.push(Err((label.to_string(), at)));
            }
            Line::Call { indent, name, path, line, note } => {
                let Some(process) = process else {
                    tree.errors.push((at, "a call before any process: name the process on a line of its own first".into()));
                    continue;
                };
                if indent % 2 != 0 {
                    tree.errors.push((at, "indent by two spaces a level".into()));
                }
                let depth = indent / 2;
                if depth > last_depth.map_or(0, |d| d + 1) {
                    tree.errors.push((at, format!("the indent jumps from level {} to {depth}", last_depth.unwrap_or(0))));
                }
                last_depth = Some(depth);
                let parent = calls.iter().rposition(|c| c.depth < depth);
                calls.push(Call {
                    process,
                    depth,
                    name: name.into(),
                    path: path.into(),
                    line,
                    note,
                    parent,
                    number: None,
                    lane_start: std::mem::take(&mut lane_open),
                    guides: vec![],
                    source_line: at,
                });
                order.push(Ok(calls.len() - 1));
            }
        }
    }
    let parents: Vec<Option<usize>> = calls.iter().map(|c| c.parent).collect();
    let siblings = |i: usize| -> Vec<usize> { (0..parents.len()).filter(|&s| parents[s] == parents[i]).collect() };
    for i in 0..calls.len() {
        let all = siblings(i);
        if calls[i].parent.is_some() && all.len() > 1 {
            calls[i].number = all.iter().position(|&s| s == i).map(|p| p + 1);
        }
    }
    for i in 0..calls.len() {
        let mut path = vec![i];
        while let Some(p) = parents[*path.last().unwrap()] {
            path.push(p);
        }
        let depths: Vec<usize> = path.iter().map(|&n| calls[n].depth).collect();
        let mut guides = vec![Guide::None; calls[i].depth];
        for (k, guide) in guides.iter_mut().enumerate() {
            let Some(at) = depths.iter().position(|&d| d == k + 1) else { continue };
            let node = path[at];
            let later = siblings(node).iter().any(|&s| s > node);
            *guide = match (node == i, later) {
                (true, true) => Guide::Tee,
                (true, false) => Guide::End,
                (false, true) => Guide::Through,
                (false, false) => Guide::None,
            };
        }
        calls[i].guides = guides;
    }
    for (position, item) in order.iter().enumerate() {
        match item {
            Ok(index) => tree.rows.push(Row::Call(calls[*index].clone())),
            Err((label, line)) => {
                let next = order[position + 1..].iter().find_map(|i| i.as_ref().ok().copied());
                let guides = next
                    .map(|n| calls[n].guides.iter().map(|g| if matches!(g, Guide::Tee | Guide::End) { Guide::Through } else { *g }).collect())
                    .unwrap_or_default();
                tree.rows.push(Row::Hop { label: label.clone(), guides, source_line: *line });
            }
        }
    }
    tree
}

pub fn repo_root(doc: &Path) -> PathBuf {
    let folder = if doc.is_dir() { doc.to_path_buf() } else { doc.parent().map(Path::to_path_buf).unwrap_or_default() };
    folder.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf).unwrap_or(folder)
}

pub fn check_tree(tree: &Tree, root: &Path) -> Vec<(usize, String)> {
    let mut out = tree.errors.clone();
    for row in &tree.rows {
        let Row::Call(call) = row else { continue };
        if call.external() {
            continue;
        }
        match std::fs::read_to_string(root.join(&call.path)) {
            Err(_) => out.push((call.source_line, format!("no such file in the repo: {}", call.path))),
            Ok(text) => {
                let count = text.lines().count();
                if call.line > count {
                    out.push((call.source_line, format!("{} has {count} lines, not {}", call.path, call.line)));
                }
            }
        }
    }
    out.sort();
    out
}

pub struct Block {
    pub fence_line: usize,
    pub source: String,
}

pub fn blocks(markdown: &str) -> Vec<Block> {
    use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
    let mut out = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (event, range) in Parser::new_ext(markdown, Options::all()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                let ours = info.split_whitespace().next().is_some_and(|w| w.eq_ignore_ascii_case(FENCE));
                open = ours.then(|| (range.start, String::new()));
            }
            Event::Text(text) => {
                if let Some((_, source)) = open.as_mut() {
                    source.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((start, source)) = open.take() {
                    out.push(Block { fence_line: markdown[..start].matches('\n').count() + 1, source });
                }
            }
            _ => {}
        }
    }
    out
}

pub fn check(markdown: &str, root: &Path) -> Vec<(usize, Vec<(usize, String)>)> {
    blocks(markdown)
        .into_iter()
        .map(|block| {
            let failures = check_tree(&parse(&block.source), root).into_iter().map(|(line, message)| (block.fence_line + line, message)).collect();
            (block.fence_line, failures)
        })
        .collect()
}

pub fn to_json(tree: &Tree, root: &Path) -> Value {
    let guides = |g: &[Guide]| g.iter().map(|g| g.name()).collect::<Vec<_>>();
    let rows: Vec<Value> = tree
        .rows
        .iter()
        .map(|row| match row {
            Row::Call(c) => json!({
                "kind": "call", "process": c.process, "depth": c.depth, "name": c.name, "note": c.note,
                "number": c.number, "laneStart": c.lane_start, "guides": guides(&c.guides), "sourceLine": c.source_line,
                "external": c.external(), "package": c.package(), "location": c.location(), "line": c.line,
                "path": if c.external() { c.path.clone() } else { root.join(&c.path).to_string_lossy().into_owned() },
            }),
            Row::Hop { label, guides: g, source_line } => json!({ "kind": "hop", "label": label, "guides": guides(g), "sourceLine": source_line }),
        })
        .collect();
    let errors: Vec<Value> = tree.errors.iter().map(|(line, message)| json!({ "line": line, "message": message })).collect();
    json!({ "processes": tree.processes, "rows": rows, "errors": errors })
}

#[no_mangle]
pub unsafe extern "C" fn kanna_calltree_parse(source: *const c_char, len: usize, doc: *const c_char, doc_len: usize) -> *mut c_char {
    let text = |p: *const c_char, n: usize| if p.is_null() { "" } else { std::str::from_utf8(std::slice::from_raw_parts(p.cast::<u8>(), n)).unwrap_or("") };
    let root = repo_root(Path::new(text(doc, doc_len)));
    let json = to_json(&parse(text(source, len)), &root);
    CString::new(json.to_string()).map(CString::into_raw).unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn kanna_calltree_free(json: *mut c_char) {
    if !json.is_null() {
        drop(CString::from_raw(json));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RENAME: &str = "kanna CLI\nmain          crates/kanna/src/lib.rs:17          kanna task name t12 \"New name\"\n  parse       crates/kanna/src/commands.rs:440    task name TASK NAME → request\n  wire        crates/kanna/src/lib.rs:316         task name with a TASK → tasks\n    -> socket {\"cmd\":\"tasks\",\"action\":\"name\"}\nkanna-daemon\n    Tasks::name    crates/kanna-daemon/src/tasks.rs:1043  finds the task on the Shelf\n      rename_pane  crates/kanna-mux/src/core.rs:1371      its held agent's pane\n      store        crates/kanna-daemon/src/tasks.rs:1067  on the reply: the new name\n";

    fn calls(tree: &Tree) -> Vec<&Call> {
        tree.rows.iter().filter_map(|r| if let Row::Call(c) = r { Some(c) } else { None }).collect()
    }

    #[test]
    fn a_tree_has_its_processes_calls_and_hop() {
        let tree = parse(RENAME);
        assert_eq!(tree.errors, vec![]);
        assert_eq!(tree.processes, ["kanna CLI", "kanna-daemon"]);
        let calls = calls(&tree);
        assert_eq!(calls.iter().map(|c| (c.name.as_str(), c.depth, c.process)).collect::<Vec<_>>(), [("main", 0, 0), ("parse", 1, 0), ("wire", 1, 0), ("Tasks::name", 2, 1), ("rename_pane", 3, 1), ("store", 3, 1)]);
        assert_eq!(calls[0].note, "kanna task name t12 \"New name\"");
        assert_eq!((calls[1].path.as_str(), calls[1].line), ("crates/kanna/src/commands.rs", 440));
        assert!(matches!(&tree.rows[3], Row::Hop { label, source_line: 5, .. } if label == r#"socket {"cmd":"tasks","action":"name"}"#));
    }

    #[test]
    fn calls_with_siblings_are_numbered() {
        let tree = parse(RENAME);
        let numbers: Vec<Option<usize>> = calls(&tree).iter().map(|c| c.number).collect();
        assert_eq!(numbers, [None, Some(1), Some(2), None, Some(1), Some(2)]);
    }

    #[test]
    fn a_stack_has_no_numbers() {
        let tree = parse("app\nrun  src/main.rs:1\n  step  src/a.rs:2\n    inner  src/b.rs:3\n");
        assert!(calls(&tree).iter().all(|c| c.number.is_none()));
    }

    #[test]
    fn guides_join_children_to_their_parent_through_the_hop() {
        let tree = parse(RENAME);
        let calls = calls(&tree);
        assert_eq!(calls[1].guides, [Guide::Tee]);
        assert_eq!(calls[2].guides, [Guide::End]);
        assert_eq!(calls[3].guides, [Guide::None, Guide::End]);
        assert_eq!(calls[4].guides, [Guide::None, Guide::None, Guide::Tee]);
        assert_eq!(calls[5].guides, [Guide::None, Guide::None, Guide::End]);
        let Row::Hop { guides, .. } = &tree.rows[3] else { panic!() };
        assert_eq!(guides, &[Guide::None, Guide::Through]);
    }

    #[test]
    fn lane_starts_follow_the_process_lines() {
        let starts: Vec<bool> = calls(&parse(RENAME)).iter().map(|c| c.lane_start).collect();
        assert_eq!(starts, [true, false, false, true, false, false]);
    }

    #[test]
    fn a_location_shows_its_package_and_file() {
        let tree = parse(RENAME);
        let c = calls(&tree)[3];
        assert_eq!((c.package(), c.location()), ("kanna-daemon".to_string(), "tasks.rs:1043".to_string()));
        let ext = parse("lldb\nmalloc  ~/.cargo/registry/src/x/tokio/src/rt.rs:40\n");
        assert!(calls(&ext)[0].external());
    }

    #[test]
    fn mistakes_are_reported_at_their_line() {
        let tree = parse("main  src/a.rs:1\napp\n      deep  src/b.rs:2\n  -> socket\n  next  src/c.rs:3\n  bad\n");
        let lines: Vec<usize> = tree.errors.iter().map(|(l, _)| *l).collect();
        assert_eq!(lines, [1, 3, 4, 6], "{:?}", tree.errors);
    }

    #[test]
    fn quotes_are_what_a_row_shows() {
        assert_eq!(quote("  parse       crates/kanna/src/commands.rs:440    task name"), Some("commands.rs:440".into()));
        assert_eq!(quote("kanna-daemon"), Some("kanna-daemon".into()));
        assert_eq!(quote("    -> socket {\"cmd\":\"tasks\"}"), Some("socket {\"cmd\":\"tasks\"}".into()));
        assert_eq!(quote("   "), None);
    }

    #[test]
    fn checking_finds_missing_files_and_lines() {
        let root = std::env::temp_dir().join(format!("kcalltree-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        let doc = "# T\n\n```calltree\napp\nmain  src/a.rs:2\n  gone  src/b.rs:1\n  long  src/a.rs:9\n  ext  /usr/lib/x.c:3\n```\n";
        let checked = check(doc, &root);
        assert_eq!(checked.len(), 1);
        assert_eq!(checked[0].0, 3);
        let lines: Vec<usize> = checked[0].1.iter().map(|(l, _)| *l).collect();
        assert_eq!(lines, [6, 7]);
        assert!(checked[0].1[0].1.contains("src/b.rs"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
