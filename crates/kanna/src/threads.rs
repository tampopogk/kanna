use crate::target::Session;
use kanna_client::{exit, fail, Failure};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

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
            update(&file, |doc| {
                let thread = thread_mut(doc, id.unwrap_or_default(), &file)?;
                if let Some(messages) = thread["messages"].as_array_mut() {
                    messages.push(json!({ "who": "agent", "text": text, "at": now() }));
                }
                thread["status"] = json!("replied");
                Ok(())
            })?;
            println!("replied to {}", id.unwrap_or_default());
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

pub fn repo_root(file: &Path) -> PathBuf {
    let start = if file.is_dir() { file.to_path_buf() } else { file.parent().map(Path::to_path_buf).unwrap_or_default() };
    start.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf).unwrap_or(start)
}

fn relative(file: &Path) -> String {
    let root = repo_root(file);
    file.strip_prefix(&root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| file.to_string_lossy().into_owned())
}

pub fn store_path(file: &Path) -> PathBuf {
    repo_root(file).join(".kanna/docs").join(format!("{}.json", relative(file).replace('/', "__")))
}

pub fn read(file: &Path) -> Value {
    let empty = json!({ "file": file, "agent": null, "next": 1, "threads": [] });
    read_no_follow(&store_path(file)).and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or(empty)
}

fn read_no_follow(path: &Path) -> Option<Vec<u8>> {
    let mut file = OpenOptions::new().read(true).custom_flags(libc_no_follow()).open(path).ok()?;
    let mut raw = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut raw).ok()?;
    Some(raw)
}

fn libc_no_follow() -> i32 {
    libc::O_NOFOLLOW
}

fn update(file: &Path, change: impl FnOnce(&mut Value) -> Result<(), Failure>) -> Result<(), Failure> {
    let path = store_path(file);
    let broken = |e: std::io::Error| fail(exit::FAILED, format!("markup: can't write {}: {e}", path.display()));
    let folder = path.parent().unwrap_or(Path::new("/"));
    for dir in [folder.parent().unwrap_or(folder), folder] {
        real_folder(dir).map_err(broken)?;
    }
    let lock = OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).custom_flags(libc_no_follow()).open(path.with_extension("json.lock")).map_err(broken)?;
    lock.lock().map_err(broken)?;
    let mut doc = read(file);
    change(&mut doc)?;
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut out = OpenOptions::new().write(true).create_new(true).mode(0o644).custom_flags(libc_no_follow()).open(&temporary).map_err(broken)?;
    out.write_all(serde_json::to_string_pretty(&doc).unwrap().as_bytes()).map_err(broken)?;
    drop(out);
    std::fs::rename(&temporary, &path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        broken(e)
    })?;
    File::unlock(&lock).map_err(broken)
}

fn real_folder(dir: &Path) -> std::io::Result<()> {
    match std::fs::create_dir(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    if std::fs::symlink_metadata(dir)?.file_type().is_dir() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("{} is not a plain folder; refusing to write through it", dir.display())))
    }
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

fn now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as libc::time_t).unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

fn with_lines(mut doc: Value) -> Value {
    let Some(source) = doc["file"].as_str().and_then(|f| std::fs::read_to_string(f).ok()) else { return doc };
    for thread in doc["threads"].as_array_mut().into_iter().flatten() {
        let line = thread["line"].as_i64().unwrap_or(1);
        let (found, detached) = if let Some(page) = thread.get("page").filter(|p| p.is_object()) {
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
    if let Some(page) = thread.get("page").filter(|p| p.is_object()) {
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
    let (first, quote) = block_at(&source, line).ok_or_else(|| fail(exit::USAGE, format!("comment: {path} has {count} lines")))?;
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
    let mut id = String::new();
    update(file, |doc| {
        let next = doc["next"].as_u64().unwrap_or(1);
        id = format!("c{next}");
        doc["next"] = json!(next + 1);
        if doc["agent"].is_null() && !agent.is_null() {
            doc["agent"] = agent.clone();
        }
        let thread = json!({ "id": id, "quote": quote, "line": first, "state": "open", "status": "replied", "messages": [{ "who": "agent", "text": text, "at": now() }] });
        match doc["threads"].as_array_mut() {
            Some(list) => list.push(thread),
            None => doc["threads"] = json!([thread]),
        }
        Ok(())
    })?;
    if json_output {
        println!("{}", json!({ "thread": id, "line": first, "pane": pane }));
    } else {
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        println!("{id} · {name}:{first} in {}", pane["id"].as_str().unwrap_or("?"));
    }
    Ok(())
}

pub fn block_at(source: &str, line: usize) -> Option<(usize, String)> {
    let lines: Vec<&str> = source.lines().collect();
    if line == 0 || line > lines.len() {
        return None;
    }
    let mut fenced = vec![false; lines.len()];
    let mut drawn = vec![false; lines.len()];
    let mut inside: Option<bool> = None;
    for (i, text) in lines.iter().enumerate() {
        let trimmed = text.trim_start();
        let fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        if fence && inside.is_none() {
            inside = Some(trimmed.trim_start_matches(['`', '~']).trim().starts_with("mermaid"));
        } else if fence {
            fenced[i] = true;
            drawn[i] = inside == Some(true);
            inside = None;
            continue;
        }
        fenced[i] = inside.is_some();
        drawn[i] = inside == Some(true);
    }
    let picture = |i: usize| drawn[i] || (lines[i].trim_start().starts_with("![") && lines[i].trim_end().ends_with(')'));
    let text = |i: usize| !lines[i].trim().is_empty() && !picture(i) && !(fenced[i] && lines[i].trim_start().starts_with(['`', '~']));
    let mut at = line - 1;
    if picture(at) {
        while at > 0 && !text(at) {
            at -= 1;
        }
    }
    while at < lines.len() && !text(at) {
        at += 1;
    }
    if at == lines.len() {
        at = line - 1;
        while at > 0 && !text(at) {
            at -= 1;
        }
    }
    let item = |text: &str| {
        let t = text.trim_start();
        t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") || t.split_once(". ").is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    let breaks = |text: &str| text.trim().is_empty() || text.trim_start().starts_with('#') || text.trim_start().starts_with('|');
    let (start, end) = if fenced[at] || lines[at].trim_start().starts_with('#') || lines[at].trim_start().starts_with('|') {
        (at, at)
    } else {
        let mut start = at;
        while start > 0 && !item(lines[start]) && !breaks(lines[start - 1]) && !fenced[start - 1] {
            start -= 1;
        }
        let mut end = at;
        while end + 1 < lines.len() && !breaks(lines[end + 1]) && !item(lines[end + 1]) && !fenced[end + 1] {
            end += 1;
        }
        (start, end)
    };
    let joined = lines[start..=end].iter().map(|l| strip(l)).collect::<Vec<_>>().join(" ");
    let plain = joined
        .split(|c: char| "*_`[]()<>|#~".contains(c))
        .map(str::trim)
        .filter(|part| part.chars().any(char::is_alphanumeric))
        .max_by_key(|part| part.chars().count())
        .unwrap_or(joined.trim())
        .to_string();
    Some((start + 1, plain))
}

fn strip(text: &str) -> &str {
    let t = text.trim_start().trim_start_matches('>').trim_start().trim_start_matches('#').trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(marker) {
            return rest.trim_start_matches("[ ] ").trim_start_matches("[x] ");
        }
    }
    match t.split_once(". ") {
        Some((n, rest)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => rest,
        _ => t.trim_end(),
    }
}

#[cfg(test)]
mod comment_tests {
    use super::block_at;

    #[test]
    fn a_line_comments_on_the_plain_words_of_its_block() {
        let doc = "# Tab Container\n\nA tab container is a slot in a split\nthat holds **several** panes.\n\n- Panes only: a container holds panes.\n- One active: one pane is shown,\n  the others keep running.\n\n```sh\nkanna open --in docs\n```\n";
        assert_eq!(block_at(doc, 1), Some((1, "Tab Container".into())));
        assert_eq!(block_at(doc, 4), Some((3, "A tab container is a slot in a split that holds".into())));
        assert_eq!(block_at(doc, 2), Some((3, "A tab container is a slot in a split that holds".into())));
        assert_eq!(block_at(doc, 8), Some((7, "One active: one pane is shown, the others keep running.".into())));
        assert_eq!(block_at(doc, 6), Some((6, "Panes only: a container holds panes.".into())));
        assert_eq!(block_at(doc, 11), Some((11, "kanna open --in docs".into())));
        assert_eq!(block_at(doc, 40), None);
        let pictures = "## How it fits\n\nThe keeper hands terminals over.\n\n```mermaid\nflowchart LR\n  A --> B\n```\n\n![The stack](stack.png)\n";
        assert_eq!(block_at(pictures, 6), Some((3, "The keeper hands terminals over.".into())));
        assert_eq!(block_at(pictures, 10), Some((3, "The keeper hands terminals over.".into())));
    }
}
