//! Final agent frames, named by immutable launch run, never by live session ID.
//! Writes are atomic and first-writer-wins. Copies survive stage cleanup and
//! daemon restarts. Server ingestion releases its copy only after DB commit;
//! failed/lost acknowledgements retain evidence until owned daemon-dir cleanup.
use crate::protocol::TerminalAttemptArchive;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

fn path(directory: &Path, attempt_id: &str) -> Result<std::path::PathBuf, String> {
    if !crate::session_id::is_safe(attempt_id) {
        return Err("unsafe terminal attempt id".into());
    }
    Ok(directory.join(format!("{attempt_id}.json")))
}
pub fn read(directory: &Path, attempt_id: &str) -> Result<Option<TerminalAttemptArchive>, String> {
    let bytes = match std::fs::read(path(directory, attempt_id)?) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let archive: TerminalAttemptArchive =
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if archive.binding.spawned_run_id != attempt_id {
        return Err("archive identity mismatch".into());
    }
    Ok(Some(archive))
}
pub fn release(directory: &Path, attempt_id: &str) -> Result<(), String> {
    match std::fs::remove_file(path(directory, attempt_id)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
pub fn persist(directory: &Path, archive: &TerminalAttemptArchive) -> Result<(), String> {
    use std::io::Write;
    let target = path(directory, &archive.binding.spawned_run_id)?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .map_err(|e| e.to_string())?;
    let payload = serde_json::to_vec(archive).map_err(|e| e.to_string())?;
    let temporary = target.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(&payload)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        match std::fs::hard_link(&temporary, &target) {
            Ok(()) => {
                std::fs::File::open(directory)
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let prior = std::fs::read(&target).map_err(|e| e.to_string())?;
                if prior == payload {
                    Ok(())
                } else {
                    Err("conflicting immutable terminal archive".into())
                }
            }
            Err(e) => Err(e.to_string()),
        }
    })();
    let _ = std::fs::remove_file(temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{TerminalAttemptBinding, TerminalSnapshot};
    #[test]
    fn immutable_archive_preserves_large_vt_and_rejects_conflicts_and_unsafe_reads() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(".tmp")
            .join(format!("archive-unit-{}", std::process::id()));
        let archive = TerminalAttemptArchive {
            binding: TerminalAttemptBinding {
                task_id: "task".into(),
                spawned_run_id: "run-task-1".into(),
            },
            session_id: "task".into(),
            cwd: "/work".into(),
            observed_exit_code: Some(7),
            unavailable_reason: None,
            snapshot: Some(TerminalSnapshot {
                version: 1,
                vt: format!(
                    "FIRST\r\n{}LAST",
                    "\u{1b}[32mcafé\u{1b}[0m\r\n".repeat(30000)
                ),
                cols: 80,
                rows: 24,
                cursor_row: 0,
                cursor_col: 0,
                cursor_visible: true,
                saved_at: 1,
                sequence: 1,
            }),
        };
        persist(&dir, &archive).unwrap();
        persist(&dir, &archive).unwrap();
        assert_eq!(
            serde_json::to_value(read(&dir, "run-task-1").unwrap()).unwrap(),
            serde_json::to_value(Some(&archive)).unwrap()
        );
        let mut conflict = archive.clone();
        conflict.observed_exit_code = None;
        assert!(persist(&dir, &conflict).is_err());
        assert!(read(&dir, "../bad").is_err());
        assert!(read(&dir, "run-task-missing").unwrap().is_none());
        std::fs::write(dir.join("corrupt.json"), "invalid").unwrap();
        assert!(read(&dir, "corrupt").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
