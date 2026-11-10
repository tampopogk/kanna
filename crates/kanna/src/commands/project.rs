use super::{absolute, caller, usage, Command, Flow};
use crate::output::tilde;
use serde_json::{json, Value};

pub const COMMAND: Command = Command {
    name: "project",
    group: "Information",
    summary: "Import a project into ~/.kanna-v4/repos, where Kanna keeps every project at the same place on each of your machines: a copy, leaving the original as it is.",
    usage: "kanna project import PATH [--json]",
    options: &[("--json", "What was imported: from, to, the worktrees copied, those left with the original, the agents' sessions carried over, the tasks and terminals started again, and the files that still name the old path.")],
    notes: "Kanna opens Desks and runs tasks only for a project at its place in ~/.kanna-v4/repos: OWNER/NAME from its origin remote (github.com/a/app is a/app), or local/NAME with none. import copies the project PATH is in (a worktree's repo) there, whole: .git, uncommitted and untracked work, stashes, local branches and its worktrees in .kanna-worktrees, as APFS clones where it can, which take no space. PATH stays as it was; Kanna still refuses it, and you can delete it once you're happy with the copy. Worktrees outside the folder stay with the original and are listed. It stops the project's running tasks and terminals first, and starts them again in the copy, each agent resuming its conversation. Kanna's records follow the copy: the tasks, messages, recent projects, planners and terminal folders in kanna.db, Claude's conversation and memory folders (copied), and the sessions codex and opencode keep by folder (pointed at the copy). Then it lists the files that still name PATH, from a quick search of ~/.claude, ~/.agents, your shell's files and the repo, for you to change. It refuses when another clone of the same remote is at the place already, saying both, and when it runs in a terminal in the project, which it would stop. On another machine, run it there: kanna --host MACHINE project import '~/work/app'.",
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
    show,
    flow: Flow::PLAIN,
};

pub fn show(_: &Value, reply: &Value) -> String {
    let list = |key: &str| reply[key].as_array().map_or(vec![], |l| l.iter().filter_map(Value::as_str).map(String::from).collect::<Vec<_>>());
    let from = tilde(reply["from"].as_str().unwrap_or("?"));
    let mut out = vec![format!("imported {from} into {}", tilde(reply["to"].as_str().unwrap_or("?")))];
    let sessions = &reply["sessions"];
    let counted = |items: &[(&str, &Value)]| -> Vec<String> { items.iter().filter_map(|(what, n)| n.as_u64().filter(|n| *n > 0).map(|n| format!("{n} {what}{}", if n == 1 { "" } else { "s" }))).collect() };
    let copied = counted(&[("worktree", &reply["worktrees"]), ("Claude folder", &sessions["claude"])]);
    if !copied.is_empty() {
        out.push(format!("  copied with it: {}", copied.join(", ")));
    }
    let resumed = counted(&[("codex session", &sessions["codex"]), ("opencode session", &sessions["opencode"])]);
    if !resumed.is_empty() {
        out.push(format!("  resuming in the copy: {}", resumed.join(", ")));
    }
    let (tasks, terminals) = (list("tasks"), list("terminals"));
    if !tasks.is_empty() || !terminals.is_empty() {
        out.push(format!("  started again there: {}", tasks.into_iter().chain(terminals).collect::<Vec<_>>().join(", ")));
    }
    for failed in list("failed") {
        out.push(format!("  didn't start again: {failed}"));
    }
    for kept in sessions["kept"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        out.push(format!("  not copied: {kept}"));
    }
    let outside = list("outside");
    if !outside.is_empty() {
        out.push("  worktrees outside it stay with the original:".into());
        out.extend(outside.iter().map(|w| format!("    {}", tilde(w))));
    }
    out.push(format!("{from} is as it was, and Kanna won't open it: delete it once you're happy with the copy."));
    let mentions = list("mentions");
    if !mentions.is_empty() {
        out.push(format!("These still name {from}; change them yourself:"));
        out.extend(mentions.into_iter().map(|m| format!("  {m}")));
    }
    out.join("\n")
}
