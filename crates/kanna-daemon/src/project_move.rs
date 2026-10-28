use crate::projects::{canonical, home_place, identity, tilde};
use kanna_mux::json::{json, Value};
use kanna_mux::{project, KmuxError};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
}

impl Move {
    pub fn froms(&self) -> Vec<PathBuf> {
        let mut froms = vec![self.from.clone()];
        if let Ok(rest) = self.from.strip_prefix("/private") {
            froms.push(Path::new("/").join(rest));
        }
        froms
    }

    pub fn shifted(&self, text: &str) -> Option<String> {
        self.froms().iter().find_map(|from| shifted(text, from, &self.to))
    }

    pub fn holds(&self, text: &str) -> bool {
        self.shifted(text).is_some()
    }
}

fn refused(code: &str, message: String) -> KmuxError {
    KmuxError::new(code, message)
}

pub fn expand(path: &str, home: &Path) -> PathBuf {
    match path {
        "~" => home.to_path_buf(),
        _ => path.strip_prefix("~/").map_or_else(|| PathBuf::from(path), |rest| home.join(rest)),
    }
}

pub fn plan(repos: &Path, path: &Path, homes: &[PathBuf]) -> Result<Move, KmuxError> {
    if !path.is_dir() {
        return Err(refused("not_found", format!("no folder {}", path.display())));
    }
    let from = project(path.to_str()).ok_or_else(|| refused("not_found", format!("no folder {}", path.display())))?;
    let repos = canonical(repos);
    if from.parent().is_none() || homes.iter().any(|home| canonical(home) == from) || repos.starts_with(&from) {
        return Err(refused("bad_request", format!("{} isn't a project: give a project's folder", tilde(&from))));
    }
    let to = repos.join(home_place(&from));
    if to == from {
        return Err(refused("in_place", format!("{} is in its place already", tilde(&from))));
    }
    if to.exists() || fs::symlink_metadata(&to).is_ok() {
        let theirs = identity(&canonical(&to));
        return Err(match identity(&from).filter(|mine| mine.starts_with("url:") && Some(mine) == theirs.as_ref()) {
            Some(same) => refused(
                "clash",
                format!("{} and {} are both clones of {}, and a project has one place: keep one of them (remove the other, or give it another origin), then move again", tilde(&from), tilde(&to), &same[4..]),
            ),
            None => refused("taken", format!("{} is already there, and isn't {}: move it away first", tilde(&to), tilde(&from))),
        });
    }
    let disk = to.ancestors().find_map(|dir| fs::metadata(dir).ok()).map(|m| m.dev());
    if disk.is_some_and(|dev| fs::metadata(&from).map(|m| m.dev()).ok() != Some(dev)) {
        return Err(refused("other_disk", format!("{} is on another disk than {}, and Kanna moves a project only within one", tilde(&from), tilde(&repos))));
    }
    Ok(Move { from, to })
}

pub fn shift(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(from).ok()?;
    Some(if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) })
}

pub fn shifted(text: &str, from: &Path, to: &Path) -> Option<String> {
    shift(Path::new(text), from, to).map(|p| p.to_string_lossy().into_owned())
}

pub fn worktrees(repo: &Path) -> Vec<PathBuf> {
    let Ok(out) = Command::new("git").arg("-C").arg(repo).args(["worktree", "list", "--porcelain"]).output() else { return vec![] };
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().filter_map(|l| l.strip_prefix("worktree ")).map(PathBuf::from).filter(|w| canonical(w) != canonical(repo)).collect()
}

pub fn carry(plan: &Move) -> Result<usize, String> {
    let others = worktrees(&plan.from);
    if let Some(parent) = plan.to.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("couldn't make {}: {e}", tilde(parent)))?;
    }
    fs::rename(&plan.from, &plan.to).map_err(|e| format!("couldn't move {} to {}: {e}", tilde(&plan.from), tilde(&plan.to)))?;
    if others.is_empty() {
        return Ok(0);
    }
    let moved: Vec<PathBuf> = others.iter().map(|w| shift(w, &plan.from, &plan.to).unwrap_or_else(|| w.clone())).collect();
    let out = Command::new("git").arg("-C").arg(&plan.to).args(["worktree", "repair"]).args(&moved).output().map_err(|e| format!("couldn't run git worktree repair: {e}"))?;
    if !out.status.success() {
        return Err(format!("git worktree repair failed in {}: {}", tilde(&plan.to), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(moved.len())
}

#[derive(Clone, Debug)]
pub struct Stores {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub copilot: PathBuf,
    pub opencode: PathBuf,
}

#[derive(Debug, Default, PartialEq)]
pub struct Moved {
    pub claude: usize,
    pub codex: usize,
    pub copilot: usize,
    pub opencode: usize,
    pub kept: Vec<String>,
}

impl Moved {
    pub fn to_value(&self) -> Value {
        json!({ "claude": self.claude, "codex": self.codex, "copilot": self.copilot, "opencode": self.opencode, "kept": self.kept })
    }
}

impl Stores {
    pub fn here() -> Stores {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = crate::agents::home();
        let ours = var("KANNA_AGENTS_HOME").is_some();
        let dir = |name: &str, default: &str| var(name).filter(|_| !ours).unwrap_or_else(|| home.join(default));
        Stores { claude: dir("CLAUDE_CONFIG_DIR", ".claude"), codex: dir("CODEX_HOME", ".codex"), copilot: dir("COPILOT_HOME", ".copilot"), opencode: dir("XDG_DATA_HOME", ".local/share").join("opencode") }
    }

    pub fn carry(&self, from: &Path, to: &Path) -> Moved {
        let mut moved = Moved::default();
        self.claude_folders(from, to, &mut moved);
        moved.codex = self.codex_rollouts(from, to);
        moved.copilot = self.copilot_sessions(from, to);
        moved.opencode = self.opencode_sessions(from, to).unwrap_or_else(|error| {
            moved.kept.push(format!("opencode's sessions in {}: {error}", tilde(&self.opencode)));
            0
        });
        moved
    }

    fn claude_folders(&self, from: &Path, to: &Path, moved: &mut Moved) {
        let projects = self.claude.join("projects");
        let (old, new) = (claude_name(from), claude_name(to));
        let Ok(listed) = fs::read_dir(&projects) else { return };
        for found in listed.flatten() {
            let name = found.file_name().to_string_lossy().into_owned();
            let ours = name == old || name.strip_prefix(&old).and_then(|rest| rest.strip_prefix('-')).is_some_and(|rest| rest.starts_with("-kanna-worktrees-") || descendant(to, rest));
            if !ours || !found.path().is_dir() {
                continue;
            }
            let target = projects.join(format!("{new}{}", &name[old.len()..]));
            merge(&found.path(), &target, &mut moved.kept);
            moved.claude += 1;
        }
    }

    fn codex_rollouts(&self, from: &Path, to: &Path) -> usize {
        let mut count = 0;
        for root in [self.codex.join("sessions"), self.codex.join("archived_sessions")] {
            for rollout in files_under(&root, 4).into_iter().filter(|f| f.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))) {
                let Some(first) = first_line(&rollout) else { continue };
                let Ok(meta) = serde_json::from_str::<Value>(&first) else { continue };
                let cwd = meta.get("payload").and_then(|p| p.get("cwd")).and_then(Value::as_str);
                if !cwd.is_some_and(|cwd| shift(Path::new(cwd), from, to).is_some()) {
                    continue;
                }
                if rewrite(&rollout, |text| json_paths(text, "cwd", from, to)) {
                    count += 1;
                }
            }
        }
        count
    }

    fn copilot_sessions(&self, from: &Path, to: &Path) -> usize {
        let Ok(listed) = fs::read_dir(self.copilot.join("session-state")) else { return 0 };
        let mut count = 0;
        for session in listed.flatten() {
            let file = session.path().join("workspace.yaml");
            if fs::metadata(&file).is_ok_and(|m| m.len() <= 64 << 10) && rewrite(&file, |text| yaml_paths(text, &["cwd", "git_root"], from, to)) {
                count += 1;
            }
        }
        count
    }

    fn opencode_sessions(&self, from: &Path, to: &Path) -> Result<usize, String> {
        let file = self.opencode.join("opencode.db");
        if !file.is_file() {
            return Ok(0);
        }
        let db = rusqlite::Connection::open(&file).map_err(|e| e.to_string())?;
        db.busy_timeout(std::time::Duration::from_secs(5)).map_err(|e| e.to_string())?;
        let (old, new) = (from.to_string_lossy(), to.to_string_lossy());
        let mut count = 0;
        for (table, column) in [("session", "directory"), ("project", "worktree")] {
            let has = db.prepare(&format!("select 1 from pragma_table_info('{table}') where name = '{column}'")).and_then(|mut s| s.exists([])).map_err(|e| e.to_string())?;
            if !has {
                continue;
            }
            let sql = format!("update {table} set {column} = ?2 || substr({column}, length(?1) + 1) where {column} = ?1 or substr({column}, 1, length(?1) + 1) = ?1 || '/'");
            let changed = db.execute(&sql, [old.as_ref(), new.as_ref()]).map_err(|e| e.to_string())?;
            if table == "session" {
                count = changed;
            }
        }
        Ok(count)
    }
}

pub fn claude_name(path: &Path) -> String {
    path.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn descendant(dir: &Path, rest: &str) -> bool {
    let Ok(listed) = fs::read_dir(dir) else { return false };
    listed.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).any(|child| {
        let name = claude_name(Path::new(&child.file_name()));
        name == rest || rest.strip_prefix(&name).and_then(|r| r.strip_prefix('-')).is_some_and(|r| descendant(&child.path(), r))
    })
}

fn merge(from: &Path, to: &Path, kept: &mut Vec<String>) {
    if fs::symlink_metadata(to).is_err() {
        if let Err(error) = fs::rename(from, to) {
            kept.push(format!("{}: {error}", tilde(from)));
        }
        return;
    }
    if !(from.is_dir() && to.is_dir()) {
        kept.push(format!("{} (there's a {} already)", tilde(from), tilde(to)));
        return;
    }
    for entry in fs::read_dir(from).into_iter().flatten().flatten() {
        merge(&entry.path(), &to.join(entry.file_name()), kept);
    }
    let _ = fs::remove_dir(from);
}

fn files_under(root: &Path, depth: usize) -> Vec<PathBuf> {
    let Ok(listed) = fs::read_dir(root) else { return vec![] };
    let mut found = vec![];
    for entry in listed.flatten() {
        match entry.file_type() {
            Ok(kind) if kind.is_dir() && depth > 0 => found.extend(files_under(&entry.path(), depth - 1)),
            Ok(kind) if kind.is_file() => found.push(entry.path()),
            _ => {}
        }
    }
    found
}

fn first_line(file: &Path) -> Option<String> {
    use std::io::{BufRead, Read};
    let mut line = String::new();
    std::io::BufReader::new(fs::File::open(file).ok()?.take(4 << 20)).read_line(&mut line).ok()?;
    Some(line)
}

fn rewrite(file: &Path, change: impl Fn(&str) -> Option<String>) -> bool {
    let Ok(text) = fs::read_to_string(file) else { return false };
    let Some(changed) = change(&text) else { return false };
    let modified = fs::metadata(file).and_then(|m| m.modified()).ok();
    let Ok(mut out) = fs::OpenOptions::new().write(true).truncate(true).open(file) else { return false };
    if std::io::Write::write_all(&mut out, changed.as_bytes()).is_err() {
        return false;
    }
    if let Some(at) = modified {
        let _ = out.set_modified(at);
    }
    true
}

fn json_paths(text: &str, key: &str, from: &Path, to: &Path) -> Option<String> {
    let (old, new) = (from.to_string_lossy(), to.to_string_lossy());
    let mut text = text.to_string();
    let mut changed = false;
    for opening in [format!("\"{key}\":\""), format!("\"{key}\": \"")] {
        let mut out = String::with_capacity(text.len());
        let mut rest = text.as_str();
        while let Some(at) = rest.find(&opening) {
            let start = at + opening.len();
            out.push_str(&rest[..start]);
            rest = &rest[start..];
            if rest.starts_with(old.as_ref()) && matches!(rest[old.len()..].chars().next(), Some('"' | '/')) {
                out.push_str(&new);
                rest = &rest[old.len()..];
                changed = true;
            }
        }
        out.push_str(rest);
        text = out;
    }
    changed.then_some(text)
}

fn yaml_paths(text: &str, keys: &[&str], from: &Path, to: &Path) -> Option<String> {
    let mut changed = false;
    let lines: Vec<String> = text
        .split_inclusive('\n')
        .map(|line| {
            let Some((key, value)) = line.split_once(':') else { return line.to_string() };
            if !keys.contains(&key.trim()) {
                return line.to_string();
            }
            let body = value.trim_end_matches(['\n', '\r']);
            let ending = &value[body.len()..];
            let trimmed = body.trim();
            let quote = trimmed.chars().next().filter(|c| *c == '"' || *c == '\'');
            let bare = quote.map_or(trimmed, |q| trimmed.trim_matches(q));
            match shifted(bare, from, to) {
                Some(path) => {
                    changed = true;
                    let q = quote.map(String::from).unwrap_or_default();
                    format!("{key}: {q}{path}{q}{ending}")
                }
                None => line.to_string(),
            }
        })
        .collect();
    changed.then(|| lines.concat())
}

const SKIPPED: &[&str] = &["projects", "todos", "shell-snapshots", "statsig", "debug", "logs", "file-history", "session-env", "plugins", "ide", "cache", "paste-cache", "backups", "telemetry", "sessions", "local", "jobs", "shares", "access-audit", "history.jsonl"];

const SHELLS: &[&str] = &[".zshrc", ".zprofile", ".zshenv", ".zlogin", ".bashrc", ".bash_profile", ".profile", ".config/fish/config.fish", ".codex/config.toml", ".codex/AGENTS.md", ".claude.json"];

const MENTIONS: usize = 100;

fn small_files(root: &Path, depth: usize, skip: &[&str]) -> Vec<PathBuf> {
    let Ok(listed) = fs::read_dir(root) else { return vec![] };
    let mut found = vec![];
    for entry in listed.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if skip.contains(&name.as_str()) || name.ends_with(".jsonl") {
            continue;
        }
        match entry.file_type() {
            Ok(kind) if kind.is_dir() && depth > 0 => found.extend(small_files(&entry.path(), depth - 1, skip)),
            Ok(kind) if kind.is_file() && entry.metadata().is_ok_and(|m| m.len() <= 256 << 10) => found.push(entry.path()),
            _ => {}
        }
    }
    found
}

pub fn mentions(home: &Path, repo: &Path, from: &Path) -> Vec<String> {
    let mut words = vec![from.to_string_lossy().into_owned()];
    if let Ok(inside) = from.strip_prefix(canonical(home)).or_else(|_| from.strip_prefix(home)) {
        words.push(format!("~/{}", inside.display()));
        words.push(format!("$HOME/{}", inside.display()));
    }
    let mut files = small_files(&home.join(".claude"), 3, SKIPPED);
    files.extend(small_files(&home.join(".agents"), 6, &[]));
    files.extend(SHELLS.iter().map(|f| home.join(f)).filter(|f| fs::metadata(f).is_ok_and(|m| m.is_file() && m.len() <= 16 << 20)));
    let mut found = vec![];
    for file in files {
        let Ok(text) = fs::read_to_string(&file) else { continue };
        for (n, line) in text.lines().enumerate() {
            if words.iter().any(|w| line.contains(w.as_str())) {
                found.push(format!("{}:{}", tilde(&file), n + 1));
            }
        }
    }
    let mut grep = Command::new("git");
    grep.arg("-C").arg(repo).args(["grep", "-n", "-I", "--untracked", "-F"]);
    for word in &words {
        grep.arg("-e").arg(word);
    }
    if let Ok(out) = grep.output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut parts = line.splitn(3, ':');
            if let (Some(file), Some(n)) = (parts.next(), parts.next()) {
                found.push(format!("{}:{n}", tilde(&repo.join(file))));
            }
        }
    }
    found.truncate(MENTIONS);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::projects::REPOS;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-move-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn git(dir: &Path, args: &[&str]) {
        assert!(Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e"]).args(args).current_dir(dir).status().unwrap().success(), "git {args:?}");
    }

    fn repo(at: &Path, origin: Option<&str>) {
        fs::create_dir_all(at).unwrap();
        git(at, &["init", "-q", "-b", "main"]);
        git(at, &["commit", "-q", "--allow-empty", "-m", "first"]);
        if let Some(url) = origin {
            git(at, &["remote", "add", "origin", url]);
        }
    }

    #[test]
    fn a_project_moves_to_its_place_unless_another_clone_has_it() {
        let dir = folder("plan");
        let repos = dir.join("home").join(REPOS);
        let app = dir.join("work/app");
        repo(&app, Some("git@github.com:someone/app.git"));
        let homes = [dir.join("home")];
        assert_eq!(plan(&repos, &app.join("src").parent().unwrap().join("."), &homes).unwrap(), Move { from: app.clone(), to: repos.join("someone/app") });
        git(&app, &["worktree", "add", "-q", "-b", "task-1", ".kanna-worktrees/task-1"]);
        assert_eq!(plan(&repos, &app.join(".kanna-worktrees/task-1"), &homes).unwrap().from, app, "a worktree moves its repo");
        assert_eq!(plan(&repos, &dir.join("nowhere"), &homes).unwrap_err().code, "not_found");
        fs::create_dir_all(dir.join("home")).unwrap();
        assert_eq!(plan(&repos, &dir.join("home"), &homes).unwrap_err().code, "bad_request", "home isn't a project");
        let other = dir.join("elsewhere/app");
        repo(&other, Some("https://github.com/someone/app"));
        fs::create_dir_all(repos.join("someone")).unwrap();
        fs::rename(&other, repos.join("someone/app")).unwrap();
        let clash = plan(&repos, &app, &homes).unwrap_err();
        assert_eq!(clash.code, "clash");
        assert!(clash.message.contains(&app.display().to_string()) && clash.message.contains("repos/someone/app") && clash.message.contains("github.com/someone/app"), "{}", clash.message);
        assert_eq!(plan(&repos, &repos.join("someone/app"), &homes).unwrap_err().code, "in_place");
        fs::remove_dir_all(repos.join("someone/app")).unwrap();
        fs::create_dir_all(repos.join("someone/app")).unwrap();
        assert_eq!(plan(&repos, &app, &homes).unwrap_err().code, "taken", "a folder that isn't a clone of it");
        let local = dir.join("work/notes");
        fs::create_dir_all(&local).unwrap();
        assert_eq!(plan(&repos, &local, &homes).unwrap().to, repos.join("local/notes"), "a folder with no remote goes in local");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_move_takes_the_worktrees_along_and_leaves_nothing_behind() {
        let dir = folder("carry");
        let app = dir.join("work/app");
        repo(&app, None);
        git(&app, &["worktree", "add", "-q", "-b", "task-1", ".kanna-worktrees/task-1"]);
        git(&app, &["worktree", "add", "-q", "-b", "side", dir.join("side").to_str().unwrap()]);
        let to = dir.join("repos/local/app");
        assert_eq!(carry(&Move { from: app.clone(), to: to.clone() }), Ok(2));
        assert!(!app.exists() && fs::symlink_metadata(&app).is_err(), "no link is left at the old path");
        let listed = |at: &Path| String::from_utf8_lossy(&Command::new("git").arg("-C").arg(at).args(["worktree", "list", "--porcelain"]).output().unwrap().stdout).to_string();
        let found = listed(&to);
        assert!(found.contains(&format!("worktree {}", to.join(".kanna-worktrees/task-1").display())) && !found.contains("prunable"), "{found}");
        for at in [to.join(".kanna-worktrees/task-1"), dir.join("side")] {
            let status = Command::new("git").arg("-C").arg(&at).args(["status", "--porcelain"]).output().unwrap();
            assert!(status.status.success(), "{} works after the move: {}", at.display(), String::from_utf8_lossy(&status.stderr));
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_agent_finds_its_conversations_in_the_new_place() {
        let dir = folder("sessions");
        let (from, to) = (dir.join("work/app"), dir.join("repos/a/app"));
        fs::create_dir_all(to.join(".kanna-worktrees/task-1")).unwrap();
        fs::create_dir_all(to.join("docs/specs")).unwrap();
        let stores = Stores { claude: dir.join(".claude"), codex: dir.join(".codex"), copilot: dir.join(".copilot"), opencode: dir.join("opencode") };
        let projects = stores.claude.join("projects");
        let old = claude_name(&from);
        for name in [old.clone(), format!("{old}--kanna-worktrees-task-1"), format!("{old}--kanna-worktrees-task-gone"), format!("{old}-docs-specs"), format!("{old}-old")] {
            fs::create_dir_all(projects.join(&name).join("memory")).unwrap();
            fs::write(projects.join(&name).join("s.jsonl"), "{}\n").unwrap();
        }
        fs::create_dir_all(projects.join(claude_name(&to)).join("memory")).unwrap();
        fs::write(projects.join(claude_name(&to)).join("memory/MEMORY.md"), "kept\n").unwrap();
        fs::write(projects.join(&old).join("memory/MEMORY.md"), "old\n").unwrap();
        fs::write(projects.join(&old).join("memory/notes.md"), "notes\n").unwrap();
        let day = stores.codex.join("sessions/2026/10/29");
        fs::create_dir_all(&day).unwrap();
        let ours = format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"s1\",\"cwd\":\"{}/.kanna-worktrees/task-1\"}}}}\n{{\"type\":\"turn_context\",\"payload\":{{\"cwd\":\"{}\"}}}}\n", from.display(), from.display());
        let theirs = format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"s2\",\"cwd\":\"{}-old\"}}}}\n", from.display());
        fs::write(day.join("rollout-1.jsonl"), &ours).unwrap();
        fs::write(day.join("rollout-2.jsonl"), &theirs).unwrap();
        let before = fs::metadata(day.join("rollout-1.jsonl")).unwrap().modified().unwrap();
        let state = stores.copilot.join("session-state/c1");
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("workspace.yaml"), format!("id: c1\ncwd: {}\ngit_root: \"{}\"\nsummary: from {}\n", from.join(".kanna-worktrees/task-1").display(), from.display(), from.display())).unwrap();
        fs::create_dir_all(&stores.opencode).unwrap();
        let db = rusqlite::Connection::open(stores.opencode.join("opencode.db")).unwrap();
        db.execute_batch(&format!("create table session (id text, directory text); insert into session values ('o1', '{}'), ('o2', '{}-old'), ('o3', '{}/sub');", from.display(), from.display(), from.display())).unwrap();
        drop(db);
        let moved = stores.carry(&from, &to);
        assert_eq!((moved.claude, moved.codex, moved.copilot, moved.opencode), (4, 1, 1, 2), "{moved:?}");
        let mut names: Vec<String> = fs::read_dir(&projects).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        let new = claude_name(&to);
        let mut wanted = vec![new.clone(), format!("{new}--kanna-worktrees-task-1"), format!("{new}--kanna-worktrees-task-gone"), format!("{new}-docs-specs"), old.clone(), format!("{old}-old")];
        wanted.sort();
        assert_eq!(names, wanted, "a sibling folder whose name starts the same isn't the project's");
        assert_eq!(files_under(&projects.join(&old), 2), [projects.join(&old).join("memory/MEMORY.md")], "only what clashed stays behind");
        assert_eq!(fs::read_to_string(projects.join(&new).join("memory/MEMORY.md")).unwrap(), "kept\n", "a file already there is kept");
        assert_eq!(fs::read_to_string(projects.join(&new).join("memory/notes.md")).unwrap(), "notes\n", "and the rest is merged in");
        assert_eq!(moved.kept.len(), 1, "{:?}", moved.kept);
        let rollout = fs::read_to_string(day.join("rollout-1.jsonl")).unwrap();
        assert_eq!(rollout, ours.replace(&from.display().to_string(), &to.display().to_string()));
        assert_eq!(fs::metadata(day.join("rollout-1.jsonl")).unwrap().modified().unwrap(), before, "the rollout keeps its time, which the session lookups go by");
        assert_eq!(fs::read_to_string(day.join("rollout-2.jsonl")).unwrap(), theirs);
        assert_eq!(fs::read_to_string(state.join("workspace.yaml")).unwrap(), format!("id: c1\ncwd: {}\ngit_root: \"{}\"\nsummary: from {}\n", to.join(".kanna-worktrees/task-1").display(), to.display(), from.display()));
        let db = rusqlite::Connection::open(stores.opencode.join("opencode.db")).unwrap();
        let dirs: Vec<String> = db.prepare("select directory from session order by id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(dirs, [to.display().to_string(), format!("{}-old", from.display()), format!("{}/sub", to.display())]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn kanna_db_follows_the_project() {
        let dir = folder("db");
        let db = Db::open(&dir);
        let row = |id: &str, repo: &str, folder: &str| crate::db::Row { id: id.into(), repo: repo.into(), place: crate::db::Place::Backlog, terminal: None, made_at: 1.0, doc: json!({ "name": id, "folder": folder }) };
        db.put(&row("task-00000001", "/w/app", "/w/app/.kanna-worktrees/task-00000001"), None).unwrap();
        db.put(&row("task-00000002", "/w/app-old", "/w/app-old/.kanna-worktrees/task-00000002"), None).unwrap();
        db.put_message("task-00000001", "/w/app", "user", "hi", None).unwrap();
        db.touch_project("/w/app").unwrap();
        db.touch_project("/r/a/app").unwrap();
        db.put_planner("/w/app", &json!({ "agent": "claude", "session": "s" })).unwrap();
        db.relocate("/w/app", "/r/a/app").unwrap();
        let tasks = db.tasks().unwrap();
        let of = |id: &str| tasks.iter().find(|t| t.id == id).cloned().unwrap();
        assert_eq!((of("task-00000001").repo, of("task-00000001").doc["folder"].clone()), ("/r/a/app".into(), json!("/r/a/app/.kanna-worktrees/task-00000001")));
        assert_eq!(of("task-00000001").doc["name"], json!("task-00000001"), "the rest of the task is kept");
        assert_eq!(of("task-00000002").repo, "/w/app-old", "a folder whose name starts the same is left alone");
        assert_eq!(db.messages(None).unwrap()[0].repo, "/r/a/app");
        assert_eq!(db.projects().unwrap().iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["/r/a/app"], "the project's recent entry replaces a stale one at its place");
        assert_eq!(db.planner("/r/a/app").unwrap(), Some(json!({ "agent": "claude", "session": "s" })), "the planner comes back to its conversation");
        assert_eq!(db.planner("/w/app").unwrap(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_still_names_the_old_path_is_listed() {
        let dir = folder("mentions");
        let home = dir.join("home");
        let to = home.join(".kanna-v4/repos/a/app");
        repo(&to, None);
        fs::create_dir_all(home.join(".claude/projects/x")).unwrap();
        fs::create_dir_all(home.join(".agents/skills/s")).unwrap();
        fs::write(home.join(".claude/CLAUDE.md"), "# Me\n\nWork in ~/work/app.\n").unwrap();
        fs::write(home.join(".claude/projects/x/MEMORY.md"), "~/work/app\n").unwrap();
        fs::write(home.join(".agents/skills/s/SKILL.md"), "ok\nrun $HOME/work/app/x.sh\n").unwrap();
        fs::write(home.join(".zshrc"), format!("cd {}\n", home.join("work/app").display())).unwrap();
        fs::write(to.join("build.sh"), "cd ~/work/app/sub\n").unwrap();
        let mut found = mentions(&home, &to, &home.join("work/app"));
        found.sort();
        assert_eq!(found, ["~/.agents/skills/s/SKILL.md:2", "~/.claude/CLAUDE.md:3", "~/.kanna-v4/repos/a/app/build.sh:1", "~/.zshrc:1"].map(|f| f.replace("~", &tilde(&home))));
        fs::remove_dir_all(&dir).unwrap();
    }
}
