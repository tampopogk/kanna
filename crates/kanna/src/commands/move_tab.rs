use super::{usage, Command, Flow};
use crate::output::{join, whose_text};
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "move-tab",
    group: "Tabs and windows",
    summary: "Reorder a tab, or move it to another window or a new one.",
    usage: "kanna move-tab TAB [--window WINDOW|new] [--index N]",
    options: &[("--window WINDOW|new", "Destination window. Default: the tab's own window."), ("--index N", "Position in the tab bar, from 0. Default: last.")],
    notes: "",
    examples: &[("kanna move-tab tab-2 --window new", "Move tab-2 into a new window."), ("kanna move-tab tab-3 --index 0", "Make tab-3 the first tab.")],
    parse: |args| {
        let tab = args.positional().ok_or_else(|| usage("move-tab", "missing TAB"))?;
        let mut out = json!({ "tab": tab });
        if let Some(window) = args.option("--window")? {
            out["window"] = json!(window);
        }
        if let Some(index) = args.option("--index")? {
            let index: u64 = index.parse().map_err(|_| usage("move-tab", "--index needs a whole number"))?;
            out["index"] = json!(index);
        }
        Ok(out)
    },
    show: show,
    flow: Flow::PLAIN,
};

pub fn show(_: &Value, reply: &Value) -> String {
    format!("{}: {}", whose_text(reply).unwrap_or_else(|| reply["window"].as_str().unwrap_or("?").to_string()), join(&reply["tabs"]))
}
