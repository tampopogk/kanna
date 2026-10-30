pub mod agents;
pub mod args;
pub mod commands;
pub mod complete;
pub mod diagrams;
pub mod machine;
pub mod output;
pub mod target;
pub mod threads;

use args::Argv;
use commands::{Command, COMMANDS, GROUPS};
use kanna_client::{exit, fail, Failure};
use serde_json::{json, Value};
use target::{process_env, Backend, Choice, Env, Session, Target};

pub fn main() -> std::process::ExitCode {
    match run(std::env::args().skip(1).collect(), &process_env) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("kanna: {}", failure.message);
            std::process::ExitCode::from(failure.code)
        }
    }
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
                Some(topic) if topic == "instances" => println!("{INSTANCES_HELP}"),
                Some(topic) if topic == "raw" => println!("{RAW_HELP}"),
                Some(topic) if topic == "completions" => println!("{COMPLETIONS_HELP}"),
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
        "config" => {
            match args.positional().as_deref() {
                Some("schema") => print!("{CONFIG_SCHEMA}"),
                _ => println!("{CONFIG_HELP}"),
            }
            return Ok(());
        }
        "completions" => {
            match args.positional().as_deref() {
                Some("zsh") => print!("{}", complete::ZSH),
                Some("bash") => print!("{}", complete::BASH),
                _ => println!("{COMPLETIONS_HELP}"),
            }
            return Ok(());
        }
        "instances" => {
            if wants_help {
                println!("{INSTANCES_HELP}");
                return Ok(());
            }
            match args.positional().as_deref() {
                Some("prune") => return prune(env),
                Some("stop") => return stop(&target, json_output),
                _ => {}
            }
            return instances(&target, json_output, env);
        }
        "raw" => {
            if wants_help {
                println!("{RAW_HELP}");
                return Ok(());
            }
            let text = args.positional().ok_or_else(|| fail(exit::USAGE, "raw: missing REQUEST, e.g. kanna raw '{\"cmd\":\"list\"}'"))?;
            let request: Value = serde_json::from_str(&text).map_err(|e| fail(exit::USAGE, format!("raw: REQUEST is not valid JSON: {e}")))?;
            let cmd = request["cmd"].as_str().ok_or_else(|| fail(exit::USAGE, "raw: REQUEST needs a \"cmd\""))?;
            let mut session = Session::connect(&target, env)?;
            session.host = host;
            let reply = session.call(cmd, request.get("args").cloned().unwrap_or(json!({})))?;
            println!("{reply}");
            return Ok(());
        }
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
    if request.get("local").is_some() {
        let mut callers: Vec<String> = commands::caller().into_iter().collect();
        let terminal = callers.first().cloned();
        let listed = terminal.as_ref().and_then(|_| Session::connect(&target, env).ok()?.call("list", json!({})).ok());
        let pane = listed.as_ref().and_then(|l| l["panes"].as_array()?.iter().find(|p| p["terminal"].as_str() == terminal.as_deref())?["id"].as_str().map(String::from));
        callers.extend(pane);
        return threads::run(&request, json_output, &callers);
    }
    if command.name == "workspace" && request["all"] != true && request["pane"].is_null() {
        return Err(commands::usage("workspace", "outside a Kanna terminal there's no workspace of yours: add --all"));
    }
    if let Some(flags) = unplaced(command.name, &request).filter(|_| target.backend == Backend::Kanna) {
        return Err(commands::usage(command.name, &format!("outside a Kanna terminal there's no window of yours: {flags}")));
    }
    if command.name == "diagrams" {
        return diagrams::run(&request, json_output);
    }
    let mut mux = Session::connect(&target, env)?;
    mux.host = host;
    if command.name == "machine" && request["action"] == "offer" {
        return machine::offer(&mut mux, json_output);
    }
    if command.name == "comment" && request["doc"] == true {
        return threads::comment(&request, &mut mux, json_output);
    }
    if command.name == "agents" && !json_output && matches!(request["action"].as_str(), Some("setup" | "update")) {
        let tool = agents::merge_tool();
        let mut call = |args: Value| mux.call("agents", args);
        let mut merge = |path: &std::path::Path, base: &std::path::Path, new: &std::path::Path| agents::merge_in_editor(path, base, new, &tool);
        let stdin = std::io::stdin();
        let mut io = agents::Io { call: &mut call, input: &mut stdin.lock(), out: &mut std::io::stdout(), merge: &mut merge, tool: tool.clone() };
        return agents::run(&request, &mut io);
    }
    let mut request = request;
    let located = if command.name == "task" { locate(&mut request, &mut mux)? } else { None };
    let waiting = if command.name == "task" && request["action"] == "done" { mux.call("tasks", serde_json::json!({ "action": "list" })).ok() } else { None };
    let (mut shorts, mut listed) = (located.as_ref().map(|(_, known)| known.clone()).unwrap_or_default(), false);
    let mut reply = match mux.call(wire(command, &request), request.clone()) {
        Ok(reply) => reply,
        Err(mut failure) => {
            if failure.message.contains("task-") {
                let shorts = mux.call("tasks", json!({ "action": "list" })).map(|b| output::shorts(&b)).unwrap_or_default();
                failure.message = output::short_text(&failure.message, &shorts);
            }
            return Err(explain(command, failure, &mut mux, &target));
        }
    };
    if let (Some(waiting), Some(done)) = (waiting, request["task"].as_str()) {
        reply["freed"] = serde_json::Value::Array(output::freed(&waiting, done));
    }
    if command.name == "task" && request["action"] == "list" {
        if let Ok(backlog) = mux.call("tasks", serde_json::json!({ "action": "list" })) {
            reply["shelf"] = backlog["shelf"].clone();
            output::name_out(&mut reply, &backlog["desk"]);
            shorts.extend(output::shorts(&backlog));
            listed = true;
            reply["script"] = backlog["script"].clone();
        }
        if let Some(query) = request["query"].as_str() {
            output::keep_matching(&mut reply, query);
        }
    }
    if let (true, Some(pane)) = (command.name == "workspace", request["pane"].as_str()) {
        reply = output::scoped(&reply, pane).ok_or_else(|| fail(exit::NOT_FOUND, format!("no pane \"{pane}\": your terminal isn't one of this Kanna's")))?;
    }
    let undrawn = match request["path"].as_str() {
        Some(path) if command.name == "open" && request["type"] == "md" => Some(diagrams::check(path).map(|r| r.failures).unwrap_or_default()),
        _ => None,
    };
    if let Some(undrawn) = &undrawn {
        reply["diagrams"] = Value::Array(undrawn.clone());
    }
    if json_output {
        println!("{reply}");
    } else {
        if let Some((id, _)) = located {
            for value in [&mut request, &mut reply] {
                if value["task"].is_string() {
                    value["task"] = json!(id);
                }
            }
        }
        if !listed && (request.to_string().contains("task-") || reply.to_string().contains("task-")) {
            shorts.extend(mux.call("tasks", json!({ "action": "list" })).map(|b| output::shorts(&b)).unwrap_or_default());
        }
        shorts.extend(output::shorts(&reply));
        output::shorten(&mut request, &shorts);
        output::shorten(&mut reply, &shorts);
        let text = (command.show)(&request, &reply);
        if !text.is_empty() {
            println!("{text}");
        }
        for failure in undrawn.iter().flatten() {
            eprintln!("{}", diagrams::line(failure));
        }
    }
    if command.name == "machine" && request["action"] == "update" && reply["machines"].as_array().is_some_and(|m| m.iter().any(|m| m["ok"] == false)) {
        return Err(fail(exit::FAILED, "not every machine took the update"));
    }
    Ok(())
}

fn unplaced(command: &str, request: &Value) -> Option<&'static str> {
    let given = |keys: &[&str]| keys.iter().any(|key| !request[*key].is_null());
    match command {
        "open" if !given(&["window", "workspace", "from"]) => Some("add --window WINDOW (or --window new)"),
        "markup" if !request["type"].is_null() && !given(&["window", "from"]) => Some("add --window WINDOW (or --window new)"),
        "comment" if !given(&["window", "from"]) => Some("add --window WINDOW (or --window new)"),
        "arrange" if !given(&["window", "from"]) => Some("add --window WINDOW"),
        "screenshot" if !given(&["pane", "window", "from"]) => Some("say PANE or --window WINDOW"),
        _ => None,
    }
}

fn locate(request: &mut Value, mux: &mut Session) -> Result<Option<(String, Vec<(String, String)>)>, Failure> {
    let action = request["action"].as_str().unwrap_or("").to_string();
    let Some(reference) = request["task"].as_str().filter(|_| ["park", "activate", "shelve", "done", "name"].contains(&action.as_str())).map(String::from) else { return Ok(None) };
    let found = match mux.call("tasks", json!({ "action": "resolve", "task": reference })) {
        Ok(found) => Some(found["task"].clone()),
        Err(failure) if failure.message.ends_with("[ambiguous]") => return Err(failure),
        Err(_) => None,
    };
    let backlog = found.as_ref().is_some_and(|t| t["place"] == "shelf");
    if action == "shelve" && backlog {
        return Err(fail(exit::FAILED, format!("{reference} is on the Shelf: `kanna task pick {reference}` moves it to the Desk")));
    }
    let pane = found.as_ref().and_then(|t| t["pane"].as_str()).map(String::from);
    let id = found.as_ref().and_then(|t| t["id"].as_str()).map(String::from);
    match (action.as_str(), id.as_deref()) {
        ("name", Some(id)) => request["task"] = json!(id),
        ("name", None) => {
            request.as_object_mut().map(|r| r.remove("task"));
            request["pane"] = json!(reference);
        }
        ("done", Some(id)) if backlog => request["task"] = json!(id),
        (_, Some(_)) => request["task"] = json!(pane.unwrap_or(reference)),
        (_, None) => {}
    }
    Ok(id.map(|id| (id, found.as_ref().map(output::shorts).unwrap_or_default())))
}

/// `kanna PATH`: a word that isn't a command but is a folder brings forward its project's Desk, or opens one.
fn project_folder(name: &str) -> Option<String> {
    if commands::find(name).is_some() {
        return None;
    }
    let folder = std::fs::canonicalize(commands::absolute(name)).ok().filter(|p| p.is_dir())?;
    Some(folder.to_string_lossy().into_owned())
}

fn lookup(name: &str) -> Result<&'static Command, Failure> {
    commands::find(name).ok_or_else(|| {
        let similar = commands::similar(name);
        let hint = if similar.is_empty() { String::new() } else { format!(" Did you mean: {}?", similar.join(", ")) };
        fail(exit::USAGE, format!("unknown command \"{name}\".{hint} Run `kanna --help` for all commands."))
    })
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

fn wire(command: &Command, request: &Value) -> &'static str {
    match command.name {
        "markup" if request.get("type").is_some() => "open",
        "task" if ["new", "edit", "start", "stop", "setup", "pick", "shelve", "send", "messages"].contains(&request["action"].as_str().unwrap_or("")) => "tasks",
        "task" if request["action"] == "done" => "tasks",
        "task" if request["action"] == "name" && request.get("task").is_some() => "tasks",
        "task" if request["action"] == "name" => "rename-pane",
        "task" if ["park", "activate", "desk"].contains(&request["action"].as_str().unwrap_or("")) => "desk",
        "workspace" => "list",
        name => name,
    }
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

const COMPLETIONS_HELP: &str = "usage: kanna completions zsh|bash\n\nPrints the tab-completion script for your shell. Load it from your shell's startup file:\n  zsh:  source <(kanna completions zsh)     in ~/.zshrc\n  bash: eval \"$(kanna completions bash)\"  in ~/.bashrc\n\nTab then completes commands, options, pane names and IDs, tabs, windows, instances,\nand files of the kind a command takes (markdown for open md, HTML for markup).";

pub const CONFIG_SCHEMA: &str = include_str!("../../../schemas/config.schema.json");

const CONFIG_HELP: &str = "usage: kanna config schema\n\nKanna's settings are ~/.kanna-v4/config.json, overridden key by key by a repo's .kanna/config.json, e.g.\n  { \"version\": 1, \"planning\": { \"context\": 300000 }, \"tasks\": { \"agent\": \"codex\" }, \"shelf\": { \"runners\": 3 } }\n\n`kanna config schema` prints their JSON Schema: every key, its type, default and what it does.";

const RAW_HELP: &str = "usage: kanna raw REQUEST\n\nSends one control-protocol request as JSON and prints the reply, e.g.\n  kanna raw '{\"cmd\":\"list\"}'\n  kanna raw '{\"cmd\":\"open\",\"args\":{\"type\":\"term\",\"name\":\"a\"}}'";

const INSTANCES_HELP: &str = "kanna instances — List the running Kanna instances.\n\
\n\
usage: kanna instances [--json]\n\
       kanna instances prune\n\
       kanna [--instance NAME] instances stop [--json]\n\
\n\
Instances are a tool for developing and testing Kanna, not a user feature: the app\n\
doesn't offer them. To work on another project, open a Desk on it (kanna PATH, or\n\
File > New Desk in the app).\n\
\n\
Each instance is a separate Kanna: its own daemon, keeper, terminals, Kanna.app and saved\n\
layout, in its own folder in your application support folder. Kanna comes in three channels,\n\
each a separate app that can run beside the others: production (Kanna, folder kanna-prod),\n\
staging (Kanna Staging, kanna-staging) and dev (Kanna Dev, kanna-dev). An instance named\n\
test uses the folder kanna-dev-test, with its socket at kanna-dev-test/kanna.sock. A dev\n\
build made in a git worktree is an app of its own whose default instance is named after the\n\
worktree: built in .kanna-worktrees/task-3f9a1c2e, it is Kanna Dev task-3f9a1c2e, keeping\n\
its files in kanna-dev-task-3f9a1c2e.\n\
\n\
Commands go to an instance of this kanna's channel:\n  \
  1. the instance named by --instance NAME, if given;\n  \
  2. else $KANNA_SOCKET (Kanna sets it, with $KANNA_INSTANCE and $KANNA_TERMINAL, in its\n     \
     terminals, so kanna run inside a pane controls that pane's Kanna, whatever its channel);\n  \
  3. else the instance named by $KANNA_INSTANCE;\n  \
  4. else this build's default instance (its worktree's, for a dev build in a worktree).\n\
Commands start the instance's Kanna.app if nothing answers (the app starts its daemon),\n\
but never for $KANNA_SOCKET.\n\
\n\
kanna instances lists the running instances of every channel, with the worktree a dev\n\
build came from. kanna instances prune deletes the folders of dev instances whose worktree\n\
is gone, once nothing runs in them. kanna instances stop quits an instance: its Kanna.app\n\
saves its windows and quits, its terminals are hung up (a task's resume when it starts\n\
again), and its keeper and daemon exit. It waits until they have, and never starts one.\n\
\n\
examples:\n  \
  kanna instances\n      \
      Which instances are running, and which one commands go to.\n  \
  kanna --instance test open --name probe --cmd \"sh\"\n      \
      Start (or use) a scratch instance named test, apart from the Kanna you work in.\n  \
  KANNA_INSTANCE=test kanna workspace list --all\n      \
      The same choice, for every command in a test script.\n  \
  kanna instances prune\n      \
      Clean up after worktrees you've removed.\n  \
  kanna --instance test instances stop\n      \
      Quit the scratch instance test: its app, daemon, keeper and terminals.";

fn instances(target: &Target, json_output: bool, env: Env) -> Result<(), Failure> {
    let running: Vec<(target::Instance, Value)> = target::instances(env)
        .into_iter()
        .filter(target::Instance::answers)
        .map(|i| {
            let socket = i.socket.clone();
            let probe = Target { instance: i.name.clone(), socket, pinned: None, ..target.clone() };
            let list = probe.try_connect().and_then(|mut m| m.call("list", json!({})).ok()).unwrap_or(json!({}));
            (i, list)
        })
        .collect();
    let count = |list: &Value, key: &str| list[key].as_array().map_or(0, Vec::len);
    if json_output {
        let rows: Vec<Value> = running
            .iter()
            .map(|(i, list)| json!({
                "name": i.name, "channel": i.channel.id(), "worktree": i.worktree, "socket": i.socket, "current": i.socket == target.socket,
                "windows": count(list, "windows"), "panes": count(list, "panes"),
            }))
            .collect();
        println!("{}", json!({ "instances": rows, "current": { "name": target.instance, "channel": target.build.channel.id(), "socket": target.socket } }));
        return Ok(());
    }
    if running.is_empty() {
        println!("No Kanna instances are running. Any command starts one, e.g. kanna . or kanna --instance work open --window new.");
        return Ok(());
    }
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    let home = env("HOME").unwrap_or_default();
    let rows: Vec<[String; 5]> = running
        .iter()
        .map(|(i, list)| {
            let star = if i.socket == target.socket { "*" } else { " " };
            let worktree = i.worktree.as_ref().map_or(String::new(), |w| {
                let shown = w.display().to_string();
                let shown = if !home.is_empty() && shown.starts_with(&home) { format!("~{}", &shown[home.len()..]) } else { shown };
                if w.exists() { shown } else { format!("{shown} (gone)") }
            });
            [star.into(), i.name.clone(), i.channel.id().into(), format!("{}, {}", plural(count(list, "windows"), "window"), plural(count(list, "panes"), "pane")), worktree]
        })
        .collect();
    let width = |n: usize| rows.iter().map(|r| r[n].len()).max().unwrap_or(0);
    let (name, channel, counts) = (width(1), width(2), width(3));
    for row in &rows {
        println!("{}", format!("{} {:<name$}  {:<channel$}  {:<counts$}  {}", row[0], row[1], row[2], row[3], row[4]).trim_end());
    }
    let variable = target.brand().variable("INSTANCE");
    println!("\n* the instance this shell's kanna commands talk to ({}). Pick another with --instance NAME or {variable}.", target.socket.display());
    Ok(())
}

const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

fn stop(target: &Target, json_output: bool) -> Result<(), Failure> {
    let shown = format!("{} ({})", target.instance, target.socket.display());
    let report = |stopped: bool, text: String| {
        if json_output {
            println!("{}", json!({ "instance": target.instance, "socket": target.socket, "stopped": stopped }));
        } else {
            println!("{text}");
        }
    };
    let Some(mut mux) = target.try_connect() else {
        report(false, format!("{shown} isn't running"));
        return Ok(());
    };
    mux.call("shutdown", json!({})).map_err(Failure::from)?;
    let started = std::time::Instant::now();
    while target.socket.exists() {
        if started.elapsed() > STOP_WAIT {
            return Err(fail(exit::FAILED, format!("asked {shown} to stop, but its socket is still there after {}s", STOP_WAIT.as_secs())));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    report(true, format!("stopped {shown}"));
    Ok(())
}

fn prune(env: Env) -> Result<(), Failure> {
    let (mut pruned, mut kept) = (0, 0);
    for instance in target::instances(env) {
        let Some(worktree) = &instance.worktree else { continue };
        if worktree.exists() {
            continue;
        }
        let folder = instance.folder.display();
        if instance.answers() {
            println!("kept {folder}: its worktree {} is gone, but it's running (quit it first)", worktree.display());
            kept += 1;
            continue;
        }
        if let Some(pid) = target::keeper_pid(&instance.folder) {
            println!("kept {folder}: its worktree {} is gone, but its keeper (pid {pid}) still holds terminals (kill {pid} first)", worktree.display());
            kept += 1;
            continue;
        }
        std::fs::remove_dir_all(&instance.folder).map_err(|e| fail(exit::FAILED, format!("can't delete {folder}: {e}")))?;
        println!("deleted {folder} (worktree {} is gone)", worktree.display());
        pruned += 1;
    }
    if pruned + kept == 0 {
        println!("Nothing to prune: every dev instance's worktree is still there, or it's still running.");
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    pub fn shell_words(line: &str) -> Vec<String> {
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
        // Run from a Kanna terminal, a command would name its pane as the caller.
        std::env::remove_var("KANNA_TERMINAL");
        let words = shell_words(line);
        assert_eq!(words[0], "kanna");
        let mut args = Argv::new(words[1..].to_vec());
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
    fn a_comment_takes_a_file_a_line_or_lines_and_the_rest_as_its_text() {
        let (name, request) = parse("kanna comment /tmp/a.rs:50 Waits for a message.").unwrap();
        assert_eq!(name, "comment");
        assert_eq!(request, json!({ "path": "/tmp/a.rs", "line": 50, "text": "Waits for a message." }));
        let (_, request) = parse("kanna comment /tmp/a.rs:20-48 'Sends it.' --tab").unwrap();
        assert_eq!((request["line"].clone(), request["last"].clone(), request["tab"].clone()), (json!(20), json!(48), json!(true)));
        assert!(parse("kanna comment /tmp/a.rs Sends it.").is_err());
        assert!(parse("kanna comment /tmp/a.rs:5").is_err());
        assert!(parse("kanna comment /tmp/a.rs:9-3 Backwards.").is_err());
        assert!(parse("kanna comment /tmp/a.rs:0 Nothing.").is_err());
    }

    #[test]
    fn a_tour_takes_pairs_of_a_spot_and_its_text() {
        let (name, request) = parse("kanna tour /tmp/a.rs:50 'Waits for a message.' /tmp/b.md:3 'The big picture.' --tab").unwrap();
        assert_eq!(name, "tour");
        assert_eq!((request["action"].clone(), request["tab"].clone()), (json!("new"), json!(true)));
        assert_eq!(request["stops"], json!([{ "path": "/tmp/a.rs", "line": 50, "text": "Waits for a message." }, { "path": "/tmp/b.md", "line": 3, "text": "The big picture." }]));
        let (_, request) = parse("kanna tour add --next /tmp/a.rs:20-48 'Sends it.'").unwrap();
        assert_eq!((request["action"].clone(), request["next"].clone(), request["stops"][0]["last"].clone()), (json!("add"), json!(true), json!(48)));
        assert_eq!(parse("kanna tour").unwrap().1["action"], "show");
        assert_eq!(parse("kanna tour go back").unwrap().1["to"], "back");
        assert_eq!(parse("kanna tour go 3").unwrap().1["to"], 3);
        assert_eq!(parse("kanna tour end").unwrap().1["action"], "end");
        for bad in [
            "kanna tour /tmp/a.rs:5 Waits for a message.",
            "kanna tour /tmp/a.rs:5",
            "kanna tour /tmp/a.rs 'No line.'",
            "kanna tour /tmp/a.rs:9-3 'Backwards.'",
            "kanna tour /tmp/a.md:3-5 'A doc block.'",
            "kanna tour add",
            "kanna tour go",
            "kanna tour go sideways",
            "kanna tour --next /tmp/a.rs:5 'Next of nothing.'",
            "kanna tour end now",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_local_page_opens_as_a_file_url_and_markup_opens_or_switches_comments() {
        let dir = std::env::temp_dir().join(format!("kanna-page-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let page = dir.join("a page.html");
        std::fs::write(&page, "<p>hi</p>").unwrap();
        let (_, request) = parse(&format!("kanna open web '{}'", page.display())).unwrap();
        assert_eq!(request["url"], json!(format!("file://{}", page.display().to_string().replace(' ', "%20"))));
        let (_, request) = parse("kanna open web a.test").unwrap();
        assert_eq!(request["url"], json!("a.test"));
        assert!(request.get("markup").is_none());
        let (name, request) = parse(&format!("kanna markup '{}' --tab", page.display())).unwrap();
        assert_eq!(name, "markup");
        assert_eq!(request["type"], json!("web"));
        assert_eq!(request["markup"], json!(true));
        assert_eq!(request["tab"], json!(true));
        let (_, request) = parse("kanna markup /tmp/notes.md --split right").unwrap();
        assert_eq!((request["type"].clone(), request["path"].clone(), request["markup"].clone()), (json!("md"), json!("/tmp/notes.md"), json!(true)));
        assert_eq!(parse("kanna markup spec --off").unwrap().1, json!({ "pane": "spec", "off": true }));
        assert_eq!(parse("kanna markup spec").unwrap().1["pane"], json!("spec"));
        assert!(parse("kanna markup spec --tab").is_err());
        assert_eq!(parse("kanna markup spec --to agent").unwrap().1, json!({ "pane": "spec", "agent": "agent" }));
        assert_eq!(parse("kanna markup /tmp/notes.md --to p4").unwrap().1["agent"], json!("p4"));
        assert!(parse("kanna markup spec --to agent --off").is_err());
        assert!(parse("kanna markup /tmp/notes.md --off").is_err());
        let (_, request) = parse("kanna open md /tmp/notes.md").unwrap();
        assert!(request.get("markup").is_none() && request.get("agent").is_none());
        assert_eq!(parse("kanna markup /tmp/m.html").unwrap().1["url"], json!("file:///tmp/m.html"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_markdown_pane_marks_changes_since_a_ref_and_a_bare_diff_means_main() {
        assert_eq!(parse("kanna open md /tmp/notes.md --diff HEAD").unwrap().1["diff"], json!("HEAD"));
        let (_, request) = parse("kanna open md --diff abc123 /tmp/notes.md --tab").unwrap();
        assert_eq!((request["path"].clone(), request["diff"].clone()), (json!("/tmp/notes.md"), json!("abc123")));
        assert_eq!(parse("kanna open md /tmp/notes.md --diff=origin/main").unwrap().1["diff"], json!("origin/main"));
        assert_eq!(parse("kanna open md /tmp/notes.md --diff v1..main").unwrap().1["diff"], json!("v1..main"));
        let here = env!("CARGO_MANIFEST_DIR");
        let main = ["main", "master"].into_iter().find(|b| std::process::Command::new("git").args(["-C", here, "rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")]).output().is_ok_and(|o| o.status.success()));
        if let Some(main) = main {
            assert_eq!(parse(&format!("kanna open md {here}/README.md --diff")).unwrap().1["diff"], json!(main));
        }
        assert!(parse("kanna open md /no-repo-here/notes.md --diff").is_err());
        assert!(parse("kanna open web a.test --diff main").is_err());
        assert!(parse("kanna open md /tmp/notes.md").unwrap().1.get("diff").is_none());
    }

    #[test]
    fn requests() {
        let cases = [
            ("kanna open", json!({ "type": "term" })),
            ("kanna open --name logs --split right --size 1/3 --cmd 'tail -f a.log'", json!({ "type": "term", "name": "logs", "split": "right", "size": "1/3", "cmd": "tail -f a.log" })),
            ("kanna open web localhost:3000 --split down", json!({ "type": "web", "url": "localhost:3000", "split": "down" })),
            ("kanna open --tab --no-wait", json!({ "type": "term", "tab": true, "wait": false })),
            ("kanna close logs", json!({ "pane": "logs" })),
            ("kanna close --window win-2", json!({ "window": "win-2" })),
            ("kanna rename-tab tab-1 my servers", json!({ "tab": "tab-1", "title": "my servers" })),
            ("kanna send server npm test", json!({ "pane": "server", "text": "npm test" })),
            ("kanna move logs --to server --side bottom", json!({ "pane": "logs", "to": "server", "side": "bottom" })),
            ("kanna move-tab tab-2 --index 0", json!({ "tab": "tab-2", "index": 0 })),
            ("kanna open web a.test --history", json!({ "type": "web", "url": "a.test", "history": true })),
            ("kanna navigate site --back", json!({ "pane": "site", "back": true })),
            ("kanna open md /tmp/a.md", json!({ "type": "md", "path": "/tmp/a.md" })),
            ("kanna navigate spec /tmp/b.md", json!({ "pane": "spec", "path": "/tmp/b.md" })),
            ("kanna read s1", json!({ "pane": "s1" })),
            ("kanna read s1 --lines 5", json!({ "pane": "s1", "lines": 5 })),
            ("kanna send p1 -- --json", json!({ "pane": "p1", "text": "--json" })),
            ("kanna send-key agent return", json!({ "pane": "agent", "keys": ["return"] })),
            ("kanna send-key p3 up up shift+tab ctrl+c", json!({ "pane": "p3", "keys": ["up", "up", "shift+tab", "ctrl+c"] })),
            ("kanna send p1 echo -- --mux -x", json!({ "pane": "p1", "text": "echo --mux -x" })),
            ("kanna open --cwd /tmp", json!({ "type": "term", "cwd": "/tmp" })),
            ("kanna move agent --dock", json!({ "pane": "agent", "dock": "left" })),
            ("kanna move agent --dock right --window win-2", json!({ "pane": "agent", "dock": "right", "window": "win-2" })),
            ("kanna move agent --tab", json!({ "pane": "agent", "tab": "new" })),
            ("kanna move agent --tab tab-3", json!({ "pane": "agent", "tab": "tab-3" })),
            ("kanna move agent --tab --window win-2", json!({ "pane": "agent", "tab": "new", "window": "win-2" })),
            ("kanna open code /tmp/a.rs:42", json!({ "type": "code", "path": "/tmp/a.rs", "line": 42 })),
            ("kanna open code /tmp/a.rs", json!({ "type": "code", "path": "/tmp/a.rs" })),
            ("kanna open code /tmp/a.rs:x", json!({ "type": "code", "path": "/tmp/a.rs:x" })),
            ("kanna navigate src /etc/hosts:3", json!({ "pane": "src", "path": "/etc/hosts", "line": 3 })),
            ("kanna navigate src /etc/hosts", json!({ "pane": "src", "path": "/etc/hosts" })),
            ("kanna navigate site localhost:5173", json!({ "pane": "site", "url": "localhost:5173" })),
        ];
        for (line, expected) in cases {
            let (_, request) = parse(line).unwrap_or_else(|f| panic!("{line}: {}", f.message));
            assert_eq!(request, expected, "{line}");
        }
    }

    #[test]
    fn usage_errors_say_what_to_do() {
        for (line, hint) in [
            ("kanna close", "PANE, --tab TAB or --window WINDOW"),
            ("kanna zoom", "missing PANE"),
            ("kanna open top", "--cmd \"top\""),
            ("kanna open web", "need a URL"),
            ("kanna move logs", "say where"),
            ("kanna zoom a b", "unexpected argument \"b\""),
            ("kanna open --history", "only for web panes"),
            ("kanna open md", "need a file"),
            ("kanna open notes.md", "kanna open md notes.md"),
            ("kanna navigate site --back x", "one of URL"),
            ("kanna workspce list", "Did you mean: workspace"),
            ("kanna rename tab-1 x", "Did you mean: rename-tab"),
            ("kanna read", "missing PANE"),
            ("kanna send-key", "missing PANE"),
            ("kanna send-key p1", "missing KEY"),
            ("kanna read p1 --lines 0", "--lines needs a whole number"),
            ("kanna open ios", "need an app"),
            ("kanna open --app x", "only for iOS panes"),
            ("kanna open --name agent --dock", "make a task with `kanna task new`"),
            ("kanna open --dock right --cmd claude", "a docked pane is a task's agent"),
            ("kanna open web a.test --dock=right --window new", "a docked pane is a task's agent"),
            ("kanna open --frozen", "a docked pane is a task's agent"),
            ("kanna markup a.md --dock", "a docked pane is a task's agent"),
            ("kanna move agent --dock --tab", "--dock goes on its own"),
            ("kanna move agent --dock --to b", "--dock goes on its own"),
        ] {
            let failure = parse(line).expect_err(line);
            assert_eq!(failure.code, exit::USAGE, "{line}");
            assert!(failure.message.contains(hint), "{line}: {}", failure.message);
        }
    }

    #[test]
    fn relative_paths_are_made_absolute_here() {
        let (_, request) = parse("kanna open md docs/a.md --cwd sub").unwrap();
        assert!(request["path"].as_str().unwrap().starts_with('/'));
        assert!(request["cwd"].as_str().unwrap().starts_with('/'));
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
        let all = json!({ "action": "list", "all": true });
        let reply = json!({
            "windows": [],
            "workspaces": [{ "id": "ws-1", "where": { "desk": "mac" }, "dock": { "pane": "agent", "id": "p1", "edge": "left", "width": "2/5", "frozen": true }, "focused": "agent",
                             "tabs": [{ "id": "tab-1", "title": "Tab 1", "active": true, "layout": { "pane": "p2" } }], "windows": [], "panes": ["p2", "p1"] }],
            "panes": [{ "id": "p1", "name": "agent", "type": "term", "state": "running", "cmd": "claude" }, { "id": "p2", "name": null, "type": "term", "state": "running" }]
        });
        let text = output::workspace(&all, &reply);
        assert!(text.contains("\nws-1, on the Desk\n  dock: p1 (agent)  left 2/5  frozen\n  focus: agent\n  *tab-1 \"Tab 1\"  p2"), "{text}");
        let opened = json!({ "pane": { "id": "p1", "name": "agent", "state": "running" }, "window": null, "tab": null, "dock": "left", "workspace": { "id": "ws-1", "where": { "desk": "mac" } } });
        assert_eq!(output::opened(&json!({}), &opened), "opened p1 (agent) docked left in ws-1 (on the Desk): running");
        assert_eq!(output::opened(&json!({ "frozen": true }), &opened), "opened p1 (agent) docked left, frozen, in ws-1 (on the Desk): running");
        let moved = json!({ "window": null, "workspace": { "id": "ws-1" }, "dock": { "pane": "agent", "edge": "right", "width": "1/3" } });
        assert_eq!(output::moved(&moved), "agent is docked right in ws-1 (width 1/3)");
    }

    #[test]
    fn workspace_list_lists_unplaced_panes_and_says_when_there_is_nothing() {
        let all = json!({ "action": "list", "all": true });
        let list = output::workspace(&all, &json!({ "windows": [], "panes": [{ "id": "p1", "type": "term", "state": "running" }] }));
        assert_eq!(list, "Unplaced panes\np1");
        assert_eq!(output::workspace(&all, &json!({ "windows": [], "panes": [] })), "no windows (open one with `kanna open`)");
        let alone = output::scoped(&json!({ "windows": [], "panes": [{ "id": "p1", "name": "logs", "type": "term" }] }), "p1").unwrap();
        assert_eq!(output::workspace(&json!({ "action": "list", "pane": "p1" }), &alone), "p1 (logs) isn't in a window or a workspace");
    }

    #[test]
    fn a_quiz_answer_is_sent_with_its_repo_relative_file_and_a_round_on_main_s_commit() {
        let git = |args: &[&str]| String::from_utf8(std::process::Command::new("git").args(args).output().unwrap().stdout).unwrap().trim().to_string();
        let (top, repo) = (git(&["rev-parse", "--show-toplevel"]), git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]));
        let (_, answer) = parse("kanna quiz answer r12 --kind why --file src/lib.rs:40-42 --wrong --again a7 --json '{\"pick\": 2}'").unwrap();
        let file = std::path::Path::new(&std::env::current_dir().unwrap().canonicalize().unwrap()).join("src/lib.rs").strip_prefix(&top).unwrap().to_string_lossy().into_owned();
        assert_eq!(answer, json!({ "action": "answer", "repo": repo.trim_end_matches("/.git"), "round": "r12", "kind": "why", "file": file, "line": 40, "last": 42, "right": false, "again": "a7", "doc": { "pick": 2 } }));
        let (_, start) = parse("kanna quiz start").unwrap();
        assert_eq!(start["head"], json!(git(&["rev-parse", "main"])));
        assert_eq!(parse("kanna quiz history --misses").unwrap().1["misses"], json!(true));
        let refused = |line: &str| parse(line).err().unwrap_or_else(|| panic!("{line} parsed")).message;
        assert!(refused("kanna quiz answer r1 --kind why --file a.rs:1 --right --wrong").contains("--right or --wrong"));
        assert!(refused("kanna quiz answer r1 --kind why --file a.rs --right").contains("needs a line"));
        assert!(refused("kanna quiz start --misses").contains("--misses is for quiz history"));
        assert!(refused("kanna quiz start --right").contains("for quiz answer"));
        assert!(refused("kanna quiz start --json '{oops'").contains("JSON object"));
        assert!(refused("kanna quiz score").contains("unknown action"));
        assert_eq!(parse("kanna quiz open --agent codex").unwrap().1, json!({ "action": "open", "repo": repo.trim_end_matches("/.git"), "agent": "codex" }));
        assert_eq!(parse("kanna quiz open").unwrap().1, json!({ "action": "open", "repo": repo.trim_end_matches("/.git") }));
        assert!(refused("kanna quiz start --agent codex").contains("--agent is for quiz open"));
        assert!(refused("kanna capabilities --json '{}'").contains("--json DOC"));
    }

    #[test]
    fn quiz_history_lists_rounds_with_their_answers_and_no_score() {
        let reply = json!({ "misses": false, "rounds": [
            { "round": "r2", "at": 0.0, "head": "67b287cabc", "answers": [] },
            { "round": "r1", "at": 0.0, "head": "90d98b9def", "answers": [
                { "answer": "a1", "kind": "where", "file": "src/a.rs", "right": false, "again": null, "doc": { "line": 4, "question": "Where?" } },
                { "answer": "a2", "kind": "why", "file": "docs/b.md", "right": true, "again": "a0", "doc": {} },
            ] },
        ]});
        let text = output::quiz(&json!({ "action": "history" }), &reply);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("r2  67b287c  "), "{text}");
        assert_eq!(lines[2], "  a1   ✗ where        src/a.rs:4  Where?");
        assert_eq!(lines[3], "  a2   ✓ why          docs/b.md  (again: a0)");
        assert!(!text.contains("of"), "{text}");
        assert_eq!(output::quiz(&json!({ "action": "history" }), &json!({ "misses": true, "rounds": [] })), "no misses to bring back");
        assert_eq!(output::quiz(&json!({ "action": "start" }), &json!({ "round": "r3" })), "r3");
        assert_eq!(output::quiz(&json!({ "action": "open" }), &json!({ "pane": "p9", "window": "win-4", "opened": true })), "opened p9 in win-4");
        assert_eq!(output::quiz(&json!({ "action": "open" }), &json!({ "pane": "p9", "window": "win-4", "opened": false })), "brought forward p9 in win-4");
    }
}
