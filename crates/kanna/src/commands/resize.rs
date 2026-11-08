use super::{pane, usage, Command, Flow, PANE};
use crate::output;
use serde_json::json;

pub const COMMAND: Command = Command {
    name: "resize",
    group: "Panes",
    summary: "Change a pane's share of its split. Its siblings share the rest in proportion.",
    usage: "kanna resize PANE FRACTION",
    options: &[],
    notes: "FRACTION is 1/3, 25% or 0.25. Fails if the pane fills its tab. On a docked pane, sets its width, a fraction of the window.",
    examples: &[("kanna resize logs 1/4", "Make logs a quarter of its split.")],
    parse: |args| {
        let pane = pane(args, "resize")?;
        let size = args.positional().ok_or_else(|| usage("resize", "missing FRACTION"))?;
        Ok(json!({ "pane": pane, "size": size }))
    },
    show: |_, reply| output::moved(reply),
    flow: Flow { panes: PANE, ..Flow::PLAIN },
};
