//! Artifact repository routes (spec §8): publish from a task workspace, read
//! and annotate by exact tree id, and open a read-only browser preview.
//!
//! Publishing is task-scoped because the bytes come from that task's
//! workspace. Everything else is repository-scoped: an artifact belongs to
//! the working repository's artifact store, not to the task that made it.
//!
//! Push and fetch share one artifact with another Kanna home through the
//! repository's configured `artifacts.remote`; the remote is never a request
//! parameter, so a caller cannot direct content anywhere configuration did
//! not name.

use super::lan_trust::PrivilegedTaskAccess;
use super::state::AppState;
use super::task_blockers::resolve_existing_task_id;
use crate::artifacts::remote::{self, ArtifactRemote};
use crate::artifacts::store::{
    parse_object_id, ArtifactStore, CommentRequest, DecisionRequest, PublishLimits, PublishRequest,
};
use crate::artifacts::types::{ArtifactAnchor, ArtifactContentKind};
use crate::artifacts::{resolve_repository_path, ArtifactError};
use crate::db::{Db, Repo};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PublishArtifactRequest {
    path: String,
    kind: String,
    #[serde(default)]
    entrypoint: Option<String>,
    #[serde(default)]
    previous: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecordCommentRequest {
    author: String,
    body: String,
    #[serde(default)]
    anchor: Option<AnchorRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AnchorRequest {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    position: Option<String>,
    #[serde(default)]
    excerpt: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ArtifactFileQuery {
    path: String,
}

/// One file of a retained tree, for a client that cannot reach this machine's
/// loopback preview listener (a phone over LAN or relay) and renders the
/// artifact itself. Bounded like a task file download, because the whole body
/// crosses the relay as one message.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ArtifactFileContent {
    repo_id: String,
    artifact_id: String,
    path: String,
    media_type: String,
    size: u64,
    data_base64: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecordDecisionRequest {
    who: String,
    what: String,
}

pub(super) async fn publish_artifact(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(request): Json<PublishArtifactRequest>,
) -> Response {
    blocking("artifact publish", move || {
        let kind = parse_kind(&request.kind)?;
        let db = open_db(&state)?;
        let task_id = resolve_existing_task_id(&db, &task_id)
            .map_err(|error| Box::new(error.into_response()))?;
        let item = db
            .get_pipeline_item(&task_id)
            .map_err(db_error)?
            .ok_or_else(|| refusal(StatusCode::NOT_FOUND, "task_not_found", "task not found"))?;
        if item.closed_at.is_some() {
            return Err(refusal(
                StatusCode::CONFLICT,
                "task_closed",
                "a closed task has no workspace to publish from",
            ));
        }
        let workspace = db
            .get_task_worktree_path(&task_id)
            .map_err(db_error)?
            .ok_or_else(|| {
                refusal(
                    StatusCode::CONFLICT,
                    "workspace_unavailable",
                    "the task workspace is unavailable",
                )
            })?;
        let repo = find_repo(&db, &item.repo_id)?;
        let policy = policy(&state, &repo)?;
        let path = resolve_repository_path(
            &state.artifact_storage,
            &repo.id,
            std::path::Path::new(&repo.path),
            policy.repository_path.as_deref(),
        )
        .map_err(artifact_error)?;
        let store = ArtifactStore::open_or_create(&path, &repo.id).map_err(artifact_error)?;
        let published = store
            .publish(PublishRequest {
                task_id: &task_id,
                workspace_root: std::path::Path::new(&workspace),
                source_path: &request.path,
                kind,
                entrypoint: request.entrypoint.as_deref(),
                previous: request.previous.as_deref(),
                retention: policy.retention,
                limits: PublishLimits::default(),
            })
            .map_err(artifact_error)?;
        Ok((StatusCode::CREATED, Json(published)).into_response())
    })
    .await
}

pub(super) async fn get_artifact(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((repo_id, artifact_id)): Path<(String, String)>,
) -> Response {
    blocking("artifact read", move || {
        let store = existing_store(&state, &repo_id, &artifact_id)?;
        let detail = store.detail(&artifact_id).map_err(artifact_error)?;
        Ok(Json(detail).into_response())
    })
    .await
}

pub(super) async fn read_artifact_file(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((repo_id, artifact_id)): Path<(String, String)>,
    Query(query): Query<ArtifactFileQuery>,
) -> Response {
    blocking("artifact file read", move || {
        let store = existing_store(&state, &repo_id, &artifact_id)?;
        let blob = store
            .read_file(&artifact_id, &query.path)
            .map_err(artifact_error)?;
        let size = blob.bytes.len() as u64;
        if size > crate::task_files::MAX_TASK_FILE_BYTES {
            let mut body = json!({
                "error": "file_too_large",
                "message": format!(
                    "{} exceeds the {} byte limit for reading one artifact file",
                    blob.path,
                    crate::task_files::MAX_TASK_FILE_BYTES
                ),
            });
            body["artifactId"] = json!(artifact_id);
            body["path"] = json!(blob.path);
            return Err(Box::new(
                (StatusCode::PAYLOAD_TOO_LARGE, Json(body)).into_response(),
            ));
        }
        Ok(Json(ArtifactFileContent {
            repo_id,
            artifact_id,
            media_type: super::artifact_preview::media_type(&blob.path),
            size,
            data_base64: base64::engine::general_purpose::STANDARD.encode(&blob.bytes),
            path: blob.path,
        })
        .into_response())
    })
    .await
}

pub(super) async fn record_artifact_comment(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((repo_id, artifact_id)): Path<(String, String)>,
    Json(request): Json<RecordCommentRequest>,
) -> Response {
    blocking("artifact comment", move || {
        let store = existing_store(&state, &repo_id, &artifact_id)?;
        let comment = store
            .record_comment(
                &artifact_id,
                CommentRequest {
                    author: &request.author,
                    body: &request.body,
                    anchor: request.anchor.map(|anchor| ArtifactAnchor {
                        path: anchor.path,
                        position: anchor.position,
                        excerpt: anchor.excerpt,
                    }),
                },
            )
            .map_err(artifact_error)?;
        Ok((StatusCode::CREATED, Json(comment)).into_response())
    })
    .await
}

pub(super) async fn record_artifact_decision(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((repo_id, artifact_id)): Path<(String, String)>,
    Json(request): Json<RecordDecisionRequest>,
) -> Response {
    blocking("artifact decision", move || {
        let store = existing_store(&state, &repo_id, &artifact_id)?;
        let decision = store
            .record_decision(
                &artifact_id,
                DecisionRequest {
                    who: &request.who,
                    what: &request.what,
                },
            )
            .map_err(artifact_error)?;
        Ok((StatusCode::CREATED, Json(decision)).into_response())
    })
    .await
}

pub(super) async fn push_artifact(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((repo_id, artifact_id)): Path<(String, String)>,
) -> Response {
    blocking("artifact push", move || {
        parse_object_id(&artifact_id).map_err(artifact_error)?;
        let (repo, path, policy) = repository_location(&state, &repo_id)?;
        let remote = configured_remote(&state, &repo, &path, &policy)?;
        let store = ArtifactStore::open_existing(&path, &repo.id)
            .map_err(artifact_error)?
            .ok_or_else(|| {
                artifact_error(ArtifactError::NotFound {
                    repo_id: repo.id.clone(),
                    artifact_id: artifact_id.clone(),
                })
            })?;
        let outcome = remote::push(&store, &remote, &artifact_id).map_err(artifact_error)?;
        Ok(Json(outcome).into_response())
    })
    .await
}

pub(super) async fn fetch_artifact(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((repo_id, artifact_id)): Path<(String, String)>,
) -> Response {
    blocking("artifact fetch", move || {
        parse_object_id(&artifact_id).map_err(artifact_error)?;
        let (repo, path, policy) = repository_location(&state, &repo_id)?;
        let remote = configured_remote(&state, &repo, &path, &policy)?;
        // Receiving is how a home that never published anything gets its
        // first artifact, so this is the one read path that creates the store.
        let store = ArtifactStore::open_or_create(&path, &repo.id).map_err(artifact_error)?;
        let outcome = remote::fetch(&store, &remote, &artifact_id).map_err(artifact_error)?;
        Ok(Json(outcome).into_response())
    })
    .await
}

/// Which shared artifact remote a repository is configured with, and where
/// that configuration came from, so a client can show what push and fetch
/// will use before either runs. Read-only: nothing is created or contacted.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ArtifactRemoteStatus {
    repo_id: String,
    configured: bool,
    /// The remote as it may be shown, URL credentials removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    remote: Option<String>,
    /// `committed` or `machine-local`.
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'static str>,
    /// The repository-relative file that configured it.
    #[serde(skip_serializing_if = "Option::is_none")]
    config_file: Option<&'static str>,
    /// Why a configured remote is unusable; push and fetch refuse it with
    /// the same code.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ArtifactRemoteStatusError>,
}

#[derive(Debug, Serialize)]
pub(super) struct ArtifactRemoteStatusError {
    code: &'static str,
    message: String,
}

pub(super) async fn get_artifact_remote(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Response {
    blocking("artifact remote status", move || {
        let (repo, path, policy) = repository_location(&state, &repo_id)?;
        let Some(configured) = policy.remote.as_deref() else {
            return Ok(Json(ArtifactRemoteStatus {
                repo_id: repo.id,
                configured: false,
                remote: None,
                source: None,
                config_file: None,
                error: None,
            })
            .into_response());
        };
        let (source, config_file) = match policy.remote_source {
            Some(crate::task_creator::ArtifactRemoteSource::MachineLocal) => {
                ("machine-local", ".kanna/config.local.json")
            }
            Some(crate::task_creator::ArtifactRemoteSource::Committed) | None => {
                ("committed", ".kanna/config.json")
            }
        };
        let (remote, error) =
            match ArtifactRemote::parse(configured, state.artifact_storage.home(), &path) {
                Ok(remote) => (Some(remote.display().to_string()), None),
                // The message names the remote only through `redact`.
                Err(error) => (
                    None,
                    Some(ArtifactRemoteStatusError {
                        code: error.code(),
                        message: error.to_string(),
                    }),
                ),
            };
        Ok(Json(ArtifactRemoteStatus {
            repo_id: repo.id,
            configured: true,
            remote,
            source: Some(source),
            config_file: Some(config_file),
            error,
        })
        .into_response())
    })
    .await
}

fn repository_location(
    state: &AppState,
    repo_id: &str,
) -> Result<
    (
        Repo,
        std::path::PathBuf,
        crate::task_creator::RepoArtifactPolicy,
    ),
    Refusal,
> {
    let db = open_db(state)?;
    let repo = find_repo(&db, repo_id)?;
    let policy = policy(state, &repo)?;
    let path = resolve_repository_path(
        &state.artifact_storage,
        &repo.id,
        std::path::Path::new(&repo.path),
        policy.repository_path.as_deref(),
    )
    .map_err(artifact_error)?;
    Ok((repo, path, policy))
}

fn configured_remote(
    state: &AppState,
    repo: &Repo,
    store_path: &std::path::Path,
    policy: &crate::task_creator::RepoArtifactPolicy,
) -> Result<ArtifactRemote, Refusal> {
    let configured = policy.remote.as_deref().ok_or_else(|| {
        artifact_error(ArtifactError::RemoteNotConfigured {
            repo_id: repo.id.clone(),
        })
    })?;
    ArtifactRemote::parse(configured, state.artifact_storage.home(), store_path)
        .map_err(artifact_error)
}

pub(super) async fn open_artifact_preview(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((repo_id, artifact_id)): Path<(String, String)>,
) -> Response {
    let prepared = {
        let state = Arc::clone(&state);
        let repo_id = repo_id.clone();
        let artifact_id = artifact_id.clone();
        super::blocking::run_handler_blocking("artifact preview", move || {
            Ok((|| {
                let (store, repository_path) =
                    existing_store_with_path(&state, &repo_id, &artifact_id)?;
                let entrypoint = store.entrypoint(&artifact_id).map_err(artifact_error)?;
                Ok::<_, Refusal>((repository_path, entrypoint))
            })())
        })
        .await
    };
    let (repository_path, entrypoint) = match prepared {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(response)) => return *response,
        Err(error) => return error.into_response(),
    };
    let Some(entrypoint) = entrypoint else {
        return *refusal(
            StatusCode::CONFLICT,
            "no_entrypoint",
            "this artifact has no entrypoint to open; read its files with kanna_get_artifact",
        );
    };
    match state
        .artifact_previews
        .open(repo_id, artifact_id, repository_path, entrypoint)
        .await
    {
        Ok(opened) => Json(opened).into_response(),
        Err(message) => *refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            "preview_unavailable",
            &message,
        ),
    }
}

pub(super) async fn close_artifact_preview(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((repo_id, artifact_id)): Path<(String, String)>,
) -> Response {
    if let Err(error) = parse_object_id(&artifact_id) {
        return *artifact_error(error);
    }
    let closed = state.artifact_previews.close(&repo_id, &artifact_id).await;
    Json(json!({ "repoId": repo_id, "artifactId": artifact_id, "closed": closed })).into_response()
}

async fn blocking(
    label: &'static str,
    work: impl FnOnce() -> Result<Response, Refusal> + Send + 'static,
) -> Response {
    match super::blocking::run_handler_blocking(label, move || Ok(work())).await {
        Ok(Ok(response)) => response,
        Ok(Err(response)) => *response,
        Err(error) => error.into_response(),
    }
}

fn parse_kind(value: &str) -> Result<ArtifactContentKind, Refusal> {
    match value {
        "document" => Ok(ArtifactContentKind::Document),
        "mockup" => Ok(ArtifactContentKind::Mockup),
        "media" => Ok(ArtifactContentKind::Media),
        "report" => Ok(ArtifactContentKind::Report),
        _ => Err(refusal(
            StatusCode::BAD_REQUEST,
            "invalid_kind",
            "kind must be one of document, mockup, media, report",
        )),
    }
}

fn existing_store(
    state: &AppState,
    repo_id: &str,
    artifact_id: &str,
) -> Result<ArtifactStore, Refusal> {
    existing_store_with_path(state, repo_id, artifact_id).map(|(store, _)| store)
}

/// Open a repository's artifact store for a read or annotation. A repository
/// with no artifact store yet has, by definition, no such artifact; nothing is
/// created on this path.
fn existing_store_with_path(
    state: &AppState,
    repo_id: &str,
    artifact_id: &str,
) -> Result<(ArtifactStore, std::path::PathBuf), Refusal> {
    parse_object_id(artifact_id).map_err(artifact_error)?;
    let db = open_db(state)?;
    let repo = find_repo(&db, repo_id)?;
    let policy = policy(state, &repo)?;
    let path = resolve_repository_path(
        &state.artifact_storage,
        &repo.id,
        std::path::Path::new(&repo.path),
        policy.repository_path.as_deref(),
    )
    .map_err(artifact_error)?;
    match ArtifactStore::open_existing(&path, &repo.id).map_err(artifact_error)? {
        Some(store) => Ok((store, path)),
        None => Err(artifact_error(ArtifactError::NotFound {
            repo_id: repo.id,
            artifact_id: artifact_id.to_string(),
        })),
    }
}

fn policy(
    state: &AppState,
    repo: &Repo,
) -> Result<crate::task_creator::RepoArtifactPolicy, Refusal> {
    crate::task_creator::load_repo_artifact_policy(&state.repo_definitions, repo).map_err(|error| {
        refusal(
            StatusCode::CONFLICT,
            "artifact_config_unresolved",
            &format!("repository configuration could not be resolved: {error}"),
        )
    })
}

fn find_repo(db: &Db, repo_id: &str) -> Result<Repo, Refusal> {
    db.get_repo(repo_id).map_err(db_error)?.ok_or_else(|| {
        refusal(
            StatusCode::NOT_FOUND,
            "repo_not_found",
            &format!("repository {repo_id} not found"),
        )
    })
}

fn open_db(state: &AppState) -> Result<Db, Refusal> {
    Db::open(&state.config().db_path).map_err(|error| {
        refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            &format!("db error: {error}"),
        )
    })
}

fn db_error(error: rusqlite::Error) -> Refusal {
    refusal(
        StatusCode::INTERNAL_SERVER_ERROR,
        "db_error",
        &format!("db error: {error}"),
    )
}

/// An error response. Boxed: a `Response` is large for an `Err` variant.
type Refusal = Box<Response>;

fn refusal(status: StatusCode, code: &str, message: &str) -> Refusal {
    Box::new((status, Json(json!({ "error": code, "message": message }))).into_response())
}

pub(super) fn artifact_status(error: &ArtifactError) -> StatusCode {
    match error {
        ArtifactError::InvalidRequest(_)
        | ArtifactError::InvalidId(_)
        | ArtifactError::WrongObjectType { .. }
        | ArtifactError::InvalidPath(_)
        | ArtifactError::InvalidEntrypoint(_)
        | ArtifactError::InvalidPrevious { .. } => StatusCode::BAD_REQUEST,
        ArtifactError::SourceNotFound(_)
        | ArtifactError::NotFound { .. }
        | ArtifactError::ContentMissing { .. }
        | ArtifactError::FileNotFound { .. } => StatusCode::NOT_FOUND,
        ArtifactError::WorkspaceUnavailable(_) | ArtifactError::Location(_) => StatusCode::CONFLICT,
        ArtifactError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
        ArtifactError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        ArtifactError::NotOnRemote { .. } => StatusCode::NOT_FOUND,
        ArtifactError::RemoteNotConfigured { .. }
        | ArtifactError::InvalidRemote(_)
        | ArtifactError::RemoteConflict { .. } => StatusCode::CONFLICT,
        ArtifactError::RemoteFailed(_) => StatusCode::BAD_GATEWAY,
    }
}

fn artifact_error(error: ArtifactError) -> Refusal {
    let mut body = json!({ "error": error.code(), "message": error.to_string() });
    match &error {
        ArtifactError::NotFound {
            repo_id,
            artifact_id,
        }
        | ArtifactError::ContentMissing {
            repo_id,
            artifact_id,
        }
        | ArtifactError::InvalidPrevious {
            repo_id,
            artifact_id,
        } => {
            body["repoId"] = json!(repo_id);
            body["artifactId"] = json!(artifact_id);
        }
        ArtifactError::FileNotFound { artifact_id, path } => {
            body["artifactId"] = json!(artifact_id);
            body["path"] = json!(path);
        }
        ArtifactError::NotOnRemote {
            remote,
            artifact_id,
        } => {
            body["remote"] = json!(remote);
            body["artifactId"] = json!(artifact_id);
        }
        ArtifactError::RemoteConflict { remote, refs } => {
            body["remote"] = json!(remote);
            body["refs"] = json!(refs);
        }
        ArtifactError::RemoteNotConfigured { repo_id } => {
            body["repoId"] = json!(repo_id);
        }
        _ => {}
    }
    Box::new((artifact_status(&error), Json(body)).into_response())
}
