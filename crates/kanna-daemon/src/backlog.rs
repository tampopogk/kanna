use crate::db::{self, Change, Db, Message, Place, Row};
use crate::messages::{self, Channel, Ready, PLANNER, USER};
use crate::task_scripts::{self, Cleanup, Scripts, Setup};
use crate::Event;
use kanna_mux::json::{json, num, opt, string, Map, Value};
use kanna_detect::{Quit, Status, Turn, TURN_SETTLE};
use kanna_mux::{project, CoreHandle, KmuxError, Pane, PaneState, Responder, TaskBook, WeakCore, MAC_APP, TASKS_PER_PROJECT};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::{Rc, Weak};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    /// The worktree of a task moved back to the backlog, where it starts again.
    pub folder: Option<String>,
    /// The agent's session there, which it resumes when it starts again.
    pub session: Option<String>,
    /// The agent's pane of a task moved back to the backlog, still running, which comes back when it starts again.
    pub held: Option<String>,
    pub after: Vec<String>,
    pub stopped: Option<String>,
    pub workspace: Option<String>,
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
            held: row.terminal.clone().filter(|_| row.place == Place::Backlog),
            after: row.doc.get("after").and_then(Value::as_array).map_or(vec![], |ids| ids.iter().filter_map(Value::as_str).map(String::from).collect()),
            stopped: text("stopped").filter(|_| row.place == Place::Backlog),
            workspace: text("workspace"),
        }
    }

    fn command(&self, session: Option<&str>) -> String {
        let command = in_its_terminal(&unprompted(&self.agent));
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
            eprintln!("kanna-daemon: {}: settings version {version} is newer than this Kanna's ({CONFIG_VERSION}); skipping it", file.display());
            continue;
        }
        if let Some(found) = value.get(section).and_then(|s| s.get(key)) {
            match read(found) {
                Some(found) => return Some(found),
                None => eprintln!("kanna-daemon: {}: {section}.{key} is {expected}", file.display()),
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

pub const DEFAULT_AGENT: &str = "claude";

pub fn task_agent(files: &[PathBuf]) -> String {
    let Some((mine, repos)) = files.split_last() else { return DEFAULT_AGENT.into() };
    let known = |value: &Value| value.as_str().map(str::trim).filter(|a| AGENTS.iter().any(|k| k.names.contains(a))).map(String::from);
    let any = |value: &Value| value.as_str().map(str::trim).filter(|a| !a.is_empty()).map(String::from);
    configured(repos, "tasks", "agent", known, "an agent Kanna knows (claude, codex, copilot, opencode, agy): a repo's file can't name a command")
        .or_else(|| configured(std::slice::from_ref(mine), "tasks", "agent", any, "an agent or a command"))
        .unwrap_or_else(|| DEFAULT_AGENT.into())
}

pub const RUNNERS: usize = 3;

const RUNNERS_MAX: usize = 10;

pub fn runners(files: &[PathBuf]) -> usize {
    let read = |value: &Value| value.as_u64().map(|n| n as usize).filter(|n| *n <= RUNNERS_MAX);
    configured(files, "backlog", "runners", read, "a number of tasks from 0 to 10").unwrap_or(RUNNERS)
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
pub fn started_as(program: &str, role: &str, context: Option<u64>) -> String {
    let mut command = in_its_terminal(&unprompted(program));
    if let (Some(flag), Some(tokens)) = (agent(program).and_then(|a| a.compact), context) {
        command = format!("{command} {flag}{tokens}");
    }
    match agent(program).and_then(|a| a.prompt) {
        Some(flag) => format!("{command} {flag} {}", quote(role)),
        None => format!("{command} {}", quote(role)),
    }
}

/// The agents a planning terminal may start, each a name, the program to find, and the command that starts it as the planner:
/// every agent Kanna knows, or for tests the programs in `KANNA_PLANNING_AGENTS`.
pub fn planner_agents(context: Option<u64>) -> Vec<(String, String, String)> {
    let programs: Vec<String> = match std::env::var("KANNA_PLANNING_AGENTS") {
        Ok(list) => list.split_whitespace().map(String::from).collect(),
        Err(_) => AGENTS.iter().map(|a| a.names[0].to_string()).collect(),
    };
    programs.iter().map(|p| (program(p).to_string(), p.split_whitespace().next().unwrap_or(p).to_string(), started_as(p, PLANNER_ROLE, context))).collect()
}

/// The planning terminal's command line: it keeps a claude agent's context to `context` tokens, then runs the planner
/// `script` (agent setup with `cli`, where to work when it's in `~`, which agent, the agent), which ends in your shell.
/// Without the script, it asks about agent setup and hands over to your shell.
/// Kanna's zsh integration (`zsh`, its folder) hands ZDOTDIR back to yours while the command runs, so the shell it
/// hands over to gets it again, as every Kanna terminal's does.
pub fn planner_command(script: Option<&str>, context: Option<u64>, zsh: Option<&str>, cli: Option<&str>, repos: &Path, agents: &[(String, String, String)]) -> String {
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
        for (name, program, start) in agents {
            command += &format!(" {} {} {}", quote(name), quote(program), quote(start));
        }
        return command;
    }
    let integrate = zsh.map_or(String::new(), |dir| format!("case \"${{SHELL##*/}}\" in zsh) [ -z \"${{ZDOTDIR+x}}\" ] || export KANNA_ZSH_ZDOTDIR=\"$ZDOTDIR\"; export ZDOTDIR={};; esac; ", quote(dir)));
    let ask = cli.map_or(String::new(), |cli| format!("{cli} agents setup --ask; {cli} agents update --ask; ", cli = quote(cli)));
    format!("{limit}{ask}{integrate}exec \"${{SHELL:-/bin/sh}}\" -l")
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
    Agent { names: &["codex"], unprompted: Some("--yolo"), prompt: None, named: None, resume: "resume", latest: Some("resume --last"), find: None, compact: Some("-c model_auto_compact_token_limit=") },
    Agent { names: &["copilot"], unprompted: Some("--yolo"), prompt: Some("-i"), named: Some("--session-id"), resume: "--session-id", latest: None, find: Some(copilot_session), compact: None },
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

fn copilot_session(folder: &Path) -> Option<String> {
    let state = std::env::var_os("COPILOT_HOME").map_or_else(|| home().join(".copilot"), PathBuf::from).join("session-state");
    latest_copilot_session(&state, folder)
}

fn latest_copilot_session(state: &Path, folder: &Path) -> Option<String> {
    let field = |text: &str, key: &str| text.lines().find_map(|l| l.strip_prefix(key)).map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string());
    let mut sessions: Vec<(String, String)> = fs::read_dir(state)
        .ok()?
        .flatten()
        .filter_map(|dir| {
            let file = dir.path().join("workspace.yaml");
            let text = fs::metadata(&file).ok().filter(|m| m.len() <= 64 << 10).and_then(|_| fs::read_to_string(&file).ok())?;
            let cwd = field(&text, "cwd:")?;
            same_folder(Path::new(&cwd), folder).then_some(())?;
            Some((field(&text, "updated_at:").unwrap_or_default(), field(&text, "id:")?))
        })
        .collect();
    sessions.sort();
    sessions.pop().map(|(_, id)| id)
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

const ASKING: &[&str] = &["--dangerously-skip-permissions", "--yolo", "--permission-mode", "--ask-for-approval", "-a", "--allow-all", "--allow-all-tools", "--mode"];

fn unprompted(agent: &str) -> String {
    let agent = agent.trim();
    let chooses = agent.split_whitespace().skip(1).any(|w| ASKING.contains(&w.split('=').next().unwrap_or(w)));
    match self::agent(agent).and_then(|a| a.unprompted) {
        Some(flag) if !chooses => format!("{agent} {flag}"),
        _ => agent.to_string(),
    }
}

static CODEX_REMOTE: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();

pub fn serve_codex_remotely(helper: &Path, dir: &Path) {
    let _ = CODEX_REMOTE.set((quote(&helper.to_string_lossy()), quote(&dir.to_string_lossy())));
}

fn in_its_terminal(agent: &str) -> String {
    wrapped(agent, CODEX_REMOTE.get())
}

fn wrapped(agent: &str, remote: Option<&(String, String)>) -> String {
    match (program(agent) == "codex" && !agent.split_whitespace().any(|w| w == "--no-daemon"), remote) {
        (true, Some((helper, dir))) => crate::codex_remote::command(helper, dir, agent),
        (true, None) => format!("{agent} --no-daemon"),
        (false, _) => agent.to_string(),
    }
}


fn who(args: &Value) -> Option<&str> {
    Some(string(args, "by").filter(|by| !by.is_empty()).unwrap_or("user"))
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64().floor())
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
    KmuxError::new("not_found", format!("no task \"{id}\" in the backlog"))
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

/// Makes a worktree in `.kanna-worktrees/NAME` on a new branch NAME, and returns its folder and branch.
/// In a repo with no commits yet, the branch starts empty (an orphan), so its first commit is the project's.
pub fn make_worktree(repo: &Path, name: &str) -> Result<(String, String), String> {
    let git = |args: &[&str]| git(repo, args);
    git(&["rev-parse", "--show-toplevel"]).map_err(|_| format!("{} isn't a git repo", repo.display()))?;
    let born = git(&["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();
    let common = git(&["rev-parse", "--git-common-dir"])?;
    let exclude = repo.join(common).join("info/exclude");
    let listed = fs::read_to_string(&exclude).unwrap_or_default();
    let missing: Vec<String> = [format!("/{WORKTREES}/"), format!("/{SCRATCH}/")].into_iter().filter(|line| !listed.lines().any(|l| l.trim() == line)).collect();
    if !missing.is_empty() {
        let _ = fs::create_dir_all(exclude.parent().unwrap_or(repo));
        let start = if listed.is_empty() || listed.ends_with('\n') { "" } else { "\n" };
        let text = format!("{listed}{start}{}\n", missing.join("\n"));
        fs::write(&exclude, text).map_err(|e| format!("can't write {}: {e}", exclude.display()))?;
    }
    if taken(repo, name) {
        return Err(format!("{WORKTREES}/{name} or the branch {name} is there already"));
    }
    let folder = repo.join(WORKTREES).join(name).to_string_lossy().to_string();
    if born {
        git(&["worktree", "add", "-b", name, &folder])?;
    } else {
        git(&["worktree", "add", "--orphan", "-b", name, &folder])?;
    }
    task_scripts::record_base(Path::new(&folder));
    Ok((folder, name.to_string()))
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
    backlog: bool,
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

pub struct Backlog {
    state: PathBuf,
    db: Rc<Db>,
    entries: RefCell<Vec<Entry>>,
    running: RefCell<Vec<Running>>,
    stopping: RefCell<HashSet<String>>,
    dirty: Cell<bool>,
    events: Sender<Event>,
    core: WeakCore,
    pending: RefCell<HashMap<u64, Starting>>,
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
    pub permissions: RefCell<Option<Box<dyn Fn(&str) -> Option<&'static str>>>>,
    pub line_mode: RefCell<Option<Box<dyn Fn(&str) -> bool>>>,
    warned: RefCell<HashMap<String, String>>,
    delivering: RefCell<HashSet<String>>,
    posting: RefCell<HashMap<u64, (String, Vec<Message>)>>,
    pub on_change: RefCell<Option<Box<dyn Fn(Value)>>>,
}

struct Book(Weak<Backlog>);

impl TaskBook for Book {
    fn workspace_of(&self, reference: &str) -> Option<Result<String, KmuxError>> {
        self.0.upgrade()?.workspace_of(reference)
    }

    fn task_of(&self, workspace: &str) -> Option<(String, Option<String>)> {
        self.0.upgrade()?.task_in(workspace)
    }

    fn sync(&self, workspaces: &[Value]) {
        if let Some(backlog) = self.0.upgrade() {
            backlog.sync_workspaces(workspaces);
        }
    }
}

impl Backlog {
    pub fn open(db: Rc<Db>, state_dir: &Path, core: WeakCore, events: Sender<Event>) -> Backlog {
        let rows = db.tasks().unwrap_or_else(|error| {
            eprintln!("kanna-daemon: can't read the tasks in {}: {error}", state_dir.join(db::FILE).display());
            vec![]
        });
        let (mut entries, mut running) = (vec![], vec![]);
        for row in &rows {
            match (row.place, &row.terminal) {
                (Place::Shelf, Some(pane)) => running.push(Running { pane: pane.clone(), entry: Entry::from_row(row), place: row.place }),
                (Place::Shelf, None) => {
                    let entry = Entry::from_row(row);
                    let back = Change::new("deferred").moved(row.place, Place::Backlog).by(Some("daemon"));
                    if let Err(error) = db.put(&entry.row(Place::Backlog, None), Some(&back)) {
                        eprintln!("kanna-daemon: couldn't put {} back on the backlog: {error}", entry.id);
                    }
                    entries.push(entry);
                }
                _ => entries.push(Entry::from_row(row)),
            }
        }
        let waiting = db.waiting_messages().unwrap_or_else(|error| {
            eprintln!("kanna-daemon: can't read the waiting messages in {}: {error}", state_dir.join(db::FILE).display());
            vec![]
        });
        Backlog {
            state: state_dir.to_path_buf(),
            db,
            entries: RefCell::new(entries),
            running: RefCell::new(running),
            stopping: RefCell::default(),
            dirty: Cell::new(true),
            events,
            core,
            pending: RefCell::default(),
            next: Cell::new(0),
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
            warned: RefCell::default(),
            delivering: RefCell::default(),
            posting: RefCell::default(),
            on_change: RefCell::default(),
        }
    }

    pub fn view(&self) -> Value {
        let tasks = self.entries.borrow().clone().iter().map(|e| self.described(e)).collect();
        json!({ "tasks": Value::Array(tasks) })
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
                    moved.push((e.clone(), Place::Backlog, pane));
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
        Ok(self.db.task(reference).ok().flatten().map(|row| row.id))
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

    fn short(&self, id: &str) -> String {
        db::short_id(id, self.open_tasks().iter().map(|e| e.id.as_str()))
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
        entry.after.iter().filter(|id| self.is_open(id)).cloned().collect()
    }

    fn label(&self, id: &str) -> String {
        match self.known(id).and_then(|e| e.name) {
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
            let after = self.entries.borrow().iter().find(|e| e.id == id).map(|e| e.after.clone()).unwrap_or_default();
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
        value
    }

    fn blockers(&self, args: &Value, me: &str) -> Result<Option<Vec<String>>, KmuxError> {
        Ok(match args.get("after") {
            None | Some(Value::Null) => None,
            Some(Value::Array(list)) => {
                let mut ids: Vec<String> = vec![];
                for reference in list.iter().filter_map(Value::as_str) {
                    let id = self.resolve(reference)?.ok_or_else(|| not_found(reference))?;
                    if !self.is_open(&id) {
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

    fn ready(&self, entry: &Entry) -> Result<(), KmuxError> {
        if entry.folder.is_some() || entry.held.is_some() {
            return Ok(());
        }
        let waits = self.waits(entry);
        if !waits.is_empty() {
            let names: Vec<String> = waits.iter().map(|id| self.label(id)).collect();
            let them = if waits.len() > 1 { "they're" } else { "it's" };
            return Err(KmuxError::new("blocked", format!("{} waits for {}: it can start once {them} done", self.short(&entry.id), names.join(", "))));
        }
        Ok(())
    }

    fn edit(&self, args: &Value) -> Result<Map, KmuxError> {
        let reference = string(args, "task").ok_or_else(|| bad("say which task"))?;
        let Some(mut entry) = self.entries.borrow().iter().find(|e| e.id == reference).cloned() else {
            return Err(KmuxError::new("not_found", format!("no backlog task \"{reference}\"")));
        };
        if entry.folder.is_some() {
            return Err(KmuxError::new("started", format!("{} has started: its branch is made, and nothing holds it back now", self.short(reference))));
        }
        entry.after = self.blockers(args, &entry.id)?.ok_or_else(|| bad("say what it waits for: after"))?;
        let change = Change::new("edited").by(who(args)).with("after", json!(entry.after));
        self.store(&entry, Place::Backlog, entry.held.as_deref(), Some(change))?;
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
            let Some(pane) = running else { return responder.reply(Err(KmuxError::new("not_found", format!("no task \"{reference}\" in the backlog or on the Shelf")))) };
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
            if let Err(error) = this.store(&entry, Place::Backlog, entry.held.as_deref(), Some(change)) {
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
        self.db.put(&entry.row(place, pane), change.as_ref()).map_err(|error| {
            eprintln!("kanna-daemon: couldn't save {} in {}: {error}", entry.id, db::FILE);
            KmuxError::new("failed", format!("couldn't save {}: {error}", entry.id))
        })
    }

    fn note(&self, pane: &str, change: Change) {
        let Some(id) = self.task_of(pane) else { return };
        if let Err(error) = self.db.log(&id, &change) {
            eprintln!("kanna-daemon: couldn't save {id}'s {} in {}: {error}", change.what, db::FILE);
        }
    }

    fn task_of(&self, pane: &str) -> Option<String> {
        let running = self.running.borrow().iter().find(|r| r.pane == pane).map(|r| r.entry.id.clone());
        running.or_else(|| self.entries.borrow().iter().find(|e| e.held.as_deref() == Some(pane)).map(|e| e.id.clone()))
    }

    fn changed(&self) {
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
        self.ready.borrow_mut().entry(pane.to_string()).or_insert_with(|| Ready::new(now)).see(status.and_then(Status::parse), ended, now);
        if let Some(turn) = self.turns.borrow_mut().get_mut(pane) {
            let status = status.and_then(Status::parse);
            if ended {
                turn.reported_end(status, Instant::now());
            } else {
                turn.see(status, Instant::now());
            }
        }
    }

    fn refill(&self) {
        self.fill_at.set(Some(Instant::now()));
    }

    fn running_in_backlog(&self, core: &CoreHandle) -> Vec<(String, Entry)> {
        self.entries.borrow().iter().filter_map(|e| e.held.clone().filter(|pane| alive(core, pane)).map(|pane| (pane, e.clone()))).collect()
    }

    pub fn tick(self: &Rc<Self>) -> Option<Instant> {
        let now = Instant::now();
        let core = self.core.upgrade()?;
        let running = self.running_in_backlog(&core);
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
            eprintln!("kanna-daemon: {agent} in {pane} didn't quit in {}s, so it's hung up", QUIT_WAIT.as_secs());
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
        let over: Vec<(String, Status)> = self.turns.borrow().iter().filter(|(pane, _)| !(self.has_mail(pane) && self.ready.borrow().get(*pane).is_some_and(|r| r.at(TURN_SETTLE).is_some()))).filter(|(pane, _)| !self.setting_up.borrow().contains_key(*pane)).filter_map(|(pane, turn)| turn.ended(now, TURN_SETTLE).map(|why| (pane.clone(), why))).collect();
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
        }
        let turns = self.turns.borrow().values().filter_map(|t| t.due(TURN_SETTLE)).min();
        let quits = self.quitting.borrow().values().filter(|q| !q.hung_up).flat_map(|q| [Some(q.by), q.next]).flatten().min();
        let setups = (!self.setting_up.borrow().is_empty()).then(|| now + SETUP_EVERY);
        [turns, quits, setups, self.fill_at.get(), mail].into_iter().flatten().min()
    }

    fn quit(self: &Rc<Self>, pane: &str, entry: &Entry, why: Status) {
        let Some(core) = self.core.upgrade() else { return };
        let why = if why == Status::NeedsYou { "needs-you" } else { "finished" };
        eprintln!("kanna-daemon: {}'s agent {pane} is {why}, so it stops", entry.id);
        let (this, id) = (Rc::downgrade(self), pane.to_string());
        self.end_agent(pane, &entry.agent, move |session| {
            if let Some(this) = this.upgrade() {
                this.stopped_by_itself(&id, why, session);
            }
        });
        core.hang_up(pane, |_| {});
    }

    fn end_agent(self: &Rc<Self>, pane: &str, agent: &str, then: impl FnOnce(Option<String>) + 'static) {
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
            eprintln!("kanna-daemon: {agent} in {pane} has no quit keys, so it's hung up");
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
                eprintln!("kanna-daemon: couldn't type {agent}'s quit keys in {pane} ({}), so it's hung up", error.message);
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
            (e.clone(), Place::Backlog)
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
        let _ = self.store(&entry, Place::Backlog, Some(pane), Some(change));
        if let Some(found) = self.entries.borrow_mut().iter_mut().find(|e| e.id == entry.id) {
            *found = entry;
        }
        self.changed();
        self.refill();
    }

    fn fill(self: &Rc<Self>, core: &CoreHandle) {
        let entries = self.entries.borrow().clone();
        let mut repos: Vec<&str> = entries.iter().map(|e| e.repo.as_str()).collect();
        repos.sort();
        repos.dedup();
        let starting: HashSet<String> = self.pending.borrow().values().map(|p| p.entry.id.clone()).chain(self.launching.borrow().iter().cloned()).collect();
        for repo in repos {
            let slots = runners(&config_files(Path::new(repo)));
            if slots == 0 {
                continue;
            }
            let mine: Vec<&Entry> = entries.iter().filter(|e| e.repo == repo).collect();
            let mut busy = mine.iter().filter(|e| starting.contains(&e.id) || e.held.as_deref().is_some_and(|pane| alive(core, pane))).count();
            let mail: HashSet<String> = self.waiting.borrow().iter().map(|m| m.recipient.clone()).collect();
            let ending = |pane: &str| self.quitting.borrow().contains_key(pane) || self.stopping.borrow().contains(pane) || self.tearing_down.borrow().contains_key(pane);
            let mut woken: Vec<&&Entry> = mine.iter().filter(|e| mail.contains(&e.id) && messages::AGENTS.contains(&program(&e.agent)) && (e.folder.is_some() || e.held.is_some()) && !starting.contains(&e.id) && !self.failed.borrow().contains(&e.id) && !e.held.as_deref().is_some_and(|pane| alive(core, pane) || ending(pane))).collect();
            woken.sort_by(|a, b| a.made_at.total_cmp(&b.made_at));
            for entry in woken {
                if busy >= slots {
                    break;
                }
                busy += 1;
                eprintln!("kanna-daemon: {} has a message, so it runs in the backlog ({busy} of {slots} in {repo})", entry.id);
                self.wake(core, (*entry).clone());
            }
            let mut waiting: Vec<&&Entry> = mine.iter().filter(|e| e.folder.is_none() && e.held.is_none() && !starting.contains(&e.id) && !self.failed.borrow().contains(&e.id) && self.waits(e).is_empty()).collect();
            waiting.sort_by(|a, b| mail.contains(&b.id).cmp(&mail.contains(&a.id)).then(a.made_at.total_cmp(&b.made_at)));
            for entry in waiting {
                if busy >= slots {
                    break;
                }
                busy += 1;
                let (this, id) = (Rc::downgrade(self), entry.id.clone());
                eprintln!("kanna-daemon: {id} runs in the backlog ({busy} of {slots} in {repo})");
                self.prepare((*entry).clone(), None, Some("daemon".into()), Answer::Then(Box::new(move |result| {
                    if let (Err(error), Some(this)) = (result, this.upgrade()) {
                        eprintln!("kanna-daemon: {id} couldn't run in the backlog: {}", error.message);
                        this.failed.borrow_mut().insert(id);
                    }
                })));
            }
        }
    }

    fn wake(self: &Rc<Self>, core: &CoreHandle, entry: Entry) {
        let id = entry.id.clone();
        let this = Rc::downgrade(self);
        let failed = move |error: KmuxError| {
            eprintln!("kanna-daemon: {id} couldn't run for its message: {}", error.message);
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
            eprintln!("kanna-daemon: {}'s agent {} is closed, so {} is done", r.entry.id, r.pane, r.entry.id);
            self.running.borrow_mut().retain(|x| x.pane != r.pane);
            let _ = self.store(&r.entry, Place::Done, None, Some(Change::new("done").moved(r.place, Place::Done).by(Some("daemon")).with("pane", json!(r.pane))));
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
            let _ = self.store(&entry, Place::Backlog, None, None);
        }
        if freed {
            self.changed();
        }
    }

    fn mint_for(&self, pane: &Pane) -> Result<Entry, KmuxError> {
        let Some(folder) = pane.opened_in.clone().or(pane.cwd.clone()) else { return Err(bad(format!("{} has no folder to come back to", pane.id))) };
        let repo = project(Some(&folder)).map(|r| r.to_string_lossy().to_string()).unwrap_or_default();
        let agent = pane.command.as_deref().map(program).filter(|p| !p.is_empty()).map(String::from);
        let args = json!({ "repo": repo, "agent": opt(agent.as_deref()) });
        Ok(Entry { folder: Some(folder), ..self.mint(&args, pane.name.clone(), None)? })
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
                eprintln!("kanna-daemon: couldn't save {}'s new terminal {to}: {error}", entry.id);
            }
        }
        let held: Vec<Entry> = self.entries.borrow_mut().iter_mut().filter(|e| e.held.as_deref() == Some(from)).map(|e| {
            e.held = Some(to.into());
            e.clone()
        }).collect();
        for entry in held {
            if let Err(error) = self.db.put(&entry.row(Place::Backlog, Some(to)), None) {
                eprintln!("kanna-daemon: couldn't save {}'s new terminal {to}: {error}", entry.id);
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
            "backlog",
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
                    Some("name") => this.name(&args, responder),
                    Some("list") => responder.reply(Ok(this.list(&args))),
                    Some("done") => this.done(&args, responder),
                    Some("start") => this.start(&args, responder),
                    Some("stop") => this.stop(&args, responder),
                    Some("setup") => this.setup(&args, responder),
                    Some("shelve") => this.shelve(&args, responder),
                    Some("defer") => this.defer(&args, responder),
                    Some("plan") => this.plan(&args, responder),
                    Some("events") => responder.reply(this.events(&args)),
                    Some("send") => responder.reply(this.send(&args)),
                    Some("messages") => responder.reply(this.messages(&args)),
                    _ => responder.reply(Err(bad("backlog needs an action: new, edit, name, list, resolve, done, start, stop, setup, shelve, defer, plan, events, send or messages"))),
                }
            }),
        );
    }

    fn find(&self, args: &Value) -> Result<Map, KmuxError> {
        let id = string(args, "task").ok_or_else(|| bad("say which task"))?;
        let waiting = self.entries.borrow().iter().find(|e| e.id == id).map(|e| (e.clone(), Place::Backlog, e.held.clone()));
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
        let message = self.db.put_message(&recipient, &repo, &from, text, mode).map_err(|error| KmuxError::new("failed", format!("couldn't save the message in {}: {error}", db::FILE)))?;
        eprintln!("kanna-daemon: a message from {from} waits for {recipient}");
        self.waiting.borrow_mut().push(message.clone());
        self.refill();
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
            eprintln!("kanna-daemon: warning: the message for {recipient} waits: {why}");
        }
        let to = if recipient == PLANNER { json!(PLANNER) } else { json!({ "id": recipient, "short": self.short(&recipient) }) };
        Ok(Map::from_iter([("message".to_string(), json!(message.id)), ("to".to_string(), to), ("state".to_string(), json!(state)), ("warning".to_string(), opt(warning.as_deref()))]))
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
        let repo = Path::new(&message.repo);
        planners.iter().cloned().find(|pane| alive(&core, pane) && core.pane(pane).and_then(|p| project(p.cwd.as_deref())).is_some_and(|r| same_folder(&r, repo)))
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
                ready.see(core.pane(pane).and_then(|p| p.status).as_deref().and_then(Status::parse), false, now);
                self.ready.borrow_mut().insert(pane.clone(), ready);
            }
        }
        for (pane, list) in by_pane {
            if self.delivering.borrow().contains(&pane) || self.quitting.borrow().contains_key(&pane) || self.setting_up.borrow().contains_key(&pane) {
                continue;
            }
            let Some(channel) = self.channel(&pane) else { continue };
            let at = self.ready.borrow().get(&pane).and_then(|r| r.at(TURN_SETTLE));
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
            eprintln!("kanna-daemon: warning: the messages for {pane} wait: {why}");
            self.warned.borrow_mut().insert(pane.to_string(), why);
        }
    }

    fn hand_over(self: &Rc<Self>, pane: String, list: Vec<Message>, channel: Channel) {
        let named = |who: &str| if db::is_task_id(who) { self.label(who) } else { who.to_string() };
        let text = messages::prompt(&list.iter().map(|m| (named(&m.sender), m.text.clone())).collect::<Vec<_>>());
        let theirs = self.permissions(&pane);
        let modes: Option<Vec<&str>> = list.iter().map(|m| if m.sender == USER { theirs } else { m.mode.as_deref() }).collect();
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
            return eprintln!("kanna-daemon: couldn't give {pane} its messages ({error}); they wait");
        }
        self.warned.borrow_mut().remove(&pane);
        let ids: Vec<i64> = list.iter().map(|m| m.id).collect();
        if let Err(error) = self.db.delivered(&ids) {
            eprintln!("kanna-daemon: couldn't mark {pane}'s messages delivered in {}: {error}", db::FILE);
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
        eprintln!("kanna-daemon: gave {pane} {} message(s)", ids.len());
    }

    fn mint(&self, args: &Value, name: Option<String>, message: Option<String>) -> Result<Entry, KmuxError> {
        let repo = string(args, "repo").filter(|r| Path::new(r).is_absolute() && Path::new(r).is_dir()).ok_or_else(|| bad("a task needs its repo: an absolute path to a folder"))?;
        if git(Path::new(repo), &["rev-parse", "--show-toplevel"]).is_err() {
            return Err(bad(format!("{repo} isn't a git repo: a task works in a worktree of the repo, so set one up first (git init)")));
        }
        let by = string(args, "by").and_then(|by| self.core.upgrade()?.pane(by)).map(|pane| pane.id);
        let id = self.db.new_id(|id| !taken(Path::new(repo), id)).map_err(|error| {
            eprintln!("kanna-daemon: couldn't save a new task ID in {}: {error}", db::FILE);
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
            ..Default::default()
        })
    }

    fn add(&self, args: &Value) -> Result<Map, KmuxError> {
        let name = string(args, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad("a task needs a name"))?.to_string();
        let message = string(args, "message").map(str::trim).filter(|m| !m.is_empty()).map(String::from);
        let after = self.blockers(args, "")?.unwrap_or_default();
        let entry = Entry { after, ..self.mint(args, Some(name), message)? };
        self.store(&entry, Place::Backlog, None, Some(Change { to: Some(Place::Backlog), ..Change::new("made") }.by(entry.by.as_deref().or(who(args)))))?;
        let value = self.described(&entry);
        self.entries.borrow_mut().push(entry);
        self.changed();
        self.refill();
        Ok(Map::from_iter([("task".to_string(), value)]))
    }

    fn list(&self, args: &Value) -> Map {
        let repo = string(args, "repo");
        let core = self.core.upgrade();
        let entries = self.entries.borrow().clone();
        let tasks = entries
            .iter()
            .filter(|e| repo.is_none_or(|r| e.repo == r))
            .map(|e| {
                let mut value = self.described(e);
                let pane = e.held.as_deref().zip(core.as_ref()).and_then(|(pane, core)| core.pane(pane).map(|p| (alive(core, pane), p)));
                value["running"] = json!(pane.as_ref().is_some_and(|(running, _)| *running));
                value["status"] = opt(pane.as_ref().and_then(|(_, p)| p.status()));
                value
            })
            .collect();
        let out = self.running.borrow().iter().filter(|r| repo.is_none_or(|repo| r.entry.repo == repo)).map(|r| json!({ "id": r.entry.id, "short": self.short(&r.entry.id), "pane": r.pane })).collect();
        let shelf: Map = self
            .running
            .borrow()
            .iter()
            .filter(|r| repo.is_none_or(|repo| r.entry.repo == repo))
            .filter_map(|r| {
                let running = core.as_ref().is_some_and(|core| alive(core, &r.pane));
                self.script_said(&r.entry, Some(&r.pane), running).map(|said| (r.pane.clone(), json!(said)))
            })
            .collect();
        Map::from_iter([("tasks".to_string(), Value::Array(tasks)), ("out".to_string(), Value::Array(out)), ("script".to_string(), Value::Object(shelf))])

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
            (None, Some(pane)) => core.handle(json!({ "cmd": "close", "args": { "task": pane } }), move |reply| match failed(&reply) {
                Some(error) => responder.reply(Err(error)),
                None => responder.reply(Ok(reply.as_object().cloned().map(|mut r| {
                    r.remove("ok");
                    r
                }).unwrap_or_default())),
            }),
            (None, None) => responder.reply(Err(not_found(reference))),
        }
    }

    /// Takes a task off the backlog, ending its agent and tabs if it was moved there still running.
    fn take_off(self: &Rc<Self>, entry: Entry, by: Option<String>, responder: Answer) {
        if let Err(error) = self.store(&entry, Place::Done, None, Some(Change::new("done").moved(Place::Backlog, Place::Done).by(by.as_deref()))) {
            return responder.reply(Err(error));
        }
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

    fn teardown_of(&self, entry: &Entry, backlog: bool, by: Option<String>) -> Option<TearingDown> {
        let worktree = entry.folder.clone().map(PathBuf::from).filter(|f| f.is_dir() && task_scripts::is_linked(f))?;
        let scripts = scripts(&config_files(Path::new(&entry.repo)));
        scripts.teardown.as_ref()?;
        Some(TearingDown { entry: entry.clone(), worktree, timeout: scripts.timeout, backlog, by })
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
        eprintln!("kanna-daemon: {} is done, so its teardown runs in {pane}: {}", tearing.entry.id, scripts.teardown.as_deref().unwrap_or(""));
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
                    eprintln!("kanna-daemon: couldn't run {task}'s teardown: {}", error.message);
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
        eprintln!("kanna-daemon: {id}'s teardown exited {code}");
        self.note(pane, Change::new("torn down").by(Some("daemon")).with("code", json!(code)));
        self.changed();
        let Some(core) = self.core.upgrade() else { return };
        if code != 0 {
            let said = task_scripts::said("teardown", code, tearing.timeout);
            eprintln!("kanna-daemon: {id}'s {said}, so it isn't done: its pane shows why");
            self.note(pane, Change::new("teardown failed").by(Some("daemon")).with("code", json!(code)).with("why", json!(said)));
            core.set_command(pane, tearing.entry.clone().resumable().command(None));
            return;
        }
        if !tearing.backlog {
            return core.handle(json!({ "cmd": "close", "args": { "task": pane } }), |_| {});
        }
        let Some(entry) = self.entries.borrow().iter().find(|e| e.id == id).cloned() else { return };
        self.take_off(entry, tearing.by, Answer::Then(Box::new(|_| {})));
    }

    fn plan(&self, args: &Value, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
        let app = string(args, "app").unwrap_or(MAC_APP);
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let folder = string(args, "folder").filter(|f| Path::new(f).is_absolute() && Path::new(f).is_dir()).unwrap_or(&home);
        let zsh = Some(self.state.join("shell/zsh")).filter(|dir| dir.join(".zshenv").is_file());
        let script = string(args, "script").filter(|s| Path::new(s).is_file());
        let repos = crate::agents::home().join(".kanna-v4/repos");
        let context = planning_context(&config_files(Path::new(folder)));
        let cmd = planner_command(script, context, zsh.as_deref().and_then(Path::to_str), cli().as_deref().and_then(Path::to_str), &repos, &planner_agents(context));
        let open = json!({ "type": "term", "planner": true, "claim": app, "cwd": folder, "cmd": cmd, "wait": false });
        core.handle(json!({ "cmd": "open", "args": open }), move |reply| match failed(&reply) {
            Some(error) => responder.reply(Err(error)),
            None => responder.reply(Ok(Map::from_iter([("pane".to_string(), reply.get("pane").and_then(|p| p.get("id")).cloned().unwrap_or(Value::Null)), ("workspace".to_string(), reply.get("workspace").and_then(|w| w.get("id")).cloned().unwrap_or(Value::Null))]))),
        });
    }

    fn shelve(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(id) = string(args, "task") else { return responder.reply(Err(bad("say which task"))) };
        let Some(entry) = self.entries.borrow().iter().find(|e| e.id == id).cloned() else { return responder.reply(Err(not_found(id))) };
        if self.pending.borrow().values().any(|p| p.entry.id == entry.id) {
            return responder.reply(Err(bad(format!("{id} is starting"))));
        }
        if let Err(error) = self.ready(&entry) {
            return responder.reply(Err(error));
        }
        self.room(entry, string(args, "app").unwrap_or(MAC_APP).to_string(), who(args).map(String::from), Answer::To(responder));
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
            if let Err(error) = self.ready(&entry) {
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
                    responder.reply(result.map(|pane| started_here(&id, &pane, "backlog")))
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
            let id = entry.as_ref().map_or_else(|| pane.clone(), |e| e.id.clone());
            let again = Rc::downgrade(&this);
            let command = entry.map(|e| this.again(&e));
            this.revive(&pane.clone(), command, move |result| {
                if let (Ok(pane), Some(this)) = (&result, again.upgrade()) {
                    this.note(pane, Change::new("started").by(by.as_deref()));
                }
                responder.reply(result.map(|pane| started_here(&id, &pane, "shelf")))
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
            let (weak, backlog) = (core.weak(), Rc::downgrade(&this));
            let agent = this.pane_entry(&pane).map_or_else(|| core.pane(&pane).and_then(|p| p.command).unwrap_or_default(), |e| e.agent);
            this.end_agent(&pane.clone(), &agent, move |session| {
                let (Some(backlog), Some(core)) = (backlog.upgrade(), weak.upgrade()) else { return responder.reply(Err(gone())) };
                backlog.keep_session(&pane, session);
                let session = backlog.pane_entry(&pane).and_then(|e| e.session);
                backlog.note(&pane, Change::new("stopped").by(by.as_deref()).with("session", opt(session.as_deref())));
                backlog.refill();
                let out = Map::from_iter([("task".to_string(), json!(task)), ("pane".to_string(), json!(pane)), ("place".to_string(), json!(place))]);
                core.hang_up(&pane.clone(), move |hung_up| responder.reply(hung_up.map(|_| out)));
            });
        };
        if let Some(entry) = found {
            let pane = entry.held.clone().filter(|pane| self.core.upgrade().is_some_and(|core| core.pane(pane).is_some()));
            return match pane {
                Some(pane) => end(entry.id, pane, "backlog", responder),
                None => responder.reply(Err(KmuxError::new("stopped", format!("{} is stopped: start it with `kanna task start {}`", entry.id, entry.id)))),
            };
        }
        let task = self.is_open(reference).then(|| reference.to_string());
        self.task_pane(&self.pane_or(reference), move |found| match found {
            Ok(pane) => end(task.unwrap_or_else(|| pane.clone()), pane, "shelf", responder),
            Err(error) => responder.reply(Err(error)),
        });
    }

    fn restarted(&self, pane: &str) {
        self.turns.borrow_mut().insert(pane.to_string(), Turn::default());
        let Some(mut entry) = self.entries.borrow().iter().find(|e| e.held.as_deref() == Some(pane)).cloned() else { return };
        if entry.stopped.take().is_some() {
            let _ = self.store(&entry, Place::Backlog, Some(pane), None);
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
            then(if listed { Ok(pane) } else { Err(KmuxError::new("not_a_task", format!("{reference} isn't a task: a task is an agent docked on a Desk or the Shelf, or in the backlog"))) })
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
                    eprintln!("kanna-daemon: couldn't start {task}'s terminals again: {}", error.message);
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
        eprintln!("kanna-daemon: {}'s setup runs in {} before its agent: {setup}", entry.id, folder.display());
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
                eprintln!("kanna-daemon: {}'s setup failed (exit {code}), and its agent starts anyway", entry.id);
                Self::first_command(entry)
            }
            _ => entry.clone().resumable().command(None),
        }
    }

    fn set_up(&self, pane: &str, setting: &SettingUp) {
        let task = self.task_of(pane).unwrap_or_else(|| pane.to_string());
        match task_scripts::setup(&setting.worktree) {
            Some(Setup::Done) => {
                eprintln!("kanna-daemon: {task} is set up, so its agent starts");
                self.note(pane, Change::new("set up").by(Some("daemon")).with("code", json!(0)));
            }
            Some(Setup::Failed(code)) => {
                let said = task_scripts::said("setup", code, setting.timeout);
                eprintln!("kanna-daemon: {task}'s {said}, so it stops");
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
        let Some(folder) = entry.folder.clone().map(PathBuf::from).filter(|f| f.is_dir()) else { return };
        if !task_scripts::is_linked(&folder) {
            return;
        }
        let id = entry.id.clone();
        let kept = |why: String| {
            eprintln!("kanna-daemon: kept {id}'s worktree {} and its branch: {why}", folder.display());
            Change::new("kept").by(Some("daemon")).with("why", json!(why))
        };
        if keep {
            return self.finished(&id, vec![kept("kanna task done --keep".into())]);
        }
        let scripts = scripts(&config_files(Path::new(&entry.repo)));
        let teardown = match task_scripts::torn_down(&folder) {
            Some(0) => None,
            Some(code) => return self.finished(&id, vec![kept(task_scripts::said("teardown", code, scripts.timeout))]),
            None => scripts.teardown.clone(),
        };
        let task = Self::script_task(entry, &folder);
        let events = self.events.clone();
        std::thread::spawn(move || {
            let id = task.id.clone();
            let mut changes = vec![];
            if let Some(teardown) = &teardown {
                eprintln!("kanna-daemon: {id} is done with no pane to tear down in, so its teardown runs in {}: {teardown}", folder.display());
                let code = task_scripts::run(teardown, &folder, &task.vars(&task.branch()), Duration::from_secs(scripts.timeout)).unwrap_or_else(|error| {
                    eprintln!("kanna-daemon: {id}'s teardown: {error}");
                    127
                });
                eprintln!("kanna-daemon: {id}'s teardown exited {code}");
                changes.push(Change::new("torn down").by(Some("daemon")).with("code", json!(code)));
                if code != 0 {
                    let why = task_scripts::said("teardown", code, scripts.timeout);
                    eprintln!("kanna-daemon: kept {id}'s worktree {} and its branch: {why}", folder.display());
                    changes.push(Change::new("kept").by(Some("daemon")).with("why", json!(why)));
                    let _ = events.send(Event::Finished { task: id, changes });
                    return;
                }
            }
            match task_scripts::clean_up(&task.repo, &folder) {
                Cleanup::Removed { branch } => {
                    eprintln!("kanna-daemon: {id} is merged, so its worktree {} and branch {branch} are removed", folder.display());
                    changes.push(Change::new("cleaned up").by(Some("daemon")).with("worktree", json!(folder.to_string_lossy())).with("branch", json!(branch)));
                }
                Cleanup::Kept(why) => {
                    eprintln!("kanna-daemon: kept {id}'s worktree {} and its branch: {why}", folder.display());
                    changes.push(Change::new("kept").by(Some("daemon")).with("why", json!(why)));
                }
            }
            let _ = events.send(Event::Finished { task: id, changes });
        });
    }

    pub fn finished(&self, task: &str, changes: Vec<Change>) {
        for change in changes {
            if let Err(error) = self.db.log(task, &change) {
                eprintln!("kanna-daemon: couldn't save {task}'s {} in {}: {error}", change.what, db::FILE);
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
                return responder.reply(Err(KmuxError::new("task_limit", format!("{} has {TASKS_PER_PROJECT} tasks on the Shelf: move one to the backlog with `kanna task defer TASK` first", entry.repo))));
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
                let branch = git(Path::new(&folder), &["branch", "--show-current"]).unwrap_or_default();
                return self.launch(Starting { entry: entry.resumable(), app, by, responder }, folder, branch);
            }
            (entry.folder, entry.session) = (None, None);
        }
        let token = self.next.get() + 1;
        self.next.set(token);
        let (repo, name, events) = (PathBuf::from(&entry.repo), entry.id.clone(), self.events.clone());
        self.pending.borrow_mut().insert(token, Starting { entry, app, by, responder });
        std::thread::spawn(move || {
            let result = make_worktree(&repo, &name);
            let _ = events.send(Event::Worktree { token, result });
        });
    }

    pub fn worktree_made(self: &Rc<Self>, token: u64, result: Result<(String, String), String>) {
        let Some(starting) = self.pending.borrow_mut().remove(&token) else { return };
        match result {
            Ok((folder, branch)) => self.launch(starting, folder, branch),
            Err(error) => starting.responder.reply(Err(KmuxError::new("failed", format!("couldn't make {}'s worktree: {error}", starting.entry.id)))),
        }
    }

    fn record(&self, pane: &str, entry: Entry, place: Place) {
        let mut running = self.running.borrow_mut();
        running.retain(|r| r.pane != pane);
        running.push(Running { pane: pane.to_string(), entry, place });
    }

    /// Move to Backlog: takes a running task off its Desk or the Shelf and puts it back on its repo's backlog,
    /// its agent and tabs still running, so starting it again brings them back as they are.
    fn defer(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
        let Some(reference) = string(args, "task") else { return responder.reply(Err(bad("say which running task"))) };
        let Some(pane) = core.pane(&self.pane_or(reference)) else { return responder.reply(Err(KmuxError::new("not_found", format!("no running task \"{reference}\"")))) };
        let recorded = self.running.borrow().iter().find(|r| r.pane == pane.id).map(|r| (r.entry.clone(), Some(r.place)));
        let (entry, from) = match recorded {
            Some(found) => found,
            None => match self.mint_for(&pane) {
                Ok(entry) => (entry, None),
                Err(error) => return responder.reply(Err(error)),
            },
        };
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
            let made = match from {
                Some(_) => Ok(()),
                None => this.store(&entry, Place::Backlog, Some(&id), Some(Change::new("made").by(Some("daemon")))),
            };
            let change = Change { from, to: Some(Place::Backlog), ..Change::new("deferred") }.by(by.as_deref());
            let saved = made.and_then(|_| this.store(&entry, Place::Backlog, Some(&id), Some(change)));
            let value = entry.to_value();
            this.entries.borrow_mut().push(entry);
            this.changed();
            if let Err(error) = saved {
                return responder.reply(Err(error));
            }
            let running = this.core.upgrade().is_some_and(|core| alive(&core, &id));
            responder.reply(Ok(Map::from_iter([("task".to_string(), value), ("pane".to_string(), json!(id)), ("running".to_string(), json!(running))])));
        });
    }

    fn launch(self: &Rc<Self>, starting: Starting, folder: String, branch: String) {
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
            None => args["backlog"] = json!(true),
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
            match app {
                Some(app) => this.started(pane, ran, branch, app, by, responder),
                None => this.running_here(pane, ran, branch, by, responder),
            }
        });
    }

    /// Brings a task held in the backlog back to the Shelf, its agent and tabs as they were, or starts it again
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
            let folder = entry.folder.clone().unwrap_or_default();
            let branch = git(Path::new(&folder), &["branch", "--show-current"]).unwrap_or_default();
            let entry = Entry { held: None, ..entry };
            if weak.upgrade().is_some_and(|core| alive(&core, &pane)) {
                return this.started(pane, entry, branch, app, by, responder);
            }
            let again = Rc::downgrade(&this);
            let command = this.again(&entry);
            let entry = entry.resumable();
            this.revive(&pane.clone(), Some(command), move |result| {
                let Some(this) = again.upgrade() else { return };
                match result {
                    Ok(pane) => this.started(pane, entry, branch, app, by, responder),
                    Err(error) => responder.reply(Err(error)),
                }
            });
        });
    }

    fn running_here(&self, pane: String, ran: Entry, branch: String, by: Option<String>, responder: Answer) {
        let out = Map::from_iter([
            ("task".to_string(), json!(ran.id)),
            ("pane".to_string(), json!(pane)),
            ("place".to_string(), json!("backlog")),
            ("folder".to_string(), json!(ran.folder)),
            ("branch".to_string(), json!(branch)),
        ]);
        let entry = Entry { held: Some(pane.clone()), stopped: None, ..ran };
        self.turns.borrow_mut().insert(pane.clone(), Turn::default());
        let saved = self.store(&entry, Place::Backlog, Some(&pane), Some(Change::new("started").by(by.as_deref())));
        if let Some(found) = self.entries.borrow_mut().iter_mut().find(|e| e.id == entry.id) {
            *found = entry;
        }
        self.changed();
        responder.reply(saved.map(|_| out));
    }

    fn started(self: &Rc<Self>, pane: String, ran: Entry, branch: String, app: String, by: Option<String>, responder: Answer) {
        self.take(&ran.id);
        self.turns.borrow_mut().remove(&pane);
        self.quitting.borrow_mut().remove(&pane);
        let ran = Entry { stopped: None, ..ran };
        let out = Map::from_iter([
            ("task".to_string(), json!(ran.id)),
            ("pane".to_string(), json!(pane)),
            ("folder".to_string(), json!(ran.folder)),
            ("branch".to_string(), json!(branch)),
            ("app".to_string(), json!(app)),
        ]);
        let saved = self.store(&ran, Place::Shelf, Some(&pane), Some(Change::new("shelved").moved(Place::Backlog, Place::Shelf).by(by.as_deref()).with("app", json!(app))));
        self.record(&pane, ran, Place::Shelf);
        self.changed();
        responder.reply(saved.map(|_| out));
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
        let command = entry.command(None);
        assert_eq!(said(command.strip_prefix("codex --yolo --no-daemon ").unwrap()), "You're a task's agent in Kanna (terminal term-9), in your task's worktree. Your task: don't stop");
        entry.message = None;
        let command = entry.command(None);
        assert_eq!(said(command.strip_prefix("codex --yolo --no-daemon ").unwrap()), "You're a task's agent in Kanna (terminal term-9), in your task's worktree.");
    }

    #[test]
    fn a_claude_task_gets_a_session_and_resumes_it_when_it_comes_back_from_the_backlog() {
        let mut entry = Entry { id: "task-1dfb12c2".into(), name: None, message: Some("go".into()), repo: "/r".into(), by: None, made_at: 0.0, agent: "/bin/claude".into(), folder: None, session: None, held: None, ..Default::default() };
        let id = uuid().unwrap();
        assert!(id.len() == 36 && id.chars().nth(14) == Some('4'), "{id}");
        assert_eq!(entry.command(Some(&id)), format!("/bin/claude --dangerously-skip-permissions --session-id {id} {}", told_task(Some("go"))));
        entry.folder = Some("/r/.kanna-worktrees/task-0000c0de".into());
        assert_eq!(entry.command(None), "/bin/claude --dangerously-skip-permissions --continue", "a task Kanna didn't start resumes the folder's latest session");
        entry.session = Some(id.clone());
        assert_eq!(entry.command(None), format!("/bin/claude --dangerously-skip-permissions --resume '{id}'"));
    }

    fn started(agent: &str) -> Entry {
        Entry { id: "task-1dfb12c2".into(), name: None, message: Some("go".into()), repo: "/r".into(), by: None, made_at: 0.0, agent: agent.into(), folder: None, session: None, held: None, ..Default::default() }
    }

    #[test]
    fn a_codex_task_resumes_the_latest_session_in_its_worktree() {
        let mut entry = started("codex");
        assert_eq!(entry.command(Some("u")), format!("codex --yolo --no-daemon {}", told_task(Some("go"))), "codex can't be given a session ID");
        entry.folder = Some("/r/.kanna-worktrees/task-0000c0de".into());
        assert_eq!(entry.command(None), "codex --yolo --no-daemon resume --last");
        entry.session = Some("019f".into());
        assert_eq!(entry.command(None), "codex --yolo --no-daemon resume '019f'");
    }

    #[test]
    fn a_copilot_task_gets_a_session_and_resumes_it() {
        let mut entry = started("copilot");
        let id = uuid().unwrap();
        assert_eq!(entry.command(Some(&id)), format!("copilot --yolo --session-id {id} -i {}", told_task(Some("go"))));
        entry.folder = Some("/r/.kanna-worktrees/task-0000c0de".into());
        assert_eq!(entry.command(None), format!("copilot --yolo -i {}", told_task(Some("go"))), "copilot's --continue isn't the folder's: without a session it starts again");
        entry.session = Some(id.clone());
        assert_eq!(entry.command(None), format!("copilot --yolo --session-id '{id}'"));
    }

    #[test]
    fn an_opencode_task_resumes_the_session_found_for_its_worktree() {
        let mut entry = started("opencode");
        assert_eq!(entry.command(Some("u")), format!("opencode --prompt {}", told_task(Some("go"))));
        entry.folder = Some("/r/.kanna-worktrees/task-0000c0de".into());
        assert_eq!(entry.command(None), format!("opencode --prompt {}", told_task(Some("go"))));
        entry.session = Some("ses_1".into());
        assert_eq!(entry.command(None), "opencode --session 'ses_1'");
    }

    #[test]
    fn an_antigravity_task_continues_its_worktree_s_conversation() {
        let mut entry = started("/usr/local/bin/agy");
        assert_eq!(entry.command(Some("u")), format!("/usr/local/bin/agy --dangerously-skip-permissions -i {}", told_task(Some("go"))));
        entry.folder = Some("/r/.kanna-worktrees/task-0000c0de".into());
        assert_eq!(entry.command(None), "/usr/local/bin/agy --dangerously-skip-permissions --continue");
        entry.session = Some("c1".into());
        assert_eq!(entry.command(None), "/usr/local/bin/agy --dangerously-skip-permissions --conversation 'c1'");
    }

    #[test]
    fn an_agent_kanna_doesn_t_know_starts_again_with_its_first_message() {
        let mut entry = started("aider");
        entry.folder = Some("/r/.kanna-worktrees/task-0000c0de".into());
        assert_eq!(entry.command(Some("u")), format!("aider {}", told_task(Some("go"))));
    }

    #[test]
    fn copilot_and_opencode_sessions_are_found_by_worktree() {
        let dir = std::env::temp_dir().join(format!("kanna-sessions-{}", std::process::id()));
        let (here, there) = (dir.join("here"), dir.join("there"));
        for (id, cwd, at) in [("old", &here, "2026-01-01T00:00:00Z"), ("new", &here, "2026-02-01T00:00:00Z"), ("other", &there, "2026-03-01T00:00:00Z")] {
            fs::create_dir_all(dir.join("state").join(id)).unwrap();
            fs::write(dir.join("state").join(id).join("workspace.yaml"), format!("id: {id}\ncwd: {}\nupdated_at: {at}\n", cwd.display())).unwrap();
        }
        fs::create_dir_all(&here).unwrap();
        assert_eq!(latest_copilot_session(&dir.join("state"), &here), Some("new".into()));
        assert_eq!(latest_copilot_session(&dir.join("state"), &dir.join("nowhere")), None);
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
        let names: Vec<&str> = agents.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(names, ["claude", "codex", "copilot", "opencode", "agy"]);
        let start = |name: &str| agents.iter().find(|(n, _, _)| n == name).unwrap().2.clone();
        assert_eq!(start("claude"), "claude --dangerously-skip-permissions 'You are the Kanna planning agent.'", "no session; no permission prompts, as a task's agent");
        assert_eq!(start("codex"), "codex --yolo --no-daemon 'You are the Kanna planning agent.'");
        assert_eq!(start("copilot"), "copilot --yolo -i 'You are the Kanna planning agent.'");
        assert_eq!(start("opencode"), "opencode --prompt 'You are the Kanna planning agent.'");
        assert_eq!(start("agy"), "agy --dangerously-skip-permissions -i 'You are the Kanna planning agent.'");
        assert_eq!(started_as("/opt/bin/codex", PLANNER_ROLE, None), "/opt/bin/codex --yolo --no-daemon 'You are the Kanna planning agent.'");
        assert_eq!(started_as("/x/mock-agent", PLANNER_ROLE, Some(300_000)), "/x/mock-agent 'You are the Kanna planning agent.'");
        assert_eq!(started_as("codex", PLANNER_ROLE, Some(300_000)), "codex --yolo --no-daemon -c model_auto_compact_token_limit=300000 'You are the Kanna planning agent.'", "codex compacts at planning.context too");
        assert_eq!(started_as("claude", PLANNER_ROLE, Some(300_000)), "claude --dangerously-skip-permissions 'You are the Kanna planning agent.'", "claude's is the terminal's CLAUDE_CODE_AUTO_COMPACT_WINDOW");
    }

    #[test]
    fn the_planning_terminal_keeps_claude_to_its_context_and_runs_the_planner_script() {
        let agents = [("claude".to_string(), "claude".to_string(), "claude 'You are the Kanna planning agent.'".to_string())];
        let command = planner_command(Some("/K/planner.sh"), Some(300_000), Some("/S/shell/zsh"), Some("/A/kanna"), Path::new("/H/.kanna-v4/repos"), &agents);
        assert_eq!(command, r#"[ -n "$CLAUDE_CODE_AUTO_COMPACT_WINDOW" ] || export CLAUDE_CODE_AUTO_COMPACT_WINDOW=300000; exec '/K/planner.sh' --kanna '/A/kanna' --zsh '/S/shell/zsh' --repos '/H/.kanna-v4/repos' -- 'claude' 'claude' 'claude '\''You are the Kanna planning agent.'\'''"#);
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
        let root = root.canonicalize().unwrap();
        Planner { home: root.join("home"), bin: root.join("bin"), root }
    }

    impl Planner {
        fn repo(&self, name: &str) -> PathBuf {
            let repo = self.root.join(name);
            fs::create_dir_all(&repo).unwrap();
            assert!(Command::new("git").args(["init", "-q"]).current_dir(&repo).status().unwrap().success());
            repo
        }

        fn run(&self, folder: &Path, input: &str) -> String {
            self.run_with(folder, input, None)
        }

        fn run_with(&self, folder: &Path, input: &str, kanna: Option<&Path>) -> String {
            let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/kanna/Resources/planner.sh");
            let mut args = vec!["--repos".to_string(), self.home.join(".kanna-v4/repos").to_string_lossy().into_owned()];
            if let Some(kanna) = kanna {
                args.extend(["--kanna".to_string(), kanna.to_string_lossy().into_owned()]);
            }
            args.push("--".to_string());
            for agent in ["claude", "codex", "copilot"] {
                args.push(agent.to_string());
                args.push(agent.to_string());
                args.push(started_as(agent, PLANNER_ROLE, None));
            }
            let mut child = Command::new(&script)
                .args(&args)
                .current_dir(folder)
                .env_clear()
                .env("HOME", &self.home)
                .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
                .env("SHELL", self.bin.join("shell"))
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
    fn the_planner_script_asks_which_agent_offers_to_make_it_the_project_s_and_starts_it() {
        let p = planner("ask", &["claude", "codex"]);
        let repo = p.repo("app");
        let out = p.run(&repo, "2\ny\n");
        assert!(out.contains("Choose an agent for planning and task creation\n\n  1  claude\n  2  codex\n\nChoose [1]: Make codex the planning agent for app? [y/N] codex plans app from now on (planning.agent in .kanna/config.json).\n"), "{out}");
        assert!(out.contains("codex in ROOT/app: --yolo --no-daemon You are the Kanna planning agent.\n"), "{out}");
        assert!(out.contains("The planning agent quit. Start it again: codex --yolo --no-daemon 'You are the Kanna planning agent.'\nshell in ROOT/app\n"), "{out}");
        let config: Value = serde_json::from_str(&fs::read_to_string(repo.join(".kanna/config.json")).unwrap()).unwrap();
        assert_eq!(config, json!({ "version": 1, "planning": { "agent": "codex" } }));
        assert_eq!(planning_context(&[repo.join(".kanna/config.json")]), Some(PLANNING_CONTEXT), "the daemon still reads the file");

        let out = p.run(&repo, "");
        assert!(!out.contains("Choose"), "the project's planner starts without asking: {out}");
        assert!(out.contains("codex in ROOT/app: --yolo --no-daemon You are the Kanna planning agent."), "{out}");

        let other = p.repo("other");
        let out = p.run(&other, "\nn\n");
        assert!(out.contains("claude in ROOT/other: --dangerously-skip-permissions You are the Kanna planning agent."), "Return takes the first: {out}");
        assert!(!other.join(".kanna").exists(), "no saves nothing");
    }

    #[test]
    fn the_planner_script_keeps_the_rest_of_the_project_s_settings() {
        let p = planner("keep", &["claude", "codex"]);
        let repo = p.repo("app");
        fs::create_dir_all(repo.join(".kanna")).unwrap();
        fs::write(repo.join(".kanna/config.json"), r#"{"$schema": "x", "version": 1, "planning": {"context": 400000}}"#).unwrap();
        p.run(&repo, "1\ny\n");
        let config: Value = serde_json::from_str(&fs::read_to_string(repo.join(".kanna/config.json")).unwrap()).unwrap();
        assert_eq!(config, json!({ "$schema": "x", "version": 1, "planning": { "context": 400000, "agent": "claude" } }));

        let cloned = p.repo("cloned");
        fs::create_dir_all(cloned.join(".kanna")).unwrap();
        std::os::unix::fs::symlink(p.home.join("elsewhere"), cloned.join(".kanna/config.json")).unwrap();
        let out = p.run(&cloned, "1\ny\n");
        assert!(out.contains("Couldn't write ROOT/cloned/.kanna/config.json."), "a repo's symlink isn't followed: {out}");
        assert!(!p.home.join("elsewhere").exists());
        fs::write(cloned.join(".kanna/config.json.real"), "{\"version\": 1, \"planning\": {\"agent\": \"evil\\u001b]0;x\\u0007\"}}").unwrap();
        fs::remove_file(cloned.join(".kanna/config.json")).unwrap();
        fs::rename(cloned.join(".kanna/config.json.real"), cloned.join(".kanna/config.json")).unwrap();
        let out = p.run(&cloned, "1\nn\n");
        assert!(out.contains("planning.agent is evil]0;x, which isn't installed.") && !out.contains('\u{7}'), "a repo's text can't drive the terminal: {out:?}");
    }

    #[test]
    fn the_planner_script_doesn_t_ask_with_one_agent_and_says_so_with_none() {
        let p = planner("one", &["claude"]);
        let repo = p.repo("app");
        let out = p.run(&repo, "");
        assert!(!out.contains("Choose") && out.contains("claude in ROOT/app: --dangerously-skip-permissions You are the Kanna planning agent."), "{out}");
        let p = planner("none", &[]);
        let repo = p.repo("app");
        let out = p.run(&repo, "");
        assert_eq!(out, "No agent Kanna knows is installed (claude, codex, copilot, opencode or agy). Install one, then start it here.\nshell in ROOT/app\n");
    }

    #[test]
    fn in_home_the_planner_script_asks_where_first() {
        let p = planner("where", &["claude", "codex"]);
        let out = p.run(&p.home, "recipe-box\n2\n");
        assert!(out.contains("Folder, repo or name: Made ~/.kanna-v4/repos/recipe-box\n"), "{out}");
        assert!(out.contains("\u{1b}]7;file://localhostROOT/home/.kanna-v4/repos/recipe-box\u{1b}\\"), "the window follows: {out}");
        assert!(!out.contains("Make codex"), "a new project isn't a repo, so nothing is offered: {out}");
        assert!(out.contains("codex in ROOT/home/.kanna-v4/repos/recipe-box: --yolo --no-daemon You are the Kanna planning agent.") && out.ends_with("shell in ROOT/home/.kanna-v4/repos/recipe-box\n"), "{out}");
        let out = p.run(&p.home, "recipe-box\n1\n");
        assert!(out.contains("Opened ~/.kanna-v4/repos/recipe-box\n"), "{out}");

        let repo = p.repo("work/app");
        let out = p.run(&p.home, "~/nowhere\nbad name!\n../work/app\n1\nn\n");
        assert!(out.contains("No folder ~/nowhere.") && out.contains("A project's name is letters, digits"), "{out}");
        assert!(out.contains(&format!("claude in ROOT/work/app: --dangerously-skip-permissions You are")), "a folder is moved to: {out}");
        let out = p.run(&p.home, &format!("{}\n1\nn\n", repo.display()));
        assert!(out.contains("claude in ROOT/work/app: --dangerously-skip-permissions You are"), "{out}");
        let out = p.run(&repo, "1\nn\n");
        assert!(!out.contains("Folder, repo or name"), "only in ~: {out}");
    }

    #[test]
    fn in_home_the_planner_script_numbers_the_recent_projects_and_return_takes_the_latest() {
        let p = planner("recent", &["claude"]);
        let (app, tool) = (p.repo("work/app"), p.repo("work/tool"));
        let gone = p.root.join("work/gone");
        let kanna = p.bin.join("kanna");
        let list = p.root.join("projects");
        stub(&kanna, &format!("[ \"$1 $2\" = 'projects --json' ] && cat '{}'; exit 0", list.display()));
        let listed = |projects: &[(&Path, &str)]| fs::write(&list, json!({ "projects": projects.iter().map(|(path, label)| json!({ "path": path, "label": label, "at": 1 })).collect::<Vec<_>>() }).to_string()).unwrap();
        listed(&[(&app, "app"), (&tool, "~/work/tool"), (&gone, "gone")]);
        let out = p.run_with(&p.home, "\n", Some(&kanna));
        assert!(out.contains("Recent:\n  1  app\n  2  ~/work/tool\n  3  gone\n\nFolder, repo, name or number [1]: "), "the daemon's labels: {out}");
        assert!(out.contains("claude in ROOT/work/app: --dangerously-skip-permissions You are"), "Return takes 1: {out}");
        assert!(out.contains("\u{1b}]7;file://localhostROOT/work/app\u{1b}\\"), "the window follows: {out}");
        let out = p.run_with(&p.home, "4\n0\n3\n2\n", Some(&kanna));
        assert!(out.contains("Choose 1 to 3, or a folder, repo or name.") && out.contains("No folder ROOT/work/gone."), "{out}");
        assert!(out.contains("claude in ROOT/work/tool: --dangerously-skip-permissions You are"), "{out}");
        let out = p.run_with(&p.home, "recipe-box\n", Some(&kanna));
        assert!(out.contains("Made ~/.kanna-v4/repos/recipe-box") && out.contains("claude in ROOT/home/.kanna-v4/repos/recipe-box"), "a name still works: {out}");
        let spaced = p.root.join("work/my app");
        fs::create_dir_all(&spaced).unwrap();
        listed(&[(&spaced, "my app")]);
        let out = p.run_with(&p.home, "1\n", Some(&kanna));
        assert!(out.contains("claude in ROOT/work/my app: "), "a path with a space: {out}");
        listed(&[]);
        let out = p.run_with(&p.home, "\nrecipe-box\n", Some(&kanna));
        assert!(!out.contains("Recent:") && out.contains("Folder, repo or name: Folder, repo or name: Opened ~/.kanna-v4/repos/recipe-box"), "with none it asks as before: {out}");
        let out = p.run_with(&app, "", Some(&kanna));
        assert!(!out.contains("Recent:"), "only in ~: {out}");
    }

    #[test]
    fn in_home_the_planner_script_clones_a_url_or_owner_repo() {
        let p = planner("clone", &["claude", "codex"]);
        let source = p.repo("source");
        assert!(Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e", "commit", "-q", "--allow-empty", "-m", "first"]).current_dir(&source).status().unwrap().success());
        let bare = p.root.join("remote/kojo.git");
        assert!(Command::new("git").args(["clone", "-q", "--bare"]).arg(&source).arg(&bare).status().unwrap().success());
        let url = format!("file://{}", bare.display());
        let out = p.run(&p.home, &format!("{url}\n2\nn\n"));
        assert!(out.contains("Cloning file://ROOT/remote/kojo.git into ~/.kanna-v4/repos/kojo…"), "{out}");
        assert!(out.contains("Make codex the planning agent for kojo? [y/N]"), "a clone is a repo: {out}");
        assert!(out.contains("codex in ROOT/home/.kanna-v4/repos/kojo: --yolo --no-daemon"), "{out}");
        let out = p.run(&p.home, &format!("{url}\n2\nn\n"));
        assert!(out.contains("Opened ~/.kanna-v4/repos/kojo (already cloned)"), "{out}");

        let out = p.run(&p.home, "someone/tool\nrecipe\n1\n");
        assert!(out.contains("gh isn't installed: give the repo's URL.") && out.contains("Made ~/.kanna-v4/repos/recipe"), "{out}");
        stub(&p.bin.join("gh"), &format!("[ \"$1 $2 $3\" = \"repo clone someone/tool\" ] && git clone -q {url} \"$4\""));
        let out = p.run(&p.home, "someone/tool\n1\nn\n");
        assert!(out.contains("Cloning someone/tool into ~/.kanna-v4/repos/tool…") && out.contains("claude in ROOT/home/.kanna-v4/repos/tool: --dangerously-skip-permissions You are"), "{out}");
        let out = p.run(&p.home, "other/kojo\n1\nn\n");
        assert!(out.contains("~/.kanna-v4/repos/kojo is already there, and isn't a clone of other/kojo."), "{out}");
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
        let runners = &schema["properties"]["backlog"]["properties"]["runners"];
        assert_eq!((runners["default"].clone(), runners["minimum"].clone(), runners["maximum"].clone()), (json!(RUNNERS), json!(0), json!(RUNNERS_MAX)));
        let keys: Vec<&String> = schema["properties"]["backlog"]["properties"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["runners"]);
        let tasks = &schema["properties"]["tasks"]["properties"];
        assert_eq!((tasks["timeout"]["default"].clone(), tasks["timeout"]["minimum"].clone(), tasks["timeout"]["maximum"].clone()), (json!(task_scripts::TIMEOUT), json!(1), json!(task_scripts::TIMEOUT_MAX)));
        let keys: Vec<&String> = tasks.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["agent", "setup", "teardown", "timeout"]);
        let names: Vec<&str> = AGENTS.iter().map(|a| a.names[0]).collect();
        assert_eq!((tasks["agent"]["default"].clone(), tasks["agent"]["examples"].clone()), (json!(DEFAULT_AGENT), json!(names)));
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
    fn backlog_runners_come_from_the_repos_config_then_yours() {
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
    fn agents_start_without_asking_for_permission_unless_told_how_to_ask() {
        assert_eq!(unprompted("codex"), "codex --yolo");
        assert_eq!(unprompted("/opt/bin/claude --model opus"), "/opt/bin/claude --model opus --dangerously-skip-permissions");
        assert_eq!(unprompted("claude --dangerously-skip-permissions"), "claude --dangerously-skip-permissions");
        assert_eq!(unprompted("claude --permission-mode plan"), "claude --permission-mode plan");
        assert_eq!(unprompted("codex --ask-for-approval=untrusted"), "codex --ask-for-approval=untrusted");
        assert_eq!(unprompted("codex -a untrusted"), "codex -a untrusted");
        assert_eq!(unprompted("copilot"), "copilot --yolo");
        assert_eq!(unprompted("copilot --allow-all-tools"), "copilot --allow-all-tools");
        assert_eq!(unprompted("agy"), "agy --dangerously-skip-permissions");
        assert_eq!(unprompted("agy --mode plan"), "agy --mode plan");
        assert_eq!(unprompted("opencode"), "opencode");
    }

    #[test]
    fn codex_runs_in_its_terminal_not_the_shared_server() {
        assert_eq!(in_its_terminal("/opt/bin/codex --yolo"), "/opt/bin/codex --yolo --no-daemon");
        assert_eq!(in_its_terminal("codex --no-daemon --yolo"), "codex --no-daemon --yolo");
        assert_eq!(in_its_terminal("claude"), "claude");
        let remote = ("'/K/kanna-daemon'".to_string(), "'/s/kanna dev'".to_string());
        assert_eq!(wrapped("/opt/bin/codex --yolo", Some(&remote)), "'/K/kanna-daemon' --codex-remote '/s/kanna dev' /opt/bin/codex --yolo");
        assert_eq!(wrapped("claude", Some(&remote)), "claude");
    }

    #[test]
    fn entries_round_trip_through_the_db() {
        let entry = Entry { id: "task-0000c0de".into(), name: Some("Fix it".into()), message: None, repo: "/r".into(), by: Some("p2".into()), made_at: 5.0, agent: "codex".into(), folder: Some("/r/.kanna-worktrees/task-0000c0de".into()), session: Some("s".into()), held: Some("p3".into()), after: vec!["task-00000002".into()], stopped: Some("needs-you".into()), workspace: None };
        assert_eq!(Entry::from_row(&entry.row(Place::Backlog, Some("p3"))), entry);
        assert_eq!(Entry::from_row(&entry.row(Place::Shelf, Some("p3"))).held, None, "a task on the Shelf isn't held");
        assert_eq!(Entry::from_row(&entry.row(Place::Shelf, Some("p3"))).stopped, None, "nor stopped by the backlog");
    }

    #[test]
    fn a_worktree_gets_its_own_branch_and_is_kept_out_of_git_status() {
        let dir = std::env::temp_dir().join(format!("kanna-backlog-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| assert!(Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap().status.success(), "git {args:?}");
        git(&["init", "-q", "-b", "main"]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "first"]);
        let (folder, branch) = make_worktree(&dir, "task-1dfb12c2").unwrap();
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
        assert!(make_worktree(&dir, "task-1dfb12c2").is_err(), "a task's worktree is made once");
        let (other, _) = make_worktree(&dir, "task-00c0ffee").unwrap();
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
    fn a_task_s_id_is_task_and_8_hex_digits_never_one_its_repo_has_a_worktree_or_branch_for() {
        let state = std::env::temp_dir().join(format!("kanna-hex-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        assert!(Command::new("git").arg("-C").arg(&state).args(["init", "-q"]).status().unwrap().success());
        let core = kanna_mux::Core::default();
        let backlog = Backlog::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let ids: HashSet<String> = (0..50).map(|_| backlog.mint(&json!({ "repo": state.to_str().unwrap() }), None, None).unwrap().id).collect();
        assert_eq!(ids.len(), 50);
        assert!(ids.iter().all(|id| db::is_task_id(id)), "{ids:?}");
        fs::create_dir_all(state.join(WORKTREES).join("task-1dfb12c2")).unwrap();
        assert!(taken(&state, "task-1dfb12c2") && !taken(&state, "task-00c0ffee"));
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn a_task_is_found_by_its_id_its_pane_or_a_unique_start_of_its_id() {
        let state = std::env::temp_dir().join(format!("kanna-prefix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        let core = kanna_mux::Core::default();
        let backlog = Backlog::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let task = |id: &str, name: &str| Entry { id: id.into(), name: Some(name.into()), repo: "/r".into(), agent: "claude".into(), ..Default::default() };
        for (id, name) in [("task-1dfb12c2", "One"), ("task-1d0e44aa", "Two")] {
            backlog.store(&task(id, name), Place::Backlog, None, None).unwrap();
            backlog.entries.borrow_mut().push(task(id, name));
        }
        backlog.running.borrow_mut().push(Running { pane: "p7".into(), entry: task("task-c0ffee00", "Out"), place: Place::Shelf });
        let found = |reference: &str| backlog.resolve(reference).map_err(|e| (e.code, e.message));
        assert_eq!(found("task-1dfb12c2"), Ok(Some("task-1dfb12c2".into())));
        assert_eq!(found("1dfb"), Ok(Some("task-1dfb12c2".into())));
        assert_eq!(found("task-1DF"), Ok(Some("task-1dfb12c2".into())));
        assert_eq!(found("c0f"), Ok(Some("task-c0ffee00".into())), "a task on the Shelf");
        assert_eq!(backlog.pane_or("task-c0ffee00"), "p7");
        let (code, message) = found("task-1d").unwrap_err();
        assert_eq!(code, "ambiguous");
        assert!(message.contains("task-1dfb \"One\"") && message.contains("task-1d0e \"Two\""), "{message}");
        assert_eq!(backlog.short("task-c0ffee00"), "task-c0ff");
        assert_eq!(found("task-"), Ok(None));
        assert_eq!(found("beef"), Ok(None));
        assert_eq!(found("t134"), Ok(None), "an old ID names nothing");
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn a_task_names_its_workspace_and_follows_it_by_its_agent_s_pane() {
        let state = std::env::temp_dir().join(format!("kanna-workspace-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        let core = kanna_mux::Core::default();
        let backlog = Backlog::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let task = |id: &str, name: &str| Entry { id: id.into(), name: Some(name.into()), repo: "/r".into(), agent: "claude".into(), ..Default::default() };
        backlog.store(&task("task-1dfb12c2", "Waiting"), Place::Backlog, None, None).unwrap();
        backlog.entries.borrow_mut().push(task("task-1dfb12c2", "Waiting"));
        let out = task("task-c0ffee00", "Out");
        backlog.store(&out, Place::Shelf, Some("p7"), None).unwrap();
        backlog.running.borrow_mut().push(Running { pane: "p7".into(), entry: out, place: Place::Shelf });
        let of = |reference: &str| backlog.workspace_of(reference).map(|r| r.map_err(|e| (e.code, e.message)));
        assert_eq!(of("ws-3"), None, "a workspace's own ID goes to the layout as it is");
        assert_eq!(of("c0ff"), Some(Err(("not_started".into(), "task-c0ff hasn't started, so it has no workspace yet".into()))));
        assert_eq!(backlog.task_in("ws-3"), None);
        backlog.sync_workspaces(&[json!({ "id": "ws-3", "dock": { "id": "p7", "pane": "Out" } }), json!({ "id": "ws-4", "dock": { "id": "p9" } })]);
        assert_eq!(of("task-c0ffee00"), Some(Ok("ws-3".into())));
        assert_eq!(backlog.task_in("ws-3"), Some(("task-c0ffee00".into(), Some("Out".into()))));
        assert_eq!(backlog.task_in("ws-4"), None, "a workspace with no task's agent docked has no task");
        assert_eq!(of("task-1dfb"), Some(Err(("not_started".into(), "task-1dfb hasn't started, so it has no workspace yet".into()))));
        let again = Backlog::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
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
        let backlog = Rc::new(Backlog::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0));
        let entry = Entry { id: "task-1dfb12c2".into(), repo: state.to_str().unwrap().into(), agent: "claude".into(), held: Some("term-7".into()), ..Default::default() };
        backlog.entries.borrow_mut().push(entry);
        backlog.turns.borrow_mut().insert("term-7".into(), Turn::underway());
        backlog.status("term-7", Some("idle"), true);
        for _ in 0..2 {
            let due = backlog.tick();
            assert!(due.is_some_and(|at| at > Instant::now()), "nothing is overdue, so the daemon sleeps");
            assert_eq!(backlog.turns.borrow()["term-7"].ended(Instant::now() + TURN_SETTLE, TURN_SETTLE), None, "the ended turn came back");
            assert!(backlog.quitting.borrow().is_empty(), "runners 0 stops none");
        }
        backlog.status("term-7", Some("working"), false);
        backlog.status("term-7", Some("idle"), true);
        assert!(backlog.turns.borrow()["term-7"].ended(Instant::now(), TURN_SETTLE).is_some(), "its next turn is tracked again");
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
        let backlog = Backlog::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let ran = Entry { id: "task-00000009".into(), name: Some("Ran".into()), message: None, repo: "/r".into(), by: None, made_at: 0.0, agent: "claude".into(), folder: Some("/r/w".into()), session: Some("s".into()), held: None, ..Default::default() };
        backlog.store(&ran, Place::Shelf, Some("p4"), Some(Change::new("shelved"))).unwrap();
        let again = Backlog::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        assert_eq!(*again.running.borrow(), vec![Running { pane: "p4".into(), entry: ran, place: Place::Shelf }], "a running task's way back survives a restart");
        let ids: HashSet<String> = (0..20).map(|_| again.mint(&args, None, None).unwrap().id).collect();
        assert!(!ids.contains("task-00000009"), "and its ID isn't handed out again");
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn a_blocked_task_waits_for_tasks_that_arent_done_and_never_for_itself() {
        let state = std::env::temp_dir().join(format!("kanna-blocked-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        assert!(Command::new("git").arg("-C").arg(&state).args(["init", "-q"]).status().unwrap().success());
        let core = kanna_mux::Core::default();
        let backlog = Backlog::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        let repo = state.to_str().unwrap();
        let made = |args: Value| backlog.add(&args).map(|m| string(&m["task"], "id").unwrap().to_string());
        let first = made(json!({ "repo": repo, "name": "First" })).unwrap();
        let second = made(json!({ "repo": repo, "name": "Second", "after": [first] })).unwrap();
        let third = made(json!({ "repo": repo, "name": "Third", "after": [second] })).unwrap();
        let entry = |id: &str| backlog.entries.borrow().iter().find(|e| e.id == id).cloned().unwrap();
        assert_eq!(backlog.ready(&entry(&second)).unwrap_err().code, "blocked");
        assert_eq!(backlog.edit(&json!({ "task": first, "after": [third] })).unwrap_err().code, "cycle");
        assert!(made(json!({ "repo": repo, "name": "Fourth", "after": ["task-00000099"] })).is_err(), "an unknown blocker");
        let gone = backlog.take(&first).unwrap();
        backlog.store(&gone, Place::Done, None, None).unwrap();
        assert!(backlog.ready(&entry(&second)).is_ok(), "its blocker is done");
        assert!(made(json!({ "repo": repo, "name": "Fifth", "after": [first] })).is_err(), "a done task holds nothing back");
        backlog.edit(&json!({ "task": third, "after": [] })).unwrap();
        assert!(entry(&third).after.is_empty());
        assert!(backlog.ready(&entry(&third)).is_ok(), "a task with its blockers taken off can start");
        let again = Backlog::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0);
        assert_eq!(again.entries.borrow().iter().find(|e| e.id == second).unwrap().after, vec![first.clone()], "blockers survive a restart");
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn a_folder_that_isnt_a_repo_is_left_alone_and_a_new_repos_first_task_starts_on_an_empty_branch() {
        let dir = std::env::temp_dir().join(format!("kanna-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        assert!(make_worktree(&dir, "task-1dfb12c2").unwrap_err().contains("isn't a git repo"));
        assert!(!dir.join(".git").exists(), "Kanna made a repo");
        assert!(Command::new("git").arg("-C").arg(&dir).args(["init", "-q"]).status().unwrap().success());
        let (folder, branch) = make_worktree(&dir, "task-1dfb12c2").unwrap();
        let run = |at: &str, args: &[&str]| Command::new("git").arg("-C").arg(at).args(args).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&run(&folder, &["branch", "--show-current"]).stdout).trim(), branch);
        assert!(!run(&folder, &["rev-parse", "--verify", "HEAD"]).status.success(), "a commit was made for the user");
        assert_eq!(String::from_utf8_lossy(&run(dir.to_str().unwrap(), &["status", "--porcelain"]).stdout), "");
        let (other, _) = make_worktree(&dir, "task-00c0ffee").unwrap();
        assert_ne!(other, folder);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[derive(Default)]
    struct Agents {
        core: RefCell<Option<WeakCore>>,
        backlog: RefCell<Weak<Backlog>>,
        deaf: Cell<bool>,
        typed: RefCell<Vec<String>>,
        hung_up: RefCell<Vec<String>>,
        screens: RefCell<HashMap<String, String>>,
    }

    const SAID: &str = "019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b";

    impl Agents {
        fn exit(&self, pane: &Pane) {
            let (Some(core), backlog, id) = (self.core.borrow().as_ref().and_then(WeakCore::upgrade), self.backlog.borrow().clone(), pane.id.clone()) else { return };
            core.clone().later(move || {
                core.update(&id, PaneState::Exited, Some(0), None);
                if let Some(backlog) = backlog.upgrade() {
                    backlog.exited(&id, 0);
                }
            });
        }
    }

    impl kanna_mux::PaneHost for Agents {
        fn start(&self, pane: &Pane) {
            let Some(core) = self.core.borrow().as_ref().and_then(WeakCore::upgrade) else { return };
            let (id, reporter) = (pane.id.clone(), core.clone());
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
        backlog: Rc<Backlog>,
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
            let backlog = Rc::new(Backlog::open(Rc::new(Db::open(&state)), &state, core.weak(), std::sync::mpsc::channel().0));
            *agents.backlog.borrow_mut() = Rc::downgrade(&backlog);
            Stopping { state, core, agents, backlog }
        }

        fn agent(&self, agent: &str, place: Place) -> (String, Entry) {
            self.agent_with(agent, place, Some("given"))
        }

        fn agent_with(&self, agent: &str, place: Place, session: Option<&str>) -> (String, Entry) {
            let opened = self.core.handle_blocking(json!({ "cmd": "open", "args": { "type": "term", "cmd": agent, "dock": "left" } }));
            let pane = opened["pane"]["id"].as_str().unwrap_or_else(|| panic!("{opened}")).to_string();
            self.settle(|| alive(&self.core, &pane));
            let entry = Entry { id: "task-1dfb12c2".into(), repo: "/r".into(), agent: agent.into(), session: session.map(String::from), folder: Some("/r".into()), ..Default::default() };
            match place {
                Place::Backlog => self.backlog.entries.borrow_mut().push(Entry { held: Some(pane.clone()), ..entry.clone() }),
                place => self.backlog.running.borrow_mut().push(Running { pane: pane.clone(), entry: entry.clone(), place }),
            }
            (pane, entry)
        }

        fn settle(&self, done: impl Fn() -> bool) {
            let by = Instant::now() + Duration::from_secs(5);
            while !done() {
                assert!(Instant::now() < by, "it didn't happen");
                self.core.poll();
                self.backlog.tick();
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn answer(&self, ask: impl FnOnce(&Rc<Backlog>, Responder)) -> Result<Map, KmuxError> {
            let (responder, reply) = kanna_mux::responder();
            ask(&self.backlog, responder);
            let mut reply = Box::pin(reply);
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            let by = Instant::now() + Duration::from_secs(5);
            loop {
                if let std::task::Poll::Ready(answer) = reply.as_mut().poll(&mut cx) {
                    return answer;
                }
                assert!(Instant::now() < by, "no answer");
                self.core.poll();
                self.backlog.tick();
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn session(&self, pane: &str) -> Option<String> {
            self.backlog.pane_entry(pane).and_then(|e| e.session)
        }
    }

    use std::future::Future;

    #[test]
    fn a_turn_s_end_quits_the_agent_with_its_keys_and_keeps_the_session_it_says() {
        let t = Stopping::new("turn");
        let (pane, entry) = t.agent("claude", Place::Backlog);
        t.backlog.quit(&pane, &entry, Status::NeedsYou);
        t.settle(|| t.backlog.pane_entry(&pane).and_then(|e| e.stopped).is_some());
        assert_eq!(*t.agents.typed.borrow(), [format!("{pane}: ctrl+c"), format!("{pane}: ctrl+c")], "claude asks for a second ctrl+c");
        assert!(t.agents.hung_up.borrow().is_empty(), "claude quit, so it isn't hung up");
        assert_eq!(t.backlog.pane_entry(&pane).and_then(|e| e.stopped).as_deref(), Some("needs-you"));
        assert_eq!(t.session(&pane).as_deref(), Some("given"), "the session Kanna gave claude stays its own");
    }

    #[test]
    fn kanna_task_stop_quits_the_agent_with_its_keys_and_keeps_the_session_it_says() {
        let t = Stopping::new("stop");
        let (pane, entry) = t.agent_with("codex", Place::Shelf, None);
        let stopped = t.answer(|backlog, responder| backlog.stop(&json!({ "task": entry.id }), responder)).unwrap();
        assert_eq!(stopped["place"], "shelf");
        assert_eq!(*t.agents.typed.borrow(), [format!("{pane}: ctrl+c")], "codex quit on the first, so it isn't typed the second");
        assert!(t.agents.hung_up.borrow().is_empty(), "codex quit, so it isn't hung up");
        assert!(!alive(&t.core, &pane));
        assert_eq!(t.session(&pane).as_deref(), Some(SAID), "codex takes no ID, so the one it said is kept");
        assert_eq!(Backlog::open(t.backlog.db.clone(), &t.state, t.core.weak(), std::sync::mpsc::channel().0).running.borrow()[0].entry.session.as_deref(), Some(SAID), "the session is saved");
    }

    #[test]
    fn done_quits_the_agent_with_its_keys_before_it_ends_the_task() {
        let t = Stopping::new("done");
        let (pane, entry) = t.agent("claude", Place::Backlog);
        t.answer(|backlog, responder| backlog.done(&json!({ "task": entry.id }), responder)).unwrap();
        assert_eq!(*t.agents.typed.borrow(), [format!("{pane}: ctrl+c"), format!("{pane}: ctrl+c")], "claude asks for a second ctrl+c");
        assert!(t.agents.hung_up.borrow().is_empty(), "claude quit, so it isn't hung up");
        assert!(t.backlog.entries.borrow().is_empty(), "it's done");
    }

    #[test]
    fn an_agent_without_quit_keys_is_hung_up() {
        let t = Stopping::new("nokeys");
        let (pane, entry) = t.agent("mock-agent", Place::Shelf);
        t.answer(|backlog, responder| backlog.stop(&json!({ "task": entry.id }), responder)).unwrap();
        assert!(t.agents.typed.borrow().is_empty());
        assert_eq!(*t.agents.hung_up.borrow(), [pane.as_str()]);
        assert_eq!(t.session(&pane).as_deref(), Some("given"), "with nothing said, the session it had is kept");
    }

    #[test]
    fn an_agent_that_left_raw_mode_is_quitting_so_it_isnt_pressed_again() {
        let t = Stopping::new("leaving");
        t.agents.deaf.set(true);
        *t.backlog.line_mode.borrow_mut() = Some(Box::new(|_| true));
        let (pane, entry) = t.agent("claude", Place::Backlog);
        t.backlog.quit(&pane, &entry, Status::Idle);
        t.settle(|| t.backlog.quitting.borrow().get(&pane).is_some_and(|q| q.keys.is_empty() && q.next.is_none()));
        assert_eq!(*t.agents.typed.borrow(), [format!("{pane}: ctrl+c")], "a second ctrl+c in line mode would interrupt it as it says its session");
        assert!(t.agents.hung_up.borrow().is_empty(), "it has 5s to quit");
    }

    #[test]
    fn an_agent_still_running_after_its_quit_keys_is_hung_up() {
        let t = Stopping::new("deaf");
        t.agents.deaf.set(true);
        let (pane, entry) = t.agent("claude", Place::Backlog);
        t.backlog.quit(&pane, &entry, Status::Idle);
        t.settle(|| t.agents.typed.borrow().len() == 2);
        assert!(t.agents.hung_up.borrow().is_empty(), "it has 5s to quit");
        t.backlog.quitting.borrow_mut().get_mut(&pane).unwrap().by = Instant::now();
        t.backlog.tick();
        t.settle(|| t.backlog.pane_entry(&pane).and_then(|e| e.stopped).is_some());
        assert_eq!(*t.agents.hung_up.borrow(), [pane.as_str()]);
        assert_eq!(t.session(&pane).as_deref(), Some("given"));
    }
}
