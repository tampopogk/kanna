use super::{pane, Command, Flow, PANE};
use crate::output;
use serde_json::json;

pub const COMMAND: Command = Command {
    name: "restart",
    group: "Panes",
    summary: "Restart what runs in a pane, and wait until it is running again.",
    usage: "kanna restart PANE",
    options: &[],
    notes: "",
    examples: &[("kanna restart server", "Restart the server pane's command.")],
    parse: |args| Ok(json!({ "pane": pane(args, "restart")? })),
    show: |_, reply| output::pane_state(reply),
    flow: Flow { panes: PANE, ..Flow::PLAIN },
};
