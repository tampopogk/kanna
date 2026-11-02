use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub fn repo_root(file: &Path) -> PathBuf {
    let start = if file.is_dir() { file.to_path_buf() } else { file.parent().map(Path::to_path_buf).unwrap_or_default() };
    start.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf).unwrap_or(start)
}

pub fn relative(file: &Path) -> String {
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

pub fn read_no_follow(path: &Path) -> Option<Vec<u8>> {
    let mut file = OpenOptions::new().read(true).custom_flags(libc_no_follow()).open(path).ok()?;
    let mut raw = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut raw).ok()?;
    Some(raw)
}

fn libc_no_follow() -> i32 {
    libc::O_NOFOLLOW
}

pub fn update<E>(file: &Path, change: impl FnOnce(&mut Value) -> Result<(), E>, broken: impl Fn(String) -> E) -> Result<(), E> {
    let path = store_path(file);
    let broken = |e: std::io::Error| broken(format!("can't write {}: {e}", path.display()));
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

pub fn is_thread_id(id: &str) -> bool {
    id.len() > 1 && id.len() < 12 && id.starts_with('c') && id[1..].bytes().all(|b| b.is_ascii_digit())
}

pub fn mark_path(file: &Path, id: &str) -> PathBuf {
    let store = store_path(file);
    let stem = store.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    store.with_file_name("marks").join(format!("{stem}-{id}.png"))
}

pub fn save_mark(file: &Path, id: &str, png: &[u8]) -> Result<String, String> {
    if !is_thread_id(id) || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("a marked-up copy is a PNG of a thread".into());
    }
    let target = mark_path(file, id);
    let broken = |e: std::io::Error| format!("can't write {}: {e}", target.display());
    let marks = target.parent().unwrap_or(Path::new("/"));
    let docs = marks.parent().unwrap_or(marks);
    for dir in [docs.parent().unwrap_or(docs), docs, marks] {
        real_folder(dir).map_err(broken)?;
    }
    let temporary = target.with_extension(format!("png.{}.tmp", std::process::id()));
    let mut out = OpenOptions::new().write(true).create_new(true).mode(0o644).custom_flags(libc_no_follow()).open(&temporary).map_err(broken)?;
    out.write_all(png).map_err(broken)?;
    drop(out);
    std::fs::rename(&temporary, &target).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        broken(e)
    })?;
    let root = repo_root(file);
    Ok(target.strip_prefix(&root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| target.to_string_lossy().into_owned()))
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

pub fn now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as libc::time_t).unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

pub fn block_at(source: &str, line: usize) -> Option<(usize, String)> {
    let lines: Vec<&str> = source.lines().collect();
    if line == 0 || line > lines.len() {
        return None;
    }
    let mut fenced = vec![false; lines.len()];
    let mut drawn = vec![false; lines.len()];
    let mut tree = vec![false; lines.len()];
    let mut inside: Option<&str> = None;
    for (i, text) in lines.iter().enumerate() {
        let trimmed = text.trim_start();
        let fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        if fence && inside.is_none() {
            let info = trimmed.trim_start_matches(['`', '~']).trim();
            inside = Some(if info.starts_with("mermaid") { "mermaid" } else if info.starts_with(kanna_calltree::FENCE) { kanna_calltree::FENCE } else { "" });
        } else if fence {
            fenced[i] = true;
            drawn[i] = inside == Some("mermaid");
            inside = None;
            continue;
        }
        fenced[i] = inside.is_some();
        drawn[i] = inside == Some("mermaid");
        tree[i] = inside == Some(kanna_calltree::FENCE) && kanna_calltree::quote(text).is_some();
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
    if tree[at] {
        return Some((at + 1, kanna_calltree::quote(lines[at]).unwrap_or_default()));
    }
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

pub fn add_agent_thread(file: &Path, line: usize, text: &str, agent: Option<&str>) -> Result<(String, usize), String> {
    let source = std::fs::read_to_string(file).map_err(|_| format!("No such file: {}", file.display()))?;
    let (first, quote) = block_at(&source, line).ok_or_else(|| format!("{} has {} lines", file.display(), source.lines().count()))?;
    let mut id = String::new();
    update(
        file,
        |doc| {
            let next = doc["next"].as_u64().unwrap_or(1);
            id = format!("c{next}");
            doc["next"] = json!(next + 1);
            if let (true, Some(agent)) = (doc["agent"].is_null(), agent) {
                doc["agent"] = json!(agent);
            }
            let thread = json!({ "id": id, "quote": quote, "line": first, "state": "open", "status": "replied", "messages": [{ "who": "agent", "text": text, "at": now() }] });
            match doc["threads"].as_array_mut() {
                Some(list) => list.push(thread),
                None => doc["threads"] = json!([thread]),
            }
            Ok(())
        },
        |message| message,
    )?;
    Ok((id, first))
}

#[cfg(test)]
mod comment_tests {
    use super::*;

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
        let calls = "## Rename\n\n```calltree\nkanna CLI\nmain  crates/kanna/src/lib.rs:17  the entry\n  -> socket tasks\nkanna-daemon\n  name  crates/kanna-daemon/src/tasks.rs:1043\n```\n";
        assert_eq!(block_at(calls, 5), Some((5, "lib.rs:17".into())));
        assert_eq!(block_at(calls, 6), Some((6, "socket tasks".into())));
        assert_eq!(block_at(calls, 7), Some((7, "kanna-daemon".into())));
        assert_eq!(block_at(calls, 8), Some((8, "tasks.rs:1043".into())));
        let pictures = "## How it fits\n\nThe keeper hands terminals over.\n\n```mermaid\nflowchart LR\n  A --> B\n```\n\n![The stack](stack.png)\n";
        assert_eq!(block_at(pictures, 6), Some((3, "The keeper hands terminals over.".into())));
        assert_eq!(block_at(pictures, 10), Some((3, "The keeper hands terminals over.".into())));
    }
}

#[cfg(test)]
mod thread_tests {
    use super::*;

    #[test]
    fn the_agent_starts_a_thread_on_the_block_at_a_line() {
        let dir = std::env::temp_dir().join(format!("kanna-threads-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let doc = dir.join("doc.md");
        std::fs::write(&doc, "# Doc\n\nThe keeper hands\nterminals over.\n").unwrap();
        assert_eq!(add_agent_thread(&doc, 4, "The whole trip.", Some("term-3")), Ok(("c1".into(), 3)));
        assert_eq!(add_agent_thread(&doc, 1, "The title.", Some("term-9")), Ok(("c2".into(), 1)));
        let saved = read(&doc);
        assert_eq!((saved["agent"].clone(), saved["next"].clone()), (json!("term-3"), json!(3)));
        assert_eq!(saved["threads"][0]["quote"], "The keeper hands terminals over.");
        assert_eq!(saved["threads"][0]["messages"][0]["text"], "The whole trip.");
        assert!(add_agent_thread(&doc, 40, "Past.", None).unwrap_err().ends_with("has 4 lines"));
        assert!(add_agent_thread(&dir.join("gone.md"), 1, "Gone.", None).unwrap_err().starts_with("No such file"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
