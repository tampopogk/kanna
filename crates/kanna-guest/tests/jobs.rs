use kanna_guest::client::{self, Outcome, Pipe, Request};
use kanna_guest::supervisor::{Config, Supervisor};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kanna-jobs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e"]).args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

struct Rig {
    socket: PathBuf,
    state: PathBuf,
    repo: PathBuf,
}

fn rig(name: &str) -> Rig {
    let dir = temp(name);
    let (socket, state, repo) = (dir.join("guest.sock"), dir.join("state"), dir.join("repo"));
    let supervisor = Supervisor::new(Config::new(state.clone())).unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let supervisor = supervisor.clone();
            std::thread::spawn(move || {
                let _ = supervisor.serve(conn);
            });
        }
    });
    std::fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("src/committed.txt"), "committed\n").unwrap();
    std::fs::write(repo.join("gone.txt"), "deleted later\n").unwrap();
    std::fs::write(repo.join(".gitignore"), "ignored.txt\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    Rig { socket, state, repo }
}

fn request(rig: &Rig, argv: &[&str]) -> (Request, client::Worktree) {
    let worktree = client::worktree(&rig.repo).unwrap();
    let request = Request { root: worktree.root.clone(), cwd: worktree.cwd.clone(), argv: argv.iter().map(|s| s.to_string()).collect(), env: BTreeMap::new(), fetch: vec![], ui: false, job: client::new_job(), needs: serde_json::Value::Null, label: String::new() };
    (request, worktree)
}

fn run(rig: &Rig, request: &Request, worktree: &client::Worktree) -> (Outcome, String, String) {
    let socket = rig.socket.clone();
    let connect = move || -> io::Result<Box<dyn Pipe>> { Ok(Box::new(UnixStream::connect(&socket)?)) };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let outcome = client::run(&connect, request, worktree, &mut out, &mut err);
    (outcome, String::from_utf8_lossy(&out).into_owned(), String::from_utf8_lossy(&err).into_owned())
}

#[test]
fn the_worktree_as_it_is_runs_on_the_guest_and_its_exit_code_comes_back() {
    let rig = rig("exit");
    std::fs::write(rig.repo.join("src/committed.txt"), "dirty\n").unwrap();
    std::fs::write(rig.repo.join("untracked.txt"), "untracked\n").unwrap();
    std::fs::write(rig.repo.join("ignored.txt"), "ignored\n").unwrap();
    std::fs::remove_file(rig.repo.join("gone.txt")).unwrap();
    let script = "cat src/committed.txt untracked.txt; test -e gone.txt && echo gone-is-there; test -e ignored.txt && echo ignored-is-there; git status --short; echo to-stderr >&2; exit 3";
    let (req, worktree) = request(&rig, &["sh", "-c", script]);
    let (outcome, out, err) = run(&rig, &req, &worktree);
    assert_eq!(outcome, Outcome::Exited(3), "{err}");
    assert!(out.starts_with("dirty\nuntracked\n"), "{out}");
    assert!(!out.contains("gone-is-there") && !out.contains("ignored-is-there"), "{out}");
    assert!(out.contains(" M src/committed.txt") && out.contains(" D gone.txt") && out.contains("?? untracked.txt"), "git on the guest sees the worktree's state: {out}");
    assert_eq!(err, "to-stderr\n");
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(rig.repo.join(".kanna/build").join(&req.job).join("job.json")).unwrap()).unwrap();
    assert_eq!(record["code"], 3);

    let (again, worktree) = request(&rig, &["sh", "-c", "cat src/committed.txt"]);
    let started = Instant::now();
    let (outcome, out, _) = run(&rig, &again, &worktree);
    assert_eq!((outcome, out.as_str()), (Outcome::Exited(0), "dirty\n"));
    assert!(started.elapsed() < Duration::from_secs(5));
    let manifest_blobs = std::fs::read_dir(rig.state.join("store")).unwrap().count();
    assert!(manifest_blobs >= 3);
}

#[test]
fn diagnostics_and_results_come_back_with_this_worktrees_paths() {
    let rig = rig("diag");
    let (mut req, worktree) = request(&rig, &["sh", "-c", "echo \"$PWD/src/committed.txt:1:2: error: bad\"; mkdir -p product.app/Contents && printf bin > product.app/Contents/exe && chmod +x product.app/Contents/exe && ln -s Contents product.app/Current; exit 1"]);
    req.fetch = vec!["product.app".into(), "never.txt".into()];
    let (outcome, out, err) = run(&rig, &req, &worktree);
    assert_eq!(outcome, Outcome::Exited(1));
    assert!(out.contains("/state/mirrors/local/"), "raw output is untouched: {out}");
    let mapped = format!("{}/src/committed.txt:1:2: error: bad", rig.repo.display());
    assert!(err.contains(&mapped), "{err}");
    assert!(err.contains("never.txt wasn't produced"), "{err}");
    let exe = rig.repo.join("product.app/Contents/exe");
    assert_eq!(std::fs::read_to_string(&exe).unwrap(), "bin");
    assert!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&exe).unwrap().permissions()) & 0o111 != 0);
    assert_eq!(std::fs::read_link(rig.repo.join("product.app/Current")).unwrap(), PathBuf::from("Contents"));
    let diagnostics = std::fs::read_to_string(rig.repo.join(".kanna/build").join(&req.job).join("diagnostics.txt")).unwrap();
    assert_eq!(diagnostics.trim(), mapped);
    let staged = rig.state.join("jobs").join(&req.job).join("fetched/0/product.app/Contents/exe");
    assert_eq!(std::fs::read_to_string(staged).unwrap(), "bin", "a fetched product is frozen in the job's folder before the mirror is released");
}

#[test]
fn a_fetch_replaces_only_what_kanna_build_put_there() {
    let rig = rig("ledger");
    std::fs::write(rig.repo.join("notes.json"), "mine").unwrap();
    std::fs::create_dir_all(rig.repo.join("target")).unwrap();
    std::fs::write(rig.repo.join(".gitignore"), "target/\n").unwrap();
    let (mut req, worktree) = request(&rig, &["sh", "-c", "mkdir -p target && echo built-$RANDOM > target/out.json && echo theirs > notes.json"]);
    req.fetch = vec!["target/out.json".into(), "notes.json".into()];
    let (outcome, _, err) = run(&rig, &req, &worktree);
    assert_eq!(outcome, Outcome::Exited(0));
    assert!(err.contains("kanna build didn't put it there"), "{err}");
    assert_eq!(std::fs::read_to_string(rig.repo.join("notes.json")).unwrap(), "mine");
    let first = std::fs::read_to_string(rig.repo.join("target/out.json")).unwrap();
    let (mut req, worktree) = request(&rig, &["sh", "-c", "mkdir -p target && echo again > target/out.json"]);
    req.fetch = vec!["target/out.json".into()];
    let (outcome, _, err) = run(&rig, &req, &worktree);
    assert_eq!(outcome, Outcome::Exited(0), "{err}");
    assert_ne!(first, "again\n");
    assert_eq!(std::fs::read_to_string(rig.repo.join("target/out.json")).unwrap(), "again\n");
}

#[test]
fn a_fetch_under_a_linked_folder_is_refused() {
    let rig = rig("linkedfetch");
    let outside = temp("linkedfetch-outside");
    std::os::unix::fs::symlink(&outside, rig.repo.join("out")).unwrap();
    std::fs::write(rig.repo.join(".gitignore"), "out\n").unwrap();
    let (mut req, worktree) = request(&rig, &["sh", "-c", "mkdir -p out && echo x > out/p.txt"]);
    req.fetch = vec!["out/p.txt".into()];
    let (_, _, err) = run(&rig, &req, &worktree);
    assert!(err.contains("refused an artifact: out isn't a folder"), "{err}");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn the_commit_captured_is_the_one_sent_even_if_head_moves() {
    let rig = rig("moved");
    let (req, worktree) = request(&rig, &["git", "rev-parse", "HEAD"]);
    std::fs::write(rig.repo.join("later.txt"), "later\n").unwrap();
    git(&rig.repo, &["add", "-A"]);
    git(&rig.repo, &["commit", "-q", "-m", "later"]);
    let (outcome, out, err) = run(&rig, &req, &worktree);
    assert_eq!(outcome, Outcome::Exited(0), "{err}");
    assert_eq!(out.trim(), worktree.commit);
    let refs = Command::new("git").arg("-C").arg(&rig.repo).args(["for-each-ref", "refs/kanna"]).output().unwrap();
    assert!(refs.stdout.is_empty(), "the ref made for the bundle is gone");
}

#[test]
fn the_developer_dir_asked_for_is_the_one_the_job_gets() {
    let rig = rig("xcode");
    let xcode = temp("xcode-dir");
    let (mut req, worktree) = request(&rig, &["sh", "-c", "echo $DEVELOPER_DIR"]);
    req.env.insert("DEVELOPER_DIR".into(), xcode.to_string_lossy().into_owned());
    let (outcome, out, err) = run(&rig, &req, &worktree);
    assert_eq!(outcome, Outcome::Exited(0), "{err}");
    assert_eq!(out.trim(), xcode.to_string_lossy());
    let (mut req, worktree) = request(&rig, &["true"]);
    req.env.insert("DEVELOPER_DIR".into(), "/Applications/Xcode-0.app/Contents/Developer".into());
    let (outcome, _, _) = run(&rig, &req, &worktree);
    assert!(matches!(&outcome, Outcome::Failed(why) if why.contains("isn't a folder on the build Mac")), "{outcome:?}");
}

#[test]
fn a_build_mac_that_sends_garbage_fails_the_build_without_retrying() {
    let rig = rig("garbage");
    let (req, worktree) = request(&rig, &["true"]);
    let connects = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = connects.clone();
    let connect = move || -> io::Result<Box<dyn Pipe>> {
        counted.fetch_add(1, Ordering::SeqCst);
        let (ours, theirs) = UnixStream::pair()?;
        std::thread::spawn(move || {
            let mut theirs = theirs;
            let _ = kanna_guest::frame::send(&mut theirs, &serde_json::json!({ "op": "exists" }), b"");
            let _ = kanna_guest::frame::write_header(&mut theirs, &serde_json::json!({ "op": "out", "fd": 1, "at": 0 }), 64u64 << 20);
            std::thread::sleep(Duration::from_secs(5));
        });
        Ok(Box::new(ours))
    };
    let started = Instant::now();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let outcome = client::run(&connect, &req, &worktree, &mut out, &mut err);
    assert!(matches!(&outcome, Outcome::Failed(why) if why.contains("stopped taking")), "{outcome:?}");
    assert_eq!(connects.load(Ordering::SeqCst), 1);
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[test]
fn a_cancel_asked_for_before_the_job_reaches_the_build_mac_still_cancels_it() {
    let rig = rig("earlycancel");
    let (req, worktree) = request(&rig, &["sleep", "30"]);
    let socket = rig.socket.clone();
    let connect = move || -> io::Result<Box<dyn Pipe>> { Ok(Box::new(UnixStream::connect(&socket)?)) };
    let early = client::cancel(&connect, &req.job);
    assert!(matches!(early, Ok(false)), "{early:?}");
    let requested = std::sync::atomic::AtomicBool::new(true);
    let started = Instant::now();
    let outcome = std::thread::scope(|scope| {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let (connect, job, requested) = (&connect, req.job.clone(), &requested);
        scope.spawn(move || client::watch_cancel(connect, &job, requested, &done_rx));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let outcome = client::run(connect, &req, &worktree, &mut out, &mut err);
        let _ = done_tx.send(());
        outcome
    });
    assert_eq!(outcome, Outcome::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(20));
}

struct Dropping {
    inner: UnixStream,
    drops: bool,
    seen: Vec<u8>,
}

impl Read for Dropping {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.drops && self.seen.windows(6).any(|w| w == b"line-2") {
            let _ = self.inner.shutdown(std::net::Shutdown::Both);
            return Err(io::Error::new(io::ErrorKind::ConnectionReset, "dropped"));
        }
        let n = self.inner.read(buf)?;
        if self.drops {
            self.seen.extend_from_slice(&buf[..n]);
        }
        Ok(n)
    }
}

impl Write for Dropping {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[test]
fn a_dropped_connection_reattaches_to_the_same_job() {
    let rig = rig("drop");
    let (req, worktree) = request(&rig, &["sh", "-c", "echo $KANNA_BUILD_JOB >> runs.log; for i in 1 2 3 4 5; do echo line-$i; sleep 0.3; done; cat runs.log | wc -l | tr -d ' '"]);
    let socket = rig.socket.clone();
    let calls = AtomicUsize::new(0);
    let connect = move || -> io::Result<Box<dyn Pipe>> {
        let inner = UnixStream::connect(&socket)?;
        Ok(Box::new(Dropping { inner, drops: calls.fetch_add(1, Ordering::SeqCst) == 0, seen: vec![] }))
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let outcome = client::run(&connect, &req, &worktree, &mut out, &mut err);
    let out = String::from_utf8_lossy(&out);
    assert_eq!(outcome, Outcome::Exited(0), "{}", String::from_utf8_lossy(&err));
    assert_eq!(out, "line-1\nline-2\nline-3\nline-4\nline-5\n1\n", "each line once, and the job ran once");
}

#[test]
fn cancelling_stops_the_job_and_says_so() {
    let rig = rig("cancel");
    let marker = rig.state.parent().unwrap().join("started");
    let script = format!("echo started; touch '{}'; sleep 30; echo never", marker.display());
    let (req, worktree) = request(&rig, &["sh", "-c", &script]);
    let socket = rig.socket.clone();
    let job = req.job.clone();
    let canceller = std::thread::spawn(move || {
        let waiting = Instant::now();
        while !marker.exists() && waiting.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(300));
        let connect = move || -> io::Result<Box<dyn Pipe>> { Ok(Box::new(UnixStream::connect(&socket)?)) };
        client::cancel(&connect, &job).unwrap();
    });
    let started = Instant::now();
    let (outcome, out, _) = run(&rig, &req, &worktree);
    canceller.join().unwrap();
    assert_eq!((outcome, out.as_str()), (Outcome::Cancelled, "started\n"));
    assert!(started.elapsed() < Duration::from_secs(25));
}

#[test]
fn a_worktree_of_thousands_of_files_goes_in_one_job_and_a_rebuild_sends_one() {
    let rig = rig("many");
    for i in 0..2000 {
        std::fs::write(rig.repo.join("src").join(format!("file-{i:04}.swift")), format!("let v{i} = {i}\n")).unwrap();
    }
    let (req, worktree) = request(&rig, &["sh", "-c", "ls src | wc -l | tr -d ' '"]);
    let (outcome, out, err) = run(&rig, &req, &worktree);
    assert_eq!((outcome, out.as_str()), (Outcome::Exited(0), "2001\n"), "{err}");
    let stored = std::fs::read_dir(rig.state.join("store")).unwrap().count();
    std::fs::write(rig.repo.join("src/file-0007.swift"), "let changed = 1\n").unwrap();
    let (req, worktree) = request(&rig, &["sh", "-c", "cat src/file-0007.swift"]);
    let (outcome, out, _) = run(&rig, &req, &worktree);
    assert_eq!((outcome, out.as_str()), (Outcome::Exited(0), "let changed = 1\n"));
    assert_eq!(std::fs::read_dir(rig.state.join("store")).unwrap().count(), stored + 1, "only the changed file travelled");
}
