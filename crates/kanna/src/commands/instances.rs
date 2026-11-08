use super::Invocation;
use crate::args::Argv;
use crate::target::{self, Env, Target};
use kanna_client::{exit, fail, Failure};
use serde_json::{json, Value};

pub const HELP: &str = "kanna instances — List the running Kanna instances.\n\
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

pub fn run(args: &mut Argv, invocation: &Invocation, wants_help: bool) -> Result<(), Failure> {
    let (target, json_output, env) = (invocation.target, invocation.json_output, invocation.env);
    if wants_help {
        println!("{HELP}");
        return Ok(());
    }
    match args.positional().as_deref() {
        Some("prune") => prune(env),
        Some("stop") => stop(target, json_output),
        _ => list(target, json_output, env),
    }
}

fn list(target: &Target, json_output: bool, env: Env) -> Result<(), Failure> {
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
