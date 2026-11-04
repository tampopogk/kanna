use super::{Command, Flow};
use crate::output::join;
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "capabilities",
    group: "Information",
    summary: "Show what the running Kanna supports: pane types and commands.",
    usage: "kanna capabilities",
    options: &[],
    notes: "",
    examples: &[("kanna capabilities", "See which commands this Kanna supports.")],
    parse: |_| Ok(json!({})),
    show: |_, reply| capabilities(reply),
    flow: Flow::PLAIN,
};

pub fn capabilities(reply: &Value) -> String {
    let mut out = format!("pane types: {}\ncommands: {}", join(&reply["paneTypes"]), join(&reply["commands"]));
    if let Some(app) = reply["app"].as_bool() {
        out += if app {
            "\nKanna.app: attached"
        } else {
            "\nKanna.app: not attached (terminals and the layout work without it; the CLI starts it for commands that need it)"
        };
    }
    out
}
