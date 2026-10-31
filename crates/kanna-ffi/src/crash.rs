use kanna_crash::Crash;
use serde_json::{json, Value};
use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;

fn give(value: Value) -> *mut c_char {
    CString::new(value.to_string()).map_or(std::ptr::null_mut(), CString::into_raw)
}

fn text(pointer: *const c_char) -> Option<String> {
    (!pointer.is_null()).then(|| unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into_owned())
}

fn written(request: &Value) -> Result<PathBuf, String> {
    let field = |key: &str| request.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let state_dir = PathBuf::from(field("stateDir"));
    if state_dir.as_os_str().is_empty() {
        return Err("no state folder".into());
    }
    let stack = request.get("stack").and_then(Value::as_array).cloned().unwrap_or_default();
    let crash = Crash {
        process: field("process"),
        executable: kanna_crash::executable(),
        pid: std::process::id(),
        kind: field("kind"),
        reason: field("reason"),
        stack: stack.iter().map(|line| line.as_str().map_or_else(|| line.clone(), kanna_crash::apple_frame)).collect(),
        build: request.get("build").cloned().unwrap_or(Value::Null),
        panes: request.get("panes").and_then(Value::as_array).cloned().unwrap_or_default(),
    };
    kanna_crash::write(&state_dir, &crash).map_err(|error| format!("can't write in {}: {error}", kanna_crash::folder(&state_dir).display()))
}

#[no_mangle]
pub extern "C" fn kanna_crash_write(request: *const c_char) -> *mut c_char {
    let request: Value = match text(request).map(|t| serde_json::from_str(&t)) {
        Some(Ok(request)) => request,
        _ => return give(json!({ "error": "the crash request isn't JSON" })),
    };
    give(match written(&request) {
        Ok(path) => json!({ "path": path }),
        Err(error) => json!({ "error": error }),
    })
}

#[no_mangle]
pub extern "C" fn kanna_crash_written() -> bool {
    kanna_crash::written()
}

#[no_mangle]
pub extern "C" fn kanna_crash_install(state_dir: *const c_char) {
    if let Some(dir) = text(state_dir).filter(|d| !d.is_empty()) {
        kanna_crash::install("app", PathBuf::from(dir));
    }
}

#[no_mangle]
pub extern "C" fn kanna_crash_free(string: *mut c_char) {
    if !string.is_null() {
        drop(unsafe { CString::from_raw(string) });
    }
}
