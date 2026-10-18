use crate::status::{from_record, Detector};
use kanna_vt::{Need, ProgramStatus};
use crate::Event;
use kanna_detect::{Agent, Config, Report, Screen, Verdict, RULES_SITE};
use kanna_mux::json::{json, Map, Value};
use kanna_mux::{CoreHandle, KmuxError, Responder, WeakCore};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Sender;

pub fn rules_config(state_dir: &Path) -> Config {
    let mut config = Config::new(state_dir.join("agent-rules"));
    let env = |name: &str| std::env::var(name).ok();
    config.overrides = Some(match env("KANNA_AGENT_RULES_DIR").filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => {
            let config_home = env("XDG_CONFIG_HOME").filter(|d| !d.is_empty()).map(PathBuf::from);
            config_home.unwrap_or_else(|| PathBuf::from(env("HOME").unwrap_or_default()).join(".config")).join("kanna/agent-rules")
        }
    });
    config.site = match env("KANNA_AGENT_RULES_URL") {
        None => Some(RULES_SITE.into()),
        Some(url) if url.is_empty() || url == "none" => None,
        Some(url) => Some(url),
    };
    config
}

pub struct StatusCommand {
    detector: Rc<Detector>,
    events: Sender<Event>,
    pending: RefCell<HashMap<u64, Responder>>,
    next: Cell<u64>,
}

fn bad(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

impl StatusCommand {
    pub fn new(detector: Rc<Detector>, events: Sender<Event>) -> StatusCommand {
        StatusCommand { detector, events, pending: RefCell::default(), next: Cell::new(0) }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let this = Rc::downgrade(self);
        let weak: WeakCore = core.weak();
        core.add_command(
            "status",
            Rc::new(move |args, responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                match args.get("action").and_then(Value::as_str) {
                    Some("explain") => match weak.upgrade() {
                        Some(core) => responder.reply(this.explain(&core, &args)),
                        None => responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))),
                    },
                    Some("update") => this.update(responder),
                    _ => responder.reply(Err(bad("status needs an action: explain or update"))),
                }
            }),
        );
    }

    fn explain(&self, core: &CoreHandle, args: &Value) -> Result<Map, KmuxError> {
        if let Some(reference) = args.get("pane").and_then(Value::as_str) {
            let pane = core.pane(reference).ok_or_else(|| KmuxError::new("not_found", format!("no pane \"{reference}\"")))?;
            let explained = pane.terminal.as_deref().and_then(|terminal| self.detector.explain(terminal)).ok_or_else(|| bad(format!("{} isn't a running terminal", pane.label())))?;
            let mut out = Map::new();
            out.insert("pane".into(), json!(pane.id));
            out.insert("agent".into(), json!(explained.agent));
            out.insert("executable".into(), json!(explained.executable));
            out.insert("status".into(), json!(pane.status()));
            describe(&mut out, explained.verdict.as_ref(), &explained.screen);
            out.insert("source".into(), json!(if explained.reported.is_some() { "osc7501" } else { "rules" }));
            out.insert("reported".into(), explained.reported.as_ref().map_or(Value::Null, record));
            out.insert("records".into(), Value::Array(explained.records.iter().map(record).collect()));
            return Ok(out);
        }
        let agent = args.get("agent").and_then(Value::as_str).ok_or_else(|| bad("say which pane, or give a screen and its agent"))?;
        let read;
        let given = match (args.get("screen"), args.get("file").and_then(Value::as_str)) {
            (Some(screen), _) => screen,
            (None, Some(file)) => {
                read = read_screen(file)?;
                &read
            }
            _ => return Err(bad("give the screen to explain")),
        };
        let lines: Vec<String> = match (given.get("rows").and_then(Value::as_array), given.get("text").and_then(Value::as_str)) {
            (Some(rows), _) => rows.iter().map(|r| r.as_str().unwrap_or_default().to_string()).collect(),
            (None, Some(text)) => text.lines().map(str::to_owned).collect(),
            _ => return Err(bad("the screen needs rows or text")),
        };
        let mut lines = lines;
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        let text = |key: &str| given.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let screen = Screen { lines, title: text("title"), progress: text("progress") };
        let version = given.get("cliVersion").or_else(|| args.get("version")).and_then(Value::as_str);
        let rules = self.detector.store().rules();
        let verdict = rules.evaluate(agent, &screen, version).ok_or_else(|| {
            let known: Vec<&str> = rules.agents().map(Agent::name).collect();
            bad(format!("no rules for \"{agent}\" (there are rules for {})", known.join(", ")))
        })?;
        let mut out = Map::new();
        out.insert("agent".into(), json!(agent));
        describe(&mut out, Some(&verdict), &screen);
        Ok(out)
    }

    fn update(&self, responder: Responder) {
        let token = self.next.get() + 1;
        self.next.set(token);
        self.pending.borrow_mut().insert(token, responder);
        let (store, events) = (self.detector.store().clone(), self.events.clone());
        let started = std::thread::Builder::new().name("kanna.rules.update".into()).spawn(move || {
            let report = store.update_now();
            let _ = events.send(Event::RulesUpdated { token, report });
        });
        if started.is_err() {
            if let Some(responder) = self.pending.borrow_mut().remove(&token) {
                responder.reply(Err(KmuxError::new("failed", "couldn't start the update")));
            }
        }
    }

    pub fn updated(&self, token: u64, report: Report) {
        let Some(responder) = self.pending.borrow_mut().remove(&token) else { return };
        let loaded: Vec<Value> = report.loaded.iter().map(|l| json!({ "agent": l.agent, "version": l.version, "source": l.source })).collect();
        let mut out = Map::new();
        out.insert("site".into(), json!(self.detector.store().config().site));
        out.insert("fetched".into(), json!(report.fetched));
        out.insert("problems".into(), json!(report.problems));
        out.insert("rules".into(), Value::Array(loaded));
        responder.reply(Ok(out));
    }
}

fn read_screen(file: &str) -> Result<Value, KmuxError> {
    let size = std::fs::metadata(file).map_err(|e| bad(format!("can't read {file}: {e}")))?.len();
    if size > 4 << 20 {
        return Err(bad(format!("{file} is over 4 MiB; save just the screen")));
    }
    let text = std::fs::read_to_string(file).map_err(|e| bad(format!("can't read {file}: {e}")))?;
    Ok(match serde_json::from_str::<Value>(&text) {
        Ok(case) if case.get("rows").is_some_and(Value::is_array) => case,
        _ => json!({ "text": text }),
    })
}

fn record(status: &ProgramStatus) -> Value {
    json!({
        "id": status.id,
        "state": status.state.as_str(),
        "kind": status.need.map(Need::as_str),
        "progress": status.progress,
        "app": status.app,
        "title": status.title,
        "message": status.message,
        "status": from_record(status.state).as_str(),
    })
}

fn describe(out: &mut Map, verdict: Option<&Verdict>, screen: &Screen) {
    if let Some(v) = verdict {
        out.insert("agentVersion".into(), json!(v.agent_version));
        out.insert("rules".into(), json!({ "version": v.file_version, "source": v.source }));
        out.insert("judged".into(), json!(v.status.as_str()));
        out.insert("rule".into(), json!(v.rule));
        out.insert("matched".into(), json!(v.matched));
        out.insert("excluded".into(), json!(v.excluded));
    }
    let last: Vec<&String> = screen.lines.iter().rev().take(10).rev().collect();
    out.insert("screen".into(), json!({ "lines": last, "title": screen.title, "progress": screen.progress }));
}
