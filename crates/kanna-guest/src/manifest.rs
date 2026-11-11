use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path};

pub const SKIPPED: &[&str] = &["target", ".build", "node_modules", "DerivedData"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    File { hash: String, size: u64, exec: bool },
    Link { target: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub kind: Kind,
}

impl Entry {
    pub fn to_json(&self) -> Value {
        match &self.kind {
            Kind::File { hash, size, exec } => json!({ "p": self.path, "h": hash, "n": size, "x": exec }),
            Kind::Link { target } => json!({ "p": self.path, "l": target }),
        }
    }

    pub fn from_json(value: &Value) -> io::Result<Entry> {
        let path = value["p"].as_str().ok_or_else(|| crate::frame::bad("a manifest entry has no path"))?.to_string();
        safe_relative(&path)?;
        let kind = match (value["h"].as_str(), value["l"].as_str()) {
            (Some(hash), None) if is_hash(hash) => Kind::File { hash: hash.into(), size: value["n"].as_u64().unwrap_or(0), exec: value["x"].as_bool().unwrap_or(false) },
            (None, Some(target)) if !target.is_empty() && !target.contains('\0') && target.len() <= 4096 => Kind::Link { target: target.into() },
            _ => return Err(crate::frame::bad(format!("manifest entry {path} is neither a file nor a link"))),
        };
        Ok(Entry { path, kind })
    }
}

pub fn is_hash(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn safe_relative(path: &str) -> io::Result<()> {
    let fine = !path.is_empty() && path.len() <= 4096 && !path.contains('\0') && !path.contains('\\') && Path::new(path).components().all(|c| matches!(c, Component::Normal(_))) && !path.ends_with('/');
    if fine { Ok(()) } else { Err(crate::frame::bad(format!("{path:?} isn't a plain relative path"))) }
}

pub fn encode(entries: &[Entry]) -> Vec<u8> {
    let list: Vec<Value> = entries.iter().map(Entry::to_json).collect();
    serde_json::to_vec(&list).unwrap_or_default()
}

pub fn decode(bytes: &[u8]) -> io::Result<Vec<Entry>> {
    let list: Value = serde_json::from_slice(bytes).map_err(|e| crate::frame::bad(format!("manifest isn't JSON: {e}")))?;
    let entries = list.as_array().ok_or_else(|| crate::frame::bad("manifest isn't a list"))?.iter().map(Entry::from_json).collect::<io::Result<Vec<_>>>()?;
    let mut seen = BTreeSet::new();
    let links: BTreeSet<&str> = entries.iter().filter(|e| matches!(e.kind, Kind::Link { .. })).map(|e| e.path.as_str()).collect();
    for entry in &entries {
        if !seen.insert(entry.path.as_str()) {
            return Err(crate::frame::bad(format!("{} is in the manifest twice", entry.path)));
        }
        let mut prefix = String::new();
        for part in entry.path.split('/').collect::<Vec<_>>().split_last().map(|(_, parents)| parents.to_vec()).unwrap_or_default() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if links.contains(prefix.as_str()) {
                return Err(crate::frame::bad(format!("{} lies under the link {prefix}", entry.path)));
            }
        }
    }
    Ok(entries)
}

pub fn skipped(path: &str) -> bool {
    path.starts_with(".kanna/build/") || path.split('/').any(|part| SKIPPED.contains(&part))
}

pub fn hash_reader(mut from: impl Read, mut copy: Option<&mut dyn Write>) -> io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = from.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        if let Some(out) = copy.as_mut() {
            out.write_all(&buffer[..n])?;
        }
        size += n as u64;
    }
    Ok((hex(&hasher.finalize()), size))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub fn capture(root: &Path, listed: &[String], staging: &Path) -> io::Result<Vec<Entry>> {
    std::fs::create_dir_all(staging)?;
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for path in listed {
        if skipped(path) || safe_relative(path).is_err() || !seen.insert(path.clone()) {
            continue;
        }
        let file = root.join(path);
        let Ok(meta) = std::fs::symlink_metadata(&file) else { continue };
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&file)?;
            entries.push(Entry { path: path.clone(), kind: Kind::Link { target: target.to_string_lossy().into_owned() } });
        } else if meta.is_file() {
            let source = match std::fs::File::open(&file) {
                Ok(source) => source,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            let temp = staging.join(format!(".in-{}", entries.len()));
            let mut out = std::fs::File::create(&temp)?;
            let (hash, size) = hash_reader(source, Some(&mut out))?;
            drop(out);
            std::fs::rename(&temp, staging.join(&hash))?;
            entries.push(Entry { path: path.clone(), kind: Kind::File { hash, size, exec: meta.permissions().mode() & 0o111 != 0 } });
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-guest-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn capture_copies_files_and_links_and_skips_build_folders() {
        let root = temp("capture-root");
        let staging = temp("capture-staging");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("src/a.swift"), "let a = 1\n").unwrap();
        std::fs::write(root.join("run.sh"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.join("target/debug/x"), "x").unwrap();
        std::os::unix::fs::symlink("src/a.swift", root.join("link")).unwrap();
        let listed: Vec<String> = ["src/a.swift", "run.sh", "target/debug/x", "link", "gone.txt", "../escape"].iter().map(|s| s.to_string()).collect();
        let entries = capture(&root, &listed, &staging).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["link", "run.sh", "src/a.swift"]);
        assert_eq!(entries[0].kind, Kind::Link { target: "src/a.swift".into() });
        let Kind::File { hash, exec, .. } = &entries[1].kind else { panic!() };
        assert!(*exec);
        assert_eq!(std::fs::read(staging.join(hash)).unwrap(), b"#!/bin/sh\n");
        assert_eq!(decode(&encode(&entries)).unwrap(), entries);
    }

    #[test]
    fn decode_refuses_paths_that_escape_or_pass_through_links() {
        let hash = "0".repeat(64);
        for bad in [json!([{ "p": "../x", "h": hash }]), json!([{ "p": "/etc/passwd", "h": hash }]), json!([{ "p": "a", "l": "/etc" }, { "p": "a/passwd", "h": hash }]), json!([{ "p": "a", "h": hash }, { "p": "a", "h": hash }]), json!([{ "p": "a", "h": "nothex" }])] {
            assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err(), "{bad}");
        }
    }
}
