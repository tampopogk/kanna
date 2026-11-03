use crate::app_link::AppLink;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use kanna_mux::json::{string, Map, Value};
use kanna_mux::{KmuxError, Responder};
use serde_json::json;
use std::fs::OpenOptions;
use std::future::Future;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

pub fn take(link: &AppLink, args: Value, responder: Responder) {
    let mut asked = args.clone();
    if let Some(asked) = asked.as_object_mut() {
        for key in ["path", "dir", "force"] {
            asked.remove(key);
        }
    }
    let (inner, answer) = kanna_mux::responder();
    link.forward("screenshot", asked, inner);
    let _ = std::thread::Builder::new().name("kanna.screenshot".into()).spawn(move || responder.reply(block_on(answer).and_then(|shot| save(&args, shot))));
}

fn save(args: &Value, mut shot: Map) -> Result<Map, KmuxError> {
    let png = shot.remove("png").and_then(|p| p.as_str().and_then(|p| BASE64.decode(p).ok())).ok_or_else(|| KmuxError::new("failed", "Kanna.app sent no picture"))?;
    let name = ["pane", "window"].iter().find_map(|key| shot.get(*key).and_then(Value::as_str)).unwrap_or("window").to_string();
    let path = target(args, &name, &stamp())?;
    write(&png, &path, args.get("force") == Some(&Value::Bool(true)))?;
    shot.insert("path".into(), json!(path.to_string_lossy()));
    Ok(shot)
}

fn target(args: &Value, name: &str, stamp: &str) -> Result<PathBuf, KmuxError> {
    if let Some(path) = string(args, "path") {
        if !path.starts_with('/') {
            return Err(KmuxError::new("bad_request", "path must be absolute"));
        }
        return Ok(PathBuf::from(path));
    }
    let dir = PathBuf::from(string(args, "dir").map(String::from).unwrap_or_else(|| std::env::temp_dir().to_string_lossy().into_owned()));
    let base = format!("kanna-{name}-{stamp}");
    let taken = |file: &str| std::fs::symlink_metadata(dir.join(file)).is_ok();
    let file = std::iter::once(format!("{base}.png")).chain((2..100).map(|n| format!("{base}-{n}.png"))).find(|f| !taken(f)).unwrap_or_else(|| format!("{base}-{}.png", std::process::id()));
    Ok(dir.join(file))
}

fn stamp() -> String {
    kanna_threads::now().replace(['-', ':'], "").replace('T', "-")
}

fn write(png: &[u8], path: &Path, replacing: bool) -> Result<(), KmuxError> {
    let mut options = OpenOptions::new();
    options.write(true).mode(0o600).custom_flags(libc::O_NOFOLLOW);
    if replacing {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let shown = path.display();
    let mut file = options.open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => KmuxError::new("exists", format!("{shown} already exists (--force replaces it)")),
        _ => KmuxError::new("bad_request", format!("can't create {shown}: {e}")),
    })?;
    file.write_all(png).map_err(|e| {
        let _ = std::fs::remove_file(path);
        KmuxError::new("bad_request", format!("can't write {shown}: {e}"))
    })
}

struct Unpark(std::thread::Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<T>(future: impl Future<Output = T>) -> T {
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shot_goes_where_the_cli_asked_and_never_over_a_file_unless_forced() {
        let dir = std::env::temp_dir().join(format!("kanna-shot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let shot = || json!({ "png": BASE64.encode(b"png"), "window": "win-1", "pane": "term-3", "width": 2, "height": 1 }).as_object().unwrap().clone();
        let saved = save(&json!({ "dir": dir }), shot()).unwrap();
        let first = saved["path"].as_str().unwrap().to_string();
        assert!(first.starts_with(&format!("{}/kanna-term-3-", dir.display())), "{first}");
        assert_eq!((saved.get("png"), &saved["width"]), (None, &json!(2)));
        assert_eq!(std::fs::read(&first).unwrap(), b"png");
        let second = save(&json!({ "dir": dir }), shot()).unwrap()["path"].as_str().unwrap().to_string();
        assert_ne!(first, second);
        let out = dir.join("out.png");
        save(&json!({ "path": out }), shot()).unwrap();
        assert_eq!(save(&json!({ "path": out }), shot()).unwrap_err().code, "exists");
        assert!(save(&json!({ "path": out, "force": true }), shot()).is_ok());
        assert_eq!(save(&json!({ "path": "rel.png" }), shot()).unwrap_err().code, "bad_request");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
