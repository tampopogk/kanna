use crate::node::{read_line, refusal, write_line, Failure, Node, MAX_LINE};
use crate::{hex, UPDATE};
use iroh::endpoint::{RecvStream, SendStream};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

pub const CURRENT: &str = "current";

pub const PREVIOUS: &str = "previous";

pub const RELEASE: &str = "release.json";

const FILES: &[&str] = &["kanna", "kanna-daemon", "kanna-keeper", RELEASE];

const MAX_TARBALL: u64 = 1 << 30;

const PROBE: Duration = Duration::from_secs(10);

const BACK: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct Install {
    pub prefix: PathBuf,
}

pub fn valid_commit(commit: &str) -> bool {
    let hash = commit.strip_suffix("-dirty").unwrap_or(commit);
    hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub fn protocols(value: &Value) -> Option<[u32; 2]> {
    let pair = value.as_array()?;
    let lo = u32::try_from(pair.first()?.as_u64()?).ok()?;
    let hi = u32::try_from(pair.get(1)?.as_u64()?).ok()?;
    (pair.len() == 2 && lo <= hi).then_some([lo, hi])
}

pub fn common(mine: [u32; 2], theirs: [u32; 2]) -> Option<u32> {
    let top = mine[1].min(theirs[1]);
    (top >= mine[0].max(theirs[0])).then_some(top)
}

pub fn short(commit: &str) -> String {
    let dirty = if commit.ends_with("-dirty") { "-dirty" } else { "" };
    format!("{}{dirty}", commit.chars().take(8).collect::<String>())
}

fn span(p: [u32; 2]) -> String {
    if p[0] == p[1] { p[0].to_string() } else { format!("{}–{}", p[0], p[1]) }
}

pub fn other_protocol(name: &str, theirs: &Value, mine: [u32; 2]) -> String {
    let their = protocols(&theirs["protocols"]).unwrap_or([1, 1]);
    let runs = match (theirs["version"].as_str(), theirs["commit"].as_str()) {
        (Some(version), Some(commit)) => format!("Kanna {version} ({})", short(commit)),
        (Some(version), None) => format!("Kanna {version}"),
        _ => "an older Kanna".into(),
    };
    let fix = if their[1] < mine[0] { format!("it's older: kanna fleet update {name}") } else { format!("this machine is older: update it, or run kanna fleet update on {name}") };
    format!("{name} runs {runs}, protocol {}, and this machine speaks {}; {fix}", span(their), span(mine))
}

impl Install {
    pub fn detect(exe: &Path) -> Option<Install> {
        let exe = exe.canonicalize().ok()?;
        let dir = exe.parent()?;
        let prefix = dir.parent()?;
        let current = prefix.join(CURRENT).canonicalize().ok()?;
        (current == dir && dir.file_name().and_then(|n| n.to_str()).is_some_and(valid_commit)).then(|| Install { prefix: prefix.to_path_buf() })
    }

    pub fn current(&self) -> Option<String> {
        std::fs::read_link(self.prefix.join(CURRENT)).ok().and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned()))
    }

    pub fn daemon(&self) -> PathBuf {
        self.prefix.join(CURRENT).join("kanna-daemon")
    }

    pub fn put(&self, tarball: &Path, commit: &str) -> Result<Option<String>, Failure> {
        if !valid_commit(commit) {
            return Err(Failure::new("bad_request", format!("\"{commit}\" isn't a commit")));
        }
        let failed = |what: &str, e: std::io::Error| Failure::new("install_failed", format!("{what}: {e}"));
        let dest = self.prefix.join(commit);
        let staged = self.prefix.join(format!(".{commit}.new"));
        let _ = std::fs::remove_dir_all(&staged);
        std::fs::create_dir_all(&staged).map_err(|e| failed(&format!("can't make {}", staged.display()), e))?;
        let untar = std::process::Command::new("tar").arg("-xzf").arg(tarball).arg("-C").arg(&staged).arg("--strip-components=1").arg("--no-same-owner").output().map_err(|e| failed("can't run tar", e))?;
        let check = || -> Result<(), Failure> {
            if !untar.status.success() {
                return Err(Failure::new("install_failed", format!("tar couldn't unpack it: {}", String::from_utf8_lossy(&untar.stderr).trim())));
            }
            for file in FILES {
                if !staged.join(file).is_file() {
                    return Err(Failure::new("install_failed", format!("the tarball has no {file}")));
                }
            }
            let release: Value = std::fs::read(staged.join(RELEASE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
            if release["commit"] != commit {
                return Err(Failure::new("install_failed", format!("the tarball's release.json is for {}, not {commit}", release["commit"])));
            }
            probe(&staged.join("kanna-daemon"))
        };
        if let Err(failure) = check() {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(failure);
        }
        let was = self.current();
        if dest.exists() {
            let old = self.prefix.join(format!(".{commit}.old"));
            let _ = std::fs::remove_dir_all(&old);
            std::fs::rename(&dest, &old).map_err(|e| failed("can't move the old copy aside", e))?;
            std::fs::rename(&staged, &dest).map_err(|e| failed("can't move it into place", e))?;
            let _ = std::fs::remove_dir_all(&old);
        } else {
            std::fs::rename(&staged, &dest).map_err(|e| failed("can't move it into place", e))?;
        }
        if let Some(was) = was.as_deref().filter(|w| *w != commit) {
            relink(&self.prefix, PREVIOUS, was).map_err(|e| failed("can't point previous at the old one", e))?;
        }
        relink(&self.prefix, CURRENT, commit).map_err(|e| failed("can't switch current", e))?;
        self.prune(commit);
        Ok(was)
    }

    fn prune(&self, commit: &str) {
        let previous = std::fs::read_link(self.prefix.join(PREVIOUS)).ok().and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned()));
        let Ok(entries) = std::fs::read_dir(&self.prefix) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == commit || Some(&name) == previous.as_ref() || !valid_commit(&name) {
                continue;
            }
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

fn relink(prefix: &Path, name: &str, target: &str) -> std::io::Result<()> {
    let staged = prefix.join(format!("{name}.new"));
    let _ = std::fs::remove_file(&staged);
    std::os::unix::fs::symlink(target, &staged)?;
    std::fs::rename(&staged, prefix.join(name))
}

fn probe(daemon: &Path) -> Result<(), Failure> {
    let mut child = std::process::Command::new(daemon).arg("--uses-keeper").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map_err(|e| Failure::new("install_failed", format!("its kanna-daemon doesn't run here: {e}")))?;
    let deadline = Instant::now() + PROBE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(Failure::new("install_failed", format!("its kanna-daemon doesn't run here ({status})"))),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::new("install_failed", "its kanna-daemon didn't answer --uses-keeper"));
            }
        }
    }
}

pub struct Tarball {
    pub path: PathBuf,
    pub release: Value,
    pub sha256: String,
    pub size: u64,
}

impl Tarball {
    pub async fn open(path: &Path) -> Result<Tarball, Failure> {
        let unreadable = |e: String| Failure::new("bad_tarball", format!("can't read {}: {e}", path.display()));
        let listed = tokio::process::Command::new("tar").arg("-xzOf").arg(path).arg(format!("kanna/{RELEASE}")).output().await.map_err(|e| unreadable(e.to_string()))?;
        if !listed.status.success() {
            return Err(unreadable(format!("it has no kanna/{RELEASE} ({})", String::from_utf8_lossy(&listed.stderr).trim())));
        }
        let release: Value = serde_json::from_slice(&listed.stdout).map_err(|e| unreadable(format!("its {RELEASE} isn't JSON: {e}")))?;
        if !release["commit"].as_str().is_some_and(valid_commit) || protocols(&release["protocols"]).is_none() || !release["os"].is_string() || !release["arch"].is_string() {
            return Err(unreadable(format!("its {RELEASE} doesn't name a commit, protocols, os and arch")));
        }
        let mut file = tokio::fs::File::open(path).await.map_err(|e| unreadable(e.to_string()))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0; 1 << 16];
        let mut size = 0u64;
        loop {
            let n = file.read(&mut buffer).await.map_err(|e| unreadable(e.to_string()))?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            size += n as u64;
        }
        Ok(Tarball { path: path.to_path_buf(), release, sha256: hex(&hasher.finalize()), size })
    }
}

impl Node {
    pub async fn update(&self, hosts: Vec<String>, tarball: Tarball) -> Result<Value, Failure> {
        self.usable()?;
        let me = self.key();
        let (os, arch) = (tarball.release["os"].as_str().unwrap_or_default().to_string(), tarball.release["arch"].as_str().unwrap_or_default().to_string());
        let named = !hosts.is_empty();
        let keys: Vec<String> = if named {
            hosts.iter().map(|h| self.resolve(h)).collect::<Result<_, _>>()?
        } else {
            self.trusted().into_iter().filter(|k| *k != me).collect()
        };
        if named && keys.contains(&me) {
            return Err(Failure::new("bad_request", "this machine can't update itself with kanna fleet update"));
        }
        let tarball = std::sync::Arc::new(tarball);
        let mut running = vec![];
        for key in keys {
            let name = self.name_of(&key);
            let caps = self.capabilities_of(&key);
            if caps["os"].as_str().is_some_and(|o| o != os) || caps["arch"].as_str().is_some_and(|a| a != arch) {
                let theirs = format!("{} {}", caps["os"].as_str().unwrap_or("?"), caps["arch"].as_str().unwrap_or("?"));
                let why = if caps["os"] == "macos" { format!("{theirs}: Kanna.app updates it") } else { format!("{theirs}: this tarball is for {os} {arch}") };
                running.push(tokio::spawn(async move { json!({ "name": name, "ok": named.then_some(false), "skipped": why }) }));
                continue;
            }
            let (node, tarball) = (self.clone(), tarball.clone());
            running.push(tokio::spawn(async move {
                match node.update_one(&key, &name, &tarball).await {
                    Ok(done) => done,
                    Err(failure) => json!({ "name": name, "ok": false, "error": { "code": failure.code, "message": failure.message } }),
                }
            }));
        }
        let mut machines = vec![];
        for handle in running {
            machines.push(handle.await.unwrap_or_else(|e| json!({ "ok": false, "error": { "code": "failed", "message": e.to_string() } })));
        }
        Ok(json!({ "release": tarball.release, "machines": machines }))
    }

    async fn update_one(&self, key: &str, name: &str, tarball: &Tarball) -> Result<Value, Failure> {
        let started = Instant::now();
        let commit = tarball.release["commit"].as_str().unwrap_or_default().to_string();
        let header = json!({ "release": tarball.release, "sha256": tarball.sha256, "size": tarball.size });
        let (mut send, mut recv, ack) = self.open_to(key, UPDATE, header).await.map_err(|failure| match failure.code.as_str() {
            "unknown_stream" => Failure::new("unknown_stream", format!("{name}'s Kanna predates kanna fleet update: run scripts/server-setup.sh there once")),
            _ => failure,
        })?;
        let from = ack["release"]["commit"].as_str().map(String::from);
        if ack["current"] == true {
            let _ = send.finish();
            return Ok(json!({ "name": name, "ok": true, "from": from, "to": commit, "already": true }));
        }
        let lost = |e: String| Failure::new("unreachable", format!("lost {name} while sending the update: {e}"));
        let mut file = tokio::fs::File::open(&tarball.path).await.map_err(|e| Failure::new("bad_tarball", format!("can't read {}: {e}", tarball.path.display())))?;
        tokio::io::copy(&mut file, &mut send).await.map_err(|e| lost(e.to_string()))?;
        send.flush().await.map_err(|e| lost(e.to_string()))?;
        let sent = started.elapsed().as_secs_f64();
        let reply = read_line(&mut recv, MAX_LINE).await.map_err(lost)?;
        let _ = send.finish();
        if reply["ok"] != true {
            return Err(Failure::from_reply(&reply));
        }
        let installed = Instant::now();
        self.forget(key);
        let back = self.back_at(key, &commit).await;
        Ok(json!({ "name": name, "ok": back.is_ok(), "from": from, "to": commit, "bytes": tarball.size, "sent_secs": sent, "back_secs": installed.elapsed().as_secs_f64(), "error": back.err().map(|f| json!({ "code": f.code, "message": f.message })) }))
    }

    async fn back_at(&self, key: &str, commit: &str) -> Result<(), Failure> {
        let deadline = Instant::now() + BACK;
        let mut last = None;
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(200)).await;
            match self.release_of(key).await {
                Ok(release) if release["commit"] == commit => return Ok(()),
                Ok(release) => last = release["commit"].as_str().map(String::from),
                Err(_) => self.forget(key),
            }
        }
        Err(Failure::new("not_back", match last {
            Some(other) => format!("it came back at {}, not {}", short(&other), short(commit)),
            None => format!("it didn't come back within {}s: journalctl --user -u kanna-daemon there", BACK.as_secs()),
        }))
    }

    pub(crate) async fn receive_update(&self, header: &Value, send: &mut SendStream, recv: &mut BufReader<RecvStream>) {
        let answer = match self.accept_update(header) {
            Ok(answer) => answer,
            Err(failure) => {
                let _ = write_line(send, &refusal(&failure)).await;
                return;
            }
        };
        let Some((install, commit)) = answer else {
            let _ = write_line(send, &json!({ "ok": true, "kind": UPDATE, "name": self.state().name, "release": self.0.release, "current": true })).await;
            return;
        };
        let result = self.take_update(&install, &commit, header, send, recv).await;
        self.0.updating.store(false, std::sync::atomic::Ordering::SeqCst);
        match result {
            Ok(was) => {
                let _ = write_line(send, &json!({ "ok": true, "from": was, "to": commit })).await;
                let _ = send.finish();
                let _ = tokio::time::timeout(Duration::from_secs(2), send.stopped()).await;
                self.0.host.restart();
            }
            Err(failure) => {
                let _ = write_line(send, &refusal(&failure)).await;
            }
        }
    }

    fn accept_update(&self, header: &Value) -> Result<Option<(Install, String)>, Failure> {
        let name = self.state().name.clone();
        let Some(install) = self.0.install.clone() else {
            return Err(Failure::new("not_installed", format!("{name}'s Kanna isn't installed in /opt/kanna/COMMIT with current beside it (a Mac updates with Kanna.app; a server is set up with scripts/server-setup.sh)")));
        };
        let release = &header["release"];
        let commit = release["commit"].as_str().filter(|c| valid_commit(c)).ok_or_else(|| Failure::new("bad_request", "the update names no commit"))?.to_string();
        for field in ["os", "arch", "channel"] {
            if release[field] != self.0.release[field] {
                return Err(Failure::new("wrong_build", format!("{name} is {} {} on the {} channel, and the update is {} {} on {}", self.0.release["os"], self.0.release["arch"], self.0.release["channel"], release["os"], release["arch"], release["channel"])));
            }
        }
        header["size"].as_u64().filter(|s| *s > 0 && *s <= MAX_TARBALL).ok_or_else(|| Failure::new("bad_request", "the update's size is missing or over 1 GiB"))?;
        if !header["sha256"].as_str().is_some_and(|s| s.len() == 64) {
            return Err(Failure::new("bad_request", "the update has no sha256"));
        }
        if install.current().as_deref() == Some(commit.as_str()) && !commit.ends_with("-dirty") {
            return Ok(None);
        }
        if self.0.updating.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Err(Failure::new("busy", format!("{name} is taking another update")));
        }
        Ok(Some((install, commit)))
    }

    async fn take_update(&self, install: &Install, commit: &str, header: &Value, send: &mut SendStream, recv: &mut BufReader<RecvStream>) -> Result<Option<String>, Failure> {
        let size = header["size"].as_u64().unwrap_or_default();
        let ack = json!({ "ok": true, "kind": UPDATE, "name": self.state().name, "release": self.0.release, "send": true });
        write_line(send, &ack).await.map_err(|e| Failure::new("unreachable", e))?;
        let path = install.prefix.join(format!(".{commit}.tar.gz"));
        let received = receive(recv, &path, size).await;
        let received = match received {
            Ok(sha) if Some(sha.as_str()) == header["sha256"].as_str() => Ok(()),
            Ok(_) => Err(Failure::new("bad_tarball", "the tarball's sha256 doesn't match what was sent")),
            Err(e) => Err(Failure::new("unreachable", format!("lost the update: {e}"))),
        };
        let installed = match received {
            Ok(()) => {
                let (install, path, commit) = (install.clone(), path.clone(), commit.to_string());
                tokio::task::spawn_blocking(move || install.put(&path, &commit)).await.unwrap_or_else(|e| Err(Failure::new("install_failed", e.to_string())))
            }
            Err(failure) => Err(failure),
        };
        let _ = tokio::fs::remove_file(&path).await;
        installed
    }
}

async fn receive(recv: &mut BufReader<RecvStream>, path: &Path, size: u64) -> Result<String, String> {
    let mut file = tokio::fs::File::create(path).await.map_err(|e| format!("can't write {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    let mut left = size;
    while left > 0 {
        let want = buffer.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let n = recv.read(&mut buffer[..want]).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err(format!("it ended {left} bytes short"));
        }
        hasher.update(&buffer[..n]);
        file.write_all(&buffer[..n]).await.map_err(|e| e.to_string())?;
        left -= n as u64;
    }
    file.sync_all().await.map_err(|e| e.to_string())?;
    Ok(hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocols_meet_at_the_highest_both_speak() {
        assert_eq!(common([1, 3], [2, 5]), Some(3));
        assert_eq!(common([1, 1], [1, 1]), Some(1));
        assert_eq!(common([1, 2], [3, 4]), None);
        assert_eq!(protocols(&json!([2, 1])), None);
        assert_eq!(protocols(&json!([1, 2])), Some([1, 2]));
    }

    #[test]
    fn the_message_says_which_side_is_older() {
        let old = json!({ "protocols": [1, 1], "version": "4.0.1", "commit": "d7cb0a1e".repeat(5) });
        assert_eq!(other_protocol("r720-1", &old, [2, 3]), "r720-1 runs Kanna 4.0.1 (d7cb0a1e), protocol 1, and this machine speaks 2–3; it's older: kanna fleet update r720-1");
        let new = json!({ "protocols": [4, 5], "version": "4.2.0" });
        assert_eq!(other_protocol("vm", &new, [2, 3]), "vm runs Kanna 4.2.0, protocol 4–5, and this machine speaks 2–3; this machine is older: update it, or run kanna fleet update on vm");
    }

    #[test]
    fn only_full_commits_name_an_install() {
        assert!(valid_commit(&"a".repeat(40)));
        assert!(valid_commit(&format!("{}-dirty", "0".repeat(40))));
        assert!(!valid_commit("../etc"));
        assert!(!valid_commit(CURRENT));
        assert!(!valid_commit(&"A".repeat(40)));
    }
}
