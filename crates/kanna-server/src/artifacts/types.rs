//! Artifact descriptor wire types (spec §8). T6 owns these; the TypeScript
//! mirror is `packages/core/src/artifacts/types.ts` and must change with them.
//!
//! Identity is the Git tree object id of the artifact's content in the
//! artifact repository. Everything else here is metadata *about* a tree id:
//! a version record says who published it and what it follows, and a comment
//! or decision names the exact tree it was made about. None of these records
//! is stored inside the content tree, so recording one never changes an id.

use serde::{Deserialize, Serialize};

/// Version of every persisted record below. Readers refuse a newer version
/// rather than silently dropping fields they do not understand.
pub(crate) const ARTIFACT_RECORD_SCHEMA_VERSION: u32 = 1;

/// What stored content is. `commit` and `pr` are not here: they are
/// [`ArtifactReference`] variants that point outside the artifact repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ArtifactContentKind {
    Document,
    Mockup,
    Media,
    Report,
}

impl ArtifactContentKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Mockup => "mockup",
            Self::Media => "media",
            Self::Report => "report",
        }
    }
}

/// Repository retention policy recorded on each version. This increment
/// records the policy and retains all content regardless; collection is a
/// later checkpoint that needs the ledger's reference contract.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ArtifactRetention {
    #[default]
    #[serde(rename = "keep")]
    Keep,
    #[serde(rename = "30-days")]
    ThirtyDays,
    #[serde(rename = "discard-on-close")]
    DiscardOnClose,
}

impl ArtifactRetention {
    pub(crate) const ALL: [&'static str; 3] = ["keep", "30-days", "discard-on-close"];

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "keep" => Some(Self::Keep),
            "30-days" => Some(Self::ThirtyDays),
            "discard-on-close" => Some(Self::DiscardOnClose),
            _ => None,
        }
    }
}

/// Something a result may name. Only `Stored` addresses the artifact
/// repository; a working-repository commit sha and a pull request are
/// references to content that lives elsewhere and are never looked up as
/// artifact trees.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum ArtifactReference {
    #[serde(rename_all = "camelCase")]
    Stored {
        repo_id: String,
        artifact_id: String,
        kind: ArtifactContentKind,
    },
    #[serde(rename_all = "camelCase")]
    Commit { repo_id: String, sha: String },
    #[serde(rename_all = "camelCase")]
    Pr { url: String, head_sha: String },
}

/// Where the publishing bytes came from. Declared by the server from the
/// request's task, not by the caller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactProducer {
    pub(crate) task_id: String,
}

/// Retention bookkeeping. The public identity is the tree id; the commit and
/// ref that keep it reachable are an implementation detail a later retention
/// checkpoint deletes, which is why they are kept apart from the id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactStorage {
    pub(crate) commit: String,
    #[serde(rename = "ref")]
    pub(crate) ref_name: String,
}

/// One publication of a tree. Publishing identical bytes again yields the
/// same `artifactId` and a second, separate version record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactVersion {
    pub(crate) schema_version: u32,
    pub(crate) record_id: String,
    pub(crate) repo_id: String,
    pub(crate) artifact_id: String,
    pub(crate) kind: ArtifactContentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) entrypoint: Option<String>,
    pub(crate) created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) previous: Option<String>,
    pub(crate) retention: ArtifactRetention,
    pub(crate) produced_by: ArtifactProducer,
    pub(crate) file_count: u64,
    pub(crate) total_bytes: u64,
    pub(crate) storage: ArtifactStorage,
}

/// Where in an artifact a comment points. Position is caller-defined text
/// (a line, a selector, a coordinate); the excerpt lets a reader find the
/// spot again if the position no longer means anything to them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactAnchor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) position: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) excerpt: Option<String>,
}

/// A remark about one exact tree. `author` is declared text: it is not a
/// verified channel identity and authorizes nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactComment {
    pub(crate) schema_version: u32,
    pub(crate) record_id: String,
    pub(crate) repo_id: String,
    pub(crate) about_artifact_id: String,
    pub(crate) created_at: String,
    pub(crate) author: String,
    pub(crate) body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) anchor: Option<ArtifactAnchor>,
}

/// A decision about one exact tree: who decided what. `who` is declared
/// text; a decision record never moves a task between stages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactDecision {
    pub(crate) schema_version: u32,
    pub(crate) record_id: String,
    pub(crate) repo_id: String,
    pub(crate) about_artifact_id: String,
    pub(crate) created_at: String,
    pub(crate) who: String,
    pub(crate) what: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactFileEntry {
    pub(crate) path: String,
    pub(crate) size: u64,
}

/// Everything known about one tree id in one repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactDetail {
    pub(crate) repo_id: String,
    pub(crate) artifact_id: String,
    /// Whether the content tree is still present. False only for a tree
    /// that has version records but whose content is gone.
    pub(crate) retained: bool,
    pub(crate) reference: ArtifactReference,
    pub(crate) files: Vec<ArtifactFileEntry>,
    pub(crate) versions: Vec<ArtifactVersion>,
    pub(crate) comments: Vec<ArtifactComment>,
    pub(crate) decisions: Vec<ArtifactDecision>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishedArtifact {
    pub(crate) artifact_id: String,
    pub(crate) reference: ArtifactReference,
    pub(crate) version: ArtifactVersion,
    /// False when this exact tree was already retained and only a new
    /// version record was written.
    pub(crate) content_created: bool,
}
