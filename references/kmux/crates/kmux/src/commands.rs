//! Every kmux command: how it is documented, how its arguments become a
//! control-protocol request (docs/kmux-spec.md §7), and how its reply prints.
//! Help, `kmux commands --json` and parsing all come from this table.

use crate::output;
use kmux_client::{exit, fail, Args, Failure};
use serde_json::{json, Map, Value};

/// A CLI command. Each sends the protocol command of the same name.
pub struct Command {
    pub name: &'static str,
    pub group: &'static str,
    pub summary: &'static str,
    pub usage: &'static str,
    pub options: &'static [(&'static str, &'static str)],
    pub notes: &'static str,
    pub examples: &'static [(&'static str, &'static str)],
    /// Turns the command's arguments into a protocol request's `args`.
    pub parse: fn(&mut Args) -> Result<Value, Failure>,
    /// Prints a successful reply for people (`--json` prints it as is),
    /// given the request's args and the reply.
    pub show: fn(&Value, &Value) -> String,
}

pub const GROUPS: &[&str] = &["Panes", "Tabs and windows", "Information"];

pub static COMMANDS: &[Command] = &[
    Command {
        name: "open",
        group: "Panes",
        summary: "Open a terminal, web, iOS or markdown pane and wait until it is running.",
        usage: "kmux open [term] [--cmd CMD] [--cwd DIR] [--name NAME] [--split right|down|auto] [--size FRACTION] [--tab] [--window ID|new] [--dock [left|right] [--frozen]] [--no-wait]\n       kmux open web URL [--history] [same placement options]\n       kmux open md FILE [same placement options]\n       kmux open ios --app APP [--device DEVICE] [same placement options]",
        options: &[
            ("--cmd CMD", "Shell command line to run in your login shell, e.g. \"npm run dev\". Put it in single quotes if it uses $VARS or ;, so your own shell leaves it alone. Default: an interactive shell."),
            ("--cwd DIR", "Working directory."),
            ("--name NAME", "Optional. A name to refer to the pane by in later commands; without one, use the ID that open prints (e.g. term-3)."),
            ("--split right|down|auto", "Where the new pane goes relative to the focused pane. auto (default) splits along the longer side."),
            ("--size FRACTION", "Share of the focused pane's space the new pane takes: 1/3, 25% or 0.25. Default 1/2."),
            ("--tab", "Open in a new tab instead of splitting."),
            ("--window ID|new", "Open in that window, or a new one. Default: the key (front) window."),
            ("--no-wait", "Return as soon as the pane exists instead of waiting until it is running."),
            ("--app APP", "iOS panes: a .app built for the simulator (installed, then launched) or the bundle ID of an installed app, e.g. com.apple.Preferences."),
            ("--device DEVICE", "iOS panes: a simulator device name or UDID, e.g. \"iPhone 16\". Default: a booted iPhone, else the newest one. The device is booted if needed (this can take a minute) and stays booted after the pane closes."),
            ("--history", "Web panes only: keep back/forward history (off by default), for `kmux navigate PANE --back` and the Pane menu's Back and Forward."),
            ("--dock [left|right]", "Make the pane the window's docked pane: along its left (default) or right edge, outside its tabs, shown whichever tab is. Not with --split, --size or --tab. A pane already docked there moves to a new tab."),
            ("--frozen", "With --dock: the docked pane is frozen. It doesn't move, and nothing drops on it or docks in its place; it closes with its window."),
        ],
        notes: "Prints the new pane's ID and name. A pane whose command exits stays on screen as `exited`. A docked pane closes only with its window.",
        examples: &[
            ("kmux open --name server --cmd \"npm run dev\"", "Run a dev server in a pane named server."),
            ("kmux open --name logs --split right --size 1/3 --cmd \"tail -f app.log\"", "Put a log tail in the right third."),
            ("kmux open --tab", "Open a shell in a new tab."),
            ("kmux open --name build --split down --size 1/4 --cmd 'for i in 1 2 3; do echo step $i; done'", "A quarter-height pane below, running a loop."),
            ("kmux open --window new --name scratch", "Open a shell in a new window."),
            ("kmux open ios --name phone --app build/MyApp.app --split right --size 1/3", "Run your app in the simulator, in the right third."),
            ("kmux open md docs/spec.md --name spec --split right", "Show a markdown file (with mermaid diagrams) beside this pane; it reloads when the file changes."),
            ("kmux open --name agent --cmd claude --dock", "Dock an agent on the window's left edge; it stays while you switch tabs."),
        ],
        parse: parse_open,
        show: output::opened,
    },
    Command {
        name: "close",
        group: "Panes",
        summary: "Close a pane, or a whole tab or window with everything in it.",
        usage: "kmux close PANE\n       kmux close --tab TAB\n       kmux close --window WINDOW",
        options: &[("--tab TAB", "Close a tab, e.g. tab-2."), ("--window WINDOW", "Close a window, e.g. win-2.")],
        notes: "Closing a tab's last pane closes the tab; closing a window's last tab closes the window, unless it has a docked pane. A docked pane closes only with its window (`kmux close --window`); undock it first with `kmux move PANE --tab` to close it alone.",
        examples: &[("kmux close logs", "Close the pane named logs."), ("kmux close --tab tab-2", "Close tab-2 and its panes.")],
        parse: |args| target(args, "close"),
        show: output::closed,
    },
    Command {
        name: "focus",
        group: "Panes",
        summary: "Bring a pane, tab or window to the front and give it the keyboard.",
        usage: "kmux focus PANE\n       kmux focus --tab TAB\n       kmux focus --window WINDOW",
        options: &[("--tab TAB", "Focus a tab."), ("--window WINDOW", "Focus a window.")],
        notes: "Focusing a pane in another tab or window switches to it. Focusing another pane cancels zoom.",
        examples: &[("kmux focus server", "Focus the pane named server."), ("kmux focus --tab tab-1", "Switch to tab-1.")],
        parse: |args| target(args, "focus"),
        show: |args, reply| match (args["pane"].as_str(), args["tab"].as_str()) {
            (Some(pane), _) => format!("focused {pane} (key window {})", reply["window"].as_str().unwrap_or("?")),
            (_, Some(tab)) => format!("switched to {tab} (key window {})", reply["window"].as_str().unwrap_or("?")),
            _ => format!("key window is now {}", reply["window"].as_str().unwrap_or("?")),
        },
    },
    Command {
        name: "zoom",
        group: "Panes",
        summary: "Toggle zoom: the pane fills its tab until you zoom again or focus another pane.",
        usage: "kmux zoom PANE",
        options: &[],
        notes: "The pane doesn't need to be focused first: zooming focuses it, switching tab or window if needed.",
        examples: &[("kmux zoom logs", "Zoom the logs pane (run again to unzoom).")],
        parse: |args| Ok(json!({ "pane": pane(args, "zoom")? })),
        show: |args, reply| {
            let pane = args["pane"].as_str().unwrap_or("pane");
            if reply["zoomed"] == json!(true) { format!("{pane} is zoomed (run again to unzoom)") } else { format!("{pane} is no longer zoomed") }
        },
    },
    Command {
        name: "restart",
        group: "Panes",
        summary: "Restart what runs in a pane, and wait until it is running again.",
        usage: "kmux restart PANE",
        options: &[],
        notes: "",
        examples: &[("kmux restart server", "Restart the server pane's command.")],
        parse: |args| Ok(json!({ "pane": pane(args, "restart")? })),
        show: |_, reply| output::pane_state(reply),
    },
    Command {
        name: "send",
        group: "Panes",
        summary: "Type a line of text into a terminal pane and press Return.",
        usage: "kmux send PANE TEXT...",
        options: &[],
        notes: "Everything after PANE is the text. Quote it to keep your shell from interpreting it.",
        examples: &[("kmux send server \"npm test\"", "Run npm test in the server pane.")],
        parse: |args| {
            let pane = pane(args, "send")?;
            let text = args.rest().ok_or_else(|| usage("send", "missing TEXT"))?;
            Ok(json!({ "pane": pane, "text": text }))
        },
        show: |_, _| String::new(),
    },
    Command {
        name: "resize",
        group: "Panes",
        summary: "Change a pane's share of its split. Its siblings share the rest in proportion.",
        usage: "kmux resize PANE FRACTION",
        options: &[],
        notes: "FRACTION is 1/3, 25% or 0.25. Fails if the pane fills its tab. On a docked pane, sets its width, a fraction of the window.",
        examples: &[("kmux resize logs 1/4", "Make logs a quarter of its split.")],
        parse: |args| {
            let pane = pane(args, "resize")?;
            let size = args.positional().ok_or_else(|| usage("resize", "missing FRACTION"))?;
            Ok(json!({ "pane": pane, "size": size }))
        },
        show: |_, reply| output::moved(reply),
    },
    Command {
        name: "move",
        group: "Panes",
        summary: "Move a pane next to another pane, swap two panes, move a pane to another tab or window, or dock it.",
        usage: "kmux move PANE --to OTHER [--side left|right|top|bottom|swap]\n       kmux move PANE --tab [TAB|new] [--window WINDOW]\n       kmux move PANE --window WINDOW|new\n       kmux move PANE --dock [left|right] [--window WINDOW|new]",
        options: &[
            ("--to OTHER", "The pane to move next to."),
            ("--side SIDE", "left, right, top or bottom of OTHER (takes half its space), or swap (default)."),
            ("--tab [TAB|new]", "Move into that tab, or a new tab (the default). Undocks a docked pane."),
            ("--window WINDOW|new", "Move into that window's active tab, or a new window."),
            ("--dock [left|right]", "Make it its window's docked pane (or that window's), on the left (default) or right edge; or move the docked pane to that edge. A pane already docked there moves to a new tab."),
        ],
        notes: "Moving a docked pane to a tab, a window or beside a pane undocks it. A docked pane can't be split: --to it only swaps. A frozen docked pane (open --dock --frozen) doesn't move, and nothing moves onto it or into its dock.",
        examples: &[
            ("kmux move logs --to server --side bottom", "Put logs below server."),
            ("kmux move logs --window new", "Move logs into a new window."),
            ("kmux move agent --dock right", "Dock agent on the right edge (or move it there if it is docked)."),
            ("kmux move agent --tab", "Undock agent into a new tab."),
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
            if !["to", "tab", "window", "dock"].iter().any(|k| out.contains_key(*k)) {
                return Err(usage("move", "say where: --to OTHER, --tab [TAB|new], --window WINDOW|new or --dock [left|right]"));
            }
            Ok(Value::Object(out))
        },
        show: |_, reply| output::moved(reply),
    },
    Command {
        name: "arrange",
        group: "Tabs and windows",
        summary: "Lay out existing panes in one go. Panes left out move to a new tab.",
        usage: "kmux arrange LAYOUT [--window WINDOW]",
        options: &[("--window WINDOW", "Arrange in that window's active tab. Default: the key window.")],
        notes: "Arranges the shown tab; a docked pane isn't part of it. LAYOUT is a JSON tree: {\"split\": \"row\"|\"column\", \"children\": [...]} where each child is {\"pane\": NAME_OR_ID, \"size\": FRACTION?} or another split. FRACTION is a string like \"2/3\" or \"25%\", or a number like 0.25. Unsized children share what is left. row = side by side, column = stacked.",
        examples: &[(
            "kmux arrange '{\"split\":\"row\",\"children\":[{\"pane\":\"server\",\"size\":\"2/3\"},{\"pane\":\"logs\"}]}'",
            "server on the left two thirds, logs on the right.",
        )],
        parse: |args| {
            let window = args.option("--window")?;
            let text = args.positional().ok_or_else(|| usage("arrange", "missing LAYOUT"))?;
            let layout: Value = serde_json::from_str(&text).map_err(|e| usage("arrange", &format!("LAYOUT is not valid JSON: {e}")))?;
            let mut out = json!({ "layout": layout });
            if let Some(window) = window {
                out["window"] = json!(window);
            }
            Ok(out)
        },
        show: |_, reply| output::layout(reply),
    },
    Command {
        name: "rename-tab",
        group: "Tabs and windows",
        summary: "Rename a tab. (In the app: double-click the tab.)",
        usage: "kmux rename-tab TAB TITLE...",
        options: &[],
        notes: "TAB is a tab ID such as tab-1 (see `kmux workspace list --all`). Everything after TAB is the title.",
        examples: &[("kmux rename-tab tab-1 servers", "Call tab-1 \"servers\".")],
        parse: |args| {
            let tab = args.positional().ok_or_else(|| usage("rename-tab", "missing TAB (an ID like tab-1; see `kmux workspace list --all`)"))?;
            let title = args.rest().ok_or_else(|| usage("rename-tab", "missing TITLE"))?;
            Ok(json!({ "tab": tab, "title": title }))
        },
        show: |_, reply| format!("{} is now \"{}\"", reply["tab"].as_str().unwrap_or("?"), reply["title"].as_str().unwrap_or("")),
    },
    Command {
        name: "move-tab",
        group: "Tabs and windows",
        summary: "Reorder a tab, or move it to another window or a new one.",
        usage: "kmux move-tab TAB [--window WINDOW|new] [--index N]",
        options: &[("--window WINDOW|new", "Destination window. Default: the tab's own window."), ("--index N", "Position in the tab bar, from 0. Default: last.")],
        notes: "",
        examples: &[("kmux move-tab tab-2 --window new", "Move tab-2 into a new window."), ("kmux move-tab tab-3 --index 0", "Make tab-3 the first tab.")],
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
        show: |_, reply| format!("{}: {}", reply["window"].as_str().unwrap_or("?"), output::join(&reply["tabs"])),
    },
    Command {
        name: "workspace",
        group: "Tabs and windows",
        summary: "Make, hide, show, close or list workspaces: named groups of windows, shown or hidden together.",
        usage: "kmux workspace new [NAME]\n       kmux workspace hide WORKSPACE\n       kmux workspace show WORKSPACE\n       kmux workspace close WORKSPACE\n       kmux workspace list [--all]",
        options: &[("--all", "Every window, workspace and pane, not just your own.")],
        notes: "WORKSPACE is a workspace's ID (ws-1) or its name. A new workspace has no windows; put some in with `kmux window assign`. Hidden, its windows close, their panes still running, and nothing on them takes the keyboard (`kmux focus` refuses); shown, they open again where they were. Closing a workspace closes its windows and everything in them.\nlist shows your own: the window your terminal ($KMUX_TERMINAL) is in, or its workspace with all its windows, and their panes. With --all: free windows first, then each workspace with its windows: `workspace ws-1 (checkout)`, with `hidden` after a hidden one. Layouts are written as `a:2/3 | b` (side by side) and `a / b` (stacked). `*` marks the key window, the active tab and the focused pane. A window's docked pane has its own line: `dock: term-1 (agent)  left 2/5`, with `frozen` after a frozen one.",
        examples: &[
            ("kmux workspace new checkout", "Make a workspace named checkout."),
            ("kmux workspace hide checkout", "Put its windows away; what runs in them keeps running."),
            ("kmux workspace show checkout", "Bring them back where they were."),
            ("kmux workspace list", "See your own window, or your workspace's."),
            ("kmux workspace list --all", "See everything that is open."),
            ("kmux workspace list --all --json", "The same, as JSON."),
        ],
        parse: parse_workspace,
        show: output::workspace,
    },
    Command {
        name: "window",
        group: "Tabs and windows",
        summary: "Put a window in a workspace, so it shows and hides with it, or free it.",
        usage: "kmux window assign WINDOW WORKSPACE\n       kmux window free WINDOW",
        options: &[],
        notes: "A window is in one workspace, or none (a free window). Put in a hidden workspace, it closes with the workspace's other windows; freed, it's on screen again. In the app: Window → Assign Window to Workspace and Free Window.",
        examples: &[("kmux window assign win-2 checkout", "Make win-2 one of checkout's windows."), ("kmux window free win-2", "Take win-2 out of its workspace; it stays open.")],
        parse: parse_window,
        show: output::window,
    },
    Command {
        name: "navigate",
        group: "Panes",
        summary: "Point a web pane at a new URL (or go back or forward), or a markdown pane at another file.",
        usage: "kmux navigate PANE URL\n       kmux navigate PANE --back|--forward\n       kmux navigate PANE FILE.md",
        options: &[
            ("--back", "Go back a page: in a markdown pane, or a web pane opened with `open web URL --history`."),
            ("--forward", "Go forward a page (after --back)."),
        ],
        notes: "Markdown panes always keep history of the files they showed. Web panes keep none unless opened with --history.",
        examples: &[
            ("kmux navigate site localhost:5173", "Show localhost:5173 in the site pane."),
            ("kmux navigate site --back", "Go back to the previous page (the pane was opened with --history)."),
            ("kmux navigate spec docs/roadmap.md", "Show another markdown file in the spec pane."),
        ],
        parse: |args| {
            let pane = pane(args, "navigate")?;
            let (back, forward) = (args.flag("--back"), args.flag("--forward"));
            match (back, forward, args.positional()) {
                (true, false, None) => Ok(json!({ "pane": pane, "back": true })),
                (false, true, None) => Ok(json!({ "pane": pane, "forward": true })),
                (false, false, Some(target)) if is_markdown(&target) => Ok(json!({ "pane": pane, "path": absolute(&target) })),
                (false, false, Some(url)) => Ok(json!({ "pane": pane, "url": url })),
                (false, false, None) => Err(usage("navigate", "missing URL (or --back / --forward)")),
                _ => Err(usage("navigate", "give one of URL, --back or --forward")),
            }
        },
        show: |_, reply| output::pane_state(reply),
    },
    Command {
        name: "capabilities",
        group: "Information",
        summary: "Show what the running kmux supports: pane types and commands.",
        usage: "kmux capabilities",
        options: &[],
        notes: "",
        examples: &[("kmux capabilities", "See which commands this kmux supports.")],
        parse: |_| Ok(json!({})),
        show: |_, reply| format!("pane types: {}\ncommands: {}", output::join(&reply["paneTypes"]), output::join(&reply["commands"])),
    },
];

pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name)
}

/// The closest command names, for "did you mean".
pub fn similar(name: &str) -> Vec<&'static str> {
    let mut names: Vec<(usize, &str)> =
        COMMANDS.iter().map(|c| (distance(name, c.name), c.name)).filter(|(d, n)| *d <= 2 || n.starts_with(name) || name.starts_with(n)).collect();
    names.sort();
    names.into_iter().map(|(_, n)| n).take(3).collect()
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for j in 0..b.len() {
            let next = (row[j + 1] + 1).min(row[j] + 1).min(prev + usize::from(ca != b[j]));
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

pub fn usage(command: &str, problem: &str) -> Failure {
    let usage = find(command).map(|c| c.usage).unwrap_or("");
    fail(exit::USAGE, format!("{command}: {problem}\nusage: {usage}\nmore: kmux help {command}"))
}

fn pane(args: &mut Args, command: &str) -> Result<String, Failure> {
    args.positional().ok_or_else(|| usage(command, "missing PANE (a pane name, or an ID like term-1; see `kmux workspace list --all`)"))
}

/// PANE, --tab TAB or --window WINDOW.
fn target(args: &mut Args, command: &str) -> Result<Value, Failure> {
    if let Some(tab) = args.option("--tab")? {
        return Ok(json!({ "tab": tab }));
    }
    if let Some(window) = args.option("--window")? {
        return Ok(json!({ "window": window }));
    }
    match args.positional() {
        Some(pane) => Ok(json!({ "pane": pane })),
        None => Err(usage(command, "say what: PANE, --tab TAB or --window WINDOW")),
    }
}

fn is_edge(value: &str) -> bool {
    value == "left" || value == "right"
}

fn is_markdown(target: &str) -> bool {
    let lower = target.to_lowercase();
    [".md", ".markdown", ".mdown"].iter().any(|ext| lower.ends_with(ext)) && !lower.contains("://")
}

/// A file path as kmux needs it: absolute, relative ones taken from here.
fn absolute(path: &str) -> String {
    let path = std::path::Path::new(path);
    if path.is_absolute() || path.starts_with("~") {
        return path.to_string_lossy().into();
    }
    std::env::current_dir().map(|dir| dir.join(path)).ok().and_then(|p| std::fs::canonicalize(&p).ok().or(Some(p)))
        .map(|p| p.to_string_lossy().into()).unwrap_or_else(|| path.to_string_lossy().into())
}

fn parse_workspace(args: &mut Args) -> Result<Value, Failure> {
    let action = args.positional().ok_or_else(|| usage("workspace", "say new, hide, show, close or list"))?;
    match action.as_str() {
        "list" if args.flag("--all") => Ok(json!({ "action": "list", "all": true })),
        "list" => Ok(match std::env::var("KMUX_TERMINAL").ok().filter(|p| !p.is_empty()) {
            Some(pane) => json!({ "action": "list", "pane": pane }),
            None => json!({ "action": "list" }),
        }),
        "new" => Ok(match args.positional() {
            Some(name) => json!({ "action": "new", "name": name }),
            None => json!({ "action": "new" }),
        }),
        "hide" | "show" | "close" => {
            let workspace = args.positional().ok_or_else(|| usage("workspace", "missing WORKSPACE (an ID like ws-1, or its name; see `kmux workspace list --all`)"))?;
            Ok(json!({ "action": action, "workspace": workspace }))
        }
        other => Err(usage("workspace", &format!("unknown action \"{other}\": new, hide, show, close or list"))),
    }
}

fn parse_window(args: &mut Args) -> Result<Value, Failure> {
    let action = args.positional().ok_or_else(|| usage("window", "say assign or free"))?;
    let window = args.positional().ok_or_else(|| usage("window", "missing WINDOW (an ID like win-2; see `kmux workspace list --all`)"))?;
    match action.as_str() {
        "assign" => {
            let workspace = args.positional().ok_or_else(|| usage("window", "missing WORKSPACE (an ID like ws-1, or its name; see `kmux workspace list --all`)"))?;
            Ok(json!({ "action": "assign", "window": window, "workspace": workspace }))
        }
        "free" => Ok(json!({ "action": "free", "window": window })),
        other => Err(usage("window", &format!("unknown action \"{other}\": assign or free"))),
    }
}

fn parse_open(args: &mut Args) -> Result<Value, Failure> {
    let mut out = Map::new();
    for (flag, key) in [("--cmd", "cmd"), ("--cwd", "cwd"), ("--name", "name"), ("--split", "split"), ("--size", "size"), ("--window", "window"), ("--app", "app"), ("--device", "device")] {
        if let Some(value) = args.option(flag)? {
            out.insert(key.into(), json!(value));
        }
    }
    // A relative .app path means one relative to here, not to kmux.
    if let Some(Value::String(app)) = out.get("app") {
        let path = std::path::Path::new(app);
        if path.is_relative() && path.exists() {
            if let Ok(full) = std::fs::canonicalize(path) {
                out.insert("app".into(), json!(full.to_string_lossy()));
            }
        }
    }
    if let Some(edge) = args.optional("--dock", is_edge) {
        out.insert("dock".into(), json!(edge.unwrap_or_else(|| "left".into())));
    }
    if args.flag("--tab") {
        out.insert("tab".into(), json!(true));
    }
    if out.contains_key("dock") && ["split", "size", "tab"].iter().any(|k| out.contains_key(*k)) {
        return Err(usage("open", "--dock goes on its own: no --split, --size or --tab"));
    }
    if args.flag("--frozen") {
        if !out.contains_key("dock") {
            return Err(usage("open", "--frozen goes with --dock: only a docked pane is frozen"));
        }
        out.insert("frozen".into(), json!(true));
    }
    if args.flag("--no-wait") {
        out.insert("wait".into(), json!(false));
    }
    let history = args.flag("--history");
    let kind = match args.positional() {
        None => "term".to_string(),
        Some(kind) if ["term", "web", "ios", "md"].contains(&kind.as_str()) => kind,
        Some(other) if is_markdown(&other) => {
            return Err(usage("open", &format!("to show a markdown file use: kmux open md {other}")));
        }
        Some(other) => {
            return Err(usage("open", &format!("unknown pane type \"{other}\" (term, web, ios or md). To run a command use --cmd \"{other}\"")));
        }
    };
    if kind == "web" {
        let url = args.positional().ok_or_else(|| usage("open", "web panes need a URL: kmux open web URL"))?;
        out.insert("url".into(), json!(url));
    }
    if kind == "md" {
        let file = args.positional().ok_or_else(|| usage("open", "markdown panes need a file: kmux open md FILE"))?;
        out.insert("path".into(), json!(absolute(&file)));
    }
    if kind == "ios" && !out.contains_key("app") {
        return Err(usage("open", "iOS panes need an app: kmux open ios --app MyApp.app (or a bundle ID)"));
    }
    if kind != "ios" && (out.contains_key("app") || out.contains_key("device")) {
        return Err(usage("open", "--app and --device are only for iOS panes: kmux open ios --app APP"));
    }
    if history {
        if kind != "web" {
            return Err(usage("open", "--history is only for web panes: kmux open web URL --history"));
        }
        out.insert("history".into(), json!(true));
    }
    out.insert("type".into(), json!(kind));
    Ok(Value::Object(out))
}
