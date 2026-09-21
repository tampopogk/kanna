//! Durable lifecycle diagnostics for this desktop's `kanna-server`.
//!
//! [`super::MobileServerManager`] used to record every start, adoption,
//! replacement and recovery with `eprintln!`. A Finder-launched macOS app has
//! no stderr: those lines reach no terminal, no file, and no unified-log
//! predicate. When a production launch on 2026-09-20 came up with no local
//! server at all, nothing anywhere could name which start path had failed —
//! the sidecar's own `kanna-server-stderr.log` proved only that no server was
//! ever spawned, which is the absence of a record rather than one.
//!
//! These lines go to a bounded file beside that stderr capture, which is
//! already the first place anyone debugging a missing server looks.

use std::io::Write;
use std::path::{Path, PathBuf};

const LIFECYCLE_LOG_FILE: &str = "kanna-server-lifecycle.log";

/// Most the record may occupy before it starts over. Deliberately small: this
/// is a handful of lines per app launch, not a log stream — the sidecar's own
/// output has its own, much larger, capture beside it.
const MAX_LIFECYCLE_LOG_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct LifecycleLog {
    path: Option<PathBuf>,
}

impl LifecycleLog {
    /// Beside `kanna-server-stderr.log`, in the directory holding the
    /// generated `server.toml`.
    pub(crate) fn beside_server_config(config_path: &Path) -> Self {
        Self {
            // `Path::parent` answers `Some("")` for a bare file name, which
            // would put this record in whatever directory the app happened to
            // be launched from.
            path: config_path
                .parent()
                .filter(|dir| !dir.as_os_str().is_empty())
                .map(|dir| dir.join(LIFECYCLE_LOG_FILE)),
        }
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Record one lifecycle fact. This never fails its caller and never
    /// returns an error: a diagnostic that can break startup is worse than a
    /// missing one. The line is still written to stderr for the launches that
    /// have one (`kd dev up`, a terminal launch, the test harness).
    pub(crate) fn record(&self, message: &str) {
        eprintln!("[mobile] {message}");
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let line = format!(
            "{} [mobile] {message}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
        );
        if let Err(error) = append_line(path, &line) {
            eprintln!(
                "[mobile] failed to record lifecycle diagnostics at {}: {error}",
                path.display()
            );
        }
    }
}

fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if size >= MAX_LIFECYCLE_LOG_BYTES {
        std::fs::write(
            path,
            format!(
                "--- kanna-server lifecycle log restarted: exceeded {} MiB ---\n",
                MAX_LIFECYCLE_LOG_BYTES / (1024 * 1024)
            ),
        )?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_lines_land_beside_the_server_config() {
        let root = super::super::tests::unique_test_root("lifecycle-log");
        std::fs::create_dir_all(&root).expect("create test root");
        let config_path = root.join("server.toml");
        let log = LifecycleLog::beside_server_config(&config_path);

        log.record("spawned kanna-server pid 4242");
        log.record("kanna-server start failed: nope");

        let path = log.path().expect("lifecycle log path").to_path_buf();
        let contents = std::fs::read_to_string(&path).expect("read lifecycle log");
        assert_eq!(path, root.join(LIFECYCLE_LOG_FILE));
        assert!(contents.contains("[mobile] spawned kanna-server pid 4242"));
        assert!(contents.contains("[mobile] kanna-server start failed: nope"));
        // Two records, two lines: an app launch's whole story has to be
        // readable in order.
        assert_eq!(contents.lines().count(), 2);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_record_without_a_directory_is_dropped_rather_than_failing() {
        // `config_path.parent()` is None only for a bare relative file name,
        // which is not a real config path — but a diagnostic must not panic or
        // fail a start on it either.
        let log = LifecycleLog::beside_server_config(Path::new("server.toml"));
        assert!(log.path().is_none());
        log.record("dropped");
    }

    #[test]
    fn the_record_restarts_when_it_crosses_its_size_cap() {
        let root = super::super::tests::unique_test_root("lifecycle-log-cap");
        std::fs::create_dir_all(&root).expect("create test root");
        let config_path = root.join("server.toml");
        let log = LifecycleLog::beside_server_config(&config_path);
        let path = log.path().expect("lifecycle log path").to_path_buf();
        std::fs::write(&path, vec![b'x'; MAX_LIFECYCLE_LOG_BYTES as usize + 1])
            .expect("seed oversized log");

        log.record("after the cap");

        let contents = std::fs::read_to_string(&path).expect("read lifecycle log");
        assert!(contents.starts_with("--- kanna-server lifecycle log restarted"));
        assert!(contents.contains("[mobile] after the cap"));
        assert!((contents.len() as u64) < MAX_LIFECYCLE_LOG_BYTES);

        std::fs::remove_dir_all(&root).ok();
    }
}
