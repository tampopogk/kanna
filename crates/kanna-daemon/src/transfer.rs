use crate::db::Db;
use crate::tasks::{add_worktree, git};
use kanna_mux::json::Value;
use rusqlite::{params, Connection, OptionalExtension};
use std::fs;
use std::path::{Path, PathBuf};

pub const WIP: &str = "kanna: work in progress of";

const IDENTITY: [&str; 4] = ["-c", "user.name=Kanna", "-c", "user.email=kanna@localhost"];

pub fn inbox(state: &Path) -> PathBuf {
    state.join("transfers").join("in")
}

pub fn outbox(state: &Path) -> PathBuf {
    state.join("transfers").join("out")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Saved {
    pub id: String,
    pub task: String,
    pub role: String,
    pub peer: String,
    pub state: String,
    pub doc: Value,
}

impl Saved {
    pub fn text(&self, key: &str) -> Option<&str> {
        self.doc.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
    }

    pub fn packed(&self) -> Option<Packed> {
        Some(Packed { branch: self.text("branch")?.to_string(), pre: self.text("pre")?.to_string(), wip: self.text("wip")?.to_string() })
    }
}

pub fn save(conn: &Connection, saved: &Saved) -> rusqlite::Result<()> {
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    conn.execute(
        "insert into transfers (id, task, role, peer, state, doc, at) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         on conflict (id) do update set peer = excluded.peer, state = excluded.state, doc = excluded.doc, at = excluded.at",
        params![saved.id, saved.task, saved.role, saved.peer, saved.state, saved.doc.to_string(), at],
    )?;
    Ok(())
}

fn saved_row(r: &rusqlite::Row) -> rusqlite::Result<Saved> {
    let doc: String = r.get(5)?;
    Ok(Saved { id: r.get(0)?, task: r.get(1)?, role: r.get(2)?, peer: r.get(3)?, state: r.get(4)?, doc: serde_json::from_str(&doc).unwrap_or(Value::Null) })
}

pub fn saved(db: &Db, id: &str) -> Option<Saved> {
    db.atomically(|conn| conn.query_row("select id, task, role, peer, state, doc from transfers where id = ?1", [id], saved_row).optional()).ok().flatten()
}

pub fn under_way(db: &Db, role: &str) -> Vec<Saved> {
    db.atomically(|conn| {
        let mut statement = conn.prepare("select id, task, role, peer, state, doc from transfers where role = ?1 and state not in ('done', 'failed') order by at")?;
        let rows = statement.query_map([role], saved_row)?;
        rows.collect()
    })
    .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Packed {
    pub branch: String,
    pub pre: String,
    pub wip: String,
}

pub fn pack(folder: &Path, task: &str) -> Result<Packed, String> {
    let git = |args: &[&str]| git(folder, args);
    let branch = git(&["symbolic-ref", "--quiet", "--short", "HEAD"]).map_err(|_| format!("{} isn't on a branch", folder.display()))?;
    let head = git(&["rev-parse", "--verify", "HEAD"]).map_err(|_| format!("{branch} has no commits"))?;
    let subject = git(&["log", "-1", "--format=%s", &head])?;
    if subject.starts_with(WIP) {
        git(&["reset", "--soft", "HEAD^"])?;
    }
    let pre = git(&["rev-parse", "--verify", "HEAD"])?;
    git(&["add", "-A"])?;
    let tree = git(&["write-tree"])?;
    let mut args: Vec<&str> = IDENTITY.to_vec();
    let message = format!("{WIP} {task}");
    args.extend(["commit-tree", &tree, "-p", &pre, "-m", &message]);
    let wip = git(&args)?;
    git(&["update-ref", &format!("refs/heads/{branch}"), &wip, &pre])?;
    Ok(Packed { branch, pre, wip })
}

pub fn unpack(folder: &Path, packed: &Packed) -> Result<(), String> {
    if git(folder, &["rev-parse", "--verify", "HEAD"]).ok().as_deref() == Some(packed.wip.as_str()) {
        git(folder, &["reset", "--soft", &packed.pre])?;
    }
    Ok(())
}

pub fn heads(repo: &Path) -> Vec<String> {
    let listed = git(repo, &["for-each-ref", "--format=%(objectname)", "refs/heads", "refs/remotes", "refs/tags"]).unwrap_or_default();
    let mut heads: Vec<String> = listed.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
    heads.sort();
    heads.dedup();
    heads
}

pub fn bundle(folder: &Path, branch: &str, theirs: &[String], out: &Path) -> Result<(), String> {
    let known: Vec<String> = theirs.iter().filter(|sha| sha.len() >= 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()) && git(folder, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_ok()).map(|sha| format!("^{sha}")).collect();
    let refname = format!("refs/heads/{branch}");
    let out_text = out.to_string_lossy();
    let mut args = vec!["bundle", "create", out_text.as_ref(), refname.as_str()];
    args.extend(known.iter().map(String::as_str));
    git(folder, &args).map(|_| ())
}

pub fn unbundle(repo: &Path, id: &str, bundle: &Path, name: &str, branch: &str, packed: &Packed, base: Option<&str>) -> Result<(String, String), String> {
    let folder = repo.join(".kanna-worktrees").join(name);
    let shown = folder.to_string_lossy().into_owned();
    let short = |sha: &str| sha.chars().take(9).collect::<String>();
    let contains = |tip: &str, commit: &str| git(repo, &["merge-base", "--is-ancestor", commit, tip]).is_ok();
    if folder.exists() {
        let mine = git(repo, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).ok().and_then(|d| fs::canonicalize(d).ok());
        let its = git(&folder, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).ok().and_then(|d| fs::canonicalize(d).ok());
        if mine.is_none() || mine != its {
            return Err(format!("{shown} is there but isn't a worktree of this clone"));
        }
        let on = git(&folder, &["symbolic-ref", "--quiet", "--short", "HEAD"]).unwrap_or_else(|_| "no branch".into());
        if on != branch {
            return Err(format!("{shown} is there on {on}, not {branch}: it isn't this task's worktree"));
        }
        let head = git(&folder, &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
        let staged = git(&folder, &["write-tree"]).unwrap_or_default();
        if head == packed.pre && git(repo, &["rev-parse", &format!("{}^{{tree}}", packed.wip)]).ok().as_deref() == Some(staged.as_str()) {
            return Ok((shown, branch.to_string()));
        }
        if !git(&folder, &["status", "--porcelain"]).unwrap_or_default().is_empty() {
            return Err(format!("{shown} is this task's worktree but has uncommitted changes of its own, which taking the task would overwrite"));
        }
    }
    let bundle_text = bundle.to_string_lossy();
    git(repo, &["bundle", "verify", "--quiet", bundle_text.as_ref()]).map_err(|e| format!("this clone can't take the bundle: {e}"))?;
    let private = format!("refs/kanna/transfers/{id}");
    git(repo, &["fetch", "--quiet", "--no-tags", bundle_text.as_ref(), &format!("+refs/heads/{}:{private}", packed_branch(bundle, repo)?)])?;
    let fetched = git(repo, &["rev-parse", "--verify", &private])?;
    if fetched != packed.wip {
        return Err(format!("the bundle's branch is at {fetched}, not {}", packed.wip));
    }
    let refname = format!("refs/heads/{branch}");
    let default = git(repo, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]).ok().and_then(|d| d.split_once('/').map(|(_, b)| b.to_string())).unwrap_or_else(|| "main".into());
    if branch == default || git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok().as_deref() == Some(branch) {
        let _ = git(repo, &["update-ref", "-d", &private]);
        return Err(format!("{branch} is this clone's own branch, not a task's"));
    }
    let tip = git(repo, &["rev-parse", "--verify", "--quiet", &refname]).ok();
    if let Some(tip) = &tip {
        if *tip != packed.wip && !contains(&packed.pre, tip) {
            let _ = git(repo, &["update-ref", "-d", &private]);
            return Err(match base.filter(|base| !contains(tip, base)) {
                Some(base) => format!("this clone's branch {branch} (at {}) isn't this task's: it doesn't contain the task's base {}", short(tip), short(base)),
                None => format!("this clone's branch {branch} has diverged from the task's: it's at {}, the task at {}, and neither contains the other", short(tip), short(&packed.pre)),
            });
        }
    }
    let made = match (folder.exists(), &tip) {
        (true, _) => git(&folder, &["merge", "--ff-only", "--quiet", &packed.wip]).map(|_| ()),
        (false, Some(tip)) => crate::tasks::exclude_worktrees(repo).and_then(|_| git(repo, &["update-ref", &refname, &packed.wip, tip])).and_then(|_| {
            git(repo, &["worktree", "add", "--quiet", &shown, branch]).map(|_| ()).map_err(|e| {
                let _ = git(repo, &["update-ref", &refname, tip, &packed.wip]);
                format!("this clone's branch {branch} can't have a worktree here: {e}")
            })
        }),
        (false, None) => add_worktree(repo, name, branch, Some(&packed.wip)).map(|_| ()),
    };
    if let Err(why) = made {
        let _ = git(repo, &["update-ref", "-d", &private]);
        return Err(why);
    }
    git(&folder, &["reset", "--soft", &packed.pre])?;
    let staged = git(&folder, &["write-tree"])?;
    let sent = git(repo, &["rev-parse", &format!("{}^{{tree}}", packed.wip)])?;
    if staged != sent {
        return Err(format!("the worktree's staged tree {staged} isn't the one sent, {sent}"));
    }
    crate::task_scripts::set_base(&folder, base.unwrap_or(&packed.pre));
    let _ = git(repo, &["update-ref", "-d", &private]);
    Ok((shown, branch.to_string()))
}

fn packed_branch(bundle: &Path, repo: &Path) -> Result<String, String> {
    let listed = git(repo, &["bundle", "list-heads", &bundle.to_string_lossy()])?;
    listed.lines().find_map(|line| line.split_whitespace().nth(1)?.strip_prefix("refs/heads/").map(String::from)).ok_or_else(|| "the bundle has no branch".to_string())
}

pub fn remove(repo: &Path, folder: &Path, branch: &str) -> Result<(), String> {
    if folder.exists() {
        git(repo, &["worktree", "remove", "--force", &folder.to_string_lossy()])?;
    }
    if git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok() {
        git(repo, &["branch", "-D", branch])?;
    }
    Ok(())
}

pub fn moves(agent: &str) -> bool {
    matches!(agent, "claude" | "codex")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn dir(name: &str) -> PathBuf {
        static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("kanna-transfer-{name}-{}-{}", std::process::id(), COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn run(repo: &Path, args: &[&str]) -> String {
        let mut all: Vec<&str> = IDENTITY.to_vec();
        all.extend(args);
        git(repo, &all).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    fn sender_and_receiver() -> (PathBuf, PathBuf, PathBuf) {
        let root = dir("pair");
        let origin = root.join("mac/notes");
        fs::create_dir_all(&origin).unwrap();
        run(&origin, &["init", "-q", "-b", "main"]);
        fs::write(origin.join("tracked.txt"), "one\n").unwrap();
        fs::write(origin.join("gone.txt"), "bye\n").unwrap();
        run(&origin, &["add", "-A"]);
        run(&origin, &["commit", "-q", "-m", "base"]);
        let other = root.join("linux/notes");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        git(&root, &["clone", "-q", &origin.to_string_lossy(), &other.to_string_lossy()]).unwrap();
        let (folder, _) = add_worktree(&origin, "task-0000beef", "fix-notes", None).unwrap();
        let folder = PathBuf::from(folder);
        fs::write(folder.join("tracked.txt"), "one\ntwo\n").unwrap();
        run(&folder, &["commit", "-q", "-am", "task's commit"]);
        (origin, other, folder)
    }

    #[test]
    fn a_dirty_worktree_moves_to_another_clone_with_its_changes_staged_and_nothing_else_changed() {
        let (origin, other, folder) = sender_and_receiver();
        fs::write(folder.join("tracked.txt"), "one\ntwo\nthree\n").unwrap();
        fs::remove_file(folder.join("gone.txt")).unwrap();
        fs::write(folder.join("new file.txt"), "untracked\n").unwrap();
        fs::write(folder.join("tool.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(folder.join("tool.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("tracked.txt", folder.join("link")).unwrap();
        let task_commit = run(&folder, &["rev-parse", "HEAD"]);

        let packed = pack(&folder, "task-0000beef").unwrap();
        assert_eq!((packed.branch.as_str(), packed.pre.as_str()), ("fix-notes", task_commit.as_str()));
        assert_eq!(run(&folder, &["status", "--porcelain"]), "", "the sender's worktree is clean at the work-in-progress commit");
        let again = pack(&folder, "task-0000beef").unwrap();
        assert_eq!(again.pre, packed.pre, "packing twice doesn't stack a second work-in-progress commit");

        let file = origin.join("t.bundle");
        bundle(&folder, &again.branch, &heads(&other), &file).unwrap();
        let listed = git(&origin, &["bundle", "verify", &file.to_string_lossy()]).unwrap_or_else(|e| e);
        assert!(!listed.contains("complete history"), "the bundle leaves out what the receiver has: {listed}");

        let (moved, branch) = unbundle(&other, "t1", &file, "task-0000beef", "fix-notes", &again, None).unwrap();
        let moved = PathBuf::from(moved);
        assert_eq!(branch, "fix-notes");
        assert_eq!(run(&moved, &["rev-parse", "HEAD"]), task_commit, "the work-in-progress commit is undone");
        assert_eq!(run(&moved, &["diff"]), "", "nothing is left unstaged");
        let staged = run(&moved, &["diff", "--cached", "--name-status"]);
        for line in ["M\ttracked.txt", "D\tgone.txt", "A\tnew file.txt", "A\ttool.sh", "A\tlink"] {
            assert!(staged.contains(line), "{line} is staged: {staged}");
        }
        assert_eq!(fs::read_to_string(moved.join("tracked.txt")).unwrap(), "one\ntwo\nthree\n");
        assert!(fs::metadata(moved.join("tool.sh")).unwrap().permissions().mode() & 0o111 != 0, "modes move too");
        assert_eq!(fs::read_link(moved.join("link")).unwrap(), PathBuf::from("tracked.txt"));
        assert!(git(&other, &["rev-parse", "--verify", "--quiet", "refs/kanna/transfers/t1"]).is_err(), "the private ref is gone");
        assert_eq!(unbundle(&other, "t1", &file, "task-0000beef", "fix-notes", &again, None).unwrap().0, moved.to_string_lossy(), "taking it twice gives the same worktree");

        unpack(&folder, &again).unwrap();
        assert_eq!(run(&folder, &["rev-parse", "HEAD"]), task_commit, "a transfer called off leaves the sender as it was, its changes staged");
        remove(&origin, &folder, "fix-notes").unwrap();
        assert!(!folder.exists() && git(&origin, &["rev-parse", "--verify", "--quiet", "refs/heads/fix-notes"]).is_err());
    }

    #[test]
    fn a_clone_with_none_of_the_branch_gets_its_whole_history() {
        let (origin, _, folder) = sender_and_receiver();
        let packed = pack(&folder, "task-0000beef").unwrap();
        let file = origin.join("t.bundle");
        bundle(&folder, &packed.branch, &["f".repeat(40)], &file).unwrap();
        let empty = dir("empty").join("notes");
        fs::create_dir_all(&empty).unwrap();
        run(&empty, &["init", "-q", "-b", "main"]);
        let (moved, _) = unbundle(&empty, "t2", &file, "task-0000beef", "fix-notes", &packed, None).unwrap();
        assert_eq!(fs::read_to_string(Path::new(&moved).join("tracked.txt")).unwrap(), "one\ntwo\n");
    }

    fn moved(origin: &Path, folder: &Path, theirs: &Path) -> (PathBuf, Packed, String) {
        let base = run(origin, &["rev-parse", "main"]);
        let packed = pack(folder, "task-0000beef").unwrap();
        let file = origin.join(format!("{}.bundle", packed.wip));
        bundle(folder, &packed.branch, &heads(theirs), &file).unwrap();
        (file, packed, base)
    }

    fn branch_at(repo: &Path, branch: &str) -> String {
        run(repo, &["rev-parse", &format!("refs/heads/{branch}")])
    }

    #[test]
    fn a_branch_or_worktree_of_this_task_the_receiver_has_is_fast_forwarded_and_reused() {
        let (origin, other, folder) = sender_and_receiver();
        let behind = run(&folder, &["rev-parse", "HEAD"]);
        let file = origin.join("early.bundle");
        git(&folder, &["bundle", "create", &file.to_string_lossy(), "fix-notes", "^main"]).unwrap();
        run(&other, &["fetch", "-q", &file.to_string_lossy(), "fix-notes:fix-notes"]);
        fs::write(folder.join("tracked.txt"), "one\ntwo\nmore\n").unwrap();
        run(&folder, &["commit", "-q", "-am", "later"]);
        fs::write(folder.join("wip.txt"), "wip\n").unwrap();
        let (file, packed, base) = moved(&origin, &folder, &other);
        let (made, branch) = unbundle(&other, "t3", &file, "task-0000beef", "fix-notes", &packed, Some(&base)).unwrap();
        assert_eq!((branch.as_str(), branch_at(&other, "fix-notes")), ("fix-notes", packed.pre.clone()), "the branch {behind} behind was fast-forwarded, its WIP commit undone");
        assert_eq!(run(Path::new(&made), &["rev-parse", "HEAD"]), packed.pre);
        assert_eq!(crate::task_scripts::base(Path::new(&made)).as_deref(), Some(base.as_str()), "the task's base comes along");

        let (origin, other, folder) = sender_and_receiver();
        let early = origin.join("early.bundle");
        git(&folder, &["bundle", "create", &early.to_string_lossy(), "fix-notes", "^main"]).unwrap();
        run(&other, &["fetch", "-q", &early.to_string_lossy(), "fix-notes:fix-notes"]);
        let kept = other.join(".kanna-worktrees/task-0000beef");
        run(&other, &["worktree", "add", "-q", &kept.to_string_lossy(), "fix-notes"]);
        fs::write(folder.join("tracked.txt"), "one\ntwo\nmore\n").unwrap();
        let (file, packed, base) = moved(&origin, &folder, &other);
        let (made, _) = unbundle(&other, "t4", &file, "task-0000beef", "fix-notes", &packed, Some(&base)).unwrap();
        assert_eq!(PathBuf::from(&made), kept, "this task's worktree from an earlier run is reused");
        assert_eq!(fs::read_to_string(kept.join("tracked.txt")).unwrap(), "one\ntwo\nmore\n");
        assert_eq!(run(&kept, &["diff", "--cached", "--name-only"]), "tracked.txt");
    }

    #[test]
    fn a_branch_or_worktree_that_isnt_this_tasks_or_has_diverged_is_refused_and_left_as_it_was() {
        let (origin, other, folder) = sender_and_receiver();
        run(&other, &["checkout", "-q", "--orphan", "fix-notes"]);
        fs::write(other.join("other.txt"), "x\n").unwrap();
        run(&other, &["add", "-A"]);
        run(&other, &["commit", "-q", "-m", "someone else's fix-notes"]);
        run(&other, &["checkout", "-q", "main"]);
        let theirs = branch_at(&other, "fix-notes");
        let (file, packed, base) = moved(&origin, &folder, &other);
        let refused = unbundle(&other, "t5", &file, "task-0000beef", "fix-notes", &packed, Some(&base)).unwrap_err();
        assert!(refused.contains("isn't this task's") && refused.contains(&base[..9]), "{refused}");
        assert_eq!(branch_at(&other, "fix-notes"), theirs, "their branch is left as it was");
        assert!(!other.join(".kanna-worktrees/task-0000beef").exists());
        assert!(git(&other, &["rev-parse", "--verify", "--quiet", "refs/kanna/transfers/t5"]).is_err());

        let (origin, other, folder) = sender_and_receiver();
        run(&other, &["branch", "fix-notes", "main"]);
        let diverged = other.join("diverged");
        run(&other, &["worktree", "add", "-q", &diverged.to_string_lossy(), "fix-notes"]);
        fs::write(diverged.join("tracked.txt"), "one\nother\n").unwrap();
        run(&diverged, &["commit", "-q", "-am", "a different change"]);
        let tip = branch_at(&other, "fix-notes");
        let (file, packed, base) = moved(&origin, &folder, &other);
        let refused = unbundle(&other, "t6", &file, "task-0000beef", "fix-notes", &packed, Some(&base)).unwrap_err();
        assert!(refused.contains("diverged") && refused.contains(&tip[..9]) && refused.contains(&packed.pre[..9]), "{refused}");
        assert_eq!(branch_at(&other, "fix-notes"), tip);

        let (origin, other, folder) = sender_and_receiver();
        let elsewhere = other.join(".kanna-worktrees/task-0000beef");
        run(&other, &["worktree", "add", "-q", "-b", "something-else", &elsewhere.to_string_lossy()]);
        let (file, packed, base) = moved(&origin, &folder, &other);
        let refused = unbundle(&other, "t7", &file, "task-0000beef", "fix-notes", &packed, Some(&base)).unwrap_err();
        assert!(refused.contains("on something-else, not fix-notes"), "{refused}");

        let (origin, other, folder) = sender_and_receiver();
        run(&other, &["branch", "fix-notes", "main"]);
        let mine = other.join(".kanna-worktrees/task-0000beef");
        run(&other, &["worktree", "add", "-q", &mine.to_string_lossy(), "fix-notes"]);
        fs::write(mine.join("tracked.txt"), "local edit\n").unwrap();
        let (file, packed, base) = moved(&origin, &folder, &other);
        let refused = unbundle(&other, "t8", &file, "task-0000beef", "fix-notes", &packed, Some(&base)).unwrap_err();
        assert!(refused.contains("uncommitted changes of its own"), "{refused}");

        let (origin, other, folder) = sender_and_receiver();
        let mut main = pack(&folder, "task-0000beef").unwrap();
        main.branch = "main".into();
        let file = origin.join("main.bundle");
        bundle(&folder, "fix-notes", &heads(&other), &file).unwrap();
        let before = branch_at(&other, "main");
        let refused = unbundle(&other, "t9", &file, "task-0000beef", "main", &main, None).unwrap_err();
        assert!(refused.contains("this clone's own branch"), "{refused}");
        assert_eq!(branch_at(&other, "main"), before);
        assert_eq!(fs::read_to_string(mine.join("tracked.txt")).unwrap(), "local edit\n", "nothing is overwritten");
    }
}
