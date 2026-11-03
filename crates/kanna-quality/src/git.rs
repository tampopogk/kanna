use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(repo).args(args).stdin(Stdio::null()).output().map_err(|e| format!("couldn't run git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub struct Entry {
    pub path: String,
    pub blob: String,
}

pub fn tree(repo: &Path, commit: &str) -> Result<Vec<Entry>, String> {
    Ok(git(repo, &["ls-tree", "-r", "-z", "--full-tree", commit])?
        .split('\0')
        .filter_map(|line| {
            let (head, path) = line.split_once('\t')?;
            let mut words = head.split_whitespace();
            let (_, kind, blob) = (words.next()?, words.next()?, words.next()?);
            (kind == "blob").then(|| Entry { path: path.to_string(), blob: blob.to_string() })
        })
        .collect())
}

pub struct Blobs {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Blobs {
    pub fn open(repo: &Path) -> Result<Blobs, String> {
        let mut child = Command::new("git").arg("-C").arg(PathBuf::from(repo)).args(["cat-file", "--batch"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("couldn't run git cat-file: {e}"))?;
        let input = child.stdin.take().ok_or("no stdin for git cat-file")?;
        let output = BufReader::new(child.stdout.take().ok_or("no stdout for git cat-file")?);
        Ok(Blobs { child, input, output })
    }

    pub fn read(&mut self, blob: &str) -> Result<String, String> {
        writeln!(self.input, "{blob}").and_then(|_| self.input.flush()).map_err(|e| format!("git cat-file: {e}"))?;
        let mut head = String::new();
        self.output.read_line(&mut head).map_err(|e| format!("git cat-file: {e}"))?;
        let size: usize = head.split_whitespace().nth(2).and_then(|s| s.parse().ok()).ok_or_else(|| format!("git cat-file said {head:?}"))?;
        let mut body = vec![0; size + 1];
        self.output.read_exact(&mut body).map_err(|e| format!("git cat-file: {e}"))?;
        body.pop();
        Ok(String::from_utf8_lossy(&body).into_owned())
    }
}

impl Drop for Blobs {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
