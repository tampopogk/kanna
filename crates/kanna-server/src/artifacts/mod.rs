//! Local artifact repository (spec §8, component T6).
//!
//! Artifacts are things a task produced, kept in a bare Git repository
//! *outside* the working repository and addressed by the Git tree id of
//! their content. This is not `transfer_engine`'s "artifacts", which are
//! provider transcripts moved with a task; nothing here touches the working
//! repository's index, objects or history.
//!
//! Artifacts are published, read, opened and annotated here, and shared with
//! another Kanna home through an ordinary Git remote (`remote`). A result
//! names them through its `artifacts` map (bound in the store and recorded in
//! its ledger entry), and a periodic sweep enforces retention without ever
//! removing a record.

pub(crate) mod remote;
pub(crate) mod store;
pub(crate) mod types;

#[cfg(test)]
mod remote_tests;
#[cfg(test)]
mod retention_tests;
#[cfg(test)]
mod sharing_retention_tests;
#[cfg(test)]
mod tests;

use std::fmt;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

pub(crate) use types::ArtifactRetention;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ArtifactError {
    /// The request itself is malformed.
    InvalidRequest(String),
    /// Not a full lowercase object id.
    InvalidId(String),
    /// A full object id that names something other than a tree.
    WrongObjectType {
        artifact_id: String,
        actual: String,
    },
    /// A workspace or artifact path that is unsafe or malformed.
    InvalidPath(String),
    InvalidEntrypoint(String),
    /// `previous` does not name an artifact in this repository.
    InvalidPrevious {
        repo_id: String,
        artifact_id: String,
    },
    /// The requested workspace path does not exist.
    SourceNotFound(String),
    WorkspaceUnavailable(String),
    /// No artifact with this id was published to this repository.
    NotFound {
        repo_id: String,
        artifact_id: String,
    },
    /// The artifact was published but its content is no longer retained.
    ContentMissing {
        repo_id: String,
        artifact_id: String,
    },
    /// The artifact exists but has no file at this path.
    FileNotFound {
        artifact_id: String,
        path: String,
    },
    TooLarge(String),
    /// The configured repository location is unusable.
    Location(String),
    Storage(String),
    /// The repository configures no `artifacts.remote`.
    RemoteNotConfigured {
        repo_id: String,
    },
    /// The configured `artifacts.remote` is not an acceptable Git remote.
    InvalidRemote(String),
    /// The remote holds neither content nor records for this id.
    NotOnRemote {
        remote: String,
        artifact_id: String,
    },
    /// Git could not reach or talk to the remote. The message never carries
    /// URL credentials.
    RemoteFailed(String),
    /// The remote already holds different objects under names Kanna treats
    /// as immutable.
    RemoteConflict {
        remote: String,
        refs: Vec<String>,
    },
}

impl ArtifactError {
    /// Stable machine-readable code for the HTTP error body.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::InvalidId(_) => "invalid_artifact_id",
            Self::WrongObjectType { .. } => "not_an_artifact_tree",
            Self::InvalidPath(_) => "invalid_path",
            Self::InvalidEntrypoint(_) => "invalid_entrypoint",
            Self::InvalidPrevious { .. } => "unknown_previous_artifact",
            Self::SourceNotFound(_) => "source_not_found",
            Self::WorkspaceUnavailable(_) => "workspace_unavailable",
            Self::NotFound { .. } => "artifact_not_found",
            Self::ContentMissing { .. } => "artifact_content_missing",
            Self::FileNotFound { .. } => "artifact_file_not_found",
            Self::TooLarge(_) => "artifact_too_large",
            Self::Location(_) => "artifact_repository_location_invalid",
            Self::Storage(_) => "artifact_storage_error",
            Self::RemoteNotConfigured { .. } => "artifact_remote_not_configured",
            Self::InvalidRemote(_) => "artifact_remote_invalid",
            Self::NotOnRemote { .. } => "artifact_not_on_remote",
            Self::RemoteFailed(_) => "artifact_remote_failed",
            Self::RemoteConflict { .. } => "artifact_remote_conflict",
        }
    }
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message)
            | Self::InvalidPath(message)
            | Self::InvalidEntrypoint(message)
            | Self::WorkspaceUnavailable(message)
            | Self::TooLarge(message)
            | Self::Location(message)
            | Self::Storage(message)
            | Self::InvalidRemote(message)
            | Self::RemoteFailed(message) => formatter.write_str(message),
            Self::RemoteNotConfigured { repo_id } => write!(
                formatter,
                "repository {repo_id} has no artifact remote; set artifacts.remote in .kanna/config.json or .kanna/config.local.json"
            ),
            Self::NotOnRemote {
                remote,
                artifact_id,
            } => write!(
                formatter,
                "artifact {artifact_id} is missing on artifact remote {remote}: it holds no content or records for that id"
            ),
            Self::RemoteConflict { remote, refs } => write!(
                formatter,
                "artifact remote {remote} already holds different objects under {}; nothing there was overwritten",
                refs.join(", ")
            ),
            Self::InvalidId(value) => write!(
                formatter,
                "{value:?} is not a full artifact id (40 lowercase hex characters)"
            ),
            Self::WrongObjectType {
                artifact_id,
                actual,
            } => write!(
                formatter,
                "{artifact_id} is a {actual} object, not an artifact tree"
            ),
            Self::InvalidPrevious {
                repo_id,
                artifact_id,
            } => write!(
                formatter,
                "previous artifact {artifact_id} was never published to repository {repo_id}"
            ),
            Self::SourceNotFound(path) => {
                write!(formatter, "{path} does not exist in the task workspace")
            }
            Self::NotFound {
                repo_id,
                artifact_id,
            } => write!(
                formatter,
                "artifact {artifact_id} was not found in repository {repo_id}"
            ),
            Self::ContentMissing {
                repo_id,
                artifact_id,
            } => write!(
                formatter,
                "artifact {artifact_id} in repository {repo_id} was produced, but its content is no longer retained"
            ),
            Self::FileNotFound { artifact_id, path } => {
                write!(formatter, "artifact {artifact_id} has no file {path}")
            }
        }
    }
}

impl std::error::Error for ArtifactError {}

/// Where artifact repositories live when a repository does not configure a
/// location. Injected so tests never touch the real `~/.kanna`.
#[derive(Clone, Debug)]
pub(crate) struct ArtifactStorageContext {
    home: PathBuf,
}

impl ArtifactStorageContext {
    pub(crate) fn from_environment() -> Self {
        Self {
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from(".")),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_home(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    pub(crate) fn home(&self) -> &Path {
        &self.home
    }

    fn default_repository_path(&self, repo_id: &str) -> PathBuf {
        self.home
            .join(".kanna")
            .join("repos")
            .join(repo_id)
            .join("artifacts.git")
    }
}

/// Resolve and vet the artifact repository location for one working
/// repository. A configured path may be absolute, `~/`-relative, or relative
/// to the registered repository root. Whatever it resolves to, it must not be
/// inside (or contain) the working repository, its Git directory, or any of
/// its worktrees — symlinks included — so artifact content can never land in
/// a checkout.
pub(crate) fn resolve_repository_path(
    context: &ArtifactStorageContext,
    repo_id: &str,
    repo_root: &Path,
    configured: Option<&str>,
) -> Result<PathBuf, ArtifactError> {
    let candidate = match configured.map(str::trim).filter(|path| !path.is_empty()) {
        Some(configured) => {
            let path = if let Some(rest) = configured.strip_prefix("~/") {
                context.home.join(rest)
            } else if Path::new(configured).is_absolute() {
                PathBuf::from(configured)
            } else {
                repo_root.join(configured)
            };
            if path
                .components()
                .any(|component| component == Component::ParentDir)
            {
                return Err(ArtifactError::Location(format!(
                    "artifacts.repositoryPath {configured:?} must not contain `..`"
                )));
            }
            path
        }
        None => {
            let valid_id = !repo_id.is_empty()
                && repo_id != "."
                && repo_id != ".."
                && repo_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte));
            if !valid_id {
                return Err(ArtifactError::Location(format!(
                    "repository id {repo_id:?} cannot name an artifact repository directory"
                )));
            }
            context.default_repository_path(repo_id)
        }
    };
    let candidate = canonical_prospective(&candidate)?;
    for forbidden in working_repository_paths(repo_root) {
        if candidate.starts_with(&forbidden) || forbidden.starts_with(&candidate) {
            return Err(ArtifactError::Location(format!(
                "artifact repository {} overlaps the working repository at {}; configure artifacts.repositoryPath outside it",
                candidate.display(),
                forbidden.display()
            )));
        }
    }
    Ok(candidate)
}

/// Canonicalize the longest existing ancestor and append the rest, so a
/// location that does not exist yet is still compared through symlinks.
fn canonical_prospective(path: &Path) -> Result<PathBuf, ArtifactError> {
    let mut existing = path.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(&existing) {
            Ok(canonical) => {
                return Ok(missing
                    .iter()
                    .rev()
                    .fold(canonical, |path, name| path.join(name)))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name().map(|name| name.to_os_string()) else {
                    return Err(ArtifactError::Location(format!(
                        "artifact repository location {} cannot be resolved",
                        path.display()
                    )));
                };
                missing.push(name);
                if !existing.pop() {
                    return Err(ArtifactError::Location(format!(
                        "artifact repository location {} cannot be resolved",
                        path.display()
                    )));
                }
            }
            Err(error) => {
                return Err(ArtifactError::Location(format!(
                    "artifact repository location {} cannot be resolved: {error}",
                    path.display()
                )))
            }
        }
    }
}

/// Every directory that belongs to the working repository: its root, its Git
/// directory, and every registered worktree.
fn working_repository_paths(repo_root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![repo_root.to_path_buf()];
    if let Ok(repository) = git2::Repository::open(repo_root) {
        paths.push(repository.path().to_path_buf());
        if let Some(workdir) = repository.workdir() {
            paths.push(workdir.to_path_buf());
        }
        if let Ok(names) = repository.worktrees() {
            for name in names.iter().flatten() {
                if let Ok(worktree) = repository.find_worktree(name) {
                    paths.push(worktree.path().to_path_buf());
                }
            }
        }
    }
    paths
        .into_iter()
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .collect()
}

pub(crate) fn random_hex(bytes: usize) -> Result<String, String> {
    let mut buffer = vec![0_u8; bytes];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut buffer))
        .map_err(|error| format!("failed to read operating-system randomness: {error}"))?;
    Ok(buffer.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` in UTC.
pub(crate) fn rfc3339_utc(time: SystemTime) -> String {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = duration.as_secs();
    let days = (seconds / 86_400) as i64;
    let second_of_day = seconds % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        second_of_day / 3_600,
        (second_of_day % 3_600) / 60,
        second_of_day % 60,
        duration.subsec_millis()
    )
}
