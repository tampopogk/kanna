use super::{pane, usage, Command, Flow, PANE};
use serde_json::json;

pub const COMMAND: Command = Command {
    name: "read",
    group: "Panes",
    summary: "Print a terminal pane's screen as text.",
    usage: "kanna read PANE [--lines N]",
    options: &[("--lines N", "The last N lines instead of the screen, counting scrollback.")],
    notes: "Terminal panes only. Works with Kanna.app closed: the daemon keeps every terminal's screen.\nPANE may be a task (task-ac2b, ac2b): its agent's screen, read on whichever of your machines runs it.",
    examples: &[("kanna read server", "What the server pane shows now."), ("kanna read ac2b", "What task ac2b's agent shows, on any of your machines."), ("kanna read server --lines 200", "Its last 200 lines, scrollback included.")],
    parse: |args| {
        let lines = match args.option("--lines")? {
            Some(n) => Some(n.parse::<u64>().ok().filter(|n| *n > 0).ok_or_else(|| usage("read", "--lines needs a whole number above 0"))?),
            None => None,
        };
        let mut out = json!({ "pane": pane(args, "read")? });
        if let Some(lines) = lines {
            out["lines"] = json!(lines);
        }
        Ok(out)
    },
    show: |_, reply| reply["text"].as_str().unwrap_or_default().to_string(),
    flow: Flow { panes: PANE, ..Flow::PLAIN },
};
