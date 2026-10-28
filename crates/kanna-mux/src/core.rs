use crate::error::KmuxError;
use crate::exec::{sleep, Executor, Signal, Turns, Wakeup};
use crate::instance::{Instance, DEFAULT_NAME};
use crate::json::{is_true, json, num, string, Map, Value};
use crate::layout::{place_request, scope_request, Edge, LayoutHost, LayoutLink, LocalLayout, Placement, Scope, SplitSide};
use crate::workspace::MAC_APP;
use crate::model::{fraction, Model};
use crate::pane::{read_diff_args, read_view, Comment, Pane, PaneState, PaneType, Panes, StopMark, CONTENT_PREFIXES};
use crate::state;

#[path = "tour.rs"]
mod tour;
use std::cell::{RefCell, RefMut};
use std::collections::{HashMap, HashSet};
use std::future::{poll_fn, Future};
use std::ops::Deref;
use std::pin::pin;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

pub trait PaneHost {
    fn start(&self, pane: &Pane);
    fn stop(&self, pane: &Pane);
    fn send(&self, pane: &Pane, text: &str, done: Responder);
    fn send_keys(&self, pane: &Pane, keys: Vec<String>, done: Responder) {
        let _ = keys;
        done.reply(Err(KmuxError::new("unsupported", format!("{} can't take keys here", pane.label()))));
    }
    fn end(&self, pane: &Pane) {
        let _ = pane;
    }
    fn read(&self, pane: &Pane, lines: Option<usize>) -> Result<String, KmuxError> {
        let _ = lines;
        Err(KmuxError::new("unsupported", format!("{} can't be read here", pane.label())))
    }
}

pub trait TaskBook {
    fn workspace_of(&self, reference: &str) -> Option<Result<String, KmuxError>>;
    fn task_of(&self, workspace: &str) -> Option<(String, Option<String>)>;
    fn sync(&self, workspaces: &[Value]) {
        let _ = workspaces;
    }
}

pub type CommandHandler = Rc<dyn Fn(Value, Responder)>;

pub type PrefixHandler = Rc<dyn Fn(&str, Value, Responder)>;

pub const TASKS_PER_PROJECT: usize = 3;

pub fn project(cwd: Option<&str>) -> Option<std::path::PathBuf> {
    let given = std::path::Path::new(cwd?);
    let resolved = std::fs::canonicalize(given).ok();
    let start = resolved.as_deref().unwrap_or(given);
    let Some(root) = start.ancestors().find(|dir| dir.join(".git").exists()) else { return Some(start.to_path_buf()) };
    let marker = root.join(".git");
    let main = std::fs::metadata(&marker)
        .ok()
        .filter(|m| m.is_file() && m.len() <= 4096)
        .and_then(|_| std::fs::read_to_string(&marker).ok())
        .and_then(|text| text.lines().next()?.strip_prefix("gitdir: ").map(|dir| root.join(dir.trim())))
        .and_then(|dir| {
            let text = dir.to_string_lossy().to_string();
            text.find("/.git/worktrees/").map(|at| std::path::PathBuf::from(&text[..at]))
        });
    Some(main.unwrap_or_else(|| root.to_path_buf()))
}

pub const CORE_COMMANDS: [&str; 13] = ["capabilities", "open", "list", "close", "restart", "send", "send-key", "navigate", "read", "markup", "comment", "tour", "rename-pane"];

pub const DEBUG_COMMANDS: [&str; 2] = ["debug.saveState", "debug.restoreState"];

const FEATURES: [&str; 7] = ["windows", "tabs", "fractionalSizing", "namedPanes", "zoom", "lifecycle", "dock"];
const CORE_FEATURES: [&str; 2] = ["namedPanes", "lifecycle"];

struct State {
    panes: Panes,
    instance: Instance,
    pane_types: Vec<PaneType>,
    start_timeout: Duration,
    ios_start_timeout: Duration,
    saved_state: Option<Value>,
}

struct Inner {
    state: RefCell<State>,
    layout: Rc<dyn LayoutLink>,
    turns: Rc<Turns>,
    exec: Rc<Executor>,
    host: RefCell<Option<Rc<dyn PaneHost>>>,
    on_change: RefCell<Option<Rc<dyn Fn()>>>,
    extra: RefCell<HashMap<String, CommandHandler>>,
    prefixes: RefCell<Vec<(String, PrefixHandler)>>,
    tasks: RefCell<Option<Rc<dyn TaskBook>>>,
    waiters: RefCell<HashMap<String, Vec<Rc<Signal>>>>,
    renamed: RefCell<Vec<Rc<dyn Fn(&str, &str)>>>,
}

pub struct Core(CoreHandle);

#[derive(Clone)]
pub struct CoreHandle(Rc<Inner>);

#[derive(Clone)]
pub struct WeakCore(Weak<Inner>);

impl WeakCore {
    pub fn upgrade(&self) -> Option<CoreHandle> {
        self.0.upgrade().map(CoreHandle)
    }
}

impl Default for Core {
    fn default() -> Core {
        Core::new(Box::new(Model::new()))
    }
}

impl Core {
    pub fn new(layout: Box<dyn LayoutHost>) -> Core {
        Core::with_link(Rc::new(LocalLayout::new(layout)))
    }

    pub fn with_link(layout: Rc<dyn LayoutLink>) -> Core {
        Core(CoreHandle(Rc::new(Inner {
            layout,
            turns: Rc::default(),
            state: RefCell::new(State {
                panes: Panes::default(),
                instance: Instance::new(DEFAULT_NAME),
                pane_types: PaneType::ALL.to_vec(),
                start_timeout: Duration::from_secs(10),
                ios_start_timeout: Duration::from_secs(180),
                saved_state: None,
            }),
            exec: Rc::default(),
            host: RefCell::new(None),
            on_change: RefCell::new(None),
            extra: RefCell::new(HashMap::new()),
            prefixes: RefCell::new(vec![]),
            tasks: RefCell::new(None),
            waiters: RefCell::new(HashMap::new()),
            renamed: RefCell::new(vec![]),
        })))
    }
}

impl Deref for Core {
    type Target = CoreHandle;
    fn deref(&self) -> &CoreHandle {
        &self.0
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        let inner = &self.0 .0;
        inner.exec.clear();
        inner.waiters.borrow_mut().clear();
        inner.extra.borrow_mut().clear();
        inner.prefixes.borrow_mut().clear();
        inner.host.borrow_mut().take();
        inner.on_change.borrow_mut().take();
    }
}

fn bad_request(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

fn reply<const N: usize>(fields: [(&str, Value); N]) -> Map {
    fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

impl CoreHandle {
    fn state(&self) -> RefMut<'_, State> {
        self.0.state.borrow_mut()
    }

    pub fn weak(&self) -> WeakCore {
        WeakCore(Rc::downgrade(&self.0))
    }

    pub fn set_host(&self, host: Option<Rc<dyn PaneHost>>) {
        *self.0.host.borrow_mut() = host;
    }

    pub fn set_on_change(&self, on_change: Option<Rc<dyn Fn()>>) {
        *self.0.on_change.borrow_mut() = on_change;
    }

    pub fn use_content_ids(&self) {
        self.state().panes.use_content_ids();
    }

    pub fn on_renamed(&self, renamed: Rc<dyn Fn(&str, &str)>) {
        self.0.renamed.borrow_mut().push(renamed);
    }

    pub fn set_wakeup(&self, wakeup: Option<Wakeup>) {
        self.0.exec.set_wakeup(wakeup);
    }

    pub fn set_instance(&self, instance: Instance) {
        self.state().instance = instance;
    }

    pub fn instance(&self) -> Instance {
        self.state().instance.clone()
    }

    pub fn set_pane_types(&self, types: Vec<PaneType>) {
        self.state().pane_types = types;
    }

    pub fn set_start_timeout(&self, timeout: Duration) {
        self.state().start_timeout = timeout;
    }

    pub fn set_ios_start_timeout(&self, timeout: Duration) {
        self.state().ios_start_timeout = timeout;
    }

    pub fn add_command(&self, name: &str, handler: CommandHandler) {
        self.0.extra.borrow_mut().insert(name.into(), handler);
    }

    pub fn set_tasks(&self, book: Rc<dyn TaskBook>) {
        *self.0.tasks.borrow_mut() = Some(book);
    }

    fn book(&self) -> Option<Rc<dyn TaskBook>> {
        self.0.tasks.borrow().clone()
    }

    fn name_tasks(&self, reply: &mut Map) {
        let Some(book) = self.book() else { return };
        let name = |ws: &mut Value| {
            if let Some((task, name)) = ws.get("id").and_then(Value::as_str).and_then(|id| book.task_of(id)) {
                ws["task"] = task.into();
                if let Some(name) = name {
                    ws["name"] = name.into();
                }
            }
        };
        for key in ["workspace", "was"] {
            if let Some(ws) = reply.get_mut(key).filter(|w| w.is_object()) {
                name(ws);
            }
        }
        for ws in reply.get_mut("workspaces").and_then(Value::as_array_mut).into_iter().flatten() {
            name(ws);
        }
    }

    fn task_workspace(&self, args: &mut Value) -> Result<(), KmuxError> {
        let Some(reference) = string(args, "workspace").map(String::from) else { return Ok(()) };
        match self.book().and_then(|book| book.workspace_of(&reference)) {
            Some(Ok(workspace)) => args["workspace"] = workspace.into(),
            Some(Err(error)) => return Err(error),
            None => {}
        }
        Ok(())
    }

    pub fn add_prefix(&self, prefix: &str, handler: PrefixHandler) {
        self.0.prefixes.borrow_mut().push((prefix.into(), handler));
    }

    pub fn commands(&self) -> Vec<String> {
        let mut commands: Vec<String> = CORE_COMMANDS.iter().chain(self.0.layout.commands()).map(|c| c.to_string()).chain(self.0.extra.borrow().keys().cloned()).collect();
        commands.sort();
        commands.dedup();
        commands
    }

    pub fn pane(&self, reference: &str) -> Option<Pane> {
        self.state().panes.pane(reference).cloned()
    }

    pub fn need_pane(&self, reference: Option<&str>) -> Result<Pane, KmuxError> {
        self.state().panes.need(reference).cloned()
    }

    pub fn with_panes<R>(&self, f: impl FnOnce(&Panes) -> R) -> R {
        f(&self.state().panes)
    }

    pub fn with_pane_mut<R>(&self, id: &str, f: impl FnOnce(&mut Pane) -> R) -> Option<R> {
        self.state().panes.get_mut(id).map(f)
    }

    pub fn set_command(&self, id: &str, command: String) {
        if self.with_pane_mut(id, |pane| pane.command = Some(command)).is_some() {
            self.changed();
        }
    }

    async fn layout(&self, request: Value) -> Result<Map, KmuxError> {
        let call = {
            let s = self.state();
            self.0.layout.call(request, &s.panes)
        };
        call.await
    }

    fn host(&self) -> Option<Rc<dyn PaneHost>> {
        self.0.host.borrow().clone()
    }

    fn changed(&self) {
        let on_change = self.0.on_change.borrow().clone();
        if let Some(on_change) = on_change {
            on_change();
        }
    }

    pub fn handle(&self, request: Value, reply: impl FnOnce(Value) + 'static) {
        let core = self.clone();
        self.0.exec.spawn(Box::pin(async move { reply(core.respond(request).await) }));
        self.0.exec.run();
    }

    pub fn handle_blocking(&self, request: Value) -> Value {
        let answer = Rc::new(RefCell::new(None));
        let slot = answer.clone();
        self.handle(request, move |reply| *slot.borrow_mut() = Some(reply));
        loop {
            if let Some(reply) = answer.borrow_mut().take() {
                return reply;
            }
            if !self.0.exec.has_ready() {
                self.0.exec.wait(self.0.exec.next_deadline());
            }
            self.0.exec.run();
        }
    }

    pub fn end(&self, id: &str, done: impl FnOnce(Result<(), KmuxError>) + 'static) {
        let core = self.clone();
        let id = id.to_string();
        self.0.exec.spawn(Box::pin(async move { done(core.ended(&id).await) }));
        self.0.exec.run();
    }

    pub fn hang_up(&self, task: &str, done: impl FnOnce(Result<Vec<String>, KmuxError>) + 'static) {
        let core = self.clone();
        let task = task.to_string();
        self.0.exec.spawn(Box::pin(async move { done(core.hung_up(&task).await) }));
        self.0.exec.run();
    }

    pub fn wake(&self, task: &str, done: impl FnOnce(Result<Vec<String>, KmuxError>) + 'static) {
        let core = self.clone();
        let task = task.to_string();
        self.0.exec.spawn(Box::pin(async move { done(core.woken(&task).await) }));
        self.0.exec.run();
    }

    async fn terminals_of(&self, task: &str, keep: impl Fn(&Pane) -> bool) -> Result<Vec<String>, KmuxError> {
        let found = self.layout(scope_request(&Scope::Task(task.into()))).await?;
        let s = self.state();
        let ids = found.get("panes").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(ids.iter().filter_map(Value::as_str).filter(|id| *id != task).filter(|id| s.panes.get(id).is_some_and(|p| p.kind == PaneType::Term && keep(p))).map(String::from).collect())
    }

    async fn hung_up(&self, task: &str) -> Result<Vec<String>, KmuxError> {
        let live = self.terminals_of(task, |p| matches!(p.state, PaneState::Starting | PaneState::Running)).await?;
        for id in &live {
            self.with_pane_mut(id, |pane| pane.hung_up = true);
        }
        self.changed();
        for id in &live {
            self.ended(id).await?;
        }
        Ok(live)
    }

    async fn woken(&self, task: &str) -> Result<Vec<String>, KmuxError> {
        let held = self.terminals_of(task, |p| p.hung_up && !matches!(p.state, PaneState::Starting | PaneState::Running)).await?;
        let mut woken = vec![];
        for id in &held {
            let snapshot = self.reload(id, true).await;
            if let Err(error) = self.check_started(&snapshot.id, &snapshot) {
                eprintln!("kanna-mux: {} didn't start again: {}", snapshot.label(), error.message);
            }
            woken.push(snapshot.id);
        }
        Ok(woken)
    }

    async fn ended(&self, id: &str) -> Result<(), KmuxError> {
        let live = |core: &CoreHandle| core.state().panes.get(id).is_some_and(|p| matches!(p.state, PaneState::Starting | PaneState::Running));
        let pane = self.need_pane(Some(id))?;
        if !live(self) {
            return Ok(());
        }
        if let Some(host) = self.host() {
            host.end(&pane);
        }
        let deadline = Instant::now() + self.state().start_timeout;
        while live(self) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(KmuxError::new("failed", format!("{} didn't end", pane.label())));
            }
            let signal = Rc::new(Signal::default());
            self.0.waiters.borrow_mut().entry(id.into()).or_default().push(signal.clone());
            let mut wait = pin!(signal.wait());
            let mut timer = pin!(sleep(self.0.exec.clone(), left));
            poll_fn(|cx| if wait.as_mut().poll(cx).is_ready() || timer.as_mut().poll(cx).is_ready() { Poll::Ready(()) } else { Poll::Pending }).await;
        }
        Ok(())
    }

    pub fn later(&self, f: impl FnOnce() + 'static) {
        self.0.exec.spawn(Box::pin(async move { f() }));
    }

    pub fn poll(&self) -> Option<Instant> {
        self.0.exec.run();
        self.0.exec.next_deadline()
    }

    pub fn update(&self, id: &str, state: PaneState, exit_code: Option<i64>, error: Option<String>) {
        {
            let mut s = self.state();
            let Some(pane) = s.panes.get_mut(id) else { return };
            pane.state = state;
            pane.exit_code = exit_code;
            pane.error = error;
        }
        if state != PaneState::Starting {
            self.resume(id);
        }
        self.changed();
        self.0.exec.run();
    }

    pub fn note_numbers(&self, numbers: &HashMap<String, usize>) {
        self.state().panes.note_numbers(numbers);
    }

    pub fn install_panes(&self, panes: Vec<Pane>, counter: usize, terminals: usize) {
        {
            let mut s = self.state();
            let mut all: Vec<Pane> = s.panes.iter().cloned().collect();
            for known in all.iter_mut().filter(|p| p.tour.is_none()) {
                known.tour = panes.iter().find(|p| p.id == known.id).and_then(|p| p.tour.clone());
            }
            all.extend(panes.into_iter().filter(|p| s.panes.get(&p.id).is_none()));
            s.panes.install(all, counter, terminals);
        }
        self.changed();
    }

    pub fn drop_panes(&self, ids: &[String], error: &str) {
        for id in ids {
            let starting = self.state().panes.get(id).is_some_and(|p| p.state == PaneState::Starting);
            if starting {
                self.update(id, PaneState::Failed, None, Some(error.into()));
            }
            self.state().panes.forget(id);
            self.resume(id);
        }
        self.changed();
        self.0.exec.run();
    }

    pub fn set_pid(&self, id: &str, pid: Option<i64>) {
        if let Some(pane) = self.state().panes.get_mut(id) {
            pane.pid = pid;
        }
    }

    pub fn page_moved(&self, id: &str, url: &str) {
        if let Some(pane) = self.state().panes.get_mut(id) {
            moved(pane, url.into());
        }
    }

    fn resume(&self, id: &str) {
        let waiters = self.0.waiters.borrow_mut().remove(id);
        for signal in waiters.into_iter().flatten() {
            signal.set();
        }
    }

    async fn settled(&self, id: &str) {
        let timeout = {
            let state = self.state();
            match state.panes.get(id) {
                Some(pane) if pane.state == PaneState::Starting => {
                    if pane.kind == PaneType::Ios { state.ios_start_timeout } else { state.start_timeout }
                }
                _ => return,
            }
        };
        let signal = Rc::new(Signal::default());
        self.0.waiters.borrow_mut().entry(id.into()).or_default().push(signal.clone());
        let mut wait = pin!(signal.wait());
        let mut timer = pin!(sleep(self.0.exec.clone(), timeout));
        let timed_out = poll_fn(|cx| {
            if wait.as_mut().poll(cx).is_ready() {
                Poll::Ready(false)
            } else if timer.as_mut().poll(cx).is_ready() {
                Poll::Ready(true)
            } else {
                Poll::Pending
            }
        })
        .await;
        if timed_out {
            let starting = self.state().panes.get(id).is_some_and(|p| p.state == PaneState::Starting);
            if starting {
                self.update(id, PaneState::Failed, None, Some(format!("did not start within {:?} seconds", timeout.as_secs_f64())));
            }
            wait.await;
        }
    }

    async fn respond(&self, request: Value) -> Value {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let result = match string(&request, "cmd") {
            None => Err(bad_request("missing cmd")),
            Some(cmd) => {
                let cmd = cmd.to_string();
                let mut args = request.get("args").cloned().unwrap_or_else(|| json!({}));
                match if cmd == "window" { self.task_workspace(&mut args) } else { Ok(()) } {
                    Ok(()) => self.run(&cmd, args).await,
                    Err(error) => Err(error),
                }
            }
        };
        match result {
            Ok(mut reply) => {
                self.name_tasks(&mut reply);
                reply.insert("id".into(), id);
                reply.insert("ok".into(), Value::Bool(true));
                Value::Object(reply)
            }
            Err(error) => json!({ "id": id, "ok": false, "error": { "code": error.code, "message": error.message } }),
        }
    }

    async fn run(&self, cmd: &str, args: Value) -> Result<Map, KmuxError> {
        match cmd {
            "capabilities" => Ok(self.capabilities()),
            "open" => self.open(&args).await,
            "list" => self.list().await,
            "close" => self.close(&args).await,
            "restart" => self.restart(&args).await,
            "send" => self.send(&args).await,
            "send-key" => self.send_key(&args).await,
            "read" => self.read(&args),
            "navigate" => self.navigate(&args).await,
            "markup" => self.markup(&args).await,
            "comment" => self.comment(&args).await,
            "tour" => self.tour(&args).await,
            "rename-pane" => self.rename_pane(&args).await,
            "debug.saveState" => {
                let state = self.export_state().await?;
                self.state().saved_state = Some(state.clone());
                Ok(reply([("state", state)]))
            }
            "debug.restoreState" => {
                let saved = self.state().saved_state.clone();
                let state = args.get("state").cloned().or(saved).ok_or_else(|| bad_request("no saved state to restore"))?;
                self.restore_state(&state).await?;
                let windows = match self.layout(json!({ "op": "windows" })).await?.remove("windows") {
                    Some(Value::Array(windows)) => windows.iter().filter_map(|w| w.get("id").cloned()).collect(),
                    _ => vec![],
                };
                let panes = self.state().panes.ids().into_iter().map(Value::String).collect();
                Ok(reply([("windows", Value::Array(windows)), ("panes", Value::Array(panes))]))
            }
            _ if self.0.layout.commands().contains(&cmd) => {
                let _turn = self.0.turns.take().await;
                let mut result = self.layout(json!({ "op": "command", "cmd": cmd, "args": args })).await;
                if result.is_ok() {
                    self.changed();
                }
                if let (Ok(listed), "task") = (&mut result, cmd) {
                    self.mark_running(listed);
                }
                result
            }
            _ => {
                let (responder, answer) = responder();
                let handler = self.0.extra.borrow().get(cmd).cloned();
                if let Some(handler) = handler {
                    handler(args, responder);
                    return answer.await;
                }
                let prefixed = self.0.prefixes.borrow().iter().find(|(prefix, _)| cmd.starts_with(prefix.as_str())).map(|(_, h)| h.clone());
                let Some(handler) = prefixed else { return Err(bad_request(format!("unknown command \"{cmd}\""))) };
                handler(cmd, args, responder);
                answer.await
            }
        }
    }

    fn capabilities(&self) -> Map {
        let commands = self.commands();
        let layout = self.0.layout.features();
        let s = self.state();
        let mut features: Vec<&str> = FEATURES.iter().copied().filter(|f| CORE_FEATURES.contains(f) || layout.contains(f)).collect();
        features.extend(layout.iter().filter(|f| !FEATURES.contains(f)));
        reply([
            ("mux", "kmux".into()),
            ("instance", s.instance.name.clone().into()),
            ("paneTypes", s.pane_types.iter().map(|t| t.as_str()).collect::<Vec<_>>().into()),
            ("commands", commands.into()),
            ("features", features.into()),
        ])
    }

    async fn list(&self) -> Result<Map, KmuxError> {
        let mut layout = self.layout(json!({ "op": "windows" })).await?;
        let windows = layout.remove("windows").unwrap_or(Value::Array(vec![]));
        let workspaces = layout.remove("workspaces");
        let hidden = layout.remove("hiddenWindows");
        if let (Some(book), Some(Value::Array(all))) = (self.book(), &workspaces) {
            book.sync(all);
        }
        let s = self.state();
        let of = |id: &str| workspaces.as_ref().and_then(Value::as_array).and_then(|all| all.iter().find(|ws| ws["panes"].as_array().is_some_and(|p| p.iter().any(|p| p == id)))).map(|ws| ws["id"].clone());
        let panes = s.panes.iter().map(|pane| {
            let mut row = pane.summary();
            if let Some(ws) = of(&pane.id) {
                row["workspace"] = ws;
            }
            row
        });
        let mut out = reply([("windows", windows), ("panes", Value::Array(panes.collect()))]);
        if let Some(workspaces) = workspaces {
            out.insert("workspaces".into(), workspaces);
        }
        if let Some(hidden) = hidden {
            out.insert("hiddenWindows".into(), hidden);
        }
        Ok(out)
    }

    fn start_pane(&self, id: &str) {
        self.state().panes.new_terminal(id);
        let pane = self.state().panes.get(id).cloned();
        if let (Some(host), Some(pane)) = (self.host(), pane) {
            host.start(&pane);
        }
    }

    fn summary(&self, id: &str, fallback: &Pane) -> Value {
        self.state().panes.get(id).unwrap_or(fallback).summary()
    }

    async fn open(&self, args: &Value) -> Result<Map, KmuxError> {
        let type_name = string(args, "type").unwrap_or("");
        let types = self.state().pane_types.clone();
        let Some(kind) = PaneType::parse(type_name).filter(|t| types.contains(t)) else {
            let names: Vec<&str> = types.iter().map(|t| t.as_str()).collect();
            let listed = match names.split_last() {
                Some((last, [])) => last.to_string(),
                Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
                None => String::new(),
            };
            return Err(bad_request(format!("unknown pane type \"{type_name}\" ({listed})")));
        };
        let empty = |key: &str| string(args, key).map_or(true, str::is_empty);
        if kind == PaneType::Ios && empty("app") {
            return Err(bad_request("ios panes need an app"));
        }
        if (args.get("app").is_some() || args.get("device").is_some()) && kind != PaneType::Ios {
            return Err(bad_request("app and device are only for ios panes"));
        }
        if kind == PaneType::Web && empty("url") {
            return Err(bad_request("web panes need a url"));
        }
        if args.get("history").is_some() && kind != PaneType::Web {
            return Err(bad_request("history is only for web panes"));
        }
        if kind == PaneType::Md && empty("path") {
            return Err(bad_request("markdown panes need a path"));
        }
        if kind == PaneType::Code && empty("path") {
            return Err(bad_request("code panes need a path"));
        }
        if kind == PaneType::Diff && empty("path") {
            return Err(bad_request("diff panes need a path: the folder git runs in"));
        }
        if args.get("path").is_some() && !matches!(kind, PaneType::Md | PaneType::Code | PaneType::Diff) {
            return Err(bad_request("path is only for markdown, code and diff panes"));
        }
        if (args.get("args").is_some() || args.get("view").is_some()) && kind != PaneType::Diff {
            return Err(bad_request("args and view are only for diff panes"));
        }
        let diff = read_diff_args(args.get("args"))?;
        let view = read_view(args.get("view"))?;
        if args.get("line").is_some() && kind != PaneType::Code {
            return Err(bad_request("line is only for code panes"));
        }
        let line = line_arg(args)?;
        if args.get("agent").is_some() && !matches!(kind, PaneType::Md | PaneType::Web) {
            return Err(bad_request("agent is only for markdown and web panes"));
        }
        if args.get("markup").is_some() && !matches!(kind, PaneType::Md | PaneType::Web) {
            return Err(bad_request("markup is only for markdown and web panes"));
        }
        if args.get("diff").is_some() && (kind != PaneType::Md || string(args, "diff").is_none_or(str::is_empty)) {
            return Err(bad_request("diff is only for markdown panes, and names a commit or branch"));
        }
        if is_true(args, "markup") && string(args, "diff").is_some_and(is_range) {
            return Err(bad_request("a range (A..B) shows a past version of the doc, so it takes no comments"));
        }
        let agent = match string(args, "agent") {
            Some(agent) => Some(self.need_pane(Some(agent))?.id),
            None => None,
        };
        let placed = ["window", "workspace", "dock", "lead", "planner"].iter().any(|key| args.get(*key).is_some_and(|v| !v.is_null() && v != &Value::Bool(false)));
        if let Some(from) = string(args, "from").filter(|from| !placed && self.state().panes.pane(from).is_none()) {
            return Err(KmuxError::new("no_caller", format!("{from} isn't one of this Kanna's panes, so there's no window of yours here: say where with --window WINDOW (or --window new)")));
        }
        let name = string(args, "name").map(String::from);
        let taken = |core: &CoreHandle| match &name {
            Some(name) if core.state().panes.pane(name).is_some() => Err(KmuxError::new("name_taken", format!("a pane named \"{name}\" already exists"))),
            _ => Ok(()),
        };
        taken(self)?;
        let split = match args.get("split").map(|split| split.as_str().unwrap_or("")) {
            None => None,
            Some("right") => Some(SplitSide::Right),
            Some("down") => Some(SplitSide::Down),
            Some("auto") => Some(SplitSide::Auto),
            Some(_) => return Err(bad_request("split must be right, down or auto")),
        };
        let size = match args.get("size") {
            Some(value) => Some(fraction(value)?),
            None => None,
        };
        if !size.is_none_or(|size| size > 0.0 && size < 1.0) {
            return Err(bad_request("size must be a fraction between 0 and 1"));
        }
        let new_tab = args.get("tab").and_then(Value::as_bool).unwrap_or(false);
        let dock = Edge::read(args.get("dock"))?;
        if dock.is_some() && (args.get("split").is_some() || args.get("size").is_some() || new_tab) {
            return Err(bad_request("a docked pane takes no split, size or tab"));
        }
        if args.get("frozen").is_some_and(|f| !f.is_null() && !f.is_boolean()) {
            return Err(bad_request("frozen must be true or false"));
        }
        let frozen = is_true(args, "frozen");
        if frozen && dock.is_none() {
            return Err(bad_request("only a docked pane is frozen"));
        }
        let backlog = is_true(args, "backlog");
        let claimed = string(args, "claim").map(String::from);
        let planner = is_true(args, "planner");
        if dock.is_some() && args.get("window").is_some() {
            return Err(bad_request("a docked pane is a task's agent, in its own workspace, not in a window"));
        }
        if backlog && dock.is_none() || claimed.is_some() && dock.is_none() && !planner {
            return Err(bad_request("claim is for a docked pane or a planner, and backlog for a docked pane"));
        }
        if backlog && claimed.is_some() {
            return Err(bad_request("a task is in the backlog or on an app's Shelf, not both"));
        }
        let lead = is_true(args, "lead");
        if lead && (kind != PaneType::Term || dock.is_some() || split.is_some() || size.is_some() || new_tab || string(args, "window").is_none()) {
            return Err(bad_request("a window's lead is a terminal in a window, with no dock, split, size or tab"));
        }
        if planner && (kind != PaneType::Term || dock.is_some() || split.is_some() || size.is_some() || new_tab || args.get("window").is_some()) {
            return Err(bad_request("a planner is a terminal of its own, with no dock, split, size, tab or window"));
        }
        let app = match (dock, backlog) {
            (Some(_), false) => Some(claimed.unwrap_or_else(|| MAC_APP.into())),
            _ if planner => Some(claimed.unwrap_or_else(|| MAC_APP.into())),
            _ => None,
        };

        if kind == PaneType::Code && name.is_none() && !new_tab && dock.is_none() && split.is_none() && size.is_none() {
            if let Some(opened) = self.open_again(string(args, "path").unwrap_or_default(), line, string(args, "from"), string(args, "window")).await? {
                return Ok(opened);
            }
        }
        let turn = self.0.turns.take().await;
        taken(self)?;
        if let (Some(_), Some(app)) = (dock, app.as_deref()) {
            self.room_for_task(string(args, "cwd"), app).await?;
        }
        let id = self.state().panes.next_id(kind);
        let placeless = !new_tab && !lead && !planner && dock.is_none() && split.is_none() && size.is_none() && args.get("window").is_none() && args.get("workspace").is_none();
        let opener = string(args, "from").filter(|_| !lead && !planner && dock.is_none()).and_then(|from| self.state().panes.pane(from).map(|p| p.id.clone()));
        let from = opener.clone().filter(|_| placeless);
        let cwd = match string(args, "cwd") {
            Some(cwd) => Some(cwd.to_string()),
            None if kind == PaneType::Term && dock.is_none() && !lead && !planner => self.task_folder(opener.as_deref(), string(args, "window"), string(args, "workspace")).await,
            None => None,
        };
        let at = Placement {
            window: string(args, "window").map(String::from),
            workspace: string(args, "workspace").map(String::from),
            split: split.unwrap_or(SplitSide::Auto),
            size: size.unwrap_or(0.5),
            new_tab,
            dock,
            frozen,
            from,
            opener,
            app,
            backlog,
            lead,
            planner,
        };
        let placed = self.layout(place_request(&Pane::new(&id, name.clone(), kind), &at)).await?;
        let placed = Value::Object(placed);
        {
            let mut s = self.state();
            let pane = s.panes.make(kind, name);
            let field = |key: &str| string(args, key).map(String::from);
            pane.command = field("cmd");
            pane.cwd = cwd.clone();
            pane.opened_in = cwd;
            pane.url = string(args, "url").map(normalize_url);
            pane.app = field("app");
            pane.device = field("device");
            pane.path = field("path");
            pane.line = line;
            pane.agent = agent.clone();
            pane.markup = is_true(args, "markup");
            pane.diff_base = field("diff");
            if kind == PaneType::Code {
                pane.comment = args.get("comment").and_then(Comment::from_json);
            }
            if matches!(kind, PaneType::Code | PaneType::Md) {
                pane.stop = args.get("tour").and_then(StopMark::from_json);
            }
            if kind == PaneType::Diff {
                pane.diff = Some(diff.unwrap_or_default());
                pane.view = view;
            }
            if (kind == PaneType::Web && is_true(args, "history")) || matches!(kind, PaneType::Md | PaneType::Code) {
                pane.history = Some(Default::default());
            }
        }
        drop(turn);
        self.changed();
        self.start_pane(&id);

        let snapshot = self.state().panes.get(&id).cloned().unwrap();
        if args.get("wait") == Some(&Value::Bool(false)) {
            return Ok(opened(self.summary(&id, &snapshot), &placed, &placed));
        }
        self.settled(&id).await;
        self.check_started(&id, &snapshot)?;
        let summary = self.summary(&id, &snapshot);
        let location = Value::Object(self.layout(json!({ "op": "location", "pane": id })).await.unwrap_or_default());
        Ok(opened(summary, &location, &placed))
    }

    async fn showing(&self, path: &str, from: Option<&str>, window: Option<&str>) -> Result<Option<(String, Map)>, KmuxError> {
        self.showing_in(PaneType::Code, path, from, window).await
    }

    async fn showing_in(&self, kind: PaneType, path: &str, from: Option<&str>, window: Option<&str>) -> Result<Option<(String, Map)>, KmuxError> {
        let window = match (window, from.and_then(|from| self.state().panes.pane(from).map(|p| p.id.clone()))) {
            (Some(window), _) => Some(Value::String(window.into())),
            (None, Some(from)) => self.layout(json!({ "op": "location", "pane": from })).await?.remove("window"),
            (None, None) => None,
        };
        let Some(window) = window.filter(|w| !w.is_null()) else { return Ok(None) };
        let showing: Vec<String> = {
            let s = self.state();
            s.panes.ids().into_iter().filter(|id| s.panes.get(id).is_some_and(|p| p.kind == kind && p.path.as_deref() == Some(path))).collect()
        };
        for id in showing {
            let location = self.layout(json!({ "op": "location", "pane": id })).await?;
            if location.get("window") == Some(&window) {
                return Ok(Some((id, location)));
            }
        }
        Ok(None)
    }

    async fn open_again(&self, path: &str, line: Option<u64>, from: Option<&str>, window: Option<&str>) -> Result<Option<Map>, KmuxError> {
        let Some((id, location)) = self.showing(path, from, window).await? else { return Ok(None) };
        {
            let _turn = self.0.turns.take().await;
            self.layout(json!({ "op": "command", "cmd": "focus", "args": { "pane": id } })).await?;
        }
        self.changed();
        let current = self.state().panes.get(&id).and_then(|p| p.line);
        let summary = match line.filter(|line| Some(*line) != current) {
            Some(line) => self.navigate(&json!({ "pane": id, "path": path, "line": line })).await?.remove("pane").unwrap_or_default(),
            None => {
                let snapshot = self.state().panes.get(&id).cloned().unwrap();
                self.summary(&id, &snapshot)
            }
        };
        let location = Value::Object(location);
        Ok(Some(opened(summary, &location, &location)))
    }

    async fn comment(&self, args: &Value) -> Result<Map, KmuxError> {
        if !self.state().pane_types.contains(&PaneType::Code) {
            return Err(bad_request("this mux has no code panes"));
        }
        let path = string(args, "path").filter(|p| !p.is_empty()).ok_or_else(|| bad_request("missing path"))?.to_string();
        let text = string(args, "text").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad_request("missing text"))?.to_string();
        let line = args.get("line").and_then(Value::as_u64).filter(|l| *l > 0).ok_or_else(|| bad_request("missing line"))?;
        let mut asked = json!({ "path": path, "line": line });
        if let Some(last) = args.get("last").and_then(Value::as_u64) {
            if last < line {
                return Err(bad_request(format!("lines {line}-{last} run backwards")));
            }
            asked["last"] = json!(last);
        }
        let snapped = self.ask_app("app.snap", asked).await.map_err(|error| match error.code.as_str() {
            "no_app" => KmuxError::new("no_app", "a comment needs Kanna.app, and none is attached to this daemon"),
            _ => error,
        })?;
        let first = snapped.get("first").and_then(Value::as_u64).unwrap_or(line);
        let last = snapped.get("last").and_then(Value::as_u64).unwrap_or(first).max(first);
        self.place_comment(Comment { path, first, last, text }, args, None).await
    }

    async fn place_comment(&self, comment: Comment, args: &Value, stop: Option<StopMark>) -> Result<Map, KmuxError> {
        let (path, first, last) = (comment.path.clone(), comment.first, comment.last);
        let placeless = ["name", "tab", "dock", "split", "size"].iter().all(|key| args.get(*key).is_none());
        let from = string(args, "from");
        let found = if placeless { self.showing(&path, from, string(args, "window")).await? } else { None };
        let (id, location, was_open) = match found {
            Some((id, location)) => {
                {
                    let _turn = self.0.turns.take().await;
                    self.layout(json!({ "op": "command", "cmd": "focus", "args": { "pane": id } })).await?;
                }
                if let Some(pane) = self.state().panes.get_mut(&id) {
                    pane.line = Some(first);
                    pane.comment = Some(comment.clone());
                    pane.stop = stop.clone();
                }
                let snapshot = self.reload(&id, false).await;
                self.check_started(&id, &snapshot)?;
                (id, Value::Object(location), true)
            }
            None => {
                let mut open = args.clone();
                for key in ["text", "last"] {
                    open.as_object_mut().map(|o| o.remove(key));
                }
                open["type"] = json!("code");
                open["line"] = json!(first);
                open["comment"] = comment.to_json();
                if let Some(stop) = &stop {
                    open["tour"] = stop.to_json();
                }
                let mut reply = self.open(&open).await?;
                let id = reply.get("pane").and_then(|p| string(p, "id")).unwrap_or_default().to_string();
                let location = json!({ "window": reply.remove("window").unwrap_or(Value::Null), "tab": reply.remove("tab").unwrap_or(Value::Null), "workspace": reply.remove("workspace").unwrap_or(Value::Null) });
                (id, location, false)
            }
        };
        let snapshot = self.state().panes.get(&id).cloned().ok_or_else(|| KmuxError::new("not_found", "the pane closed"))?;
        let mut out = reply([("pane", self.summary(&id, &snapshot)), ("first", num(first as f64)), ("last", num(last as f64)), ("opened", Value::Bool(!was_open))]);
        for key in ["window", "tab"] {
            out.insert(key.into(), location.get(key).cloned().unwrap_or(Value::Null));
        }
        if let Some(workspace) = location.get("workspace").filter(|w| !w.is_null()) {
            out.insert("workspace".into(), workspace.clone());
        }
        Ok(out)
    }

    async fn ask_app(&self, cmd: &str, args: Value) -> Result<Map, KmuxError> {
        let prefixed = self.0.prefixes.borrow().iter().find(|(prefix, _)| cmd.starts_with(prefix.as_str())).map(|(_, h)| h.clone());
        let Some(handler) = prefixed else { return Err(KmuxError::new("no_app", format!("{cmd} needs Kanna.app"))) };
        let (responder, answer) = responder();
        handler(cmd, args, responder);
        answer.await
    }

    async fn task_folder(&self, from: Option<&str>, window: Option<&str>, workspace: Option<&str>) -> Option<String> {
        let task = match from {
            Some(from) => from.to_string(),
            None => {
                let mut listed = self.layout(json!({ "op": "windows" })).await.ok()?;
                let window = window.map(|id| Value::String(id.into()));
                let workspaces = listed.remove("workspaces")?;
                let ws = workspaces.as_array()?.iter().find(|ws| match (&window, workspace) {
                    (_, Some(id)) => ws["id"] == id,
                    (Some(window), None) => ws["windows"].as_array().is_some_and(|ws| ws.iter().any(|w| w == window || &w["id"] == window)),
                    (None, None) => false,
                })?;
                string(&ws["dock"], "pane")?.to_string()
            }
        };
        self.state().panes.pane(&task)?.opened_in.clone()
    }

    async fn refuse_docked(&self, id: &str) -> Result<(), KmuxError> {
        let location = self.layout(json!({ "op": "location", "pane": id })).await?;
        if location.get("dock").is_some_and(|dock| !dock.is_null()) {
            return Err(KmuxError::new("docked", "docked: it closes with its window"));
        }
        Ok(())
    }

    fn check_started(&self, id: &str, fallback: &Pane) -> Result<(), KmuxError> {
        let s = self.state();
        let pane = s.panes.get(id).unwrap_or(fallback);
        if pane.state == PaneState::Failed {
            return Err(KmuxError::new("start_failed", pane.error.clone().unwrap_or_else(|| format!("{} failed to start", pane.label()))));
        }
        Ok(())
    }

    async fn close(&self, args: &Value) -> Result<Map, KmuxError> {
        let _turn = self.0.turns.take().await;
        let mut workspace = None;
        let victims: Vec<String> = if let Some(reference) = string(args, "pane") {
            let id = self.need_pane(Some(reference))?.id;
            self.refuse_docked(&id).await?;
            vec![id]
        } else {
            let scope = if let Some(reference) = string(args, "tab") {
                Scope::Tab(reference.into())
            } else if let Some(task) = string(args, "task") {
                Scope::Task(self.need_pane(Some(task))?.id)
            } else if args.get("window").is_some() {
                Scope::Window(string(args, "window").map(String::from))
            } else {
                return Err(bad_request("close needs a pane, tab, window or task"));
            };
            let found = self.layout(scope_request(&scope)).await?;
            let ids = found.get("panes").and_then(Value::as_array).cloned().unwrap_or_default();
            let kept = match &scope {
                Scope::Task(task) if is_true(args, "tabs") => Some(task.clone()),
                Scope::Task(task) => {
                    workspace = Some(task.clone());
                    None
                }
                _ => None,
            };
            ids.iter().filter_map(Value::as_str).filter(|id| kept.as_deref() != Some(*id)).map(String::from).collect()
        };
        let known: Vec<&String> = victims.iter().filter(|id| self.state().panes.get(id).is_some()).collect();
        for id in known {
            self.forget_pane(id);
        }
        if let Err(error) = self.layout(json!({ "op": "remove", "ids": victims, "task": workspace, "tidy": true })).await {
            eprintln!("kanna-mux: closed {} but the layout kept them: {error}", victims.join(", "));
        }
        self.changed();
        Ok(reply([("closed", victims.into())]))
    }

    fn mark_running(&self, listed: &mut Map) {
        let s = self.state();
        for task in listed.get_mut("tasks").and_then(Value::as_array_mut).into_iter().flatten() {
            let Some(pane) = task.get("id").and_then(Value::as_str).and_then(|id| s.panes.get(id)) else { continue };
            let running = matches!(pane.state, PaneState::Starting | PaneState::Running);
            let status = pane.status().map_or(Value::Null, |s| s.into());
            task["running"] = Value::Bool(running);
            task["status"] = status;
        }
    }

    pub async fn room_for_task(&self, cwd: Option<&str>, app: &str) -> Result<(), KmuxError> {
        let Ok(listed) = self.layout(json!({ "op": "command", "cmd": "task", "args": { "action": "list", "app": app } })).await else { return Ok(()) };
        let here = project(cwd);
        let same: Vec<String> = {
            let s = self.state();
            let tasks = listed.get("tasks").and_then(Value::as_array).cloned().unwrap_or_default();
            tasks.iter().filter_map(|t| s.panes.get(t["id"].as_str()?)).filter(|p| project(p.cwd.as_deref()) == here).map(|p| p.label()).collect()
        };
        if same.len() < TASKS_PER_PROJECT {
            return Ok(());
        }
        let place = here.map_or("this project".to_string(), |p| p.display().to_string());
        Err(KmuxError::new("task_limit", format!("{place} has {TASKS_PER_PROJECT} tasks on the Shelf already ({}): move one to the backlog with `kanna task defer TASK` first", same.join(", "))))
    }

    fn forget_pane(&self, id: &str) {
        self.forget_tour_of(id);
        let pane = self.state().panes.forget(id);
        if let (Some(host), Some(pane)) = (self.host(), &pane) {
            host.stop(pane);
        }
        self.resume(id);
    }

    async fn reload(&self, id: &str, clear_exit: bool) -> Pane {
        let pane = self.state().panes.get(id).cloned().unwrap();
        if let Some(host) = self.host() {
            host.stop(&pane);
        }
        let renamed = self.state().panes.renew_terminal(id);
        let id = match &renamed {
            Some(to) => {
                let _ = self.layout(json!({ "op": "rename", "from": id, "to": to })).await;
                let hooks: Vec<Rc<dyn Fn(&str, &str)>> = self.0.renamed.borrow().clone();
                for hook in hooks {
                    hook(id, to);
                }
                to.as_str()
            }
            None => id,
        };
        if let Some(pane) = self.state().panes.get_mut(id) {
            pane.state = PaneState::Starting;
            pane.pid = None;
            pane.hung_up = false;
            if clear_exit {
                pane.exit_code = None;
            }
            pane.error = None;
        }
        self.changed();
        self.start_pane(id);
        let snapshot = self.state().panes.get(id).cloned().unwrap_or(pane);
        self.settled(id).await;
        snapshot
    }

    async fn restart(&self, args: &Value) -> Result<Map, KmuxError> {
        let id = self.need_pane(string(args, "pane"))?.id;
        let snapshot = self.reload(&id, true).await;
        self.check_started(&snapshot.id, &snapshot)?;
        Ok(reply([("pane", self.summary(&snapshot.id, &snapshot))]))
    }

    async fn send(&self, args: &Value) -> Result<Map, KmuxError> {
        let pane = self.need_pane(string(args, "pane"))?;
        if pane.kind != PaneType::Term {
            return Err(KmuxError::new("wrong_type", format!("{} is a {} pane; send only works on term panes", pane.label(), pane.kind.as_str())));
        }
        if pane.state != PaneState::Running {
            return Err(bad_request(format!("{} is {}", pane.label(), pane.state.as_str())));
        }
        let Some(host) = self.host() else { return Ok(Map::new()) };
        let (responder, answer) = responder();
        host.send(&pane, string(args, "text").unwrap_or(""), responder);
        answer.await
    }

    async fn send_key(&self, args: &Value) -> Result<Map, KmuxError> {
        let pane = self.need_pane(string(args, "pane"))?;
        if pane.kind != PaneType::Term {
            return Err(KmuxError::new("wrong_type", format!("{} is a {} pane; send-key only works on term panes", pane.label(), pane.kind.as_str())));
        }
        let keys: Vec<String> = match args.get("keys") {
            Some(Value::Array(keys)) if !keys.is_empty() => keys.iter().map(|k| k.as_str().map(str::to_string)).collect::<Option<_>>().ok_or_else(|| bad_request("keys must be strings"))?,
            _ => return Err(bad_request("missing keys")),
        };
        if pane.state != PaneState::Running {
            return Err(bad_request(format!("{} is {}", pane.label(), pane.state.as_str())));
        }
        let Some(host) = self.host() else { return Ok(Map::new()) };
        let (responder, answer) = responder();
        host.send_keys(&pane, keys, responder);
        answer.await
    }

    fn read(&self, args: &Value) -> Result<Map, KmuxError> {
        let pane = self.need_pane(string(args, "pane"))?;
        if pane.kind != PaneType::Term {
            return Err(KmuxError::new("wrong_type", format!("{} is a {} pane; read only works on term panes", pane.label(), pane.kind.as_str())));
        }
        let lines = match args.get("lines") {
            None | Some(Value::Null) => None,
            Some(value) => match value.as_f64() {
                Some(n) if n >= 1.0 && n.fract() == 0.0 => Some(n as usize),
                _ => return Err(bad_request("lines must be a whole number above 0")),
            },
        };
        let host = self.host().ok_or_else(|| KmuxError::new("unsupported", "nothing runs terminals here"))?;
        let text = host.read(&pane, lines)?;
        Ok(reply([("text", text.into())]))
    }

    async fn navigate(&self, args: &Value) -> Result<Map, KmuxError> {
        let pane = self.need_pane(string(args, "pane"))?;
        let step = if is_true(args, "back") {
            Some("back")
        } else if is_true(args, "forward") {
            Some("forward")
        } else {
            None
        };
        let mut changed = pane.clone();
        if pane.kind == PaneType::Diff {
            if step.is_some() {
                return Err(bad_request("a diff pane keeps no history"));
            }
            changed.diff = Some(read_diff_args(args.get("args"))?.unwrap_or_default());
        } else if matches!(pane.kind, PaneType::Md | PaneType::Code) {
            let code = pane.kind == PaneType::Code;
            if !code && args.get("line").is_some() {
                return Err(bad_request("line is only for code panes"));
            }
            let mut history = pane.history.clone().unwrap_or_default();
            let current = spot(pane.path.as_deref().unwrap_or_default(), pane.line);
            let target = if let Some(step) = step {
                let target = if step == "back" { history.back.pop() } else { history.forward.pop() };
                let target = target.ok_or_else(|| bad_request(format!("nothing to go {step} to")))?;
                if step == "back" { history.forward.push(current) } else { history.back.push(current) }
                target
            } else {
                let path = string(args, "path").filter(|p| !p.is_empty()).ok_or_else(|| bad_request("missing path"))?;
                let target = spot(&resolve(path, pane.path.as_deref().unwrap_or_default()), line_arg(args)?);
                if target != current {
                    history.back.push(current);
                    history.forward.clear();
                }
                target
            };
            let (path, line) = if code { unspot(&target) } else { (target, None) };
            changed.history = Some(history);
            changed.path = Some(path);
            changed.line = line;
        } else {
            if pane.kind != PaneType::Web {
                return Err(KmuxError::new(
                    "wrong_type",
                    format!("{} is a {} pane; navigate only works on web, markdown, code and diff panes", pane.label(), pane.kind.as_str()),
                ));
            }
            if let Some(step) = step {
                let mut history = pane.history.clone().ok_or_else(|| bad_request(format!("{} keeps no history; open it with history: true", pane.label())))?;
                let url = if step == "back" { history.back.pop() } else { history.forward.pop() };
                let url = url.ok_or_else(|| bad_request(format!("nothing to go {step} to")))?;
                if let Some(current) = &pane.url {
                    if step == "back" { history.forward.push(current.clone()) } else { history.back.push(current.clone()) }
                }
                changed.history = Some(history);
                changed.url = Some(url);
            } else {
                let url = match (string(args, "url").filter(|u| !u.is_empty()), string(args, "path").filter(|p| p.starts_with('/'))) {
                    (Some(url), _) => normalize_url(url),
                    (None, Some(path)) => format!("file://{path}"),
                    (None, None) => return Err(bad_request("missing url")),
                };
                moved(&mut changed, url);
            }
        }
        if let Some(pane) = self.state().panes.get_mut(&pane.id) {
            pane.history = changed.history;
            pane.path = changed.path;
            pane.line = changed.line;
            pane.url = changed.url;
            pane.diff = changed.diff;
        }
        let snapshot = self.reload(&pane.id, false).await;
        Ok(reply([("pane", self.summary(&pane.id, &snapshot))]))
    }

    async fn markup(&self, args: &Value) -> Result<Map, KmuxError> {
        let pane = self.need_pane(string(args, "pane"))?;
        if !matches!(pane.kind, PaneType::Md | PaneType::Web) {
            return Err(KmuxError::new(
                "wrong_type",
                format!("{} is a {} pane; comments only work on web and markdown panes", pane.label(), pane.kind.as_str()),
            ));
        }
        let on = !is_true(args, "off");
        if on && pane.diff_base.as_deref().is_some_and(is_range) {
            return Err(bad_request(format!("{} shows a range ({}), a past version of the doc, so it takes no comments", pane.label(), pane.diff_base.as_deref().unwrap_or_default())));
        }
        let agent = match string(args, "agent").filter(|_| on) {
            Some(agent) => Some(self.need_pane(Some(agent))?.id),
            None => None,
        };
        if let Some(pane) = self.state().panes.get_mut(&pane.id) {
            pane.markup = on;
            if agent.is_some() {
                pane.agent = agent;
            }
        }
        let snapshot = self.reload(&pane.id, false).await;
        Ok(reply([("pane", self.summary(&pane.id, &snapshot))]))
    }

    async fn rename_pane(&self, args: &Value) -> Result<Map, KmuxError> {
        let pane = self.need_pane(string(args, "pane"))?;
        let name = string(args, "name").map(str::trim).filter(|n| !n.is_empty()).map(String::from);
        if let Some(name) = &name {
            if self.state().panes.pane(name).is_some_and(|other| other.id != pane.id) {
                return Err(KmuxError::new("name_taken", format!("a pane named \"{name}\" already exists")));
            }
        }
        if let Some(found) = self.state().panes.get_mut(&pane.id) {
            found.name = name;
        }
        self.changed();
        let _ = self.layout(json!({ "op": "windows" })).await;
        Ok(reply([("pane", self.summary(&pane.id, &pane))]))
    }

    async fn export_state(&self) -> Result<Value, KmuxError> {
        let mut out = self.layout(json!({ "op": "export" })).await?;
        let s = self.state();
        let mut counters = match out.remove("counters") {
            Some(Value::Object(counters)) => counters,
            _ => Map::new(),
        };
        let version = if s.panes.content_ids() {
            let numbers = s.panes.numbers();
            counters.insert("term".into(), num(s.panes.terminals() as f64));
            for prefix in CONTENT_PREFIXES.into_iter().filter(|p| *p != "term") {
                counters.insert(prefix.into(), num(numbers.get(prefix).copied().unwrap_or(0) as f64));
            }
            state::STATE_VERSION
        } else {
            counters.insert("pane".into(), num(s.panes.counter() as f64));
            1
        };
        out.insert("kmux".into(), "state".into());
        out.insert("version".into(), num(version as f64));
        out.insert("counters".into(), Value::Object(counters));
        out.insert("panes".into(), state::export_panes(&s.panes));
        Ok(Value::Object(out))
    }

    async fn restore_state(&self, saved: &Value) -> Result<(), KmuxError> {
        let content = self.state().panes.content_ids();
        let saved = &if content { state::upgrade(saved) } else { saved.clone() };
        let (panes, counter, numbers) = if content {
            let (panes, numbers) = state::import_content(saved)?;
            (panes, 0, numbers)
        } else {
            let (panes, counter) = state::import_panes(saved)?;
            (panes, counter, HashMap::new())
        };
        let ids: Vec<String> = panes.iter().map(|p| p.id.clone()).collect();
        let _turn = self.0.turns.take().await;
        self.layout(json!({ "op": "check", "state": saved, "ids": ids })).await?;
        let old = self.state().panes.ids();
        for id in &old {
            self.forget_pane(id);
        }
        self.layout(json!({ "op": "remove", "ids": old })).await?;
        let installed = self.layout(json!({ "op": "install", "state": saved, "ids": ids })).await?;
        let placed: HashSet<&str> = installed.get("placed").and_then(Value::as_array).map(|ids| ids.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let restored = {
            let mut s = self.state();
            let restored: Vec<Pane> = panes.into_iter().filter(|p| placed.contains(p.id.as_str())).collect();
            let ids = restored.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
            s.panes.install(restored, counter, numbers.get("term").copied().unwrap_or(0));
            s.panes.note_numbers(&numbers);
            ids
        };
        self.changed();
        for id in &restored {
            self.start_pane(id);
        }
        Ok(())
    }
}

fn opened(summary: Value, location: &Value, placed: &Value) -> Map {
    let window = if location["window"].is_null() { placed["window"].clone() } else { location["window"].clone() };
    let mut out = reply([("pane", summary), ("window", window), ("tab", location["tab"].clone())]);
    if let Some(edge) = location.get("dock").filter(|d| !d.is_null()) {
        out.insert("dock".into(), edge.clone());
    }
    if let Some(workspace) = location.get("workspace").or_else(|| placed.get("workspace")).filter(|w| !w.is_null()) {
        out.insert("workspace".into(), workspace.clone());
    }
    out
}

fn moved(pane: &mut Pane, url: String) {
    let same = |a: &str, b: &str| a == b || format!("{a}/") == b || a == format!("{b}/");
    if let (Some(history), Some(current)) = (pane.history.as_mut(), pane.url.as_ref()) {
        if !same(current, &url) {
            history.back.push(current.clone());
            history.forward.clear();
        }
    }
    pane.url = Some(url);
}

pub fn normalize_url(url: &str) -> String {
    let mut chars = url.chars();
    let scheme = chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && {
        let rest = &url[1..];
        let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '+' || c == '.' || c == '-')).unwrap_or(rest.len());
        rest[end..].starts_with("://")
    };
    if scheme { url.into() } else { format!("http://{url}") }
}

fn line_arg(args: &Value) -> Result<Option<u64>, KmuxError> {
    match args.get("line") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => match value.as_f64() {
            Some(n) if n >= 1.0 && n.fract() == 0.0 => Ok(Some(n as u64)),
            _ => Err(bad_request("line must be a whole number above 0")),
        },
    }
}

fn spot(path: &str, line: Option<u64>) -> String {
    match line {
        Some(line) => format!("{path}:{line}"),
        None => path.into(),
    }
}

fn unspot(spot: &str) -> (String, Option<u64>) {
    match spot.rsplit_once(':') {
        Some((path, line)) if !path.is_empty() => match line.parse::<u64>() {
            Ok(n) if n > 0 => (path.into(), Some(n)),
            _ => (spot.into(), None),
        },
        _ => (spot.into(), None),
    }
}

pub fn resolve(link: &str, file: &str) -> String {
    if link.starts_with('/') {
        return link.into();
    }
    let mut parts: Vec<&str> = file.split('/').collect();
    parts.pop();
    for part in link.split('/').filter(|p| !p.is_empty()) {
        if part == ".." {
            parts.pop();
        } else if part != "." {
            parts.push(part);
        }
    }
    parts.join("/")
}

pub struct Responder(Arc<Mutex<Answer>>);

#[derive(Default)]
struct Answer {
    result: Option<Result<Map, KmuxError>>,
    waker: Option<Waker>,
    answered: bool,
}

impl Responder {
    pub fn reply(mut self, result: Result<Map, KmuxError>) {
        self.answer(result);
    }

    fn answer(&mut self, result: Result<Map, KmuxError>) {
        let waker = {
            let mut answer = self.0.lock().unwrap();
            if answer.answered {
                return;
            }
            answer.answered = true;
            answer.result = Some(result);
            answer.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        self.answer(Err(bad_request("the command ended without answering")));
    }
}

pub fn responder() -> (Responder, impl Future<Output = Result<Map, KmuxError>>) {
    let answer = Arc::new(Mutex::new(Answer::default()));
    let waiting = answer.clone();
    let future = poll_fn(move |cx| {
        let mut answer = waiting.lock().unwrap();
        match answer.result.take() {
            Some(result) => Poll::Ready(result),
            None => {
                answer.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    });
    (Responder(answer), future)
}

fn is_range(diff: &str) -> bool {
    diff.contains("..")
}
