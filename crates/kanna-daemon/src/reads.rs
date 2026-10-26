use crate::files::{absolute, MAX_BYTES};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use kanna_mux::json::{string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const HOPS: usize = 32;

pub fn install(core: &CoreHandle) {
    let commands: [(&str, fn(&Value) -> Result<Map, KmuxError>); 4] = [("file.read", read), ("markup.post", post), ("markup.edit", edit), ("markup.agent", agent)];
    for (name, handler) in commands {
        core.add_command(name, Rc::new(move |args, responder| responder.reply(handler(&args))));
    }
}

fn bad(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

fn path_of(args: &Value) -> Result<String, KmuxError> {
    string(args, "path").filter(|p| !p.is_empty()).map(absolute).ok_or_else(|| bad("missing path"))
}

pub fn read(args: &Value) -> Result<Map, KmuxError> {
    let path = path_of(args)?;
    let real = resolve(&path)?;
    let meta = std::fs::metadata(&real).map_err(|_| KmuxError::new("not_found", format!("No such file: {path}")))?;
    let kind = if meta.is_file() {
        "file"
    } else if meta.is_dir() {
        "folder"
    } else {
        "other"
    };
    let mut out = Map::new();
    out.insert("path".into(), json!(real.to_string_lossy()));
    out.insert("type".into(), json!(kind));
    out.insert("size".into(), json!(meta.len()));
    if args.get("data") == Some(&Value::Bool(true)) && kind == "file" {
        let too_big = || KmuxError::new("too_big", format!("{path} is over {} MiB", MAX_BYTES >> 20));
        if meta.len() > MAX_BYTES as u64 {
            return Err(too_big());
        }
        let bytes = std::fs::read(&real).map_err(|_| KmuxError::new("not_found", format!("No such file: {path}")))?;
        if bytes.len() > MAX_BYTES {
            return Err(too_big());
        }
        out.insert("data".into(), json!(BASE64.encode(bytes)));
    }
    Ok(out)
}

pub fn resolve(path: &str) -> Result<PathBuf, KmuxError> {
    let not_local = || KmuxError::new("not_local", format!("{path} isn't on a local disk"));
    let missing = || KmuxError::new("not_found", format!("No such file: {path}"));
    let mut pending: Vec<String> = path.split('/').filter(|p| !p.is_empty()).map(String::from).collect();
    pending.reverse();
    let mut done: Vec<String> = vec![];
    let mut hops = 0;
    while let Some(part) = pending.pop() {
        match part.as_str() {
            "." => continue,
            ".." => {
                done.pop();
                continue;
            }
            _ => done.push(part),
        }
        if automount(&done) {
            return Err(not_local());
        }
        let current = PathBuf::from(format!("/{}", done.join("/")));
        let meta = std::fs::symlink_metadata(&current).map_err(|_| missing())?;
        if !meta.file_type().is_symlink() {
            continue;
        }
        hops += 1;
        let target = std::fs::read_link(&current).map_err(|_| missing())?;
        if hops > HOPS {
            return Err(missing());
        }
        done.pop();
        let target = target.to_string_lossy().into_owned();
        if target.starts_with('/') {
            done.clear();
        }
        pending.extend(target.split('/').filter(|p| !p.is_empty()).rev().map(String::from));
    }
    let real = PathBuf::from(format!("/{}", done.join("/")));
    if !local_disk(&real) {
        return Err(not_local());
    }
    Ok(real)
}

#[cfg(target_os = "macos")]
fn automount(parts: &[String]) -> bool {
    let mut rest = parts;
    if rest.len() >= 3 && rest[..3] == ["System", "Volumes", "Data"] {
        rest = &rest[3..];
    }
    if rest.first().map(String::as_str) == Some("private") {
        rest = &rest[1..];
    }
    rest.len() >= 2 && ["net", "Network", "home"].contains(&rest[0].as_str())
}

#[cfg(not(target_os = "macos"))]
fn automount(_: &[String]) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn local_disk(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(name) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    let mut info: libc::statfs = unsafe { std::mem::zeroed() };
    unsafe { libc::statfs(name.as_ptr(), &mut info) == 0 && info.f_flags & libc::MNT_LOCAL as u32 != 0 }
}

#[cfg(not(target_os = "macos"))]
fn local_disk(_: &Path) -> bool {
    true
}

fn failed(message: String) -> KmuxError {
    KmuxError::new("failed", format!("markup: {message}"))
}

fn answer(doc: &Path, mut out: Map) -> Result<Map, KmuxError> {
    out.insert("record".into(), kanna_threads::read(doc));
    Ok(out)
}

fn thread_mut<'a>(record: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    record["threads"].as_array_mut()?.iter_mut().find(|t| t["id"] == id)
}

pub fn post(args: &Value) -> Result<Map, KmuxError> {
    let doc = PathBuf::from(path_of(args)?);
    let mut thread = args.get("thread").filter(|t| t.is_object()).cloned().ok_or_else(|| bad("markup.post needs a thread"))?;
    let mark = match string(args, "mark") {
        Some(data) => Some(BASE64.decode(data).ok().filter(|png| png.starts_with(b"\x89PNG\r\n\x1a\n")).ok_or_else(|| bad("a mark is a base64 PNG"))?),
        None => None,
    };
    let mut id = String::new();
    kanna_threads::update(
        &doc,
        |record| {
            let next = record["next"].as_u64().unwrap_or(1);
            id = format!("c{next}");
            record["next"] = json!(next + 1);
            thread["id"] = json!(id);
            if let Some(png) = &mark {
                thread["mark"] = json!(kanna_threads::save_mark(&doc, &id, png).map_err(failed)?);
            }
            match record["threads"].as_array_mut() {
                Some(list) => list.push(thread),
                None => record["threads"] = json!([thread]),
            }
            Ok(())
        },
        failed,
    )?;
    let mut out = Map::new();
    out.insert("thread".into(), json!(id));
    answer(&doc, out)
}

pub fn edit(args: &Value) -> Result<Map, KmuxError> {
    let doc = PathBuf::from(path_of(args)?);
    let id = string(args, "id").filter(|id| kanna_threads::is_thread_id(id)).ok_or_else(|| bad("markup.edit needs a thread's id"))?.to_string();
    kanna_threads::update(
        &doc,
        |record| {
            let thread = thread_mut(record, &id).ok_or_else(|| KmuxError::new("not_found", format!("markup: no comment {id} in {}", kanna_threads::relative(&doc))))?;
            for key in ["state", "status", "error", "line"] {
                match args.get(key) {
                    Some(Value::Null) => {
                        thread.as_object_mut().map(|t| t.remove(key));
                    }
                    Some(value) => thread[key] = value.clone(),
                    None => {}
                }
            }
            if let Some(message) = args.get("message").filter(|m| m.is_object()) {
                match thread["messages"].as_array_mut() {
                    Some(list) => list.push(message.clone()),
                    None => thread["messages"] = json!([message]),
                }
            }
            Ok(())
        },
        failed,
    )?;
    answer(&doc, Map::new())
}

pub fn agent(args: &Value) -> Result<Map, KmuxError> {
    let doc = PathBuf::from(path_of(args)?);
    let agent = string(args, "agent").filter(|a| !a.is_empty()).ok_or_else(|| bad("markup.agent needs an agent"))?.to_string();
    kanna_threads::update(
        &doc,
        |record| {
            record["agent"] = json!(agent);
            Ok(())
        },
        failed,
    )?;
    answer(&doc, Map::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-reads-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::canonicalize(dir).unwrap()
    }

    #[test]
    fn a_file_is_read_with_its_real_path_and_type() {
        let dir = repo("read");
        std::fs::write(dir.join("a.png"), b"\x89PNG").unwrap();
        std::os::unix::fs::symlink("a.png", dir.join("link.png")).unwrap();
        let read = read(&json!({ "path": dir.join("link.png"), "data": true })).unwrap();
        assert_eq!(read["path"], json!(dir.join("a.png")));
        assert_eq!((read["type"].clone(), read["size"].clone()), (json!("file"), json!(4)));
        assert_eq!(BASE64.decode(read["data"].as_str().unwrap()).unwrap(), b"\x89PNG");
        let folder = super::read(&json!({ "path": dir, "data": true })).unwrap();
        assert_eq!((folder["type"].clone(), folder.get("data")), (json!("folder"), None));
        assert_eq!(super::read(&json!({ "path": dir.join("none.png") })).unwrap_err().code, "not_found");
        std::fs::write(dir.join("big"), vec![0u8; MAX_BYTES + 1]).unwrap();
        assert_eq!(super::read(&json!({ "path": dir.join("big"), "data": true })).unwrap_err().code, "too_big");
        assert!(super::read(&json!({ "path": dir.join("big") })).is_ok());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn automount_paths_are_refused_before_they_are_touched() {
        let dir = repo("away");
        std::os::unix::fs::symlink("/net/example.invalid/share", dir.join("away")).unwrap();
        for path in ["/net/example.invalid/a.png", "//net/example.invalid/a.png", "/./net/example.invalid/a.png", "/private/net/example.invalid/a.png", "/System/Volumes/Data/net/example.invalid/a.png", "/Network/Servers/example.invalid/a.png", "/home/someone/a.png"] {
            assert!(resolve(path).is_err(), "{path}");
        }
        assert!(resolve(&format!("{}/away/a.png", dir.display())).is_err());
        assert!(resolve(&format!("{}/../../../../../../../../net/example.invalid/a.png", dir.display())).is_err());
        assert_eq!(automount(&["net".into(), "example.invalid".into()]), true);
        assert_eq!(automount(&["System".into(), "Volumes".into(), "Data".into(), "private".into(), "home".into(), "u".into()]), true);
        assert_eq!(automount(&["Users".into(), "home".into()]), false);
    }

    #[test]
    fn threads_are_posted_edited_and_marked_as_the_cli_keeps_them() {
        let dir = repo("threads");
        let doc = dir.join("docs/spec.md");
        std::fs::create_dir_all(doc.parent().unwrap()).unwrap();
        std::fs::write(&doc, "# Spec\n").unwrap();
        let thread = json!({ "quote": "Spec", "line": 1, "state": "open", "status": "sent", "messages": [{ "who": "you", "text": "Why?", "at": "2026-10-26T10:00:00" }] });
        let posted = post(&json!({ "path": doc, "thread": thread, "mark": BASE64.encode(b"\x89PNG\r\n\x1a\nrest") })).unwrap();
        assert_eq!(posted["thread"], "c1");
        assert_eq!(posted["record"]["threads"][0]["mark"], ".kanna/docs/marks/docs__spec.md-c1.png");
        assert_eq!(std::fs::read(dir.join(".kanna/docs/marks/docs__spec.md-c1.png")).unwrap(), b"\x89PNG\r\n\x1a\nrest");
        assert_eq!(post(&json!({ "path": doc, "thread": thread })).unwrap()["thread"], "c2");
        let edited = edit(&json!({ "path": doc, "id": "c1", "status": "unsent", "error": "no agent", "message": { "who": "you", "text": "And?", "at": "x" } })).unwrap();
        assert_eq!(edited["record"]["threads"][0]["error"], "no agent");
        assert_eq!(edited["record"]["threads"][0]["messages"].as_array().unwrap().len(), 2);
        let cleared = edit(&json!({ "path": doc, "id": "c1", "error": null, "state": "resolved" })).unwrap();
        assert_eq!((cleared["record"]["threads"][0].get("error"), &cleared["record"]["threads"][0]["state"]), (None, &json!("resolved")));
        assert_eq!(edit(&json!({ "path": doc, "id": "c9", "state": "open" })).unwrap_err().code, "not_found");
        assert_eq!(agent(&json!({ "path": doc, "agent": "term-4" })).unwrap()["record"]["agent"], "term-4");
        assert_eq!(kanna_threads::read(&doc)["next"], 3);
        assert!(post(&json!({ "path": doc, "thread": thread, "mark": BASE64.encode(b"not a png") })).is_err());
        std::fs::remove_dir_all(dir.join(".kanna/docs/marks")).unwrap();
        std::fs::write(dir.join(".kanna/docs/marks"), "not a folder").unwrap();
        assert!(post(&json!({ "path": doc, "thread": thread, "mark": BASE64.encode(b"\x89PNG\r\n\x1a\nrest") })).is_err());
        assert_eq!(kanna_threads::read(&doc)["threads"].as_array().unwrap().len(), 2, "a thread whose mark wasn't saved isn't saved");
    }
}
