use super::{usage, Command, Flow};
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "rename-tab",
    group: "Tabs and windows",
    summary: "Rename a tab. (In the app: double-click the tab.)",
    usage: "kanna rename-tab TAB TITLE...",
    options: &[],
    notes: "TAB is a tab ID such as tab-1 (see `kanna workspace list --all`). Everything after TAB is the title.",
    examples: &[("kanna rename-tab tab-1 servers", "Call tab-1 \"servers\".")],
    parse: |args| {
        let tab = args.positional().ok_or_else(|| usage("rename-tab", "missing TAB (an ID like tab-1; see `kanna workspace list --all`)"))?;
        let title = args.rest().ok_or_else(|| usage("rename-tab", "missing TITLE"))?;
        Ok(json!({ "tab": tab, "title": title }))
    },
    show,
    flow: Flow::PLAIN,
};

pub fn show(_: &Value, reply: &Value) -> String {
    format!("{} is now \"{}\"", reply["tab"].as_str().unwrap_or("?"), reply["title"].as_str().unwrap_or(""))
}
