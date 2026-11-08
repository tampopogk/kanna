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
       kanna instances move TO [--dry-run] [--json]\n\
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
kanna instances move TO moves this Kanna's work to the instance folder TO (a folder name in\n\
your application support folder, such as kanna-staging, or a path): every task, its history\n\
and messages, the schedules, the phone's chat and quiz, and this machine's key, so TO is\n\
the same machine to your other machines. Desk tasks go to the Shelf; started tasks keep their\n\
worktree, branch and conversation, and those whose agent was running start again when TO's\n\
Kanna starts. TO's Kanna must not be running, must be a build that reads these records,\n\
and must have no tasks of its own. No task starts meanwhile, and each agent quits at its\n\
turn's end (it waits up to 10 minutes for them). Then this Kanna stops, and starts again empty\n\
with a key of its own; its records are kept beside, as kanna.db.moved-TIME. Agent accounts\n\
don't move: add them again in TO's Kanna. --dry-run says what would move, which agents would\n\
quit, and why it would be refused, and changes nothing.\n\
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
      Quit the scratch instance test: its app, daemon, keeper and terminals.\n  \
  kanna instances move kanna-staging --dry-run\n      \
      What moving this Kanna to Kanna Staging would move, and what stops it.";

pub fn run(args: &mut Argv, invocation: &Invocation, wants_help: bool) -> Result<(), Failure> {
    let (target, json_output, env) = (invocation.target, invocation.json_output, invocation.env);
    if wants_help {
        println!("{HELP}");
        return Ok(());
    }
    match args.positional().as_deref() {
        Some("prune") => prune(env),
        Some("stop") => stop(target, json_output),
        Some("move") => {
            let dry = args.flag("--dry-run");
            let to = args.positional().ok_or_else(|| fail(exit::USAGE, "usage: kanna instances move TO [--dry-run]".to_string()))?;
            move_to(target, &to, dry, json_output)
        }
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

fn move_to(target: &Target, to: &str, dry: bool, json_output: bool) -> Result<(), Failure> {
    let Some(mut mux) = target.try_connect() else {
        return Err(fail(exit::FAILED, format!("{} ({}) isn't running: run this from the Kanna you're moving", target.instance, target.socket.display())));
    };
    if !dry {
        eprintln!("moving to {to}: no task starts now, and each agent quits at its turn's end…");
    }
    let reply = mux.call("instance", json!({ "action": "move", "to": to, "dry": dry })).map_err(Failure::from)?;
    if json_output {
        println!("{reply}");
        return Ok(());
    }
    print!("{}", described(&reply, dry));
    Ok(())
}

fn described(reply: &Value, dry: bool) -> String {
    let plural = |n: u64, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    let count = |key: &str| reply["tasks"][key].as_u64().unwrap_or(0);
    let names = |key: &str| reply[key].as_array().map_or(vec![], |list| list.iter().map(|t| t["name"].as_str().map_or_else(|| t["id"].as_str().unwrap_or("?").to_string(), |n| format!("\"{n}\""))).collect::<Vec<_>>());
    let (from, to) = (reply["from"].as_str().unwrap_or("this Kanna"), reply["to"].as_str().unwrap_or("?"));
    let mut out = String::new();
    let tasks = format!("{} waiting, {} started, {} from the Desk onto the Shelf", plural(count("waiting"), "task"), count("started"), count("desk"));
    match dry {
        true => out += &format!("Moving {from} to {to} ({}) would move:\n  {tasks}\n  their history, {} waiting, the schedules, the phone's chat and quiz\n  this machine's key: {to} is the same machine to your others\n", reply["folder"].as_str().unwrap_or("?"), plural(reply["messages"].as_u64().unwrap_or(0), "message")),
        false => out += &format!("Moved {from} to {to}: {tasks}.\n"),
    }
    let quit = names(if dry { "quit" } else { "resume" });
    if !quit.is_empty() {
        let lead = if dry { "These agents would quit at their turn's end and start again in" } else { "These agents quit and start again when you open" };
        out += &format!("{lead} {to}: {}\n", quit.join(", "));
    }
    let busy = names("busy");
    if dry && !busy.is_empty() {
        out += &format!("In the middle of a turn now (the move waits for them): {}\n", busy.join(", "));
    }
    let accounts: Vec<&str> = reply["accounts"].as_array().map_or(vec![], |a| a.iter().filter_map(Value::as_str).collect());
    if !accounts.is_empty() {
        out += &format!("Agent accounts stay here; add them again in {to}: {}\n", accounts.join(", "));
    }
    let refused = reply["refused"].as_array().cloned().unwrap_or_default();
    if dry && refused.is_empty() {
        out += &format!("Nothing stops it: run kanna instances move {to} without --dry-run.\n");
    }
    for why in &refused {
        out += &format!("Refused ({}): {}\n", why["code"].as_str().unwrap_or("?"), why["message"].as_str().unwrap_or("?"));
    }
    if !dry {
        out += &format!("This Kanna stops now and starts again empty. Quit it, then open {to}'s Kanna and your projects.\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dry_run_says_what_moves_what_quits_and_what_stops_it() {
        let reply = json!({ "from": "kanna-dev", "to": "kanna-staging", "folder": "/S/kanna-staging", "tasks": { "waiting": 2, "started": 1, "desk": 1 }, "quit": [{ "id": "task-1", "name": "Fix" }], "busy": [], "messages": 1, "accounts": ["claude (Claude subscription)"], "refused": [{ "code": "running", "message": "kanna-staging's Kanna is running" }] });
        let text = described(&reply, true);
        assert!(text.contains("2 tasks waiting, 1 started, 1 from the Desk onto the Shelf"), "{text}");
        assert!(text.contains("\"Fix\""), "{text}");
        assert!(text.contains("add them again in kanna-staging: claude (Claude subscription)"), "{text}");
        assert!(text.contains("Refused (running)"), "{text}");
        assert!(!text.contains("Nothing stops it"), "{text}");
    }
}
