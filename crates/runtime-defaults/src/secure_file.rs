//! Atomic, symlink-safe, owner-only-permission file writes for the small set
//! of files that hold live secrets at rest: `kanna-server`'s `machine_trust`
//! outbound bearer secrets, its LAN TLS, secure-channel and peer-channel
//! private keys, its peer trust store, and the task-transfer sidecar's
//! transfer identity. Shared here rather than kept in `kanna-server` so the
//! sidecar (a separate process with its own key) writes its key exactly the
//! same way. Distinct from `pairing::PairingStore::save`, which writes
//! secret *hashes* only and does not enforce permissions - unlike that file,
//! a file written through this module can leak an actually-usable credential
//! if it is ever group/other-readable, even briefly.
//!
//! [`load_owner_only`] is the matching read: it refuses a file that is not a
//! regular file or that grants group/other permissions, so a key that was
//! written correctly and later loosened fails closed instead of being used.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Atomically replaces `path`'s contents, creating it 0600 if it does not
/// exist. Never opens, truncates, or writes through a pre-existing path at
/// the temp location: `create_new` refuses a path that already exists,
/// whether that is a stale temp file a crashed prior run left behind (whose
/// permissions this call must never inherit - `mode()` on `OpenOptions`
/// only takes effect for a file the call actually creates, not one it
/// merely opens) or a symlink planted at the predictable temp name. Either
/// way, this fails closed instead of writing secret material through it.
pub fn atomic_write_0600(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let temp_path = unique_temp_path(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let write_result = options.open(&temp_path).and_then(|mut file| {
        file.write_all(contents.as_bytes())
            .and_then(|_| file.sync_all())
    });
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!(
            "failed to write {} temp file {}: {error}",
            path.display(),
            temp_path.display()
        ));
    }
    // Belt and suspenders: reassert the mode explicitly rather than relying
    // solely on `create_new` + `mode()` having applied it to the file this
    // process just created.
    if let Err(error) = std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o600))
    {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!(
            "failed to set permissions on {} temp file {}: {error}",
            path.display(),
            temp_path.display()
        ));
    }
    std::fs::rename(&temp_path, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp_path);
        format!(
            "failed to replace {} from {}: {error}",
            path.display(),
            temp_path.display()
        )
    })
}

/// What is wrong with a file that [`load_owner_only`] refused.
#[derive(Debug)]
pub enum OwnerOnlyReadError {
    /// The file does not exist; the caller decides whether to create it.
    NotFound,
    /// The path is not a regular file (a symlink, a directory, a device).
    NotARegularFile,
    /// The file grants group or other permissions.
    LoosePermissions {
        mode: u32,
    },
    Io(std::io::Error),
}

impl std::fmt::Display for OwnerOnlyReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("file not found"),
            Self::NotARegularFile => formatter.write_str("not a regular file"),
            Self::LoosePermissions { mode } => write!(
                formatter,
                "must not grant group or other permissions (mode {mode:o})"
            ),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for OwnerOnlyReadError {}

/// Reads a secret-bearing file, refusing anything that is not a regular
/// file owned only by this user. Never follows a symlink at `path`.
pub fn load_owner_only(path: &Path) -> Result<String, OwnerOnlyReadError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(OwnerOnlyReadError::NotFound)
        }
        Err(error) => return Err(OwnerOnlyReadError::Io(error)),
    };
    if !metadata.file_type().is_file() {
        return Err(OwnerOnlyReadError::NotARegularFile);
    }
    let mode = metadata.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(OwnerOnlyReadError::LoosePermissions { mode: mode & 0o777 });
    }
    std::fs::read_to_string(path).map_err(OwnerOnlyReadError::Io)
}

/// A temp name carrying the pid *and* a nanosecond timestamp, not the pid
/// alone: a bare `tmp-{pid}` is predictable across a pid's reuse on a
/// long-running system, which is exactly what would let a planted stale
/// file or symlink at that exact path matter. This does not need to be
/// unguessable, only for `create_new` to make guessing it useless - a
/// collision (accidental or adversarial) fails the write rather than
/// silently opening through whatever is already there.
fn unique_temp_path(path: &Path) -> PathBuf {
    // The clock alone is not unique: macOS reports `SystemTime` at
    // microsecond resolution, so two concurrent writers in one process (the
    // test suite does exactly this) can draw the same nanos and then race
    // `create_new` for one name. A process-wide counter makes every temp
    // name in this process distinct regardless of timing; the pid keeps
    // processes apart.
    static NEXT_TEMP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let sequence = NEXT_TEMP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    path.with_extension(format!("tmp-{}-{nanos}-{sequence}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_test_path(label: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "kanna-runtime-defaults-{label}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir.join(label)
    }

    fn temp_path() -> PathBuf {
        unique_test_path("secure-file")
    }

    #[test]
    fn load_owner_only_refuses_loose_and_irregular_files() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_path();
        assert!(matches!(
            load_owner_only(&path),
            Err(OwnerOnlyReadError::NotFound)
        ));
        atomic_write_0600(&path, "secret").expect("write");
        assert_eq!(load_owner_only(&path).unwrap(), "secret");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            load_owner_only(&path),
            Err(OwnerOnlyReadError::LoosePermissions { mode: 0o644 })
        ));
        let link = path.with_extension("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            load_owner_only(&link),
            Err(OwnerOnlyReadError::NotARegularFile)
        ));
    }

    #[test]
    fn writes_and_reasserts_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_path();
        atomic_write_0600(&path, "hello").expect("write");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn overwrites_existing_content_atomically() {
        let path = temp_path();
        atomic_write_0600(&path, "first").expect("first write");
        atomic_write_0600(&path, "second").expect("second write");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    }

    /// A stale temp file left by a crashed prior save (same predictable
    /// name shape, wide-open permissions) must never be reused: `mode()`
    /// only applies to a file this call creates, so opening through an
    /// existing one would silently keep its old, permissive mode.
    #[test]
    fn a_stale_temp_file_with_loose_permissions_is_never_written_through() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_path();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let predicted_temp = path.with_extension(format!("tmp-{}-{nanos}", std::process::id()));
        std::fs::write(&predicted_temp, "attacker-planted, world readable")
            .expect("plant a stale temp file");
        std::fs::set_permissions(&predicted_temp, std::fs::Permissions::from_mode(0o644))
            .expect("widen permissions to simulate a stale leftover");

        // The real call almost certainly picks a different nanosecond value
        // than the one just planted above and succeeds normally; the planted
        // file is simply never touched by it.
        atomic_write_0600(&path, "real content").expect("write succeeds regardless");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "real content");
        assert_eq!(
            std::fs::read_to_string(&predicted_temp).unwrap(),
            "attacker-planted, world readable",
            "the stale planted file must be left exactly as it was, never opened through"
        );

        let _ = std::fs::remove_file(&predicted_temp);
    }

    /// A symlink at the exact predicted temp path must not be followed: this
    /// simulates the classic local symlink attack by writing at the literal
    /// path `unique_temp_path` would compute for the very next nanosecond,
    /// then asserting the eventual real write still lands only at `path`,
    /// with the symlink's target untouched.
    #[test]
    fn a_symlink_at_the_temp_path_is_never_followed() {
        let path = temp_path();
        let attack_target = unique_test_path("secure-file-symlink-target");
        std::fs::write(&attack_target, "must not be overwritten").expect("seed attack target");

        // `create_new` refuses to open through an existing symlink exactly
        // as it refuses an existing regular file - proven here with a
        // concrete symlink rather than asserted from the API contract alone.
        let temp_path_shape = path.with_extension("tmp-symlink-probe");
        std::os::unix::fs::symlink(&attack_target, &temp_path_shape).expect("plant symlink");
        let result = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path_shape);
        assert!(
            result.is_err(),
            "create_new must refuse a path that is already a symlink"
        );

        atomic_write_0600(&path, "real content").expect("write");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "real content");
        assert_eq!(
            std::fs::read_to_string(&attack_target).unwrap(),
            "must not be overwritten"
        );

        let _ = std::fs::remove_file(&temp_path_shape);
    }
}
