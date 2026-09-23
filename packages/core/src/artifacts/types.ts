// Artifact descriptor wire types (spec §8). Mirrors
// crates/kanna-server/src/artifacts/types.rs; T6 owns both and they change
// together.
//
// Identity is the Git tree id of the artifact's content in the repository's
// artifact store. Version, comment and decision records are metadata *about*
// one exact tree id and are never part of the content, so recording one never
// changes an id.

/** Version of every persisted artifact record. */
export const ARTIFACT_RECORD_SCHEMA_VERSION = 1;

/** A full, lowercase, 40-hex Git tree id. */
export type ArtifactId = string;

export const ARTIFACT_CONTENT_KINDS = ["document", "mockup", "media", "report"] as const;
export type ArtifactContentKind = (typeof ARTIFACT_CONTENT_KINDS)[number];

export const ARTIFACT_RETENTION_POLICIES = ["keep", "30-days", "discard-on-close"] as const;
/** Recorded on each version; enforcement is a later checkpoint. */
export type ArtifactRetention = (typeof ARTIFACT_RETENTION_POLICIES)[number];

/**
 * Something a result may name. Only `stored` addresses the artifact store; a
 * working-repository commit and a pull request point at content that lives
 * elsewhere and are never resolved as artifact trees.
 */
export type ArtifactReference =
  | { type: "stored"; repoId: string; artifactId: ArtifactId; kind: ArtifactContentKind }
  | { type: "commit"; repoId: string; sha: string }
  | { type: "pr"; url: string; headSha: string };

/** Retention bookkeeping: the commit and ref that keep a tree reachable. Not the id. */
export interface ArtifactStorage {
  commit: string;
  ref: string;
}

/** One publication of a tree. Identical bytes published twice: one id, two versions. */
export interface ArtifactVersion {
  schemaVersion: number;
  recordId: string;
  repoId: string;
  artifactId: ArtifactId;
  kind: ArtifactContentKind;
  entrypoint?: string;
  createdAt: string;
  previous?: ArtifactId;
  retention: ArtifactRetention;
  producedBy: { taskId: string };
  fileCount: number;
  totalBytes: number;
  storage: ArtifactStorage;
}

export interface ArtifactAnchor {
  path?: string;
  position?: string;
  excerpt?: string;
}

/** `author` is declared text, not a verified identity. */
export interface ArtifactComment {
  schemaVersion: number;
  recordId: string;
  repoId: string;
  aboutArtifactId: ArtifactId;
  createdAt: string;
  author: string;
  body: string;
  anchor?: ArtifactAnchor;
}

/** `who` is declared text; a decision never moves a task. */
export interface ArtifactDecision {
  schemaVersion: number;
  recordId: string;
  repoId: string;
  aboutArtifactId: ArtifactId;
  createdAt: string;
  who: string;
  what: string;
}

export interface ArtifactFileEntry {
  path: string;
  size: number;
}

export interface ArtifactDetail {
  repoId: string;
  artifactId: ArtifactId;
  /** False when versions exist but the content is no longer retained. */
  retained: boolean;
  reference: Extract<ArtifactReference, { type: "stored" }>;
  files: ArtifactFileEntry[];
  versions: ArtifactVersion[];
  comments: ArtifactComment[];
  decisions: ArtifactDecision[];
}

export interface PublishedArtifact {
  artifactId: ArtifactId;
  reference: Extract<ArtifactReference, { type: "stored" }>;
  version: ArtifactVersion;
  /** False when the tree was already stored and only a version was recorded. */
  contentCreated: boolean;
}

export interface OpenedArtifactPreview {
  repoId: string;
  artifactId: ArtifactId;
  entrypoint: string;
  url: string;
  expiresAt: number;
  idleTimeoutSecs: number;
}
