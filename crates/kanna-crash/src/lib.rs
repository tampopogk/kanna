use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const FOLDER: &str = "crashes";

pub const VERSION: u64 = 1;

pub const TOP: usize = 5;

const MACHINERY: &[&str] = &[
    "__exceptionPreprocess",
    "objc_exception_throw",
    "objc_exception_rethrow",
    "-[NSException raise]",
    "+[NSException raise:format:",
    "_sigtramp",
    "__pthread_kill",
    "pthread_kill",
    "abort",
    "__abort",
    "__assert_rtn",
    "Swift._assertionFailure",
    "Swift.fatalError",
    "std::panicking",
    "std::panic",
    "core::panicking",
    "core::panic",
    "rust_begin_unwind",
    "__rustc::rust_begin_unwind",
    "std::backtrace",
    "std::sys::backtrace",
    "kanna_crash::install",
    "kanna_ffi::crash",
    "Kanna.Crashes.",
    "Kanna.CrashHandlers.",
];

pub struct Crash {
    pub process: String,
    pub executable: String,
    pub pid: u32,
    pub kind: String,
    pub reason: String,
    pub stack: Vec<Value>,
    pub build: Value,
    pub panes: Vec<Value>,
}

static PANES: Mutex<Vec<Value>> = Mutex::new(Vec::new());

static WRITTEN: AtomicBool = AtomicBool::new(false);

pub fn written() -> bool {
    WRITTEN.load(Ordering::SeqCst)
}

pub fn note_panes(panes: Vec<Value>) {
    if let Ok(mut noted) = PANES.lock() {
        *noted = panes;
    }
}

fn noted_panes() -> Vec<Value> {
    PANES.try_lock().map(|panes| panes.clone()).unwrap_or_default()
}

pub fn folder(state_dir: &Path) -> PathBuf {
    state_dir.join(FOLDER)
}

pub fn build() -> Value {
    json!({
        "id": kanna_build::sibling_daemon_build_id(),
        "channel": kanna_build::Build::current().channel.id(),
        "commit": kanna_build::COMMIT,
        "version": kanna_build::VERSION,
    })
}

fn civil(seconds: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (seconds / 86400) as i64;
    let rest = seconds % 86400;
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, (rest / 3600) as u32, (rest % 3600 / 60) as u32, (rest % 60) as u32)
}

pub fn stamps(at: SystemTime) -> (String, String) {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let (y, mo, d, h, mi, s) = civil(since.as_secs());
    let ms = since.subsec_millis();
    (format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}.{ms:03}Z"), format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z"))
}

fn symbol(frame: &Value) -> &str {
    frame.get("symbol").and_then(Value::as_str).unwrap_or("")
}

fn machinery(frame: &Value) -> bool {
    let symbol = symbol(frame).trim_start_matches('<');
    MACHINERY.iter().any(|m| symbol.starts_with(m)) || symbol.contains("as core::ops::function::Fn") && symbol.contains("Box<")
}

pub fn site(stack: &[Value]) -> &[Value] {
    let start = stack.iter().position(|f| !machinery(f)).unwrap_or(stack.len());
    if start >= stack.len() {
        return stack;
    }
    &stack[start..]
}

pub fn signature(process: &str, stack: &[Value]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(process.as_bytes());
    for frame in site(stack).iter().filter(|f| !machinery(f)).take(TOP) {
        hasher.update(b"\n");
        hasher.update(symbol(frame).as_bytes());
    }
    hasher.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

pub fn record(crash: &Crash, at: SystemTime) -> Value {
    json!({
        "version": VERSION,
        "process": crash.process,
        "executable": crash.executable,
        "pid": crash.pid,
        "time": stamps(at).1,
        "kind": crash.kind,
        "reason": crash.reason,
        "stack": crash.stack,
        "signature": signature(&crash.process, &crash.stack),
        "build": crash.build,
        "panes": crash.panes,
    })
}

pub fn write(state_dir: &Path, crash: &Crash) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let folder = folder(state_dir);
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&folder)?;
    WRITTEN.store(true, Ordering::SeqCst);
    let at = SystemTime::now();
    let record = record(crash, at);
    let stamp = stamps(at).0;
    let mut path = folder.join(format!("{stamp}-{}.json", crash.process));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = folder.join(format!("{stamp}-{}-{n}.json", crash.process));
    }
    let temporary = folder.join(format!(".{}.{}.tmp", crash.process, std::process::id()));
    std::fs::write(&temporary, format!("{record:#}\n"))?;
    std::fs::rename(&temporary, &path)?;
    Ok(path)
}

pub fn records(state_dir: &Path) -> Vec<(PathBuf, Value)> {
    let Ok(entries) = std::fs::read_dir(folder(state_dir)) else { return vec![] };
    let mut found: Vec<(PathBuf, Value)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json") && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .filter_map(|p| Some((p.clone(), serde_json::from_str::<Value>(&std::fs::read_to_string(&p).ok()?).ok()?)))
        .filter(|(_, v)| v.get("version").and_then(Value::as_u64) == Some(VERSION))
        .collect();
    found.sort_by(|a, b| a.0.file_name().cmp(&b.0.file_name()));
    found
}

pub fn apple_frame(line: &str) -> Value {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let address = fields.iter().position(|f| f.starts_with("0x"));
    let Some(address) = address.filter(|a| *a >= 1) else { return json!({ "symbol": line.trim() }) };
    let image = fields[1..address].join(" ");
    let rest = &fields[address + 1..];
    let (name, offset) = match rest.iter().rposition(|f| *f == "+") {
        Some(plus) => (rest[..plus].join(" "), rest.get(plus + 1).and_then(|o| o.parse::<u64>().ok())),
        None => (rest.join(" "), None),
    };
    let mut frame = json!({ "image": image, "symbol": demangle(&name) });
    if let Some(offset) = offset {
        frame["offset"] = json!(offset);
    }
    frame
}

#[cfg(target_os = "macos")]
fn demangle(name: &str) -> String {
    type SwiftDemangle = unsafe extern "C" fn(*const libc::c_char, libc::size_t, *mut libc::c_char, *mut libc::size_t, u32) -> *mut libc::c_char;
    if !(name.starts_with("$s") || name.starts_with("_$s") || name.starts_with("$S") || name.starts_with("_T")) {
        return name.to_string();
    }
    let found = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"swift_demangle".as_ptr()) };
    if found.is_null() {
        return name.to_string();
    }
    let demangle: SwiftDemangle = unsafe { std::mem::transmute(found) };
    let Ok(mangled) = std::ffi::CString::new(name) else { return name.to_string() };
    let out = unsafe { demangle(mangled.as_ptr(), name.len(), std::ptr::null_mut(), std::ptr::null_mut(), 0) };
    if out.is_null() {
        return name.to_string();
    }
    let text = unsafe { std::ffi::CStr::from_ptr(out) }.to_string_lossy().into_owned();
    unsafe { libc::free(out.cast()) };
    text
}

#[cfg(not(target_os = "macos"))]
fn demangle(name: &str) -> String {
    name.to_string()
}

pub fn rust_frames(backtrace: &str, image: &str) -> Vec<Value> {
    let mut frames: Vec<Value> = vec![];
    for line in backtrace.lines().map(str::trim) {
        if let Some(at) = line.strip_prefix("at ") {
            if let Some(last) = frames.last_mut() {
                last["at"] = json!(short_path(at));
            }
            continue;
        }
        let Some((number, name)) = line.split_once(": ") else { continue };
        if number.chars().all(|c| c.is_ascii_digit()) && !number.is_empty() {
            frames.push(json!({ "image": image, "symbol": name.trim() }));
        }
    }
    frames
}

fn short_path(at: &str) -> String {
    for marker in ["/crates/", "/src/", "/library/"] {
        if let Some(index) = at.find(marker) {
            return at[index + 1..].to_string();
        }
    }
    at.to_string()
}

fn panic_reason(info: &std::panic::PanicHookInfo) -> String {
    let payload = info.payload();
    let message = payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_else(|| "a panic".into());
    match info.location() {
        Some(at) => format!("{message} at {}:{}", at.file(), at.line()),
        None => message,
    }
}

pub fn executable() -> String {
    std::env::current_exe().ok().and_then(|e| e.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default()
}

pub fn install(process: &'static str, state_dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let image = executable();
        let crash = Crash {
            process: process.into(),
            executable: image.clone(),
            pid: std::process::id(),
            kind: "panic".into(),
            reason: panic_reason(info),
            stack: rust_frames(&std::backtrace::Backtrace::force_capture().to_string(), &image),
            build: build(),
            panes: noted_panes(),
        };
        match write(&state_dir, &crash) {
            Ok(path) => eprintln!("{image}: wrote the crash record {}", path.display()),
            Err(error) => eprintln!("{image}: can't write a crash record in {}: {error}", folder(&state_dir).display()),
        }
        previous(info)
    }));
}

pub fn reports_folder() -> PathBuf {
    if let Some(folder) = std::env::var_os("KANNA_DIAGNOSTIC_REPORTS").filter(|f| !f.is_empty()) {
        return PathBuf::from(folder);
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Logs/DiagnosticReports")
}

pub fn ips_body(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let (_, body) = text.split_once('\n')?;
    serde_json::from_str(body).ok()
}

pub fn find_ips(executable: &str, pid: u64, around: SystemTime) -> Option<PathBuf> {
    let entries = std::fs::read_dir(reports_folder()).ok()?;
    let early = around.checked_sub(Duration::from_secs(60)).unwrap_or(UNIX_EPOCH);
    let late = around + Duration::from_secs(3600);
    let prefix = format!("{executable}-");
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ips") && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&prefix)))
        .filter(|p| std::fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|m| m >= early && m <= late))
        .find(|p| ips_body(p).is_some_and(|body| body.get("pid").and_then(Value::as_u64) == Some(pid)))
}

pub fn ips_for(record: &Value, written: SystemTime) -> Option<PathBuf> {
    let executable = record.get("executable").and_then(Value::as_str).filter(|e| !e.is_empty())?;
    find_ips(executable, record.get("pid").and_then(Value::as_u64)?, written)
}

pub fn ips_reason(body: &Value) -> String {
    let exception = &body["exception"];
    let text = |v: &Value| v.as_str().map(String::from);
    let mut parts: Vec<String> = [text(&exception["type"]), text(&exception["signal"])].into_iter().flatten().collect();
    if let Some(message) = text(&exception["message"]).or_else(|| text(&body["termination"]["indicator"])) {
        parts.push(message);
    }
    if parts.is_empty() {
        "a crash macOS reported".into()
    } else {
        parts.join(" · ")
    }
}

pub fn ips_stack(body: &Value) -> Vec<Value> {
    let images = body["usedImages"].as_array().cloned().unwrap_or_default();
    let thread = body["faultingThread"].as_u64().and_then(|t| body["threads"].get(t as usize)).cloned().unwrap_or(Value::Null);
    let frames = body["lastExceptionBacktrace"].as_array().filter(|f| !f.is_empty()).or_else(|| thread["frames"].as_array()).cloned().unwrap_or_default();
    frames
        .iter()
        .map(|frame| {
            let image = frame["imageIndex"].as_u64().and_then(|i| images.get(i as usize)).and_then(|i| i["name"].as_str()).unwrap_or("?");
            let mut out = json!({ "image": image, "symbol": frame["symbol"].as_str().map(demangle).unwrap_or_else(|| "?".into()) });
            if let Some(offset) = frame["symbolLocation"].as_u64() {
                out["offset"] = json!(offset);
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crash(stack: Vec<Value>) -> Crash {
        Crash { process: "app".into(), executable: "Kanna".into(), pid: 7, kind: "exception".into(), reason: "boom".into(), stack, build: json!({}), panes: vec![] }
    }

    #[test]
    fn stamps_are_utc_and_sort_by_time() {
        let at = UNIX_EPOCH + Duration::from_millis(1_793_435_479_412);
        assert_eq!(stamps(at), ("20261031T083119.412Z".into(), "2026-10-31T08:31:19.412Z".into()));
        assert_eq!(stamps(UNIX_EPOCH).1, "1970-01-01T00:00:00.000Z");
        assert_eq!(stamps(UNIX_EPOCH + Duration::from_secs(951_782_400)).1, "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn apple_frames_split_into_image_symbol_and_offset() {
        let frame = apple_frame("3   AppKit                              0x000000018a2c4d8c -[NSView _drawRect:clip:] + 3012");
        assert_eq!(frame, json!({ "image": "AppKit", "symbol": "-[NSView _drawRect:clip:]", "offset": 3012 }));
        let frame = apple_frame("12  Kanna Dev                           0x0000000102a3c4d8 main + 40");
        assert_eq!(frame["image"], "Kanna Dev");
        assert_eq!(apple_frame("garbage")["symbol"], "garbage");
    }

    #[test]
    fn rust_frames_keep_their_file_and_line() {
        let text = "   0: std::backtrace::Backtrace::force_capture\n             at /rustc/abc/library/std/src/backtrace.rs:312:13\n   1: kanna_daemon::run\n             at /r/kanna/crates/kanna-daemon/src/main.rs:12:5\n   2: main\n";
        let frames = rust_frames(text, "kanna-daemon");
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[1], json!({ "image": "kanna-daemon", "symbol": "kanna_daemon::run", "at": "crates/kanna-daemon/src/main.rs:12:5" }));
        assert_eq!(frames[2]["at"], Value::Null);
    }

    #[test]
    fn a_signature_is_the_top_frames_after_the_crash_machinery() {
        let frames = |names: &[&str]| names.iter().map(|n| json!({ "image": "x", "symbol": n, "offset": names.len() })).collect::<Vec<_>>();
        let raised = frames(&["__exceptionPreprocess", "objc_exception_throw", "-[NSException raise]", "Kanna.CrashView.draw(__C.CGRect) -> ()", "-[NSView _drawRect:clip:]"]);
        let again = frames(&["__exceptionPreprocess", "objc_exception_throw", "Kanna.CrashView.draw(__C.CGRect) -> ()", "-[NSView _drawRect:clip:]"]);
        assert_eq!(site(&raised)[0]["symbol"], "Kanna.CrashView.draw(__C.CGRect) -> ()");
        assert_eq!(signature("app", &raised), signature("app", &again));
        assert_ne!(signature("app", &raised), signature("daemon", &raised));
        assert_ne!(signature("app", &raised), signature("app", &frames(&["Kanna.Other.draw()"])));
        assert_eq!(signature("app", &raised).len(), 16);
        let all_machinery = frames(&["abort", "__pthread_kill"]);
        assert_eq!(site(&all_machinery).len(), 2);
    }

    #[test]
    fn a_record_is_written_once_and_read_back() {
        let dir = std::env::temp_dir().join(format!("kanna-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let one = write(&dir, &crash(vec![json!({ "symbol": "f" })])).unwrap();
        let two = write(&dir, &crash(vec![])).unwrap();
        assert_ne!(one, two);
        assert!(one.file_name().unwrap().to_string_lossy().ends_with("-app.json"));
        let read = records(&dir);
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].1["reason"], "boom");
        assert_eq!(read[0].1["executable"], "Kanna");
        assert_eq!(read[0].1["version"], VERSION);
        std::fs::write(folder(&dir).join("junk.json"), "{").unwrap();
        assert_eq!(records(&dir).len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_ips_names_its_reason_and_faulting_frames() {
        let body = json!({
            "pid": 42,
            "exception": { "type": "EXC_CRASH", "signal": "SIGABRT" },
            "termination": { "indicator": "Abort trap: 6" },
            "usedImages": [{ "name": "Kanna" }],
            "faultingThread": 0,
            "threads": [{ "frames": [{ "imageIndex": 0, "symbol": "main", "symbolLocation": 4 }] }],
        });
        assert_eq!(ips_reason(&body), "EXC_CRASH · SIGABRT · Abort trap: 6");
        assert_eq!(ips_stack(&body), vec![json!({ "image": "Kanna", "symbol": "main", "offset": 4 })]);
    }

    #[test]
    fn the_panic_hook_writes_a_record_before_the_process_dies() {
        if let Some(dir) = std::env::var_os("KANNA_CRASH_TEST_DIR") {
            install("daemon", PathBuf::from(dir));
            panic!("on purpose");
        }
        let dir = std::env::temp_dir().join(format!("kanna-crash-hook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::the_panic_hook_writes_a_record_before_the_process_dies", "--nocapture"])
            .env("KANNA_CRASH_TEST_DIR", &dir)
            .stderr(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success());
        let read = records(&dir);
        assert_eq!(read.len(), 1, "one record");
        let record = &read[0].1;
        assert_eq!(record["kind"], "panic");
        let reason = record["reason"].as_str().unwrap();
        assert!(reason.starts_with("on purpose at ") && reason.contains("lib.rs:"), "{reason}");
        let top = site(record["stack"].as_array().unwrap());
        assert!(top[0]["symbol"].as_str().unwrap().contains("the_panic_hook_writes_a_record"), "{:#}", record["stack"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
