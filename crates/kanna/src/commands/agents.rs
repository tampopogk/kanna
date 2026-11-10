use super::{usage, Flow};
use crate::args::Argv;
use crate::target::Session;
use kanna_client::Failure;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const COMMAND: super::Command = super::Command {
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
    parse,
    show,
    flow: Flow { online: Some(online), ..Flow::PLAIN },
};

fn online(request: &Value, mux: &mut Session, json_output: bool) -> Option<Result<(), Failure>> {
    if json_output || !matches!(request["action"].as_str(), Some("setup" | "update")) {
        return None;
    }
    let tool = merge_tool();
    let mut call = |args: Value| mux.call("agents", args);
    let mut merge = |path: &std::path::Path, base: &std::path::Path, new: &std::path::Path| merge_in_editor(path, base, new, &tool);
    let stdin = std::io::stdin();
    let mut io = Io { call: &mut call, input: &mut stdin.lock(), out: &mut std::io::stdout(), merge: &mut merge, tool: tool.clone() };
    Some(run(request, &mut io))
}

fn parse(args: &mut Argv) -> Result<Value, Failure> {
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

pub fn changed_skills(reply: &Value) -> Option<String> {
    let changes = reply["changes"].as_object().filter(|c| !c.is_empty())?;
    let width = changes.keys().map(|k| k.chars().count()).max().unwrap_or(0);
    Some(changes.iter().map(|(skill, change)| {
        let change = match change.as_str().unwrap_or("") {
            "merged" => "updated, merged with your changes",
            "conflict" => "updated; your changes conflict, kept yours",
            other => other,
        };
        format!("  {skill:width$}  {change}")
    }).collect::<Vec<_>>().join("\n"))
}

pub fn show(args: &Value, reply: &Value) -> String {
    let lines = |key: &str, label: &str| reply[key].as_array().filter(|a| !a.is_empty()).map(|a| format!("{label}:\n{}", a.iter().filter_map(Value::as_str).map(|p| format!("  {}", tilde(p))).collect::<Vec<_>>().join("\n")));
    let changes = changed_skills(reply).map(|c| format!("{c}\n"));
    let conflicts = reply["conflicts"].as_array().filter(|a| !a.is_empty()).map(|a| format!("your changes conflict with Kanna's, so these are as you left them (kanna agents update to merge them):\n{}", a.iter().filter_map(|c| c["path"].as_str()).map(|p| format!("  {}", tilde(p))).collect::<Vec<_>>().join("\n")));
    match args["action"].as_str() {
        Some("setup") => [
            Some("Kanna's skills are installed; Kanna asks before it updates them".to_string()),
            changes.map(|c| c.trim_end().to_string()),
            lines("kept", "left alone (yours; kanna agents update --force puts Kanna's back)"),
            conflicts,
            (reply["settings"] != true).then(|| "~/.claude/settings.json isn't JSON, so Claude's hook and sandbox entry weren't added".to_string()),
        ].into_iter().flatten().collect::<Vec<_>>().join("\n"),
        Some("update") => [
            Some(if changes.is_some() { "updated Kanna's skills".to_string() } else { "Kanna's skills are up to date".to_string() }),
            changes.map(|c| c.trim_end().to_string()),
            lines("kept", "left alone (yours; --force puts Kanna's back)"),
            conflicts,
        ].into_iter().flatten().collect::<Vec<_>>().join("\n"),
        Some("remove") => [
            Some("took Kanna's skills and sandbox entry back out; Kanna won't ask again (kanna agents setup to set them up again)".to_string()),
            lines("kept", "left alone (changed since Kanna installed them)"),
        ].into_iter().flatten().collect::<Vec<_>>().join("\n"),
        _ => {
            let skills = reply["skills"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            let state = match reply["setup"].as_str() {
                Some("yes") => match changed_skills(reply) {
                    Some(changes) => format!("set up; this Kanna has newer skills (kanna agents update):\n{changes}"),
                    None => "set up, and up to date".to_string(),
                },
                Some("no") => "not set up (you said no), so agents don't name their tasks, plan or answer comments: kanna agents setup".to_string(),
                _ => "not set up yet, so agents don't name their tasks, plan or answer comments; Kanna's planning terminal asks, or: kanna agents setup".to_string(),
            };
            format!("{state}\nskills: {}", if skills.is_empty() { "none in this Kanna".into() } else { skills })
        }
    }
}

pub const SETUP: &str = "Kanna needs its skills installed to work properly: they're how Claude and
Codex, in any repo, know to name their task, plan with you and answer your
review comments.

  ~/.agents/skills/kanna-*      the skills (Codex)
  ~/.claude/skills/kanna-*      links to them (Claude)
  ~/.claude/settings.json       Kanna's socket in Claude's sandbox, which lets
                                sandboxed agents control Kanna's terminals, and
                                a deny on reading Kanna's device keys

Kanna keeps them up to date and never changes a file you've edited.
Undo it any time with kanna agents remove.
";

pub struct Io<'a> {
    pub call: &'a mut dyn FnMut(Value) -> Result<Value, Failure>,
    pub input: &'a mut dyn BufRead,
    pub out: &'a mut dyn Write,
    pub merge: &'a mut dyn FnMut(&Path, &Path, &Path) -> bool,
    pub tool: String,
}

pub fn run(request: &Value, io: &mut Io) -> Result<(), Failure> {
    let ask = request["ask"] == true;
    match request["action"].as_str() {
        Some("setup") if ask => {
            if (io.call)(json!({ "action": "ask", "kind": "setup" }))?["ask"] != true {
                return Ok(());
            }
            say(io, &format!("{SETUP}\n"));
            match answer(io, "Install Kanna's skills? [Y/n] ", &["y", "n"]) {
                Some("y") => finish(request, io),
                Some(_) => {
                    (io.call)(json!({ "action": "decline" }))?;
                    say(io, "Kanna won't ask again; kanna agents setup installs them any time.\n\n");
                    Ok(())
                }
                None => Ok(()),
            }
        }
        Some("update") if ask => {
            let reply = (io.call)(json!({ "action": "ask", "kind": "update" }))?;
            if reply["ask"] != true {
                return Ok(());
            }
            say(io, &format!("This Kanna has newer skills.\n\n{}\n\n", changed_skills(&reply).unwrap_or_default()));
            match answer(io, "Update Kanna's skills? [Y]es, [n]ot now, [s]kip this version: ", &["y", "n", "s"]) {
                Some("y") => finish(request, io),
                Some("s") => {
                    (io.call)(json!({ "action": "skip" }))?;
                    say(io, "Kanna won't ask about these skills again; kanna agents update updates any time.\n\n");
                    Ok(())
                }
                _ => Ok(()),
            }
        }
        _ => finish(request, io),
    }
}

fn finish(request: &Value, io: &mut Io) -> Result<(), Failure> {
    let mut args = request.clone();
    if let Some(args) = args.as_object_mut() {
        args.remove("ask");
    }
    let reply = (io.call)(args.clone())?;
    let mut unmerged = Vec::new();
    for conflict in reply["conflicts"].as_array().into_iter().flatten() {
        let (Some(path), Some(base), Some(new)) = (conflict["path"].as_str(), conflict["base"].as_str(), conflict["new"].as_str()) else { continue };
        say(io, &format!("{}: your changes conflict with Kanna's.\n", tilde(path)));
        let prompt = format!("Open it in your merge editor ({})? [Y/n] ", io.tool);
        if answer(io, &prompt, &["y", "n"]) == Some("y") && (io.merge)(Path::new(path), Path::new(base), Path::new(new)) {
            (io.call)(json!({ "action": "resolved", "path": path }))?;
            say(io, &format!("merged {}\n", tilde(path)));
        } else {
            unmerged.push(conflict.clone());
        }
    }
    let mut shown = reply.clone();
    shown["conflicts"] = Value::Array(unmerged);
    let text = show(&args, &shown);
    say(io, &format!("{text}\n{}", if request["ask"] == true { "\n" } else { "" }));
    Ok(())
}

fn say(io: &mut Io, text: &str) {
    let _ = io.out.write_all(text.as_bytes());
    let _ = io.out.flush();
}

fn answer(io: &mut Io, prompt: &str, choices: &[&'static str]) -> Option<&'static str> {
    loop {
        say(io, prompt);
        let mut line = String::new();
        if io.input.read_line(&mut line).ok()? == 0 {
            say(io, "\n");
            return None;
        }
        let word = line.trim().to_lowercase();
        if word.is_empty() {
            return choices.first().copied();
        }
        if let Some(choice) = choices.iter().find(|c| word.starts_with(**c)) {
            return Some(choice);
        }
    }
}

fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&format!("{home}/")) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

pub fn merge_tool() -> String {
    Command::new("git").args(["config", "--get", "merge.tool"]).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| "opendiff".into())
}

pub fn merge_in_editor(path: &Path, base: &Path, new: &Path, tool: &str) -> bool {
    let dir = std::env::temp_dir().join(format!("kanna-merge-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let merged = merge_with_git(&dir, path, base, new, tool);
    let _ = fs::remove_dir_all(&dir);
    merged.is_some()
}

fn merge_with_git(dir: &Path, path: &Path, base: &Path, new: &Path, tool: &str) -> Option<()> {
    let name = path.file_name()?;
    let file: PathBuf = dir.join(name);
    fs::create_dir_all(dir).ok()?;
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=Kanna", "-c", "user.email=kanna@localhost", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "-c", "mergetool.keepBackup=false"])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let name = name.to_str()?;
    let commit = |from: &Path, message: &str| fs::copy(from, &file).is_ok() && git(&["add", name]) && git(&["commit", "-q", "-m", message]);
    let ready = git(&["init", "-q", "-b", "yours"])
        && commit(base, "as Kanna installed it")
        && git(&["checkout", "-q", "-b", "kanna"])
        && commit(new, "Kanna's new version")
        && git(&["checkout", "-q", "yours"])
        && commit(path, "yours");
    if !ready {
        return None;
    }
    git(&["merge", "-q", "kanna"]);
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "mergetool.keepBackup=false", "mergetool", "--no-prompt", "--tool", tool, name])
        .status()
        .ok()?;
    let merged = fs::read(&file).ok()?;
    if !status.success() || merged.windows(7).any(|w| w == b"<<<<<<<") {
        return None;
    }
    fs::write(path, merged).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_with(request: Value, typed: &str, replies: Vec<Value>, merges: bool) -> (String, Vec<Value>) {
        let mut sent = Vec::new();
        let mut replies = replies.into_iter();
        let mut call = |args: Value| {
            sent.push(args);
            Ok(replies.next().unwrap_or(json!({})))
        };
        let mut input = typed.as_bytes();
        let mut out = Vec::new();
        let mut merge = |_: &Path, _: &Path, _: &Path| merges;
        let mut io = Io { call: &mut call, input: &mut input, out: &mut out, merge: &mut merge, tool: "opendiff".into() };
        run(&request, &mut io).unwrap();
        (String::from_utf8(out).unwrap(), sent)
    }

    #[test]
    fn setup_asks_once_and_says_what_it_installs() {
        let ask = json!({ "action": "setup", "ask": true });
        let (out, sent) = run_with(ask.clone(), "", vec![json!({ "ask": false })], false);
        assert_eq!((out.as_str(), sent.len()), ("", 1), "nothing to ask: silent");
        let (out, sent) = run_with(ask.clone(), "\n", vec![json!({ "ask": true }), json!({ "changes": { "kanna-tasks": "new" }, "settings": true })], false);
        assert!(out.starts_with("Kanna needs its skills installed to work properly") && out.contains("Install Kanna's skills? [Y/n] "), "{out}");
        assert!(out.contains("kanna-tasks  new"), "{out}");
        assert_eq!(sent[1], json!({ "action": "setup" }));
        let (_, sent) = run_with(ask.clone(), "no\n", vec![json!({ "ask": true })], false);
        assert_eq!(sent[1], json!({ "action": "decline" }));
        let (_, sent) = run_with(ask, "", vec![json!({ "ask": true })], false);
        assert_eq!(sent.len(), 1, "closing the terminal answers nothing");
    }

    #[test]
    fn update_lists_the_changes_and_asks() {
        let ask = json!({ "action": "update", "ask": true, "force": false });
        let changes = json!({ "ask": true, "changes": { "kanna-markup": "merged", "kanna-planning": "conflict", "kanna-tasks": "updated" } });
        let (out, sent) = run_with(ask.clone(), "s\n", vec![changes.clone()], false);
        assert!(out.contains("This Kanna has newer skills.\n\n  kanna-markup    updated, merged with your changes\n  kanna-planning  updated; your changes conflict, kept yours\n  kanna-tasks     updated\n"), "{out}");
        assert!(out.contains("[Y]es, [n]ot now, [s]kip this version: "));
        assert_eq!(sent[1], json!({ "action": "skip" }));
        let (_, sent) = run_with(ask.clone(), "n\n", vec![changes.clone()], false);
        assert_eq!(sent.len(), 1, "not now: nothing");
        let conflict = json!({ "changes": {}, "conflicts": [{ "path": "/h/.agents/skills/kanna-planning/SKILL.md", "base": "/b", "new": "/n" }] });
        let (out, sent) = run_with(ask.clone(), "y\ny\n", vec![changes.clone(), conflict.clone()], true);
        assert_eq!(sent[1], json!({ "action": "update", "force": false }));
        assert!(out.contains("kanna-planning/SKILL.md: your changes conflict with Kanna's.\nOpen it in your merge editor (opendiff)? [Y/n] "), "{out}");
        assert_eq!(sent[2], json!({ "action": "resolved", "path": "/h/.agents/skills/kanna-planning/SKILL.md" }));
        let (out, sent) = run_with(ask, "y\nn\n", vec![changes, conflict], true);
        assert_eq!(sent.len(), 2);
        assert!(out.contains("as you left them (kanna agents update to merge them)"), "{out}");
    }

    #[test]
    fn the_merge_editor_merges_three_ways_into_your_file() {
        let dir = std::env::temp_dir().join(format!("kanna-merge-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let [mine, base, new] = ["SKILL.md", "base.md", "new.md"].map(|n| dir.join(n));
        fs::write(&base, "one\n").unwrap();
        fs::write(&new, "one kanna\n").unwrap();
        fs::write(&mine, "one mine\n").unwrap();
        let tool = dir.join("tool.sh");
        fs::write(&tool, "#!/bin/sh\nprintf 'one both\\n' > \"$1\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let config = dir.join("gitconfig");
        fs::write(&config, format!("[mergetool \"fake\"]\n\tcmd = {} \"$MERGED\"\n\ttrustExitCode = true\n", tool.display())).unwrap();
        std::env::set_var("GIT_CONFIG_GLOBAL", &config);
        assert!(merge_in_editor(&mine, &base, &new, "fake"));
        assert_eq!(fs::read_to_string(&mine).unwrap(), "one both\n");
        fs::write(&tool, "#!/bin/sh\nexit 1\n").unwrap();
        fs::write(&mine, "one mine\n").unwrap();
        assert!(!merge_in_editor(&mine, &base, &new, "fake"));
        assert_eq!(fs::read_to_string(&mine).unwrap(), "one mine\n", "a merge you abandon leaves yours");
        let _ = fs::remove_dir_all(&dir);
    }
}
