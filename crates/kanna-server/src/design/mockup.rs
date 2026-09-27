//! HTML mockups (docs/specs/app-design.md §5): the agent writes a mockup in
//! the design's disposable repository and publishes it as the page a
//! position shows. The page becomes an immutable version in the repository's
//! artifact store, so what the person sees is exactly what was published and
//! renders through the store's sandboxed preview, never from the agent's
//! files directly. Republishing a position chains the new version to the
//! last one (`previous`).

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::approval::artifact_store;
use super::service::{self, DesignError};
use super::DesignRuntime;
use crate::artifacts::store::{PublishLimits, PublishRequest};
use crate::artifacts::types::ArtifactContentKind;
use crate::db::Db;
use crate::http_api::AppState;

const OP_KIND: &str = "mockup";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishMockupRequest {
    pub(crate) op_id: String,
    /// A file or directory in the design's disposable repository.
    pub(crate) path: String,
    /// The position that shows it; the current one when absent.
    #[serde(default)]
    pub(crate) position: Option<String>,
    /// The page to open, inside a published directory; `index.html` (or the
    /// single file) when absent.
    #[serde(default)]
    pub(crate) entrypoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishMockupResult {
    pub(crate) op_id: String,
    pub(crate) position: String,
    pub(crate) artifact_id: String,
    pub(crate) entrypoint: String,
    /// Whether the position now shows a different page than before.
    pub(crate) changed: bool,
    /// A retried `op_id`: this is the first call's result, applied once.
    #[serde(default)]
    pub(crate) replayed: bool,
}

pub(crate) fn publish(
    state: &AppState,
    db: &Db,
    runtime: &DesignRuntime,
    db_path: &str,
    task_id: &str,
    request: &PublishMockupRequest,
) -> Result<PublishMockupResult, DesignError> {
    if let Some((kind, result)) = db.design_agent_op(task_id, &request.op_id)? {
        if kind != OP_KIND {
            return Err(DesignError::Conflict {
                message: format!(
                    "op_id {} was already used for a different call ({kind})",
                    request.op_id
                ),
            });
        }
        let mut replayed: PublishMockupResult =
            serde_json::from_str(&result).map_err(DesignError::internal)?;
        replayed.replayed = true;
        return Ok(replayed);
    }
    let (stage, session) = service::ensure_session(db, task_id)?;
    service::require_designing(&stage, &session)?;
    let position = request
        .position
        .clone()
        .unwrap_or_else(|| session.position.clone());
    service::require_position(&stage, &position)?;
    let repo_id = db
        .get_pipeline_item(task_id)?
        .ok_or_else(|| DesignError::NotFound {
            message: format!("task not found: {task_id}"),
        })?
        .repo_id;
    let scratch =
        service::scratch_repository(db_path, task_id, session.epoch).ok_or_else(|| {
            DesignError::Unavailable {
                message: "the design's disposable repository could not be created".into(),
            }
        })?;
    let source_path = relative_to(&scratch, &request.path)?;
    let previous = db
        .design_mockups(task_id, session.epoch)?
        .into_iter()
        .find(|mockup| mockup.position == position)
        .map(|mockup| mockup.artifact_id);
    let repo = db
        .get_repo(&repo_id)?
        .ok_or_else(|| DesignError::NotFound {
            message: format!("repository {repo_id} not found"),
        })?;
    let retention = crate::task_creator::load_repo_artifact_policy(state.repo_definitions(), &repo)
        .map_err(|error| DesignError::Unavailable {
            message: format!("repository configuration could not be resolved: {error}"),
        })?
        .retention;
    let store = artifact_store(state, db, &repo_id)?;
    let published = store
        .publish(PublishRequest {
            task_id,
            workspace_root: &scratch,
            source_path: &source_path,
            kind: ArtifactContentKind::Mockup,
            entrypoint: request.entrypoint.as_deref(),
            previous: previous.as_deref(),
            retention,
            limits: PublishLimits::default(),
        })
        .map_err(|error| DesignError::invalid(format!("the mockup was not published: {error}")))?;
    let entrypoint = published
        .version
        .entrypoint
        .clone()
        .ok_or_else(|| DesignError::internal("a published mockup has no entrypoint"))?;
    let changed = db.set_design_mockup(
        task_id,
        session.epoch,
        &position,
        &repo_id,
        &published.artifact_id,
        &entrypoint,
        &source_path,
    )?;
    let result = PublishMockupResult {
        op_id: request.op_id.clone(),
        position,
        artifact_id: published.artifact_id,
        entrypoint,
        changed,
        replayed: false,
    };
    db.record_design_agent_op(
        task_id,
        &request.op_id,
        OP_KIND,
        &serde_json::to_string(&result).map_err(DesignError::internal)?,
    )?;
    if changed {
        runtime.feed_changed(task_id);
    }
    Ok(result)
}

/// `path` as the store reads it: relative to the disposable repository. An
/// absolute path is accepted when it is inside it, since that is where the
/// agent was told to work.
fn relative_to(scratch: &Path, path: &str) -> Result<String, DesignError> {
    let requested = Path::new(path.trim());
    if requested.as_os_str().is_empty() {
        return Err(DesignError::invalid("path is empty"));
    }
    if !requested.is_absolute() {
        return Ok(requested.to_string_lossy().into_owned());
    }
    let canonical_scratch = scratch.canonicalize().map_err(DesignError::internal)?;
    let canonical = requested.canonicalize().map_err(|error| {
        DesignError::invalid(format!("{} cannot be read: {error}", requested.display()))
    })?;
    canonical
        .strip_prefix(&canonical_scratch)
        .map(|relative| relative.to_string_lossy().into_owned())
        .map_err(|_| {
            DesignError::invalid(format!(
                "{} is outside the design's disposable repository {}; write the mockup there",
                requested.display(),
                scratch.display()
            ))
        })
}
