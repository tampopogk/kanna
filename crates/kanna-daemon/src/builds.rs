use crate::build_queue::{self, Cancelled, Guest, Job, Poll, Queue};
use crate::machines::Caller;
use kanna_guest::frame;
use kanna_mux::json::{json, Value};
use kanna_mux::{CoreHandle, KmuxError, Takeover};
use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const OPEN: &str = "build.open";

const MAX_FIRST: u64 = 256 << 20;

const OPS: &[&str] = &["hello", "run", "attach", "cancel", "status"];

const POLL: Duration = Duration::from_millis(200);

const WATCH: Duration = Duration::from_secs(1);

const LOST_AFTER: Duration = Duration::from_secs(120);

const CAPS_LIFE: Duration = Duration::from_secs(300);

const POOL_WAIT: Duration = Duration::from_secs(8);

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

fn peer_macs(caller: &Caller) -> Vec<String> {
    let status = caller.status();
    status["peers"].as_array().into_iter().flatten().filter(|m| (m["release"]["os"] == "macos" || m["release"]["os"].is_null()) && m["me"] != true && m["revoked"] != true).filter_map(|m| m["name"].as_str().map(String::from)).collect()
}

pub fn pool(caller: &Caller, macs: Vec<String>, args: Value) -> Vec<(String, Result<Value, String>)> {
    let (tx, rx) = std::sync::mpsc::channel();
    for mac in &macs {
        let (tx, caller, mac, args) = (tx.clone(), caller.clone(), mac.clone(), args.clone());
        std::thread::spawn(move || {
            let reply = caller.call(&mac, "build.jobs", args).map_err(|e| e.message);
            let _ = tx.send((mac, reply));
        });
    }
    drop(tx);
    let deadline = Instant::now() + POOL_WAIT;
    let mut answers: Vec<(String, Result<Value, String>)> = Vec::new();
    while answers.len() < macs.len() {
        let Ok(answer) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) else { break };
        answers.push(answer);
    }
    for mac in macs {
        if !answers.iter().any(|(name, _)| *name == mac) {
            answers.push((mac.clone(), Err(format!("{mac} didn't answer in time"))));
        }
    }
    answers.sort_by(|a, b| a.0.cmp(&b.0));
    answers
}

pub fn pick(answers: &[(String, Result<Value, String>)], needs: &Value) -> Result<String, String> {
    let matching = |reply: &Value| reply["guests"].as_array().into_iter().flatten().any(|g| needs.is_null() || build_queue::satisfies(needs, g));
    let waiting = |reply: &Value| reply["jobs"].as_array().map_or(0, Vec::len);
    let usable = answers.iter().filter_map(|(mac, reply)| Some((mac, reply.as_ref().ok()?))).filter(|(_, reply)| matching(reply));
    if let Some((mac, _)) = usable.min_by_key(|(_, reply)| (reply["mirror"] != true, waiting(reply))) {
        return Ok(mac.clone());
    }
    let guests = answers.iter().any(|(_, reply)| reply.as_ref().is_ok_and(|r| r["guests"].as_array().is_some_and(|g| !g.is_empty())));
    Err(match (guests, needs.is_null()) {
        (false, _) => "none of your Macs has a guest running; builds run only inside a guest".to_string(),
        (true, false) => format!("no build Mac has a guest with {}", build_queue::unmet(needs)),
        (true, true) => "no build Mac answered".to_string(),
    })
}

fn chosen_mac(args: &Value, caller: &Caller) -> Result<String, (String, String)> {
    if let Some(mac) = args["mac"].as_str().filter(|m| !m.is_empty()) {
        return Ok(mac.to_string());
    }
    if let Some(mac) = crate::tasks::build_mac(&crate::agents::home().join(".kanna-v4/config.json")) {
        return Ok(mac);
    }
    let macs = peer_macs(caller);
    let needs = args.get("needs").cloned().unwrap_or(Value::Null);
    match macs.as_slice() {
        [one] if needs.is_null() => Ok(one.clone()),
        [] => Err(("no_build_mac".into(), "none of your machines is a Mac; pair a build Mac (kanna machine pair) or set build.mac".into())),
        _ => {
            let mut query = args.clone();
            query["machine"] = caller.status()["name"].clone();
            pick(&pool(caller, macs, query), &needs).map_err(|why| ("no_build_mac".into(), why))
        }
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

fn known_guest(state_dir: &Path, job: &str) -> Option<(String, PathBuf)> {
    let name = load_routes(state_dir)["jobs"][job].as_str()?.to_string();
    guests(state_dir).into_iter().find(|(n, _)| *n == name)
}

fn remember(state_dir: &Path, job: &Job, guest: &str) {
    let _held = ROUTES.lock().unwrap_or_else(|p| p.into_inner());
    let mut routes = load_routes(state_dir);
    routes["jobs"][&job.id] = json!(guest);
    routes["mirrors"][job.mirror()] = json!(guest);
    if let Some(jobs) = routes["jobs"].as_object_mut() {
        while jobs.len() > 4096 {
            let Some(oldest) = jobs.keys().next().cloned() else { break };
            jobs.remove(&oldest);
        }
    }
    let file = routes_file(state_dir);
    let _ = std::fs::write(file.with_extension("tmp"), serde_json::to_vec(&routes).unwrap_or_default()).and_then(|()| std::fs::rename(file.with_extension("tmp"), &file));
}

static QUEUE: OnceLock<Queue> = OnceLock::new();

fn queue(state_dir: &Path) -> &'static Queue {
    QUEUE.get_or_init(|| Queue::load(&guests_dir(state_dir)))
}

static CAPS: Mutex<Option<HashMap<String, (Instant, Value)>>> = Mutex::new(None);

fn first_version(text: &str) -> String {
    text.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or_default().to_string()
}

pub fn caps_of(hello: &Value) -> Value {
    let versions = &hello["versions"];
    json!({ "arch": versions["arch"], "macos": versions["macos"], "xcode": first_version(versions["xcode"].as_str().unwrap_or_default()), "runtimes": hello["runtimes"].as_array().cloned().unwrap_or_default() })
}

fn ask_guest(socket: &Path, request: &Value, within: Duration) -> std::io::Result<frame::Frame> {
    let mut conn = UnixStream::connect(socket)?;
    conn.set_read_timeout(Some(within))?;
    conn.set_write_timeout(Some(within))?;
    frame::send(&mut conn, request, b"")?;
    let (reply, _) = frame::read(&mut conn, 1 << 20)?;
    Ok(reply)
}

fn refresh_caps(state_dir: &Path) {
    let stale: Vec<(String, PathBuf)> = {
        let known = CAPS.lock().unwrap_or_else(|p| p.into_inner());
        guests(state_dir).into_iter().filter(|(name, _)| known.as_ref().and_then(|k| k.get(name)).is_none_or(|(at, _)| at.elapsed() > CAPS_LIFE)).collect()
    };
    for (name, socket) in stale {
        if let Ok(reply) = ask_guest(&socket, &json!({ "op": "hello" }), Duration::from_secs(20)) {
            let caps = caps_of(&reply.header);
            CAPS.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_default().insert(name, (Instant::now(), caps));
        }
    }
}

fn guest_list(state_dir: &Path) -> (Vec<Guest>, Value) {
    let routes = load_routes(state_dir);
    let known = CAPS.lock().unwrap_or_else(|p| p.into_inner());
    let list = guests(state_dir).into_iter().map(|(name, _)| {
        let mirrors = routes["mirrors"].as_object().map_or(0, |m| m.values().filter(|v| v.as_str() == Some(name.as_str())).count());
        let caps = known.as_ref().and_then(|k| k.get(&name)).map_or(Value::Null, |(_, caps)| caps.clone());
        Guest { name, mirrors, caps }
    }).collect();
    (list, routes)
}

fn host_facts(state_dir: &Path) -> Value {
    let cores = std::thread::available_parallelism().map_or(0, |n| n.get());
    let mut disk: libc::statvfs = unsafe { std::mem::zeroed() };
    let path = std::ffi::CString::new(state_dir.to_string_lossy().as_bytes()).unwrap_or_default();
    let free = if unsafe { libc::statvfs(path.as_ptr(), &mut disk) } == 0 { disk.f_bavail as u64 * disk.f_frsize as u64 } else { 0 };
    json!({ "cores": cores, "memory": memory(), "free_disk": free })
}

#[cfg(target_os = "macos")]
fn memory() -> u64 {
    let mut size: u64 = 0;
    let mut length = std::mem::size_of::<u64>();
    let name = c"hw.memsize";
    let found = unsafe { libc::sysctlbyname(name.as_ptr(), (&raw mut size).cast(), &mut length, std::ptr::null_mut(), 0) } == 0;
    if found { size } else { 0 }
}

#[cfg(not(target_os = "macos"))]
fn memory() -> u64 {
    let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    (pages.max(0) as u64) * (size.max(0) as u64)
}

pub fn jobs(state_dir: &Path, args: &Value) -> Value {
    refresh_caps(state_dir);
    let (list, routes) = guest_list(state_dir);
    let mut reply = queue(state_dir).list();
    reply["guests"] = list.iter().filter(|g| !g.caps.is_null()).map(|g| {
        let mut record = g.caps.clone();
        record["name"] = json!(g.name);
        record
    }).collect::<Vec<_>>().into();
    reply["host"] = host_facts(state_dir);
    if let (Some(machine), Some(repo), Some(task)) = (args["machine"].as_str(), args["repo"].as_str(), args["task"].as_str()) {
        reply["mirror"] = json!(routes["mirrors"][format!("{machine}/{repo}/{task}")].is_string());
    }
    reply
}

fn stop_in_guest(state_dir: &Path, job: &str, guest: &str) -> bool {
    let Some((_, socket)) = guests(state_dir).into_iter().find(|(name, _)| name == guest) else { return false };
    ask_guest(&socket, &json!({ "op": "cancel", "job": job }), Duration::from_secs(5)).is_ok_and(|reply| reply.op() == "ok")
}

pub fn cancel(state_dir: &Path, job: &str) -> Result<&'static str, KmuxError> {
    match queue(state_dir).cancel(job) {
        Cancelled::Dequeued => Ok("dequeued"),
        Cancelled::Asked(guest) => {
            let reached = guest.is_some_and(|guest| stop_in_guest(state_dir, job, &guest));
            Ok(if reached { "stopping" } else { "asked" })
        }
        Cancelled::Finished => Ok("finished"),
        Cancelled::Unknown => Err(KmuxError::new("not_found", format!("no build job {job}"))),
    }
}

fn watch(state_dir: PathBuf, job: String, guest: String, relay: Arc<std::sync::atomic::AtomicBool>) {
    let queue = queue(&state_dir);
    let mut asked = false;
    let mut last_heard = Instant::now();
    let lost = |why: &str| json!({ "phase": "failed", "job": job, "error": why });
    loop {
        std::thread::sleep(WATCH);
        let socket = guests(&state_dir).into_iter().find(|(name, _)| *name == guest).map(|(_, socket)| socket);
        let reply = socket.as_ref().map(|socket| ask_guest(socket, &json!({ "op": "status", "job": job }), Duration::from_secs(5)));
        let status = match reply {
            Some(Ok(reply)) if reply.op() == "status" => reply.header["status"].clone(),
            Some(Ok(_)) => {
                last_heard = Instant::now();
                if !relay.load(std::sync::atomic::Ordering::SeqCst) {
                    return queue.finish(&job, &lost("the job never reached its guest"));
                }
                continue;
            }
            _ => {
                if last_heard.elapsed() >= LOST_AFTER {
                    return queue.finish(&job, &lost("its guest stopped answering"));
                }
                continue;
            }
        };
        last_heard = Instant::now();
        if !asked && queue.wants_cancel(&job) {
            asked = stop_in_guest(&state_dir, &job, &guest);
        }
        match status["phase"].as_str() {
            Some("queued") => {}
            Some("running") => queue.running(&job, None),
            _ => return queue.finish(&job, &status),
        }
    }
}

pub fn install(state_dir: &Path, core: &CoreHandle, callers: Callers) {
    let dir = state_dir.to_path_buf();
    for (job, guest) in queue(state_dir).resumed() {
        let dir = dir.clone();
        std::thread::spawn(move || watch(dir, job, guest, Arc::default()));
    }
    let here = dir.clone();
    core.add_command("build.jobs", std::rc::Rc::new(move |args, responder| {
        let dir = here.clone();
        std::thread::spawn(move || responder.reply(Ok(jobs(&dir, &args).as_object().cloned().unwrap_or_default())));
    }));
    let here = dir.clone();
    core.add_command("build.cancel", std::rc::Rc::new(move |args, responder| {
        let dir = here.clone();
        std::thread::spawn(move || {
            let job = args["job"].as_str().unwrap_or_default().to_string();
            responder.reply(cancel(&dir, &job).map(|done| kanna_mux::json::Map::from_iter([("done".to_string(), json!(done))])));
        });
    }));
    let here = dir.clone();
    core.add_command("build.pool", std::rc::Rc::new(move |args, responder| {
        let (dir, callers) = (here.clone(), callers.clone());
        std::thread::spawn(move || {
            let mut macs: Vec<Value> = Vec::new();
            let mine = jobs(&dir, &Value::Null);
            if mine["guests"].as_array().is_some_and(|g| !g.is_empty()) || mine["jobs"].as_array().is_some_and(|j| !j.is_empty()) {
                let mut record = mine;
                record["mac"] = json!(callers.get().and_then(|c| c.status()["name"].as_str().map(String::from)).unwrap_or_else(|| "this Mac".into()));
                record["here"] = json!(true);
                macs.push(record);
            }
            let cancelling = args["job"].as_str().map(String::from);
            if let Some(caller) = callers.get() {
                for (mac, reply) in pool(caller, peer_macs(caller), json!({})) {
                    macs.push(match reply {
                        Ok(mut reply) => {
                            reply["mac"] = json!(mac);
                            reply.as_object_mut().map(|o| { o.remove("ok"); o.remove("id"); });
                            reply
                        }
                        Err(why) => json!({ "mac": mac, "unreachable": why }),
                    });
                }
            }
            let Some(job) = cancelling else {
                return responder.reply(Ok(kanna_mux::json::Map::from_iter([("macs".to_string(), json!(macs))])));
            };
            let holds = |record: &&Value| ["jobs", "recent"].iter().any(|list| record[*list].as_array().into_iter().flatten().any(|j| j["job"] == job.as_str()));
            let Some(record) = macs.iter().find(holds) else {
                return responder.reply(Err(KmuxError::new("not_found", format!("no build job {job} on your build Macs"))));
            };
            let done = if record["here"] == true {
                cancel(&dir, &job).map(|done| json!(done))
            } else {
                let mac = record["mac"].as_str().unwrap_or_default();
                callers.get().ok_or_else(|| KmuxError::new("unavailable", "this daemon has no endpoint")).and_then(|c| c.call(mac, "build.cancel", json!({ "job": job })).map(|reply| reply["done"].clone()))
            };
            responder.reply(done.map(|done| kanna_mux::json::Map::from_iter([("done".to_string(), done), ("mac".to_string(), record["mac"].clone())])));
        });
    }));
}

fn enqueue(state_dir: &Path, stream: &mut UnixStream, job: Job) -> Option<String> {
    let queue = queue(state_dir);
    let id = job.id.clone();
    let relay = queue.submit(job);
    let _ = stream.set_read_timeout(Some(POLL));
    let mut said = None;
    let granted = loop {
        let (list, routes) = guest_list(state_dir);
        match queue.poll(&id, relay, &list, &routes) {
            Poll::Granted(guest) => break Some(guest),
            Poll::Gone(ended) => {
                if let Some(ended) = ended {
                    let _ = frame::send(stream, &json!({ "op": "done", "status": ended["status"] }), b"");
                }
                break None;
            }
            Poll::Waiting(ahead) if said != Some(ahead) => {
                said = Some(ahead);
                if frame::send(stream, &json!({ "op": "queued", "ahead": ahead, "jobs": queue.waiting() }), b"").is_err() {
                    queue.detach(&id, relay);
                    break None;
                }
            }
            Poll::Waiting(_) => {}
        }
        let mut byte = [0u8; 1];
        match stream.read(&mut byte) {
            Ok(_) => {
                queue.detach(&id, relay);
                break None;
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => {
                queue.detach(&id, relay);
                break None;
            }
        }
    };
    let _ = stream.set_read_timeout(None);
    granted
}

fn run(state_dir: &Path, from: &str, mut stream: UnixStream, first: frame::Frame) {
    let refuse = |stream: &mut UnixStream, message: String| {
        let _ = frame::send(stream, &json!({ "op": "error", "error": message }), b"");
    };
    let mut payload = vec![0u8; first.size as usize];
    if stream.read_exact(&mut payload).is_err() {
        return;
    }
    let mut header = first.header.clone();
    header["mirror"]["machine"] = json!(from);
    let id = header["job"].as_str().unwrap_or_default().to_string();
    let queue = queue(state_dir);
    let resumed = queue.has_guest(&id).or_else(|| known_guest(state_dir, &id).map(|(guest, _)| guest));
    let job = Job::new(&id, &header);
    let fresh = resumed.is_none();
    let guest = match resumed {
        Some(guest) => guest,
        None => {
            refresh_caps(state_dir);
            let (list, _) = guest_list(state_dir);
            if guests(state_dir).is_empty() {
                return refuse(&mut stream, "this build Mac has no guest running; builds run only inside a guest".into());
            }
            if !job.needs.is_null() && !list.iter().any(|g| build_queue::satisfies(&job.needs, &g.caps)) {
                return refuse(&mut stream, format!("no guest on this build Mac has {}", build_queue::unmet(&job.needs)));
            }
            match enqueue(state_dir, &mut stream, job.clone()) {
                Some(guest) => guest,
                None => return,
            }
        }
    };
    let Some((_, socket)) = guests(state_dir).into_iter().find(|(name, _)| *name == guest) else {
        queue.finish(&id, &json!({ "phase": "failed", "job": id, "error": format!("guest {guest} went away") }));
        return refuse(&mut stream, format!("guest {guest} went away"));
    };
    let relaying = Arc::new(std::sync::atomic::AtomicBool::new(true));
    if fresh {
        remember(state_dir, &job, &guest);
        let (dir, watched, name, flag) = (state_dir.to_path_buf(), id.clone(), guest.clone(), relaying.clone());
        std::thread::spawn(move || watch(dir, watched, name, flag));
    }
    if let Some(guests) = crate::guests::GUESTS.get() {
        header["capability"] = json!(guests.issue(&guest, &id));
    }
    let _ = frame::send(&mut stream, &json!({ "op": "granted", "guest": guest }), b"");
    let finish = |relaying: &std::sync::atomic::AtomicBool| relaying.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut to_guest = match UnixStream::connect(&socket) {
        Ok(conn) => conn,
        Err(e) => {
            finish(&relaying);
            return refuse(&mut stream, format!("guest {guest} isn't answering: {e}"));
        }
    };
    if frame::write_header(&mut to_guest, &header, first.size).and_then(|()| to_guest.write_all(&payload)).is_err() {
        finish(&relaying);
        return refuse(&mut stream, format!("lost guest {guest}"));
    }
    drop(payload);
    splice(stream, vec![], to_guest);
    finish(&relaying);
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
    if first.op() == "run" {
        return run(state_dir, from, stream, first);
    }
    let job = first.str("job").unwrap_or_default().to_string();
    if first.op() == "cancel" && queue(state_dir).cancel(&job) == Cancelled::Dequeued {
        let _ = frame::send(&mut stream, &json!({ "op": "ok", "status": { "phase": "cancelled", "job": job } }), b"");
        return;
    }
    let found = if first.op() == "hello" {
        guests(state_dir).into_iter().next().ok_or_else(|| "this build Mac has no guest running; builds run only inside a guest".to_string())
    } else if guests(state_dir).is_empty() {
        Err("this build Mac has no guest running; builds run only inside a guest".to_string())
    } else {
        known_guest(state_dir, &job).ok_or_else(|| format!("no such job {job}"))
    };
    let (guest, socket) = match found {
        Ok(found) => found,
        Err(message) => return refuse(&mut stream, message),
    };
    let mut to_guest = match UnixStream::connect(&socket) {
        Ok(conn) => conn,
        Err(e) => return refuse(&mut stream, format!("guest {guest} isn't answering: {e}")),
    };
    if frame::write_header(&mut to_guest, &first.header, first.size).is_err() {
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
    fn the_pool_prefers_the_mac_with_the_tasks_mirror_then_the_shortest_queue() {
        let guest = |xcode: &str| json!({ "xcode": xcode, "arch": "aarch64", "macos": "26.0", "runtimes": ["26.1"] });
        let mac = |xcode: &str, jobs: usize, mirror: bool| Ok(json!({ "guests": [guest(xcode)], "jobs": vec![json!({}); jobs], "mirror": mirror }));
        let answers = vec![("busy".to_string(), mac("26.1", 3, false)), ("idle".to_string(), mac("26.1", 0, false)), ("old".to_string(), mac("15.4", 0, false)), ("gone".to_string(), Err("away".to_string()))];
        assert_eq!(pick(&answers, &Value::Null), Ok("idle".to_string()));
        assert_eq!(pick(&answers, &json!({ "xcode": "15" })), Ok("old".to_string()));
        let warm = vec![("idle".to_string(), mac("26.1", 0, false)), ("warm".to_string(), mac("26.1", 2, true))];
        assert_eq!(pick(&warm, &Value::Null), Ok("warm".to_string()));
        assert_eq!(pick(&answers, &json!({ "xcode": "99" })).unwrap_err(), "no build Mac has a guest with xcode 99");
        assert!(pick(&[("linux".to_string(), Ok(json!({ "guests": [] })))], &Value::Null).unwrap_err().contains("has a guest running"));
    }

    #[test]
    fn a_guests_hello_becomes_its_capabilities() {
        let caps = caps_of(&json!({ "versions": { "arch": "aarch64", "macos": "26.0", "xcode": "Xcode 26.1\nBuild version 17B55" }, "runtimes": ["26.1"] }));
        assert_eq!(caps, json!({ "arch": "aarch64", "macos": "26.0", "xcode": "26.1", "runtimes": ["26.1"] }));
    }

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
