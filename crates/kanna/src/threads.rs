use crate::target::Session;
use kanna_client::{exit, fail, Failure};
pub use kanna_threads::{block_at, read, repo_root, store_path};
use kanna_threads::{add_agent_thread, now, read_no_follow, relative};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn update(file: &Path, change: impl FnOnce(&mut Value) -> Result<(), Failure>) -> Result<(), Failure> {
    kanna_threads::update(file, change, |message| fail(exit::FAILED, format!("markup: {message}")))
}

pub const ACTIONS: &[&str] = &["comments", "show", "reply", "resolve", "reopen"];

pub fn run(request: &Value, json_output: bool, callers: &[String]) -> Result<(), Failure> {
    let action = request["local"].as_str().unwrap_or_default();
    let id = request["id"].as_str();
    let file = find(request["doc"].as_str(), id, callers)?;
    match action {
        "comments" => {
            let doc = with_lines(read(&file));
            let all = request["all"] == true;
            let shown: Vec<&Value> = threads(&doc).iter().filter(|t| all || t["state"] == "open").collect();
            if json_output {
                println!("{}", serde_json::to_string_pretty(&shown).unwrap());
                return Ok(());
            }
            println!("{}", printable(&relative(&file)));
            for thread in &shown {
                println!("  {}", printable(&summary(thread)));
            }
            if shown.is_empty() {
                println!("  (no open comments)");
            }
        }
        "show" => {
            let doc = with_lines(read(&file));
            let thread = thread(&doc, id.unwrap_or_default(), &file)?;
            if json_output {
                println!("{}", serde_json::to_string_pretty(thread).unwrap());
            } else {
                println!("{}", printable(&show(thread, &relative(&file))));
            }
        }
        "reply" => {
            let text = request["text"].as_str().unwrap_or_default().to_string();
            let mut reopened = false;
            update(&file, |doc| {
                let thread = thread_mut(doc, id.unwrap_or_default(), &file)?;
                if let Some(messages) = thread["messages"].as_array_mut() {
                    messages.push(json!({ "who": "agent", "text": text, "at": now() }));
                }
                thread["status"] = json!("replied");
                reopened = thread["state"] == "resolved";
                thread["state"] = json!("open");
                Ok(())
            })?;
            let note = if reopened { ", reopened" } else { "" };
            println!("replied to {}{note}", id.unwrap_or_default());
        }
        "resolve" | "reopen" => {
            let state = if action == "resolve" { "resolved" } else { "open" };
            update(&file, |doc| {
                thread_mut(doc, id.unwrap_or_default(), &file)?["state"] = json!(state);
                Ok(())
            })?;
            println!("{} {state}", id.unwrap_or_default());
        }
        _ => return Err(fail(exit::USAGE, format!("markup: unknown action \"{action}\""))),
    }
    Ok(())
}

fn docs_in(root: &Path) -> Vec<Value> {
    let mut found: Vec<(std::time::SystemTime, Value)> = std::fs::read_dir(root.join(".kanna/docs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|e| e == "json"))
        .filter_map(|entry| {
            let raw = read_no_follow(&entry.path())?;
            let when = entry.metadata().and_then(|m| m.modified()).ok()?;
            let doc: Value = serde_json::from_slice(&raw).ok()?;
            let file = Path::new(doc["file"].as_str()?);
            (file.is_absolute() && store_path(file) == entry.path()).then_some((when, doc))
        })
        .collect();
    found.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
    found.into_iter().map(|(_, doc)| doc).collect()
}

pub fn find(doc: Option<&str>, id: Option<&str>, callers: &[String]) -> Result<PathBuf, Failure> {
    if let Some(doc) = doc {
        return Ok(PathBuf::from(crate::commands::absolute(doc)));
    }
    let here = std::env::current_dir().unwrap_or_default();
    let docs = docs_in(&repo_root(&here.join(".")));
    let mine: Vec<&Value> = docs.iter().filter(|d| d["agent"].as_str().is_some_and(|agent| callers.iter().any(|c| c == agent))).collect();
    for group in [mine, docs.iter().collect()] {
        for doc in group {
            if id.is_none_or(|id| threads(doc).iter().any(|t| t["id"] == id)) {
                if let Some(file) = doc["file"].as_str() {
                    return Ok(PathBuf::from(file));
                }
            }
        }
    }
    let with = id.map(|id| format!(" with comment {id}")).unwrap_or_default();
    Err(fail(exit::NOT_FOUND, format!("markup: no doc{with} here; pass --doc FILE")))
}

pub fn open_threads(all: bool) -> Vec<(String, String)> {
    let here = std::env::current_dir().unwrap_or_default();
    let pane = crate::commands::caller();
    let docs = docs_in(&repo_root(&here.join(".")));
    let mine: Vec<&Value> = docs.iter().filter(|d| pane.is_some() && d["agent"].as_str() == pane.as_deref()).collect();
    let chosen = if mine.is_empty() { docs.iter().collect() } else { mine };
    let mut out = Vec::new();
    for doc in chosen {
        for thread in threads(doc).iter().filter(|t| all || t["state"] == "open") {
            let id = thread["id"].as_str().unwrap_or_default().to_string();
            if !out.iter().any(|(have, _)| *have == id) {
                out.push((printable(&id), printable(&short(&what(thread), 50))));
            }
        }
    }
    out
}

fn threads(doc: &Value) -> &[Value] {
    doc["threads"].as_array().map(Vec::as_slice).unwrap_or_default()
}

fn thread<'a>(doc: &'a Value, id: &str, file: &Path) -> Result<&'a Value, Failure> {
    threads(doc).iter().find(|t| t["id"] == id).ok_or_else(|| missing(id, file))
}

fn thread_mut<'a>(doc: &'a mut Value, id: &str, file: &Path) -> Result<&'a mut Value, Failure> {
    doc["threads"].as_array_mut().and_then(|list| list.iter_mut().find(|t| t["id"] == id)).ok_or_else(|| missing(id, file))
}

fn missing(id: &str, file: &Path) -> Failure {
    fail(exit::NOT_FOUND, format!("markup: no comment {id} in {}", relative(file)))
}

fn with_lines(mut doc: Value) -> Value {
    let Some(source) = doc["file"].as_str().and_then(|f| std::fs::read_to_string(f).ok()) else { return doc };
    for thread in doc["threads"].as_array_mut().into_iter().flatten() {
        let line = thread["line"].as_i64().unwrap_or(1);
        let (found, detached) = if let Some(ink) = thread.get("ink").filter(|i| i.is_object()) {
            (line_of(&source, ink["marks"][0]["anchor"]["text"].as_str().unwrap_or_default(), line).0, false)
        } else if let Some(page) = thread.get("page").filter(|p| p.is_object()) {
            (line_of(&source, page["text"].as_str().unwrap_or_default(), line).0, false)
        } else if let Some(image) = thread["image"].as_str() {
            image_line(&source, image, line)
        } else {
            line_of(&source, thread["quote"].as_str().unwrap_or_default(), line)
        };
        thread["line"] = json!(found);
        thread["detached"] = json!(detached);
    }
    doc
}

pub fn line_of(source: &str, quote: &str, near: i64) -> (i64, bool) {
    let words: Vec<&str> = quote.split_whitespace().collect();
    for probe in [quote.to_string(), words.iter().take(3).copied().collect::<Vec<_>>().join(" "), words.first().copied().unwrap_or_default().to_string()] {
        if probe.is_empty() {
            continue;
        }
        let hits: Vec<i64> = source.match_indices(probe.as_str()).map(|(at, _)| source[..at].matches('\n').count() as i64 + 1).collect();
        if let Some(best) = hits.into_iter().min_by_key(|n| (n - near).abs()) {
            return (best, false);
        }
    }
    (near, true)
}

fn image_line(source: &str, image: &str, near: i64) -> (i64, bool) {
    let lines: Vec<i64> = source
        .lines()
        .enumerate()
        .filter(|(_, text)| text.contains(&format!("]({image})")) || text.contains(&format!("]({image} ")))
        .map(|(n, _)| n as i64 + 1)
        .collect();
    lines.into_iter().min_by_key(|n| (n - near).abs()).map_or((near, true), |n| (n, false))
}

pub fn printable(text: &str) -> String {
    text.chars().filter(|c| *c == '\n' || !c.is_control()).collect()
}

fn what(thread: &Value) -> String {
    if let Some(marks) = thread["ink"]["marks"].as_array() {
        let kinds: Vec<&str> = marks.iter().map(|m| m["kind"].as_str().unwrap_or("writing")).collect();
        return format!("ink, {} mark{}: {}", marks.len(), if marks.len() == 1 { "" } else { "s" }, kinds.join(", "));
    }
    match thread.get("page").filter(|p| p.is_object()) {
        Some(page) => describe_page(page),
        None => match thread["image"].as_str() {
            Some(image) => format!("image {image}, {}", describe_region(&thread["region"])),
            None => thread["quote"].as_str().unwrap_or_default().to_string(),
        },
    }
}

fn short(text: &str, cap: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(cap) {
        Some((at, _)) => format!("{}…", &flat[..at]),
        None => flat,
    }
}

pub fn summary(thread: &Value) -> String {
    let id = thread["id"].as_str().unwrap_or("?");
    let place = if thread["detached"] == true { "detached".to_string() } else { format!("line {}", thread["line"].as_i64().unwrap_or(1)) };
    let replied = thread["messages"].as_array().and_then(|m| m.last()).is_some_and(|m| m["who"] == "agent");
    format!("{id}  {place}  {}{}  “{}”", thread["state"].as_str().unwrap_or("open"), if replied { "  (agent replied)" } else { "" }, short(&what(thread), 50))
}

pub fn show(thread: &Value, file: &str) -> String {
    let mut out = format!(
        "{} · {file}:{} · {}{}",
        thread["id"].as_str().unwrap_or("?"),
        thread["line"].as_i64().unwrap_or(1),
        thread["state"].as_str().unwrap_or("open"),
        if thread["detached"] == true { " · detached: the quote is no longer in the file" } else { "" }
    );
    let mark = thread["mark"].as_str().map(|m| format!("\n> marked up: {m}")).unwrap_or_default();
    if let Some(ink) = thread.get("ink").filter(|i| i.is_object()) {
        for line in ink_lines(ink) {
            out += &format!("\n{line}");
        }
        out += &mark;
    } else if let Some(page) = thread.get("page").filter(|p| p.is_object()) {
        let n = |i: usize| page["box"][i].as_i64().unwrap_or(0);
        out += &format!("\n> {}\n> box {},{} to {},{} px{mark}", describe_page(page), n(0), n(1), n(0) + n(2), n(1) + n(3));
    } else if let Some(image) = thread["image"].as_str() {
        out += &format!("\n> image {image}, {}{mark}", describe_region(&thread["region"]));
    } else {
        for line in thread["quote"].as_str().unwrap_or_default().lines() {
            out += &format!("\n> {line}");
        }
    }
    for message in thread["messages"].as_array().into_iter().flatten() {
        out += &format!("\n{}: {}", message["who"].as_str().unwrap_or("?"), message["text"].as_str().unwrap_or_default());
    }
    out
}

fn describe_page(page: &Value) -> String {
    let text = |key: &str| page[key].as_str().unwrap_or_default().to_string();
    let (hash, selector, quote) = (text("hash"), text("selector"), text("text"));
    let place = if hash.is_empty() { "the page".to_string() } else { format!("the page at {hash}") };
    if !selector.is_empty() && page["selection"] == true {
        return format!("{place}, the text \"{quote}\" in {selector}");
    }
    if !selector.is_empty() {
        return format!("{place}, element {selector}{}", if quote.is_empty() { String::new() } else { format!(" \"{quote}\"") });
    }
    format!("{place}, a box")
}

pub fn ink_lines(ink: &Value) -> Vec<String> {
    let marks = ink["marks"].as_array().map(Vec::as_slice).unwrap_or_default();
    let mut lines = Vec::new();
    if ink["view"].is_object() {
        lines.push(format!("> {}", describe_view(&ink["view"])));
    }
    lines.push(format!("> ink, {} mark{}, numbered in the picture:", marks.len(), if marks.len() == 1 { "" } else { "s" }));
    for (n, mark) in marks.iter().enumerate() {
        lines.push(format!("> {} {}", n + 1, describe_mark(mark, ink["view"].is_object())));
        let note = flat(mark["note"].as_str().unwrap_or_default(), 400);
        if !note.is_empty() {
            lines.push(format!(">   note: {note}"));
        }
    }
    lines
}

fn describe_view(view: &Value) -> String {
    let hash = flat(view["hash"].as_str().unwrap_or_default(), 100);
    let place = if hash.is_empty() { "the page".to_string() } else { format!("the page at {hash}") };
    let n = |key: &str, i: usize| view[key][i].as_i64();
    match (n("viewport", 0), n("viewport", 1), n("scroll", 0), n("scroll", 1)) {
        (Some(w), Some(h), Some(x), Some(y)) => format!("{place}, in a {w}×{h} view, scrolled to {x},{y}"),
        _ => place,
    }
}

fn describe_mark(mark: &Value, page: bool) -> String {
    let kind = mark["kind"].as_str().filter(|k| ["circle", "underline", "strike", "cross", "arrow", "writing"].contains(k)).unwrap_or("writing");
    let verb = match kind {
        "circle" => Some("circle around"),
        "underline" => Some("underline under"),
        "strike" => Some("strike through"),
        "cross" => Some("cross over"),
        _ => None,
    };
    let words = flat(mark["words"].as_str().unwrap_or_default(), 120);
    let anchor = &mark["anchor"];
    let to = Some(&mark["to"]).filter(|t| kind == "arrow" && t.is_object());
    let side = if mark["side"] == "before" { "before" } else { "after" };
    let text = |place: &Value, cap: usize| flat(place["text"].as_str().unwrap_or_default(), cap);
    if page {
        let element = |place: &Value| format!("element {} \"{}\"", flat(place["selector"].as_str().unwrap_or_default(), 200), text(place, 120));
        if let (Some(verb), false) = (verb, words.is_empty()) {
            let quoted = if words == text(anchor, 120) { String::new() } else { format!(", the text \"{words}\"") };
            return format!("{verb} {}{quoted}{}", element(anchor), at(box_of(&anchor["box"])));
        }
        if let Some(to) = to {
            return format!("arrow from {} to {side} {}", element(anchor), element(to));
        }
        let read = if kind == "writing" { ", read it in the picture" } else { "" };
        return format!("{kind} near {}{}{read}", element(anchor), at(bounds(mark)));
    }
    let line = |place: &Value| format!("line {}", place["line"].as_i64().unwrap_or(1));
    if let (Some(verb), false) = (verb, words.is_empty()) {
        return format!("{verb} \"{words}\" ({})", line(anchor));
    }
    if let Some(to) = to {
        return format!("arrow from \"{}\" ({}) to {side} \"{}\" ({})", trim(&text(anchor, usize::MAX)), line(anchor), trim(&text(to, usize::MAX)), line(to));
    }
    if kind == "writing" {
        return format!("writing beside {}, read it in the picture", line(anchor));
    }
    format!("{kind} at {}, around nothing", line(anchor))
}

fn box_of(value: &Value) -> Option<[i64; 4]> {
    let values = value.as_array().filter(|v| v.len() == 4)?;
    let mut out = [0; 4];
    for (slot, v) in out.iter_mut().zip(values) {
        *slot = v.as_i64()?;
    }
    Some(out)
}

fn bounds(mark: &Value) -> Option<[i64; 4]> {
    let origin = box_of(&mark["anchor"]["box"])?;
    let points: Vec<(f64, f64)> = mark["strokes"].as_array()?.iter().flat_map(|s| s.as_array().cloned().unwrap_or_default()).filter_map(|p| Some((p[0].as_f64()?, p[1].as_f64()?))).collect();
    let fold = |pick: fn(&(f64, f64)) -> f64, min: bool| points.iter().map(pick).fold(if min { f64::MAX } else { f64::MIN }, if min { f64::min } else { f64::max });
    if points.is_empty() {
        return None;
    }
    let (left, top) = ((origin[0] as f64 + fold(|p| p.0, true)).round() as i64, (origin[1] as f64 + fold(|p| p.1, true)).round() as i64);
    let (right, bottom) = ((origin[0] as f64 + fold(|p| p.0, false)).round() as i64, (origin[1] as f64 + fold(|p| p.1, false)).round() as i64);
    Some([left, top, right - left, bottom - top])
}

fn at(rect: Option<[i64; 4]>) -> String {
    rect.map(|b| format!(" at {},{} to {},{} px", b[0], b[1], b[0] + b[2], b[1] + b[3])).unwrap_or_default()
}

fn flat(text: &str, cap: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().map(|c| if c.is_control() || c == '\u{2028}' || c == '\u{2029}' { '?' } else { c }).collect();
    match one.char_indices().nth(cap.saturating_sub(1)) {
        Some((at, _)) if one.chars().count() > cap => format!("{}…", &one[..at]),
        _ => one,
    }
}

fn trim(text: &str) -> String {
    if text.chars().count() <= 34 {
        return text.to_string();
    }
    let cut: String = text.chars().take(33).collect();
    format!("{}…", cut.trim_end_matches(|c: char| c.is_whitespace() || ",;:.".contains(c)))
}

fn describe_region(region: &Value) -> String {
    let Some(values) = region.as_array().filter(|v| v.len() == 4) else { return "the whole image".into() };
    let p: Vec<i64> = values.iter().map(|v| (v.as_f64().unwrap_or(0.0) * 100.0).round() as i64).collect();
    format!("the box from {}%,{}% to {}%,{}% (left,top to right,bottom)", p[0], p[1], p[0] + p[2], p[1] + p[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-threads-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("docs/specs")).unwrap();
        dir
    }

    #[test]
    fn threads_live_beside_the_repo_as_the_app_keeps_them() {
        let dir = repo("store");
        let file = dir.join("docs/specs/desk.md");
        assert_eq!(store_path(&file), dir.join(".kanna/docs/docs__specs__desk.md.json"));
        assert_eq!(relative(&file), "docs/specs/desk.md");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replying_and_resolving_keep_every_other_field() {
        let dir = repo("reply");
        let file = dir.join("docs/specs/desk.md");
        std::fs::write(&file, "# Desk\n\nThe Shelf holds the rest.\n").unwrap();
        std::fs::create_dir_all(dir.join(".kanna/docs")).unwrap();
        let saved = json!({ "file": file, "agent": "p3", "next": 2, "custom": 7, "threads": [
            { "id": "c1", "quote": "The Shelf holds", "line": 9, "state": "open", "status": "sent", "extra": true,
              "messages": [{ "who": "you", "text": "Why?", "at": "2026-10-11T09:00:00" }] }
        ] });
        std::fs::write(store_path(&file), saved.to_string()).unwrap();
        run(&json!({ "local": "reply", "id": "c1", "text": "Because.", "doc": file }), false, &[]).unwrap();
        run(&json!({ "local": "resolve", "id": "c1", "doc": file }), false, &[]).unwrap();
        let doc = read(&file);
        let thread = &doc["threads"][0];
        assert_eq!((doc["custom"].clone(), thread["extra"].clone()), (json!(7), json!(true)));
        assert_eq!((thread["state"].clone(), thread["status"].clone()), (json!("resolved"), json!("replied")));
        assert_eq!(thread["messages"][1]["who"], "agent");
        assert_eq!(thread["messages"][1]["text"], "Because.");
        let shown = with_lines(read(&file));
        assert_eq!(shown["threads"][0]["line"], 3);
        assert!(show(&shown["threads"][0], "docs/specs/desk.md").starts_with("c1 · docs/specs/desk.md:3 · resolved\n> The Shelf holds\nyou: Why?\nagent: Because."));
        assert!(run(&json!({ "local": "reply", "id": "c9", "text": "x", "doc": file }), false, &[]).is_err());
        run(&json!({ "local": "reply", "id": "c1", "text": "Done.", "doc": file }), false, &[]).unwrap();
        let thread = &read(&file)["threads"][0];
        assert_eq!((thread["state"].clone(), thread["status"].clone()), (json!("open"), json!("replied")));
        assert_eq!(thread["messages"][2]["text"], "Done.");
        run(&json!({ "local": "resolve", "id": "c1", "doc": file }), false, &[]).unwrap();
        let ink = json!({ "marks": [{ "kind": "strike", "strokes": [[[0, 4], [90, 5]]], "anchor": { "line": 9, "text": "The Shelf holds the rest." }, "words": "the rest." }] });
        update(&file, |doc| {
            doc["threads"].as_array_mut().unwrap().push(json!({ "id": "c2", "quote": "", "line": 9, "state": "open", "status": "sent", "messages": [], "ink": ink }));
            Ok(())
        })
        .unwrap();
        run(&json!({ "local": "reply", "id": "c2", "text": "Cut it.", "doc": file }), false, &[]).unwrap();
        run(&json!({ "local": "resolve", "id": "c2", "doc": file }), false, &[]).unwrap();
        let shown = with_lines(read(&file));
        let thread = &shown["threads"][1];
        assert_eq!((thread["ink"].clone(), thread["state"].clone(), thread["line"].clone()), (ink, json!("resolved"), json!(3)));
        assert!(show(thread, "docs/specs/desk.md").ends_with("> 1 strike through \"the rest.\" (line 9)\nagent: Cut it."));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_thread_file_counts_only_for_the_doc_it_is_named_for_and_never_writes_through_links() {
        let dir = repo("trust");
        let file = dir.join("docs/specs/desk.md");
        std::fs::write(&file, "# Desk\n").unwrap();
        std::fs::create_dir_all(dir.join(".kanna/docs")).unwrap();
        let elsewhere = dir.join("elsewhere.md");
        std::fs::write(dir.join(".kanna/docs/docs__specs__desk.md.json"), json!({ "file": elsewhere, "threads": [{ "id": "c1", "state": "open", "messages": [] }] }).to_string()).unwrap();
        assert!(docs_in(&dir).is_empty());
        std::fs::write(store_path(&file), json!({ "file": file, "threads": [{ "id": "c1", "state": "open", "messages": [] }] }).to_string()).unwrap();
        assert_eq!(docs_in(&dir).len(), 1);
        let linked = repo("linked");
        let target = linked.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, linked.join(".kanna")).unwrap();
        let doc = linked.join("docs/specs/desk.md");
        std::fs::write(&doc, "# Desk\n").unwrap();
        assert!(run(&json!({ "local": "resolve", "id": "c1", "doc": doc }), false, &[]).is_err() || !target.join("docs").exists());
        assert!(!target.join("docs").exists());
        assert_eq!(printable("ok\u{1b}]52;c;x\u{7}\nnext\u{9b}2J"), "ok]52;c;x\nnext2J");
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&linked).unwrap();
    }

    #[test]
    fn ink_threads_show_their_marks_as_the_agent_got_them() {
        let place = |line: i64, text: &str| json!({ "line": line, "text": text });
        let panes = place(7, "Panes only: a container holds panes, not splits or other containers.");
        let thread = json!({ "id": "c2", "quote": "", "line": 7, "state": "open", "mark": ".kanna/docs/marks/docs__specs__tab-container.md-c2.png", "messages": [],
            "ink": { "marks": [
                { "kind": "circle", "strokes": [[[2, 1], [80, 0]]], "anchor": panes, "words": "Panes only:" },
                { "kind": "writing", "strokes": [[[96, 0]]], "anchor": panes },
                { "kind": "arrow", "strokes": [[[-12, 4], [-10, -390]]], "anchor": place(23, "The panes inside stay ordinary panes, with their own IDs."),
                  "to": place(3, "A tab container is a slot in a split that holds several panes."), "side": "after", "tip": 1, "note": "say this up front" },
                { "kind": "strike", "strokes": [[[180, 8], [360, 9]]], "anchor": place(14, "Click a tab to show it; middle-click or × to close it."), "words": "middle-click or × to close it." }
            ] } });
        assert_eq!(show(&thread, "docs/specs/tab-container.md"), "c2 · docs/specs/tab-container.md:7 · open
> ink, 4 marks, numbered in the picture:
> 1 circle around \"Panes only:\" (line 7)
> 2 writing beside line 7, read it in the picture
> 3 arrow from \"The panes inside stay ordinary pa…\" (line 23) to after \"A tab container is a slot in a sp…\" (line 3)
>   note: say this up front
> 4 strike through \"middle-click or × to close it.\" (line 14)
> marked up: .kanna/docs/marks/docs__specs__tab-container.md-c2.png");
        assert_eq!(what(&thread), "ink, 4 marks: circle, writing, arrow, strike");
        let page = json!({ "view": { "hash": "", "viewport": [1220, 816], "scroll": [0, 40] }, "marks": [
            { "kind": "writing", "strokes": [[[620, 4], [640, 12]], [[630, 20], [650, 24]]],
              "anchor": { "line": 15, "text": "Name Jeremy Hale", "selector": "#settings .row:nth-of-type(1)", "box": [40, 100, 600, 40] } }
        ] });
        assert_eq!(ink_lines(&page), [
            "> the page, in a 1220×816 view, scrolled to 0,40",
            "> ink, 1 mark, numbered in the picture:",
            "> 1 writing near element #settings .row:nth-of-type(1) \"Name Jeremy Hale\" at 660,104 to 690,124 px, read it in the picture",
        ]);
    }

    #[test]
    fn pages_and_images_are_described_as_the_agent_saw_them() {
        let page = json!({ "page": { "selector": "#ring", "text": "Simulate", "hash": "", "box": [165, 93, 158, 25] }, "id": "c11", "line": 143, "state": "open", "messages": [], "mark": ".kanna/docs/marks/x.png" });
        assert_eq!(show(&page, "desk.html"), "c11 · desk.html:143 · open\n> the page, element #ring \"Simulate\"\n> box 165,93 to 323,118 px\n> marked up: .kanna/docs/marks/x.png");
        let image = json!({ "image": "desk.png", "region": [0.05, 0.2, 0.25, 0.3] });
        assert_eq!(what(&image), "image desk.png, the box from 5%,20% to 30%,50% (left,top to right,bottom)");
        assert_eq!(now().len(), "2026-10-11T09:00:00".len());
    }
}

pub fn comment(request: &Value, mux: &mut Session, json_output: bool) -> Result<(), Failure> {
    let path = request["path"].as_str().unwrap_or_default();
    let file = Path::new(path);
    let source = std::fs::read_to_string(file).map_err(|_| fail(exit::NOT_FOUND, format!("No such file: {path}")))?;
    let line = request["line"].as_u64().unwrap_or(1) as usize;
    let count = source.lines().count();
    block_at(&source, line).ok_or_else(|| fail(exit::USAGE, format!("comment: {path} has {count} lines")))?;
    let text = request["text"].as_str().unwrap_or_default().to_string();
    let agent = request["agent"].clone();
    let listed = mux.call("list", json!({}))?;
    let shown = listed["panes"].as_array().into_iter().flatten().find(|p| p["type"] == "md" && p["path"] == path).cloned();
    let pane = match shown {
        Some(pane) if pane["markup"] == true && (agent.is_null() || pane["agent"] == agent) => pane,
        Some(pane) => {
            let mut args = json!({ "pane": pane["id"] });
            if !agent.is_null() {
                args["agent"] = agent.clone();
            }
            mux.call("markup", args)?["pane"].clone()
        }
        None => {
            let mut args = json!({ "type": "md", "path": path, "markup": true });
            for key in ["agent", "from", "name", "split", "size", "tab", "window", "wait"] {
                if !request[key].is_null() {
                    args[key] = request[key].clone();
                }
            }
            mux.call("open", args)?["pane"].clone()
        }
    };
    let (id, first) = add_agent_thread(file, line, &text, agent.as_str()).map_err(|message| fail(exit::FAILED, format!("comment: {message}")))?;
    if json_output {
        println!("{}", json!({ "thread": id, "line": first, "pane": pane }));
    } else {
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        println!("{id} · {name}:{first} in {}", pane["id"].as_str().unwrap_or("?"));
    }
    Ok(())
}

