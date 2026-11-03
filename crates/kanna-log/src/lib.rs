pub mod graph;

use kanna_diff::git;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const LIMIT: usize = 2000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskRef {
    pub id: String,
    pub name: Option<String>,
    pub place: String,
    pub repo: String,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    pub tip: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Label {
    pub id: String,
    pub name: String,
    pub place: String,
    pub branch: String,
    pub tip: String,
    pub ahead: u64,
    pub behind: u64,
    pub merged: bool,
}

impl Label {
    pub fn to_json(&self) -> Value {
        json!({ "id": self.id, "name": self.name, "place": self.place, "branch": self.branch, "tip": self.tip, "ahead": self.ahead, "behind": self.behind, "merged": self.merged })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub time: i64,
    pub refs: Vec<String>,
    pub subject: String,
    pub spine: bool,
    pub side: Option<String>,
    pub fold: usize,
}

impl Row {
    pub fn to_json(&self) -> Value {
        json!({
            "hash": self.hash, "parents": self.parents, "author": self.author, "time": self.time, "refs": self.refs, "subject": self.subject,
            "spine": self.spine, "side": self.side, "fold": self.fold,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Log {
    pub folder: String,
    pub args: Vec<String>,
    pub top: String,
    pub git_dir: String,
    pub common_dir: String,
    pub main: String,
    pub range: Vec<String>,
    pub rows: Vec<Row>,
    pub tasks: Vec<Label>,
    pub more: bool,
    pub error: Option<String>,
}

impl Log {
    pub fn command(&self) -> String {
        std::iter::once("git log").chain(self.range.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
    }

    pub fn to_json(&self) -> Value {
        json!({
            "folder": self.folder, "args": self.args, "top": self.top, "main": self.main, "range": self.range, "command": self.command(),
            "rows": self.rows.iter().map(Row::to_json).collect::<Vec<_>>(), "tasks": self.tasks.iter().map(Label::to_json).collect::<Vec<_>>(),
            "more": self.more, "error": self.error,
        })
    }
}

fn text(args: &[&str], folder: &str) -> Result<String, String> {
    git(args, folder, None).map(|out| String::from_utf8_lossy(&out).into_owned())
}

fn line(args: &[&str], folder: &str) -> Option<String> {
    text(args, folder).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

pub struct Repo {
    pub top: String,
    pub git_dir: String,
    pub common_dir: String,
    pub checkout: Option<String>,
    pub main: String,
}

impl Repo {
    pub fn find(folder: &str) -> Result<Repo, String> {
        let dirs = text(&["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-dir", "--git-common-dir"], folder)?;
        let mut dirs = dirs.lines().map(String::from);
        let (top, git_dir, common_dir) = (dirs.next().unwrap_or_default(), dirs.next().unwrap_or_default(), dirs.next().unwrap_or_default());
        let checkout = Path::new(&common_dir).file_name().filter(|n| *n == ".git").and_then(|_| Path::new(&common_dir).parent()).map(|p| p.to_string_lossy().into_owned()).filter(|p| Path::new(p).is_dir());
        let main = checkout.as_deref().and_then(|c| line(&["symbolic-ref", "--quiet", "--short", "HEAD"], c)).or_else(|| ["main", "master"].into_iter().find(|b| line(&["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")], folder).is_some()).map(String::from)).unwrap_or_else(|| "HEAD".into());
        Ok(Repo { top, git_dir, common_dir, checkout, main })
    }

    pub fn linked(&self) -> bool {
        canonical(&self.git_dir) != canonical(&self.common_dir)
    }

    fn owns(&self, repo: &str) -> bool {
        self.checkout.as_deref().is_some_and(|c| canonical(c) == canonical(repo))
    }
}

fn canonical(path: &str) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

pub fn split(args: &[String]) -> (&[String], &[String]) {
    (kanna_diff::options(args), kanna_diff::pathspecs(args))
}

pub fn load(folder: &str, args: &[String], tasks: &[TaskRef]) -> Log {
    let mut log = Log { folder: folder.into(), args: args.to_vec(), ..Default::default() };
    if let Err(error) = fill(&mut log, tasks) {
        log.error = Some(error);
        log.rows.clear();
        log.tasks.clear();
    }
    log
}

fn fill(log: &mut Log, tasks: &[TaskRef]) -> Result<(), String> {
    let repo = Repo::find(&log.folder)?;
    log.top = repo.top.clone();
    log.git_dir = repo.git_dir.clone();
    log.common_dir = repo.common_dir.clone();
    log.main = repo.main.clone();
    let labels = labels(&repo, tasks);
    let (revs, paths) = split(&log.args);
    let mut range: Vec<String> = revs.to_vec();
    if range.iter().all(|r| r.starts_with('-')) {
        if repo.linked() {
            range.push("HEAD".into());
        } else {
            range.extend(labels.iter().filter(|l| l.place != "done" && !l.merged).map(|l| l.branch.clone()).filter(|b| *b != repo.main));
        }
        range.push(repo.main.clone());
        range.dedup();
    }
    log.range = range.clone();
    let mut query: Vec<String> = ["-c", "log.showSignature=false", "log", "--no-color", "--topo-order", "-z", "--format=%H%x1f%P%x1f%an%x1f%at%x1f%D%x1f%s", "--decorate-refs-exclude=refs/remotes/*/HEAD"].map(String::from).to_vec();
    query.push(format!("--max-count={}", LIMIT + 1));
    query.extend(range);
    query.push("--".into());
    query.extend(paths.iter().cloned());
    let out = text(&query.iter().map(String::as_str).collect::<Vec<_>>(), &log.folder)?;
    let mut rows = parse(&out);
    log.more = rows.len() > LIMIT;
    rows.truncate(LIMIT);
    let main_tip = line(&["rev-parse", "--verify", "--quiet", &format!("{}^{{commit}}", repo.main)], &log.folder);
    graph::shape(&mut rows, main_tip.as_deref());
    let mut labels = labels;
    labels.extend(done_labels(&repo, &rows, main_tip.as_deref(), tasks));
    log.rows = rows;
    log.tasks = labels;
    Ok(())
}

pub fn parse(out: &str) -> Vec<Row> {
    out.split('\0')
        .map(|record| record.trim_start_matches('\n'))
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let fields: Vec<&str> = record.splitn(6, '\x1f').collect();
            let [hash, parents, author, time, refs, subject] = fields.as_slice() else { return None };
            Some(Row {
                hash: hash.to_string(),
                parents: parents.split_whitespace().map(String::from).collect(),
                author: author.to_string(),
                time: time.parse().unwrap_or(0),
                refs: refs
                    .split(", ")
                    .flat_map(|r| match r.strip_prefix("HEAD -> ") {
                        Some(branch) => vec!["HEAD".to_string(), branch.to_string()],
                        None => vec![r.to_string()],
                    })
                    .filter(|r| !r.is_empty())
                    .collect(),
                subject: subject.to_string(),
                ..Default::default()
            })
        })
        .collect()
}

pub fn labels(repo: &Repo, tasks: &[TaskRef]) -> Vec<Label> {
    let mine: Vec<&TaskRef> = tasks.iter().filter(|t| t.place != "done" && repo.owns(&t.repo)).collect();
    let folder = repo.top.clone();
    let main = repo.main.clone();
    std::thread::scope(|scope| {
        let checks: Vec<_> = mine.into_iter().map(|task| scope.spawn(|| label(task, &folder, &main))).collect();
        checks.into_iter().filter_map(|c| c.join().ok().flatten()).collect()
    })
}

pub fn done_labels(repo: &Repo, rows: &[Row], main: Option<&str>, tasks: &[TaskRef]) -> Vec<Label> {
    let at: HashMap<&str, &Row> = rows.iter().map(|r| (r.hash.as_str(), r)).collect();
    let mut in_main: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = main.into_iter().collect();
    while let Some(hash) = stack.pop() {
        if let Some(row) = at.get(hash).filter(|_| in_main.insert(hash)) {
            stack.extend(row.parents.iter().map(String::as_str));
        }
    }
    tasks
        .iter()
        .filter(|t| t.place == "done" && repo.owns(&t.repo))
        .filter_map(|t| {
            let (branch, tip) = (t.branch.clone()?, t.tip.clone()?);
            at.contains_key(tip.as_str()).then(|| Label { id: t.id.clone(), name: t.name.clone().unwrap_or_else(|| t.id.clone()), place: t.place.clone(), merged: in_main.contains(tip.as_str()), branch, tip, ahead: 0, behind: 0 })
        })
        .collect()
}

fn label(task: &TaskRef, folder: &str, main: &str) -> Option<Label> {
    let worktree = task.worktree.as_deref().filter(|w| Path::new(w).is_dir())?;
    let branch = line(&["branch", "--show-current"], worktree)?;
    let tip = line(&["rev-parse", "--verify", "--quiet", "HEAD"], worktree)?;
    let base = line(&["rev-parse", "--path-format=absolute", "--git-dir"], worktree).and_then(|dir| std::fs::read_to_string(Path::new(&dir).join("kanna-base")).ok()).map(|b| b.trim().to_string());
    if base.as_deref() == Some(tip.as_str()) {
        return None;
    }
    let counts = line(&["rev-list", "--left-right", "--count", &format!("{main}...{tip}")], folder)?;
    let mut counts = counts.split_whitespace().map(|n| n.parse::<u64>().unwrap_or(0));
    let (behind, ahead) = (counts.next().unwrap_or(0), counts.next().unwrap_or(0));
    Some(Label { id: task.id.clone(), name: task.name.clone().unwrap_or_else(|| task.id.clone()), place: task.place.clone(), branch, tip, ahead, behind, merged: ahead == 0 })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changed {
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

pub fn commit(folder: &str, hash: &str) -> Result<Value, String> {
    if hash.is_empty() || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{hash:?} isn't a commit hash"));
    }
    let head = text(&["show", "--no-color", "-s", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%B", hash], folder)?;
    let fields: Vec<&str> = head.splitn(6, '\x1f').collect();
    let [full, parents, author, email, time, message] = fields.as_slice() else { return Err(format!("git show {hash} said something unexpected")) };
    let files = changes(folder, full)?;
    Ok(json!({
        "hash": full, "parents": parents.split_whitespace().collect::<Vec<_>>(), "author": author, "email": email, "time": time.parse::<i64>().unwrap_or(0),
        "message": message.trim_end(),
        "files": files.iter().map(|f| json!({ "path": f.path, "old": f.old_path, "status": f.status, "added": f.added, "removed": f.removed })).collect::<Vec<_>>(),
    }))
}

fn changes(folder: &str, hash: &str) -> Result<Vec<Changed>, String> {
    let common = ["show", "--no-color", "--format=", "-z", "--diff-merges=first-parent", "-M"];
    let names = text(&[&common[..], &["--name-status", hash]].concat(), folder)?;
    let mut files = vec![];
    let mut fields = names.split('\0').filter(|f| !f.is_empty()).map(|f| f.trim_start_matches('\n'));
    while let Some(status) = fields.next() {
        let letter = status.chars().next().unwrap_or('M').to_string();
        let (old_path, path) = if matches!(letter.as_str(), "R" | "C") { (fields.next().map(String::from), fields.next().unwrap_or_default().to_string()) } else { (None, fields.next().unwrap_or_default().to_string()) };
        files.push(Changed { path, old_path, status: letter, added: None, removed: None });
    }
    let stats = text(&[&common[..], &["--numstat", hash]].concat(), folder)?;
    let mut counts: HashMap<String, (Option<u64>, Option<u64>)> = HashMap::new();
    let mut fields = stats.split('\0').map(|f| f.trim_start_matches('\n'));
    while let Some(field) = fields.next() {
        let mut parts = field.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let path = if path.is_empty() {
            let _old = fields.next();
            fields.next().unwrap_or_default().to_string()
        } else {
            path.to_string()
        };
        counts.insert(path, (added.parse().ok(), removed.parse().ok()));
    }
    for file in &mut files {
        if let Some((added, removed)) = counts.get(&file.path) {
            file.added = *added;
            file.removed = *removed;
        }
    }
    Ok(files)
}
