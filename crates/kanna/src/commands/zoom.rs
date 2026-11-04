use super::{pane, Command, Flow};
use crate::output::{hidden, tagged, whose};
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "zoom",
    group: "Panes",
    summary: "Toggle zoom: the pane fills its tab until you zoom again or focus another pane.",
    usage: "kanna zoom PANE",
    options: &[],
    notes: "The pane doesn't need to be focused first: zooming focuses it, switching tab or window if needed.",
    examples: &[("kanna zoom logs", "Zoom the logs pane (run again to unzoom).")],
    parse: |args| Ok(json!({ "pane": pane(args, "zoom")? })),
    show,
    flow: Flow::PLAIN,
};

pub fn show(args: &Value, reply: &Value) -> String {
    let pane = args["pane"].as_str().unwrap_or("pane");
    let zoomed = reply["zoomed"] == serde_json::json!(true);
    match (whose(reply), zoomed) {
        (Some((name, tags)), true) if hidden(reply) => format!("{pane} is zoomed in {name} ({}; run again to unzoom)", tags.join(", ")),
        (Some((name, tags)), true) => format!("{pane} is zoomed in {} (run again to unzoom)", tagged(&name, &tags)),
        (Some((name, tags)), false) => format!("{pane} is no longer zoomed in {}", tagged(&name, &tags)),
        (None, true) => format!("{pane} is zoomed (run again to unzoom)"),
        (None, false) => format!("{pane} is no longer zoomed"),
    }
}
