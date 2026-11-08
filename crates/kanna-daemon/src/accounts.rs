use kanna_mux::json::{json, string, Map, Value};
use crate::machines::Caller;
use kanna_mux::{CoreHandle, KmuxError, Responder};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const SECRETS: &str = "agent-secrets";

pub const BOOK: &str = "agent-accounts.json";

pub const FLAG: &str = "--with-account";

const RECEIVED: &str = "received";

const SOON: u64 = 300;

const REFRESH_WAIT: Duration = Duration::from_secs(30);

const RETRY_BROKEN: Duration = Duration::from_secs(60);

const SECRETS_WAIT: Duration = Duration::from_secs(5);

#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "Kanna agent accounts";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    ClaudeSubscription,
    ClaudeApiKey,
    CodexChatgpt,
    CodexApiKey,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::ClaudeSubscription, Kind::ClaudeApiKey, Kind::CodexChatgpt, Kind::CodexApiKey];

    pub fn name(self) -> &'static str {
        match self {
            Kind::ClaudeSubscription => "claude-subscription",
            Kind::ClaudeApiKey => "claude-api-key",
            Kind::CodexChatgpt => "codex-chatgpt",
            Kind::CodexApiKey => "codex-api-key",
        }
    }

    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }

    pub fn agent(self) -> &'static str {
        match self {
            Kind::ClaudeSubscription | Kind::ClaudeApiKey => "claude",
            Kind::CodexChatgpt | Kind::CodexApiKey => "codex",
        }
    }

    fn held(self) -> bool {
        self == Kind::CodexChatgpt
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Credential {
    Env(&'static str, String),
    CodexApiKey(String),
    CodexHolder { account: String, holder: String },
}

pub trait Secrets: Send + Sync {
    fn read(&self, name: &str) -> Result<Option<String>, String>;
    fn write(&self, name: &str, value: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;
}

pub struct Files(PathBuf);

impl Files {
    pub fn new(dir: PathBuf) -> Files {
        Files(dir)
    }
}

impl Secrets for Files {
    fn read(&self, name: &str) -> Result<Option<String>, String> {
        match std::fs::read_to_string(self.0.join(name)) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("can't read {}: {e}", self.0.join(name).display())),
        }
    }

    fn write(&self, name: &str, value: &str) -> Result<(), String> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        let failed = |e: std::io::Error| format!("can't keep a secret in {}: {e}", self.0.display());
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&self.0).map_err(failed)?;
        std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700)).map_err(failed)?;
        let temporary = self.0.join(format!(".{name}.{}", std::process::id()));
        let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&temporary).map_err(failed)?;
        file.write_all(value.as_bytes()).and_then(|()| file.sync_all()).map_err(failed)?;
        std::fs::rename(&temporary, self.0.join(name)).map_err(failed)
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        match std::fs::remove_file(self.0.join(name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("can't remove {}: {e}", self.0.join(name).display())),
            _ => Ok(()),
        }
    }
}

#[cfg(target_os = "macos")]
pub struct Keychain(String);

#[cfg(target_os = "macos")]
impl Keychain {
    pub fn new(state: &Path) -> Keychain {
        if let Ok(lock) = security_framework::os::macos::keychain::SecKeychain::disable_user_interaction() {
            std::mem::forget(lock);
        }
        Keychain(state.to_string_lossy().into_owned())
    }

    fn account(&self, name: &str) -> String {
        format!("{}/{name}", self.0)
    }
}

#[cfg(target_os = "macos")]
fn keychain_error(doing: &str, error: security_framework::base::Error) -> String {
    const INTERACTION_NOT_ALLOWED: i32 = -25308;
    const AUTH_FAILED: i32 = -25293;
    match error.code() {
        INTERACTION_NOT_ALLOWED => format!("can't {doing}: the Keychain wants a dialog answered (it's locked, or asks to allow this build), and Kanna never shows one"),
        AUTH_FAILED => format!("can't {doing}: the Keychain refused this build"),
        _ => format!("can't {doing}: {error}"),
    }
}

#[cfg(target_os = "macos")]
impl Secrets for Keychain {
    fn read(&self, name: &str) -> Result<Option<String>, String> {
        const NOT_FOUND: i32 = -25300;
        match security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, &self.account(name)) {
            Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| format!("the Keychain's {name} isn't text")),
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(keychain_error(&format!("read {name} from the Keychain"), e)),
        }
    }

    fn write(&self, name: &str, value: &str) -> Result<(), String> {
        security_framework::passwords::set_generic_password(KEYCHAIN_SERVICE, &self.account(name), value.as_bytes()).map_err(|e| keychain_error(&format!("keep {name} in the Keychain"), e))
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        const NOT_FOUND: i32 = -25300;
        match security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, &self.account(name)) {
            Err(e) if e.code() != NOT_FOUND => Err(keychain_error(&format!("remove {name} from the Keychain"), e)),
            _ => Ok(()),
        }
    }
}

pub struct Bounded {
    inner: Arc<dyn Secrets>,
    place: &'static str,
    wait: Duration,
    stuck: Arc<Mutex<Option<String>>>,
}

impl Bounded {
    pub fn new(inner: Arc<dyn Secrets>, place: &'static str, wait: Duration) -> Bounded {
        Bounded { inner, place, wait, stuck: Arc::default() }
    }

    fn run<T: Send + 'static>(&self, what: String, op: impl FnOnce(&dyn Secrets) -> Result<T, String> + Send + 'static) -> Result<T, String> {
        if let Some(earlier) = self.stuck.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            return Err(format!("can't {what}: {} still hasn't answered when asked to {earlier}", self.place));
        }
        let (sent, answer) = std::sync::mpsc::channel();
        let (inner, stuck, asked) = (self.inner.clone(), self.stuck.clone(), what.clone());
        let spawned = std::thread::Builder::new().name("kanna.secrets".into()).spawn(move || {
            let result = op(&*inner);
            let mut stuck = stuck.lock().unwrap_or_else(|p| p.into_inner());
            if stuck.as_deref() == Some(asked.as_str()) {
                *stuck = None;
            }
            let _ = sent.send(result);
        });
        if let Err(e) = spawned {
            return Err(format!("can't {what}: no thread to ask {} on: {e}", self.place));
        }
        match answer.recv_timeout(self.wait) {
            Ok(result) => result,
            Err(_) => {
                let mut stuck = self.stuck.lock().unwrap_or_else(|p| p.into_inner());
                if let Ok(result) = answer.try_recv() {
                    return result;
                }
                *stuck = Some(what.clone());
                Err(format!("can't {what}: {} didn't answer in {}s (a dialog nobody can click, or a locked login keychain)", self.place, self.wait.as_secs_f32()))
            }
        }
    }
}

impl Secrets for Bounded {
    fn read(&self, name: &str) -> Result<Option<String>, String> {
        let owned = name.to_string();
        self.run(format!("read {name}"), move |inner| inner.read(&owned))
    }

    fn write(&self, name: &str, value: &str) -> Result<(), String> {
        let (owned, value) = (name.to_string(), value.to_string());
        self.run(format!("keep {name}"), move |inner| inner.write(&owned, &value))
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        let owned = name.to_string();
        self.run(format!("remove {name}"), move |inner| inner.delete(&owned))
    }
}

pub fn secrets(state: &Path) -> Arc<dyn Secrets> {
    #[cfg(target_os = "macos")]
    if std::env::var("KANNA_KEYCHAIN").map_or(true, |v| v != "0") {
        return Arc::new(Bounded::new(Arc::new(Keychain::new(state)), "the Keychain", SECRETS_WAIT));
    }
    Arc::new(Bounded::new(Arc::new(Files::new(state.join(SECRETS))), "the secrets folder", SECRETS_WAIT))
}

pub trait Refresher: Send + Sync {
    fn refresh(&self, login: &str, program: Option<&str>) -> Result<String, String>;
}

pub struct Accounts {
    secrets: Arc<dyn Secrets>,
    path: PathBuf,
    book: Mutex<Value>,
    refresher: Box<dyn Refresher>,
    refreshing: Mutex<()>,
    broken: Mutex<BTreeMap<String, (String, Instant)>>,
    on_broken: Mutex<Option<Box<dyn Fn(Value) + Send>>>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn bad(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

fn new_id() -> String {
    use sha2::Digest;
    let mut seed = [0u8; 16];
    let random = std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut seed));
    let digest = sha2::Sha256::digest(format!("{random:?}{seed:?}{}{:?}", std::process::id(), SystemTime::now()));
    format!("acct-{}", digest.iter().take(4).map(|b| format!("{b:02x}")).collect::<String>())
}

pub fn claims(jwt: &str) -> Option<Value> {
    use base64::Engine;
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn tokens(login: &str) -> Result<Value, KmuxError> {
    let login: Value = serde_json::from_str(login).map_err(|_| KmuxError::new("broken", "the Codex login isn't one Codex wrote"))?;
    let access = string(&login["tokens"], "access_token").filter(|t| !t.is_empty()).ok_or_else(|| KmuxError::new("broken", "the Codex login has no ChatGPT access token"))?;
    let account = string(&login["tokens"], "account_id").map(String::from).or_else(|| claims(access).and_then(|c| string(&c["https://api.openai.com/auth"], "chatgpt_account_id").map(String::from)));
    let account = account.ok_or_else(|| KmuxError::new("broken", "the Codex login has no ChatGPT account"))?;
    let plan = claims(access).and_then(|c| string(&c["https://api.openai.com/auth"], "chatgpt_plan_type").map(String::from));
    Ok(json!({ "accessToken": access, "chatgptAccountId": account, "chatgptPlanType": plan }))
}

fn expires_soon(access: &str) -> bool {
    claims(access).and_then(|c| c["exp"].as_u64()).map_or(false, |exp| exp < now() + SOON)
}

fn validate(kind: Kind, secret: &str) -> Result<(), KmuxError> {
    let secret = secret.trim();
    let ok = match kind {
        Kind::ClaudeSubscription => secret.starts_with("sk-ant-oat"),
        Kind::ClaudeApiKey => secret.starts_with("sk-ant-api"),
        Kind::CodexApiKey => secret.starts_with("sk-"),
        Kind::CodexChatgpt => {
            let login: Value = serde_json::from_str(secret).map_err(|_| bad("a Codex ChatGPT login is the auth.json Codex wrote"))?;
            tokens(secret).is_ok() && string(&login["tokens"], "refresh_token").is_some_and(|r| !r.is_empty())
        }
    };
    match ok {
        true => Ok(()),
        false => Err(bad(format!("that isn't a {} credential", kind.name()))),
    }
}

fn book_at(path: &Path) -> Value {
    std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).filter(Value::is_object).unwrap_or_else(|| json!({ "version": 0, "own": [], "delivered": {} }))
}

fn listed_in(book: &Value) -> Option<Map> {
    let entries = book["received"].as_array().cloned().unwrap_or_default();
    entries.iter().all(Value::is_object).then(|| entries.into_iter().filter_map(|entry| Some((string(&entry, "from")?.to_string(), entry))).collect())
}

fn without_secrets(set: &Value) -> Value {
    let accounts: Vec<Value> = set["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|account| {
            let mut account = account.clone();
            account.as_object_mut().map(|a| a.remove("secret"));
            account
        })
        .collect();
    json!({ "version": set["version"], "accounts": accounts })
}

fn has(book: &Value, agent: &str) -> bool {
    let own = book["own"].as_array().into_iter().flatten().any(|a| a["agent"] == agent);
    let received = || match listed_in(book) {
        Some(listed) => listed.values().flat_map(|set| set["accounts"].as_array().cloned().unwrap_or_default()).any(|a| a["agent"] == agent),
        None => true,
    };
    own || received()
}

fn named(account: &Value) -> String {
    let agent = match account["agent"].as_str() {
        Some("claude") => "Claude",
        Some("codex") => "Codex",
        _ => "agent",
    };
    match string(account, "label") {
        Some(label) => format!("your {agent} account \"{label}\""),
        None => format!("your {agent} account"),
    }
}

impl Accounts {
    pub fn open(state: &Path, secrets: Arc<dyn Secrets>, refresher: Box<dyn Refresher>) -> Accounts {
        let path = state.join(BOOK);
        let book = book_at(&path);
        Accounts { secrets, path, book: Mutex::new(book), refresher, refreshing: Mutex::new(()), broken: Mutex::default(), on_broken: Mutex::default() }
    }

    pub fn on_broken(&self, report: Box<dyn Fn(Value) + Send>) {
        *self.on_broken.lock().unwrap_or_else(|p| p.into_inner()) = Some(report);
    }

    fn book(&self) -> std::sync::MutexGuard<'_, Value> {
        self.book.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn save(&self, book: &Value) -> Result<(), KmuxError> {
        let text = format!("{book:#}\n");
        let temporary = self.path.with_extension(format!("json.{}", std::process::id()));
        std::fs::write(&temporary, text).and_then(|()| std::fs::rename(&temporary, &self.path)).map_err(|e| KmuxError::new("failed", format!("can't save {}: {e}", self.path.display())))
    }

    fn forget(&self, name: &str) {
        if let Err(why) = self.secrets.delete(name) {
            log!("kanna-daemon: {why}");
        }
    }

    pub fn version(&self) -> u64 {
        self.book()["version"].as_u64().unwrap_or(0)
    }

    fn own(&self) -> Vec<Value> {
        self.book()["own"].as_array().cloned().unwrap_or_default()
    }

    pub fn received(&self) -> Result<Map, String> {
        if self.book()["received"].as_array().map_or(true, Vec::is_empty) {
            return Ok(Map::new());
        }
        let text = self.secrets.read(RECEIVED).map_err(|why| format!("can't read the accounts your other machines gave this one: {why}"))?;
        Ok(text.and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default())
    }

    fn listed(&self) -> Result<Map, String> {
        if let Some(listed) = listed_in(&self.book()) {
            return Ok(listed);
        }
        Ok(self.received()?.iter().map(|(source, set)| (source.clone(), without_secrets(set))).collect())
    }

    fn keep_received(&self, received: &Map) -> Result<(), KmuxError> {
        match received.is_empty() {
            true => self.secrets.delete(RECEIVED).map_err(|e| KmuxError::new("failed", e))?,
            false => self.secrets.write(RECEIVED, &Value::Object(received.clone()).to_string()).map_err(|e| KmuxError::new("failed", e))?,
        }
        let mut book = self.book();
        let listed: Vec<Value> = received
            .iter()
            .map(|(source, set)| {
                let mut entry = without_secrets(set);
                entry["from"] = json!(source);
                entry
            })
            .collect();
        book["received"] = json!(listed);
        self.save(&book)
    }

    pub fn add(&self, kind: Kind, secret: &str, label: Option<&str>, program: Option<&str>, me: &str) -> Result<Value, KmuxError> {
        validate(kind, secret)?;
        let id = new_id();
        self.secrets.write(&id, secret.trim()).map_err(|e| KmuxError::new("failed", e))?;
        let mut book = self.book();
        let mut own: Vec<Value> = book["own"].as_array().cloned().unwrap_or_default();
        let (replaced, kept): (Vec<Value>, Vec<Value>) = own.drain(..).partition(|a| a["agent"] == kind.agent());
        let account = json!({ "id": id, "agent": kind.agent(), "kind": kind.name(), "label": label.unwrap_or(kind.name()), "added_at": now(), "holder": kind.held().then_some(me), "program": program });
        let mut own = kept;
        own.push(account.clone());
        book["own"] = json!(own);
        book["version"] = json!(book["version"].as_u64().unwrap_or(0) + 1);
        self.save(&book)?;
        drop(book);
        for old in &replaced {
            if let Some(old) = string(old, "id") {
                self.forget(old);
                self.broken.lock().unwrap_or_else(|p| p.into_inner()).remove(old);
            }
        }
        Ok(json!({ "account": account, "replaced": replaced }))
    }

    pub fn remove(&self, which: &str) -> Result<Value, KmuxError> {
        let mut book = self.book();
        let own: Vec<Value> = book["own"].as_array().cloned().unwrap_or_default();
        let (gone, kept): (Vec<Value>, Vec<Value>) = own.into_iter().partition(|a| a["id"] == which || a["agent"] == which || a["kind"] == which);
        if gone.is_empty() {
            return Err(KmuxError::new("not_found", format!("no account \"{which}\" was added on this machine (kanna accounts lists them)")));
        }
        book["own"] = json!(kept);
        book["version"] = json!(book["version"].as_u64().unwrap_or(0) + 1);
        self.save(&book)?;
        drop(book);
        for account in &gone {
            if let Some(id) = string(account, "id") {
                self.forget(id);
                self.broken.lock().unwrap_or_else(|p| p.into_inner()).remove(id);
            }
        }
        Ok(json!({ "removed": gone }))
    }

    pub fn list(&self, names: &dyn Fn(&str) -> String) -> Value {
        let broken = self.broken.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let mark = |mut account: Value| {
            if let Some((why, _)) = string(&account, "id").and_then(|id| broken.get(id)) {
                account["broken"] = json!(why);
            }
            if let Some(holder) = string(&account, "holder").map(String::from) {
                account["holder_name"] = json!(names(&holder));
            }
            account.as_object_mut().map(|a| a.remove("secret"));
            account
        };
        let own: Vec<Value> = self.own().into_iter().map(mark).collect();
        let mut received = vec![];
        let listed = self.listed();
        for (source, set) in listed.clone().unwrap_or_default() {
            for account in set["accounts"].as_array().cloned().unwrap_or_default() {
                let mut account = mark(account);
                account["from"] = json!(names(&source));
                received.push(account);
            }
        }
        let mut answer = json!({ "accounts": own, "received": received, "version": self.version() });
        if let Err(why) = listed {
            answer["received_error"] = json!(why);
        }
        answer
    }

    pub fn payload(&self) -> Result<Value, String> {
        let mut accounts = vec![];
        for mut account in self.own() {
            let Some(kind) = string(&account, "kind").and_then(Kind::parse) else { continue };
            if !kind.held() {
                let Some(id) = string(&account, "id") else { continue };
                let Some(secret) = self.secrets.read(id).map_err(|why| format!("can't read {}: {why}", named(&account)))? else { continue };
                account["secret"] = json!(secret);
            }
            account.as_object_mut().map(|a| a.remove("program"));
            accounts.push(account);
        }
        Ok(json!({ "version": self.version(), "accounts": accounts }))
    }

    pub fn outbox(&self, peers: &BTreeSet<String>) -> Vec<String> {
        let book = self.book();
        let version = book["version"].as_u64().unwrap_or(0);
        peers.iter().filter(|peer| book["delivered"][peer.as_str()].as_u64() != Some(version)).cloned().collect()
    }

    pub fn delivered(&self, peer: &str, version: u64) {
        let mut book = self.book();
        book["delivered"][peer] = json!(version);
        let _ = self.save(&book);
    }

    pub fn supply(&self, source: &str, payload: &Value) -> Result<Value, KmuxError> {
        let version = payload["version"].as_u64().ok_or_else(|| bad("accounts supply needs a version"))?;
        let accounts = payload["accounts"].as_array().ok_or_else(|| bad("accounts supply needs accounts"))?;
        let mut kept = vec![];
        for account in accounts {
            let kind = string(account, "kind").and_then(Kind::parse).ok_or_else(|| bad("an account of a kind this Kanna doesn't know"))?;
            match (kind.held(), string(account, "secret"), string(account, "holder")) {
                (true, _, Some(_)) | (false, Some(_), _) => kept.push(account.clone()),
                _ => return Err(bad(format!("a {} account without its {}", kind.name(), if kind.held() { "holder" } else { "secret" }))),
            }
        }
        let mut received = self.received().map_err(|why| KmuxError::new("failed", why))?;
        if received.get(source).and_then(|s| s["version"].as_u64()).is_some_and(|v| v > version) {
            return Ok(json!({ "version": received[source]["version"] }));
        }
        match kept.is_empty() {
            true => received.remove(source),
            false => received.insert(source.to_string(), json!({ "version": version, "accounts": kept })),
        };
        self.keep_received(&received)?;
        Ok(json!({ "version": version, "accounts": kept.len() }))
    }

    pub fn prune(&self, trusted: &BTreeSet<String>, me_revoked: bool) -> bool {
        let keeps = |source: &String| !me_revoked && trusted.contains(source);
        let held = self.received();
        let mut sources: BTreeSet<String> = self.listed().map(|listed| listed.keys().cloned().collect()).unwrap_or_default();
        match &held {
            Ok(received) => sources.extend(received.keys().cloned()),
            Err(_) if me_revoked => {
                sources.insert(RECEIVED.to_string());
            }
            Err(why) => log!("kanna-daemon: {why}"),
        }
        let mut changed = false;
        if sources.iter().any(|source| !keeps(source)) {
            let kept = match (sources.iter().all(|source| !keeps(source)), held) {
                (true, _) => Ok(Map::new()),
                (false, held) => held.map(|mut received| {
                    received.retain(|source, _| keeps(source));
                    received
                }),
            };
            match kept.map_err(|why| KmuxError::new("failed", why)).and_then(|kept| self.keep_received(&kept)) {
                Ok(()) => changed = true,
                Err(error) => log!("kanna-daemon: can't drop the accounts of machines you revoked: {}", error.message),
            }
        }
        let mut book = self.book();
        let delivered = book["delivered"].as_object().cloned().unwrap_or_default();
        let kept: Map = delivered.iter().filter(|(peer, _)| trusted.contains(peer.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
        if kept.len() != delivered.len() {
            book["delivered"] = Value::Object(kept);
            let _ = self.save(&book);
        }
        changed
    }

    fn account_for(&self, agent: &str) -> Result<Option<(Value, Option<String>)>, String> {
        if let Some(own) = self.own().into_iter().find(|a| a["agent"] == agent) {
            return Ok(Some((own, None)));
        }
        let newest = self.listed()?.into_iter().flat_map(|(source, set)| set["accounts"].as_array().cloned().unwrap_or_default().into_iter().map(move |a| (a, source.clone()))).filter(|(a, _)| a["agent"] == agent).max_by_key(|(a, _)| a["added_at"].as_u64());
        Ok(newest.map(|(account, source)| (account, Some(source))))
    }

    fn secret_of(&self, account: &Value, source: Option<&str>) -> Result<Option<String>, String> {
        let unreadable = |why: String| format!("can't read {}: {why}", named(account));
        let Some(id) = string(account, "id") else { return Ok(None) };
        match source {
            None => self.secrets.read(id).map_err(unreadable),
            Some(source) => {
                let received = self.received().map_err(unreadable)?;
                let accounts = received.get(source).and_then(|set| set["accounts"].as_array().cloned()).unwrap_or_default();
                Ok(accounts.iter().find(|a| a["id"] == id).and_then(|a| string(a, "secret")).map(String::from))
            }
        }
    }

    pub fn credential(&self, agent: &str) -> Result<Option<Credential>, String> {
        let Some((account, source)) = self.account_for(agent)? else { return Ok(None) };
        let Some(kind) = string(&account, "kind").and_then(Kind::parse) else { return Ok(None) };
        if kind == Kind::CodexChatgpt {
            let (Some(id), Some(holder)) = (string(&account, "id"), string(&account, "holder")) else { return Ok(None) };
            return Ok(Some(Credential::CodexHolder { account: id.to_string(), holder: holder.to_string() }));
        }
        let Some(secret) = self.secret_of(&account, source.as_deref())? else { return Ok(None) };
        Ok(Some(match kind {
            Kind::ClaudeSubscription => Credential::Env("CLAUDE_CODE_OAUTH_TOKEN", secret),
            Kind::ClaudeApiKey => Credential::Env("ANTHROPIC_API_KEY", secret),
            Kind::CodexApiKey | Kind::CodexChatgpt => Credential::CodexApiKey(secret),
        }))
    }

    pub fn holder_of(&self, account: &str) -> Option<String> {
        let own = self.own().into_iter().find(|a| a["id"] == account && a["kind"] == Kind::CodexChatgpt.name()).and_then(|a| string(&a, "holder").map(String::from));
        own.or_else(|| self.listed().ok()?.values().flat_map(|set| set["accounts"].as_array().cloned().unwrap_or_default()).find(|a| a["id"] == account).and_then(|a| string(&a, "holder").map(String::from)))
    }

    pub fn token(&self, account: &str, previous: Option<&str>) -> Result<Value, KmuxError> {
        let Some(held) = self.own().into_iter().find(|a| a["id"] == account && a["kind"] == Kind::CodexChatgpt.name()) else {
            return Err(KmuxError::new("not_found", format!("no Codex account {account} is held here: it was removed")));
        };
        let _one_at_a_time = self.refreshing.lock().unwrap_or_else(|p| p.into_inner());
        let login = self.secrets.read(account).map_err(|why| KmuxError::new("failed", format!("can't read {}: {why}", named(&held))))?;
        let login = login.ok_or_else(|| KmuxError::new("not_found", format!("the Codex account {account} has no login here")))?;
        let current = tokens(&login)?;
        let access = string(&current, "accessToken").unwrap_or_default();
        let stale = match previous {
            Some(previous) => previous == access,
            None => expires_soon(access),
        };
        if !stale {
            return Ok(current);
        }
        if let Some((why, at)) = self.broken.lock().unwrap_or_else(|p| p.into_inner()).get(account).cloned() {
            if at.elapsed() < RETRY_BROKEN {
                return Err(KmuxError::new("broken", why));
            }
        }
        match self.refresher.refresh(&login, string(&held, "program")) {
            Ok(fresh) => {
                let replied = tokens(&fresh)?;
                self.secrets.write(account, &fresh).map_err(|e| KmuxError::new("failed", e))?;
                self.broken.lock().unwrap_or_else(|p| p.into_inner()).remove(account);
                Ok(replied)
            }
            Err(why) => {
                let first = self.broken.lock().unwrap_or_else(|p| p.into_inner()).insert(account.to_string(), (why.clone(), Instant::now())).is_none();
                if first {
                    if let Some(report) = self.on_broken.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
                        report(json!({ "id": account, "agent": held["agent"], "label": held["label"], "why": why }));
                    }
                }
                Err(KmuxError::new("broken", why))
            }
        }
    }
}

pub const COMMAND: &str = "accounts";

const SUPPLY_EVERY: Duration = Duration::from_secs(60);

const BACKOFF_MAX: Duration = Duration::from_secs(900);

pub struct Service {
    accounts: Arc<Accounts>,
    caller: Option<Caller>,
    wake: Mutex<Sender<()>>,
    told: Mutex<BTreeSet<String>>,
    on_unreadable: Mutex<Option<Box<dyn Fn(Value) + Send>>>,
}

struct Peers {
    trusted: BTreeSet<String>,
    me_revoked: bool,
    names: BTreeMap<String, String>,
}

fn peers(status: &Value) -> Peers {
    let mut found = Peers { trusted: BTreeSet::new(), me_revoked: false, names: BTreeMap::new() };
    for peer in status["peers"].as_array().into_iter().flatten() {
        let (Some(key), Some(name)) = (string(peer, "key"), string(peer, "name")) else { continue };
        found.names.insert(key.to_string(), name.to_string());
        match (peer["me"] == true, peer["revoked"] == true, peer["trusted"] == true) {
            (true, true, _) => found.me_revoked = true,
            (false, false, true) => {
                found.trusted.insert(key.to_string());
            }
            _ => {}
        }
    }
    found
}

fn object(value: Value) -> Result<Map, KmuxError> {
    match value {
        Value::Object(map) => Ok(map),
        other => Ok(Map::from_iter([("value".to_string(), other)])),
    }
}

impl Service {
    pub fn start(state: &Path, caller: Option<Caller>) -> Arc<Service> {
        Service::with(Accounts::open(state, secrets(state), Box::new(CodexRefresher)), caller)
    }

    pub fn with(accounts: Accounts, caller: Option<Caller>) -> Arc<Service> {
        let (wake, woken) = std::sync::mpsc::channel();
        let service = Arc::new(Service { accounts: Arc::new(accounts), caller, wake: Mutex::new(wake), told: Mutex::default(), on_unreadable: Mutex::default() });
        let supplier = Arc::downgrade(&service);
        let _ = std::thread::Builder::new().name("kanna.accounts".into()).spawn(move || supply(supplier, woken));
        service.changed();
        service
    }

    pub fn on_broken(&self, report: Box<dyn Fn(Value) + Send>) {
        self.accounts.on_broken(report);
    }

    pub fn on_unreadable(&self, report: Box<dyn Fn(Value) + Send>) {
        *self.on_unreadable.lock().unwrap_or_else(|p| p.into_inner()) = Some(report);
    }

    fn unreadable(&self, args: &Value) -> Result<Value, KmuxError> {
        let agent = string(args, "agent").ok_or_else(|| bad("accounts unreadable needs an agent"))?;
        let why = string(args, "why").unwrap_or("its secret couldn't be read");
        log!("kanna-daemon: {agent} started on this machine's own sign-in: {why}");
        let first = self.told.lock().unwrap_or_else(|p| p.into_inner()).insert(agent.to_string());
        if first {
            if let Some(report) = self.on_unreadable.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
                report(json!({ "agent": agent, "why": why }));
            }
        }
        Ok(json!({ "told": first }))
    }

    pub fn changed(&self) {
        let _ = self.wake.lock().unwrap_or_else(|p| p.into_inner()).send(());
    }

    pub fn install(self: &Arc<Self>, core: &CoreHandle) {
        let service = self.clone();
        core.add_command(COMMAND, std::rc::Rc::new(move |args, responder| service.local(args, responder)));
    }

    pub fn withdraw(&self, host: &str) -> Option<bool> {
        let caller = self.caller.as_ref()?;
        let held = self.accounts.own().iter().any(|a| a["kind"] != Kind::CodexChatgpt.name());
        if !held {
            return None;
        }
        let told = caller.call(host, COMMAND, json!({ "action": "supply", "version": self.accounts.version(), "accounts": [] }));
        Some(told.is_ok())
    }

    fn me(&self) -> String {
        self.caller.as_ref().map(Caller::key).unwrap_or_default()
    }

    fn peers(&self) -> Peers {
        self.caller.as_ref().map(|caller| peers(&caller.status())).unwrap_or(Peers { trusted: BTreeSet::new(), me_revoked: false, names: BTreeMap::new() })
    }

    fn list(&self) -> Value {
        let peers = self.peers();
        let me = self.me();
        let name = |key: &str| match key == me {
            true => "this machine".to_string(),
            false => peers.names.get(key).cloned().unwrap_or_else(|| key.chars().take(10).collect()),
        };
        let mut listed = self.accounts.list(&name);
        listed["machines"] = json!(peers.trusted.iter().map(|k| name(k)).collect::<Vec<_>>());
        listed
    }

    fn local(self: &Arc<Self>, args: Value, responder: Responder) {
        let action = string(&args, "action").unwrap_or("list").to_string();
        let service = self.clone();
        let _ = std::thread::Builder::new().name("kanna.accounts.request".into()).spawn(move || {
            let result = match action.as_str() {
                "list" => Ok(service.list()),
                "add" => service.add(&args),
                "remove" => match string(&args, "account") {
                    Some(which) => service.accounts.remove(which).map(|mut removed| {
                        service.changed();
                        removed["machines"] = service.list()["machines"].clone();
                        removed
                    }),
                    None => Err(bad("accounts remove needs an ACCOUNT: its ID, or claude or codex")),
                },
                "token" => service.token(&args),
                "unreadable" => service.unreadable(&args),
                "supply" => Err(KmuxError::new("refused", "only your other machines supply accounts")),
                other => Err(bad(format!("unknown accounts action \"{other}\" (list, add, remove or token)"))),
            };
            responder.reply(result.and_then(object));
        });
    }

    fn add(&self, args: &Value) -> Result<Value, KmuxError> {
        let kind = string(args, "kind").and_then(Kind::parse).ok_or_else(|| bad(format!("accounts add needs a kind: {}", Kind::ALL.map(Kind::name).join(", "))))?;
        let secret = string(args, "secret").filter(|s| !s.trim().is_empty()).ok_or_else(|| bad("accounts add needs the account's secret"))?;
        let mut added = self.accounts.add(kind, secret, string(args, "label"), string(args, "program"), &self.me())?;
        self.told.lock().unwrap_or_else(|p| p.into_inner()).remove(kind.agent());
        self.changed();
        added["machines"] = self.list()["machines"].clone();
        Ok(added)
    }

    fn token(&self, args: &Value) -> Result<Value, KmuxError> {
        let account = match string(args, "account") {
            Some(account) => account.to_string(),
            None => match self.accounts.credential("codex").map_err(|why| KmuxError::new("failed", why))? {
                Some(Credential::CodexHolder { account, .. }) => account,
                _ => return Err(KmuxError::new("not_found", "no Codex ChatGPT account is set up")),
            },
        };
        let holder = self.accounts.holder_of(&account).ok_or_else(|| KmuxError::new("not_found", format!("no Codex account {account} here: it was removed")))?;
        let previous = string(args, "previous");
        let tokens = match (holder == self.me(), &self.caller) {
            (true, _) => self.accounts.token(&account, previous)?,
            (false, Some(caller)) => caller.call(&holder, COMMAND, json!({ "action": "token", "account": account, "previous": previous }))?["tokens"].clone(),
            (false, None) => return Err(KmuxError::new("unreachable", "this machine can't reach the one holding your Codex account")),
        };
        Ok(json!({ "tokens": tokens }))
    }

    pub fn peer(self: &Arc<Self>, from: &str, request: Value, reply: Box<dyn FnOnce(Value) + Send>) {
        let service = self.clone();
        let from = from.to_string();
        let _ = std::thread::Builder::new().name("kanna.accounts.peer".into()).spawn(move || {
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let args = request.get("args").cloned().unwrap_or_else(|| json!({}));
            let result = match string(&args, "action") {
                Some("supply") => {
                    let peers = service.peers();
                    match peers.names.iter().find(|(key, name)| **name == from && peers.trusted.contains(*key)) {
                        Some((source, _)) => service.accounts.supply(source, &args),
                        None => Err(KmuxError::new("unpaired", format!("{from} isn't one of your machines"))),
                    }
                }
                Some("token") => match string(&args, "account") {
                    Some(account) => service.accounts.token(account, string(&args, "previous")).map(|tokens| json!({ "tokens": tokens })),
                    None => Err(bad("accounts token needs an account")),
                },
                _ => Err(KmuxError::new("refused", "your other machines may only supply accounts or ask for a Codex token")),
            };
            reply(match result {
                Ok(fields) => {
                    let mut answer = json!({ "id": id, "ok": true });
                    for (key, value) in fields.as_object().cloned().unwrap_or_default() {
                        answer[key] = value;
                    }
                    answer
                }
                Err(error) => json!({ "id": id, "ok": false, "error": { "code": error.code, "message": error.message } }),
            });
        });
    }
}

fn supply(service: std::sync::Weak<Service>, woken: Receiver<()>) {
    let mut waits: BTreeMap<String, (Instant, Duration)> = BTreeMap::new();
    loop {
        match woken.recv_timeout(SUPPLY_EVERY) {
            Ok(()) => waits.clear(),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let Some(service) = service.upgrade() else { return };
        let Some(caller) = service.caller.clone() else { continue };
        let peers = peers(&caller.status());
        service.accounts.prune(&peers.trusted, peers.me_revoked);
        if peers.me_revoked || service.accounts.version() == 0 {
            continue;
        }
        for peer in service.accounts.outbox(&peers.trusted) {
            if waits.get(&peer).is_some_and(|(at, _)| Instant::now() < *at) {
                continue;
            }
            let mut payload = match service.accounts.payload() {
                Ok(payload) => payload,
                Err(why) => {
                    let wait = waits.get(&peer).map_or(SUPPLY_EVERY, |(_, wait)| (*wait * 2).min(BACKOFF_MAX));
                    waits.insert(peer.clone(), (Instant::now() + wait, wait));
                    log!("kanna-daemon: couldn't supply your agent accounts: {why}");
                    continue;
                }
            };
            let version = payload["version"].as_u64().unwrap_or(0);
            payload["action"] = json!("supply");
            match caller.call(&peer, COMMAND, payload) {
                Ok(_) => {
                    service.accounts.delivered(&peer, version);
                    waits.remove(&peer);
                }
                Err(error) => {
                    let wait = waits.get(&peer).map_or(SUPPLY_EVERY, |(_, wait)| (*wait * 2).min(BACKOFF_MAX));
                    waits.insert(peer.clone(), (Instant::now() + wait, wait));
                    eprintln!("kanna-daemon: couldn't supply your agent accounts to {}: {}", peers.names.get(&peer).map_or(peer.as_str(), String::as_str), error.message);
                }
            }
        }
    }
}

pub struct CodexRefresher;

impl Refresher for CodexRefresher {
    fn refresh(&self, login: &str, program: Option<&str>) -> Result<String, String> {
        let folder = private_folder()?;
        let result = refresh_in(&folder, login, program.unwrap_or("codex"));
        let _ = std::fs::remove_dir_all(&folder);
        result
    }
}

fn private_folder() -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let folder = std::env::temp_dir().join(format!("kanna-holder-{}", new_id()));
    std::fs::DirBuilder::new().mode(0o700).create(&folder).map_err(|e| format!("can't make a private folder for the refresh: {e}"))?;
    Ok(folder)
}

fn refresh_in(folder: &Path, login: &str, program: &str) -> Result<String, String> {
    Files::new(folder.to_path_buf()).write("auth.json", login)?;
    let mut server = Command::new(program)
        .args(["-c", "cli_auth_credentials_store=\"file\"", "app-server", "--listen", "stdio://"])
        .env("CODEX_HOME", folder)
        .current_dir(folder)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("can't run {program} to refresh the Codex login: {e}"))?;
    let outcome = (|| {
        let mut input = server.stdin.take().ok_or("no stdin")?;
        let output = server.stdout.take().ok_or("no stdout")?;
        let (lines, replies) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
        let mut send = |message: Value| writeln!(input, "{message}").map_err(|e| format!("lost Codex: {e}"));
        send(json!({ "id": 1, "method": "initialize", "params": { "clientInfo": { "name": "kanna_holder", "version": kanna_build::VERSION } } }))?;
        wait_for(&replies, 1)?;
        send(json!({ "method": "initialized", "params": {} }))?;
        send(json!({ "id": 2, "method": "account/read", "params": { "refreshToken": true } }))?;
        let reply = wait_for(&replies, 2)?;
        if reply["result"]["account"].is_null() {
            return Err("Codex is signed out of this account".to_string());
        }
        Ok::<(), String>(())
    })();
    let _ = server.kill();
    let _ = server.wait();
    outcome?;
    let fresh = std::fs::read_to_string(folder.join("auth.json")).map_err(|e| format!("Codex left no login: {e}"))?;
    let (before, after) = (tokens(login).map_err(|e| e.message)?, tokens(&fresh).map_err(|e| e.message)?);
    match before["accessToken"] == after["accessToken"] {
        true => Err("Codex couldn't refresh the login: sign in to Codex again".to_string()),
        false => Ok(fresh),
    }
}

fn wait_for(replies: &std::sync::mpsc::Receiver<String>, id: u64) -> Result<Value, String> {
    let deadline = Instant::now() + REFRESH_WAIT;
    loop {
        let left = deadline.checked_duration_since(Instant::now()).ok_or("Codex didn't answer the refresh in time")?;
        let line = replies.recv_timeout(left).map_err(|_| "Codex didn't answer the refresh in time".to_string())?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        if message["id"] == id && message.get("method").is_none() {
            if let Some(message) = message["error"]["message"].as_str() {
                return Err(format!("Codex couldn't refresh the login: {}", message.chars().take(200).collect::<String>()));
            }
            return Ok(message);
        }
    }
}

pub fn daemon_socket(state: &Path) -> PathBuf {
    std::env::var_os("KANNA_SOCKET").filter(|s| !s.is_empty()).map_or_else(|| state.join(kanna_build::SOCKET), PathBuf::from)
}

pub fn ask_daemon(socket: &Path, args: Value) -> Result<Value, String> {
    ask_daemon_within(socket, args, Duration::from_secs(60))
}

fn ask_daemon_within(socket: &Path, args: Value, wait: Duration) -> Result<Value, String> {
    use std::os::unix::net::UnixStream;
    let stream = UnixStream::connect(socket).map_err(|e| format!("can't reach Kanna at {}: {e}", socket.display()))?;
    let _ = stream.set_read_timeout(Some(wait));
    let _ = stream.set_write_timeout(Some(wait));
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    writeln!(writer, "{}", json!({ "id": 1, "cmd": "accounts", "args": args })).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(|e| e.to_string())?;
    let reply: Value = serde_json::from_str(&line).map_err(|e| format!("Kanna's reply isn't JSON: {e}"))?;
    match reply["ok"] == true {
        true => Ok(reply),
        false => Err(reply["error"]["message"].as_str().unwrap_or("Kanna refused").to_string()),
    }
}

pub fn has_credential(state: &Path, agent: &str) -> bool {
    has(&book_at(&state.join(BOOK)), agent)
}

pub fn started_without(state: &Path, agent: &str, why: &str) {
    let name = match agent {
        "claude" => "Claude",
        _ => "Codex",
    };
    eprintln!("kanna: {why}, so {name} starts on this machine's own sign-in");
    let _ = ask_daemon_within(&daemon_socket(state), json!({ "action": "unreadable", "agent": agent, "why": why }), Duration::from_secs(2));
}

pub fn run_with_account(state: &Path, program: &str, args: &[String]) -> i32 {
    use std::os::unix::process::CommandExt;
    let accounts = Accounts::open(state, secrets(state), Box::new(CodexRefresher));
    let mut command = Command::new(program);
    command.args(args);
    match accounts.credential("claude") {
        Ok(Some(Credential::Env(name, value))) => {
            command.env(name, value);
            for other in ["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"].into_iter().filter(|other| *other != name) {
                command.env_remove(other);
            }
        }
        Ok(_) => {}
        Err(why) => started_without(state, "claude", &why),
    }
    let error = command.exec();
    eprintln!("kanna: couldn't run {program}: {error}");
    127
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        calls: Arc<AtomicUsize>,
        fail: bool,
        slow: Duration,
    }

    impl Refresher for Fake {
        fn refresh(&self, login: &str, _program: Option<&str>) -> Result<String, String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            std::thread::sleep(self.slow);
            if self.fail {
                return Err("the refresh token was revoked".into());
            }
            let mut login: Value = serde_json::from_str(login).unwrap();
            login["tokens"]["access_token"] = json!(jwt(&format!("acc-{n}"), now() + 3600));
            Ok(login.to_string())
        }
    }

    fn jwt(tag: &str, exp: u64) -> String {
        use base64::Engine;
        let part = |v: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
        format!("{}.{}.{tag}", part(json!({ "alg": "RS256" })), part(json!({ "exp": exp, "https://api.openai.com/auth": { "chatgpt_account_id": "workspace-1", "chatgpt_plan_type": "pro" } })))
    }

    fn login(exp: u64) -> String {
        json!({ "tokens": { "access_token": jwt("acc-0", exp), "refresh_token": "rt-0", "id_token": "id-0", "account_id": "workspace-1" } }).to_string()
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-accounts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn accounts(dir: &Path, fail: bool, slow: Duration) -> (Arc<Accounts>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let refresher = Fake { calls: calls.clone(), fail, slow };
        (Arc::new(Accounts::open(dir, Arc::new(Files::new(dir.join(SECRETS))), Box::new(refresher))), calls)
    }

    fn holder(dir: &Path, fail: bool, slow: Duration, exp: u64) -> (Arc<Accounts>, Arc<AtomicUsize>, String) {
        let (accounts, calls) = accounts(dir, fail, slow);
        let added = accounts.add(Kind::CodexChatgpt, &login(exp), None, None, "mac-key").unwrap();
        let id = added["account"]["id"].as_str().unwrap().to_string();
        (accounts, calls, id)
    }

    fn access(reply: &Value) -> String {
        reply["accessToken"].as_str().unwrap().to_string()
    }

    #[test]
    fn a_fresh_token_is_handed_out_without_a_refresh() {
        let dir = folder("fresh");
        let (accounts, calls, id) = holder(&dir, false, Duration::ZERO, now() + 3600);
        let reply = accounts.token(&id, None).unwrap();
        assert_eq!(access(&reply), jwt("acc-0", claims(&access(&reply)).unwrap()["exp"].as_u64().unwrap()));
        assert_eq!(reply["chatgptAccountId"], "workspace-1");
        assert_eq!(reply["chatgptPlanType"], "pro");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_token_about_to_expire_is_refreshed_and_the_new_login_kept() {
        let dir = folder("expiring");
        let (accounts, calls, id) = holder(&dir, false, Duration::ZERO, now() + 10);
        let reply = accounts.token(&id, None).unwrap();
        assert!(access(&reply).ends_with(".acc-1"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(access(&accounts.token(&id, None).unwrap()).ends_with(".acc-1"), "kept: no second refresh");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let kept: Value = serde_json::from_str(&Files::new(dir.join(SECRETS)).read(&id).unwrap().unwrap()).unwrap();
        assert_eq!(kept["tokens"]["refresh_token"], "rt-0", "the holder keeps the refresh credential");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renewals_of_the_same_rejected_token_share_one_refresh() {
        let dir = folder("coalesced");
        let (accounts, calls, id) = holder(&dir, false, Duration::from_millis(300), now() + 3600);
        let rejected = access(&accounts.token(&id, None).unwrap());
        let renewals: Vec<_> = (0..6)
            .map(|_| {
                let (accounts, id, rejected) = (accounts.clone(), id.clone(), rejected.clone());
                std::thread::spawn(move || access(&accounts.token(&id, Some(&rejected)).unwrap()))
            })
            .collect();
        let answers: BTreeSet<String> = renewals.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one refresh served every renewal");
        assert_eq!(answers.len(), 1);
        assert!(answers.iter().next().unwrap().ends_with(".acc-1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refreshes_never_overlap() {
        let dir = folder("serialised");
        let (accounts, calls, id) = holder(&dir, false, Duration::from_millis(100), now() + 3600);
        let inside = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        struct Watch {
            inside: Arc<AtomicUsize>,
            most: Arc<AtomicUsize>,
            calls: Arc<AtomicUsize>,
        }
        impl Refresher for Watch {
            fn refresh(&self, login: &str, _program: Option<&str>) -> Result<String, String> {
                let now_inside = self.inside.fetch_add(1, Ordering::SeqCst) + 1;
                self.most.fetch_max(now_inside, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(50));
                let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                self.inside.fetch_sub(1, Ordering::SeqCst);
                let mut login: Value = serde_json::from_str(login).unwrap();
                login["tokens"]["access_token"] = json!(jwt(&format!("w-{n}"), now() + 3600));
                Ok(login.to_string())
            }
        }
        drop(accounts);
        let watched = Arc::new(Accounts::open(&dir, Arc::new(Files::new(dir.join(SECRETS))), Box::new(Watch { inside: inside.clone(), most: most.clone(), calls: calls.clone() })));
        let workers: Vec<_> = (0..8)
            .map(|i| {
                let (accounts, id) = (watched.clone(), id.clone());
                std::thread::spawn(move || {
                    let mut seen = access(&accounts.token(&id, None).unwrap());
                    for _ in 0..3 {
                        seen = access(&accounts.token(&id, Some(&seen)).unwrap());
                        std::thread::sleep(Duration::from_millis(i * 7));
                    }
                })
            })
            .collect();
        workers.into_iter().for_each(|w| w.join().unwrap());
        assert_eq!(most.load(Ordering::SeqCst), 1, "one refresh at a time");
        assert!(calls.load(Ordering::SeqCst) >= 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_refresh_breaks_the_account_once_and_is_not_retried_at_once() {
        let dir = folder("broken");
        let (accounts, calls, id) = holder(&dir, true, Duration::ZERO, now() + 3600);
        let reports = Arc::new(Mutex::new(vec![]));
        let seen = reports.clone();
        accounts.on_broken(Box::new(move |account| seen.lock().unwrap().push(account)));
        let rejected = access(&accounts.token(&id, None).unwrap());
        for _ in 0..3 {
            assert_eq!(accounts.token(&id, Some(&rejected)).unwrap_err().code, "broken");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "a broken account isn't refreshed again for a while");
        assert_eq!(reports.lock().unwrap().len(), 1, "one toast");
        assert_eq!(reports.lock().unwrap()[0]["agent"], "codex");
        assert_eq!(accounts.list(&|k| k.to_string())["accounts"][0]["broken"], "the refresh token was revoked");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_removed_account_hands_out_nothing_and_leaves_no_secret() {
        let dir = folder("removed");
        let (accounts, _, id) = holder(&dir, false, Duration::ZERO, now() + 3600);
        accounts.remove("codex").unwrap();
        assert_eq!(accounts.token(&id, None).unwrap_err().code, "not_found");
        assert!(!dir.join(SECRETS).join(&id).exists());
        assert_eq!(accounts.credential("codex").unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workers_get_lasting_secrets_and_only_the_holder_of_a_chatgpt_login() {
        let mac = folder("supply-mac");
        let vm = folder("supply-vm");
        let (holder, _, codex) = holder(&mac, false, Duration::ZERO, now() + 3600);
        holder.add(Kind::ClaudeSubscription, "sk-ant-oat01-abc", None, None, "mac-key").unwrap();
        let payload = holder.payload().unwrap();
        assert!(!payload.to_string().contains("rt-0"), "no refresh credential leaves the holder");
        assert!(!payload.to_string().contains("acc-0"), "no access token in the supply either");
        let (worker, _) = accounts(&vm, false, Duration::ZERO);
        worker.supply("mac-key", &payload).unwrap();
        assert_eq!(worker.credential("claude").unwrap(), Some(Credential::Env("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-abc".into())));
        assert_eq!(worker.credential("codex").unwrap(), Some(Credential::CodexHolder { account: codex.clone(), holder: "mac-key".into() }));
        assert_eq!(worker.holder_of(&codex).as_deref(), Some("mac-key"));
        assert_eq!(worker.token(&codex, None).unwrap_err().code, "not_found", "a worker holds no login");
        let peers = BTreeSet::from(["vm-key".to_string()]);
        assert_eq!(holder.outbox(&peers), ["vm-key"]);
        holder.delivered("vm-key", payload["version"].as_u64().unwrap());
        assert!(holder.outbox(&peers).is_empty());
        let _ = std::fs::remove_dir_all(&mac);
        let _ = std::fs::remove_dir_all(&vm);
    }

    #[test]
    fn removing_an_account_reaches_every_machine_it_went_to() {
        let mac = folder("remove-mac");
        let vm = folder("remove-vm");
        let (holder, _, _) = holder(&mac, false, Duration::ZERO, now() + 3600);
        holder.add(Kind::ClaudeSubscription, "sk-ant-oat01-abc", None, None, "mac-key").unwrap();
        let (worker, _) = accounts(&vm, false, Duration::ZERO);
        worker.supply("mac-key", &holder.payload().unwrap()).unwrap();
        let peers = BTreeSet::from(["vm-key".to_string()]);
        holder.delivered("vm-key", holder.version());
        holder.remove("claude").unwrap();
        assert_eq!(holder.outbox(&peers), ["vm-key"], "the removal is owed to the worker");
        worker.supply("mac-key", &holder.payload().unwrap()).unwrap();
        assert_eq!(worker.credential("claude").unwrap(), None);
        assert!(worker.credential("codex").unwrap().is_some());
        holder.remove("codex").unwrap();
        worker.supply("mac-key", &holder.payload().unwrap()).unwrap();
        assert_eq!(worker.credential("codex").unwrap(), None);
        assert!(!vm.join(SECRETS).join(RECEIVED).exists(), "nothing left on the worker");
        let _ = std::fs::remove_dir_all(&mac);
        let _ = std::fs::remove_dir_all(&vm);
    }

    #[test]
    fn an_older_supply_never_undoes_a_newer_one() {
        let vm = folder("older");
        let (worker, _) = accounts(&vm, false, Duration::ZERO);
        let account = |secret: &str| json!({ "id": "acct-1", "agent": "claude", "kind": "claude-subscription", "secret": secret, "added_at": 1 });
        worker.supply("mac-key", &json!({ "version": 5, "accounts": [account("sk-ant-oat01-new")] })).unwrap();
        worker.supply("mac-key", &json!({ "version": 4, "accounts": [account("sk-ant-oat01-old")] })).unwrap();
        assert_eq!(worker.credential("claude").unwrap(), Some(Credential::Env("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-new".into())));
        let _ = std::fs::remove_dir_all(&vm);
    }

    #[test]
    fn revoking_a_machine_takes_back_what_it_was_given() {
        let vm = folder("revoked-self");
        let (worker, _) = accounts(&vm, false, Duration::ZERO);
        let supply = json!({ "version": 1, "accounts": [{ "id": "acct-1", "agent": "claude", "kind": "claude-subscription", "secret": "sk-ant-oat01-x", "added_at": 1 }] });
        worker.supply("mac-key", &supply).unwrap();
        let trusted = BTreeSet::from(["mac-key".to_string(), "vm-key".to_string()]);
        assert!(!worker.prune(&trusted, false));
        assert!(worker.credential("claude").unwrap().is_some());
        assert!(worker.prune(&BTreeSet::from(["mac-key".to_string()]), true), "this machine learns it was revoked");
        assert_eq!(worker.credential("claude").unwrap(), None);
        assert!(!vm.join(SECRETS).join(RECEIVED).exists());
        worker.supply("mac-key", &supply).unwrap();
        assert!(worker.prune(&BTreeSet::from(["vm-key".to_string()]), false), "its source was revoked");
        assert_eq!(worker.credential("claude").unwrap(), None);
        let _ = std::fs::remove_dir_all(&vm);
    }

    #[test]
    fn a_revoked_peer_is_owed_nothing_more() {
        let mac = folder("revoked-peer");
        let (holder, _, _) = holder(&mac, false, Duration::ZERO, now() + 3600);
        holder.delivered("old-key", 0);
        holder.prune(&BTreeSet::from(["vm-key".to_string()]), false);
        assert_eq!(holder.outbox(&BTreeSet::from(["vm-key".to_string()])), ["vm-key"]);
        assert!(holder.book()["delivered"].get("old-key").is_none());
        let _ = std::fs::remove_dir_all(&mac);
    }

    #[test]
    fn a_new_account_replaces_the_agent_s_old_one() {
        let mac = folder("replace");
        let (accounts, _) = accounts(&mac, false, Duration::ZERO);
        let first = accounts.add(Kind::ClaudeSubscription, "sk-ant-oat01-one", None, None, "mac-key").unwrap();
        accounts.add(Kind::ClaudeApiKey, "sk-ant-api03-two", None, None, "mac-key").unwrap();
        assert_eq!(accounts.credential("claude").unwrap(), Some(Credential::Env("ANTHROPIC_API_KEY", "sk-ant-api03-two".into())));
        assert!(!mac.join(SECRETS).join(first["account"]["id"].as_str().unwrap()).exists());
        assert_eq!(accounts.list(&|k| k.to_string())["accounts"].as_array().unwrap().len(), 1);
        assert!(!accounts.list(&|k| k.to_string()).to_string().contains("sk-ant"), "list shows no secret");
        let _ = std::fs::remove_dir_all(&mac);
    }

    #[test]
    fn credentials_that_are_not_what_their_kind_says_are_refused() {
        let mac = folder("refused");
        let (accounts, _) = accounts(&mac, false, Duration::ZERO);
        assert!(accounts.add(Kind::ClaudeSubscription, "sk-ant-api03-key", None, None, "k").is_err());
        assert!(accounts.add(Kind::CodexChatgpt, &json!({ "tokens": { "access_token": jwt("a", 1) } }).to_string(), None, None, "k").is_err(), "a login without its refresh credential");
        assert!(accounts.add(Kind::CodexApiKey, "nope", None, None, "k").is_err());
        let _ = std::fs::remove_dir_all(&mac);
    }

    struct Gated {
        gate: Arc<Mutex<()>>,
        reads: Arc<AtomicUsize>,
    }

    impl Secrets for Gated {
        fn read(&self, name: &str) -> Result<Option<String>, String> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let _open = self.gate.lock().unwrap_or_else(|p| p.into_inner());
            Ok(Some(format!("secret of {name}")))
        }
        fn write(&self, _name: &str, _value: &str) -> Result<(), String> {
            let _open = self.gate.lock().unwrap_or_else(|p| p.into_inner());
            Ok(())
        }
        fn delete(&self, _name: &str) -> Result<(), String> {
            Ok(())
        }
    }

    struct Refusing(Arc<AtomicUsize>);

    impl Secrets for Refusing {
        fn read(&self, name: &str) -> Result<Option<String>, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(format!("can't read {name} from the Keychain: the Keychain wants a dialog answered"))
        }
        fn write(&self, name: &str, _value: &str) -> Result<(), String> {
            Err(format!("can't keep {name} in the Keychain: the Keychain wants a dialog answered"))
        }
        fn delete(&self, _name: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn reopened(dir: &Path, secrets: Arc<dyn Secrets>) -> Accounts {
        Accounts::open(dir, secrets, Box::new(Fake { calls: Arc::default(), fail: false, slow: Duration::ZERO }))
    }

    fn worker_with_received(name: &str) -> PathBuf {
        let vm = folder(name);
        let (worker, _) = accounts(&vm, false, Duration::ZERO);
        let supply = json!({ "version": 1, "accounts": [{ "id": "acct-1", "agent": "claude", "kind": "claude-subscription", "label": "Max", "secret": "sk-ant-oat01-x", "added_at": 1 }, { "id": "acct-2", "agent": "codex", "kind": "codex-chatgpt", "holder": "mac-key", "added_at": 1 }] });
        worker.supply("mac-key", &supply).unwrap();
        vm
    }

    #[test]
    fn a_secret_store_that_never_answers_times_out_with_what_it_was_asked() {
        let gate = Arc::new(Mutex::new(()));
        let reads = Arc::new(AtomicUsize::new(0));
        let bounded = Bounded::new(Arc::new(Gated { gate: gate.clone(), reads: reads.clone() }), "the Keychain", Duration::from_millis(100));
        let closed = gate.lock().unwrap();
        let started = Instant::now();
        let why = bounded.read("acct-1").unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(why.contains("read acct-1") && why.contains("the Keychain didn't answer"), "{why}");
        let started = Instant::now();
        let why = bounded.read("acct-2").unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(50), "a store still stuck isn't asked again");
        assert!(why.contains("still hasn't answered when asked to read acct-1"), "{why}");
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        drop(closed);
        let deadline = Instant::now() + Duration::from_secs(2);
        while bounded.stuck.lock().unwrap().is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(bounded.read("acct-2").unwrap().as_deref(), Some("secret of acct-2"), "it answers again once the stuck call returns");
    }

    #[test]
    fn whether_an_agent_has_an_account_comes_from_the_book_without_a_secret() {
        let mac = folder("book-only");
        let (accounts, _) = accounts(&mac, false, Duration::ZERO);
        accounts.add(Kind::ClaudeSubscription, "sk-ant-oat01-abc", Some("Max"), None, "mac-key").unwrap();
        std::fs::remove_dir_all(mac.join(SECRETS)).unwrap();
        assert!(has_credential(&mac, "claude"));
        assert!(!has_credential(&mac, "codex"));
        let vm = worker_with_received("book-only-vm");
        std::fs::remove_dir_all(vm.join(SECRETS)).unwrap();
        assert!(has_credential(&vm, "claude") && has_credential(&vm, "codex"), "the book answers with no secret there to read");
        let _ = std::fs::remove_dir_all(&mac);
        let _ = std::fs::remove_dir_all(&vm);
    }

    #[test]
    fn an_unreadable_secret_is_an_error_that_names_the_account() {
        let mac = folder("unreadable");
        let (accounts, _) = accounts(&mac, false, Duration::ZERO);
        accounts.add(Kind::ClaudeSubscription, "sk-ant-oat01-abc", Some("Max"), None, "mac-key").unwrap();
        let refusing = reopened(&mac, Arc::new(Refusing(Arc::default())));
        let why = refusing.credential("claude").unwrap_err();
        assert!(why.contains("your Claude account \"Max\"") && why.contains("wants a dialog answered"), "{why}");
        assert!(refusing.payload().is_err(), "an unreadable secret isn't supplied as a missing account");
        assert_eq!(refusing.credential("codex").unwrap(), None);
        let _ = std::fs::remove_dir_all(&mac);
    }

    #[test]
    fn a_worker_whose_store_is_stuck_still_lists_and_names_holders() {
        let vm = worker_with_received("stuck-worker");
        let gate = Arc::new(Mutex::new(()));
        let reads = Arc::new(AtomicUsize::new(0));
        let stuck = reopened(&vm, Arc::new(Bounded::new(Arc::new(Gated { gate: gate.clone(), reads: reads.clone() }), "the Keychain", Duration::from_millis(100))));
        let closed = gate.lock().unwrap();
        let started = Instant::now();
        let listed = stuck.list(&|k| k.to_string());
        assert_eq!(listed["received"].as_array().unwrap().len(), 2);
        assert!(listed.get("received_error").is_none());
        assert_eq!(stuck.holder_of("acct-2").as_deref(), Some("mac-key"));
        assert_eq!(stuck.credential("codex").unwrap(), Some(Credential::CodexHolder { account: "acct-2".into(), holder: "mac-key".into() }));
        assert_eq!(reads.load(Ordering::SeqCst), 0, "none of that needs a secret");
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(stuck.credential("claude").unwrap_err().contains("didn't answer"));
        assert!(stuck.supply("mac-key", &json!({ "version": 2, "accounts": [] })).is_err(), "a supply can't drop what it couldn't read");
        drop(closed);
        let _ = std::fs::remove_dir_all(&vm);
    }

    #[test]
    fn a_revoked_source_the_book_missed_is_still_dropped() {
        let vm = worker_with_received("drift");
        let files = Files::new(vm.join(SECRETS));
        let mut received: Value = serde_json::from_str(&files.read(RECEIVED).unwrap().unwrap()).unwrap();
        received["old-key"] = received["mac-key"].clone();
        files.write(RECEIVED, &received.to_string()).unwrap();
        let (worker, _) = accounts(&vm, false, Duration::ZERO);
        assert!(worker.prune(&BTreeSet::from(["mac-key".to_string()]), false), "old-key was in the secret but not the book");
        assert!(worker.received().unwrap().get("old-key").is_none());
        assert!(worker.received().unwrap().get("mac-key").is_some());
        let _ = std::fs::remove_dir_all(&vm);
    }

    #[test]
    fn an_agent_started_without_its_account_is_told_to_the_user_once() {
        let mac = folder("told");
        let service = Service::with(reopened(&mac, Arc::new(Refusing(Arc::default()))), None);
        let told = Arc::new(Mutex::new(vec![]));
        let seen = told.clone();
        service.on_unreadable(Box::new(move |account| seen.lock().unwrap().push(account)));
        let why = service.accounts.credential("claude").err().unwrap_or_else(|| "can't read your Claude account".into());
        for _ in 0..3 {
            service.unreadable(&json!({ "agent": "claude", "why": why })).unwrap();
        }
        assert_eq!(told.lock().unwrap().len(), 1);
        assert_eq!(told.lock().unwrap()[0]["agent"], "claude");
        service.unreadable(&json!({ "agent": "codex", "why": why })).unwrap();
        assert_eq!(told.lock().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&mac);
    }

    #[test]
    fn secrets_are_files_only_their_owner_can_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = folder("files");
        let files = Files::new(dir.join(SECRETS));
        files.write("one", "secret").unwrap();
        assert_eq!(std::fs::metadata(dir.join(SECRETS)).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(dir.join(SECRETS).join("one")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(files.read("one").unwrap().as_deref(), Some("secret"));
        files.delete("one").unwrap();
        assert_eq!(files.read("one").unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
