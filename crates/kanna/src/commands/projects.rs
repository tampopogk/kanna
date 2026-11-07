use super::{usage, Command, Flow};
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "projects",
    group: "Information",
    summary: "List your recent projects, most recent first, as File → Open Recent and a new Desk's planning terminal show them.",
    usage: "kanna projects [--json]\n       kanna projects clear",
    options: &[("--json", "Each project's path, its label (its name if it's in ~/.kanna-v4/repos or ~/.kanna/repos, else its path from ~), and when a Desk last opened on it or moved to it.")],
    notes: "A project is its repo's folder (the main repo for a worktree). One moves to the top whenever a Desk opens on it or moves to it: kanna PATH, Open Recent, a planning terminal's folder, a task pulled onto the Desk. Ten are kept, and a folder that's gone drops off. Each is printed as an absolute path on a line of its own. clear empties the list, as Open Recent's Clear Menu does.",
    examples: &[("kanna projects", "Your recent projects, latest first."), ("kanna projects --json", "Each one's path and when it was last opened, as JSON."), ("kanna projects clear", "Forget them.")],
    parse: |args| match args.positional().as_deref() {
        None => Ok(json!({ "action": "list" })),
        Some("clear") => Ok(json!({ "action": "clear" })),
        Some(other) => Err(usage("projects", &format!("unknown action \"{other}\" (clear, or none to list them)"))),
    },
    show,
    flow: Flow::PLAIN,
};

pub fn show(request: &Value, reply: &Value) -> String {
    if request["action"] == "clear" {
        return "cleared the recent projects".into();
    }
    reply["projects"].as_array().map_or(vec![], |list| list.iter().filter_map(|p| p["path"].as_str()).collect::<Vec<_>>()).join("\n")
}
