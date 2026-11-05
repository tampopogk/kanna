use kanna_net::{hex, unhex, Client, LineRecv, LineSend, Relay, SecretKey};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use tokio::runtime::Runtime;
use tokio::sync::Mutex as AsyncMutex;

struct Stream {
    send: AsyncMutex<LineSend>,
    recv: AsyncMutex<LineRecv>,
}

#[derive(Default)]
struct Phone {
    client: Option<Client>,
    secret: Option<SecretKey>,
    streams: HashMap<u64, Arc<Stream>>,
}

static NEXT: AtomicU64 = AtomicU64::new(1);

fn runtime() -> Result<&'static Runtime, Value> {
    static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();
    let built = RUNTIME.get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("kanna.phone").enable_all().build().map_err(|e| format!("can't start the phone's runtime: {e}")));
    built.as_ref().map_err(|reason| failure("unavailable", reason.clone()))
}

fn phone() -> std::sync::MutexGuard<'static, Phone> {
    static PHONE: OnceLock<Mutex<Phone>> = OnceLock::new();
    PHONE.get_or_init(Mutex::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn give(value: Value) -> *mut c_char {
    CString::new(value.to_string()).map_or(std::ptr::null_mut(), CString::into_raw)
}

fn text(pointer: *const c_char) -> Option<String> {
    (!pointer.is_null()).then(|| unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into_owned())
}

fn failure(code: &str, message: impl Into<String>) -> Value {
    json!({ "error": { "code": code, "message": message.into() } })
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn secret_of(text: Option<String>) -> Result<SecretKey, Value> {
    match text {
        None => Ok(SecretKey::generate()),
        Some(text) => unhex(&text).and_then(|b| <[u8; 32]>::try_from(b).ok()).map(|b| SecretKey::from_bytes(&b)).ok_or_else(|| failure("bad_request", "that isn't a device key")),
    }
}

fn start(secret: *const c_char) -> Result<Value, Value> {
    let runtime = runtime()?;
    let secret = secret_of(text(secret))?;
    let began = Instant::now();
    let client = runtime.block_on(Client::start(secret.clone())).map_err(|reason| failure("unavailable", reason))?;
    let key = client.key();
    let mut phone = phone();
    phone.secret = Some(secret.clone());
    if let Some(old) = phone.client.replace(client) {
        runtime.block_on(old.close());
    }
    phone.streams.clear();
    Ok(json!({ "key": key, "secret": hex(&secret.to_bytes()), "ms": began.elapsed().as_secs_f64() * 1000.0 }))
}

fn open(request: *const c_char) -> Result<Value, Value> {
    let runtime = runtime()?;
    let request: Value = text(request).and_then(|t| serde_json::from_str(&t).ok()).ok_or_else(|| failure("bad_request", "the request isn't JSON"))?;
    let client = phone().client.clone().ok_or_else(|| failure("unavailable", "the endpoint isn't started"))?;
    let key = request["key"].as_str().unwrap_or_default().to_string();
    let relay = Relay::from_request(&request).map_err(|f| failure(&f.code, f.message))?;
    let began = Instant::now();
    let (send, recv, ack) = runtime.block_on(client.lines(&key, &strings(&request["addrs"]), relay.as_ref(), request["header"].clone())).map_err(|f| failure(&f.code, f.message))?;
    let stream = NEXT.fetch_add(1, Ordering::Relaxed);
    phone().streams.insert(stream, Arc::new(Stream { send: AsyncMutex::new(send), recv: AsyncMutex::new(recv) }));
    Ok(json!({ "stream": stream, "ack": ack, "ms": began.elapsed().as_secs_f64() * 1000.0, "path": client.path(&key) }))
}

fn join(offer: *const c_char) -> Result<Value, Value> {
    let runtime = runtime()?;
    let offer: Value = text(offer).and_then(|t| serde_json::from_str(&t).ok()).ok_or_else(|| failure("bad_request", "that isn't a pairing code"))?;
    let (client, secret) = {
        let phone = phone();
        (phone.client.clone(), phone.secret.clone())
    };
    let (client, secret) = client.zip(secret).ok_or_else(|| failure("unavailable", "the endpoint isn't started"))?;
    runtime.block_on(kanna_net::phone::join(&client, &secret, &offer)).map_err(|f| failure(&f.code, f.message))
}

fn stream(id: u64) -> Result<Arc<Stream>, Value> {
    phone().streams.get(&id).cloned().ok_or_else(|| failure("closed", "that stream is closed"))
}

fn write(id: u64, line: *const c_char) -> Result<Value, Value> {
    let runtime = runtime()?;
    let line: Value = text(line).and_then(|t| serde_json::from_str(&t).ok()).ok_or_else(|| failure("bad_request", "the line isn't JSON"))?;
    let stream = stream(id)?;
    runtime.block_on(async { stream.send.lock().await.write(&line).await }).map_err(|f| failure(&f.code, f.message))?;
    Ok(json!({ "ok": true }))
}

fn read(id: u64) -> Result<Value, Value> {
    let runtime = runtime()?;
    let stream = stream(id)?;
    runtime.block_on(async { stream.recv.lock().await.read().await }).map_err(|f| failure(&f.code, f.message))
}

fn answer(result: Result<Value, Value>) -> *mut c_char {
    give(result.unwrap_or_else(|error| error))
}

#[no_mangle]
pub extern "C" fn kanna_phone_start(secret: *const c_char) -> *mut c_char {
    answer(start(secret))
}

#[no_mangle]
pub extern "C" fn kanna_phone_join(offer: *const c_char) -> *mut c_char {
    answer(join(offer))
}

#[no_mangle]
pub extern "C" fn kanna_phone_instance(key: *const c_char) -> *mut c_char {
    answer(text(key).and_then(|key| kanna_net::phone::instance_of(&key)).map(|instance| json!({ "instance": instance })).ok_or_else(|| failure("bad_request", "that isn't a key")))
}

#[no_mangle]
pub extern "C" fn kanna_phone_key(secret: *const c_char) -> *mut c_char {
    answer(secret_of(text(secret)).map(|secret| json!({ "key": secret.public().to_string() })))
}

#[no_mangle]
pub extern "C" fn kanna_phone_open(request: *const c_char) -> *mut c_char {
    answer(open(request))
}

#[no_mangle]
pub extern "C" fn kanna_phone_write(stream: u64, line: *const c_char) -> *mut c_char {
    answer(write(stream, line))
}

#[no_mangle]
pub extern "C" fn kanna_phone_read(stream: u64) -> *mut c_char {
    answer(read(stream))
}

#[no_mangle]
pub extern "C" fn kanna_phone_path(key: *const c_char) -> *mut c_char {
    let client = phone().client.clone();
    let path = match (client, text(key)) {
        (Some(client), Some(key)) => client.path(&key),
        _ => Value::Null,
    };
    give(json!({ "path": path }))
}

#[no_mangle]
pub extern "C" fn kanna_phone_highlight(file: *const c_char, source: *const c_char) -> *mut c_char {
    let (file, source) = (text(file).unwrap_or_default(), text(source).unwrap_or_default());
    let first = source.lines().next().unwrap_or_default();
    let found = kanna_syntax::language_for(&file, first).and_then(|language| kanna_syntax::highlight(language, &source)).unwrap_or_default();
    let spans: Vec<Value> = found.spans.iter().map(|s| json!({ "start": s.start, "end": s.end, "capture": kanna_syntax::CAPTURES.get(s.capture as usize) })).collect();
    give(json!({ "spans": spans }))
}

#[no_mangle]
pub extern "C" fn kanna_phone_stop() {
    let client = {
        let mut phone = phone();
        phone.streams.clear();
        phone.secret = None;
        phone.client.take()
    };
    if let (Some(client), Ok(runtime)) = (client, runtime()) {
        runtime.block_on(client.close());
    }
}

#[no_mangle]
pub extern "C" fn kanna_phone_free(text: *mut c_char) {
    if !text.is_null() {
        drop(unsafe { CString::from_raw(text) });
    }
}
