use crate::builds::Callers;
use kanna_guest::client::{self, Pipe};
use kanna_guest::frame;
use kanna_guest::intake::Intake;
use kanna_guest::manifest;
use kanna_guest::supervisor::send_artifact;
use kanna_mux::json::{json, Map, Value};
use kanna_mux::{CoreHandle, KmuxError};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const KEPT_FOR: Duration = Duration::from_secs(7 * 24 * 3600);

fn checked(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if !path.is_absolute() || path.extension().is_none_or(|e| e != "app") {
        return Err(format!("{} isn't a .app's full path", path.display()));
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(format!("{} isn't a folder", path.display())),
        Err(_) => Err(format!("no app at {}", path.display())),
    }
}

pub fn serve(path: &str, mut stream: UnixStream) {
    if let Err(message) = checked(path) {
        let _ = frame::send(&mut stream, &json!({ "op": "error", "error": message }), b"");
        return;
    }
    let _ = send_artifact(&mut stream, 0, &json!({ "path": path }));
}

fn folder(state_dir: &Path, peer: &str, path: &str) -> PathBuf {
    let digest = manifest::sha256(format!("{peer}\0{path}").as_bytes());
    state_dir.join("ios-apps").join(&digest[..16])
}

fn sweep(root: &Path, keep: &Path) {
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|at| SystemTime::now().duration_since(at).ok()).is_some_and(|age| age > KEPT_FOR);
        if old && entry.path() != keep {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

pub fn receive(mut conn: impl Pipe, base: &Path, name: &str) -> Result<PathBuf, String> {
    let first = frame::read(&mut conn, 1 << 20).map_err(|e| format!("lost the app on the way: {e}"))?.0;
    if first.op() != "artifact" {
        return Err(first.str("error").unwrap_or("the machine didn't send the app").to_string());
    }
    if first.header["dir"] != true {
        return Err(format!("{name} isn't a folder"));
    }
    std::fs::create_dir_all(base).map_err(|e| format!("can't keep {name}: {e}"))?;
    let mut intake = Intake::new(base, name, false).map_err(|e| format!("can't take {name}: {e}"))?;
    let conn: &mut dyn Pipe = &mut conn;
    match client::receive_entries(conn, Some(&mut intake)) {
        Ok(None) => intake.finish(true).map_err(|e| format!("can't keep {name}: {e}")),
        Ok(Some(why)) => {
            intake.abandon();
            Err(format!("refused {name}: {why}"))
        }
        Err(e) => {
            intake.abandon();
            Err(format!("lost {name} on the way: {e}"))
        }
    }
}

fn fetch(state_dir: &Path, callers: &Callers, peer: &str, path: &str) -> Result<PathBuf, KmuxError> {
    let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| n.ends_with(".app")).ok_or_else(|| KmuxError::new("bad_request", format!("{path} isn't a .app")))?;
    let caller = callers.get().ok_or_else(|| KmuxError::new("unavailable", "this daemon has no endpoint"))?;
    let conn = caller.bundle(peer, path).map_err(|e| KmuxError::new(e.code, e.message))?;
    let base = folder(state_dir, peer, path);
    let kept = receive(conn, &base, &name).map_err(|why| KmuxError::new("failed", why))?;
    if let Some(root) = base.parent() {
        sweep(root, &base);
    }
    Ok(kept)
}

pub fn install(state_dir: &Path, core: &CoreHandle, callers: Callers) {
    let dir = state_dir.to_path_buf();
    core.add_command("ios.fetch", std::rc::Rc::new(move |args: Value, responder| {
        let (dir, callers) = (dir.clone(), callers.clone());
        std::thread::spawn(move || {
            let (peer, path) = (args["peer"].as_str().unwrap_or_default(), args["path"].as_str().unwrap_or_default());
            let fetched = fetch(&dir, &callers, peer, path);
            responder.reply(fetched.map(|kept| Map::from_iter([("path".to_string(), json!(kept))])));
        });
    }));
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-ios-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app(dir: &Path) -> PathBuf {
        let app = dir.join("Recipes.app");
        std::fs::create_dir_all(app.join("Frameworks/Kit.framework")).unwrap();
        std::fs::write(app.join("Recipes"), "binary").unwrap();
        std::fs::set_permissions(app.join("Recipes"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(app.join("Info.plist"), "plist").unwrap();
        std::fs::write(app.join("Frameworks/Kit.framework/Kit"), "kit").unwrap();
        std::os::unix::fs::symlink("Kit", app.join("Frameworks/Kit.framework/Current")).unwrap();
        app
    }

    fn sent(path: &str) -> UnixStream {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let path = path.to_string();
        std::thread::spawn(move || serve(&path, theirs));
        ours
    }

    #[test]
    fn a_bundle_arrives_whole_and_a_second_fetch_replaces_the_first() {
        let dir = temp("whole");
        let source = app(&dir.join("from"));
        let base = dir.join("to");
        let kept = receive(sent(source.to_str().unwrap()), &base, "Recipes.app").unwrap();
        assert_eq!(std::fs::read_to_string(kept.join("Frameworks/Kit.framework/Kit")).unwrap(), "kit");
        assert_eq!(std::fs::read_link(kept.join("Frameworks/Kit.framework/Current")).unwrap(), Path::new("Kit"));
        assert!(std::fs::metadata(kept.join("Recipes")).unwrap().permissions().mode() & 0o100 != 0);
        std::fs::write(source.join("Recipes"), "rebuilt").unwrap();
        let again = receive(sent(source.to_str().unwrap()), &base, "Recipes.app").unwrap();
        assert_eq!(std::fs::read_to_string(again.join("Recipes")).unwrap(), "rebuilt");
    }

    #[test]
    fn only_a_real_app_folder_is_served() {
        let dir = temp("served");
        std::fs::write(dir.join("Plain.app"), "file").unwrap();
        for (path, expected) in [("relative/App.app", "full path"), ("/etc", "full path"), (dir.join("Missing.app").to_str().unwrap(), "no app"), (dir.join("Plain.app").to_str().unwrap(), "isn't a folder")] {
            let why = receive(sent(path), &dir.join("to"), "App.app").unwrap_err();
            assert!(why.contains(expected), "{path}: {why}");
        }
        let real = app(&dir.join("from"));
        std::os::unix::fs::symlink(&real, dir.join("Linked.app")).unwrap();
        assert!(receive(sent(dir.join("Linked.app").to_str().unwrap()), &dir.join("to"), "App.app").is_err());
    }

    #[test]
    fn a_hostile_name_in_a_bundle_is_refused_and_nothing_stays() {
        let dir = temp("hostile");
        let (mut ours, theirs) = UnixStream::pair().unwrap();
        frame::send(&mut ours, &json!({ "op": "artifact", "dir": true }), b"").unwrap();
        frame::send(&mut ours, &json!({ "op": "file", "path": "../../evil", "sha": "00", "exec": true }), b"x").unwrap();
        frame::send(&mut ours, &json!({ "op": "end" }), b"").unwrap();
        let why = receive(theirs, &dir, "App.app").unwrap_err();
        assert!(why.starts_with("refused App.app"), "{why}");
        assert!(!dir.join("App.app").exists());
        assert!(!dir.parent().unwrap().parent().unwrap().join("evil").exists());
    }
}
