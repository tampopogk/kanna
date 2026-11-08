use super::{usage, Command, Flow, PANE};
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "rename-pane",
    group: "Panes",
    summary: "Give a pane a name, change it, or clear it.",
    usage: "kanna rename-pane PANE NAME",
    options: &[],
    notes: "PANE is the pane's current name or its ID (see `kanna workspace list --all`). An empty NAME (\"\") clears the name, so the pane goes by its ID. No two panes share a name, and a name can't be another pane's ID.",
    examples: &[("kanna rename-pane term-3 server", "Call term-3 \"server\"."), ("kanna rename-pane server \"\"", "Clear the name; refer to it as term-3 again.")],
    parse: |args| {
        let pane = args.positional().ok_or_else(|| usage("rename-pane", "missing PANE"))?;
        let name = args.positional().ok_or_else(|| usage("rename-pane", "missing NAME (\"\" to clear it)"))?;
        Ok(json!({ "pane": pane, "name": name }))
    },
    show,
    flow: Flow { panes: PANE, ..Flow::PLAIN },
};

pub fn show(_: &Value, reply: &Value) -> String {
    let pane = &reply["pane"];
    let id = pane["id"].as_str().unwrap_or("?");
    match pane["name"].as_str() {
        Some(name) => format!("{id} is now \"{name}\""),
        None => format!("{id} has no name"),
    }
}
