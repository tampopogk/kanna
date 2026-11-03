use kanna_log::{commit, load, TaskRef};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn sh(dir: &Path, script: &str) -> String {
    let out = Command::new("sh").arg("-c").arg(script).current_dir(dir).env("GIT_AUTHOR_NAME", "Ann").env("GIT_AUTHOR_EMAIL", "ann@example.invalid").env("GIT_COMMITTER_NAME", "Ann").env("GIT_COMMITTER_EMAIL", "ann@example.invalid").output().unwrap();
    assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kanna-log-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let dir = fs::canonicalize(&dir).unwrap();
    sh(&dir, "git init -q -b main repo");
    dir.join("repo")
}

fn commit_file(dir: &Path, file: &str, message: &str) {
    sh(dir, &format!("echo '{message}' >> {file} && git add {file} && git commit -q -m '{message}'"));
}

fn history(repo: &Path) -> PathBuf {
    commit_file(repo, "a.txt", "first");
    sh(repo, "git checkout -q -b long");
    for n in 1..=5 {
        commit_file(repo, "long.txt", &format!("long {n}"));
    }
    sh(repo, "git checkout -q main");
    commit_file(repo, "a.txt", "main 1");
    sh(repo, "git merge -q --no-ff long -m 'Merge long'");
    for b in ["x", "y", "z"] {
        sh(repo, &format!("git checkout -q -b {b} main~1"));
        commit_file(repo, &format!("{b}.txt"), &format!("{b} 1"));
    }
    sh(repo, "git checkout -q main && git merge -q --no-ff x y z -m 'Octopus'");
    let worktree = repo.parent().unwrap().join("task-1");
    sh(repo, &format!("git worktree add -q -b feature {} main", worktree.display()));
    sh(&worktree, "git rev-parse HEAD > \"$(git rev-parse --git-dir)/kanna-base\"");
    commit_file(&worktree, "f.txt", "feature 1");
    commit_file(repo, "a.txt", "main 2");
    sh(&worktree, "git merge -q --no-ff main -m \"Merge branch 'main' into feature\"");
    commit_file(&worktree, "f.txt", "feature 2");
    commit_file(repo, "a.txt", "main 3");
    worktree
}

fn task(repo: &Path, id: &str, place: &str, worktree: Option<&Path>) -> TaskRef {
    TaskRef { id: id.into(), name: Some(format!("Task {id}")), place: place.into(), repo: repo.to_string_lossy().into(), worktree: worktree.map(|w| w.to_string_lossy().into()), ..Default::default() }
}

#[test]
fn the_main_checkout_shows_main_and_the_open_tasks_branches_labelled() {
    let repo = scratch("planner");
    let worktree = history(&repo);
    let log = load(&repo.to_string_lossy(), &[], &[task(&repo, "task-1", "desk", Some(&worktree))]);
    assert_eq!(log.error, None);
    assert_eq!(log.range, ["feature", "main"]);
    assert_eq!(log.main, "main");
    let label = &log.tasks[0];
    assert_eq!((label.name.as_str(), label.place.as_str(), label.branch.as_str(), label.ahead, label.behind, label.merged), ("Task task-1", "desk", "feature", 3, 1, false));
    assert!(log.rows.iter().any(|r| r.subject == "feature 2"));
    let spine: Vec<&str> = log.rows.iter().filter(|r| r.spine).map(|r| r.subject.as_str()).collect();
    assert_eq!(spine, ["main 3", "main 2", "Octopus", "Merge long", "main 1", "first"]);
    let merge = log.rows.iter().find(|r| r.subject == "Merge long").unwrap();
    assert_eq!(merge.fold, 5);
    assert!(log.rows.iter().filter(|r| r.subject.starts_with("long ")).all(|r| r.side.as_deref() == Some(merge.hash.as_str())));
    let octopus = log.rows.iter().find(|r| r.subject == "Octopus").unwrap();
    assert_eq!((octopus.parents.len(), octopus.fold), (4, 0));
    assert!(log.rows.iter().find(|r| r.subject == "main 3").unwrap().refs.contains(&"main".to_string()));
}

#[test]
fn a_tasks_worktree_shows_its_branch_and_main() {
    let repo = scratch("task");
    let worktree = history(&repo);
    let log = load(&worktree.to_string_lossy(), &[], &[]);
    assert_eq!(log.range, ["HEAD", "main"]);
    let head = &log.rows.iter().find(|r| r.subject == "feature 2").unwrap().refs;
    assert!(head.contains(&"HEAD".to_string()) && head.contains(&"feature".to_string()));
}

#[test]
fn a_merged_task_is_labelled_merged_and_left_out_of_the_range() {
    let repo = scratch("merged");
    let worktree = history(&repo);
    sh(&repo, "git merge -q --no-ff feature -m 'Merge feature'");
    let log = load(&repo.to_string_lossy(), &[], &[task(&repo, "task-1", "shelf", Some(&worktree))]);
    assert_eq!(log.range, ["main"]);
    assert!(log.tasks[0].merged);
    assert_eq!(log.tasks[0].behind, 2);
}

#[test]
fn a_task_with_no_commits_of_its_own_has_no_label_and_other_repos_tasks_none() {
    let repo = scratch("fresh");
    commit_file(&repo, "a.txt", "first");
    let worktree = repo.parent().unwrap().join("task-2");
    sh(&repo, &format!("git worktree add -q -b fresh {} main", worktree.display()));
    sh(&worktree, "git rev-parse HEAD > \"$(git rev-parse --git-dir)/kanna-base\"");
    let other = task(Path::new("/elsewhere"), "task-3", "desk", Some(&worktree));
    let log = load(&repo.to_string_lossy(), &[], &[task(&repo, "task-2", "desk", Some(&worktree)), other]);
    assert!(log.tasks.is_empty());
}

#[test]
fn a_done_task_is_labelled_by_its_recorded_branch_and_tip() {
    let repo = scratch("done");
    let worktree = history(&repo);
    sh(&repo, "git merge -q --no-ff feature -m 'Merge feature'");
    let tip = sh(&worktree, "git rev-parse HEAD");
    sh(&repo, &format!("git worktree remove {} && git branch -q -d feature", worktree.display()));
    let done = TaskRef { branch: Some("feature".into()), tip: Some(tip.clone()), ..task(&repo, "task-1", "done", None) };
    let log = load(&repo.to_string_lossy(), &[], &[done]);
    assert_eq!(log.range, ["main"]);
    assert_eq!((log.tasks[0].tip.as_str(), log.tasks[0].merged, log.tasks[0].place.as_str()), (tip.as_str(), true, "done"));
}

#[test]
fn revisions_and_paths_pass_through_and_a_bad_one_is_gits_error() {
    let repo = scratch("revs");
    history(&repo);
    let log = load(&repo.to_string_lossy(), &["long".into(), "--".into(), "long.txt".into()], &[]);
    assert_eq!(log.range, ["long"]);
    assert_eq!(log.rows.len(), 5);
    let bad = load(&repo.to_string_lossy(), &["no-such-branch".into()], &[]);
    assert!(bad.error.as_deref().is_some_and(|e| e.contains("no-such-branch")), "{:?}", bad.error);
    assert!(bad.rows.is_empty());
}

#[test]
fn a_commits_detail_has_its_message_and_files() {
    let repo = scratch("detail");
    commit_file(&repo, "a.txt", "first");
    sh(&repo, "printf 'one\\ntwo\\n' > b.txt && git add b.txt && git mv a.txt c.txt && git commit -q -m 'Subject line' -m 'Body text.'");
    let hash = sh(&repo, "git rev-parse HEAD");
    let detail = commit(&repo.to_string_lossy(), &hash).unwrap();
    assert_eq!(detail["message"], "Subject line\n\nBody text.");
    let files = detail["files"].as_array().unwrap();
    let b = files.iter().find(|f| f["path"] == "b.txt").unwrap();
    assert_eq!((b["status"].as_str(), b["added"].as_u64()), (Some("A"), Some(2)));
    let c = files.iter().find(|f| f["path"] == "c.txt").unwrap();
    assert_eq!((c["status"].as_str(), c["old"].as_str()), (Some("R"), Some("a.txt")));
    assert!(commit(&repo.to_string_lossy(), "HEAD; rm").is_err());
}
