use kanna_net::Client;
use serde_json::{json, Value};
use std::ffi::{c_char, CStr, CString};
use std::os::fd::IntoRawFd;
use std::sync::OnceLock;

struct Machines {
    runtime: tokio::runtime::Runtime,
    client: Client,
}

static MACHINES: OnceLock<Result<Machines, String>> = OnceLock::new();

fn give(value: Value) -> *mut c_char {
    CString::new(value.to_string()).map_or(std::ptr::null_mut(), CString::into_raw)
}

fn text(pointer: *const c_char) -> Option<String> {
    (!pointer.is_null()).then(|| unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into_owned())
}

fn failure(code: &str, message: impl Into<String>) -> Value {
    json!({ "error": { "code": code, "message": message.into() } })
}

fn start(key_path: &str) -> Result<Machines, String> {
    let secret = kanna_net::read_key(std::path::Path::new(key_path))?;
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("kanna.machines").enable_all().build().map_err(|e| format!("can't start the machines' runtime: {e}"))?;
    let client = runtime.block_on(Client::start(secret))?;
    Ok(Machines { runtime, client })
}

fn machines() -> Result<&'static Machines, Value> {
    match MACHINES.get() {
        Some(Ok(machines)) => Ok(machines),
        Some(Err(reason)) => Err(failure("unavailable", reason.clone())),
        None => Err(failure("unavailable", "the app's endpoint isn't started")),
    }
}

#[no_mangle]
pub extern "C" fn kanna_machines_client(key_path: *const c_char) -> *mut c_char {
    let Some(path) = text(key_path) else { return give(failure("bad_request", "no key path")) };
    give(match MACHINES.get_or_init(|| start(&path)) {
        Ok(machines) => json!({ "key": machines.client.key() }),
        Err(reason) => failure("unavailable", reason.clone()),
    })
}

#[no_mangle]
pub extern "C" fn kanna_machines_link(request: *const c_char) -> *mut c_char {
    let request: Value = match text(request).map(|t| serde_json::from_str(&t)) {
        Some(Ok(request)) => request,
        _ => return give(failure("bad_request", "the link request isn't JSON")),
    };
    let machines = match machines() {
        Ok(machines) => machines,
        Err(error) => return give(error),
    };
    let key = request["key"].as_str().unwrap_or_default().to_string();
    let addrs: Vec<String> = request["addrs"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    give(match machines.runtime.block_on(machines.client.link(&key, &addrs, &request["cert"])) {
        Ok(stream) => json!({ "fd": stream.into_raw_fd() }),
        Err(failed) => failure(&failed.code, failed.message),
    })
}

#[no_mangle]
pub extern "C" fn kanna_machines_path(key: *const c_char) -> *mut c_char {
    let (Ok(machines), Some(key)) = (machines(), text(key)) else { return give(Value::Null) };
    give(machines.client.path(&key))
}

#[no_mangle]
pub extern "C" fn kanna_machines_free(string: *mut c_char) {
    if !string.is_null() {
        drop(unsafe { CString::from_raw(string) });
    }
}
