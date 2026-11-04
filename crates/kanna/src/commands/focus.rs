use super::{target, Command, Flow};
use crate::output::{tagged, whose};
use serde_json::Value;

pub const COMMAND: Command = Command {
    name: "focus",
    group: "Panes",
    summary: "Bring a pane, tab or window to the front and give it the keyboard.",
    usage: "kanna focus PANE\n       kanna focus --tab TAB\n       kanna focus --window WINDOW",
    options: &[("--tab TAB", "Focus a tab."), ("--window WINDOW", "Focus a window.")],
    notes: "Focusing a pane in another tab or window switches to it. Focusing another pane cancels zoom.",
    examples: &[("kanna focus server", "Focus the pane named server."), ("kanna focus --tab tab-1", "Switch to tab-1.")],
    parse: |args| target(args, "focus"),
    show: show,
    flow: Flow::PLAIN,
};

pub fn show(args: &Value, reply: &Value) -> String {
    let window = reply["window"].as_str().or(reply["front"].as_str()).or(args["window"].as_str()).unwrap_or("?");
    let front = !reply["front"].is_null();
    let shown = if reply["behind"] == true { "behind the app you're in" } else { "in front" };
    let place = match (whose(reply), reply["window"].as_str()) {
        (Some((name, mut tags)), _) => {
            if front {
                tags.push(shown.into());
            }
            format!(" in {}", tagged(&name, &tags))
        }
        (None, Some(window)) if front => format!(" in {window} ({shown})"),
        (None, Some(window)) => format!(" in {window}"),
        (None, None) => String::new(),
    };
    match (args["pane"].as_str(), args["tab"].as_str()) {
        (Some(pane), _) => format!("focused {pane}{place}"),
        (_, Some(tab)) => format!("switched to {tab}{place}"),
        _ if front && reply["behind"] == true => format!("put {window} {shown}"),
        _ if front => format!("brought {window} forward"),
        _ => format!("{window} isn't on screen"),
    }
}
