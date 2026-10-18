//! kmux: the command line for the kmux terminal multiplexer. Each command
//! sends one control-protocol request (docs/kmux-spec.md §7) to the running
//! app over its Unix socket.

mod commands;
mod output;

use commands::{Command, COMMANDS, GROUPS};
use kmux_client::{exit, fail, valid_instance, Args, Failure, Kmux, Target};
use serde_json::{json, Value};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("kmux: {}", failure.message);
            ExitCode::from(failure.code)
        }
    }
}

fn run(args: Vec<String>) -> Result<(), Failure> {
    let mut args = Args::new(args);
    let json_output = args.flag("--json");
    let wants_help = args.flag("--help") || args.flag("-h");
    let background = args.flag("--bg");
    let mut target = match args.option("--instance")? {
        Some(name) if !valid_instance(&name) => {
            return Err(fail(exit::USAGE, format!("bad instance name \"{name}\": use letters, digits, - and _ (up to 32)")))
        }
        Some(name) => Target::named(&name),
        None => Target::from_env(),
    };
    target.background |= background;
    // If kmux has to be started, start it with a new window, not the saved layout.
    if args.flag("--fresh") {
        std::env::set_var(target.brand.variable("FRESH"), "1");
    }
    let Some(name) = args.positional() else {
        if args.flag("--version") {
            println!("kmux {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        if let Some(extra) = args.first() {
            return Err(fail(exit::USAGE, format!("unknown option \"{extra}\"\n\n{}", overview(&target))));
        }
        println!("{}", overview(&target));
        return Ok(());
    };

    match name.as_str() {
        "help" => {
            match args.positional() {
                None => println!("{}", overview(&target)),
                Some(topic) if topic == "instances" => println!("{INSTANCES_HELP}"),
                Some(topic) => println!("{}", help(lookup(&topic)?, &target)),
            }
            return Ok(());
        }
        "commands" => {
            let supported = supported(&target);
            if json_output {
                println!("{}", serde_json::to_string_pretty(&describe(supported.as_deref())).unwrap());
            } else {
                for command in COMMANDS {
                    println!("{:<13} {}{}", command.name, command.summary, mark(command, supported.as_deref()));
                }
            }
            return Ok(());
        }
        "instances" => {
            if wants_help {
                println!("{INSTANCES_HELP}");
                return Ok(());
            }
            return instances(&target, json_output);
        }
        "raw" => {
            if wants_help {
                println!("usage: kmux raw REQUEST\n\nSends one control-protocol request as JSON and prints the reply, e.g.\n  kmux raw '{{\"cmd\":\"list\"}}'\n  kmux raw '{{\"cmd\":\"open\",\"args\":{{\"type\":\"term\",\"name\":\"a\"}}}}'");
                return Ok(());
            }
            let text = args.positional().ok_or_else(|| fail(exit::USAGE, "raw: missing REQUEST, e.g. kmux raw '{\"cmd\":\"list\"}'"))?;
            let request: Value = serde_json::from_str(&text).map_err(|e| fail(exit::USAGE, format!("raw: REQUEST is not valid JSON: {e}")))?;
            let cmd = request["cmd"].as_str().ok_or_else(|| fail(exit::USAGE, "raw: REQUEST needs a \"cmd\""))?;
            let reply = Kmux::connect_to(&target)?.call(cmd, request.get("args").cloned().unwrap_or(json!({})))?;
            println!("{reply}");
            return Ok(());
        }
        _ => {}
    }

    let command = lookup(&name)?;
    if wants_help {
        println!("{}", help(command, &target));
        return Ok(());
    }
    let request = (command.parse)(&mut args)?;
    if let Some(extra) = args.first() {
        return Err(commands::usage(command.name, &format!("unexpected argument \"{extra}\"")));
    }
    let listing = command.name == "workspace" && request["action"] == "list";
    if listing && request["all"] != true && request["pane"].is_null() {
        return Err(commands::usage("workspace", "outside a kmux terminal there's no window of yours: add --all"));
    }
    let mut mux = Kmux::connect_to(&target)?;
    let mut reply = match mux.call(if listing { "list" } else { command.name }, if listing { json!({}) } else { request.clone() }) {
        Ok(reply) => reply,
        Err(error) => return Err(explain(command, Failure::from(error), &mut mux)),
    };
    if let (true, Some(pane)) = (listing, request["pane"].as_str()) {
        reply = output::scoped(&reply, pane).ok_or_else(|| fail(exit::NOT_FOUND, format!("no pane \"{pane}\": your terminal isn't one of this kmux's")))?;
    }
    if json_output {
        println!("{reply}");
    } else {
        let text = (command.show)(&request, &reply);
        if !text.is_empty() {
            println!("{text}");
        }
    }
    Ok(())
}

fn lookup(name: &str) -> Result<&'static Command, Failure> {
    commands::find(name).ok_or_else(|| {
        let similar = commands::similar(name);
        let hint = if similar.is_empty() { String::new() } else { format!(" Did you mean: {}?", similar.join(", ")) };
        fail(exit::USAGE, format!("unknown command \"{name}\".{hint} Run `kmux --help` for all commands."))
    })
}

/// Adds the next step to an error from kmux.
fn explain(command: &Command, mut failure: Failure, mux: &mut Kmux) -> Failure {
    match failure.code {
        exit::NOT_FOUND => failure.message += "\nRun `kmux workspace list --all` to see pane names and IDs (term-1, file-1, …), tabs (tab-1, …) and windows (win-1, …).",
        exit::UNSUPPORTED => {
            let supported = mux.call("capabilities", json!({})).map(|c| output::join(&c["commands"])).unwrap_or_default();
            failure.message = format!("the running kmux doesn't support `{}` yet. It supports: {supported}", command.name);
        }
        exit::USAGE => failure.message += &format!("\nusage: {}\nmore: kmux help {}", command.usage, command.name),
        _ => {}
    }
    failure
}

/// The commands the running instance supports, if it is running (help never
/// starts kmux).
fn supported(target: &Target) -> Option<Vec<String>> {
    let reply = Kmux::try_connect(target)?.call("capabilities", json!({})).ok()?;
    Some(reply["commands"].as_array()?.iter().filter_map(|c| c.as_str().map(String::from)).collect())
}

const UNSUPPORTED: &str = "  [not in the running kmux]";

fn mark(command: &Command, supported: Option<&[String]>) -> &'static str {
    match supported {
        Some(names) if !names.iter().any(|n| n == command.name) => UNSUPPORTED,
        _ => "",
    }
}

const INSTANCES_HELP: &str = "kmux instances — List the running kmux instances.\n\
\n\
usage: kmux instances [--json]\n\
\n\
Each instance is a separate kmux app with its own windows and socket. The default\n\
instance listens on kmux.sock; one named work listens on kmux-work.sock beside it.\n\
\n\
Commands go to:\n  \
  1. the instance named by --instance NAME, if given;\n  \
  2. else $KMUX_SOCKET (kmux sets it, with $KMUX_INSTANCE and $KMUX_TERMINAL, in its\n     \
     terminals, so kmux run inside a pane controls that pane's kmux);\n  \
  3. else the instance named by $KMUX_INSTANCE;\n  \
  4. else the default instance.\n\
Commands start the instance if it isn't running. In the app, kmux → New Instance\n\
starts one named 2, 3, ….\n\
\n\
examples:\n  \
  kmux instances\n      \
      Which instances are running, and which one commands go to.\n  \
  kmux --instance work open --name server --cmd \"npm run dev\"\n      \
      Start (or use) the instance named work, and open a pane in it.\n  \
  KMUX_INSTANCE=work kmux workspace list --all\n      \
      The same choice, for every command in a script.";

fn instances(target: &Target, json_output: bool) -> Result<(), Failure> {
    let running: Vec<(Target, Value)> = Target::running()
        .into_iter()
        .map(|t| {
            let list = Kmux::try_connect(&t).and_then(|mut m| m.call("list", json!({})).ok()).unwrap_or(json!({}));
            (t, list)
        })
        .collect();
    let count = |list: &Value, key: &str| list[key].as_array().map_or(0, Vec::len);
    if json_output {
        let rows: Vec<Value> = running
            .iter()
            .map(|(t, list)| json!({
                "name": t.instance, "socket": t.socket, "current": t.socket == target.socket,
                "windows": count(list, "windows"), "panes": count(list, "panes"),
            }))
            .collect();
        println!("{}", json!({ "instances": rows, "current": { "name": target.instance, "socket": target.socket } }));
        return Ok(());
    }
    if running.is_empty() {
        println!("No kmux instances are running. Any command starts one, e.g. kmux open (default) or kmux --instance work open.");
        return Ok(());
    }
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    let width = running.iter().map(|(t, _)| t.instance.len()).max().unwrap_or(0);
    for (t, list) in &running {
        let star = if t.socket == target.socket { "*" } else { " " };
        println!("{star} {:<width$}  {}, {}", t.instance, plural(count(list, "windows"), "window"), plural(count(list, "panes"), "pane"));
    }
    println!("\n* the instance this shell's kmux commands talk to ({}). Pick another with --instance NAME or KMUX_INSTANCE.", target.socket.display());
    Ok(())
}

fn overview(target: &Target) -> String {
    let supported = supported(target);
    let mut out = String::from(
        "kmux — control the kmux terminal multiplexer (windows → tabs → split panes).\n\
         \n\
         usage: kmux [--instance NAME] [--bg] [--fresh] COMMAND [ARGS] [--json]\n",
    );
    for group in GROUPS {
        out += &format!("\n{group}:\n");
        for command in COMMANDS.iter().filter(|c| c.group == *group) {
            out += &format!("  {:<13} {}{}\n", command.name, command.summary, mark(command, supported.as_deref()));
        }
    }
    out += &format!(
        "\nMore:\n  {:<13} {}\n  {:<13} {}\n  {:<13} {}\n  {:<13} {}\n",
        "help COMMAND", "Details, options and examples for one command (or: kmux COMMAND --help).",
        "commands", "All commands with summaries; `kmux commands --json` describes them for scripts.",
        "raw REQUEST", "Send a control-protocol request as JSON, e.g. kmux raw '{\"cmd\":\"list\"}'.",
        "instances", "List the running kmux instances (separate apps, each with its own windows).",
    );
    out += &format!(
        "\nReferring to things:\n  \
         PANE    a pane's name (set with `open --name`) or its ID, e.g. term-1\n  \
         TAB     a tab ID, e.g. tab-1;  WINDOW  a window ID, e.g. win-1  (all shown by `kmux workspace list --all`)\n  \
         FRACTION  1/3, 25% or 0.25\n\
         \n\
         Quick start:\n  \
         kmux open --name server --cmd \"npm run dev\"\n  \
         kmux open --name logs --split right --size 1/3 --cmd \"tail -f app.log\"\n  \
         kmux workspace list --all\n  \
         kmux open --window new --name scratch      (a new window)\n\
         \n\
         {}\n\
         --json prints the reply from kmux as JSON. kmux must be running; the CLI starts it if it isn't\n\
         (in front, or behind your other windows with --bg or KMUX_BG=1). A started kmux brings back the layout\n\
         it saved when it last quit; --fresh (or KMUX_FRESH=1) opens a new window instead.\n\
         Instance: {} at {} (--instance NAME for another; kmux help instances).\n\
         Exit codes: 0 ok, 1 failed, 2 bad usage, 3 kmux not reachable, 4 not found, 5 not supported by the running kmux.",
        match &supported {
            None => "kmux isn't running, so help can't check which commands it supports.".to_string(),
            Some(_) if COMMANDS.iter().any(|c| !mark(c, supported.as_deref()).is_empty()) =>
                format!("Commands marked {} aren't in the running kmux yet.", UNSUPPORTED.trim()),
            Some(_) => "The running kmux supports every command.".to_string(),
        },
        target.instance,
        target.socket.display()
    );
    out
}

fn help(command: &Command, target: &Target) -> String {
    let mut out = format!("kmux {} — {}\n\nusage: {}\n", command.name, command.summary, command.usage);
    if !mark(command, supported(target).as_deref()).is_empty() {
        out += &format!("\nThe running kmux ({}) doesn't support {} yet.\n", target.instance, command.name);
    }
    let options: Vec<(&str, &str)> = command.options.iter().copied().chain([("--json", "Print the reply as JSON.")]).collect();
    out += "\noptions:\n";
    let width = options.iter().map(|(flag, _)| flag.len()).max().unwrap_or(0);
    for (flag, text) in options {
        out += &format!("  {flag:<width$}  {text}\n");
    }
    if !command.notes.is_empty() {
        out += &format!("\n{}\n", command.notes);
    }
    out += "\nexamples:\n";
    for (example, text) in command.examples {
        out += &format!("  {example}\n      {text}\n");
    }
    out.trim_end().to_string()
}

fn describe(supported: Option<&[String]>) -> Value {
    json!({
        "usage": "kmux [--instance NAME] [--bg] [--fresh] COMMAND [ARGS] [--json]",
        "globalOptions": [
            { "flag": "--instance NAME", "description": "Talk to the kmux instance NAME (default: $KMUX_SOCKET, else $KMUX_INSTANCE, else default). See kmux help instances." },
            { "flag": "--bg", "description": "If kmux has to be started, keep it behind your other windows (also KMUX_BG=1)." },
            { "flag": "--fresh", "description": "If kmux has to be started, open a new window instead of the layout it saved when it last quit (also KMUX_FRESH=1)." },
            { "flag": "--json", "description": "Print the reply as JSON." }
        ],
        "running": supported.is_some(),
        "refs": {
            "PANE": "a pane name (open --name) or ID like term-1, file-2 or page-3",
            "TAB": "a tab ID like tab-1",
            "WINDOW": "a window ID like win-1",
            "FRACTION": "1/3, 25% or 0.25"
        },
        "exitCodes": { "0": "ok", "1": "failed", "2": "bad usage", "3": "kmux not reachable", "4": "not found", "5": "not supported by the running kmux" },
        "commands": COMMANDS.iter().map(|c| json!({
            "name": c.name,
            "group": c.group,
            "summary": c.summary,
            "supported": supported.map(|names| names.iter().any(|n| n == c.name)),
            "usage": c.usage,
            "options": c.options.iter().map(|(flag, text)| json!({ "flag": flag, "description": text })).collect::<Vec<_>>(),
            "notes": c.notes,
            "examples": c.examples.iter().map(|(example, text)| json!({ "command": example, "description": text })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Splits an example command line the way a POSIX shell would (enough for
    /// the quoting the examples use).
    fn shell_words(line: &str) -> Vec<String> {
        let mut words = vec![];
        let mut word = String::new();
        let mut quote: Option<char> = None;
        let mut started = false;
        for c in line.chars() {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), c) => word.push(c),
                (None, '"' | '\'') => {
                    quote = Some(c);
                    started = true;
                }
                (None, ' ') => {
                    if started || !word.is_empty() {
                        words.push(std::mem::take(&mut word));
                    }
                    started = false;
                }
                (None, c) => word.push(c),
            }
        }
        if started || !word.is_empty() {
            words.push(word);
        }
        words
    }

    fn parse(line: &str) -> Result<(String, Value), Failure> {
        let words = shell_words(line);
        assert_eq!(words[0], "kmux");
        let mut args = Args::new(words[1..].to_vec());
        args.flag("--json");
        let name = args.positional().unwrap();
        let command = lookup(&name)?;
        let request = (command.parse)(&mut args)?;
        if let Some(extra) = args.first() {
            return Err(commands::usage(command.name, &format!("unexpected argument \"{extra}\"")));
        }
        Ok((command.name.to_string(), request))
    }

    #[test]
    fn every_documented_example_parses() {
        for command in COMMANDS {
            assert!(!command.examples.is_empty(), "{} has no examples", command.name);
            for (example, _) in command.examples {
                let (name, _) = parse(example).unwrap_or_else(|f| panic!("{example}: {}", f.message));
                assert_eq!(name, command.name, "{example}");
            }
        }
    }

    #[test]
    fn requests() {
        let cases = [
            ("kmux open", json!({ "type": "term" })),
            ("kmux open --name logs --split right --size 1/3 --cmd 'tail -f a.log'", json!({ "type": "term", "name": "logs", "split": "right", "size": "1/3", "cmd": "tail -f a.log" })),
            ("kmux open web localhost:3000 --split down", json!({ "type": "web", "url": "localhost:3000", "split": "down" })),
            ("kmux open --tab --no-wait", json!({ "type": "term", "tab": true, "wait": false })),
            ("kmux close logs", json!({ "pane": "logs" })),
            ("kmux close --window win-2", json!({ "window": "win-2" })),
            ("kmux rename-tab tab-1 my servers", json!({ "tab": "tab-1", "title": "my servers" })),
            ("kmux send server npm test", json!({ "pane": "server", "text": "npm test" })),
            ("kmux move logs --to server --side bottom", json!({ "pane": "logs", "to": "server", "side": "bottom" })),
            ("kmux move-tab tab-2 --index 0", json!({ "tab": "tab-2", "index": 0 })),
            ("kmux open web a.test --history", json!({ "type": "web", "url": "a.test", "history": true })),
            ("kmux navigate site --back", json!({ "pane": "site", "back": true })),
            ("kmux open md /tmp/a.md", json!({ "type": "md", "path": "/tmp/a.md" })),
            ("kmux navigate spec /tmp/b.md", json!({ "pane": "spec", "path": "/tmp/b.md" })),
            ("kmux open --name agent --dock", json!({ "type": "term", "name": "agent", "dock": "left" })),
            ("kmux open --dock right --cmd claude", json!({ "type": "term", "dock": "right", "cmd": "claude" })),
            ("kmux open --dock md /tmp/a.md", json!({ "type": "md", "dock": "left", "path": "/tmp/a.md" })),
            ("kmux open web a.test --dock=right --window new", json!({ "type": "web", "url": "a.test", "dock": "right", "window": "new" })),
            ("kmux open --name agent --dock --frozen", json!({ "type": "term", "name": "agent", "dock": "left", "frozen": true })),
            ("kmux move agent --dock", json!({ "pane": "agent", "dock": "left" })),
            ("kmux move agent --dock right --window win-2", json!({ "pane": "agent", "dock": "right", "window": "win-2" })),
            ("kmux move agent --tab", json!({ "pane": "agent", "tab": "new" })),
            ("kmux move agent --tab tab-3", json!({ "pane": "agent", "tab": "tab-3" })),
            ("kmux move agent --tab --window win-2", json!({ "pane": "agent", "tab": "new", "window": "win-2" })),
            ("kmux workspace new", json!({ "action": "new" })),
            ("kmux workspace new checkout", json!({ "action": "new", "name": "checkout" })),
            ("kmux workspace hide ws-1", json!({ "action": "hide", "workspace": "ws-1" })),
            ("kmux workspace close checkout", json!({ "action": "close", "workspace": "checkout" })),
            ("kmux window assign win-2 ws-1", json!({ "action": "assign", "window": "win-2", "workspace": "ws-1" })),
            ("kmux window free win-2", json!({ "action": "free", "window": "win-2" })),
            ("kmux workspace list --all", json!({ "action": "list", "all": true })),
        ];
        for (line, expected) in cases {
            let (_, request) = parse(line).unwrap_or_else(|f| panic!("{line}: {}", f.message));
            assert_eq!(request, expected, "{line}");
        }
    }

    #[test]
    fn usage_errors_say_what_to_do() {
        for (line, hint) in [
            ("kmux close", "PANE, --tab TAB or --window WINDOW"),
            ("kmux zoom", "missing PANE"),
            ("kmux open top", "--cmd \"top\""),
            ("kmux open web", "need a URL"),
            ("kmux move logs", "say where"),
            ("kmux zoom a b", "unexpected argument \"b\""),
            ("kmux open --history", "only for web panes"),
            ("kmux open md", "need a file"),
            ("kmux open notes.md", "kmux open md notes.md"),
            ("kmux navigate site --back x", "one of URL"),
            ("kmux workspce list", "Did you mean: workspace"),
            ("kmux rename tab-1 x", "Did you mean: rename-tab"),
            ("kmux open --dock --split right", "--dock goes on its own"),
            ("kmux open --dock --tab", "--dock goes on its own"),
            ("kmux move agent --dock --tab", "--dock goes on its own"),
            ("kmux move agent --dock --to b", "--dock goes on its own"),
            ("kmux open --frozen", "--frozen goes with --dock"),
            ("kmux workspace", "say new, hide, show, close or list"),
            ("kmux workspace hide", "missing WORKSPACE"),
            ("kmux workspace park ws-1", "unknown action \"park\""),
            ("kmux window assign win-2", "missing WORKSPACE"),
            ("kmux window free", "missing WINDOW"),
        ] {
            let failure = parse(line).expect_err(line);
            assert_eq!(failure.code, exit::USAGE, "{line}");
            assert!(failure.message.contains(hint), "{line}: {}", failure.message);
        }
    }

    #[test]
    fn layouts_print_as_expressions() {
        let layout = json!({ "split": "row", "children": [
            { "pane": "a", "size": "2/3" },
            { "split": "column", "size": "1/3", "children": [{ "pane": "b", "size": "1/2" }, { "pane": "c", "size": "1/2" }] }
        ] });
        assert_eq!(output::expression(&layout, true), "a:2/3 | (b:1/2 / c:1/2):1/3");
    }

    #[test]
    fn docked_panes_print_on_their_own_line() {
        let reply = json!({
            "windows": [{ "id": "win-1", "key": true, "focused": "agent", "dock": { "pane": "agent", "edge": "left", "width": "2/5" },
                          "tabs": [{ "id": "tab-1", "title": "Tab 1", "active": true, "layout": { "pane": "term-2" } }] }],
            "panes": [{ "id": "term-1", "name": "agent", "type": "term", "state": "running", "cmd": "claude" }, { "id": "term-2", "name": null, "type": "term", "state": "running" }]
        });
        let text = output::list(&reply);
        assert!(text.contains("\n  dock: term-1 (agent)  left 2/5\n  *tab-1 \"Tab 1\"  term-2"), "{text}");
        let opened = json!({ "pane": { "id": "term-1", "name": "agent", "state": "running" }, "window": "win-1", "tab": null, "dock": "left" });
        assert_eq!(output::opened(&json!({}), &opened), "opened term-1 (agent) docked left in win-1: running");
        assert_eq!(output::opened(&json!({ "frozen": true }), &opened), "opened term-1 (agent) docked left, frozen, in win-1: running");
        let frozen = json!({ "windows": [{ "id": "win-1", "dock": { "pane": "agent", "edge": "left", "width": "2/5", "frozen": true }, "tabs": [] }],
                             "panes": [{ "id": "term-1", "name": "agent", "type": "term", "state": "running" }] });
        assert!(output::list(&frozen).contains("\n  dock: term-1 (agent)  left 2/5  frozen"), "{}", output::list(&frozen));
        let moved = json!({ "window": "win-1", "dock": { "pane": "agent", "edge": "right", "width": "1/3" } });
        assert_eq!(output::moved(&moved), "agent is docked right in win-1 (width 1/3)");
    }

    #[test]
    fn windows_print_by_workspace() {
        let tab = |id: &str, pane: &str| json!([{ "id": id, "title": "Tab 1", "active": true, "layout": { "pane": pane } }]);
        let reply = json!({
            "windows": [
                { "id": "win-1", "key": true, "tabs": tab("tab-1", "a") },
                { "id": "win-2", "key": false, "workspace": "ws-1", "hidden": true, "tabs": tab("tab-2", "b") },
                { "id": "win-3", "key": false, "tabs": tab("tab-3", "c") }
            ],
            "workspaces": [
                { "id": "ws-1", "name": "checkout", "hidden": true, "windows": ["win-2"] },
                { "id": "ws-2", "name": null, "hidden": false, "windows": [] }
            ],
            "panes": []
        });
        let text = output::list(&reply);
        assert!(text.starts_with("*win-1\n  *tab-1 \"Tab 1\"  a\n win-3\n  *tab-3 \"Tab 1\"  c\nworkspace ws-1 (checkout)  hidden\n win-2\n  *tab-2 \"Tab 1\"  b\nworkspace ws-2  no windows\n"), "{text}");
        let pane = json!({ "id": "term-5", "name": null, "state": "running" });
        let hidden = json!({ "pane": pane, "window": "win-2", "tab": "tab-9", "workspace": { "id": "ws-1", "name": "checkout", "hidden": true } });
        assert_eq!(output::opened(&json!({}), &hidden), "opened term-5 in ws-1's win-2 (hidden), tab-9: running");
        let shown = json!({ "pane": pane, "window": "win-2", "tab": "tab-9", "workspace": { "id": "ws-1", "name": "checkout", "hidden": false } });
        assert_eq!(output::opened(&json!({}), &shown), "opened term-5 in ws-1's win-2, tab-9: running");
        let assign = json!({ "action": "assign" });
        assert_eq!(output::window(&assign, &json!({ "window": "win-2", "workspace": { "id": "ws-1", "hidden": true }, "shown": false })),
                   "assigned win-2 to ws-1 (hidden: it opens again when ws-1 is shown)");
        assert_eq!(output::window(&json!({ "action": "free" }), &json!({ "window": "win-2", "was": { "id": "ws-1", "hidden": true }, "shown": true })),
                   "win-2 is free (out of ws-1, and open again)");
        assert_eq!(output::workspace(&json!({ "action": "hide" }), &json!({ "workspace": { "id": "ws-1", "name": "checkout", "hidden": true }, "windows": ["win-2"] })),
                   "hid ws-1 (checkout): win-2 off screen, their panes still running");
        assert_eq!(output::workspace(&json!({ "action": "close" }), &json!({ "workspace": "ws-1", "closed": [] })), "closed ws-1 (it had no panes)");
        assert_eq!(output::workspace(&json!({ "action": "close" }), &json!({ "workspace": "ws-1", "closed": ["term-2", "term-3"] })), "closed ws-1 (panes term-2, term-3)");
    }

    #[test]
    fn workspace_list_is_your_window_or_your_workspace_s_windows() {
        let tab = |id: &str, pane: &str| json!([{ "id": id, "title": "Tab 1", "active": true, "layout": { "pane": pane } }]);
        let reply = json!({
            "windows": [
                { "id": "win-1", "key": true, "tabs": tab("tab-1", "a") },
                { "id": "win-2", "key": false, "workspace": "ws-1", "hidden": true, "tabs": tab("tab-2", "b") },
                { "id": "win-3", "key": false, "workspace": "ws-1", "hidden": true, "tabs": tab("tab-3", "term-3") }
            ],
            "workspaces": [{ "id": "ws-1", "name": "checkout", "hidden": true, "windows": ["win-2", "win-3"] }],
            "panes": [{ "id": "term-1", "name": "a", "type": "term", "state": "running" }, { "id": "term-2", "name": "b", "type": "term", "state": "running", "workspace": "ws-1" },
                      { "id": "term-3", "name": null, "type": "term", "state": "running", "workspace": "ws-1" }, { "id": "term-4", "name": null, "type": "term", "state": "running" }]
        });
        let list = json!({ "action": "list" });
        let mine = output::workspace(&list, &output::scoped(&reply, "term-3").unwrap());
        assert!(mine.starts_with("workspace ws-1 (checkout)  hidden\n win-2\n  *tab-2 \"Tab 1\"  b\n win-3\n  *tab-3 \"Tab 1\"  term-3\n\nPANE"), "{mine}");
        assert!(!mine.contains("term-1  a"), "{mine}");
        let free = output::workspace(&list, &output::scoped(&reply, "a").unwrap());
        assert!(free.starts_with("*win-1\n  *tab-1 \"Tab 1\"  a\n\nPANE"), "{free}");
        assert_eq!(output::workspace(&list, &output::scoped(&reply, "term-4").unwrap()), "term-4 isn't in a window");
        assert!(output::scoped(&reply, "term-9").is_none());
        assert_eq!(output::workspace(&json!({ "action": "list", "all": true }), &reply), output::list(&reply));
    }
}
