use crate::frame::{self, Frame};
use crate::intake::Intake;
use crate::manifest::{self, Entry, Kind};
use crate::paths::{self, Roots};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub trait Pipe: Read + Write + Send {}

impl<T: Read + Write + Send> Pipe for T {}

pub type Connect<'a> = &'a (dyn Fn() -> io::Result<Box<dyn Pipe>> + Sync);

pub const TRANSPORT: i32 = 125;

const RECONNECT_FOR: Duration = Duration::from_secs(600);

const DIAGNOSTICS_SHOWN: usize = 40;

const MAX_FRAME: u64 = 2 << 20;

pub struct Request {
    pub root: PathBuf,
    pub cwd: String,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub fetch: Vec<String>,
    pub ui: bool,
    pub job: String,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Exited(i32),
    Signalled(i32),
    Cancelled,
    Failed(String),
}

impl Outcome {
    pub fn code(&self) -> i32 {
        match self {
            Outcome::Exited(code) => *code,
            Outcome::Signalled(signal) => 128 + signal,
            Outcome::Cancelled => 130,
            Outcome::Failed(_) => TRANSPORT,
        }
    }
}

pub fn new_job() -> String {
    let mut random = [0u8; 8];
    let _ = getrandom::getrandom(&mut random);
    format!("b-{}", manifest::hex(&random))
}

fn git(dir: &Path, args: &[&str]) -> io::Result<Vec<u8>> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).stdin(Stdio::null()).output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(out.stdout)
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).trim().to_string()
}

fn name_of(text: &str) -> String {
    let name: String = text.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect();
    let name = name.trim_start_matches('.').chars().take(48).collect::<String>();
    if name.is_empty() { "repo".into() } else { name }
}

pub struct Worktree {
    pub root: PathBuf,
    pub cwd: String,
    pub commit: String,
    pub repo: String,
    pub task: String,
}

pub fn worktree(dir: &Path) -> io::Result<Worktree> {
    let root = PathBuf::from(text(git(dir, &["rev-parse", "--show-toplevel"]).map_err(|_| io::Error::other(format!("{} isn't in a git worktree; kanna build sends a worktree to the build Mac", dir.display())))?));
    let root = root.canonicalize()?;
    let here = dir.canonicalize()?;
    let cwd = here.strip_prefix(&root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let commit = text(git(&root, &["rev-parse", "HEAD"]).map_err(|_| io::Error::other("the worktree has no commit yet; commit once before building"))?);
    let common = PathBuf::from(text(git(&root, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?));
    let repo_dir = if common.file_name().is_some_and(|n| n == ".git") { common.parent().map(Path::to_path_buf).unwrap_or(common.clone()) } else { common.clone() };
    let repo = name_of(repo_dir.file_name().map(|n| n.to_string_lossy().trim_end_matches(".git").to_string()).unwrap_or_default().as_str());
    let identity = manifest::sha256(format!("{}\0{}", common.display(), root.display()).as_bytes());
    let task = format!("{}-{}", name_of(&root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()), &identity[..8]);
    Ok(Worktree { root, cwd, commit, repo, task })
}

pub fn listed(root: &Path) -> io::Result<Vec<String>> {
    let out = git(root, &["ls-files", "-z", "--cached", "--others", "--exclude-standard"])?;
    Ok(out.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect())
}

const PRODUCTS: &str = ".kanna/build/products";

fn products(root: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(root.join(PRODUCTS)).unwrap_or_default().lines().map(String::from).collect()
}

fn record_product(root: &Path, path: &Path) {
    if path.starts_with(root.join(".kanna/build")) {
        return;
    }
    let mut known = products(root);
    if known.insert(path.to_string_lossy().into_owned()) {
        let _ = std::fs::create_dir_all(root.join(".kanna/build"));
        let _ = std::fs::write(root.join(PRODUCTS), known.into_iter().map(|p| p + "\n").collect::<String>());
    }
}

pub fn results_dir(root: &Path, job: &str) -> PathBuf {
    root.join(".kanna/build").join(job)
}

struct Receiver<'a> {
    request: &'a Request,
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
    at: [u64; 2],
    map: Vec<(String, String)>,
    lines: [Vec<u8>; 2],
    diagnostics: Vec<String>,
    expected_to: BTreeSet<String>,
    status: Value,
    exited: bool,
    notes: Vec<String>,
}

impl Receiver<'_> {
    fn output(&mut self, fd: usize, bytes: &[u8]) -> io::Result<()> {
        if fd == 1 { self.out.write_all(bytes)?; self.out.flush()? } else { self.err.write_all(bytes)?; self.err.flush()? }
        let lines = &mut self.lines[fd - 1];
        lines.extend_from_slice(bytes);
        while let Some(at) = lines.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = lines.drain(..=at).collect();
            let line = String::from_utf8_lossy(&line);
            self.diagnostics.extend(paths::diagnostics(line.trim_end_matches(['\n', '\r']), &self.map));
        }
        if lines.len() > 1 << 16 {
            lines.clear();
        }
        Ok(())
    }

    fn destination(&self, header: &Value) -> Option<(PathBuf, String)> {
        let root = &self.request.root;
        let inside = |path: &Path| path.strip_prefix(root).ok().map(|rel| rel.to_string_lossy().into_owned()).filter(|rel| !rel.is_empty());
        if let Some(to) = header["to"].as_str() {
            if !self.expected_to.contains(to) {
                return None;
            }
            let to = Path::new(to);
            return match inside(to) {
                Some(rel) => Some((root.clone(), rel)),
                None => Some((to.parent()?.to_path_buf(), to.file_name()?.to_string_lossy().into_owned())),
            };
        }
        if let Some(name) = header["name"].as_str() {
            let rel = format!(".kanna/build/{}/{name}", self.request.job);
            return (manifest::safe_relative(name).is_ok() && !name.contains('/')).then(|| (root.clone(), rel));
        }
        if let Some(rel) = header["fetch"].as_str() {
            return self.request.fetch.iter().any(|f| f == rel).then(|| (root.clone(), rel.to_string()));
        }
        None
    }

    fn may_replace(&self, dest: &Path) -> bool {
        dest.starts_with(self.request.root.join(".kanna/build")) && !dest.starts_with(self.request.root.join(PRODUCTS))
            || products(&self.request.root).contains(&dest.to_string_lossy().into_owned())
    }

    fn artifact(&mut self, conn: &mut dyn Pipe, header: &Value) -> io::Result<()> {
        if header["missing"] == true {
            let what = header["fetch"].as_str().or(header["name"].as_str()).unwrap_or("an artifact");
            self.notes.push(format!("{what} wasn't produced on the build Mac"));
            return Ok(());
        }
        let destination = self.destination(header);
        let single = header["dir"] != true;
        let mut intake = match &destination {
            Some((base, rel)) => match Intake::new(base, rel, single) {
                Ok(intake) => Some(intake),
                Err(e) => {
                    self.notes.push(format!("refused an artifact: {e}"));
                    None
                }
            },
            None => {
                self.notes.push(format!("refused an artifact the job didn't ask for: {header}"));
                None
            }
        };
        let mut failure: Option<String> = None;
        loop {
            let frame = frame::read_header(conn, crate::intake::MAX_FILE)?;
            let rel = frame.str("path").unwrap_or_default().to_string();
            let mut payload = Counted { inner: (&mut *conn).take(frame.size), read: 0 };
            let result = match (frame.op(), intake.as_mut()) {
                ("end", _) => break,
                (_, Some(intake)) if failure.is_none() => match frame.op() {
                    "file" => intake.file(&rel, frame.header["exec"] == true, frame.size, frame.str("sha").unwrap_or_default(), &mut payload),
                    "dir" => intake.dir(&rel),
                    "link" => intake.link(&rel, frame.str("target").unwrap_or_default()),
                    other => return Err(frame::bad(format!("unexpected {other:?} inside an artifact"))),
                },
                ("file" | "dir" | "link", _) => Ok(()),
                (other, _) => return Err(frame::bad(format!("unexpected {other:?} inside an artifact"))),
            };
            let left = frame.size - payload.read;
            io::copy(&mut payload.inner.take(left), &mut io::sink())?;
            match result {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => failure = Some(e.to_string()),
                Err(e) => return Err(e),
            }
        }
        if let Some(intake) = intake {
            match failure {
                Some(why) => {
                    intake.abandon();
                    self.notes.push(format!("refused an artifact: {why}"));
                }
                None => match {
                    let dest = destination.as_ref().map(|(base, rel)| base.join(rel)).unwrap_or_default();
                    let replace = self.may_replace(&dest);
                    intake.finish(replace)
                } {
                    Ok(path) => {
                        record_product(&self.request.root, &path);
                        self.notes.push(format!("{}", path.display()));
                    }
                    Err(e) => self.notes.push(format!("couldn't keep an artifact: {e}")),
                },
            }
        }
        Ok(())
    }

    fn receive(&mut self, conn: &mut dyn Pipe) -> io::Result<Option<Outcome>> {
        loop {
            let frame = frame::read_header(conn, MAX_FRAME)?;
            match frame.op() {
                "started" => {
                    let payload = frame::read_payload(conn, &frame)?;
                    let started: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
                    self.map = started["map"].as_array().into_iter().flatten().filter_map(|p| Some((p[0].as_str()?.to_string(), p[1].as_str()?.to_string()))).collect();
                }
                "out" => {
                    let fd = frame.header["fd"].as_u64().unwrap_or(1).clamp(1, 2) as usize;
                    let at = frame.header["at"].as_u64().unwrap_or(0);
                    let payload = frame::read_payload(conn, &frame)?;
                    let end = at.checked_add(payload.len() as u64).ok_or_else(|| frame::bad("output offset overflows"))?;
                    if end > self.at[fd - 1] {
                        let skip = self.at[fd - 1].saturating_sub(at) as usize;
                        self.output(fd, &payload[skip.min(payload.len())..])?;
                        self.at[fd - 1] = end;
                    }
                }
                "exit" => {
                    self.status = frame.header["status"].clone();
                    self.exited = true;
                }
                "artifact" => {
                    let header = frame.header.clone();
                    self.artifact(conn, &header)?;
                }
                "done" => {
                    self.status = frame.header["status"].clone();
                    return Ok(Some(outcome(&self.status)));
                }
                "error" => return Ok(Some(Outcome::Failed(frame.str("error").unwrap_or("the build Mac failed").to_string()))),
                "exists" => {}
                other => return Err(frame::bad(format!("unexpected {other:?} from the build Mac"))),
            }
        }
    }
}

fn outcome(status: &Value) -> Outcome {
    match status["phase"].as_str() {
        Some("exited") => Outcome::Exited(status["code"].as_i64().unwrap_or(1) as i32),
        Some("signalled") => Outcome::Signalled(status["signal"].as_i64().unwrap_or(9) as i32),
        Some("cancelled") => Outcome::Cancelled,
        _ => Outcome::Failed(status["error"].as_str().unwrap_or("the job failed on the build Mac").to_string()),
    }
}

fn send_snapshot(conn: &mut dyn Pipe, root: &Path, staging: &Path, entries: &[Entry], need: &Frame, lists: &[u8]) -> io::Result<()> {
    let lists: Value = serde_json::from_slice(lists).map_err(|e| frame::bad(format!("the build Mac's list of files isn't JSON: {e}")))?;
    if need.header["commit"] != true {
        let haves: Vec<String> = lists["heads"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|h| git(root, &["cat-file", "-e", &format!("{h}^{{commit}}")]).is_ok()).map(|h| format!("^{h}")).collect();
        let bundle = staging.join(".bundle");
        let mut args = vec!["bundle".to_string(), "create".into(), "-q".into(), bundle.to_string_lossy().into_owned(), "HEAD".into()];
        args.extend(haves);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        git(root, &args)?;
        let size = std::fs::metadata(&bundle)?.len();
        frame::send_reader(conn, &json!({ "op": "bundle" }), size, &mut std::fs::File::open(&bundle)?)?;
    }
    let sizes: BTreeMap<&str, u64> = entries.iter().filter_map(|e| match &e.kind { Kind::File { hash, size, .. } => Some((hash.as_str(), *size)), _ => None }).collect();
    for hash in lists["blobs"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let Some(size) = sizes.get(hash) else { return Err(frame::bad(format!("the build Mac asked for {hash}, which isn't in the snapshot"))) };
        frame::send_reader(conn, &json!({ "op": "blob", "hash": hash }), *size, &mut std::fs::File::open(staging.join(hash))?)?;
    }
    frame::send(conn, &json!({ "op": "sent" }), b"")
}

pub fn run(connect: Connect, request: &Request, worktree: &Worktree, out: &mut dyn Write, err: &mut dyn Write) -> Outcome {
    let staging = std::env::temp_dir().join(format!("kanna-build-{}", request.job));
    let result = submit_and_follow(connect, request, worktree, &staging, out, err);
    let _ = std::fs::remove_dir_all(&staging);
    if let Outcome::Failed(why) = &result {
        let _ = writeln!(err, "kanna build: {why}");
    }
    result
}

fn submit_and_follow(connect: Connect, request: &Request, worktree: &Worktree, staging: &Path, out: &mut dyn Write, err: &mut dyn Write) -> Outcome {
    let listed = match listed(&request.root) {
        Ok(listed) => listed,
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    let entries = match manifest::capture(&request.root, &listed, staging) {
        Ok(entries) => entries,
        Err(e) => return Outcome::Failed(format!("can't take the worktree's snapshot: {e}")),
    };
    let expected_to: BTreeSet<String> = {
        let fake = PathBuf::from("/kanna-guest");
        let roots = Roots { linux: &request.root.to_string_lossy(), cwd: &request.cwd, src: &fake.join("src"), results: &fake.join("results"), cache: &fake.join("cache") };
        match paths::translate(&request.argv, &roots) {
            Ok(t) => t.results.into_iter().map(|(_, linux)| linux).collect(),
            Err(e) => return Outcome::Failed(e),
        }
    };
    let header = json!({
        "op": "run", "job": request.job, "mirror": { "machine": "local", "repo": worktree.repo, "task": worktree.task },
        "commit": worktree.commit, "root": request.root, "cwd": request.cwd, "argv": request.argv, "env": request.env, "fetch": request.fetch, "ui": request.ui,
    });
    let mut receiver = Receiver { request, out, err, at: [0, 0], map: vec![], lines: [vec![], vec![]], diagnostics: vec![], expected_to, status: Value::Null, exited: false, notes: vec![] };
    let submitted = (|| -> io::Result<Option<(Box<dyn Pipe>, bool)>> {
        let mut conn = connect()?;
        frame::send(&mut conn, &header, &manifest::encode(&entries))?;
        let (reply, payload) = frame::read(&mut conn, 64 << 20)?;
        match reply.op() {
            "need" => {
                send_snapshot(&mut *conn, &request.root, staging, &entries, &reply, &payload)?;
                Ok(Some((conn, true)))
            }
            "exists" => Ok(Some((conn, true))),
            "error" => Err(io::Error::other(reply.str("error").unwrap_or("refused").to_string())),
            other => Err(frame::bad(format!("unexpected {other:?} ({} bytes) from the build Mac", payload.len()))),
        }
    })();
    let mut conn = match submitted {
        Ok(Some((conn, _))) => conn,
        Ok(None) => return Outcome::Failed("the build Mac refused the job".into()),
        Err(e) => return Outcome::Failed(format!("couldn't send the job to the build Mac: {e}")),
    };
    let _ = std::fs::remove_dir_all(staging);
    let deadline = || Instant::now() + RECONNECT_FOR;
    let mut give_up = deadline();
    let outcome = loop {
        match receiver.receive(&mut *conn) {
            Ok(Some(outcome)) => break outcome,
            Ok(None) => {}
            Err(_) => {}
        }
        let attach = loop {
            if Instant::now() > give_up {
                break None;
            }
            std::thread::sleep(Duration::from_secs(1));
            if let Ok(mut fresh) = connect() {
                if frame::send(&mut fresh, &json!({ "op": "attach", "job": request.job, "at": receiver.at }), b"").is_ok() {
                    break Some(fresh);
                }
            }
        };
        match attach {
            Some(fresh) => {
                conn = fresh;
                give_up = deadline();
            }
            None => break Outcome::Failed(format!("lost the build Mac and couldn't reach it again within {}s; job {} may still be running there", RECONNECT_FOR.as_secs(), request.job)),
        }
    };
    finish(&mut receiver, &outcome);
    outcome
}

fn finish(receiver: &mut Receiver, outcome: &Outcome) {
    let results = results_dir(&receiver.request.root, &receiver.request.job);
    let _ = std::fs::create_dir_all(&results);
    let record = json!({ "job": receiver.request.job, "argv": receiver.request.argv, "cwd": receiver.request.cwd, "status": receiver.status, "artifacts": receiver.notes, "code": outcome.code() });
    let _ = std::fs::write(results.join("job.json"), serde_json::to_vec_pretty(&record).unwrap_or_default());
    let _ = std::fs::write(receiver.request.root.join(".kanna/build/last"), format!("{}\n", receiver.request.job));
    for fd in 0..2 {
        if !receiver.lines[fd].is_empty() {
            let rest = String::from_utf8_lossy(&std::mem::take(&mut receiver.lines[fd])).into_owned();
            let found = paths::diagnostics(&rest, &receiver.map);
            receiver.diagnostics.extend(found);
        }
    }
    let shown = receiver.root_relative();
    if !receiver.diagnostics.is_empty() {
        let _ = std::fs::write(results.join("diagnostics.txt"), receiver.diagnostics.join("\n") + "\n");
        let _ = writeln!(receiver.err, "kanna build: {} diagnostics with this worktree's paths ({shown}/diagnostics.txt):", receiver.diagnostics.len());
        for line in receiver.diagnostics.iter().take(DIAGNOSTICS_SHOWN) {
            let _ = writeln!(receiver.err, "{line}");
        }
    }
    let refused: Vec<&String> = receiver.notes.iter().filter(|n| n.starts_with("refused") || n.starts_with("couldn't") || n.contains("wasn't produced")).collect();
    for note in refused {
        let _ = writeln!(receiver.err, "kanna build: {note}");
    }
    let _ = receiver.err.flush();
}

impl Receiver<'_> {
    fn root_relative(&self) -> String {
        format!(".kanna/build/{}", self.request.job)
    }
}

struct Counted<R> {
    inner: R,
    read: u64,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        Ok(n)
    }
}

pub fn cancel(connect: Connect, job: &str) -> io::Result<()> {
    let mut conn = connect()?;
    frame::send(&mut conn, &json!({ "op": "cancel", "job": job }), b"")?;
    frame::read(&mut conn, 1 << 20).map(drop)
}
