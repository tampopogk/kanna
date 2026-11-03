use crate::machines::Caller;
use kanna_guest::frame;
use kanna_mux::json::{json, Value};
use kanna_mux::Takeover;
use std::io::{BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub const OPEN: &str = "build.open";

const MAX_FIRST: u64 = 256 << 20;

const OPS: &[&str] = &["hello", "run", "attach", "cancel", "status"];

pub type Callers = Arc<OnceLock<Caller>>;

pub fn takeover(callers: Callers) -> Takeover {
    Arc::new(move |request: Value, reader: BufReader<UnixStream>, writer: UnixStream| open(&callers, &request, reader, writer))
}

fn reply(writer: &mut UnixStream, id: &Value, result: Result<Value, (String, String)>) {
    let line = match result {
        Ok(mut value) => {
            value["id"] = id.clone();
            value["ok"] = json!(true);
            value
        }
        Err((code, message)) => json!({ "id": id, "ok": false, "error": { "code": code, "message": message } }),
    };
    let _ = writer.write_all(format!("{line}\n").as_bytes());
}

fn chosen_mac(args: &Value, caller: &Caller) -> Result<String, (String, String)> {
    if let Some(mac) = args["mac"].as_str().filter(|m| !m.is_empty()) {
        return Ok(mac.to_string());
    }
    if let Some(mac) = crate::tasks::build_mac(&crate::agents::home().join(".kanna-v4/config.json")) {
        return Ok(mac);
    }
    let status = caller.status();
    let macs: Vec<String> = status["peers"].as_array().into_iter().flatten().filter(|m| m["release"]["os"] == "macos" && m["me"] != true && m["revoked"] != true).filter_map(|m| m["name"].as_str().map(String::from)).collect();
    match macs.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(("no_build_mac".into(), "none of your machines is a Mac; pair a build Mac (kanna machine pair) or set build.mac".into())),
        _ => Err(("no_build_mac".into(), format!("several of your machines are Macs ({}); set build.mac in ~/.kanna-v4/config.json or pass --mac", macs.join(", ")))),
    }
}

fn open(callers: &Callers, request: &Value, reader: BufReader<UnixStream>, mut writer: UnixStream) {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let args = request.get("args").cloned().unwrap_or(Value::Null);
    let Some(caller) = callers.get() else {
        return reply(&mut writer, &id, Err(("unavailable".into(), "this daemon has no endpoint, so it can't reach a build Mac".into())));
    };
    let mac = match chosen_mac(&args, caller) {
        Ok(mac) => mac,
        Err(failure) => return reply(&mut writer, &id, Err(failure)),
    };
    let (remote, name) = match caller.job(&mac) {
        Ok(opened) => opened,
        Err(e) => return reply(&mut writer, &id, Err((e.code, e.message))),
    };
    reply(&mut writer, &id, Ok(json!({ "mac": name })));
    let buffered = reader.buffer().to_vec();
    let client = reader.into_inner();
    splice(client, buffered, remote);
}

fn splice(a: UnixStream, a_buffered: Vec<u8>, b: UnixStream) {
    let (Ok(a_read), Ok(mut b_write)) = (a.try_clone(), b.try_clone()) else { return };
    let forward = std::thread::spawn(move || {
        let _ = b_write.write_all(&a_buffered);
        let _ = std::io::copy(&mut &a_read, &mut b_write);
        let _ = b_write.shutdown(std::net::Shutdown::Write);
    });
    let _ = std::io::copy(&mut &b, &mut &a);
    let _ = a.shutdown(std::net::Shutdown::Write);
    let _ = forward.join();
}

pub const SHIMS: &[&str] = &["xcodebuild", "xcrun", "codesign", "swift"];

pub fn install_shim(state_dir: &Path, kanna: &Path, wanted: bool) -> Option<PathBuf> {
    if !wanted || !kanna.is_file() {
        return None;
    }
    let dir = state_dir.join("shim");
    std::fs::create_dir_all(&dir).ok()?;
    for tool in SHIMS {
        let link = dir.join(tool);
        if std::fs::read_link(&link).ok().as_deref() != Some(kanna) {
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(kanna, &link).ok()?;
        }
    }
    Some(dir)
}

pub fn shim_wanted() -> bool {
    match std::env::var("KANNA_BUILD_SHIM").ok().as_deref() {
        Some("1") => true,
        Some("0") => false,
        _ => cfg!(target_os = "linux"),
    }
}

pub fn guests_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("guests")
}

fn guests(state_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(guests_dir(state_dir)).into_iter().flatten().flatten().filter_map(|e| {
        let socket = e.path().join("agent.sock");
        socket.exists().then(|| (e.file_name().to_string_lossy().into_owned(), socket))
    }).collect();
    found.sort();
    found
}

static ROUTES: Mutex<()> = Mutex::new(());

fn routes_file(state_dir: &Path) -> PathBuf {
    guests_dir(state_dir).join("routes.json")
}

fn load_routes(state_dir: &Path) -> Value {
    std::fs::read(routes_file(state_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| json!({ "jobs": {}, "mirrors": {} }))
}

fn route(state_dir: &Path, first: &Value) -> Result<(String, PathBuf), String> {
    let _held = ROUTES.lock().unwrap_or_else(|p| p.into_inner());
    let available = guests(state_dir);
    if available.is_empty() {
        return Err("this build Mac has no guest running; builds run only inside a guest".into());
    }
    let mut routes = load_routes(state_dir);
    let op = first["op"].as_str().unwrap_or_default();
    let job = first["job"].as_str().unwrap_or_default().to_string();
    let find = |name: &str| available.iter().find(|(n, _)| n == name).cloned();
    if op == "hello" {
        return Ok(available[0].clone());
    }
    if op != "run" {
        let name = routes["jobs"][&job].as_str().unwrap_or_default().to_string();
        return find(&name).ok_or_else(|| format!("no such job {job}"));
    }
    let mirror = &first["mirror"];
    let key = format!("{}/{}/{}", mirror["machine"].as_str().unwrap_or_default(), mirror["repo"].as_str().unwrap_or_default(), mirror["task"].as_str().unwrap_or_default());
    let known = routes["jobs"][&job].as_str().or(routes["mirrors"][&key].as_str()).and_then(find);
    let chosen = known.unwrap_or_else(|| {
        let busy = |name: &str| routes["mirrors"].as_object().map_or(0, |m| m.values().filter(|v| v.as_str() == Some(name)).count());
        available.iter().min_by_key(|(name, _)| busy(name)).cloned().unwrap_or_else(|| available[0].clone())
    });
    routes["jobs"][&job] = json!(chosen.0);
    routes["mirrors"][&key] = json!(chosen.0);
    if let Some(jobs) = routes["jobs"].as_object_mut() {
        while jobs.len() > 4096 {
            let Some(oldest) = jobs.keys().next().cloned() else { break };
            jobs.remove(&oldest);
        }
    }
    let file = routes_file(state_dir);
    let _ = std::fs::write(file.with_extension("tmp"), serde_json::to_vec(&routes).unwrap_or_default()).and_then(|()| std::fs::rename(file.with_extension("tmp"), &file));
    Ok(chosen)
}

pub fn relay(state_dir: &Path, from: &str, mut stream: UnixStream) {
    let first = match frame::read_header(&mut stream, MAX_FIRST) {
        Ok(first) => first,
        Err(_) => return,
    };
    let refuse = |stream: &mut UnixStream, message: String| {
        let _ = frame::send(stream, &json!({ "op": "error", "error": message }), b"");
    };
    if !OPS.contains(&first.op()) {
        return refuse(&mut stream, format!("no op {:?}", first.op()));
    }
    let mut header = first.header.clone();
    if header["op"] == "run" {
        header["mirror"]["machine"] = json!(from);
    }
    let (guest, socket) = match route(state_dir, &header) {
        Ok(found) => found,
        Err(message) => return refuse(&mut stream, message),
    };
    let mut to_guest = match UnixStream::connect(&socket) {
        Ok(conn) => conn,
        Err(e) => return refuse(&mut stream, format!("guest {guest} isn't answering: {e}")),
    };
    if frame::write_header(&mut to_guest, &header, first.size).is_err() {
        return refuse(&mut stream, format!("lost guest {guest}"));
    }
    if std::io::copy(&mut Read::by_ref(&mut stream).take(first.size), &mut to_guest).ok() != Some(first.size) {
        return;
    }
    splice(stream, vec![], to_guest);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shim_links_each_tool_to_kanna_once() {
        let dir = std::env::temp_dir().join(format!("kanna-shim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let kanna = dir.join("kanna");
        std::fs::write(&kanna, "").unwrap();
        assert_eq!(install_shim(&dir, &kanna, false), None);
        let shim = install_shim(&dir, &kanna, true).unwrap();
        assert_eq!(std::fs::read_link(shim.join("xcodebuild")).unwrap(), kanna);
        assert_eq!(install_shim(&dir, &kanna, true), Some(shim.clone()));
        assert_eq!(std::fs::read_dir(&shim).unwrap().count(), SHIMS.len());
    }
}
