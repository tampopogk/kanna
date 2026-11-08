use super::{target, Command, Flow, PANE};
use crate::output::join;
use serde_json::Value;

pub const COMMAND: Command = Command {
    name: "close",
    group: "Panes",
    summary: "Close a pane, or a whole tab or window with everything in it.",
    usage: "kanna close PANE\n       kanna close --tab TAB\n       kanna close --window WINDOW",
    options: &[("--tab TAB", "Close a tab, e.g. tab-2."), ("--window WINDOW", "Close a window, e.g. win-2.")],
    notes: "Closing a tab's last pane closes the tab; closing a window's last tab closes the window, unless it has a docked pane. A docked pane closes only with its window (`kanna close --window`); undock it first with `kanna move PANE --tab` to close it alone.",
    examples: &[("kanna close logs", "Close the pane named logs."), ("kanna close --tab tab-2", "Close tab-2 and its panes.")],
    parse: |args| target(args, "close"),
    show,
    flow: Flow { panes: PANE, ..Flow::PLAIN },
};

pub fn show(args: &Value, reply: &Value) -> String {
    let panes = join(&reply["closed"]);
    match (args["window"].as_str(), args["tab"].as_str()) {
        (Some(window), _) => format!("closed {window} (panes {panes})"),
        (_, Some(tab)) => format!("closed {tab} (panes {panes})"),
        _ => format!("closed {panes}"),
    }
}
