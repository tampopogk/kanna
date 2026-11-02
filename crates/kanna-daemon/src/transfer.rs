use crate::db::Db;
use crate::tasks::{add_worktree, git};
use kanna_mux::json::Value;
use rusqlite::{params, Connection, OptionalExtension};
use std::fs;
use std::io::{Read, Write};
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
         on conflict (id) do update set state = excluded.state, doc = excluded.doc, at = excluded.at",
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

/// Commits everything in the worktree, untracked files included, on top of its branch, and returns that commit and the one before it.
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

/// Takes the work-in-progress commit back off the branch, leaving its changes staged, if it's still the branch's tip.
pub fn unpack(folder: &Path, packed: &Packed) -> Result<(), String> {
    if git(folder, &["rev-parse", "--verify", "HEAD"]).ok().as_deref() == Some(packed.wip.as_str()) {
        git(folder, &["reset", "--soft", &packed.pre])?;
    }
    Ok(())
}

/// The commits a clone has at the tips of its branches, remote branches and tags, which a bundle for it can leave out.
pub fn heads(repo: &Path) -> Vec<String> {
    let listed = git(repo, &["for-each-ref", "--format=%(objectname)", "refs/heads", "refs/remotes", "refs/tags"]).unwrap_or_default();
    let mut heads: Vec<String> = listed.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
    heads.sort();
    heads.dedup();
    heads
}

/// Writes a bundle of the branch that leaves out the commits the receiver has and this repo knows.
pub fn bundle(folder: &Path, branch: &str, theirs: &[String], out: &Path) -> Result<(), String> {
    let known: Vec<String> = theirs.iter().filter(|sha| sha.len() >= 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()) && git(folder, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_ok()).map(|sha| format!("^{sha}")).collect();
    let refname = format!("refs/heads/{branch}");
    let out_text = out.to_string_lossy();
    let mut args = vec!["bundle", "create", out_text.as_ref(), refname.as_str()];
    args.extend(known.iter().map(String::as_str));
    git(folder, &args).map(|_| ())
}

/// Takes a bundle into a clone and makes the task's worktree from it, its work-in-progress commit undone and its changes staged.
pub fn unbundle(repo: &Path, id: &str, bundle: &Path, name: &str, branch: &str, packed: &Packed) -> Result<(String, String), String> {
    let folder = repo.join(".kanna-worktrees").join(name);
    if folder.is_dir() {
        let head = git(&folder, &["rev-parse", "--verify", "HEAD"]).ok();
        let on = git(&folder, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
        if head.as_deref() == Some(packed.pre.as_str()) && on.as_deref() == Some(branch) {
            return Ok((folder.to_string_lossy().into_owned(), branch.to_string()));
        }
        return Err(format!("{} is there already", folder.display()));
    }
    let bundle_text = bundle.to_string_lossy();
    git(repo, &["bundle", "verify", "--quiet", bundle_text.as_ref()]).map_err(|e| format!("this clone can't take the bundle: {e}"))?;
    let private = format!("refs/kanna/transfers/{id}");
    git(repo, &["fetch", "--quiet", "--no-tags", bundle_text.as_ref(), &format!("+refs/heads/{}:{private}", packed_branch(bundle, repo)?)])?;
    let fetched = git(repo, &["rev-parse", "--verify", &private])?;
    if fetched != packed.wip {
        return Err(format!("the bundle's branch is at {fetched}, not {}", packed.wip));
    }
    let (folder, branch) = add_worktree(repo, name, branch, Some(&packed.wip))?;
    git(Path::new(&folder), &["reset", "--soft", &packed.pre])?;
    let staged = git(Path::new(&folder), &["write-tree"])?;
    let sent = git(repo, &["rev-parse", &format!("{}^{{tree}}", packed.wip)])?;
    if staged != sent {
        return Err(format!("the worktree's staged tree {staged} isn't the one sent, {sent}"));
    }
    let _ = git(repo, &["update-ref", "-d", &private]);
    Ok((folder, branch))
}

fn packed_branch(bundle: &Path, repo: &Path) -> Result<String, String> {
    let listed = git(repo, &["bundle", "list-heads", &bundle.to_string_lossy()])?;
    listed.lines().find_map(|line| line.split_whitespace().nth(1)?.strip_prefix("refs/heads/").map(String::from)).ok_or_else(|| "the bundle has no branch".to_string())
}

/// Removes the sender's worktree and branch once the task carries on elsewhere.
pub fn remove(repo: &Path, folder: &Path, branch: &str) -> Result<(), String> {
    if folder.exists() {
        git(repo, &["worktree", "remove", "--force", &folder.to_string_lossy()])?;
    }
    if git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok() {
        git(repo, &["branch", "-D", branch])?;
    }
    Ok(())
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

#[derive(Clone, Debug)]
pub struct Homes {
    pub claude: PathBuf,
    pub codex: PathBuf,
}

impl Homes {
    pub fn here() -> Homes {
        let dir = |var: &str, default: &str| std::env::var_os(var).filter(|d| !d.is_empty()).map_or_else(|| home().join(default), PathBuf::from);
        Homes { claude: dir("CLAUDE_CONFIG_DIR", ".claude"), codex: dir("CODEX_HOME", ".codex") }
    }

    fn sessions(&self) -> PathBuf {
        self.codex.join("sessions")
    }
}

pub fn claude_project(folder: &Path) -> String {
    folder.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn folders(folder: &Path) -> Vec<PathBuf> {
    let mut all = vec![folder.to_path_buf()];
    if let Ok(real) = folder.canonicalize() {
        if real != folder {
            all.push(real);
        }
    }
    all
}

pub fn moves(agent: &str) -> bool {
    matches!(agent, "claude" | "codex")
}

/// Where the agent keeps a session's transcript, and its name to keep on another machine.
pub fn transcript(homes: &Homes, agent: &str, folder: &Path, session: &str) -> Result<(PathBuf, String), String> {
    if session.is_empty() || session.contains('/') || session.contains("..") {
        return Err(format!("\"{session}\" isn't a session ID"));
    }
    let file = format!("{session}.jsonl");
    match agent {
        "claude" => {
            let projects = homes.claude.join("projects");
            let here = folders(folder).into_iter().map(|f| projects.join(claude_project(&f)).join(&file)).find(|p| p.is_file());
            let found = here.or_else(|| {
                let all: Vec<PathBuf> = fs::read_dir(&projects).into_iter().flatten().flatten().map(|d| d.path().join(&file)).filter(|p| p.is_file()).collect();
                (all.len() == 1).then(|| all[0].clone())
            });
            found.map(|path| (path, file.clone())).ok_or_else(|| format!("claude's transcript of {session} isn't in {}", projects.display()))
        }
        "codex" => {
            let sessions = homes.sessions();
            let suffix = format!("-{session}.jsonl");
            let listed = |dir: &Path| fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect::<Vec<_>>();
            let found = listed(&sessions).into_iter().flat_map(|y| listed(&y)).flat_map(|m| listed(&m)).flat_map(|d| listed(&d)).find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-") && n.ends_with(&suffix)));
            let path = found.ok_or_else(|| format!("codex's transcript of {session} isn't in {}", sessions.display()))?;
            let name = path.strip_prefix(&sessions).map_err(|e| e.to_string())?.to_string_lossy().into_owned();
            Ok((path, name))
        }
        other => Err(format!("{other}'s conversations don't move between machines yet: only claude's and codex's")),
    }
}

/// Puts a transcript where the agent looks for it when it resumes in this folder.
pub fn install(homes: &Homes, agent: &str, folder: &Path, name: &str, bytes: &[u8]) -> Result<Vec<PathBuf>, String> {
    let safe = !name.is_empty() && Path::new(name).components().all(|c| matches!(c, std::path::Component::Normal(_)));
    if !safe || !name.ends_with(".jsonl") {
        return Err(format!("\"{name}\" isn't a transcript's name"));
    }
    let places: Vec<PathBuf> = match agent {
        "claude" if !name.contains('/') => folders(folder).into_iter().map(|f| homes.claude.join("projects").join(claude_project(&f)).join(name)).collect(),
        "codex" => vec![homes.sessions().join(name)],
        other => return Err(format!("{other}'s transcript \"{name}\" has no place here")),
    };
    for place in &places {
        let dir = place.parent().unwrap_or(folder);
        fs::create_dir_all(dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
        let part = place.with_extension("jsonl.part");
        let mut out = fs::File::create(&part).map_err(|e| format!("can't write {}: {e}", part.display()))?;
        out.write_all(bytes).and_then(|_| out.sync_all()).map_err(|e| format!("can't write {}: {e}", part.display()))?;
        fs::rename(&part, place).map_err(|e| format!("can't write {}: {e}", place.display()))?;
    }
    Ok(places)
}

/// One file of a bundle followed by a transcript, whose sizes the transfer's header gives.
pub fn join(out: &Path, bundle: &Path, transcript: &Path) -> Result<(u64, u64), String> {
    let mut file = fs::File::create(out).map_err(|e| format!("can't write {}: {e}", out.display()))?;
    let mut sizes = [0u64; 2];
    for (i, part) in [bundle, transcript].iter().enumerate() {
        let mut from = fs::File::open(part).map_err(|e| format!("can't read {}: {e}", part.display()))?;
        sizes[i] = std::io::copy(&mut from, &mut file).map_err(|e| format!("can't write {}: {e}", out.display()))?;
    }
    file.sync_all().map_err(|e| e.to_string())?;
    Ok((sizes[0], sizes[1]))
}

pub fn split(file: &Path, bundle_size: u64, transcript_size: u64, bundle: &Path) -> Result<Vec<u8>, String> {
    let mut from = fs::File::open(file).map_err(|e| format!("can't read {}: {e}", file.display()))?;
    let total = from.metadata().map_err(|e| e.to_string())?.len();
    if total != bundle_size + transcript_size {
        return Err(format!("the transfer has {total} bytes, not {}", bundle_size + transcript_size));
    }
    let mut out = fs::File::create(bundle).map_err(|e| format!("can't write {}: {e}", bundle.display()))?;
    std::io::copy(&mut (&mut from).take(bundle_size), &mut out).map_err(|e| e.to_string())?;
    let mut transcript = Vec::with_capacity(transcript_size as usize);
    from.read_to_end(&mut transcript).map_err(|e| e.to_string())?;
    Ok(transcript)
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

        let (moved, branch) = unbundle(&other, "t1", &file, "task-0000beef", "fix-notes", &again).unwrap();
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
        assert_eq!(unbundle(&other, "t1", &file, "task-0000beef", "fix-notes", &again).unwrap().0, moved.to_string_lossy(), "taking it twice gives the same worktree");

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
        let (moved, _) = unbundle(&empty, "t2", &file, "task-0000beef", "fix-notes", &packed).unwrap();
        assert_eq!(fs::read_to_string(Path::new(&moved).join("tracked.txt")).unwrap(), "one\ntwo\n");
    }

    #[test]
    fn transcripts_are_found_where_each_agent_keeps_them_and_installed_at_the_receivers_own_paths() {
        let home = dir("home");
        let homes = Homes { claude: home.join("claude"), codex: home.join("codex") };
        let sender = Path::new("/Users/me/.kanna-v4/repos/team/notes/.kanna-worktrees/task-0000beef");
        let receiver = Path::new("/home/me/repos/notes/.kanna-worktrees/task-0000beef");
        let claude = home.join("claude/projects/-Users-me--kanna-v4-repos-team-notes--kanna-worktrees-task-0000beef/s1.jsonl");
        fs::create_dir_all(claude.parent().unwrap()).unwrap();
        fs::write(&claude, "{\"a\":1}\n").unwrap();
        assert_eq!(transcript(&homes, "claude", sender, "s1").unwrap(), (claude.clone(), "s1.jsonl".into()));
        let placed = install(&homes, "claude", receiver, "s1.jsonl", b"{\"a\":1}\n").unwrap();
        assert_eq!(placed, vec![home.join("claude/projects/-home-me-repos-notes--kanna-worktrees-task-0000beef/s1.jsonl")]);
        assert_eq!(fs::read(&placed[0]).unwrap(), fs::read(&claude).unwrap());

        let codex = home.join("codex/sessions/2026/11/02/rollout-2026-11-02T10-00-00-c1.jsonl");
        fs::create_dir_all(codex.parent().unwrap()).unwrap();
        fs::write(&codex, "{}\n").unwrap();
        assert_eq!(transcript(&homes, "codex", sender, "c1").unwrap(), (codex.clone(), "2026/11/02/rollout-2026-11-02T10-00-00-c1.jsonl".into()));
        assert_eq!(install(&homes, "codex", receiver, "2026/11/02/rollout-2026-11-02T10-00-00-c1.jsonl", b"{}\n").unwrap(), vec![codex]);

        assert!(transcript(&homes, "claude", sender, "../x").is_err());
        assert!(install(&homes, "codex", receiver, "../../.ssh/x.jsonl", b"").is_err(), "a name can't climb out of the sessions folder");
        assert!(transcript(&homes, "opencode", sender, "s1").is_err());
    }
}
