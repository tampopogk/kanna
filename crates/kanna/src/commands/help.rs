use super::{lookup, sent_as, Command, COMMANDS, GROUPS, LOCAL};
use crate::args::Argv;
use crate::target::{self, Backend, Target};
use kanna_client::Failure;
use serde_json::{json, Value};

pub fn run(args: &mut Argv, target: &Target) -> Result<(), Failure> {
    match args.positional() {
        None => println!("{}", overview(target)),
        Some(topic) if topic == "instances" => println!("{}", super::instances::HELP),
        Some(topic) if topic == "raw" => println!("{}", super::raw::HELP),
        Some(topic) if topic == "completions" => println!("{}", super::completions::HELP),
        Some(topic) => println!("{}", help(lookup(&topic)?, target)),
    }
    Ok(())
}

pub fn commands(target: &Target, json_output: bool) -> Result<(), Failure> {
    let supported = supported(target);
    if json_output {
        println!("{}", serde_json::to_string_pretty(&describe(supported.as_deref())).unwrap());
    } else {
        for command in COMMANDS {
            println!("{:<13} {}{}", command.name, command.summary, mark(command, supported.as_deref(), target));
        }
    }
    Ok(())
}

fn supported(target: &Target) -> Option<Vec<String>> {
    let reply = target::capabilities(target)?;
    Some(reply["commands"].as_array()?.iter().filter_map(|c| c.as_str().map(String::from)).collect())
}

fn unsupported_mark(target: &Target) -> String {
    format!("  [not in the running {}]", target.name())
}

fn mark(command: &Command, supported: Option<&[String]>, target: &Target) -> String {
    match supported {
        Some(names) if !names.iter().any(|n| n == sent_as(command.name)) && !LOCAL.contains(&command.name) => unsupported_mark(target),
        _ => String::new(),
    }
}

pub fn overview(target: &Target) -> String {
    let supported = supported(target);
    let mut out = String::from(
        "kanna — control the Kanna terminal multiplexer (windows → tabs → split panes).\n\
         \n\
         usage: kanna [--mux kanna|kmux] [--instance NAME] [--host MACHINE] [--bg] [--fresh] COMMAND [ARGS] [--json] [-- TEXT]\n\
         \x20      kanna PATH                           (the Desk on folder PATH's project, opening one if none is open)\n",
    );
    for group in GROUPS {
        out += &format!("\n{group}:\n");
        for command in COMMANDS.iter().filter(|c| c.group == *group) {
            out += &format!("  {:<13} {}{}\n", command.name, command.summary, mark(command, supported.as_deref(), target));
        }
    }
    out += &format!(
        "\nMore:\n  {:<13} {}\n  {:<13} {}\n  {:<13} {}\n  {:<13} {}\n  {:<13} {}\n  {:<13} {}\n",
        "help COMMAND", "Details, options and examples for one command (or: kanna COMMAND --help).",
        "commands", "All commands with summaries; `kanna commands --json` describes them for scripts.",
        "raw REQUEST", "Send a control-protocol request as JSON, e.g. kanna raw '{\"cmd\":\"list\"}'.",
        "instances", "For developing Kanna: list the running instances (dev builds and test daemons).",
        "completions", "Tab completion for zsh or bash: source <(kanna completions zsh) in ~/.zshrc.",
        "config schema", "The JSON Schema of Kanna's settings (~/.kanna-v4/config.json, a repo's .kanna/config.json).",
    );
    let name = target.name();
    let running = match &supported {
        None => format!("{name} isn't running, so help can't check which commands it supports."),
        Some(_) if COMMANDS.iter().any(|c| !mark(c, supported.as_deref(), target).is_empty()) => {
            format!("Commands marked {} aren't in the running {name} yet.", unsupported_mark(target).trim())
        }
        Some(_) => format!("The running {name} supports every command."),
    };
    let backend = match target.backend {
        Backend::Kanna => String::new(),
        Backend::Kmux => "Backend: kmux (--mux kmux), not Kanna.\n".into(),
    };
    out += &format!(
        "\nReferring to things:\n  \
         PANE    a pane's name (set with `open --name`) or its ID, e.g. p1, on this machine;\n  \
         \x20       or a task (task-ac2b, ac2b) for its agent's pane, on whichever machine runs it\n  \
         TAB     a tab ID, e.g. t1;  WINDOW  a window ID, e.g. w1  (all shown by `kanna workspace list --all`)\n  \
         FRACTION  1/3, 25% or 0.25\n\
         \n\
         Quick start:\n  \
         kanna open --name server --cmd \"npm run dev\"\n  \
         kanna open --name logs --split right --size 1/3 --cmd \"tail -f app.log\"\n  \
         kanna workspace list --all\n  \
         kanna read server\n  \
         kanna open --window new --name scratch      (a new window)\n\
         \n\
         {running}\n\
         --json prints the reply from Kanna as JSON; -- ends the options (kanna send p1 -- --json types --json).\n\
         Terminals and the layout live in Kanna's daemon and keep going with Kanna.app closed: every command but\n\
         screenshot and debug.* works without the app. Those start it if it isn't running (in front, or behind your\n\
         other windows with --bg or KANNA_BG=1), as does any command when nothing answers. A started Kanna.app shows\n\
         the layout as it was left; a daemon started with --fresh (or KANNA_FRESH=1) forgets the saved layout.\n\
         With KANNA_SOCKET set, kanna never starts the app.\n\
         {backend}\
         Instance: {} of {} at {} (for developing Kanna, --instance NAME picks another; kanna help instances).\n\
         Exit codes: 0 ok, 1 failed, 2 bad usage, 3 Kanna not reachable, 4 not found, 5 not supported by the running Kanna,\n\
         6 needs Kanna.app, which isn't attached and wasn't started.",
        target.instance,
        target.build.app_name(),
        target.socket.display()
    );
    out
}

pub fn help(command: &Command, target: &Target) -> String {
    let mut out = format!("kanna {} — {}\n\nusage: {}\n", command.name, command.summary, command.usage);
    if !mark(command, supported(target).as_deref(), target).is_empty() {
        out += &format!("\nThe running {} ({}) doesn't support {} yet.\n", target.name(), target.instance, command.name);
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
        "usage": "kanna [--mux kanna|kmux] [--instance NAME] [--bg] [--fresh] COMMAND [ARGS] [--json] [-- TEXT]",
        "globalOptions": [
            { "flag": "--mux kanna|kmux", "description": "Talk to Kanna (default; also KANNA_MUX) or to a kmux you run." },
            { "flag": "--instance NAME", "description": "For developing and testing Kanna: talk to the instance NAME (default: $KANNA_SOCKET, else $KANNA_INSTANCE, else default). See kanna help instances." },
            { "flag": "--bg", "description": "If Kanna.app has to be started, keep it behind your other windows (also KANNA_BG=1)." },
            { "flag": "--fresh", "description": "If Kanna's daemon has to be started, start it without the layout it saved (also KANNA_FRESH=1)." },
            { "flag": "--json", "description": "Print the reply as JSON." },
            { "flag": "--", "description": "End the options: what follows is text, e.g. kanna send term-1 -- --json." }
        ],
        "running": supported.is_some(),
        "refs": {
            "PANE": "a pane name (open --name) or ID like term-1, file-2 or page-3",
            "TAB": "a tab ID like tab-1",
            "WINDOW": "a window ID like win-1",
            "FRACTION": "1/3, 25% or 0.25"
        },
        "exitCodes": {
            "0": "ok", "1": "failed", "2": "bad usage", "3": "Kanna not reachable", "4": "not found", "5": "not supported by the running Kanna",
            "6": "needs Kanna.app, which isn't attached and wasn't started"
        },
        "commands": COMMANDS.iter().map(|c| json!({
            "name": c.name,
            "group": c.group,
            "summary": c.summary,
            "supported": supported.map(|names| names.iter().any(|n| n == sent_as(c.name)) || LOCAL.contains(&c.name)),
            "usage": c.usage,
            "options": c.options.iter().map(|(flag, text)| json!({ "flag": flag, "description": text })).collect::<Vec<_>>(),
            "notes": c.notes,
            "examples": c.examples.iter().map(|(example, text)| json!({ "command": example, "description": text })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}
