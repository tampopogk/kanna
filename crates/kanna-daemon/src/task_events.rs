use crate::stacks::{self, Where};
use crate::task_scripts;
use crate::tasks::git;
use kanna_mux::json::{json, num, string, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

const COMMITS_SHOWN: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskEvent {
    MainConflicts,
    MainMoved,
    BaseMerged,
    BaseMoved,
}

impl TaskEvent {
    pub const ALL: [TaskEvent; 4] = [TaskEvent::MainConflicts, TaskEvent::MainMoved, TaskEvent::BaseMerged, TaskEvent::BaseMoved];

    pub fn name(self) -> &'static str {
        match self {
            TaskEvent::MainConflicts => "main-conflicts",
            TaskEvent::MainMoved => "main-moved",
            TaskEvent::BaseMerged => "base-merged",
            TaskEvent::BaseMoved => "base-moved",
        }
    }

    pub fn parse(name: &str) -> Option<TaskEvent> {
        TaskEvent::ALL.into_iter().find(|event| event.name() == name)
    }

    pub fn names() -> String {
        TaskEvent::ALL.map(TaskEvent::name).join(", ")
    }

    fn heard_by_default(self) -> bool {
        self != TaskEvent::MainMoved
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hears {
    added: Vec<String>,
    dropped: Vec<String>,
}

impl Hears {
    pub fn hears(&self, event: TaskEvent) -> bool {
        match event.heard_by_default() {
            true => !self.dropped.iter().any(|name| name == event.name()),
            false => self.added.iter().any(|name| name == event.name()),
        }
    }

    pub fn set(&mut self, event: TaskEvent, on: bool) {
        let name = event.name().to_string();
        self.added.retain(|n| *n != name);
        self.dropped.retain(|n| *n != name);
        if on != event.heard_by_default() {
            (if on { &mut self.added } else { &mut self.dropped }).push(name);
        }
    }

    pub fn names(&self) -> Vec<&'static str> {
        TaskEvent::ALL.into_iter().filter(|event| self.hears(*event)).map(TaskEvent::name).collect()
    }

    pub fn to_value(&self) -> Value {
        json!({ "added": self.added, "dropped": self.dropped })
    }

    pub fn from_value(value: Option<&Value>) -> Hears {
        Hears { added: stacks::ids(value.and_then(|v| v.get("added"))), dropped: stacks::ids(value.and_then(|v| v.get("dropped"))) }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sent {
    pub event: String,
    pub message: i64,
    pub about: Option<String>,
    pub at: f64,
}

impl Sent {
    pub fn to_value(&self) -> Value {
        json!({ "event": self.event, "message": self.message, "about": self.about, "at": num(self.at) })
    }

    pub fn list(value: Option<&Value>) -> Vec<Sent> {
        value
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|sent| {
                Some(Sent { event: string(sent, "event")?.to_string(), message: sent.get("message")?.as_i64()?, about: string(sent, "about").map(String::from), at: sent.get("at").and_then(Value::as_f64).unwrap_or(0.0) })
            })
            .collect()
    }

    pub fn of(sent: &[Sent], event: TaskEvent) -> Option<&Sent> {
        sent.iter().find(|s| s.event == event.name())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Main {
    pub branch: String,
    pub head: String,
}

impl Main {
    pub fn saved(text: &str) -> Option<Main> {
        let (branch, head) = text.rsplit_once(' ')?;
        Some(Main { branch: branch.to_string(), head: head.to_string() })
    }

    pub fn text(&self) -> String {
        format!("{} {}", self.branch, self.head)
    }
}

#[derive(Clone, Debug)]
pub struct Probe {
    pub repo: PathBuf,
    pub last: Option<Main>,
    pub tasks: Vec<TaskProbe>,
}

#[derive(Clone, Debug)]
pub struct TaskProbe {
    pub id: String,
    pub folder: PathBuf,
    pub conflicts: bool,
    pub commits: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub repo: PathBuf,
    pub main: Main,
    pub moved: bool,
    pub tasks: Vec<SeenTask>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeenTask {
    pub id: String,
    pub base: String,
    pub conflicts: Vec<String>,
    pub ahead: usize,
    pub commits: Vec<String>,
}

pub fn look(probes: Vec<Probe>) -> Vec<Seen> {
    probes
        .into_iter()
        .filter_map(|probe| {
            let main = main_of(&probe.repo)?;
            let moved = probe.last.as_ref().is_some_and(|last| last.branch == main.branch && last.head != main.head);
            let tasks = match moved {
                true => probe.tasks.iter().filter_map(|task| check(&probe.repo, &main, task)).collect(),
                false => vec![],
            };
            Some(Seen { repo: probe.repo, main, moved, tasks })
        })
        .collect()
}

fn main_of(repo: &Path) -> Option<Main> {
    let branch = git(repo, &["branch", "--show-current"]).ok().filter(|b| !b.is_empty())?;
    let head = stacks::head(repo, &Where::Folder(repo.to_path_buf()))?;
    Some(Main { branch, head })
}

fn check(repo: &Path, main: &Main, task: &TaskProbe) -> Option<SeenTask> {
    git(&task.folder, &["branch", "--show-current"]).ok().filter(|b| !b.is_empty())?;
    if task_scripts::merged(repo, &task.folder) {
        return None;
    }
    let tip = stacks::head(repo, &Where::Folder(task.folder.clone()))?;
    let base = git(repo, &["merge-base", &main.head, &tip]).ok()?;
    let conflicts = match task.conflicts {
        true => conflicts(repo, &main.head, &tip).inspect_err(|error| log!("kanna-daemon: can't check {} against {}: {error}", task.id, main.branch)).ok()?,
        false => vec![],
    };
    let since = format!("{base}..{}", main.head);
    let (ahead, commits) = match task.commits {
        true => {
            let ahead = git(repo, &["rev-list", "--count", &since]).ok()?.parse().ok()?;
            let commits = git(repo, &["log", &format!("-{COMMITS_SHOWN}"), "--format=%h %s", &since]).ok()?;
            (ahead, commits.lines().map(String::from).collect())
        }
        false => (0, vec![]),
    };
    Some(SeenTask { id: task.id.clone(), base, conflicts, ahead, commits })
}

pub fn conflicts(repo: &Path, main: &str, tip: &str) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false"])
        .arg("-C")
        .arg(repo)
        .args(["merge-tree", "--write-tree", "--name-only", "--no-messages", "-z", main, tip])
        .output()
        .map_err(|e| format!("can't run git: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(vec![]),
        Some(1) if !out.stdout.is_empty() => {
            let mut files: Vec<String> = out.stdout.split(|b| *b == 0).skip(1).filter(|f| !f.is_empty()).map(|f| String::from_utf8_lossy(f).into_owned()).collect();
            files.sort();
            files.dedup();
            Ok(files)
        }
        _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Nothing,
    Tell { about: String },
    Over,
}

pub fn conflict_step(seen: &SeenTask, told: Option<&Sent>, waiting: bool) -> Step {
    let about = told.and_then(|sent| sent.about.as_deref());
    if seen.conflicts.is_empty() {
        return if about.is_some() { Step::Over } else { Step::Nothing };
    }
    let now = format!("{} {}", seen.base, seen.conflicts.join("\n"));
    if waiting || about == Some(now.as_str()) {
        return Step::Nothing;
    }
    Step::Tell { about: now }
}

pub fn conflict_text(main: &str, files: &[String]) -> String {
    format!("{main} conflicts with your branch in {}: merge or rebase {main}, rerun your tests, then carry on with what you were doing.", files.join(", "))
}

pub fn moved_text(main: &str, seen: &SeenTask) -> String {
    let mut text = format!("{main} moved: {} commit{} your branch hasn't merged yet.", seen.ahead, if seen.ahead == 1 { "" } else { "s" });
    for commit in &seen.commits {
        text += &format!("\n{commit}");
    }
    if seen.ahead > seen.commits.len() {
        text += &format!("\nand {} older", seen.ahead - seen.commits.len());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, script: &str) {
        let out = Command::new("/bin/sh").arg("-c").arg(script).current_dir(dir).env("GIT_AUTHOR_NAME", "e").env("GIT_AUTHOR_EMAIL", "e@e").env("GIT_COMMITTER_NAME", "e").env("GIT_COMMITTER_EMAIL", "e@e").output().unwrap();
        assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-task-events-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn a_task_hears_the_defaults_and_keeps_only_what_it_changed() {
        let mut hears = Hears::default();
        assert_eq!(hears.names(), vec!["main-conflicts", "base-merged", "base-moved"]);
        hears.set(TaskEvent::MainMoved, true);
        hears.set(TaskEvent::BaseMoved, false);
        assert_eq!(hears.names(), vec!["main-conflicts", "main-moved", "base-merged"]);
        assert_eq!(hears.to_value(), json!({ "added": ["main-moved"], "dropped": ["base-moved"] }));
        assert_eq!(Hears::from_value(Some(&hears.to_value())), hears);
        hears.set(TaskEvent::MainMoved, false);
        hears.set(TaskEvent::BaseMoved, true);
        assert_eq!(hears, Hears::default(), "back at the defaults, it keeps nothing, so a new default reaches it");
        assert_eq!(Hears::from_value(None), Hears::default(), "a task from before events hears the defaults");
    }

    #[test]
    fn conflicts_name_each_file_once_and_touch_no_worktree() {
        let dir = scratch("conflicts");
        sh(&dir, "git init -q -b main && git config user.name e && git config user.email e@e && printf 'a\\n' >'a file' && printf 'b\\n' >b && printf 'c\\n' >c && git add . && git commit -q -m first && git checkout -q -b t && printf 'x\\n' >'a file' && printf 'x\\n' >b && git commit -q -am t && git checkout -q main && printf 'y\\n' >'a file' && printf 'y\\n' >b && printf 'y\\n' >c && git commit -q -am m");
        assert_eq!(conflicts(&dir, "main", "t").unwrap(), vec!["a file".to_string(), "b".to_string()]);
        assert_eq!(std::fs::read_to_string(dir.join("b")).unwrap(), "y\n");
        sh(&dir, "git checkout -q -b clean main~1 && echo z >z && git add z && git commit -q -m z && git checkout -q main");
        assert_eq!(conflicts(&dir, "main", "clean").unwrap(), Vec::<String>::new());
        assert!(conflicts(&dir, "main", "no-such-commit").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_move_is_the_same_branch_at_a_new_commit_and_the_first_look_only_records() {
        let dir = scratch("moves");
        sh(&dir, "git init -q -b main && git config user.name e && git config user.email e@e && git commit -q --allow-empty -m first && git worktree add -q -b t wt && git -C wt commit -q --allow-empty -m t");
        let probe = |last: Option<Main>| Probe { repo: dir.clone(), last, tasks: vec![TaskProbe { id: "t".into(), folder: dir.join("wt"), conflicts: true, commits: true }] };
        let first = look(vec![probe(None)]).remove(0);
        assert!(!first.moved && first.tasks.is_empty(), "{first:?}");
        assert_eq!(first.main.branch, "main");
        let still = look(vec![probe(Some(first.main.clone()))]).remove(0);
        assert!(!still.moved);
        sh(&dir, "git commit -q --allow-empty -m second");
        let moved = look(vec![probe(Some(first.main.clone()))]).remove(0);
        assert!(moved.moved);
        assert_eq!((moved.tasks[0].ahead, moved.tasks[0].commits.len()), (1, 1));
        assert!(moved.tasks[0].commits[0].ends_with(" second"), "{:?}", moved.tasks[0].commits);
        let elsewhere = Main { branch: "other".into(), head: first.main.head.clone() };
        assert!(!look(vec![probe(Some(elsewhere))])[0].moved, "another branch checked out isn't main moving");
        sh(&dir, "git -C wt checkout -q --detach");
        assert!(look(vec![probe(Some(first.main.clone()))])[0].tasks.is_empty(), "a task mid-rebase has no branch to check");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_conflict_is_told_once_until_its_files_or_merge_base_change() {
        let seen = |base: &str, files: &[&str]| SeenTask { id: "t".into(), base: base.into(), conflicts: files.iter().map(|f| f.to_string()).collect(), ..Default::default() };
        let Step::Tell { about } = conflict_step(&seen("b1", &["a"]), None, false) else { panic!("a new conflict is told") };
        let told = Sent { event: "main-conflicts".into(), message: 1, about: Some(about), at: 0.0 };
        assert_eq!(conflict_step(&seen("b1", &["a"]), Some(&told), false), Step::Nothing, "same files, same merge base");
        assert_eq!(conflict_step(&seen("b1", &["a", "b"]), Some(&told), true), Step::Nothing, "the last message still waits");
        assert!(matches!(conflict_step(&seen("b1", &["a", "b"]), Some(&told), false), Step::Tell { .. }), "new files, once it's taken");
        assert!(matches!(conflict_step(&seen("b2", &["a"]), Some(&told), false), Step::Tell { .. }), "it merged main, and main conflicts with it again");
        assert_eq!(conflict_step(&seen("b1", &[]), Some(&told), false), Step::Over);
        let over = Sent { about: None, ..told };
        assert_eq!(conflict_step(&seen("b1", &[]), Some(&over), false), Step::Nothing);
        assert!(matches!(conflict_step(&seen("b1", &["a"]), Some(&over), false), Step::Tell { .. }), "a conflict that went and came back is told");
    }

    #[test]
    fn main_moving_says_how_many_commits_and_shows_the_latest() {
        let seen = SeenTask { ahead: 22, commits: vec!["abc one".into(), "def two".into()], ..Default::default() };
        assert_eq!(moved_text("main", &seen), "main moved: 22 commits your branch hasn't merged yet.\nabc one\ndef two\nand 20 older");
        let one = SeenTask { ahead: 1, commits: vec!["abc one".into()], ..Default::default() };
        assert_eq!(moved_text("trunk", &one), "trunk moved: 1 commit your branch hasn't merged yet.\nabc one");
    }
}
