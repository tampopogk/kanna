use crate::db::Db;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;

#[derive(Debug)]
pub(super) struct MergeConflict {
    pub(super) branch: String,
    pub(super) message: String,
}

#[derive(Debug)]
pub(super) enum MergeBranchesError {
    Conflict(MergeConflict),
    Other(String),
}

pub(super) fn remove_prepared_worktree(worktree_path: &str, branch: &str) -> Result<(), String> {
    let worktree = Path::new(worktree_path);
    let repo_path = worktree
        .parent()
        .and_then(|parent| {
            if parent.file_name().and_then(|name| name.to_str()) == Some(".kanna-worktrees") {
                parent.parent()
            } else {
                None
            }
        })
        .ok_or_else(|| format!("cannot derive repo path from worktree path: {worktree_path}"))?;

    let remove_output = Command::new("git")
        .args(["worktree", "remove", "--force", worktree_path])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("failed to run git worktree remove: {}", e))?;
    if !remove_output.status.success() {
        let fallback_result = std::fs::remove_dir_all(worktree_path);
        if let Err(err) = fallback_result {
            return Err(format!(
                "failed to remove worktree: {}; fallback remove_dir_all failed: {}",
                String::from_utf8_lossy(&remove_output.stderr).trim(),
                err
            ));
        }
    }

    let delete_output = Command::new("git")
        .args(["branch", "-D", branch])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("failed to run git branch delete: {}", e))?;
    if !delete_output.status.success() {
        let message = String::from_utf8_lossy(&delete_output.stderr);
        if !message.contains("not found") && !message.contains("not a branch") {
            return Err(format!("failed to delete task branch: {}", message.trim()));
        }
    }

    Ok(())
}
pub(crate) fn resolve_current_source_worktree_branch(
    repo_path: &str,
    stored_branch: Option<&str>,
) -> Option<String> {
    let stored_branch = stored_branch?;
    let worktree_path = Path::new(repo_path)
        .join(".kanna-worktrees")
        .join(stored_branch);
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(&worktree_path)
        .output();

    let Ok(output) = output else {
        return Some(stored_branch.to_string());
    };
    if !output.status.success() {
        return Some(stored_branch.to_string());
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        Some(stored_branch.to_string())
    } else {
        Some(branch)
    }
}

/// Whether `branch` exists as a local ref in `repo_path`.
///
/// A task's `pipeline_item.branch` is written at creation, but the branch
/// itself only comes into being when the task's first workspace is forked —
/// so a task that never started names a branch that git has never heard of.
/// Anything that hands such a name to git (a worktree start point, a merge)
/// has to check first.
pub(crate) fn local_branch_exists(repo_path: &str, branch: &str) -> bool {
    Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", branch),
        ])
        .current_dir(repo_path)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// The workspace number a task branch or worktree directory name carries:
/// `task-<id>` is the creation workspace (1), `task-<id>-<n>` is `n`.
/// Anything else (a renamed PR branch, another task's name) carries none.
pub(crate) fn task_branch_number(task_id: &str, name: &str) -> Option<i64> {
    let prefix = format!("task-{task_id}");
    let rest = name.strip_prefix(&prefix)?;
    if rest.is_empty() {
        return Some(1);
    }
    let digits = rest.strip_prefix('-')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The highest workspace number anything already names for this task: local
/// refs, directories under `.kanna-worktrees`, and every branch or workspace
/// the task's own records mention. At least 1, the creation workspace.
fn used_task_branch_floor(db: &Db, repo_path: &str, task_id: &str) -> Result<i64, String> {
    let mut floor = 1;
    let mut consider = |name: &str| {
        if let Some(number) = task_branch_number(task_id, name) {
            floor = floor.max(number);
        }
    };
    let output = Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            &format!("refs/heads/task-{task_id}"),
            &format!("refs/heads/task-{task_id}-*"),
        ])
        .current_dir(repo_path)
        .output()
        .map_err(|error| format!("failed to run git for-each-ref: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to list task branches: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    for name in String::from_utf8_lossy(&output.stdout).lines() {
        consider(name.trim());
    }
    if let Ok(entries) = std::fs::read_dir(Path::new(repo_path).join(".kanna-worktrees")) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                consider(name);
            }
        }
    }
    for name in db
        .task_recorded_branch_names(task_id)
        .map_err(|error| format!("db error: {error}"))?
    {
        consider(&name);
    }
    Ok(floor)
}

/// Reserve the task's next workspace branch, `task-<id>-<n>` (spec §6).
///
/// `n` comes from the task's persisted high-water counter and is durable
/// before this returns, so it is spent before any git work: a failed
/// worktree add, a rolled-back spawn, or a branch deleted later never makes
/// the number available again. The counter is seeded above every suffix a
/// ref, a directory, or the task's records already use, so tasks that forked
/// before the counter existed continue above their highest workspace.
pub(super) fn allocate_task_branch(
    db: &Db,
    repo_path: &str,
    task_id: &str,
) -> Result<String, String> {
    let floor = used_task_branch_floor(db, repo_path, task_id)?;
    let number = db
        .reserve_task_branch_number(task_id, floor)
        .map_err(|error| format!("db error: {error}"))?;
    Ok(format!("task-{task_id}-{number}"))
}

/// A retained workspace's git state, read without changing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkspaceGitState {
    pub(super) head: String,
    /// The branch checked out, or `None` for a detached HEAD.
    pub(super) branch: Option<String>,
    /// Uncommitted changes, including untracked files.
    pub(super) dirty: bool,
}

pub(super) fn workspace_git_state(worktree_path: &str) -> Result<WorkspaceGitState, String> {
    let git = |args: &[&str]| -> Result<std::process::Output, String> {
        Command::new("git")
            .args(args)
            .current_dir(worktree_path)
            .output()
            .map_err(|error| format!("failed to run git {}: {error}", args.join(" ")))
    };
    let head = git(&["rev-parse", "--verify", "-q", "HEAD^{commit}"])?;
    if !head.status.success() {
        return Err(format!("{worktree_path} has no committed HEAD"));
    }
    let branch = git(&["symbolic-ref", "--short", "-q", "HEAD"])?;
    let status = git(&["status", "--porcelain", "--untracked-files=normal"])?;
    if !status.status.success() {
        return Err(format!(
            "git status failed in {worktree_path}: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        ));
    }
    let branch = String::from_utf8_lossy(&branch.stdout).trim().to_string();
    Ok(WorkspaceGitState {
        head: String::from_utf8_lossy(&head.stdout).trim().to_string(),
        branch: (!branch.is_empty()).then_some(branch),
        dirty: !status.stdout.is_empty(),
    })
}

/// The full commit id `revision` names in `repo_path`, if it names a commit.
pub(super) fn resolve_commit(repo_path: &str, revision: &str) -> Option<String> {
    let output = Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "-q",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!commit.is_empty()).then_some(commit)
}

/// Whether `ancestor` is reachable from `descendant` (true when equal).
pub(super) fn is_ancestor(repo_path: &str, ancestor: &str, descendant: &str) -> bool {
    if ancestor == descendant {
        return true;
    }
    Command::new("git")
        .args([
            "merge-base",
            "--is-ancestor",
            "--end-of-options",
            ancestor,
            descendant,
        ])
        .current_dir(repo_path)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Check out a newly allocated branch at `start_point` in an existing
/// workspace. The caller has established that this moves nothing it must
/// not: the workspace is either already at `start_point` (uncommitted
/// changes stay where they are) or clean and behind it.
pub(super) fn check_out_new_branch(
    worktree_path: &str,
    branch: &str,
    start_point: &str,
) -> Result<(), String> {
    let output = Command::new("git")
        .args(["switch", "--no-track", "-c", branch, start_point])
        .current_dir(worktree_path)
        .output()
        .map_err(|error| format!("failed to run git switch: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to check out {branch} in {worktree_path}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Confirm a retained workspace is still in the state a revisit plan
/// observed, immediately before its branch is switched. Anything that moved
/// in between — a commit, a branch change, new local changes — means the
/// plan's reasons no longer hold; the difference is returned so the caller
/// can preserve the directory and report it.
pub(super) fn revalidate_revisit(
    worktree_path: &str,
    previous_branch: Option<&str>,
    previous_head: &str,
    observed_dirty: bool,
) -> Result<(), String> {
    let state = workspace_git_state(worktree_path)?;
    let mut changes = Vec::new();
    if state.head != previous_head {
        changes.push(format!(
            "HEAD moved from {} to {}",
            short(previous_head),
            short(&state.head)
        ));
    }
    if state.branch.as_deref() != previous_branch {
        changes.push(format!(
            "branch changed from {} to {}",
            previous_branch.unwrap_or("detached HEAD"),
            state.branch.as_deref().unwrap_or("detached HEAD")
        ));
    }
    if state.dirty != observed_dirty {
        changes.push(if state.dirty {
            "it gained uncommitted changes".to_string()
        } else {
            "its uncommitted changes went away".to_string()
        });
    }
    if changes.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "retained workspace {worktree_path} changed after it was planned for reuse ({})",
            changes.join("; ")
        ))
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// The revisit a failed spawn has to undo: the branch it checked out, where
/// it started, and what the directory had before.
pub(super) struct RevisitCheckout<'a> {
    pub(super) worktree_path: &'a str,
    pub(super) new_branch: &'a str,
    pub(super) start_point: &'a str,
    pub(super) previous_branch: Option<&'a str>,
    pub(super) previous_head: &'a str,
    pub(super) observed_dirty: bool,
}

/// Undo [`check_out_new_branch`] after a failed spawn: return the workspace
/// to what it had checked out and delete the unused branch. The directory
/// itself is retained; its number stays spent.
///
/// A shell, editor or outgoing agent may have used the directory between the
/// checkout and the failure. The undo runs only while the directory is
/// exactly as the checkout left it — the new branch checked out, still at
/// its start point, with no local changes the plan did not already see.
/// Otherwise nothing is touched and `Ok(Some(report))` says what was kept.
pub(super) fn restore_revisited_workspace(
    checkout: &RevisitCheckout<'_>,
) -> Result<Option<String>, String> {
    let RevisitCheckout {
        worktree_path,
        new_branch,
        start_point,
        previous_branch,
        previous_head,
        observed_dirty,
    } = *checkout;
    let state = workspace_git_state(worktree_path)?;
    let mut changes = Vec::new();
    if state.branch.as_deref() != Some(new_branch) {
        changes.push(format!(
            "{} is checked out instead of {new_branch}",
            state.branch.as_deref().unwrap_or("detached HEAD")
        ));
    } else if state.head != start_point {
        changes.push(format!(
            "{new_branch} moved from {} to {}",
            short(start_point),
            short(&state.head)
        ));
    }
    if state.dirty && !observed_dirty {
        changes.push("it has uncommitted changes made after the checkout".to_string());
    }
    if !changes.is_empty() {
        return Ok(Some(format!(
            "retained workspace {worktree_path} was used after {new_branch} was checked out              ({}); the branch and directory were preserved untouched",
            changes.join("; ")
        )));
    }
    let mut args = vec!["switch"];
    match previous_branch {
        Some(branch) => args.push(branch),
        None => {
            args.push("--detach");
            args.push(previous_head);
        }
    }
    let output = Command::new("git")
        .args(&args)
        .current_dir(worktree_path)
        .output()
        .map_err(|error| format!("failed to run git switch: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to restore {worktree_path}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    // Verified above to still sit at the start point this revisit chose,
    // which another ref already holds, so deleting it drops no commit.
    let output = Command::new("git")
        .args(["branch", "-D", new_branch])
        .current_dir(worktree_path)
        .output()
        .map_err(|error| format!("failed to run git branch delete: {error}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        if !message.contains("not found") {
            return Err(format!(
                "failed to delete unused branch {new_branch}: {}",
                message.trim()
            ));
        }
    }
    Ok(None)
}

pub(super) fn generate_task_id() -> Result<String, String> {
    let mut bytes = [0u8; 4];
    File::open("/dev/urandom")
        .map_err(|e| format!("failed to open /dev/urandom: {}", e))?
        .read_exact(&mut bytes)
        .map_err(|e| format!("failed to read random bytes: {}", e))?;
    Ok(bytes.iter().map(|byte| format!("{:02x}", byte)).collect())
}

/// Random UUIDv4 for a fresh agent-CLI session (`claude --session-id`
/// requires a valid UUID).
pub(super) fn generate_agent_session_uuid() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .map_err(|e| format!("failed to open /dev/urandom: {}", e))?
        .read_exact(&mut bytes)
        .map_err(|e| format!("failed to read random bytes: {}", e))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: Vec<String> = bytes.iter().map(|byte| format!("{:02x}", byte)).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        hex[0..4].join(""),
        hex[4..6].join(""),
        hex[6..8].join(""),
        hex[8..10].join(""),
        hex[10..16].join(""),
    ))
}

pub(super) fn fetch_start_point(
    repo_path: &str,
    default_branch: Option<&str>,
) -> Result<String, String> {
    let branch = default_branch.unwrap_or("main");
    let remote_tracking_ref = format!("refs/remotes/origin/{branch}");
    let fetch_refspec = format!("+refs/heads/{branch}:{remote_tracking_ref}");
    let fetch_output = Command::new("git")
        .args(["fetch", "--no-tags", "--", "origin", &fetch_refspec])
        .current_dir(repo_path)
        .output();

    // The fetch above explicitly refreshes the remote-tracking ref. It may
    // fail when the repo is offline or has no origin; an already-fetched
    // remote ref is still a better base than a stale local branch.
    if let Some(resolved) = crate::git_refs::resolve_base_ref(Path::new(repo_path), branch) {
        return Ok(resolved.reference);
    }

    let fetch_detail = match fetch_output {
        Ok(output) if output.status.success() => String::new(),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                String::new()
            } else {
                format!("; fetch failed: {stderr}")
            }
        }
        Err(error) => format!("; fetch failed to start: {error}"),
    };
    Err(format!(
        "cannot resolve default branch `{branch}`: neither `origin/{branch}` nor local `{branch}` points to a commit{fetch_detail}"
    ))
}

pub(super) fn create_worktree(
    repo_path: &str,
    branch: &str,
    worktree_path: &str,
    start_point: Option<&str>,
) -> Result<(), String> {
    let branch_exists = local_branch_exists(repo_path, branch);

    let mut args = vec!["worktree", "add"];
    if branch_exists {
        args.push(worktree_path);
        args.push(branch);
    } else {
        args.push("-b");
        args.push(branch);
        args.push(worktree_path);
        let start_point = start_point.ok_or_else(|| {
            format!("refusing to create new branch `{branch}` without an explicit start point")
        })?;
        args.push(start_point);
    }

    // The main checkout is a shared resource: the desktop frontend polls git
    // status/diff against it constantly, and agents push from sibling
    // worktrees. `git worktree add` can transiently lose a race for the
    // repo's lock files, so retry briefly before giving up — a one-shot
    // failure here used to leave unblocked dependent tasks permanently
    // dormant.
    let mut last_error = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(300));
            log::warn!(
                "retrying git worktree add for {branch} after lock contention: {last_error}"
            );
        }
        let output = Command::new("git")
            .args(&args)
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("failed to run git worktree add: {}", e))?;
        if output.status.success() {
            last_error.clear();
            break;
        }
        last_error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !last_error.contains(".lock") {
            return Err(last_error);
        }
    }
    if !last_error.is_empty() {
        return Err(last_error);
    }

    // Worktrees contain exactly what the branch checkout contains. Repo-
    // specific scaffolding (like a Rust `.cargo/config.toml`) must come from
    // the repo itself — committed, or created by `.kanna/config.json` setup
    // commands. Kanna used to inject a `.cargo/config.toml` here for its own
    // build layout; the stray untracked file made every commit post in other
    // repos report a dirty worktree.
    Ok(())
}

pub(super) fn merge_branches_into_worktree(
    worktree_path: &str,
    branches: &[String],
) -> Result<(), MergeBranchesError> {
    for branch in branches {
        let output = Command::new("git")
            .args(["merge", "--no-edit", branch])
            .current_dir(worktree_path)
            .output()
            .map_err(|e| {
                MergeBranchesError::Other(format!("failed to run git merge {branch}: {e}"))
            })?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let details = format!("{}{}", stdout, stderr).trim().to_string();
            let message = format!("failed to merge blocker branch {branch}: {}", details);
            if details.contains("CONFLICT")
                || details.contains("Automatic merge failed")
                || details.contains("fix conflicts")
            {
                let abort_output = Command::new("git")
                    .args(["merge", "--abort"])
                    .current_dir(worktree_path)
                    .output();
                if let Ok(abort_output) = abort_output {
                    if !abort_output.status.success() {
                        log::warn!(
                            "failed to abort conflicted blocker merge for {branch}: {}",
                            String::from_utf8_lossy(&abort_output.stderr).trim()
                        );
                    }
                }
                return Err(MergeBranchesError::Conflict(MergeConflict {
                    branch: branch.clone(),
                    message,
                }));
            }
            return Err(MergeBranchesError::Other(message));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::fetch_start_point;
    use std::path::Path;
    use std::process::Command;

    fn run_git(repo_path: Option<&Path>, args: &[&str]) {
        let mut command = Command::new("git");
        if let Some(repo_path) = repo_path {
            command.arg("-C").arg(repo_path);
        }
        let output = command.args(args).output().expect("git should launch");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn fetch_start_point_treats_option_shaped_branches_as_revisions() {
        let temp = tempfile::tempdir().expect("temp dir");
        let remote = temp.path().join("remote.git");
        let seed = temp.path().join("seed");
        let local = temp.path().join("local");
        let remote_path = remote.to_string_lossy();

        run_git(None, &["init", "--bare", &remote_path]);
        run_git(None, &["init", seed.to_string_lossy().as_ref()]);
        run_git(
            Some(&seed),
            &[
                "-c",
                "user.name=Kanna Test",
                "-c",
                "user.email=kanna-test@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "seed",
            ],
        );

        let remote_branch = "--upload-pack=kanna-missing-upload-pack";
        let remote_refspec = format!("HEAD:refs/heads/{remote_branch}");
        run_git(Some(&seed), &["push", "--", &remote_path, &remote_refspec]);

        run_git(None, &["init", local.to_string_lossy().as_ref()]);
        run_git(Some(&local), &["remote", "add", "origin", &remote_path]);
        assert_eq!(
            fetch_start_point(local.to_string_lossy().as_ref(), Some(remote_branch)),
            Ok(format!("origin/{remote_branch}"))
        );

        let local_branch = "--local-option-shaped-branch";
        let local_ref = format!("refs/heads/{local_branch}");
        run_git(Some(&local), &["update-ref", &local_ref, "FETCH_HEAD"]);
        assert_eq!(
            fetch_start_point(local.to_string_lossy().as_ref(), Some(local_branch)),
            Ok(local_branch.to_string())
        );
    }
}
