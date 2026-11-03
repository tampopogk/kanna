use crate::manifest::hash_reader;
use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{FromRawFd, OwnedFd, AsRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub const MAX_FILE: u64 = 4 << 30;

pub const MAX_TOTAL: u64 = 16 << 30;

pub const MAX_FILES: usize = 200_000;

const MAX_COMPONENT: usize = 255;

pub fn rejected(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message.into())
}

pub fn parts(rel: &str) -> io::Result<Vec<&str>> {
    if rel.is_empty() || rel.len() > 4096 || rel.contains('\0') || rel.contains('\\') || rel.starts_with('/') {
        return Err(rejected(format!("{rel:?} isn't a plain relative path")));
    }
    let parts: Vec<&str> = rel.split('/').collect();
    for part in &parts {
        if part.is_empty() || *part == "." || *part == ".." || part.len() > MAX_COMPONENT || part.chars().any(char::is_control) {
            return Err(rejected(format!("{rel:?} has a component that isn't allowed")));
        }
    }
    Ok(parts)
}

fn cstring(text: &str) -> io::Result<CString> {
    CString::new(text).map_err(|_| rejected("NUL in a name"))
}

fn open_dir_at(dir: &OwnedFd, name: &str, create: bool) -> io::Result<OwnedFd> {
    let name = cstring(name)?;
    if create && unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o755) } != 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error);
        }
    }
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(rejected(format!("{} isn't a folder: {}", name.to_string_lossy(), io::Error::last_os_error())));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn open_base(path: &Path) -> io::Result<OwnedFd> {
    let name = CString::new(path.as_os_str().as_bytes()).map_err(|_| rejected("NUL in a path"))?;
    let fd = unsafe { libc::open(name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn random_name(prefix: &str) -> String {
    let mut random = [0u8; 6];
    let _ = getrandom::getrandom(&mut random);
    format!("{prefix}{}", crate::manifest::hex(&random))
}

fn exists_at(dir: &OwnedFd, name: &CString) -> bool {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    unsafe { libc::fstatat(dir.as_raw_fd(), name.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) == 0 }
}

fn rename_at(dir: &OwnedFd, from: &str, to: &str) -> io::Result<()> {
    let (from, to) = (cstring(from)?, cstring(to)?);
    if unsafe { libc::renameat(dir.as_raw_fd(), from.as_ptr(), dir.as_raw_fd(), to.as_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub struct Intake {
    folder: PathBuf,
    parent: OwnedFd,
    staging: String,
    name: String,
    dest: PathBuf,
    root: OwnedFd,
    files: BTreeSet<String>,
    dirs: BTreeSet<String>,
    total: u64,
    single: bool,
}

impl Intake {
    pub fn new(base: &Path, rel: &str, single: bool) -> io::Result<Intake> {
        let steps = parts(rel)?;
        let mut parent = open_base(base)?;
        for step in &steps[..steps.len() - 1] {
            parent = open_dir_at(&parent, step, true)?;
        }
        let folder = steps[..steps.len() - 1].iter().fold(base.to_path_buf(), |path, step| path.join(step));
        let staging = random_name(".kanna-in-");
        let root = open_dir_at(&parent, &staging, true)?;
        let name = steps[steps.len() - 1].to_string();
        Ok(Intake { dest: folder.join(&name), folder, parent, staging, name, root, files: BTreeSet::new(), dirs: BTreeSet::new(), total: 0, single })
    }

    fn claim(&mut self, rel: &str, dir: bool) -> io::Result<Vec<String>> {
        let parts: Vec<String> = parts(rel)?.into_iter().map(String::from).collect();
        let folded = rel.to_lowercase();
        let mut prefix = String::new();
        for part in &parts[..parts.len() - 1] {
            prefix = if prefix.is_empty() { part.to_lowercase() } else { format!("{prefix}/{}", part.to_lowercase()) };
            if self.files.contains(&prefix) {
                return Err(rejected(format!("{rel} lies under the file {prefix}")));
            }
            self.dirs.insert(prefix.clone());
        }
        if self.files.contains(&folded) || (!dir && self.dirs.contains(&folded)) {
            return Err(rejected(format!("{rel} collides with another returned file")));
        }
        if self.files.len() + self.dirs.len() + parts.len() > MAX_FILES {
            return Err(rejected("too many returned files"));
        }
        if dir { self.dirs.insert(folded) } else { self.files.insert(folded) };
        Ok(parts)
    }

    fn parent(&self, parts: &[String]) -> io::Result<OwnedFd> {
        let mut dir = self.root.try_clone()?;
        for part in &parts[..parts.len() - 1] {
            dir = open_dir_at(&dir, part, true)?;
        }
        Ok(dir)
    }

    pub fn dir(&mut self, rel: &str) -> io::Result<()> {
        let parts = self.claim(rel, true)?;
        let parent = self.parent(&parts)?;
        open_dir_at(&parent, &parts[parts.len() - 1], true).map(drop)
    }

    pub fn file(&mut self, rel: &str, exec: bool, size: u64, sha: &str, from: &mut impl Read) -> io::Result<()> {
        if size > MAX_FILE || self.total + size > MAX_TOTAL {
            return Err(rejected(format!("{rel} is over the size allowed")));
        }
        let parts = self.claim(rel, false)?;
        let parent = self.parent(&parts)?;
        let name = cstring(&parts[parts.len() - 1])?;
        let mode = if exec { 0o755 } else { 0o644 };
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, mode as libc::c_uint) };
        if fd < 0 {
            return Err(rejected(format!("can't write {rel}: {}", io::Error::last_os_error())));
        }
        let mut out = unsafe { File::from_raw_fd(fd) };
        let (got, written) = hash_reader(from.take(size), Some(&mut out))?;
        if written != size {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("{rel} was cut short")));
        }
        if got != sha {
            return Err(rejected(format!("{rel} doesn't match its checksum")));
        }
        self.total += size;
        Ok(())
    }

    pub fn link(&mut self, rel: &str, target: &str) -> io::Result<()> {
        if target.is_empty() || target.contains('\0') || target.len() > 4096 || target.starts_with('/') {
            return Err(rejected(format!("{rel} links to {target:?}, which isn't a relative path")));
        }
        if self.single {
            return Err(rejected(format!("{rel} is a link, not a file")));
        }
        let mut up = 0usize;
        let mut named = false;
        for part in target.split('/') {
            match part {
                "" | "." => {}
                ".." if named => return Err(rejected(format!("{rel} links to {target:?}, which climbs after a name"))),
                ".." => up += 1,
                _ => named = true,
            }
        }
        if up >= rel.split('/').count() {
            return Err(rejected(format!("{rel} links to {target:?}, outside what was returned")));
        }
        let parts = self.claim(rel, false)?;
        let parent = self.parent(&parts)?;
        let (name, target) = (cstring(&parts[parts.len() - 1])?, cstring(target)?);
        if unsafe { libc::symlinkat(target.as_ptr(), parent.as_raw_fd(), name.as_ptr()) } != 0 {
            return Err(rejected(format!("can't link {rel}: {}", io::Error::last_os_error())));
        }
        Ok(())
    }

    pub fn finish(self, replace: bool) -> io::Result<PathBuf> {
        let source = if self.single {
            let only: Vec<_> = std::fs::read_dir(self.folder.join(&self.staging))?.collect::<Result<_, _>>()?;
            match only.as_slice() {
                [one] => format!("{}/{}", self.staging, one.file_name().to_string_lossy()),
                _ => {
                    self.abandon();
                    return Err(rejected("expected exactly one returned file"));
                }
            }
        } else {
            self.staging.clone()
        };
        if exists_at(&self.parent, &cstring(&self.name)?) {
            if !replace {
                let dest = self.dest.clone();
                self.abandon();
                return Err(rejected(format!("{} is already there and kanna build didn't put it there; not replacing it", dest.display())));
            }
            let old = random_name(".kanna-old-");
            rename_at(&self.parent, &self.name, &old)?;
            let old = self.folder.join(old);
            if std::fs::symlink_metadata(&old)?.is_dir() { std::fs::remove_dir_all(&old)? } else { std::fs::remove_file(&old)? }
        }
        rename_at(&self.parent, &source, &self.name)?;
        if self.single {
            let _ = std::fs::remove_dir_all(self.folder.join(&self.staging));
        }
        Ok(self.dest)
    }

    pub fn abandon(self) {
        let _ = std::fs::remove_dir_all(self.folder.join(&self.staging));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::sha256;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-intake-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_bundle_arrives_with_its_modes_and_links() {
        let dir = temp("bundle");
        let dest = dir.join("Kanna.app");
        let mut intake = Intake::new(&dir, "Kanna.app", false).unwrap();
        intake.dir("Contents").unwrap();
        intake.file("Contents/MacOS/Kanna", true, 3, &sha256(b"bin"), &mut &b"bin"[..]).unwrap();
        intake.link("Contents/Current", "MacOS").unwrap();
        intake.finish(false).unwrap();
        let exe = std::fs::metadata(dest.join("Contents/MacOS/Kanna")).unwrap();
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&exe.permissions()) & 0o777, 0o755);
        assert_eq!(std::fs::read_link(dest.join("Contents/Current")).unwrap(), PathBuf::from("MacOS"));
    }

    #[test]
    fn traversal_links_checksums_and_collisions_are_refused() {
        let dir = temp("refused");
        std::fs::write(dir.join("secret"), "s").unwrap();
        let mut intake = Intake::new(&dir, "out.xcresult", false).unwrap();
        assert!(intake.file("../x", false, 1, &sha256(b"x"), &mut &b"x"[..]).is_err());
        assert!(intake.file("/etc/x", false, 1, &sha256(b"x"), &mut &b"x"[..]).is_err());
        assert!(intake.link("up", "..").is_err());
        assert!(intake.link("abs", "/etc").is_err());
        assert!(intake.link("a/b/c", "../../../x").is_err());
        intake.link("a/b/ok", "../sibling").unwrap();
        intake.link("here", ".").unwrap();
        assert!(intake.link("escape", "here/..").is_err());
        intake.link("dir", "a").unwrap();
        assert!(intake.file("dir/secret", false, 1, &sha256(b"x"), &mut &b"x"[..]).is_err());
        assert!(intake.file("a", false, 1, &sha256(b"y"), &mut &b"x"[..]).is_err());
        intake.file("B", false, 1, &sha256(b"x"), &mut &b"x"[..]).unwrap();
        assert!(intake.file("b", false, 1, &sha256(b"x"), &mut &b"x"[..]).is_err());
        intake.finish(false).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("secret")).unwrap(), "s");
    }

    #[test]
    fn something_already_there_is_replaced_only_when_allowed() {
        let dir = temp("kept");
        std::fs::write(dir.join("package.json"), "mine").unwrap();
        let mut intake = Intake::new(&dir, "package.json", true).unwrap();
        intake.file("package.json", false, 1, &sha256(b"x"), &mut &b"x"[..]).unwrap();
        assert!(intake.finish(false).is_err());
        assert_eq!(std::fs::read_to_string(dir.join("package.json")).unwrap(), "mine");
        let mut intake = Intake::new(&dir, "package.json", true).unwrap();
        intake.file("package.json", false, 1, &sha256(b"x"), &mut &b"x"[..]).unwrap();
        intake.finish(true).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("package.json")).unwrap(), "x");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn a_destination_under_a_linked_folder_is_refused() {
        let dir = temp("linked");
        let outside = temp("linked-outside");
        std::os::unix::fs::symlink(&outside, dir.join("out")).unwrap();
        assert!(Intake::new(&dir, "out/Kanna.app", false).is_err());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }
}
