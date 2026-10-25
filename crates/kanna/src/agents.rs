use crate::output;
use kanna_client::Failure;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
            say(io, &format!("This Kanna has newer skills.\n\n{}\n\n", output::changed_skills(&reply).unwrap_or_default()));
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
    let text = output::agents(&args, &shown);
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
