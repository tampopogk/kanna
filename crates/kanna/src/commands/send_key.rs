use super::{pane, usage, Command, Flow};
use serde_json::json;

pub const COMMAND: Command = Command {
    name: "send-key",
    group: "Panes",
    summary: "Press keys in a terminal pane: answer a menu, interrupt a command, submit a prompt.",
    usage: "kanna send-key PANE KEY...",
    options: &[],
    notes: "Each KEY is a key, with ctrl+, alt+ or shift+ in front if held: return, esc, tab, space, backspace (or delete), forward-delete, up, down, left, right, home, end, pageup, pagedown, insert, f1 to f12, or one character (a, A, ?, 1). Keys go one after the other, each 50 ms after the last (the first too, so it follows kanna send as a key press), as the program in the pane expects them in its current mode. cmd+ keys only mean something to Kanna.app, so they're refused. To type text, use kanna send.\nWorks with Kanna.app closed.",
    examples: &[
        ("kanna send-key agent return", "Submit what's typed in the agent pane's prompt."),
        ("kanna send-key server ctrl+c", "Interrupt the server pane's command."),
        ("kanna send-key agent down down return", "Pick the third choice in the agent's menu."),
    ],
    parse: |args| {
        let pane = pane(args, "send-key")?;
        let keys = args.positionals();
        if keys.is_empty() {
            return Err(usage("send-key", "missing KEY (e.g. return, esc, ctrl+c, up)"));
        }
        Ok(json!({ "pane": pane, "keys": keys }))
    },
    show: |_, _| String::new(),
    flow: Flow::PLAIN,
};
