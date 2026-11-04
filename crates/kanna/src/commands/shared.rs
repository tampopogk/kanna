use super::find;
use crate::args::Argv;
use kanna_client::{exit, fail, Failure};
use serde_json::{json, Map, Value};

pub fn main_commit(repo: &str) -> Result<String, Failure> {
    ["main", "master"]
        .iter()
        .find_map(|branch| git_line(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}^{{commit}}")]))
        .ok_or_else(|| usage("quiz", "a quiz asks about main, and this repo has no main (or master) branch"))
}

pub fn diff_base(path: &str, reference: Option<String>) -> Result<String, Failure> {
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

pub fn git_line(at: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").arg("-C").arg(at).args(args).output().ok().filter(|out| out.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|line| !line.is_empty())
}

pub fn in_repo(file: &str) -> String {
    let path = absolute(file);
    let path = std::fs::canonicalize(&path).unwrap_or_else(|_| path.into());
    let top = git_line(".", &["rev-parse", "--show-toplevel"]);
    match top.as_deref().and_then(|top| path.strip_prefix(top).ok()) {
        Some(inside) => inside.to_string_lossy().into_owned(),
        None => file.to_string(),
    }
}

pub fn repo_here() -> Result<String, Failure> {
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

pub fn from_caller(out: &mut Map<String, Value>) {
    if out.contains_key("dock") {
        return;
    }
    if let Some(pane) = caller() {
        out.insert("from".into(), json!(pane));
    }
}

pub fn file_lines(target: &str) -> Option<(String, u64, Option<u64>)> {
    let (file, spot) = target.rsplit_once(':')?;
    let (line, last) = match spot.split_once('-') {
        Some((line, last)) => (line.parse::<u64>().ok()?, Some(last.parse::<u64>().ok()?)),
        None => (spot.parse::<u64>().ok()?, None),
    };
    (line > 0 && !file.is_empty()).then(|| (file.to_string(), line, last))
}

pub fn usage(command: &str, problem: &str) -> Failure {
    let usage = find(command).map(|c| c.usage).unwrap_or("");
    fail(exit::USAGE, format!("{command}: {problem}\nusage: {usage}\nmore: kanna help {command}"))
}

pub fn pane(args: &mut Argv, command: &str) -> Result<String, Failure> {
    args.positional().ok_or_else(|| usage(command, "missing PANE (a pane name, or an ID like term-1; see `kanna workspace list --all`)"))
}

pub fn target(args: &mut Argv, command: &str) -> Result<Value, Failure> {
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

pub fn is_edge(value: &str) -> bool {
    value == "left" || value == "right"
}

pub fn is_markdown(target: &str) -> bool {
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

pub fn is_source(target: &str) -> bool {
    !target.contains("://") && std::path::Path::new(&absolute(&file_line(target).0)).is_file()
}

pub fn local_page(target: &str) -> Option<String> {
    if target.contains("://") {
        return None;
    }
    let path = absolute(target);
    std::path::Path::new(&path).is_file().then(|| file_url(&path))
}

pub fn file_url(path: &str) -> String {
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

pub fn no_dock(args: &mut Argv, command: &str) -> Result<(), Failure> {
    if args.optional("--dock", is_edge).is_some() || args.flag("--frozen") {
        return Err(usage(command, "a docked pane is a task's agent: make a task with `kanna task new`, then `kanna task pick` or `kanna task start`"));
    }
    Ok(())
}
