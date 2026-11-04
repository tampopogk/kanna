use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

type Stamp = (SystemTime, u64, u64);

pub struct Stamped<T>(RefCell<HashMap<PathBuf, (Stamp, T)>>);

impl<T> Default for Stamped<T> {
    fn default() -> Self {
        Stamped(RefCell::default())
    }
}

fn stamp(path: &Path) -> Option<Stamp> {
    let found = std::fs::metadata(path).ok()?;
    Some((found.modified().ok()?, found.len(), found.ino()))
}

impl<T: Clone> Stamped<T> {
    pub fn get(&self, path: &Path, read: impl FnOnce() -> T) -> T {
        let Some(now) = stamp(path) else {
            self.0.borrow_mut().remove(path);
            return read();
        };
        if let Some((_, value)) = self.0.borrow().get(path).filter(|(seen, _)| *seen == now) {
            return value.clone();
        }
        let value = read();
        self.0.borrow_mut().insert(path.to_path_buf(), (now, value.clone()));
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_file_is_read_again_only_once_it_changes() {
        let dir = std::env::temp_dir().join(format!("kanna-stamped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("HEAD");
        let (seen, reads) = (Stamped::default(), Cell::new(0));
        let read = || {
            reads.set(reads.get() + 1);
            std::fs::read_to_string(&file).ok()
        };
        assert_eq!(seen.get(&file, read), None);
        std::fs::write(&file, "one").unwrap();
        assert_eq!(seen.get(&file, read).as_deref(), Some("one"));
        assert_eq!(seen.get(&file, read).as_deref(), Some("one"));
        assert_eq!(reads.get(), 2, "unchanged, it isn't read");
        std::fs::write(dir.join("next"), "two").unwrap();
        std::fs::rename(dir.join("next"), &file).unwrap();
        assert_eq!(seen.get(&file, read).as_deref(), Some("two"), "same size, another file");
        std::fs::remove_file(&file).unwrap();
        assert_eq!(seen.get(&file, read), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
