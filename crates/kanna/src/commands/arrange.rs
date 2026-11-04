use super::{from_caller, usage, Command, Flow};
use crate::output::self;
use serde_json::{json, Map, Value};

pub const COMMAND: Command = Command {
    name: "arrange",
    group: "Tabs and windows",
    summary: "Lay out existing panes in one go. Panes left out move to a new tab.",
    usage: "kanna arrange LAYOUT [--window WINDOW]",
    options: &[("--window WINDOW", "Arrange in that window's active tab. Default: the window of the terminal you run it in.")],
    notes: "Arranges the shown tab; a docked pane isn't part of it. LAYOUT is a JSON tree: {\"split\": \"row\"|\"column\", \"children\": [...]} where each child is {\"pane\": NAME_OR_ID, \"size\": FRACTION?} or another split. FRACTION is a string like \"2/3\" or \"25%\", or a number like 0.25. Unsized children share what is left. row = side by side, column = stacked.",
    examples: &[(
        "kanna arrange '{\"split\":\"row\",\"children\":[{\"pane\":\"server\",\"size\":\"2/3\"},{\"pane\":\"logs\"}]}'",
        "server on the left two thirds, logs on the right.",
    )],
    parse: |args| {
        let window = args.option("--window")?;
        let text = args.positional().ok_or_else(|| usage("arrange", "missing LAYOUT"))?;
        let layout: Value = serde_json::from_str(&text).map_err(|e| usage("arrange", &format!("LAYOUT is not valid JSON: {e}")))?;
        let mut out = Map::new();
        out.insert("layout".into(), layout);
        match window {
            Some(window) => {
                out.insert("window".into(), json!(window));
            }
            None => from_caller(&mut out),
        }
        Ok(Value::Object(out))
    },
    show: |_, reply| output::layout(reply),
    flow: Flow { unplaced: Some(unplaced), ..Flow::PLAIN },
};

fn unplaced(request: &Value) -> Option<&'static str> {
    (request["window"].is_null() && request["from"].is_null()).then_some("add --window WINDOW")
}
