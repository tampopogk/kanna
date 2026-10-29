use crate::args::Argv;
use crate::output;
use kanna_client::{exit, fail, Failure};
use serde_json::{json, Map, Value};

pub struct Command {
    pub name: &'static str,
    pub group: &'static str,
    pub summary: &'static str,
    pub usage: &'static str,
    pub options: &'static [(&'static str, &'static str)],
    pub notes: &'static str,
    pub examples: &'static [(&'static str, &'static str)],
    pub parse: fn(&mut Argv) -> Result<Value, Failure>,
    pub show: fn(&Value, &Value) -> String,
}

pub const GROUPS: &[&str] = &["Panes", "Tabs and windows", "Information"];

pub const LOCAL: &[&str] = &["diagrams"];

pub static COMMANDS: &[Command] = &[
    Command {
        name: "open",
        group: "Panes",
        summary: "Open a terminal, web, iOS, markdown, code or diff pane and wait until it is running.",
        usage: "kanna open [term] [--cmd CMD] [--cwd DIR] [--name NAME] [--split right|down|auto] [--size FRACTION] [--tab] [--window ID|new] [--dock [left|right] [--frozen]] [--no-wait]\n       kanna open web URL [--history] [same placement options]\n       kanna open md FILE [same placement options]\n       kanna open ios --app APP [--device DEVICE] [same placement options]\n       kanna open code FILE[:LINE] [same placement options]\n       kanna open diff [GIT-DIFF-ARGS] [same placement options]\n       kanna open md FILE --diff [REF|A..B] [same placement options]",
        options: &[
            ("--cmd CMD", "Shell command line to run in your login shell, e.g. \"npm run dev\". Put it in single quotes if it uses $VARS or ;, so your own shell leaves it alone. Default: an interactive shell."),
            ("--cwd DIR", "Working directory."),
            ("--name NAME", "Optional. A name to refer to the pane by in later commands; without one, use the ID that open prints (e.g. term-3)."),
            ("--split right|down|auto", "Where the new pane goes relative to the focused pane. auto (default) splits along the longer side."),
            ("--size FRACTION", "Share of the focused pane's space the new pane takes: 1/3, 25% or 0.25. Default 1/2."),
            ("--tab", "Open in a new tab instead of splitting."),
            ("--window ID|new", "Open in that window, or a new one. Default: the window of the terminal you run it in."),
            ("--no-wait", "Return as soon as the pane exists instead of waiting until it is running."),
            ("--app APP", "iOS panes: a .app built for the simulator (installed, then launched) or the bundle ID of an installed app, e.g. com.apple.Preferences."),
            ("--device DEVICE", "iOS panes: a simulator device name or UDID, e.g. \"iPhone 16\". Default: a booted iPhone, else the newest one. The device is booted if needed (this can take a minute) and stays booted after the pane closes."),
            ("--history", "Web panes only: keep back/forward history (off by default), for `kanna navigate PANE --back` and the Pane menu's Back and Forward."),
            ("--dock [left|right]", "Make the pane the window's docked pane: along its left (default) or right edge, outside its tabs, shown whichever tab is. Not with --split, --size or --tab. A pane already docked there moves to a new tab."),
            ("--frozen", "With --dock: the docked pane is frozen. It doesn't move, and nothing drops on it or docks in its place; it closes with its window."),
        ],
        notes: "Prints the new pane's ID and name. A pane whose command exits stays on screen as `exited`. A docked pane closes only with its window.\nA terminal opened without --split, --size, --tab, --window or --dock opens even with Kanna.app closed, and takes its place when the app next opens; anything else starts the app.\nRun from a pane without --window or --dock, it opens in that pane's window, not the key one; from a task kept off-screen (the backlog, the Shelf), in a new tab of the task's window. Run from outside a Kanna terminal, it needs --window (or --dock): there's no window of yours to open in.\n--diff [REF] marks what changed in a markdown doc since REF, inline: added text in green, removed text struck through in red, a bar beside each changed block; ]c and [c jump to the next and previous change. REF is any commit or branch, compared as `git diff REF...` compares: from where HEAD and REF meet, uncommitted changes included. Default: main (or master), so a task's doc shows everything the task changed. --diff A..B shows the doc as it is in commit B, marked against commit A (compared directly, not from where they meet); it's read-only, doesn't follow the file and takes no comments.",
        examples: &[
            ("kanna open --name server --cmd \"npm run dev\"", "Run a dev server in a pane named server."),
            ("kanna open --name logs --split right --size 1/3 --cmd \"tail -f app.log\"", "Put a log tail in the right third."),
            ("kanna open --tab", "Open a shell in a new tab."),
            ("kanna open --name build --split down --size 1/4 --cmd 'for i in 1 2 3; do echo step $i; done'", "A quarter-height pane below, running a loop."),
            ("kanna open --window new --name scratch", "Open a shell in a new window."),
            ("kanna open ios --name phone --app build/MyApp.app --split right --size 1/3", "Run your app in the simulator, in the right third."),
            ("kanna open md docs/spec.md --name spec --split right", "Show a markdown file (with mermaid diagrams) beside this pane; it reloads when the file changes."),
            ("kanna open --name agent --cmd claude --dock", "Dock an agent on the window's left edge; it stays while you switch tabs."),
            ("kanna open md docs/specs/presenting.md --diff main --tab", "Show a spec with what this branch changed in it marked."),
            ("kanna open code src/main.rs:42", "Show a source file, highlighted, at line 42; it follows the file on disk."),
            ("kanna open diff --merge-base main", "Show everything this branch changed since it left main, committed or not, with untracked files; it follows the worktree."),
        ],
        parse: parse_open,
        show: output::opened,
    },
    Command {
        name: "close",
        group: "Panes",
        summary: "Close a pane, or a whole tab or window with everything in it.",
        usage: "kanna close PANE\n       kanna close --tab TAB\n       kanna close --window WINDOW",
        options: &[("--tab TAB", "Close a tab, e.g. tab-2."), ("--window WINDOW", "Close a window, e.g. win-2.")],
        notes: "Closing a tab's last pane closes the tab; closing a window's last tab closes the window, unless it has a docked pane. A docked pane closes only with its window (`kanna close --window`); undock it first with `kanna move PANE --tab` to close it alone.",
        examples: &[("kanna close logs", "Close the pane named logs."), ("kanna close --tab tab-2", "Close tab-2 and its panes.")],
        parse: |args| target(args, "close"),
        show: output::closed,
    },
    Command {
        name: "focus",
        group: "Panes",
        summary: "Bring a pane, tab or window to the front and give it the keyboard.",
        usage: "kanna focus PANE\n       kanna focus --tab TAB\n       kanna focus --window WINDOW",
        options: &[("--tab TAB", "Focus a tab."), ("--window WINDOW", "Focus a window.")],
        notes: "Focusing a pane in another tab or window switches to it. Focusing another pane cancels zoom.",
        examples: &[("kanna focus server", "Focus the pane named server."), ("kanna focus --tab tab-1", "Switch to tab-1.")],
        parse: |args| target(args, "focus"),
        show: output::focused,
    },
    Command {
        name: "zoom",
        group: "Panes",
        summary: "Toggle zoom: the pane fills its tab until you zoom again or focus another pane.",
        usage: "kanna zoom PANE",
        options: &[],
        notes: "The pane doesn't need to be focused first: zooming focuses it, switching tab or window if needed.",
        examples: &[("kanna zoom logs", "Zoom the logs pane (run again to unzoom).")],
        parse: |args| Ok(json!({ "pane": pane(args, "zoom")? })),
        show: output::zoomed,
    },
    Command {
        name: "restart",
        group: "Panes",
        summary: "Restart what runs in a pane, and wait until it is running again.",
        usage: "kanna restart PANE",
        options: &[],
        notes: "",
        examples: &[("kanna restart server", "Restart the server pane's command.")],
        parse: |args| Ok(json!({ "pane": pane(args, "restart")? })),
        show: |_, reply| output::pane_state(reply),
    },
    Command {
        name: "send",
        group: "Panes",
        summary: "Type a line of text into a terminal pane and press Return.",
        usage: "kanna send PANE TEXT...",
        options: &[],
        notes: "Everything after PANE is the text. Quote it to keep your shell from interpreting it.\n-- ends the options: `kanna send term-1 -- --json` types --json.\nReturn is pressed on its own after the text (pasted, if the program asked for bracketed paste), so an agent like Claude Code submits it.",
        examples: &[("kanna send server \"npm test\"", "Run npm test in the server pane.")],
        parse: |args| {
            let pane = pane(args, "send")?;
            let text = args.rest().ok_or_else(|| usage("send", "missing TEXT"))?;
            Ok(json!({ "pane": pane, "text": text }))
        },
        show: |_, _| String::new(),
    },
    Command {
        name: "send-key",
        group: "Panes",
        summary: "Press keys in a terminal pane: answer a menu, interrupt a command, submit a prompt.",
        usage: "kanna send-key PANE KEY...",
        options: &[],
        notes: "Each KEY is a key, with ctrl+, alt+ or shift+ in front if held: return, esc, tab, space, backspace (or delete), forward-delete, up, down, left, right, home, end, pageup, pagedown, insert, f1 to f12, or one character (a, A, ?, 1). Keys go one after the other, each 50 ms after the last (the first too, so it follows kanna send as a key press), as the program in the pane expects them in its current mode. cmd+ keys only mean something to Kanna.app, so they're refused. To type text, use kanna send.\nWorks with Kanna.app closed.",
        examples: &[
            ("kanna send-key agent return", "Submit what's typed in the agent pane's prompt."),
            ("kanna send-key server ctrl+c", "Interrupt the server pane's command."),
            ("kanna send-key agent down down return", "Pick the third choice in the agent's menu."),
        ],
        parse: |args| {
            let pane = pane(args, "send-key")?;
            let keys = args.positionals();
            if keys.is_empty() {
                return Err(usage("send-key", "missing KEY (e.g. return, esc, ctrl+c, up)"));
            }
            Ok(json!({ "pane": pane, "keys": keys }))
        },
        show: |_, _| String::new(),
    },
    Command {
        name: "read",
        group: "Panes",
        summary: "Print a terminal pane's screen as text.",
        usage: "kanna read PANE [--lines N]",
        options: &[("--lines N", "The last N lines instead of the screen, counting scrollback.")],
        notes: "Terminal panes only. Works with Kanna.app closed: the daemon keeps every terminal's screen.",
        examples: &[("kanna read server", "What the server pane shows now."), ("kanna read server --lines 200", "Its last 200 lines, scrollback included.")],
        parse: |args| {
            let lines = match args.option("--lines")? {
                Some(n) => Some(n.parse::<u64>().ok().filter(|n| *n > 0).ok_or_else(|| usage("read", "--lines needs a whole number above 0"))?),
                None => None,
            };
            let mut out = json!({ "pane": pane(args, "read")? });
            if let Some(lines) = lines {
                out["lines"] = json!(lines);
            }
            Ok(out)
        },
        show: |_, reply| reply["text"].as_str().unwrap_or_default().to_string(),
    },
    Command {
        name: "screenshot",
        group: "Panes",
        summary: "Save a PNG of a pane or a window as it is drawn now, and print its path.",
        usage: "kanna screenshot [PANE | --window WINDOW] [-o|--out FILE] [--force]",
        options: &[
            ("--window WINDOW", "The whole window, e.g. win-2: its tab bar, every pane showing and the space between them."),
            ("--out FILE", "Where to save it, relative to this folder. It must not exist yet, unless you add --force. Default: a new file in $TMPDIR."),
            ("-o FILE", "Same as --out."),
            ("--force", "Replace FILE if it exists."),
        ],
        notes: "PANE gives just that pane, cut from its window; it must be showing (not in a tab that is behind another). With neither PANE nor --window: the window of the terminal you run it in, else the key (front) window.\nWorks for every kind of pane (terminal, web, markdown, code, iOS), also when the window is behind other windows. The image is at the screen's resolution: a 2x display gives twice the window's size in points.\nWith --json, the reply also has the window, the pane and the image's width and height in pixels. Starts Kanna.app if it isn't running, and shoots the window it opens with.",
        examples: &[
            ("kanna screenshot", "Save this window and print where."),
            ("kanna screenshot diff -o diff.png", "Save the pane named diff to diff.png."),
            ("kanna screenshot --window win-2 --out shots/win-2.png --force", "Save win-2, replacing shots/win-2.png."),
        ],
        parse: parse_screenshot,
        show: |_, reply| reply["path"].as_str().unwrap_or_default().to_string(),
    },
    Command {
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
    },
    Command {
        name: "move",
        group: "Panes",
        summary: "Move a pane next to another pane, swap two panes, move a pane to another tab or window, or dock it.",
        usage: "kanna move PANE --to OTHER [--side left|right|top|bottom|swap]\n       kanna move PANE --tab [TAB|new] [--window WINDOW]\n       kanna move PANE --window WINDOW|new\n       kanna move PANE --dock [left|right] [--window WINDOW|new]",
        options: &[
            ("--to OTHER", "The pane to move next to."),
            ("--side SIDE", "left, right, top or bottom of OTHER (takes half its space), or swap (default)."),
            ("--tab [TAB|new]", "Move into that tab, or a new tab (the default). Undocks a docked pane."),
            ("--window WINDOW|new", "Move into a new tab of that window (as --tab new --window), or a new window."),
            ("--dock [left|right]", "Make it its window's docked pane (or that window's), on the left (default) or right edge; or move the docked pane to that edge. A pane already docked there moves to a new tab."),
        ],
        notes: "Moving a docked pane to a tab, a window or beside a pane undocks it. A docked pane can't be split: --to it only swaps. A frozen docked pane (open --dock --frozen) doesn't move, and nothing moves onto it or into its dock. A docked pane is a running task, so --dock moves only the pane that's docked already: start a task with `kanna task start`. A task's agent is frozen.",
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
    },
    Command {
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
        show: output::renamed,
    },
    Command {
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
        show: output::tabs,
    },
    Command {
        name: "workspace",
        group: "Tabs and windows",
        summary: "Show your workspace as it will be seen: its dock, tabs and panes, which tab is shown, the focus and the zoom.",
        usage: "kanna workspace list [--all]",
        options: &[("--all", "Every window, workspace and pane, not just your own.")],
        notes: "Your workspace is the one your terminal ($KANNA_TERMINAL) is in, on a Desk, on the Shelf or in the backlog: its ID (ws-3), its task, where it is, its docked pane, the focused pane (and whether it's zoomed), its tabs with `*` on the shown one, and its own windows. From a pane in a window that's no workspace's, it's that window. Layouts are written as `a:2/3 | b` (side by side) and `a / b` (stacked).\nWith --all: every window and what it shows, every workspace, free windows' tabs, and panes in no window. With Kanna.app closed there are no windows, and the terminals, which keep running, are listed alone. --json prints the reply: your own windows, workspace and panes, or with --all all of them.",
        examples: &[
            ("kanna workspace list", "See your workspace as the user will when they pull your task."),
            ("kanna workspace list --all", "See every window, workspace and pane."),
            ("kanna workspace list --all --json", "The same, as JSON."),
        ],
        parse: parse_workspace,
        show: output::workspace,
    },
    Command {
        name: "window",
        group: "Tabs and windows",
        summary: "Make a window one of a task's, so it shows and hides with the task, or free it.",
        usage: "kanna window assign WINDOW TASK\n       kanna window free WINDOW",
        options: &[],
        notes: "A task's windows are on screen while the task is on a Desk; on the Shelf or in the backlog they close, their panes still running, and they open again at their frames when it's pulled. A window opened from a task's pane (`kanna open --window new`) or torn off its tabs is the task's already; one made anywhere else is free. TASK is a task's ID (task-1dfb) or its agent's pane. A Desk, a window with a lead (the Quiz window) and a task that hasn't started are refused. In the app: Window → Assign Window to Task and Free Window.",
        examples: &[("kanna window assign win-4 task-1dfb", "Make win-4 one of task-1dfb's windows."), ("kanna window free win-4", "Take win-4 out of its task; it stays open.")],
        parse: parse_window,
        show: output::window,
    },
    Command {
        name: "rename-pane",
        group: "Panes",
        summary: "Give a pane a name, change it, or clear it.",
        usage: "kanna rename-pane PANE NAME",
        options: &[],
        notes: "PANE is the pane's current name or its ID (see `kanna workspace list --all`). An empty NAME (\"\") clears the name, so the pane goes by its ID. No two panes share a name, and a name can't be another pane's ID.",
        examples: &[("kanna rename-pane term-3 server", "Call term-3 \"server\"."), ("kanna rename-pane server \"\"", "Clear the name; refer to it as term-3 again.")],
        parse: |args| {
            let pane = args.positional().ok_or_else(|| usage("rename-pane", "missing PANE"))?;
            let name = args.positional().ok_or_else(|| usage("rename-pane", "missing NAME (\"\" to clear it)"))?;
            Ok(json!({ "pane": pane, "name": name }))
        },
        show: output::pane_renamed,
    },
    Command {
        name: "task",
        group: "Tabs and windows",
        summary: "Make, edit, list, start, stop, set up, shelve, park, pull, defer, name, message or end tasks: an agent docked with its tabs, on Kanna.app's Shelf or in its repo's backlog.",
        usage: "kanna task new NAME [FIRST-MESSAGE] [--agent CMD] [--after TASK,...]\n       kanna task edit TASK --after TASK,...|none\n       kanna task start TASK\n       kanna task stop [TASK]\n       kanna task setup [TASK]\n       kanna task shelve TASK\n       kanna task list [QUERY]\n       kanna task park [TASK]\n       kanna task pull TASK\n       kanna task defer [TASK]\n       kanna task name [TASK] NAME\n       kanna task send TASK|planner TEXT\n       kanna task messages [TASK]\n       kanna task done [TASK] [--keep]",
        options: &[
            ("--agent CMD", "new: the agent to start (default: claude)."),
            ("--after TASK,...|none", "new, edit: the tasks it waits for; it can't start until they're all done. edit: replaces the list, and none clears it."),
            ("--keep", "done: end it without the repo's tasks.teardown, keeping its worktree and branch."),
        ],
        notes: "A task is a window's docked pane (usually an agent) and its tabs. TASK is its ID as kanna task list shows it (task-1dfb: the first 4 of its 8 hex digits, more if another task starts the same way), any start of the ID that only one task has, with or without task- (1dfb), the full ID, or its agent's pane name or ID; its worktree and first branch are named after its full ID. Each Kanna.app has its own Desk and Shelf; the backlog is every app's. Shelve claims a task for Kanna.app's Shelf, and defer gives it back to the backlog. Park and pull are Kanna.app's, which the CLI starts if it isn't running: parking takes the task off its Desk to the Shelf, still running, and pulling puts it as it was on the Desk your terminal is on, else on a Desk on its project (the one Kanna.app had in front last), else a new one, parking whatever that Desk held; pulling a task that's on a Desk brings that window forward instead. Done closes the task's panes, ending its agent and terminals; its Desk stays, empty. Whether a task runs and where it is are separate. Start runs a task where it is: a backlog task gets its worktree, branch and agent and stays in the backlog, running, and any number can run there; a stopped task's agent comes back, and a claude agent resumes its conversation. Stop ends a task's agent and hangs up its terminals, and leaves it where it is, its tabs, focus and zoom as they were and its worktree and branch kept; starting it again reruns the terminals that were running. Shelve (Move to Shelf) moves a backlog task to the Shelf, starting it if it's stopped; an app's Shelf holds three tasks per repo, counting its Desk's. Defer (Move to Backlog) moves a task back to the backlog as it is, running or stopped, freeing its place on the Shelf. A backlog task can wait for other tasks (--after): it's blocked, and starting or shelving it is refused until they're all done; `task edit` changes what it waits for until it starts, and `task done` names the tasks it frees. A repo's tasks.setup (`kanna config schema`) runs in a new worktree's agent terminal before the agent starts, once per worktree; if it fails the task stops, `task start` starts the agent anyway, and `task setup` runs it again (in a Setup tab of a running task). Done runs tasks.teardown in the task's agent pane, its tabs closed: the task is Done once it succeeds, and a merged task's worktree and branch are removed (an unmerged one's kept); if it fails, the task stays where it is with its pane showing why, `task done` runs it again and `task done --keep` ends the task without it. A task's name is its docked pane's name, and a backlog task's own until it starts; `task name` sets either, and a backlog task keeps its new name when it starts. A backlog task's first message never changes: to change it, make a new task and take the old one off with `task done`. Send leaves a message for a task's agent, or for the repo's planning agent (planner), from the task you run it in, the planner, or else you: Kanna types it into the agent's terminal as a prompt, [message from SENDER] TEXT, once the agent's turn is over and it's at its prompt (never while it asks you something, nor while you're typing there), several at once as one prompt, oldest first. A stopped backlog task with a message runs for it, as one of the repo's backlog.runners, and a running one gets it rather than being stopped at its turn's end; a stopped Shelf task gets it when you start it; a done task takes none. `task messages` lists a task's messages, sent and received, delivered or waiting, or every message. Run from a task's terminal, `task park`, `task name` and `task done` mean that task, and `kanna open`, `kanna comment`, `kanna focus` and `kanna zoom` act in its own workspace, which a Desk shows when the task is pulled onto it.",
        examples: &[
            ("kanna task new \"Fix the page_comments pinch\" \"The mark drifts when you pinch; find out why and add a test\"", "Add a task to this repo's backlog."),
            ("kanna task new \"Release notes\" \"Write the 0.9 notes\" --after 1dfb,c0ff", "Add a task that can't start until task-1dfb and task-c0ff are done."),
            ("kanna task edit task-3f9a --after none", "Let a blocked task start: it waits for nothing now."),
            ("kanna task start 3f9a", "Start a backlog task where it is: its own worktree and branch, and its agent with the first message."),
            ("kanna task shelve 3f9a", "Move a backlog task to the Shelf, starting it if it's stopped."),
            ("kanna task stop 3f9a", "End a task's agent and hang up its terminals, leaving it and its tabs where they are; start it again to resume its conversation and rerun its terminals."),
            ("kanna task setup 3f9a", "Run the repo's tasks.setup in task-3f9a's worktree again: then its agent, if it's stopped."),
            ("kanna task list", "Every task, where it is (the Shelf or the backlog) and whether it runs."),
            ("kanna task list resume agent", "Only the tasks with both words in their name or first message."),
            ("kanna task park agent", "Park the task docked as agent; from its terminal, leave out agent."),
            ("kanna task pull agent", "Put the task docked as agent on your Desk (the one your terminal is on), else on its project's Desk."),
            ("kanna task name agent \"Tab bar light mode\"", "Name the task docked as agent; from its terminal, leave out agent."),
            ("kanna task name task-3f9a \"Search the backlog\"", "Rename task-3f9a, waiting in the backlog; it starts with that name."),
            ("kanna task defer agent", "Move the task docked as agent back to the backlog; from its terminal, leave out agent."),
            ("kanna task send 71af \"Is your spec at docs/specs/x.md final?\"", "Leave task-71af a message: it gets it when its turn is over, and answers with kanna task send."),
            ("kanna task send planner \"The spec is final\"", "Tell this repo's planning agent something, when the user asks you to."),
            ("kanna task messages 71af", "task-71af's messages: sent, received, and whether each was delivered."),
            ("kanna task done agent", "End the task docked as agent: close its agent and tabs."),
            ("kanna task done 3f9a --keep", "End task-3f9a without running the repo's tasks.teardown, keeping its worktree and branch."),
        ],
        parse: |args| {
            let action = args.positional().ok_or_else(|| usage("task", "say what: new, edit, start, stop, setup, shelve, list, park, pull, defer, name, send, messages or done"))?;
            let agent = args.option("--agent")?;
            if agent.is_some() && action != "new" {
                return Err(usage("task", "--agent is for task new"));
            }
            let after = args.option("--after")?;
            if after.is_some() && !["new", "edit"].contains(&action.as_str()) {
                return Err(usage("task", "--after is for task new and task edit"));
            }
            let after = after.map(|list| if list == "none" { vec![] } else { list.split(',').map(str::trim).filter(|id| !id.is_empty()).map(String::from).collect::<Vec<_>>() });
            let mut out = json!({ "action": action });
            match action.as_str() {
                "list" => {
                    if let Some(query) = args.rest() {
                        out["query"] = json!(query);
                    }
                }
                "new" => {
                    out["name"] = json!(args.positional().ok_or_else(|| usage("task", "new needs a NAME: kanna task new NAME [FIRST-MESSAGE]"))?);
                    if let Some(message) = args.positional() {
                        out["message"] = json!(message);
                    }
                    out["repo"] = json!(repo_here()?);
                    if let Some(agent) = agent {
                        out["agent"] = json!(agent);
                    }
                    if let Some(after) = after {
                        out["after"] = json!(after);
                    }
                    if let Some(by) = caller() {
                        out["by"] = json!(by);
                    }
                }
                "start" => {
                    out["task"] = json!(args.positional().ok_or_else(|| usage("task", "start needs a TASK (see `kanna task list`)"))?);
                }
                "edit" => {
                    out["task"] = json!(args.positional().ok_or_else(|| usage("task", "edit needs a TASK from the backlog (see `kanna task list`)"))?);
                    out["after"] = json!(after.ok_or_else(|| usage("task", "edit needs --after TASK,... or --after none"))?);
                }
                "shelve" => out["task"] = json!(args.positional().ok_or_else(|| usage("task", "shelve needs a TASK from the backlog (see `kanna task list`)"))?),
                "stop" => {
                    let task = args.positional().or_else(|| caller());
                    out["task"] = json!(task.ok_or_else(|| usage("task", "stop needs a TASK, or run it from the task's terminal"))?);
                }
                "setup" => {
                    let task = args.positional().or_else(|| caller());
                    out["task"] = json!(task.ok_or_else(|| usage("task", "setup needs a TASK, or run it from the task's terminal"))?);
                }
                "park" => {
                    let task = args.positional().or_else(|| caller());
                    out["task"] = json!(task.ok_or_else(|| usage("task", "park needs a TASK, or run it from the task's terminal"))?);
                }
                "pull" => out["task"] = json!(args.positional().ok_or_else(|| usage("task", "pull needs a TASK (see `kanna task list`)"))?),
                "defer" => {
                    let task = args.positional().or_else(|| caller());
                    out["task"] = json!(task.ok_or_else(|| usage("task", "defer needs a TASK, or run it from the task's terminal"))?);
                }
                "name" => {
                    let first = args.positional().ok_or_else(|| usage("task", "name needs a NAME: kanna task name [TASK] NAME"))?;
                    let (task, name) = match args.positional() {
                        Some(name) => (first, name),
                        None => (caller().ok_or_else(|| usage("task", "say which task: kanna task name TASK NAME"))?, first),
                    };
                    out["task"] = json!(task);
                    out["name"] = json!(name);
                }
                "send" => {
                    out["to"] = json!(args.positional().ok_or_else(|| usage("task", "send needs a TASK or planner, and TEXT: kanna task send TASK TEXT"))?);
                    out["text"] = json!(args.rest().ok_or_else(|| usage("task", "send needs TEXT: kanna task send TASK TEXT"))?);
                    if let Ok(repo) = repo_here() {
                        out["repo"] = json!(repo);
                    }
                    if let Some(by) = caller() {
                        out["by"] = json!(by);
                    }
                }
                "messages" => {
                    if let Some(task) = args.positional() {
                        out["task"] = json!(task);
                    }
                }
                "done" => {
                    if args.flag("--keep") {
                        out["keep"] = json!(true);
                    }
                    let task = args.positional().or_else(|| caller());
                    out["task"] = json!(task.ok_or_else(|| usage("task", "done needs a TASK (see `kanna task list`)"))?);
                }
                other => return Err(usage("task", &format!("unknown action \"{other}\" (new, edit, start, stop, setup, shelve, list, park, pull, defer, name, send, messages or done)"))),
            }
            if ["edit", "start", "stop", "setup", "shelve", "defer", "done", "park", "pull"].contains(&action.as_str()) || (action == "name" && out.get("task").is_some()) {
                if let Some(by) = caller() {
                    out["by"] = json!(by);
                }
            }
            Ok(out)
        },
        show: output::task,
    },
    Command {
        name: "navigate",
        group: "Panes",
        summary: "Point a web pane at a new URL (or go back or forward), a markdown pane at another file, a code pane at another file or line, or a diff pane at another diff.",
        usage: "kanna navigate PANE URL\n       kanna navigate PANE --back|--forward\n       kanna navigate PANE FILE.md\n       kanna navigate PANE FILE[:LINE]\n       kanna navigate PANE diff [GIT-DIFF-ARGS]",
        options: &[
            ("--back", "Go back a page: in a markdown pane, or a web pane opened with `open web URL --history`."),
            ("--forward", "Go forward a page (after --back)."),
        ],
        notes: "Markdown panes always keep history of the files they showed. Web panes keep none unless opened with --history.",
        examples: &[
            ("kanna navigate site localhost:5173", "Show localhost:5173 in the site pane."),
            ("kanna navigate site --back", "Go back to the previous page (the pane was opened with --history)."),
            ("kanna navigate spec docs/roadmap.md", "Show another markdown file in the spec pane."),
        ],
        parse: |args| {
            let pane = pane(args, "navigate")?;
            if args.first() == Some("diff") {
                args.positional();
                return Ok(json!({ "pane": pane, "args": args.remaining() }));
            }
            let (back, forward) = (args.flag("--back"), args.flag("--forward"));
            match (back, forward, args.positional()) {
                (true, false, None) => Ok(json!({ "pane": pane, "back": true })),
                (false, true, None) => Ok(json!({ "pane": pane, "forward": true })),
                (false, false, Some(target)) if is_markdown(&target) => Ok(json!({ "pane": pane, "path": absolute(&target) })),
                (false, false, Some(target)) if is_source(&target) => {
                    let (file, line) = file_line(&target);
                    let mut out = json!({ "pane": pane, "path": absolute(&file) });
                    if let Some(line) = line {
                        out["line"] = json!(line);
                    }
                    Ok(out)
                }
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
        summary: "Show what the running Kanna supports: pane types and commands.",
        usage: "kanna capabilities",
        options: &[],
        notes: "",
        examples: &[("kanna capabilities", "See which commands this Kanna supports.")],
        parse: |_| Ok(json!({})),
        show: |_, reply| output::capabilities(reply),
    },
    Command {
        name: "diagrams",
        group: "Information",
        summary: "Check that a markdown file's mermaid diagrams draw in Kanna, without opening anything.",
        usage: "kanna diagrams check FILE.md...",
        options: &[],
        notes: "Prints FILE:LINE: message for each ```mermaid block that won't draw, LINE being its fence line: merman can't parse it, or Kanna.app doesn't draw its type. Then `N diagrams, all draw`, or exits 1 saying how many don't. `kanna open md` checks the same and prints the failures after it opens the pane.\nNeeds no Kanna.app or daemon. It can't see labels that overlap or clip: take `kanna screenshot PANE` of the markdown pane and look.",
        examples: &[
            ("kanna diagrams check docs/specs/plan.md", "Check one doc's diagrams."),
            ("kanna diagrams check docs/specs/*.md --json", "Check many; the count and the failures as JSON."),
        ],
        parse: |args| {
            match args.positional().as_deref() {
                Some("check") => {}
                Some(other) => return Err(usage("diagrams", &format!("unknown action \"{other}\" (check)"))),
                None => return Err(usage("diagrams", "missing check")),
            }
            let files = args.positionals();
            if files.is_empty() {
                return Err(usage("diagrams", "missing FILE.md"));
            }
            Ok(json!({ "action": "check", "files": files }))
        },
        show: |_, _| String::new(),
    },
    Command {
        name: "status",
        group: "Information",
        summary: "Explain an agent pane's status, or check the rules site for new status rules now.",
        usage: "kanna status explain PANE\n       kanna status explain --file SCREEN --agent AGENT\n       kanna status update",
        options: &[
            ("--file SCREEN", "A saved screen: text (as `kanna read` prints it) or a corpus case (JSON with rows, title, progress and cliVersion)."),
            ("--agent AGENT", "Whose rules judge the saved screen: claude, codex, …"),
        ],
        notes: "An agent pane's status (working, needs-you, idle, exited) is in `kanna task list`, and in `kanna workspace list --all --json`. explain shows the agent and its version, where its status came from (the agent's own OSC 7501 report, with the record, which wins when there is one, or the rules), the rules file and where it came from, the rule that matched and the text it matched.\nRules: bundled in Kanna, from the rules site (checked every 30 minutes), or your own in ~/.config/kanna/agent-rules/<agent>.json, which always wins and reloads when you save it.",
        examples: &[
            ("kanna status explain term-3", "Why term-3's agent shows the status it does."),
            ("kanna status explain --file screen.txt --agent claude", "How Claude's rules judge a saved screen."),
            ("kanna status update", "Fetch new rules from the rules site now."),
        ],
        parse: parse_status,
        show: output::status,
    },
    Command {
        name: "toast",
        group: "Information",
        summary: "Show a toast in Kanna.app: a small notice in the front window that fades by itself.",
        usage: "kanna toast TITLE [DETAIL] [--pane PANE] [--task TASK] [--key KEY]",
        options: &[
            ("--pane PANE", "The pane it's about: clicking the toast shows that pane, bringing its task onto the Desk."),
            ("--task TASK", "The task it's about (an ID like task-3f9a, or its pane): clicking the toast brings it onto the Desk."),
            ("--key KEY", "Raise it once: a later toast with the same key, from the CLI or the daemon, isn't shown until the daemon restarts."),
        ],
        notes: "The daemon raises toasts for its warnings, such as messages that wait because their recipient can't take them. A toast stacks below the title bar on the right, newest on top, and fades after 6 seconds (8 when it's about a pane or task) unless the pointer is on it. With no app attached, a toast waits up to a minute for one.",
        examples: &[
            ("kanna toast \"Build finished\" \"scripts/build.sh took 3m 12s\"", "A toast with a line of detail."),
            ("kanna toast \"Tests failed\" \"3 of 120\" --pane tests --key tests-failed", "A toast about the pane named tests, raised once."),
        ],
        parse: parse_toast,
        show: output::toast,
    },
    Command {
        name: "quiz",
        group: "Information",
        summary: "Open this repo's Quiz window, and keep a quiz's rounds and answers, so misses come back in a later round and a round can cover what's new since the last.",
        usage: "kanna quiz open [--agent CMD]\n       kanna quiz start [--json DOC]\n       kanna quiz answer ROUND --kind KIND --file FILE:LINE[-LAST] --right|--wrong [--again ANSWER] [--json DOC]\n       kanna quiz history [--misses]",
        options: &[
            ("--agent CMD", "open: the quiz agent to start (default: claude)."),
            ("--kind KIND", "answer: architecture, drawing, behaviour, where, why or bug."),
            ("--file FILE:LINE[-LAST]", "answer: the lines the question is about, kept relative to the repo."),
            ("--right, --wrong", "answer: whether the user picked the right choice."),
            ("--again ANSWER", "answer: the earlier miss this question asks again (a7); it stops being a miss."),
            ("--json DOC", "start, answer: a JSON object kept with the round (commits covered, agent) or the answer (question, choices, the pick, the right choice, explanation)."),
            ("--misses", "history: only the misses not asked again since."),
        ],
        notes: "For the kanna-quiz skill's agents, run in the repo. open, for the planning agent, brings the repo's Quiz window forward, or opens one: a window with the quiz agent docked on the left, in the repo's main checkout, told it's the quiz agent, and what it opens as tabs beside it; it isn't a task. start begins a round on this repo at main's commit and prints its ID (r12); answer records one answer to it and prints the answer's ID (a7); history lists this repo's rounds, newest first, each with its commit and answers. Nothing here is a score: history is for picking the next round's questions.",
        examples: &[
            ("kanna quiz open --agent codex", "Open the Quiz window with Codex as the quiz agent, or bring it forward."),
            ("kanna quiz start --json '{\"commits\": [\"67b287c\"]}'", "Start a round on main's commit; prints its ID, e.g. r12."),
            ("kanna quiz answer r12 --kind why --file docs/specs/database.md:40 --wrong --json '{\"question\": \"Why one SQLite file?\", \"pick\": 2, \"answer\": 1}'", "Record a miss in round r12."),
            ("kanna quiz answer r13 --kind why --file docs/specs/database.md:40 --right --again a7", "Ask miss a7 again, and record that it was right this time."),
            ("kanna quiz history --misses --json", "The misses to bring back, as JSON."),
        ],
        parse: parse_quiz,
        show: output::quiz,
    },
    Command {
        name: "projects",
        group: "Information",
        summary: "List your recent projects, most recent first, as File → Open Recent and a new Desk's planning terminal show them.",
        usage: "kanna projects [--json]\n       kanna projects clear",
        options: &[("--json", "Each project's path, its label (its name if it's in ~/.kanna-v4/repos or ~/.kanna/repos, else its path from ~), and when a Desk last opened on it or moved to it.")],
        notes: "A project is its repo's folder (the main repo for a worktree). One moves to the top whenever a Desk opens on it or moves to it: kanna PATH, Open Recent, a planning terminal's folder, a task pulled onto the Desk. Ten are kept, and a folder that's gone drops off. Each is printed as an absolute path on a line of its own. clear empties the list, as Open Recent's Clear Menu does.",
        examples: &[("kanna projects", "Your recent projects, latest first."), ("kanna projects --json", "Each one's path and when it was last opened, as JSON."), ("kanna projects clear", "Forget them.")],
        parse: |args| match args.positional().as_deref() {
            None => Ok(json!({ "action": "list" })),
            Some("clear") => Ok(json!({ "action": "clear" })),
            Some(other) => Err(usage("projects", &format!("unknown action \"{other}\" (clear, or none to list them)"))),
        },
        show: output::projects,
    },
    Command {
        name: "project",
        group: "Information",
        summary: "Import a project into ~/.kanna-v4/repos, where Kanna keeps every project at the same place on each of your machines: a copy, leaving the original as it is.",
        usage: "kanna project import PATH [--json]",
        options: &[("--json", "What was imported: from, to, the worktrees copied, those left with the original, the agents' sessions carried over, the tasks and terminals started again, and the files that still name the old path.")],
        notes: "Kanna opens Desks and runs tasks only for a project at its place in ~/.kanna-v4/repos: OWNER/NAME from its origin remote (github.com/a/app is a/app), or local/NAME with none. import copies the project PATH is in (a worktree's repo) there, whole: .git, uncommitted and untracked work, stashes, local branches and its worktrees in .kanna-worktrees, as APFS clones where it can, which take no space. PATH stays as it was; Kanna still refuses it, and you can delete it once you're happy with the copy. Worktrees outside the folder stay with the original and are listed. It stops the project's running tasks and terminals first, and starts them again in the copy, each agent resuming its conversation. Kanna's records follow the copy: the tasks, messages, recent projects, planners and terminal folders in kanna.db, Claude's conversation and memory folders (copied), and the sessions codex, copilot and opencode keep by folder (pointed at the copy). Then it lists the files that still name PATH, from a quick search of ~/.claude, ~/.agents, your shell's files and the repo, for you to change. It refuses when another clone of the same remote is at the place already, saying both, and when it runs in a terminal in the project, which it would stop. On another machine, run it there: kanna --host MACHINE project import '~/work/app'.",
        examples: &[("kanna project import ~/work/app", "Copy ~/work/app to ~/.kanna-v4/repos/OWNER/NAME and move its tasks and terminals to the copy."), ("kanna project import '~/work/app'", "The same, with ~ as the daemon's machine has it: with --host MACHINE, that machine's copy.")],
        parse: |args| {
            let action = args.positional().ok_or_else(|| usage("project", "say what: import PATH"))?;
            if action != "import" {
                return Err(usage("project", &format!("unknown action \"{action}\" (import)")));
            }
            let path = args.positional().ok_or_else(|| usage("project", "import needs a PATH: kanna project import PATH"))?;
            let path = if path.starts_with('~') { path } else { absolute(&path) };
            let mut out = json!({ "action": "import", "path": path });
            if let Some(by) = caller() {
                out["by"] = json!(by);
            }
            Ok(out)
        },
        show: output::project,
    },
    Command {
        name: "machine",
        group: "Information",
        summary: "Your machines: pair one, list them, check one's link, give an address, update them, or revoke one. kanna --host MACHINE COMMAND runs any command on one.",
        usage: "kanna machine [list]\n       kanna machine pair\n       kanna machine pair CODE [--peer KEY[@HOST[:PORT]]]\n       kanna machine name NAME\n       kanna machine ping MACHINE\n       kanna machine addr MACHINE HOST[:PORT]\n       kanna machine revoke MACHINE\n       kanna machine update [MACHINE ...] [--tarball FILE]\n       kanna --host MACHINE COMMAND ...",
        options: &[
            ("--peer KEY[@HOST[:PORT]]", "pair CODE: the machine to pair with, as `kanna machine pair` prints it there, when it isn't on this LAN."),
            ("--tarball FILE", "update: the Linux tarball to install (scripts/build-linux.sh makes one); without it, this release's tarball is downloaded."),
            ("--host MACHINE", "Before any command: run it on that machine (its name or key) instead of this one. KANNA_HOST does the same."),
        ],
        notes: "Each machine's daemon has a key of its own (device.key in its state folder) and listens for the others over iroh, on UDP port 7470 when it's free. Machines on a LAN find each other by themselves.\nTo add a machine, run `kanna machine pair` on it: it shows a six-digit code and waits. On one of your machines (or the first one, which starts them), run `kanna machine pair CODE`; Kanna.app there asks you to confirm, and the new machine joins. A code works once, for five minutes, and three wrong codes close it.\nA machine is known by its name, the first part of its host name unless `kanna machine name` changed it before pairing. A name that comes back with another key is refused; a revoked machine is never let back in, under any name, and has to be paired with a new key.\n`kanna machine addr` gives an address to try for a machine that isn't on this LAN, such as one on your VPN. ping opens a connection and one stream of each kind, and says whether the path is direct.\n`kanna machine update` puts one commit on each of your Linux machines (or those named): this release's tarball, or the one --tarball gives. Each machine installs it in /opt/kanna/COMMIT beside the one it runs, switches current to it and restarts its daemon; the keeper keeps every terminal and agent running. Macs update with Kanna.app. Machines of another protocol take an update too.",
        examples: &[
            ("kanna machine list", "This machine, your other machines and how each is reached."),
            ("kanna machine pair", "On a new machine: show a code to pair it with."),
            ("kanna machine pair 042917", "On your Mac: add the machine showing 042917."),
            ("kanna machine ping r720-1", "Whether r720-1 answers, by which path, and that each kind of stream opens."),
            ("kanna machine addr r720-1 10.0.0.5", "Also try r720-1 at 10.0.0.5, port 7470."),
            ("kanna machine revoke old-laptop", "Shut old-laptop out of every machine for good."),
            ("kanna machine update", "Put this release on each of your Linux machines."),
            ("kanna machine update r720-1 --tarball target/release-linux/Kanna-4.1.0-linux-x86_64.tar.gz", "Put the build in that tarball on r720-1."),
        ],
        parse: parse_machine,
        show: output::machine,
    },
    Command {
        name: "agents",
        group: "Information",
        summary: "Set up your agents for Kanna: Kanna's skills for Claude and Codex, a hook that tells them they're in Kanna, and Claude's sandbox letting them reach Kanna but not read its device keys.",
        usage: "kanna agents [status]\n       kanna agents setup [--ask]\n       kanna agents update [--ask] [--force]\n       kanna agents remove",
        options: &[
            ("--ask", "Say what it would do and ask first; says nothing if there's nothing to ask (the planning terminal runs it)."),
            ("--force", "update: put Kanna's version over files you've changed."),
        ],
        notes: "setup copies Kanna's skills (kanna-tasks, kanna-planning, kanna-new-project, kanna-markup, kanna-explain, kanna-quiz) to ~/.agents/skills, where Codex reads them, links each from ~/.claude/skills for Claude, adds Kanna's socket to sandbox.network.allowUnixSockets in ~/.claude/settings.json, and denies reading every instance's device.key there (sandbox.filesystem.denyRead and a Read rule in permissions.deny). update brings the skills up to this Kanna's: a file you've changed is merged with Kanna's changes, and where they conflict it offers your merge editor (git's merge.tool, else opendiff). remove takes back what Kanna put there. Setup and update also take out the SessionStart hooks earlier Kannas added; Kanna tells the agents it starts that they're in Kanna in their first message. Kanna's planning terminal asks before setting up and before each update; nothing is written until you agree.",
        examples: &[
            ("kanna agents", "Whether your agents are set up, and whether this Kanna has newer skills."),
            ("kanna agents setup", "Install Kanna's skills, the hook and the sandbox entry."),
            ("kanna agents update", "Bring the skills up to this Kanna's, merging your changes."),
            ("kanna agents remove", "Take them back out."),
        ],
        parse: parse_agents,
        show: output::agents,
    },
    Command {
        name: "markup",
        group: "Panes",
        summary: "Open a markdown file or a local page to comment on, turn comments on or off in a pane, and answer them; the comments come to this pane.",
        usage: "kanna markup FILE.md|FILE.html [--to PANE] [--name NAME] [--split right|down|auto] [--size FRACTION] [--tab] [--window ID|new] [--dock [left|right]] [--no-wait]\n       kanna markup PANE [--to PANE | --off]\n       kanna markup comments [--all] [--doc FILE]\n       kanna markup show|resolve|reopen THREAD [--doc FILE]\n       kanna markup reply THREAD TEXT... [--doc FILE]",
        options: &[
            ("--to PANE", "The terminal comments go to, e.g. the pane an agent runs in. Default: this one."),
            ("--name NAME", "A name to refer to the pane by in later commands."),
            ("--split right|down|auto", "Where the pane goes relative to the focused pane, as for open."),
            ("--size FRACTION", "Share of the focused pane's space it takes, as for open."),
            ("--tab", "Open in a new tab instead of splitting."),
            ("--window ID|new", "Open in that window, or a new one."),
            ("--dock [left|right]", "Make it the window's docked pane, as for open."),
            ("--no-wait", "Return as soon as the pane exists."),
            ("--off", "Turn comments off in PANE: it shows the doc alone again. Its comments are kept."),
            ("--all", "comments: resolved ones too."),
            ("--doc FILE", "The doc a comment is on. Default: found from the comment's ID, the docs whose comments come to this pane first."),
        ],
        notes: "A markdown or web pane like `kanna open md FILE` or `kanna open web FILE`, with a thread list and a comment box: select text and press ⌘↵, ⌘-click a part, or ⌘-drag a box. Each comment is sent to the terminal that ran markup (or the one --to names), with the part or box and, for a page or an image, a screenshot with it marked up. `kanna open` shows the doc without comments; `kanna markup PANE` turns them on there.",
        examples: &[
            ("kanna markup references/mockups/desk.html --split right", "Review a mockup beside this pane."),
            ("kanna markup docs/specs/desk.md --tab", "Review a spec in a new tab."),
            ("kanna markup docs/specs/desk.md --to agent --tab", "Open a spec for comments in a new tab and send them to the pane named agent."),
            ("kanna markup spec", "Turn on comments in the pane named spec; they come to this pane."),
            ("kanna markup spec --to agent", "Send spec's comments to the pane named agent instead."),
            ("kanna markup spec --off", "Show spec without comments again."),
            ("kanna markup comments", "The open comments on the docs whose comments come to this pane."),
            ("kanna markup reply c4 \"Renamed it.\"", "Answer comment c4 beside the doc; leave it open for its author to read and resolve."),
        ],
        parse: parse_markup,
        show: output::marked,
    },
    Command {
        name: "comment",
        group: "Panes",
        summary: "Comment on some lines of a file, shown beside them, to explain the code.",
        usage: "kanna comment FILE:LINE[-LAST] TEXT... [--name NAME] [--split right|down|auto] [--size FRACTION] [--tab] [--window ID|new] [--no-wait]",
        options: &[
            ("--name NAME", "A name for the code pane, if it opens one."),
            ("--split right|down|auto", "Open the file in a new pane split from the focused one."),
            ("--size FRACTION", "Share of the focused pane's space the new pane takes."),
            ("--tab", "Open the file in a new tab, even if it's already open."),
            ("--window ID|new", "Open the file in that window, or a new one."),
        ],
        notes: "On a markdown file, starts a comment thread (as `kanna markup`'s) on the block at LINE (a paragraph, a list item, a heading), written by you, in the doc's pane with comments on; the user's replies in it come to you as comments do.\nOn a source file, opens FILE in a code pane at LINE, or uses the code pane in this window that already shows it, tints the lines and dims the rest, with the comment in a box above them; it replaces the pane's last comment. LINE takes the whole scope it opens or sits in (a function, a loop, a block); LINE-LAST takes those lines as they are. The pane keeps the comment until it closes. Prints the pane and the lines commented on.",
        examples: &[
            ("kanna comment src/fdpass.rs:50 \"Waits for a message and takes the terminals that came with it.\"", "Comment on the function that starts at line 50."),
            ("kanna comment src/client.rs:244-246 \"Takes as many terminals as the message says.\"", "Comment on lines 244 to 246 exactly."),
        ],
        parse: parse_comment,
        show: output::commented,
    },
    Command {
        name: "tour",
        group: "Panes",
        summary: "Queue a tour of comments the user steps through with Next and Back, without you.",
        usage: "kanna tour [add [--next]] FILE:LINE[-LAST] TEXT [FILE:LINE TEXT ...] [--name NAME] [--split right|down|auto] [--size FRACTION] [--tab] [--window ID|new]\n       kanna tour go next|back|N\n       kanna tour [show]\n       kanna tour end",
        options: &[
            ("--next", "With add: put the stops right after the one the user is on, and show the first of them."),
            ("--name NAME", "A name for the code pane, if the stop shown opens one."),
            ("--split right|down|auto", "Open the stop shown in a new pane split from the focused one."),
            ("--size FRACTION", "Share of the focused pane's space the new pane takes."),
            ("--tab", "Open the stop shown in a new tab, even if its file is already open."),
            ("--window ID|new", "Open the stop shown in that window, or a new one."),
        ],
        notes: "A tour is yours, the agent's: a list of stops, each a comment as `kanna comment` makes it, given as pairs of FILE:LINE and its text (one argument each; quote it). A new tour replaces your last and shows its first stop; every stop is checked first, and nothing is queued if one fails. The user steps through it with Back and Next on the comment, and nothing is sent to you as they do: when they ask you something, `kanna tour` says which stop they're on. Answer a question with `kanna tour add --next` on the code that answers it. `add` without --next adds stops at the end while the user reads. Next on the last stop ends the tour; its comment stays.",
        examples: &[
            ("kanna tour src/server.rs:92 \"Queues the message with its terminals.\" src/fdpass.rs:20 \"Sends them across.\"", "A tour of two stops; shows the first."),
            ("kanna tour", "Which stop the user is on, and the rest."),
            ("kanna tour add --next src/client.rs:244 \"Each line says how many terminals are its.\"", "Answer a question with a stop after the current one, shown now."),
        ],
        parse: parse_tour,
        show: output::toured,
    },
];

fn parse_markup(args: &mut Argv) -> Result<Value, Failure> {
    let mut out = Map::new();
    for (flag, key) in [("--name", "name"), ("--split", "split"), ("--size", "size"), ("--window", "window")] {
        if let Some(value) = args.option(flag)? {
            out.insert(key.into(), json!(value));
        }
    }
    if let Some(edge) = args.optional("--dock", is_edge) {
        out.insert("dock".into(), json!(edge.unwrap_or_else(|| "left".into())));
    }
    if args.flag("--tab") {
        out.insert("tab".into(), json!(true));
    }
    if out.contains_key("dock") && ["split", "size", "tab"].iter().any(|k| out.contains_key(*k)) {
        return Err(usage("markup", "--dock goes on its own: no --split, --size or --tab"));
    }
    if args.flag("--no-wait") {
        out.insert("wait".into(), json!(false));
    }
    if let Some(action) = args.first().filter(|a| crate::threads::ACTIONS.contains(a)).map(String::from) {
        return parse_thread_action(args, &action);
    }
    let off = args.flag("--off");
    let to = args.option("--to")?;
    if off && to.is_some() {
        return Err(usage("markup", "--off sends comments nowhere, so it takes no --to"));
    }
    let target = args.positional().ok_or_else(|| usage("markup", "say what: kanna markup FILE.md, kanna markup FILE.html or kanna markup PANE"))?;
    let lower = target.to_lowercase();
    let agent = to.or_else(|| caller());
    let page = lower.ends_with(".html") || lower.ends_with(".htm");
    if !is_markdown(&target) && !page {
        if !out.is_empty() {
            return Err(usage("markup", &format!("{target} is a pane, so it stays where it is: no placement options")));
        }
        let mut pane = json!({ "pane": target });
        if off {
            pane["off"] = json!(true);
        } else if let Some(agent) = agent {
            pane["agent"] = json!(agent);
        }
        return Ok(pane);
    }
    if off {
        return Err(usage("markup", "--off is for a pane: kanna markup PANE --off"));
    }
    if page {
        out.insert("type".into(), json!("web"));
        out.insert("url".into(), json!(file_url(&absolute(&target))));
    } else {
        out.insert("type".into(), json!("md"));
        out.insert("path".into(), json!(absolute(&target)));
    }
    out.insert("markup".into(), json!(true));
    if let Some(agent) = agent {
        out.insert("agent".into(), json!(agent));
    }
    from_caller(&mut out);
    Ok(Value::Object(out))
}

fn parse_quiz(args: &mut Argv) -> Result<Value, Failure> {
    let misses = args.flag("--misses");
    let (right, wrong) = (args.flag("--right"), args.flag("--wrong"));
    let kind = args.option("--kind")?;
    let file = args.option("--file")?;
    let again = args.option("--again")?;
    let agent = args.option("--agent")?;
    let doc = match args.doc() {
        Some(text) => Some(serde_json::from_str::<Value>(&text).ok().filter(Value::is_object).ok_or_else(|| usage("quiz", "--json takes a JSON object"))?),
        None => None,
    };
    let action = args.positional().ok_or_else(|| usage("quiz", "say what: open, start, answer or history"))?;
    let only = |given: bool, flag: &str, action: &str| if given { Err(usage("quiz", &format!("{flag} is for quiz {action}"))) } else { Ok(()) };
    if action != "answer" {
        for (given, flag) in [(kind.is_some(), "--kind"), (file.is_some(), "--file"), (right || wrong, "--right and --wrong"), (again.is_some(), "--again")] {
            only(given, flag, "answer")?;
        }
    }
    if action != "history" {
        only(misses, "--misses", "history")?;
    }
    if action != "open" {
        only(agent.is_some(), "--agent", "open")?;
    }
    let repo = repo_here().map_err(|_| usage("quiz", "a quiz is about a git repo: run this in one"))?;
    let mut out = json!({ "action": action, "repo": repo });
    match action.as_str() {
        "open" => {
            if doc.is_some() {
                return Err(usage("quiz", "--json DOC is for start and answer"));
            }
            if let Some(agent) = agent {
                out["agent"] = json!(agent);
            }
        }
        "start" => {
            out["head"] = json!(main_commit(&repo)?);
            if let Some(by) = caller() {
                out["by"] = json!(by);
            }
        }
        "answer" => {
            out["round"] = json!(args.positional().ok_or_else(|| usage("quiz", "answer needs a ROUND, the ID quiz start printed"))?);
            out["kind"] = json!(kind.ok_or_else(|| usage("quiz", "answer needs --kind: architecture, drawing, behaviour, where, why or bug"))?);
            let spot = file.ok_or_else(|| usage("quiz", "answer needs --file FILE:LINE"))?;
            let (file, line, last) = file_lines(&spot).ok_or_else(|| usage("quiz", &format!("{spot} needs a line: FILE:LINE or FILE:LINE-LAST")))?;
            out["file"] = json!(in_repo(&file));
            out["line"] = json!(line);
            if let Some(last) = last {
                out["last"] = json!(last);
            }
            out["right"] = match (right, wrong) {
                (true, false) => json!(true),
                (false, true) => json!(false),
                _ => return Err(usage("quiz", "say --right or --wrong")),
            };
            if let Some(again) = again {
                out["again"] = json!(again);
            }
        }
        "history" => {
            out["misses"] = json!(misses);
            if doc.is_some() {
                return Err(usage("quiz", "--json DOC is for start and answer; history --json prints JSON"));
            }
        }
        other => return Err(usage("quiz", &format!("unknown action \"{other}\" (open, start, answer or history)"))),
    }
    if let Some(doc) = doc {
        out["doc"] = doc;
    }
    Ok(out)
}

fn main_commit(repo: &str) -> Result<String, Failure> {
    ["main", "master"]
        .iter()
        .find_map(|branch| git_line(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}^{{commit}}")]))
        .ok_or_else(|| usage("quiz", "a quiz asks about main, and this repo has no main (or master) branch"))
}

fn diff_base(path: &str, reference: Option<String>) -> Result<String, Failure> {
    if let Some(reference) = reference {
        return Ok(reference);
    }
    let folder = std::path::Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| ".".into());
    ["main", "master"]
        .iter()
        .find(|branch| git_line(&folder, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_some())
        .map(|branch| branch.to_string())
        .ok_or_else(|| usage("open", "--diff compares with main (or master), and there's none here: say what to compare with, e.g. --diff HEAD"))
}

fn git_line(at: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").arg("-C").arg(at).args(args).output().ok().filter(|out| out.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|line| !line.is_empty())
}

fn in_repo(file: &str) -> String {
    let path = absolute(file);
    let path = std::fs::canonicalize(&path).unwrap_or_else(|_| path.into());
    let top = git_line(".", &["rev-parse", "--show-toplevel"]);
    match top.as_deref().and_then(|top| path.strip_prefix(top).ok()) {
        Some(inside) => inside.to_string_lossy().into_owned(),
        None => file.to_string(),
    }
}

fn repo_here() -> Result<String, Failure> {
    let here = std::env::current_dir().map_err(|e| usage("task", &format!("can't tell where you are: {e}")))?;
    let common = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(&here)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()));
    match common {
        Some(git) if git.file_name().is_some_and(|n| n == ".git") => Ok(git.parent().unwrap_or(&git).to_string_lossy().into()),
        Some(git) => Ok(git.to_string_lossy().into()),
        None => Err(usage("task", "a task belongs to a git repo, and works in a worktree of it: run this in one, or set one up first (git init)")),
    }
}

pub fn caller() -> Option<String> {
    std::env::var("KANNA_TERMINAL").ok().filter(|id| !id.is_empty())
}

fn from_caller(out: &mut Map<String, Value>) {
    if out.contains_key("dock") {
        return;
    }
    if let Some(pane) = caller() {
        out.insert("from".into(), json!(pane));
    }
}

fn parse_workspace(args: &mut Argv) -> Result<Value, Failure> {
    let action = args.positional().ok_or_else(|| usage("workspace", "say list"))?;
    if action != "list" {
        return Err(usage("workspace", &format!("unknown action \"{action}\": list")));
    }
    if args.flag("--all") {
        return Ok(json!({ "action": "list", "all": true }));
    }
    Ok(match caller() {
        Some(pane) => json!({ "action": "list", "pane": pane }),
        None => json!({ "action": "list" }),
    })
}

fn parse_window(args: &mut Argv) -> Result<Value, Failure> {
    let action = args.positional().ok_or_else(|| usage("window", "say assign or free"))?;
    let window = args.positional().ok_or_else(|| usage("window", "missing WINDOW (an ID like win-4; see `kanna workspace list --all`)"))?;
    match action.as_str() {
        "assign" => {
            let task = args.positional().ok_or_else(|| usage("window", "missing TASK (an ID like task-1dfb; see `kanna task list`)"))?;
            Ok(json!({ "action": "assign", "window": window, "workspace": task }))
        }
        "free" => Ok(json!({ "action": "free", "window": window })),
        other => Err(usage("window", &format!("unknown action \"{other}\": assign or free"))),
    }
}

fn parse_thread_action(args: &mut Argv, action: &str) -> Result<Value, Failure> {
    let all = args.flag("--all");
    let doc = args.option("--doc")?;
    args.positional();
    let mut out = json!({ "local": action });
    if let Some(doc) = doc {
        out["doc"] = json!(doc);
    }
    if action == "comments" {
        out["all"] = json!(all);
        return Ok(out);
    }
    let id = args.positional().ok_or_else(|| usage("markup", &format!("{action} which comment? e.g. kanna markup {action} c2")))?;
    out["id"] = json!(id);
    if action == "reply" {
        let text = args.rest().ok_or_else(|| usage("markup", "reply with what? e.g. kanna markup reply c2 \"Fixed.\""))?;
        out["text"] = json!(text);
    }
    Ok(out)
}

fn parse_comment(args: &mut Argv) -> Result<Value, Failure> {
    let mut out = Map::new();
    for (flag, key) in [("--name", "name"), ("--split", "split"), ("--size", "size"), ("--window", "window")] {
        if let Some(value) = args.option(flag)? {
            out.insert(key.into(), json!(value));
        }
    }
    if args.flag("--tab") {
        out.insert("tab".into(), json!(true));
    }
    if args.flag("--no-wait") {
        out.insert("wait".into(), json!(false));
    }
    let spot = args.positional().ok_or_else(|| usage("comment", "say where: kanna comment FILE:LINE \"text\""))?;
    let (file, line, last) = file_lines(&spot).ok_or_else(|| usage("comment", &format!("{spot} needs a line: FILE:LINE or FILE:LINE-LAST")))?;
    let text = args.rest().filter(|t| !t.trim().is_empty()).ok_or_else(|| usage("comment", "say what: kanna comment FILE:LINE \"text\""))?;
    out.insert("path".into(), json!(absolute(&file)));
    out.insert("line".into(), json!(line));
    if let Some(last) = last {
        if last < line {
            return Err(usage("comment", &format!("lines {line}-{last} run backwards")));
        }
        out.insert("last".into(), json!(last));
    }
    out.insert("text".into(), json!(text));
    if is_markdown(&file) {
        if last.is_some() {
            return Err(usage("comment", "a comment on a markdown file is on the block at one line: FILE.md:LINE"));
        }
        out.insert("doc".into(), json!(true));
        if let Some(agent) = caller() {
            out.insert("agent".into(), json!(agent));
        }
    }
    from_caller(&mut out);
    Ok(Value::Object(out))
}

fn parse_tour(args: &mut Argv) -> Result<Value, Failure> {
    let mut out = Map::new();
    for (flag, key) in [("--name", "name"), ("--split", "split"), ("--size", "size"), ("--window", "window")] {
        if let Some(value) = args.option(flag)? {
            out.insert(key.into(), json!(value));
        }
    }
    if args.flag("--tab") {
        out.insert("tab".into(), json!(true));
    }
    let next = args.flag("--next");
    let mut words = args.positionals();
    let action = match words.first().map(String::as_str) {
        None | Some("show") => "show",
        Some("add") => "add",
        Some("go") => "go",
        Some("end") => "end",
        Some(_) => "new",
    };
    if action != "new" && !words.is_empty() {
        words.remove(0);
    }
    if next && action != "add" {
        return Err(usage("tour", "--next goes with add: kanna tour add --next FILE:LINE \"text\""));
    }
    out.insert("action".into(), json!(action));
    match action {
        "new" | "add" => {
            if words.is_empty() {
                return Err(usage("tour", "say where and what: FILE:LINE \"text\" ..."));
            }
            if words.len() % 2 == 1 {
                let last = words.last().cloned().unwrap_or_default();
                return Err(usage("tour", &match file_lines(&last) {
                    Some(_) => format!("{last} has no text: give each stop as FILE:LINE \"text\""),
                    None => format!("\"{last}\" has no FILE:LINE before it: give each stop as FILE:LINE \"text\", the text quoted as one argument"),
                }));
            }
            let mut stops = vec![];
            for pair in words.chunks(2) {
                let (spot, text) = (&pair[0], pair[1].trim());
                let (file, line, last) = file_lines(spot).ok_or_else(|| usage("tour", &format!("{spot} needs a line: FILE:LINE or FILE:LINE-LAST, then its text quoted as one argument")))?;
                if text.is_empty() {
                    return Err(usage("tour", &format!("{spot} has no text")));
                }
                let mut stop = json!({ "path": absolute(&file), "line": line, "text": text });
                if let Some(last) = last {
                    if last < line {
                        return Err(usage("tour", &format!("{spot}: lines {line}-{last} run backwards")));
                    }
                    if is_markdown(&file) {
                        return Err(usage("tour", &format!("{spot}: a comment on a markdown file is on the block at one line: FILE.md:LINE")));
                    }
                    stop["last"] = json!(last);
                }
                stops.push(stop);
            }
            out.insert("stops".into(), Value::Array(stops));
            if next {
                out.insert("next".into(), json!(true));
            }
        }
        "go" => {
            let to = match words.as_slice() {
                [to] if to == "next" || to == "back" => json!(to),
                [to] => json!(to.parse::<u64>().ok().filter(|n| *n > 0).ok_or_else(|| usage("tour", &format!("\"{to}\" isn't a stop: next, back or a stop's number")))?),
                _ => return Err(usage("tour", "say where: kanna tour go next|back|N")),
            };
            out.insert("to".into(), to);
        }
        _ => {
            if let Some(extra) = words.first() {
                return Err(usage("tour", &format!("unexpected argument \"{extra}\"")));
            }
        }
    }
    from_caller(&mut out);
    Ok(Value::Object(out))
}

pub fn file_lines(target: &str) -> Option<(String, u64, Option<u64>)> {
    let (file, spot) = target.rsplit_once(':')?;
    let (line, last) = match spot.split_once('-') {
        Some((line, last)) => (line.parse::<u64>().ok()?, Some(last.parse::<u64>().ok()?)),
        None => (spot.parse::<u64>().ok()?, None),
    };
    (line > 0 && !file.is_empty()).then(|| (file.to_string(), line, last))
}

fn parse_status(args: &mut Argv) -> Result<Value, Failure> {
    let file = args.option("--file")?;
    let agent = args.option("--agent")?;
    match args.positional().as_deref() {
        Some("update") => Ok(json!({ "action": "update" })),
        Some("explain") => match (file, agent) {
            (Some(file), Some(agent)) => Ok(json!({ "action": "explain", "agent": agent, "file": absolute(&file) })),
            (Some(_), None) => Err(usage("status", "a saved screen needs --agent (claude, codex, …)")),
            (None, _) => Ok(json!({ "action": "explain", "pane": pane(args, "status")? })),
        },
        Some(other) => Err(usage("status", &format!("unknown action \"{other}\" (explain or update)"))),
        None => Err(usage("status", "say what: explain PANE, explain --file SCREEN --agent AGENT, or update")),
    }
}

fn parse_toast(args: &mut Argv) -> Result<Value, Failure> {
    let mut out = json!({});
    for flag in ["pane", "task", "key"] {
        if let Some(value) = args.option(&format!("--{flag}"))? {
            out[flag] = json!(value);
        }
    }
    out["title"] = json!(args.positional().filter(|t| !t.trim().is_empty()).ok_or_else(|| usage("toast", "missing TITLE"))?);
    if let Some(detail) = args.rest() {
        out["detail"] = json!(detail);
    }
    Ok(out)
}

fn parse_agents(args: &mut Argv) -> Result<Value, Failure> {
    let force = args.flag("--force");
    let ask = args.flag("--ask");
    let action = args.positional();
    let request = match action.as_deref() {
        None | Some("status") => json!({ "action": "status" }),
        Some("setup") => json!({ "action": "setup", "ask": ask }),
        Some("update") => json!({ "action": "update", "ask": ask, "force": force }),
        Some("remove") => json!({ "action": "remove" }),
        Some(other) => return Err(usage("agents", &format!("unknown action \"{other}\" (status, setup, update or remove)"))),
    };
    if force && request["action"] != "update" || ask && !matches!(action.as_deref(), Some("setup" | "update")) {
        return Err(usage("agents", "--ask goes with setup or update, --force with update"));
    }
    Ok(request)
}

fn parse_machine(args: &mut Argv) -> Result<Value, Failure> {
    let peer = args.option("--peer")?;
    let tarball = args.option("--tarball")?;
    let action = args.positional();
    let machine = |what: &str, args: &mut Argv| args.positional().ok_or_else(|| usage("machine", &format!("machine {what} needs a MACHINE: its name or key")));
    let request = match action.as_deref() {
        None | Some("list") => json!({ "action": "status" }),
        Some("pair") => match args.positional() {
            None => json!({ "action": "offer" }),
            Some(code) => json!({ "action": "pair", "code": code, "peer": peer }),
        },
        Some("name") => json!({ "action": "name", "name": args.positional().ok_or_else(|| usage("machine", "machine name needs a NAME"))? }),
        Some(what @ ("ping" | "revoke" | "sync")) => json!({ "action": what, "host": machine(what, args)? }),
        Some("update") => {
            let tarball = tarball.as_ref().map(|t| std::path::absolute(t).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| t.clone()));
            json!({ "action": "update", "hosts": args.positionals(), "tarball": tarball })
        }
        Some("addr") => {
            let host = machine("addr", args)?;
            json!({ "action": "addr", "host": host, "addr": args.positional().ok_or_else(|| usage("machine", "machine addr needs a HOST or HOST:PORT"))? })
        }
        Some(other) => return Err(usage("machine", &format!("unknown action \"{other}\" (list, pair, name, ping, addr, update or revoke)"))),
    };
    if peer.is_some() && request["action"] != "pair" {
        return Err(usage("machine", "--peer goes with pair CODE"));
    }
    if tarball.is_some() && request["action"] != "update" {
        return Err(usage("machine", "--tarball goes with update"));
    }
    Ok(request)
}

pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name)
}

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

pub fn sent_as(name: &str) -> &str {
    match name {
        "workspace" => "list",
        name => name,
    }
}

pub fn usage(command: &str, problem: &str) -> Failure {
    let usage = find(command).map(|c| c.usage).unwrap_or("");
    fail(exit::USAGE, format!("{command}: {problem}\nusage: {usage}\nmore: kanna help {command}"))
}

fn pane(args: &mut Argv, command: &str) -> Result<String, Failure> {
    args.positional().ok_or_else(|| usage(command, "missing PANE (a pane name, or an ID like term-1; see `kanna workspace list --all`)"))
}

fn parse_screenshot(args: &mut Argv) -> Result<Value, Failure> {
    let out = match (args.option("--out")?, args.option("-o")?) {
        (Some(_), Some(_)) => return Err(usage("screenshot", "give --out or -o, not both")),
        (out, short) => out.or(short),
    };
    let force = args.flag("--force");
    let mut request = Map::new();
    match (args.option("--window")?, args.positional()) {
        (Some(_), Some(_)) => return Err(usage("screenshot", "give PANE or --window WINDOW, not both")),
        (Some(window), None) => {
            request.insert("window".into(), json!(window));
        }
        (None, Some(pane)) => {
            request.insert("pane".into(), json!(pane));
        }
        (None, None) => from_caller(&mut request),
    }
    match out {
        Some(path) if path.is_empty() => return Err(usage("screenshot", "--out needs a file")),
        Some(path) => {
            request.insert("path".into(), json!(absolute(&path)));
        }
        None => {
            request.insert("dir".into(), json!(std::env::temp_dir().to_string_lossy()));
        }
    }
    if force {
        request.insert("force".into(), json!(true));
    }
    Ok(Value::Object(request))
}

fn target(args: &mut Argv, command: &str) -> Result<Value, Failure> {
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

pub fn file_line(target: &str) -> (String, Option<u64>) {
    if !std::path::Path::new(&absolute(target)).exists() {
        if let Some((file, line)) = target.rsplit_once(':') {
            if let Ok(line) = line.parse::<u64>() {
                if line > 0 && !file.is_empty() {
                    return (file.into(), Some(line));
                }
            }
        }
    }
    (target.into(), None)
}

fn is_source(target: &str) -> bool {
    !target.contains("://") && std::path::Path::new(&absolute(&file_line(target).0)).is_file()
}

fn local_page(target: &str) -> Option<String> {
    if target.contains("://") {
        return None;
    }
    let path = absolute(target);
    std::path::Path::new(&path).is_file().then(|| file_url(&path))
}

fn file_url(path: &str) -> String {
    let encoded: String = path
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("file://{encoded}")
}

pub fn absolute(path: &str) -> String {
    let expanded = match path.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(home) if !home.is_empty() => format!("{}/{rest}", home.trim_end_matches('/')),
            _ => path.to_string(),
        },
        None => path.to_string(),
    };
    let path = std::path::Path::new(&expanded);
    if path.is_absolute() || path.starts_with("~") {
        return expanded;
    }
    std::env::current_dir()
        .map(|dir| dir.join(path))
        .ok()
        .and_then(|p| std::fs::canonicalize(&p).ok().or(Some(p)))
        .map(|p| p.to_string_lossy().into())
        .unwrap_or(expanded)
}

fn parse_open(args: &mut Argv) -> Result<Value, Failure> {
    let mut out = Map::new();
    for (flag, key) in [("--cmd", "cmd"), ("--cwd", "cwd"), ("--name", "name"), ("--split", "split"), ("--size", "size"), ("--window", "window"), ("--app", "app"), ("--device", "device")] {
        if let Some(value) = args.option(flag)? {
            out.insert(key.into(), json!(value));
        }
    }
    if let Some(Value::String(cwd)) = out.get("cwd") {
        let cwd = absolute(cwd);
        out.insert("cwd".into(), json!(cwd));
    }
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
    let diff = args.optional("--diff", |word| !word.starts_with('-') && !is_markdown(word));
    let kind = match args.positional() {
        None => "term".to_string(),
        Some(kind) if ["term", "web", "ios", "md", "code", "diff"].contains(&kind.as_str()) => kind,
        Some(other) if is_markdown(&other) => {
            return Err(usage("open", &format!("to show a markdown file use: kanna open md {other}")));
        }
        Some(other) => {
            return Err(usage("open", &format!("unknown pane type \"{other}\" (term, web, ios, md, code or diff). To run a command use --cmd \"{other}\"")));
        }
    };
    if kind == "web" {
        let url = args.positional().ok_or_else(|| usage("open", "web panes need a URL: kanna open web URL"))?;
        out.insert("url".into(), json!(local_page(&url).unwrap_or(url)));
    }
    if kind == "md" {
        let file = args.positional().ok_or_else(|| usage("open", "markdown panes need a file: kanna open md FILE"))?;
        out.insert("path".into(), json!(absolute(&file)));
    }
    if kind == "code" {
        let file = args.positional().ok_or_else(|| usage("open", "code panes need a file: kanna open code FILE[:LINE]"))?;
        let (file, line) = file_line(&file);
        out.insert("path".into(), json!(absolute(&file)));
        if let Some(line) = line {
            out.insert("line".into(), json!(line));
        }
    }
    if kind == "diff" {
        let folder = out.remove("cwd").and_then(|cwd| cwd.as_str().map(String::from)).unwrap_or_else(|| absolute("."));
        out.insert("path".into(), json!(folder));
        out.insert("args".into(), json!(args.remaining()));
    }
    if kind == "ios" && !out.contains_key("app") {
        return Err(usage("open", "iOS panes need an app: kanna open ios --app MyApp.app (or a bundle ID)"));
    }
    if kind != "ios" && (out.contains_key("app") || out.contains_key("device")) {
        return Err(usage("open", "--app and --device are only for iOS panes: kanna open ios --app APP"));
    }
    if let Some(reference) = diff {
        if kind != "md" {
            return Err(usage("open", "--diff is only for markdown panes: kanna open md FILE --diff REF"));
        }
        let path = out["path"].as_str().unwrap_or_default().to_string();
        out.insert("diff".into(), json!(diff_base(&path, reference)?));
    }
    if history {
        if kind != "web" {
            return Err(usage("open", "--history is only for web panes: kanna open web URL --history"));
        }
        out.insert("history".into(), json!(true));
    }
    out.insert("type".into(), json!(kind));
    from_caller(&mut out);
    Ok(Value::Object(out))
}
