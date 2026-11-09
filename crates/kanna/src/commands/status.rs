use super::{pane, usage, Command, Flow, PANE};
use crate::args::Argv;
use kanna_client::Failure;
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "status",
    group: "Information",
    summary: "Explain an agent pane's status.",
    usage: "kanna status explain PANE",
    options: &[
        ("--file SCREEN", "A saved screen: text (as `kanna read` prints it) or a corpus case (JSON with rows, title, progress and cliVersion)."),
        ("--agent AGENT", "Whose rules judge the saved screen: claude, codex, …"),
    ],
    notes: "An agent pane's status (working, needs-you, idle, exited) is in `kanna task list`, and in `kanna workspace list --all --json`. It comes only from the agent's own report (OSC 7501). explain shows the agent, its version and the version Kanna needs, and the records it has reported, the root one first.",
    examples: &[
        ("kanna status explain term-3", "Why term-3's agent shows the status it does."),
    ],
    parse,
    show,
    flow: Flow { panes: PANE, ..Flow::PLAIN },
};

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    match args.positional().as_deref() {
        Some("explain") => Ok(json!({ "action": "explain", "pane": pane(args, "status")? })),
        Some(other) => Err(usage("status", &format!("unknown action \"{other}\" (explain)"))),
        None => Err(usage("status", "say what: explain PANE")),
    }
}

fn program_record(record: &Value) -> String {
    let mut parts = vec![format!("state={}", record["state"].as_str().unwrap_or("?"))];
    if let Some(kind) = record["kind"].as_str() {
        parts.push(format!("kind={kind}"));
    }
    if let Some(progress) = record["progress"].as_u64() {
        parts.push(format!("progress={progress}"));
    }
    for (key, name) in [("app", "app"), ("title", "title"), ("message", "msg")] {
        if let Some(text) = record[key].as_str().filter(|t| !t.is_empty()) {
            parts.push(format!("{name}={text:?}"));
        }
    }
    parts.join(" ")
}

pub fn show(_: &Value, reply: &Value) -> String {
    let mut out = vec![];
    let who = reply["pane"].as_str().map(|p| format!("{p}: ")).unwrap_or_default();
    let Some(agent) = reply["agent"].as_str() else {
        return format!("{who}not one of Kanna's agents{}", reply["status"].as_str().map(|s| format!("; status {s}")).unwrap_or_default());
    };
    let version = reply["agentVersion"].as_str().map(|v| format!(" {v}")).unwrap_or_else(|| " (version unknown)".into());
    let exe = reply["executable"].as_str().map(|e| format!(" ({e})")).unwrap_or_default();
    out.push(format!("{who}{agent}{version}{exe}"));
    if let Some(needs) = reply["needs"].as_str() {
        out.push(format!("needs: {needs} or newer"));
    }
    out.push(format!("status: {}", reply["status"].as_str().unwrap_or("none")));
    if reply["reported"].is_null() {
        out.push("record: no report yet".into());
    } else {
        out.push(format!("record: {}", program_record(&reply["reported"])));
    }
    for child in reply["records"].as_array().into_iter().flatten().filter(|r| r["id"].as_str().is_some_and(|id| !id.is_empty())) {
        out.push(format!("record {:?}: {}", child["id"].as_str().unwrap_or_default(), program_record(child)));
    }
    out.join("\n")
}

#[cfg(test)]
mod status_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn explain_before_a_report() {
        let reply = json!({ "pane": "p3", "agent": "claude", "agentVersion": "2.1.296", "executable": "/bin/claude", "needs": "2.1.295", "status": null, "reported": null, "records": [] });
        assert_eq!(show(&json!({ "action": "explain" }), &reply), "p3: claude 2.1.296 (/bin/claude)\nneeds: 2.1.295 or newer\nstatus: none\nrecord: no report yet");
        assert_eq!(show(&json!({ "action": "explain" }), &json!({ "pane": "p4", "agent": null, "status": null })), "p4: not one of Kanna's agents");
    }

    #[test]
    fn explain_shows_the_agents_own_report() {
        let reply = json!({ "pane": "p3", "agent": "claude", "agentVersion": "2.1.300", "executable": "/bin/claude", "needs": "2.1.295", "status": "needs-you",
            "reported": { "id": "", "state": "blocked", "kind": "permission", "progress": null, "app": "claude-code", "title": "", "message": "Edit \"main.rs\"?", "status": "needs-you" },
            "records": [
                { "id": "", "state": "blocked", "kind": "permission", "progress": null, "app": "claude-code", "title": "", "message": "Edit \"main.rs\"?", "status": "needs-you" },
                { "id": "build", "state": "working", "kind": null, "progress": 40, "app": "", "title": "", "message": "", "status": "working" }
            ] });
        assert_eq!(
            show(&json!({ "action": "explain" }), &reply),
            "p3: claude 2.1.300 (/bin/claude)\nneeds: 2.1.295 or newer\nstatus: needs-you\nrecord: state=blocked kind=permission app=\"claude-code\" msg=\"Edit \\\"main.rs\\\"?\"\nrecord \"build\": state=working progress=40"
        );
    }
}
