pub mod args;
pub mod build;
pub mod commands;
pub mod complete;
pub mod output;
pub mod target;
pub mod threads;

use args::Argv;
use commands::{lookup, Call, Command, Invocation, COMMANDS, GROUPS};
use kanna_client::{exit, fail, Failure};
use serde_json::{json, Value};
use target::{process_env, Backend, Choice, Env, Session, Target};

pub fn main() -> std::process::ExitCode {
    let argv0 = std::env::args().next().unwrap_or_default();
    if let Some(tool) = build::shim_name(&argv0) {
        let args: Vec<String> = std::env::args().skip(1).collect();
        return match shim(tool, &argv0, args) {
            Ok(code) => std::process::ExitCode::from(code.clamp(0, 255) as u8),
            Err(failure) => {
                eprintln!("kanna build: {}", failure.message);
                std::process::ExitCode::from(failure.code)
            }
        };
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "build") {
        if args.get(1).is_some_and(|a| a == "--help" || a == "-h") {
            println!("{}", build::HELP);
            return std::process::ExitCode::SUCCESS;
        }
        let code = build::parse(&args[1..]).and_then(|options| build::run(build_socket(&process_env)?, options));
        return match code {
            Ok(code) => std::process::ExitCode::from(code.clamp(0, 255) as u8),
            Err(failure) => {
                eprintln!("kanna: {}", failure.message);
                std::process::ExitCode::from(failure.code)
            }
        };
    }
    if args.first().is_some_and(|a| a == "guests") {
        return match commands::guests::run(&args[1..], &process_env) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(failure) => {
                eprintln!("kanna: {}", failure.message);
                std::process::ExitCode::from(failure.code)
            }
        };
    }
    match run(args, &process_env) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("kanna: {}", failure.message);
            std::process::ExitCode::from(failure.code)
        }
    }
}

fn build_socket(env: Env) -> Result<std::path::PathBuf, Failure> {
    let target = Target::resolve(&kanna_build::Build::current(), Choice { mux: None, instance: None, background: false, fresh: false }, env)?;
    Ok(target.socket)
}

fn swift_goes_to_the_mac() -> bool {
    let Ok(here) = std::env::current_dir() else { return false };
    let Ok(worktree) = kanna_guest::client::worktree(&here) else { return false };
    let config = std::fs::read(worktree.root.join(".kanna/config.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).unwrap_or(Value::Null);
    config["build"]["swift"] == "mac"
}

fn shim(tool: &str, argv0: &str, args: Vec<String>) -> Result<i32, Failure> {
    if tool == "swift" && !swift_goes_to_the_mac() {
        use std::os::unix::process::CommandExt;
        let me = std::fs::canonicalize(std::env::current_exe().unwrap_or_default()).unwrap_or_default();
        let shim_dir = std::path::Path::new(argv0).parent().and_then(|d| d.canonicalize().ok());
        let path = std::env::var("PATH").unwrap_or_default();
        let next = path.split(':').map(|dir| std::path::Path::new(dir).join(tool)).find(|candidate| candidate.is_file() && candidate.parent().and_then(|d| d.canonicalize().ok()) != shim_dir && std::fs::canonicalize(candidate).ok().as_ref() != Some(&me));
        let Some(next) = next else { return Err(fail(exit::FAILED, "swift: there's no swift on this machine, and this repo doesn't send swift to the build Mac (build.swift in .kanna/config.json)")) };
        let error = std::process::Command::new(next).args(args).exec();
        return Err(fail(exit::FAILED, format!("swift: {error}")));
    }
    let argv = std::iter::once(tool.to_string()).chain(args).collect();
    build::run(build_socket(&process_env)?, build::Options { mac: None, fetch: fetch_policy(), ui: std::env::var("KANNA_BUILD_UI").is_ok_and(|v| v == "1"), argv })
}

fn fetch_policy() -> Vec<String> {
    std::env::var("KANNA_BUILD_FETCH").map(|v| v.split(':').filter(|p| !p.is_empty()).map(String::from).collect()).unwrap_or_default()
}

pub fn run(args: Vec<String>, env: Env) -> Result<(), Failure> {
    if args.first().is_some_and(|a| a == "__complete") {
        let start = if args.get(1).is_some_and(|a| a == "--") { 2 } else { 1 };
        let words = if args.len() > start { args[start..].to_vec() } else { vec![String::new()] };
        for line in complete::lines(&complete::wants(&words), &words, env) {
            println!("{line}");
        }
        return Ok(());
    }
    let mut args = Argv::new(args);
    let json_output = args.flag("--json");
    let wants_help = args.flag("--help") || args.flag("-h");
    let background = args.flag("--bg");
    let fresh = args.flag("--fresh");
    let instance = args.option("--instance")?;
    let mux = args.option("--mux")?;
    let host = args.option("--host")?.or_else(|| env("KANNA_HOST"));
    let target = Target::resolve(&kanna_build::Build::current(), Choice { mux, instance, background, fresh }, env)?;
    let invocation = Invocation { target: &target, env, host: host.clone(), json_output };
    let Some(name) = args.positional() else {
        if args.flag("--version") {
            println!("kanna {} ({})", kanna_build::VERSION, target.build.app_name());
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
                Some(topic) if topic == "instances" => println!("{}", commands::instances::HELP),
                Some(topic) if topic == "raw" => println!("{}", commands::raw::HELP),
                Some(topic) if topic == "completions" => println!("{}", commands::completions::HELP),
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
                    println!("{:<13} {}{}", command.name, command.summary, mark(command, supported.as_deref(), &target));
                }
            }
            return Ok(());
        }
        "config" => return commands::config::run(&mut args),
        "completions" => return commands::completions::run(&mut args),
        "instances" => return commands::instances::run(&mut args, &invocation, wants_help),
        "raw" => return commands::raw::run(&mut args, &invocation, wants_help),
        _ => {}
    }

    let (command, request) = match project_folder(&name) {
        Some(folder) => (lookup("task")?, json!({ "action": "desk", "project": folder })),
        None => {
            let command = lookup(&name)?;
            if wants_help {
                println!("{}", help(command, &target));
                return Ok(());
            }
            (command, (command.parse)(&mut args)?)
        }
    };
    if let Some(extra) = args.first() {
        return Err(commands::usage(command.name, &format!("unexpected argument \"{extra}\"")));
    }
    if let Some(done) = command.flow.offline.and_then(|offline| offline(&request, &invocation)) {
        return done;
    }
    if let Some(flags) = command.flow.unplaced.and_then(|unplaced| unplaced(&request)).filter(|_| target.backend == Backend::Kanna) {
        return Err(commands::usage(command.name, &format!("outside a Kanna terminal there's no window of yours: {flags}")));
    }
    let mut mux = Session::connect(&target, env)?;
    mux.host = host;
    if let Some(done) = command.flow.online.and_then(|online| online(&request, &mut mux, json_output)) {
        return done;
    }
    let mut request = request;
    if target.backend == Backend::Kanna && commands::caller().is_none() && matches!(command.wire(&request), "focus" | "open" | "move" | "move-tab" | "desk") {
        request["user"] = json!(true);
    }
    let mut call = match command.flow.before {
        Some(before) => before(&mut request, &mut mux)?,
        None => Call::default(),
    };
    let mut reply = match mux.call(command.wire(&request), request.clone()) {
        Ok(reply) => reply,
        Err(mut failure) => {
            if failure.message.contains("task-") {
                let shorts = mux.call("tasks", json!({ "action": "list" })).map(|b| output::shorts(&b)).unwrap_or_default();
                failure.message = output::short_text(&failure.message, &shorts);
            }
            return Err(explain(command, failure, &mut mux, &target));
        }
    };
    if let Some(after) = command.flow.after {
        after(&request, &mut reply, &mut mux, &mut call)?;
    }
    if json_output {
        println!("{reply}");
    } else {
        if let Some(id) = &call.task {
            for value in [&mut request, &mut reply] {
                if value["task"].is_string() {
                    value["task"] = json!(id);
                }
            }
        }
        let mut shorts = std::mem::take(&mut call.shorts);
        if !call.listed && (request.to_string().contains("task-") || reply.to_string().contains("task-")) {
            shorts.extend(mux.call("tasks", json!({ "action": "list" })).map(|b| output::shorts(&b)).unwrap_or_default());
        }
        shorts.extend(output::shorts(&reply));
        output::shorten(&mut request, &shorts);
        output::shorten(&mut reply, &shorts);
        let text = (command.show)(&request, &reply);
        if !text.is_empty() {
            println!("{text}");
        }
        for warning in &call.warnings {
            eprintln!("{warning}");
        }
    }
    Ok(())
}

/// `kanna PATH`: a word that isn't a command but is a folder brings forward its project's Desk, or opens one.
fn project_folder(name: &str) -> Option<String> {
    if commands::find(name).is_some() {
        return None;
    }
    let folder = std::fs::canonicalize(commands::absolute(name)).ok().filter(|p| p.is_dir())?;
    Some(folder.to_string_lossy().into_owned())
}

fn explain(command: &Command, mut failure: Failure, mux: &mut Session, target: &Target) -> Failure {
    match failure.code {
        exit::NOT_FOUND if !failure.message.contains("your machines") => failure.message += "\nRun `kanna workspace list --all` to see pane names and IDs (term-1, file-1, …), tabs (tab-1, …) and windows (win-1, …).",
        exit::UNSUPPORTED => {
            let supported = mux.call("capabilities", json!({})).map(|c| output::join(&c["commands"])).unwrap_or_default();
            failure.message = format!("the running {} doesn't support `{}` yet. It supports: {supported}", target.name(), command.name);
        }
        exit::USAGE => failure.message += &format!("\nusage: {}\nmore: kanna help {}", command.usage, command.name),
        _ => {}
    }
    failure
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
        Some(names) if !names.iter().any(|n| n == commands::sent_as(command.name)) && !commands::LOCAL.contains(&command.name) => unsupported_mark(target),
        _ => String::new(),
    }
}

fn overview(target: &Target) -> String {
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
         PANE    a pane's name (set with `open --name`) or its ID, e.g. p1\n  \
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

fn help(command: &Command, target: &Target) -> String {
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

pub fn describe(supported: Option<&[String]>) -> Value {
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
            "supported": supported.map(|names| names.iter().any(|n| n == commands::sent_as(c.name)) || commands::LOCAL.contains(&c.name)),
            "usage": c.usage,
            "options": c.options.iter().map(|(flag, text)| json!({ "flag": flag, "description": text })).collect::<Vec<_>>(),
            "notes": c.notes,
            "examples": c.examples.iter().map(|(example, text)| json!({ "command": example, "description": text })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}
