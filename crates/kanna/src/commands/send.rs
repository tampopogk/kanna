use super::{pane, usage, Command, Flow, PANE};
use serde_json::json;

pub const COMMAND: Command = Command {
    name: "send",
    group: "Panes",
    summary: "Type a line of text into a terminal pane and press Return.",
    usage: "kanna send PANE TEXT...",
    options: &[],
    notes: "Everything after PANE is the text. Quote it to keep your shell from interpreting it.\n-- ends the options: `kanna send term-1 -- --json` types --json.\nReturn is pressed on its own after the text (pasted, if the program asked for bracketed paste), so an agent like Claude Code submits it.",
    examples: &[("kanna send server \"npm test\"", "Run npm test in the server pane.")],
    parse: |args| {
        let pane = pane(args, "send")?;
        let text = args.rest().ok_or_else(|| usage("send", "missing TEXT"))?;
        Ok(json!({ "pane": pane, "text": text }))
    },
    show: |_, _| String::new(),
    flow: Flow { panes: PANE, ..Flow::PLAIN },
};
