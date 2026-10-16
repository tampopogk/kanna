use crate::backlog::git;
use std::fs;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const TIMEOUT: u64 = 600;

pub const TIMEOUT_MAX: u64 = 86_400;

const TIMED_OUT: i32 = 142;

const BASE: &str = "kanna-base";

const RECORD: &str = "kanna-setup";

#[derive(Clone, Debug, PartialEq)]
pub struct Scripts {
    pub setup: Option<String>,
    pub teardown: Option<String>,
    pub timeout: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: String,
    pub name: Option<String>,
    pub repo: PathBuf,
    pub worktree: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Setup {
    Done,
    Failed(i32),
    Skipped(i32),
}

impl Setup {
    pub fn said(code: i32, timeout: u64) -> String {
        match code {
            TIMED_OUT => format!("setup timed out after {timeout}s"),
            code => format!("setup failed (exit {code})"),
        }
    }
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

pub fn git_dir(worktree: &Path) -> Option<PathBuf> {
    git(worktree, &["rev-parse", "--absolute-git-dir"]).ok().map(PathBuf::from)
}

pub fn is_linked(worktree: &Path) -> bool {
    let common = git(worktree, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).ok().and_then(|d| fs::canonicalize(d).ok());
    let own = git_dir(worktree).and_then(|d| fs::canonicalize(d).ok());
    common.is_some() && own.is_some() && common != own
}

pub fn record_base(worktree: &Path) {
    let Some(dir) = git_dir(worktree) else { return };
    let base = git(worktree, &["rev-parse", "--verify", "--quiet", "HEAD"]).unwrap_or_default();
    let _ = fs::write(dir.join(BASE), format!("{base}\n"));
}

pub fn base(worktree: &Path) -> Option<String> {
    fs::read_to_string(git_dir(worktree)?.join(BASE)).ok().map(|b| b.trim().to_string())
}

pub fn setup_record(worktree: &Path) -> Option<PathBuf> {
    git_dir(worktree).map(|dir| dir.join(RECORD))
}

pub fn setup(worktree: &Path) -> Option<Setup> {
    read_setup(&setup_record(worktree)?)
}

pub fn read_setup(record: &Path) -> Option<Setup> {
    let text = fs::read_to_string(record).ok()?;
    let mut words = text.split_whitespace();
    let code: i32 = words.next()?.parse().ok()?;
    Some(match (code, words.next()) {
        (0, _) => Setup::Done,
        (code, Some("started")) => Setup::Skipped(code),
        (code, _) => Setup::Failed(code),
    })
}

pub fn started_anyway(worktree: &Path, code: i32) {
    if let Some(record) = setup_record(worktree) {
        let _ = fs::write(record, format!("{code} started\n"));
    }
}

pub fn forget_setup(worktree: &Path) {
    if let Some(record) = setup_record(worktree) {
        let _ = fs::remove_file(record);
    }
    if base(worktree).is_none() {
        record_base(worktree);
    }
}

pub fn needs_setup(worktree: &Path) -> bool {
    base(worktree).is_some() && setup(worktree).is_none()
}

impl Task {
    pub fn vars(&self, branch: &str) -> Vec<(&'static str, String)> {
        let text = |path: &Path| path.to_string_lossy().to_string();
        vec![
            ("KANNA_TASK", self.id.clone()),
            ("KANNA_TASK_NAME", self.name.clone().unwrap_or_default()),
            ("KANNA_REPO", text(&self.repo)),
            ("KANNA_WORKTREE", text(&self.worktree)),
            ("KANNA_BRANCH", branch.to_string()),
            ("KANNA_BASE", base(&self.worktree).unwrap_or_default()),
        ]
    }

    pub fn branch(&self) -> String {
        git(&self.worktree, &["branch", "--show-current"]).unwrap_or_default()
    }

    pub fn setup_command(&self, setup: &str, timeout: u64, then: Option<&str>) -> Option<String> {
        let record = setup_record(&self.worktree)?;
        let vars: Vec<String> = self.vars(&self.branch()).into_iter().map(|(k, v)| format!("{k}={}", quote(&v))).collect();
        let failed = format!("if [ \"$kanna_setup\" -eq {TIMED_OUT} ]; then printf '\\nsetup timed out after {timeout}s\\n'; else printf '\\nsetup failed (exit %s)\\n' \"$kanna_setup\"; fi; exit \"$kanna_setup\"");
        let mut command = format!(
            "printf 'Setting up: %s\\n\\n' {shown}; {vars} perl -e 'alarm shift; exec @ARGV' {timeout} \"${{SHELL:-/bin/zsh}}\" -lc {setup}; kanna_setup=$?; printf '%s\\n' \"$kanna_setup\" > {record}; if [ \"$kanna_setup\" -ne 0 ]; then {failed}; fi",
            shown = quote(setup),
            vars = vars.join(" "),
            setup = quote(setup),
            record = quote(&record.to_string_lossy()),
        );
        if let Some(then) = then {
            command = format!("{command}; printf '\\n'; {then}");
        }
        Some(command)
    }
}

pub fn run(command: &str, cwd: &Path, vars: &[(&str, String)], timeout: Duration) -> Result<i32, String> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    let mut cmd = Command::new(shell);
    cmd.args(["-lc", command]).current_dir(cwd).stdin(Stdio::null()).process_group(0);
    for (key, value) in vars {
        cmd.env(key, value);
    }
    unsafe {
        cmd.pre_exec(|| {
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            libc::pthread_sigmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("can't run it: {e}"))?;
    let by = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok(status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0)));
        }
        if Instant::now() >= by {
            let _ = Command::new("kill").args(["-KILL", &format!("-{}", child.id())]).status();
            let _ = child.wait();
            return Ok(TIMED_OUT);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cleanup {
    Removed { branch: String, target: Option<PathBuf> },
    Kept(String),
}

fn checkouts(repo: &Path) -> Vec<PathBuf> {
    let listed = git(repo, &["worktree", "list", "--porcelain"]).unwrap_or_default();
    listed.lines().filter_map(|l| l.strip_prefix("worktree ")).filter_map(|p| fs::canonicalize(p).ok()).collect()
}

fn link_target(worktree: &Path) -> Option<PathBuf> {
    let link = worktree.join("target");
    if !fs::symlink_metadata(&link).ok()?.file_type().is_symlink() {
        return None;
    }
    let to = fs::read_link(&link).ok()?;
    let to = if to.is_absolute() { to } else { worktree.join(to) };
    fs::canonicalize(to).ok().filter(|t| t.is_dir())
}

fn removable(target: &Path, worktree: &Path, others: &[PathBuf]) -> bool {
    let home = fs::canonicalize(crate::agents::home()).ok();
    let mine = fs::canonicalize(worktree).ok();
    let near = |p: &Path| target.starts_with(p) || p.starts_with(target);
    target.components().count() > 2 && !home.as_deref().is_some_and(|h| h.starts_with(target)) && !others.iter().filter(|o| Some(o.as_path()) != mine.as_deref()).any(|o| near(o))
}

pub fn clean_up(repo: &Path, worktree: &Path) -> Cleanup {
    let kept = |why: String| Cleanup::Kept(why);
    if !worktree.is_dir() {
        return kept("its worktree is gone".into());
    }
    if !is_linked(worktree) {
        return kept(format!("{} isn't a task's worktree", worktree.display()));
    }
    let branch = git(worktree, &["branch", "--show-current"]).unwrap_or_default();
    if branch.is_empty() {
        return kept("it isn't on a branch".into());
    }
    match git(worktree, &["status", "--porcelain"]) {
        Ok(changes) if changes.is_empty() => {}
        Ok(_) => return kept("it has changes".into()),
        Err(error) => return kept(format!("git status failed: {error}")),
    }
    let head = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|_| "HEAD".into());
    if git(repo, &["merge-base", "--is-ancestor", &format!("refs/heads/{branch}"), "HEAD"]).is_err() {
        return kept(format!("{branch} isn't merged into {head}"));
    }
    let others = checkouts(repo);
    let target = link_target(worktree).filter(|t| removable(t, worktree, &others));
    if let Err(error) = git(repo, &["worktree", "remove", &worktree.to_string_lossy()]) {
        return kept(format!("git worktree remove failed: {error}"));
    }
    if let Some(target) = &target {
        if let Err(error) = fs::remove_dir_all(target) {
            eprintln!("kanna-daemon: couldn't remove {}, {}'s build folder: {error}", target.display(), worktree.display());
        }
    }
    if let Err(error) = git(repo, &["branch", "-d", &branch]) {
        return kept(format!("removed the worktree, but git branch -d {branch} failed: {error}"));
    }
    Cleanup::Removed { branch, target }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn sh(dir: &Path, script: &str) {
        let ok = Command::new("sh").arg("-c").arg(script).current_dir(dir).status().unwrap().success();
        assert!(ok, "{script}");
    }

    fn repo(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("kanna-scripts-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let repo = fs::canonicalize(&repo).unwrap();
        sh(&repo, "git init -q -b main && printf 'target\\n' > .gitignore && git add .gitignore && git -c user.email=e@e -c user.name=e commit -q -m first");
        let (folder, _) = crate::backlog::make_worktree(&repo, "task-1dfb12c2").unwrap();
        (fs::canonicalize(dir).unwrap(), PathBuf::from(folder))
    }

    #[test]
    fn a_merged_worktree_goes_with_the_folder_its_target_links_to_but_nothing_that_folder_links_to() {
        let (dir, worktree) = repo("linked");
        let repo = dir.join("repo");
        let main_build = repo.join("target/ghosttykit");
        fs::create_dir_all(main_build.join("lib")).unwrap();
        fs::write(main_build.join("lib/libghostty.a"), "main's").unwrap();
        fs::create_dir_all(repo.join("target/.offline")).unwrap();
        let volume = dir.join("volume/kanna-targets/repo").join(worktree.file_name().unwrap());
        fs::create_dir_all(volume.join("debug")).unwrap();
        fs::write(volume.join("debug/kanna"), "built").unwrap();
        symlink(&main_build, volume.join("ghosttykit")).unwrap();
        symlink(&volume, worktree.join("target")).unwrap();
        let branch = git(&worktree, &["branch", "--show-current"]).unwrap();
        match clean_up(&repo, &worktree) {
            Cleanup::Removed { branch: gone, target } => {
                assert_eq!(gone, branch);
                assert_eq!(target, Some(fs::canonicalize(dir.join("volume/kanna-targets/repo")).unwrap().join(worktree.file_name().unwrap())));
            }
            kept => panic!("{kept:?}"),
        }
        assert!(!worktree.exists(), "the worktree is removed");
        assert!(!volume.exists(), "the folder its target links to is removed");
        assert!(dir.join("volume/kanna-targets/repo").is_dir(), "only that folder");
        assert_eq!(fs::read_to_string(main_build.join("lib/libghostty.a")).unwrap(), "main's", "a link inside it isn't followed");
        assert!(repo.join("target/.offline").is_dir());
        assert!(git(&repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_err(), "the branch is deleted");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_target_linking_into_the_main_checkout_goes_as_a_link_only() {
        let (dir, worktree) = repo("borrowed");
        let repo = dir.join("repo");
        fs::create_dir_all(repo.join("target/.offline")).unwrap();
        symlink(repo.join("target"), worktree.join("target")).unwrap();
        assert!(matches!(clean_up(&repo, &worktree), Cleanup::Removed { target: None, .. }));
        assert!(!worktree.exists());
        assert!(repo.join("target/.offline").is_dir(), "the main checkout's target stays");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unmerged_or_changed_worktree_is_kept_and_says_why() {
        let (dir, worktree) = repo("kept");
        let repo = dir.join("repo");
        fs::write(worktree.join("notes.txt"), "work").unwrap();
        assert_eq!(clean_up(&repo, &worktree), Cleanup::Kept("it has changes".into()));
        sh(&worktree, "git add notes.txt && git -c user.email=e@e -c user.name=e commit -q -m work");
        let branch = git(&worktree, &["branch", "--show-current"]).unwrap();
        assert_eq!(clean_up(&repo, &worktree), Cleanup::Kept(format!("{branch} isn't merged into main")));
        assert!(worktree.join("notes.txt").is_file());
        sh(&repo, &format!("git merge -q --ff-only {branch}"));
        assert!(matches!(clean_up(&repo, &worktree), Cleanup::Removed { .. }));
        assert_eq!(clean_up(&repo, &worktree), Cleanup::Kept("its worktree is gone".into()));
        assert_eq!(clean_up(&repo, &repo), Cleanup::Kept(format!("{} isn't a task's worktree", repo.display())), "the main checkout is never removed");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn setup_runs_with_the_tasks_details_and_records_how_it_went() {
        let (dir, worktree) = repo("setup");
        let task = Task { id: "t7".into(), name: Some("Seven's".into()), repo: dir.join("repo"), worktree: worktree.clone() };
        assert!(needs_setup(&worktree), "a new worktree is set up once");
        let command = task.setup_command("env | grep ^KANNA_ | sort > seen", 5, Some("echo agent")).unwrap();
        let out = Command::new("sh").arg("-c").arg(&command).current_dir(&worktree).env("SHELL", "/bin/sh").output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).ends_with("agent\n"), "{out:?}");
        let seen = fs::read_to_string(worktree.join("seen")).unwrap();
        assert!(seen.contains("KANNA_TASK=t7\n") && seen.contains("KANNA_TASK_NAME=Seven's\n"), "{seen}");
        assert!(seen.contains(&format!("KANNA_WORKTREE={}\n", worktree.display())), "{seen}");
        assert!(seen.contains(&format!("KANNA_BASE={}\n", git(&worktree, &["rev-parse", "HEAD"]).unwrap())), "{seen}");
        assert_eq!(setup(&worktree), Some(Setup::Done));
        assert!(!needs_setup(&worktree));
        forget_setup(&worktree);
        let command = task.setup_command("exit 3", 5, Some("echo agent")).unwrap();
        let out = Command::new("sh").arg("-c").arg(&command).current_dir(&worktree).env("SHELL", "/bin/sh").output().unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&out.stdout).contains("setup failed (exit 3)"), "{out:?}");
        assert_eq!(setup(&worktree), Some(Setup::Failed(3)));
        started_anyway(&worktree, 3);
        assert_eq!(setup(&worktree), Some(Setup::Skipped(3)));
        forget_setup(&worktree);
        let command = task.setup_command("sleep 5", 1, None).unwrap();
        let out = Command::new("sh").arg("-c").arg(&command).current_dir(&worktree).env("SHELL", "/bin/sh").output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("setup timed out after 1s"), "{out:?}");
        assert_eq!(run("echo hi; exit 4", &worktree, &[], Duration::from_secs(5)), Ok(4));
        assert_eq!(run("sleep 5", &worktree, &[], Duration::from_millis(300)), Ok(TIMED_OUT));
        fs::remove_dir_all(&dir).unwrap();
    }
}
