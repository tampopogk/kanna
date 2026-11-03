use crate::status::{from_record, Detector};
use kanna_detect::agents;
use kanna_mux::json::{json, Map, Value};
use kanna_mux::{CoreHandle, KmuxError, WeakCore};
use kanna_vt::{Need, ProgramStatus};
use std::rc::Rc;

pub struct StatusCommand {
    detector: Rc<Detector>,
}

fn bad(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

impl StatusCommand {
    pub fn new(detector: Rc<Detector>) -> StatusCommand {
        StatusCommand { detector }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let this = Rc::downgrade(self);
        let weak: WeakCore = core.weak();
        core.add_command(
            "status",
            Rc::new(move |args, responder| {
                let (Some(this), Some(core)) = (this.upgrade(), weak.upgrade()) else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                match args.get("action").and_then(Value::as_str) {
                    Some("explain") => responder.reply(this.explain(&core, &args)),
                    _ => responder.reply(Err(bad("status needs an action: explain"))),
                }
            }),
        );
    }

    fn explain(&self, core: &CoreHandle, args: &Value) -> Result<Map, KmuxError> {
        let reference = args.get("pane").and_then(Value::as_str).ok_or_else(|| bad("say which pane"))?;
        let pane = core.pane(reference).ok_or_else(|| KmuxError::new("not_found", format!("no pane \"{reference}\"")))?;
        let explained = pane.terminal.as_deref().and_then(|terminal| self.detector.explain(terminal)).ok_or_else(|| bad(format!("{} isn't a running terminal", pane.label())))?;
        let mut out = Map::new();
        out.insert("pane".into(), json!(pane.id));
        out.insert("agent".into(), json!(explained.agent));
        out.insert("executable".into(), json!(explained.executable));
        out.insert("agentVersion".into(), json!(explained.version));
        out.insert("needs".into(), json!(explained.agent.as_deref().and_then(agents::minimum)));
        out.insert("status".into(), json!(pane.status()));
        out.insert("reported".into(), explained.reported.as_ref().map_or(Value::Null, record));
        out.insert("records".into(), Value::Array(explained.records.iter().map(record).collect()));
        Ok(out)
    }
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
