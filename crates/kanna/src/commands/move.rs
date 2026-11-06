use super::{is_edge, pane, usage, Command, Flow};
use crate::output;
use serde_json::{json, Map, Value};

pub const COMMAND: Command = Command {
    name: "move",
    group: "Panes",
    summary: "Move a pane next to another pane, swap two panes, move a pane to another tab or window, or dock it.",
    usage: "kanna move PANE --to OTHER [--side left|right|top|bottom|swap]\n       kanna move PANE --tab [TAB|new] [--window WINDOW]\n       kanna move PANE --window WINDOW|new\n       kanna move PANE --dock [left|right] [--window WINDOW|new]\n       kanna move PANE --background",
    options: &[
        ("--to OTHER", "The pane to move next to."),
        ("--side SIDE", "left, right, top or bottom of OTHER (takes half its space), or swap (default)."),
        ("--tab [TAB|new]", "Move into that tab, or a new tab (the default). Undocks a docked pane."),
        ("--window WINDOW|new", "Move into a new tab of that window (as --tab new --window), or a new window."),
        ("--dock [left|right]", "Make it its window's docked pane (or that window's), on the left (default) or right edge; or move the docked pane to that edge. A pane already docked there moves to a new tab."),
        ("--background", "Put it in its task's workspace's background: still the task's and running, but in no tab and never drawn."),
    ],
    notes: "Moving a docked pane to a tab, a window or beside a pane undocks it. A docked pane can't be split: --to it only swaps. A frozen docked pane doesn't move, and nothing moves onto it or into its dock. A docked pane is a running task, so --dock moves only the pane that's docked already: start a task with `kanna task start`. A task's agent is frozen. A run's stages run in its task's background; `kanna move PANE --tab` shows one, and --background puts it back.",
    examples: &[
        ("kanna move logs --to server --side bottom", "Put logs below server."),
        ("kanna move logs --window new", "Move logs into a new window."),
        ("kanna move agent --dock right", "Dock agent on the right edge (or move it there if it is docked)."),
        ("kanna move agent --tab", "Undock agent into a new tab."),
    ],
    parse: |args| {
        let pane = pane(args, "move")?;
        let mut out = Map::new();
        out.insert("pane".into(), json!(pane));
        for (flag, key) in [("--to", "to"), ("--side", "side"), ("--window", "window")] {
            if let Some(value) = args.option(flag)? {
                out.insert(key.into(), json!(value));
            }
        }
        if let Some(tab) = args.optional("--tab", |v| !v.starts_with("--")) {
            out.insert("tab".into(), json!(tab.unwrap_or_else(|| "new".into())));
        }
        if let Some(edge) = args.optional("--dock", is_edge) {
            if out.contains_key("to") || out.contains_key("tab") {
                return Err(usage("move", "--dock goes on its own (with --window at most)"));
            }
            out.insert("dock".into(), json!(edge.unwrap_or_else(|| "left".into())));
        }
        if args.flag("--background") {
            if out.len() > 1 {
                return Err(usage("move", "--background goes on its own"));
            }
            out.insert("background".into(), json!(true));
        }
        if !["to", "tab", "window", "dock", "background"].iter().any(|k| out.contains_key(*k)) {
            return Err(usage("move", "say where: --to OTHER, --tab [TAB|new], --window WINDOW|new or --dock [left|right]"));
        }
        Ok(Value::Object(out))
    },
    show: |_, reply| output::moved(reply),
    flow: Flow::PLAIN,
};
