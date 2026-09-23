//! The local artifact repository: one bare Git repository per working
//! repository, outside it.
//!
//! Layout inside the bare repository:
//!
//! - Content. Each published tree is kept reachable by a parentless commit
//!   whose root tree *is* the payload, under `refs/kanna/artifacts/trees/<id>`.
//!   Parentless on purpose: `previous` is a metadata link, and a Git parent
//!   would keep every older payload reachable no matter what retention says.
//! - Metadata. One commit history under `refs/kanna/artifacts/metadata` whose
//!   tree holds JSON records under `versions/`, `comments/` and `decisions/`,
//!   each at `<dir>/<id[0..2]>/<id>/<record-id>.json`. Tree ids appear only as
//!   names and JSON strings, never as tree entries, so metadata never keeps
//!   content alive and content never carries metadata.
//!
//! Every mutation holds an exclusive `flock` on `kanna-artifacts.lock` in the
//! repository directory, and the metadata ref is additionally moved with a
//! compare-and-swap. Writes are ordered objects → content ref → metadata ref,
//! and the repository is initialized with `core.fsyncObjectFiles`, so a crash
//! can leave retained content without a descriptor but never a descriptor
//! for content that was not retained.

use super::types::{
    ArtifactAnchor, ArtifactComment, ArtifactContentKind, ArtifactDecision, ArtifactDetail,
    ArtifactFileEntry, ArtifactProducer, ArtifactReference, ArtifactRetention, ArtifactStorage,
    ArtifactVersion, PublishedArtifact, ARTIFACT_RECORD_SCHEMA_VERSION,
};
use super::{random_hex, rfc3339_utc, ArtifactError};
use crate::repo_browser::BrowseError;
use git2::{ObjectType, Oid, Repository, Signature, Tree};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

const METADATA_REF: &str = "refs/kanna/artifacts/metadata";
const CONTENT_REF_PREFIX: &str = "refs/kanna/artifacts/trees/";
const LOCK_FILE_NAME: &str = "kanna-artifacts.lock";
const VERSIONS_DIR: &str = "versions";
const COMMENTS_DIR: &str = "comments";
const DECISIONS_DIR: &str = "decisions";
const FILE_MODE_BLOB: i32 = 0o100_644;
const FILE_MODE_TREE: i32 = 0o040_000;

pub(crate) const MAX_DECLARED_NAME_BYTES: usize = 200;
pub(crate) const MAX_TEXT_BYTES: usize = 64 * 1024;
pub(crate) const MAX_ANCHOR_POSITION_BYTES: usize = 1024;
pub(crate) const MAX_ANCHOR_EXCERPT_BYTES: usize = 4 * 1024;

/// Bounds on what one publication may read from a workspace. Exceeding any of
/// them refuses the publication; nothing is ever silently truncated.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PublishLimits {
    pub(crate) max_files: usize,
    pub(crate) max_depth: usize,
    pub(crate) max_file_bytes: u64,
    pub(crate) max_total_bytes: u64,
}

impl Default for PublishLimits {
    fn default() -> Self {
        Self {
            max_files: 2_000,
            max_depth: 24,
            max_file_bytes: 16 * 1024 * 1024,
            max_total_bytes: 64 * 1024 * 1024,
        }
    }
}

pub(crate) struct PublishRequest<'a> {
    pub(crate) task_id: &'a str,
    /// The task workspace. Nothing outside it is read.
    pub(crate) workspace_root: &'a Path,
    /// A file or directory inside `workspace_root`.
    pub(crate) source_path: &'a str,
    pub(crate) kind: ArtifactContentKind,
    pub(crate) entrypoint: Option<&'a str>,
    pub(crate) previous: Option<&'a str>,
    pub(crate) retention: ArtifactRetention,
    pub(crate) limits: PublishLimits,
}

pub(crate) struct CommentRequest<'a> {
    pub(crate) author: &'a str,
    pub(crate) body: &'a str,
    pub(crate) anchor: Option<ArtifactAnchor>,
}

pub(crate) struct DecisionRequest<'a> {
    pub(crate) who: &'a str,
    pub(crate) what: &'a str,
}

/// A file read out of a retained tree.
pub(crate) struct ArtifactBlob {
    pub(crate) path: String,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) struct ArtifactStore {
    repository: Repository,
    path: PathBuf,
    repo_id: String,
}

/// Injected failures for proving publication ordering.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailPoint {
    /// Objects were written but the content ref was not.
    BeforeContentRef,
    /// Content is retained but no descriptor was recorded.
    BeforeMetadataRef,
}

#[cfg(test)]
thread_local! {
    static FAIL_POINT: std::cell::Cell<Option<FailPoint>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn inject_failure(point: Option<FailPoint>) {
    FAIL_POINT.with(|cell| cell.set(point));
}

fn fail_at(_point: &'static str) -> Result<(), ArtifactError> {
    #[cfg(test)]
    {
        let wanted = match _point {
            "before-content-ref" => FailPoint::BeforeContentRef,
            _ => FailPoint::BeforeMetadataRef,
        };
        if FAIL_POINT.with(|cell| cell.get()) == Some(wanted) {
            return Err(ArtifactError::Storage(format!("injected failure {_point}")));
        }
    }
    Ok(())
}

impl ArtifactStore {
    /// Open the repository, creating it (and only it) if absent.
    pub(crate) fn open_or_create(path: &Path, repo_id: &str) -> Result<Self, ArtifactError> {
        std::fs::create_dir_all(path).map_err(|error| {
            ArtifactError::Storage(format!(
                "cannot create artifact repository directory {}: {error}",
                path.display()
            ))
        })?;
        let _lock = RepositoryLock::acquire(path)?;
        let is_empty = std::fs::read_dir(path)
            .map_err(|error| {
                ArtifactError::Storage(format!(
                    "cannot read artifact repository directory {}: {error}",
                    path.display()
                ))
            })?
            .filter_map(Result::ok)
            .all(|entry| entry.file_name() == LOCK_FILE_NAME);
        if is_empty {
            let repository = Repository::init_bare(path).map_err(|error| {
                ArtifactError::Storage(format!(
                    "cannot initialize artifact repository {}: {error}",
                    path.display()
                ))
            })?;
            let mut config = repository
                .config()
                .and_then(|config| config.open_level(git2::ConfigLevel::Local))
                .map_err(storage)?;
            config
                .set_bool("core.fsyncObjectFiles", true)
                .map_err(storage)?;
            // Retention is Kanna's decision, made through refs; automatic
            // maintenance must never be the thing that drops an object.
            config.set_i32("gc.auto", 0).map_err(storage)?;
        }
        Self::open_existing(path, repo_id)?.ok_or_else(|| {
            ArtifactError::Location(format!(
                "{} exists but is not a Kanna artifact repository",
                path.display()
            ))
        })
    }

    /// Open an existing repository without creating anything. `Ok(None)`
    /// means there is no artifact repository yet, which read operations
    /// report as the requested artifact not existing.
    pub(crate) fn open_existing(path: &Path, repo_id: &str) -> Result<Option<Self>, ArtifactError> {
        if !path.exists() {
            return Ok(None);
        }
        // `open_bare` never searches parent directories, so a missing or
        // damaged artifact repository can never resolve to the working
        // repository around it.
        let repository = match Repository::open_bare(path) {
            Ok(repository) => repository,
            Err(error) if error.code() == git2::ErrorCode::NotFound => {
                return Err(ArtifactError::Location(format!(
                    "{} exists but is not a bare Git repository",
                    path.display()
                )))
            }
            Err(error) => return Err(storage(error)),
        };
        if !repository.is_bare() {
            return Err(ArtifactError::Location(format!(
                "{} is not a bare Git repository",
                path.display()
            )));
        }
        Ok(Some(Self {
            repository,
            path: path.to_path_buf(),
            repo_id: repo_id.to_string(),
        }))
    }

    pub(crate) fn publish(
        &self,
        request: PublishRequest<'_>,
    ) -> Result<PublishedArtifact, ArtifactError> {
        let payload = read_payload(request.workspace_root, request.source_path, request.limits)?;
        let previous = request.previous.map(parse_object_id).transpose()?;

        let _lock = RepositoryLock::acquire(&self.path)?;
        let tree_id = write_tree(&self.repository, &payload.root)?;
        let tree = self.repository.find_tree(tree_id).map_err(storage)?;
        let entrypoint = resolve_entrypoint(
            &tree,
            request.kind,
            request.entrypoint,
            payload.single_file_name.as_deref(),
        )?;
        if let Some(previous) = previous {
            if self
                .list_records::<ArtifactVersion>(VERSIONS_DIR, previous)?
                .is_empty()
            {
                return Err(ArtifactError::InvalidPrevious {
                    repo_id: self.repo_id.clone(),
                    artifact_id: previous.to_string(),
                });
            }
        }

        fail_at("before-content-ref")?;
        let (commit, ref_name, content_created) = self.retain_tree(tree_id, request.kind)?;
        fail_at("before-metadata-ref")?;

        let artifact_id = tree_id.to_string();
        let (record_id, created_at) = new_record_identity()?;
        let version = ArtifactVersion {
            schema_version: ARTIFACT_RECORD_SCHEMA_VERSION,
            record_id: record_id.clone(),
            repo_id: self.repo_id.clone(),
            artifact_id: artifact_id.clone(),
            kind: request.kind,
            entrypoint,
            created_at,
            previous: previous.map(|oid| oid.to_string()),
            retention: request.retention,
            produced_by: ArtifactProducer {
                task_id: request.task_id.to_string(),
            },
            file_count: payload.file_count as u64,
            total_bytes: payload.total_bytes,
            storage: ArtifactStorage {
                commit: commit.to_string(),
                ref_name,
            },
        };
        self.append_record(
            VERSIONS_DIR,
            tree_id,
            &record_id,
            &version,
            &format!("version {artifact_id}"),
        )?;
        Ok(PublishedArtifact {
            reference: ArtifactReference::Stored {
                repo_id: self.repo_id.clone(),
                artifact_id: artifact_id.clone(),
                kind: request.kind,
            },
            artifact_id,
            version,
            content_created,
        })
    }

    pub(crate) fn detail(&self, artifact_id: &str) -> Result<ArtifactDetail, ArtifactError> {
        let oid = parse_object_id(artifact_id)?;
        let versions = self.list_records::<ArtifactVersion>(VERSIONS_DIR, oid)?;
        if versions.is_empty() {
            self.reject_non_tree(oid)?;
            return Err(self.not_found(oid));
        }
        let tree = self.retained_tree(oid)?;
        let mut files = Vec::new();
        if let Some(tree) = &tree {
            collect_files(&self.repository, tree, "", &mut files)?;
        }
        let kind = versions
            .last()
            .map(|version| version.kind)
            .unwrap_or(ArtifactContentKind::Document);
        Ok(ArtifactDetail {
            repo_id: self.repo_id.clone(),
            artifact_id: oid.to_string(),
            retained: tree.is_some(),
            reference: ArtifactReference::Stored {
                repo_id: self.repo_id.clone(),
                artifact_id: oid.to_string(),
                kind,
            },
            files,
            versions,
            comments: self.list_records(COMMENTS_DIR, oid)?,
            decisions: self.list_records(DECISIONS_DIR, oid)?,
        })
    }

    /// The entrypoint of the newest version record, for opening an artifact.
    pub(crate) fn entrypoint(&self, artifact_id: &str) -> Result<Option<String>, ArtifactError> {
        let oid = parse_object_id(artifact_id)?;
        let versions = self.list_records::<ArtifactVersion>(VERSIONS_DIR, oid)?;
        let Some(latest) = versions.last() else {
            self.reject_non_tree(oid)?;
            return Err(self.not_found(oid));
        };
        if self.retained_tree(oid)?.is_none() {
            return Err(ArtifactError::ContentMissing {
                repo_id: self.repo_id.clone(),
                artifact_id: oid.to_string(),
            });
        }
        Ok(latest.entrypoint.clone())
    }

    /// Read one file of a retained tree. The path is a `/`-separated path of
    /// already-decoded names.
    pub(crate) fn read_file(
        &self,
        artifact_id: &str,
        path: &str,
    ) -> Result<ArtifactBlob, ArtifactError> {
        let oid = self.require_published(artifact_id)?;
        let tree = self
            .retained_tree(oid)?
            .ok_or_else(|| ArtifactError::ContentMissing {
                repo_id: self.repo_id.clone(),
                artifact_id: oid.to_string(),
            })?;
        let relative = normalize_artifact_path(path)?;
        let missing = || ArtifactError::FileNotFound {
            artifact_id: oid.to_string(),
            path: relative.clone(),
        };
        let entry = tree.get_path(Path::new(&relative)).map_err(|_| missing())?;
        if entry.kind() != Some(ObjectType::Blob) {
            return Err(missing());
        }
        let blob = self
            .repository
            .find_blob(entry.id())
            .map_err(|_| missing())?;
        Ok(ArtifactBlob {
            path: relative.clone(),
            bytes: blob.content().to_vec(),
        })
    }

    /// Whether `path` names a subtree of a retained artifact, so a request
    /// for a directory can be served its `index.html`.
    pub(crate) fn is_directory(&self, artifact_id: &str, path: &str) -> bool {
        let Ok(oid) = parse_object_id(artifact_id) else {
            return false;
        };
        let Ok(Some(tree)) = self.retained_tree(oid) else {
            return false;
        };
        if path.is_empty() {
            return true;
        }
        tree.get_path(Path::new(path))
            .is_ok_and(|entry| entry.kind() == Some(ObjectType::Tree))
    }

    pub(crate) fn record_comment(
        &self,
        artifact_id: &str,
        request: CommentRequest<'_>,
    ) -> Result<ArtifactComment, ArtifactError> {
        let author = declared_text("author", request.author, MAX_DECLARED_NAME_BYTES)?;
        let body = declared_text("body", request.body, MAX_TEXT_BYTES)?;
        let anchor = request
            .anchor
            .map(|anchor| self.validate_anchor(artifact_id, anchor))
            .transpose()?
            .flatten();
        let _lock = RepositoryLock::acquire(&self.path)?;
        let oid = self.require_published(artifact_id)?;
        let (record_id, created_at) = new_record_identity()?;
        let comment = ArtifactComment {
            schema_version: ARTIFACT_RECORD_SCHEMA_VERSION,
            record_id: record_id.clone(),
            repo_id: self.repo_id.clone(),
            about_artifact_id: oid.to_string(),
            created_at,
            author,
            body,
            anchor,
        };
        self.append_record(
            COMMENTS_DIR,
            oid,
            &record_id,
            &comment,
            &format!("comment {oid}"),
        )?;
        Ok(comment)
    }

    pub(crate) fn record_decision(
        &self,
        artifact_id: &str,
        request: DecisionRequest<'_>,
    ) -> Result<ArtifactDecision, ArtifactError> {
        let who = declared_text("who", request.who, MAX_DECLARED_NAME_BYTES)?;
        let what = declared_text("what", request.what, MAX_TEXT_BYTES)?;
        let _lock = RepositoryLock::acquire(&self.path)?;
        let oid = self.require_published(artifact_id)?;
        let (record_id, created_at) = new_record_identity()?;
        let decision = ArtifactDecision {
            schema_version: ARTIFACT_RECORD_SCHEMA_VERSION,
            record_id: record_id.clone(),
            repo_id: self.repo_id.clone(),
            about_artifact_id: oid.to_string(),
            created_at,
            who,
            what,
        };
        self.append_record(
            DECISIONS_DIR,
            oid,
            &record_id,
            &decision,
            &format!("decision {oid}"),
        )?;
        Ok(decision)
    }

    fn validate_anchor(
        &self,
        artifact_id: &str,
        anchor: ArtifactAnchor,
    ) -> Result<Option<ArtifactAnchor>, ArtifactError> {
        let bounded = |name: &str, value: Option<String>, limit: usize| {
            value
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .map(|value| {
                    if value.len() > limit {
                        Err(ArtifactError::InvalidRequest(format!(
                            "anchor {name} exceeds {limit} bytes"
                        )))
                    } else {
                        Ok(value)
                    }
                })
                .transpose()
        };
        let position = bounded("position", anchor.position, MAX_ANCHOR_POSITION_BYTES)?;
        let excerpt = bounded("excerpt", anchor.excerpt, MAX_ANCHOR_EXCERPT_BYTES)?;
        let path = match anchor.path.filter(|path| !path.trim().is_empty()) {
            Some(path) => Some(self.read_file(artifact_id, &path)?.path),
            None => None,
        };
        Ok(
            (path.is_some() || position.is_some() || excerpt.is_some()).then_some(ArtifactAnchor {
                path,
                position,
                excerpt,
            }),
        )
    }

    fn require_published(&self, artifact_id: &str) -> Result<Oid, ArtifactError> {
        let oid = parse_object_id(artifact_id)?;
        if self
            .list_records::<ArtifactVersion>(VERSIONS_DIR, oid)?
            .is_empty()
        {
            self.reject_non_tree(oid)?;
            return Err(self.not_found(oid));
        }
        Ok(oid)
    }

    fn not_found(&self, oid: Oid) -> ArtifactError {
        ArtifactError::NotFound {
            repo_id: self.repo_id.clone(),
            artifact_id: oid.to_string(),
        }
    }

    /// A full object id that names a blob or commit is a malformed request,
    /// not a missing artifact.
    fn reject_non_tree(&self, oid: Oid) -> Result<(), ArtifactError> {
        match self.repository.find_object(oid, None) {
            Ok(object) if object.kind() != Some(ObjectType::Tree) => {
                Err(ArtifactError::WrongObjectType {
                    artifact_id: oid.to_string(),
                    actual: object
                        .kind()
                        .map(|kind| kind.str().to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                })
            }
            _ => Ok(()),
        }
    }

    /// The content tree, if its retention ref still keeps it.
    fn retained_tree(&self, oid: Oid) -> Result<Option<Tree<'_>>, ArtifactError> {
        let ref_name = content_ref_name(oid);
        let reference = match self.repository.find_reference(&ref_name) {
            Ok(reference) => reference,
            Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(error) => return Err(storage(error)),
        };
        let commit = match reference.peel_to_commit() {
            Ok(commit) => commit,
            Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(error) => return Err(storage(error)),
        };
        if commit.tree_id() != oid {
            return Err(ArtifactError::Storage(format!(
                "retention ref {ref_name} points at a commit for tree {}, not {oid}",
                commit.tree_id()
            )));
        }
        match self.repository.find_tree(oid) {
            Ok(tree) => Ok(Some(tree)),
            Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(error) => Err(storage(error)),
        }
    }

    fn retain_tree(
        &self,
        tree_id: Oid,
        kind: ArtifactContentKind,
    ) -> Result<(Oid, String, bool), ArtifactError> {
        let ref_name = content_ref_name(tree_id);
        if let Ok(reference) = self.repository.find_reference(&ref_name) {
            let commit = reference.peel_to_commit().map_err(storage)?;
            if commit.tree_id() == tree_id {
                return Ok((commit.id(), ref_name, false));
            }
            return Err(ArtifactError::Storage(format!(
                "retention ref {ref_name} is corrupt: it keeps tree {}",
                commit.tree_id()
            )));
        }
        let tree = self.repository.find_tree(tree_id).map_err(storage)?;
        let signature = kanna_signature()?;
        let commit = self
            .repository
            .commit(
                None,
                &signature,
                &signature,
                &format!("kanna artifact {} {tree_id}", kind.as_str()),
                &tree,
                &[],
            )
            .map_err(storage)?;
        self.repository
            .reference(&ref_name, commit, false, "kanna: retain artifact tree")
            .map_err(storage)?;
        Ok((commit, ref_name, true))
    }

    fn metadata_tip(&self) -> Result<Option<git2::Commit<'_>>, ArtifactError> {
        match self.repository.find_reference(METADATA_REF) {
            Ok(reference) => reference.peel_to_commit().map(Some).map_err(storage),
            Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(error) => Err(storage(error)),
        }
    }

    /// Append one record. The caller holds the repository lock; the ref is
    /// still moved with a compare-and-swap so a writer that bypassed the lock
    /// fails loudly instead of discarding another writer's record.
    fn append_record(
        &self,
        directory: &str,
        artifact: Oid,
        record_id: &str,
        record: &impl serde::Serialize,
        message: &str,
    ) -> Result<(), ArtifactError> {
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|error| ArtifactError::Storage(format!("cannot encode record: {error}")))?;
        let blob = self.repository.blob(&bytes).map_err(storage)?;
        let parent = self.metadata_tip()?;
        let base = parent
            .as_ref()
            .map(|commit| commit.tree())
            .transpose()
            .map_err(storage)?;
        let artifact_hex = artifact.to_string();
        let file_name = format!("{record_id}.json");
        let components = [directory, &artifact_hex[..2], &artifact_hex, &file_name];
        let tree_id = upsert_path(&self.repository, base.as_ref(), &components, blob)?;
        let tree = self.repository.find_tree(tree_id).map_err(storage)?;
        let signature = kanna_signature()?;
        let parents = parent.iter().collect::<Vec<_>>();
        let commit = self
            .repository
            .commit(None, &signature, &signature, message, &tree, &parents)
            .map_err(storage)?;
        match &parent {
            Some(parent) => self
                .repository
                .reference_matching(METADATA_REF, commit, true, parent.id(), message)
                .map(|_| ()),
            None => self
                .repository
                .reference(METADATA_REF, commit, false, message)
                .map(|_| ()),
        }
        .map_err(|error| {
            ArtifactError::Storage(format!(
                "artifact metadata changed concurrently; record not written: {error}"
            ))
        })
    }

    fn list_records<T: DeserializeOwned + RecordSchema>(
        &self,
        directory: &str,
        artifact: Oid,
    ) -> Result<Vec<T>, ArtifactError> {
        let Some(tip) = self.metadata_tip()? else {
            return Ok(Vec::new());
        };
        let root = tip.tree().map_err(storage)?;
        let artifact_hex = artifact.to_string();
        let path = format!("{directory}/{}/{artifact_hex}", &artifact_hex[..2]);
        let entry = match root.get_path(Path::new(&path)) {
            Ok(entry) => entry,
            Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(storage(error)),
        };
        let records_tree = self.repository.find_tree(entry.id()).map_err(storage)?;
        // Tree entries are sorted by name, and record ids begin with a
        // zero-padded millisecond timestamp, so this is publication order.
        let mut records = Vec::new();
        for entry in records_tree.iter() {
            let blob = self.repository.find_blob(entry.id()).map_err(storage)?;
            let record: T = serde_json::from_slice(blob.content()).map_err(|error| {
                ArtifactError::Storage(format!(
                    "artifact metadata record {path}/{} is unreadable: {error}",
                    entry.name().unwrap_or("?")
                ))
            })?;
            if record.schema_version() > ARTIFACT_RECORD_SCHEMA_VERSION {
                return Err(ArtifactError::Storage(format!(
                    "artifact metadata record {path}/{} was written by a newer Kanna (schema {})",
                    entry.name().unwrap_or("?"),
                    record.schema_version()
                )));
            }
            records.push(record);
        }
        Ok(records)
    }
}

trait RecordSchema {
    fn schema_version(&self) -> u32;
}

impl RecordSchema for ArtifactVersion {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

impl RecordSchema for ArtifactComment {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

impl RecordSchema for ArtifactDecision {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

fn content_ref_name(tree: Oid) -> String {
    format!("{CONTENT_REF_PREFIX}{tree}")
}

fn kanna_signature() -> Result<Signature<'static>, ArtifactError> {
    Signature::now("Kanna", "kanna@localhost").map_err(storage)
}

fn storage(error: git2::Error) -> ArtifactError {
    ArtifactError::Storage(format!("artifact repository error: {}", error.message()))
}

fn new_record_identity() -> Result<(String, String), ArtifactError> {
    let now = SystemTime::now();
    let millis = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let suffix = random_hex(8).map_err(ArtifactError::Storage)?;
    Ok((format!("{millis:013}-{suffix}"), rfc3339_utc(now)))
}

/// Parse a full, canonical (lowercase, 40 hex) object id. Abbreviations and
/// revision expressions are refused so an id always names one exact tree.
pub(crate) fn parse_object_id(value: &str) -> Result<Oid, ArtifactError> {
    let valid = value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid {
        return Err(ArtifactError::InvalidId(value.chars().take(80).collect()));
    }
    Oid::from_str(value).map_err(|_| ArtifactError::InvalidId(value.to_string()))
}

fn declared_text(name: &str, value: &str, limit: usize) -> Result<String, ArtifactError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ArtifactError::InvalidRequest(format!(
            "{name} must not be empty"
        )));
    }
    if value.len() > limit {
        return Err(ArtifactError::InvalidRequest(format!(
            "{name} exceeds {limit} bytes"
        )));
    }
    Ok(value.to_string())
}

fn upsert_path(
    repository: &Repository,
    base: Option<&Tree<'_>>,
    components: &[&str],
    blob: Oid,
) -> Result<Oid, ArtifactError> {
    let mut builder = repository.treebuilder(base).map_err(storage)?;
    let (name, rest) = components
        .split_first()
        .ok_or_else(|| ArtifactError::Storage("empty metadata path".to_string()))?;
    if rest.is_empty() {
        builder
            .insert(name, blob, FILE_MODE_BLOB)
            .map_err(storage)?;
    } else {
        let child = match base.and_then(|tree| tree.get_name(name)) {
            Some(entry) if entry.kind() == Some(ObjectType::Tree) => {
                Some(repository.find_tree(entry.id()).map_err(storage)?)
            }
            _ => None,
        };
        let child_id = upsert_path(repository, child.as_ref(), rest, blob)?;
        builder
            .insert(name, child_id, FILE_MODE_TREE)
            .map_err(storage)?;
    }
    builder.write().map_err(storage)
}

fn collect_files(
    repository: &Repository,
    tree: &Tree<'_>,
    prefix: &str,
    files: &mut Vec<ArtifactFileEntry>,
) -> Result<(), ArtifactError> {
    for entry in tree.iter() {
        let name = entry.name().unwrap_or_default();
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        match entry.kind() {
            Some(ObjectType::Tree) => {
                let child = repository.find_tree(entry.id()).map_err(storage)?;
                collect_files(repository, &child, &path, files)?;
            }
            Some(ObjectType::Blob) => {
                let size = repository
                    .find_blob(entry.id())
                    .map(|blob| blob.size() as u64)
                    .map_err(storage)?;
                files.push(ArtifactFileEntry { path, size });
            }
            _ => {}
        }
    }
    Ok(())
}

/// Normalize a path inside an artifact tree: `/`-separated, relative, no
/// empty, `.` or `..` components.
pub(crate) fn normalize_artifact_path(path: &str) -> Result<String, ArtifactError> {
    let invalid = || ArtifactError::InvalidPath(format!("invalid artifact file path: {path:?}"));
    if path.is_empty() || path.contains('\0') || path.contains('\\') {
        return Err(invalid());
    }
    let components = path.split('/').collect::<Vec<_>>();
    if components
        .iter()
        .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        return Err(invalid());
    }
    Ok(components.join("/"))
}

fn resolve_entrypoint(
    tree: &Tree<'_>,
    kind: ArtifactContentKind,
    requested: Option<&str>,
    single_file_name: Option<&str>,
) -> Result<Option<String>, ArtifactError> {
    let entrypoint = match requested.map(str::trim).filter(|value| !value.is_empty()) {
        Some(requested) => {
            let normalized = normalize_artifact_path(requested)?;
            let is_file = tree
                .get_path(Path::new(&normalized))
                .is_ok_and(|entry| entry.kind() == Some(ObjectType::Blob));
            if !is_file {
                return Err(ArtifactError::InvalidEntrypoint(format!(
                    "entrypoint {normalized} is not a file in the published tree"
                )));
            }
            Some(normalized)
        }
        None => single_file_name.map(str::to_string).or_else(|| {
            tree.get_name("index.html")
                .filter(|entry| entry.kind() == Some(ObjectType::Blob))
                .map(|_| "index.html".to_string())
        }),
    };
    if kind == ArtifactContentKind::Mockup {
        let is_html = entrypoint.as_deref().is_some_and(|path| {
            let lower = path.to_ascii_lowercase();
            lower.ends_with(".html") || lower.ends_with(".htm")
        });
        if !is_html {
            return Err(ArtifactError::InvalidEntrypoint(
                "a mockup needs an HTML entrypoint: publish a directory with index.html or name the entrypoint"
                    .to_string(),
            ));
        }
    }
    Ok(entrypoint)
}

// ---------------------------------------------------------------------------
// Reading a payload out of a task workspace
// ---------------------------------------------------------------------------

enum PayloadNode {
    File(Vec<u8>),
    Directory(BTreeMap<String, PayloadNode>),
}

struct Payload {
    root: BTreeMap<String, PayloadNode>,
    single_file_name: Option<String>,
    file_count: usize,
    total_bytes: u64,
}

struct PayloadBudget {
    limits: PublishLimits,
    file_count: usize,
    total_bytes: u64,
}

fn read_payload(
    workspace_root: &Path,
    source_path: &str,
    limits: PublishLimits,
) -> Result<Payload, ArtifactError> {
    let relative = normalize_source_path(workspace_root, source_path)?;
    let root = crate::repo_browser::open_browse_root(workspace_root).map_err(|_| {
        ArtifactError::WorkspaceUnavailable(format!(
            "task workspace {} is unavailable",
            workspace_root.display()
        ))
    })?;
    let parent = relative.parent().unwrap_or(Path::new(""));
    let name = relative
        .file_name()
        .ok_or_else(|| ArtifactError::InvalidPath("source path names no file".to_string()))?;
    let display = display_path(&relative);
    let parent_fd = crate::repo_browser::open_directory_from_root(&root, parent)
        .map_err(|error| map_source_error(error, &display))?;
    let target = crate::repo_browser::open_child(&parent_fd, name)
        .map_err(|error| map_source_error(error, &display))?;
    let mut budget = PayloadBudget {
        limits,
        file_count: 0,
        total_bytes: 0,
    };
    let (node, single_file_name) = match read_node(target, &display, 0, &mut budget)? {
        Some(PayloadNode::Directory(children)) => (children, None),
        Some(PayloadNode::File(bytes)) => {
            let name = name
                .to_str()
                .ok_or_else(|| non_utf8_name(&display))?
                .to_string();
            (
                BTreeMap::from([(name.clone(), PayloadNode::File(bytes))]),
                Some(name),
            )
        }
        None => (BTreeMap::new(), None),
    };
    if node.is_empty() {
        return Err(ArtifactError::InvalidPath(format!(
            "{display} contains no files to publish"
        )));
    }
    Ok(Payload {
        root: node,
        single_file_name,
        file_count: budget.file_count,
        total_bytes: budget.total_bytes,
    })
}

/// Read one opened entry. Directories and regular files are all a payload
/// may contain; anything else refuses the whole publication. Returns `None`
/// for a directory with no files, which Git cannot represent.
fn read_node(
    descriptor: OwnedFd,
    display: &str,
    depth: usize,
    budget: &mut PayloadBudget,
) -> Result<Option<PayloadNode>, ArtifactError> {
    let file = std::fs::File::from(descriptor);
    let metadata = file
        .metadata()
        .map_err(|error| ArtifactError::Storage(format!("cannot inspect {display}: {error}")))?;
    if metadata.is_file() {
        budget.file_count += 1;
        if budget.file_count > budget.limits.max_files {
            return Err(ArtifactError::TooLarge(format!(
                "publication exceeds {} files",
                budget.limits.max_files
            )));
        }
        if metadata.len() > budget.limits.max_file_bytes {
            return Err(ArtifactError::TooLarge(format!(
                "{display} exceeds the {} byte per-file limit",
                budget.limits.max_file_bytes
            )));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        (&file)
            .take(budget.limits.max_file_bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| ArtifactError::Storage(format!("cannot read {display}: {error}")))?;
        if bytes.len() as u64 > budget.limits.max_file_bytes {
            return Err(ArtifactError::TooLarge(format!(
                "{display} exceeds the {} byte per-file limit",
                budget.limits.max_file_bytes
            )));
        }
        budget.total_bytes += bytes.len() as u64;
        if budget.total_bytes > budget.limits.max_total_bytes {
            return Err(ArtifactError::TooLarge(format!(
                "publication exceeds {} bytes",
                budget.limits.max_total_bytes
            )));
        }
        return Ok(Some(PayloadNode::File(bytes)));
    }
    if !metadata.is_dir() {
        return Err(ArtifactError::InvalidPath(format!(
            "{display} is not a regular file or directory"
        )));
    }
    if depth >= budget.limits.max_depth {
        return Err(ArtifactError::TooLarge(format!(
            "{display} is nested deeper than {} directories",
            budget.limits.max_depth
        )));
    }
    let directory = OwnedFd::from(file);
    let mut children = BTreeMap::new();
    for name in crate::repo_browser::directory_names(&directory)
        .map_err(|error| map_source_error(error, display))?
    {
        let child_display = format!("{display}/{}", name.to_string_lossy());
        let Some(utf8_name) = name.to_str() else {
            return Err(non_utf8_name(&child_display));
        };
        if utf8_name == ".git" {
            return Err(ArtifactError::InvalidPath(format!(
                "{child_display}: .git entries cannot be published"
            )));
        }
        let child = crate::repo_browser::open_child(&directory, &name)
            .map_err(|error| map_source_error(error, &child_display))?;
        if let Some(node) = read_node(child, &child_display, depth + 1, budget)? {
            children.insert(utf8_name.to_string(), node);
        }
    }
    Ok((!children.is_empty()).then_some(PayloadNode::Directory(children)))
}

fn write_tree(
    repository: &Repository,
    entries: &BTreeMap<String, PayloadNode>,
) -> Result<Oid, ArtifactError> {
    let mut builder = repository.treebuilder(None).map_err(storage)?;
    for (name, node) in entries {
        match node {
            PayloadNode::File(bytes) => {
                let blob = repository.blob(bytes).map_err(storage)?;
                builder
                    .insert(name, blob, FILE_MODE_BLOB)
                    .map_err(storage)?;
            }
            PayloadNode::Directory(children) => {
                let tree = write_tree(repository, children)?;
                builder
                    .insert(name, tree, FILE_MODE_TREE)
                    .map_err(storage)?;
            }
        }
    }
    builder.write().map_err(storage)
}

fn normalize_source_path(root: &Path, requested: &str) -> Result<PathBuf, ArtifactError> {
    let invalid = || {
        ArtifactError::InvalidPath(
            "path must name a file or directory inside the task workspace".to_string(),
        )
    };
    if requested.trim().is_empty() || requested.contains('\0') {
        return Err(invalid());
    }
    let requested = Path::new(requested);
    let relative = if requested.is_absolute() {
        requested.strip_prefix(root).map_err(|_| invalid())?
    } else {
        requested
    };
    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) if name == ".git" => {
                return Err(ArtifactError::InvalidPath(
                    ".git entries cannot be published".to_string(),
                ))
            }
            Component::Normal(name) => normalized.push(name),
            Component::CurDir => {}
            _ => return Err(invalid()),
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(invalid());
    }
    Ok(normalized)
}

fn map_source_error(error: BrowseError, display: &str) -> ArtifactError {
    match error {
        BrowseError::TargetNotFound => ArtifactError::SourceNotFound(display.to_string()),
        BrowseError::InvalidPath | BrowseError::NotFile => ArtifactError::InvalidPath(format!(
            "{display} is a symbolic link, is unreadable, or leaves the task workspace"
        )),
        BrowseError::RootNotFound => {
            ArtifactError::WorkspaceUnavailable("task workspace is unavailable".to_string())
        }
        BrowseError::Internal(message) => ArtifactError::Storage(message),
    }
}

fn non_utf8_name(display: &str) -> ArtifactError {
    ArtifactError::InvalidPath(format!("{display} is not a UTF-8 file name"))
}

fn display_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Exclusive advisory lock over one artifact repository, shared by every
/// Kanna process on the machine. Released when dropped.
struct RepositoryLock {
    _file: std::fs::File,
}

impl RepositoryLock {
    fn acquire(repository_path: &Path) -> Result<Self, ArtifactError> {
        let path = repository_path.join(LOCK_FILE_NAME);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|error| {
                ArtifactError::Storage(format!(
                    "cannot open artifact repository lock {}: {error}",
                    path.display()
                ))
            })?;
        loop {
            // SAFETY: `file` owns a valid descriptor for the whole call.
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if result == 0 {
                return Ok(Self { _file: file });
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(ArtifactError::Storage(format!(
                    "cannot lock artifact repository {}: {error}",
                    repository_path.display()
                )));
            }
        }
    }
}
