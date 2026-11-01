use crate::task_scripts;
use crate::tasks::{git, make_worktree};
use kanna_mux::json::{json, num, string, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const KANNA: &str = "kanna";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mark {
    pub head: String,
    pub branch: String,
    pub at: f64,
}

impl Mark {
    pub fn to_value(&self) -> Value {
        json!({ "head": self.head, "branch": self.branch, "at": num(self.at) })
    }

    pub fn from_value(value: Option<&Value>) -> Option<Mark> {
        let value = value?;
        Some(Mark { head: string(value, "head")?.to_string(), branch: string(value, "branch").unwrap_or_default().to_string(), at: value.get("at").and_then(Value::as_f64).unwrap_or(0.0) })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Base {
    pub id: String,
    pub branch: String,
    pub head: String,
}

impl Base {
    pub fn to_value(&self) -> Value {
        json!({ "id": self.id, "branch": self.branch, "head": self.head })
    }

    pub fn list(value: Option<&Value>) -> Vec<Base> {
        value.and_then(Value::as_array).into_iter().flatten().filter_map(|b| Some(Base { id: string(b, "id")?.to_string(), branch: string(b, "branch").unwrap_or_default().to_string(), head: string(b, "head")?.to_string() })).collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Conflict {
    pub heads: Vec<String>,
    pub why: String,
}

impl Conflict {
    pub fn to_value(&self) -> Value {
        json!({ "heads": self.heads, "why": self.why })
    }

    pub fn from_value(value: Option<&Value>) -> Option<Conflict> {
        let value = value?;
        Some(Conflict { heads: value.get("heads")?.as_array()?.iter().filter_map(Value::as_str).map(String::from).collect(), why: string(value, "why").unwrap_or_default().to_string() })
    }
}

pub fn ids(list: Option<&Value>) -> Vec<String> {
    list.and_then(Value::as_array).map_or(vec![], |ids| ids.iter().filter_map(Value::as_str).map(String::from).collect())
}

#[derive(Debug)]
pub enum Made {
    Worktree { folder: String, branch: String, bases: Vec<Base> },
    Conflict(String),
}

pub fn in_main(repo: &Path, head: &str) -> bool {
    contains(repo, head, "HEAD")
}

pub fn contains(repo: &Path, ancestor: &str, of: &str) -> bool {
    git(repo, &["merge-base", "--is-ancestor", ancestor, of]).is_ok()
}

pub fn make(repo: &Path, name: &str, bases: Vec<Base>) -> Result<Made, String> {
    let born = git(repo, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();
    let bases: Vec<Base> = bases.into_iter().filter(|b| !(born && in_main(repo, &b.head))).collect();
    let (folder, branch) = make_worktree(repo, name, match bases.as_slice() {
        [one] => Some(one.head.as_str()),
        _ => None,
    })?;
    if bases.len() > 1 {
        let here = Path::new(&folder);
        for base in &bases {
            let said = format!("Merge {} ({})", if base.branch.is_empty() { &base.head } else { &base.branch }, base.id);
            if let Err(error) = git(here, &["merge", "--no-edit", "-m", &said, &base.head]) {
                let _ = git(here, &["merge", "--abort"]);
                let _ = git(repo, &["worktree", "remove", "--force", &folder]);
                let _ = git(repo, &["branch", "-D", name]);
                let error = error.lines().next().filter(|l| !l.is_empty()).map_or(String::new(), |l| format!(" ({l})"));
                return Ok(Made::Conflict(format!("merging {}'s branch {} conflicts{error}", base.id, base.branch)));
            }
        }
        task_scripts::record_base(here);
    }
    Ok(Made::Worktree { folder, branch, bases })
}

#[derive(Clone, Debug, PartialEq)]
pub enum Where {
    Folder(PathBuf),
    Branch(String),
}

pub fn head(repo: &Path, at: &Where) -> Option<String> {
    match at {
        Where::Folder(folder) if folder.is_dir() => git(folder, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok(),
        Where::Folder(_) => None,
        Where::Branch(branch) if !branch.is_empty() => git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).ok(),
        Where::Branch(_) => None,
    }
}

pub fn dirty(folder: &Path) -> Result<bool, String> {
    git(folder, &["status", "--porcelain", "--untracked-files=no"]).map(|out| !out.is_empty())
}

#[derive(Clone, Debug)]
pub struct Probe {
    pub id: String,
    pub repo: PathBuf,
    pub at: Where,
    pub bases: Vec<(Base, Where)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seen {
    pub id: String,
    pub head: Option<String>,
    pub main: Option<String>,
    pub bases: Vec<SeenBase>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeenBase {
    pub id: String,
    pub head: String,
    pub merged: bool,
    pub contained: bool,
}

pub fn look(probes: Vec<Probe>) -> Vec<Seen> {
    probes
        .into_iter()
        .map(|probe| {
            let head = head(&probe.repo, &probe.at);
            let main = git(&probe.repo, &["branch", "--show-current"]).ok().filter(|b| !b.is_empty());
            let bases = match &head {
                None => vec![],
                Some(tip) => probe
                    .bases
                    .iter()
                    .map(|(base, at)| {
                        let now = self::head(&probe.repo, at).unwrap_or_else(|| base.head.clone());
                        let merged = in_main(&probe.repo, &now);
                        SeenBase { id: base.id.clone(), contained: merged || contains(&probe.repo, &now, tip), merged, head: now }
                    })
                    .collect(),
            };
            Seen { id: probe.id, head, main, bases }
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Task {
    pub id: String,
    pub ready: Option<Mark>,
    pub bases: Vec<Base>,
    pub stale: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Unready { id: String },
    Merged { id: String, base: String, main: Option<String> },
    Stale { id: String, behind: Vec<String>, waiting: Vec<String> },
}

pub fn settle(tasks: &[Task], seen: &[Seen]) -> Vec<Step> {
    let seen_of = |id: &str| seen.iter().find(|s| s.id == id);
    let mut steps = vec![];
    for task in tasks {
        if let (Some(mark), Some(head)) = (&task.ready, seen_of(&task.id).and_then(|s| s.head.as_ref())) {
            if *head != mark.head {
                steps.push(Step::Unready { id: task.id.clone() });
            }
        }
    }
    let mut behind: HashMap<&str, Vec<String>> = HashMap::new();
    let mut merged: HashSet<(&str, &str)> = HashSet::new();
    for task in tasks {
        let Some(seen) = seen_of(&task.id) else { continue };
        for base in seen.bases.iter().filter(|b| task.bases.iter().any(|t| t.id == b.id)) {
            if base.merged {
                merged.insert((&task.id, &base.id));
                steps.push(Step::Merged { id: task.id.clone(), base: base.id.clone(), main: seen.main.clone() });
            } else if !base.contained {
                behind.entry(&task.id).or_default().push(base.id.clone());
            }
        }
    }
    let mut stale: HashSet<&str> = tasks.iter().filter(|t| !t.stale.is_empty()).map(|t| t.id.as_str()).collect();
    loop {
        let mut grew = false;
        for task in tasks {
            if stale.contains(task.id.as_str()) {
                continue;
            }
            let waiting: Vec<String> = task.bases.iter().filter(|b| !merged.contains(&(task.id.as_str(), b.id.as_str())) && stale.contains(b.id.as_str())).map(|b| b.id.clone()).collect();
            let behind = behind.get(task.id.as_str()).cloned().unwrap_or_default();
            if behind.is_empty() && waiting.is_empty() {
                continue;
            }
            stale.insert(&task.id);
            steps.push(Step::Stale { id: task.id.clone(), behind, waiting });
            grew = true;
        }
        if !grew {
            return steps;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn task(id: &str, ready: Option<&str>, bases: &[&str]) -> Task {
        Task { id: id.into(), ready: ready.map(|head| Mark { head: head.into(), ..Default::default() }), bases: bases.iter().map(|b| Base { id: (*b).into(), ..Default::default() }).collect(), stale: vec![] }
    }

    fn seen(id: &str, head: &str, bases: &[(&str, bool, bool)]) -> Seen {
        Seen { id: id.into(), head: Some(head.into()), main: Some("main".into()), bases: bases.iter().map(|(b, merged, contained)| SeenBase { id: (*b).into(), head: String::new(), merged: *merged, contained: *contained }).collect() }
    }

    #[test]
    fn a_ready_task_whose_head_moves_is_unready_and_everything_stacked_on_it_goes_stale_down_the_chain() {
        let tasks = [task("a", Some("a1"), &[]), task("b", Some("b1"), &["a"]), task("c", None, &["b"])];
        let still = [seen("a", "a1", &[]), seen("b", "b1", &[("a", false, true)]), seen("c", "c1", &[("b", false, true)])];
        assert!(settle(&tasks, &still).is_empty(), "nothing moved");
        let moved = [seen("a", "a2", &[]), seen("b", "b1", &[("a", false, false)]), seen("c", "c1", &[("b", false, true)])];
        assert_eq!(
            settle(&tasks, &moved),
            vec![Step::Unready { id: "a".into() }, Step::Stale { id: "b".into(), behind: vec!["a".into()], waiting: vec![] }, Step::Stale { id: "c".into(), behind: vec![], waiting: vec!["b".into()] }]
        );
    }

    #[test]
    fn a_stale_task_stays_stale_until_it_is_marked_ready_and_only_then_its_dependents_wait_on_it_no_more() {
        let mut tasks = [task("a", None, &[]), task("b", None, &["a"]), task("c", None, &["b"])];
        tasks[1].stale = vec!["a".into()];
        tasks[2].stale = vec!["b".into()];
        let caught_up = [seen("a", "a2", &[]), seen("b", "b2", &[("a", false, true)]), seen("c", "c1", &[("b", false, false)])];
        assert!(settle(&tasks, &caught_up).is_empty(), "staleness is cleared by ready, not by git");
        tasks[1] = task("b", Some("b2"), &["a"]);
        assert!(settle(&tasks, &caught_up).is_empty(), "c is stale already");
        tasks[2].stale.clear();
        assert_eq!(settle(&tasks, &caught_up), vec![Step::Stale { id: "c".into(), behind: vec!["b".into()], waiting: vec![] }], "c is behind b's new head");
    }

    #[test]
    fn a_base_in_main_is_dropped_and_holds_nothing_back() {
        let mut tasks = [task("a", None, &[]), task("b", None, &["a"])];
        tasks[0].stale = vec!["x".into()];
        let merged = [seen("b", "b1", &[("a", true, true)])];
        assert_eq!(settle(&tasks, &merged), vec![Step::Merged { id: "b".into(), base: "a".into(), main: Some("main".into()) }]);
    }

    fn sh(dir: &Path, script: &str) {
        let out = Command::new("/bin/sh").arg("-c").arg(script).current_dir(dir).env("GIT_AUTHOR_NAME", "e").env("GIT_AUTHOR_EMAIL", "e@e").env("GIT_COMMITTER_NAME", "e").env("GIT_COMMITTER_EMAIL", "e@e").output().unwrap();
        assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn rev(dir: &Path, what: &str) -> String {
        git(dir, &["rev-parse", what]).unwrap()
    }

    #[test]
    fn a_stacked_worktree_starts_from_its_one_base_or_from_main_with_several_merged_in_and_a_conflict_leaves_nothing() {
        let dir = std::env::temp_dir().join(format!("kanna-stacks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        sh(&dir, "git init -q -b main && git commit -q --allow-empty -m first && git config user.name e && git config user.email e@e");
        sh(&dir, "git checkout -q -b a && echo a >a && git add a && git commit -q -m a && git checkout -q -b b main && echo b >b && git add b && git commit -q -m b && git checkout -q -b x main && echo x >a && git add a && git commit -q -m x && git checkout -q main");
        let base = |id: &str| Base { id: format!("task-{id}"), branch: id.into(), head: rev(&dir, id) };
        let Made::Worktree { folder, bases, .. } = make(&dir, "task-00000001", vec![base("a")]).unwrap() else { panic!("one base") };
        assert_eq!(rev(Path::new(&folder), "HEAD"), rev(&dir, "a"));
        assert_eq!(bases, vec![base("a")]);
        let Made::Worktree { folder, bases, .. } = make(&dir, "task-00000002", vec![base("a"), base("b")]).unwrap() else { panic!("two bases") };
        assert!(contains(&dir, &rev(&dir, "a"), &rev(Path::new(&folder), "HEAD")) && contains(&dir, &rev(&dir, "b"), &rev(Path::new(&folder), "HEAD")));
        assert_eq!(bases.len(), 2);
        let Made::Conflict(why) = make(&dir, "task-00000003", vec![base("a"), base("x")]).unwrap() else { panic!("a conflict") };
        assert!(why.contains("task-x's branch x conflicts"), "{why}");
        assert!(!dir.join(".kanna-worktrees/task-00000003").exists() && git(&dir, &["rev-parse", "--verify", "--quiet", "refs/heads/task-00000003"]).is_err(), "a conflict leaves no worktree or branch");
        sh(&dir, "git merge -q --no-edit a");
        let Made::Worktree { bases, .. } = make(&dir, "task-00000004", vec![base("a")]).unwrap() else { panic!("in main") };
        assert!(bases.is_empty(), "a base in main is no base");
        let probe = Probe { id: "t".into(), repo: dir.clone(), at: Where::Folder(PathBuf::from(&folder)), bases: vec![(base("a"), Where::Branch("a".into())), (base("x"), Where::Branch("x".into()))] };
        let seen = look(vec![probe]);
        assert_eq!(seen[0].bases.iter().map(|b| (b.merged, b.contained)).collect::<Vec<_>>(), vec![(true, true), (false, false)]);
        assert_eq!(seen[0].main.as_deref(), Some("main"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
