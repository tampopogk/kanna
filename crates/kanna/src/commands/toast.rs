use super::{from_caller, usage, Command, Flow};
use crate::args::Argv;
use kanna_client::Failure;
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "toast",
    group: "Information",
    summary: "Show a toast in Kanna.app: a small notice in the front window that fades by itself.",
    usage: "kanna toast TITLE [DETAIL] [--pane PANE] [--task TASK] [--key KEY]",
    options: &[
        ("--pane PANE", "The pane it's about: clicking the toast shows that pane, bringing its task onto the Desk."),
        ("--task TASK", "The task it's about (an ID like task-3f9a, or its pane): clicking the toast brings it onto the Desk."),
        ("--key KEY", "Raise it once: a later toast with the same key, from the CLI or the daemon, isn't shown until the daemon restarts."),
    ],
    notes: "The daemon raises toasts for its warnings, such as messages that wait because their recipient can't take them. A toast stacks below the title bar on the right, newest on top, and fades after 6 seconds (8 when it's about a pane or task) unless the pointer is on it. With no app attached, a toast waits up to a minute for one.",
    examples: &[
        ("kanna toast \"Build finished\" \"scripts/build.sh took 3m 12s\"", "A toast with a line of detail."),
        ("kanna toast \"Tests failed\" \"3 of 120\" --pane tests --key tests-failed", "A toast about the pane named tests, raised once."),
    ],
    parse,
    show,
    flow: Flow::PLAIN,
};

fn parse(args: &mut Argv) -> Result<Value, Failure> {
    let mut out = json!({});
    for flag in ["pane", "task", "key"] {
        if let Some(value) = args.option(&format!("--{flag}"))? {
            out[flag] = json!(value);
        }
    }
    out["title"] = json!(args.positional().filter(|t| !t.trim().is_empty()).ok_or_else(|| usage("toast", "missing TITLE"))?);
    if let Some(detail) = args.rest() {
        out["detail"] = json!(detail);
    }
    if let Some(fields) = out.as_object_mut() {
        from_caller(fields);
    }
    Ok(out)
}

pub fn show(_: &Value, reply: &Value) -> String {
    match reply["toast"].as_str() {
        Some(id) if reply["shown"] == true => format!("raised {id}"),
        Some(id) => format!("raised {id}; no app is attached, so it shows when one attaches within a minute"),
        None => format!("already raised a toast with key {}", reply["already"].as_str().unwrap_or("?")),
    }
}
