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
/**
 * Recorded on each version and enforced by the server's retention sweep:
 * `keep` never collects, `30-days` collects 30 days after the producing task
 * closed, `discard-on-close` once it closed. Content an open task still names
 * is never collected, and collection removes content only, never records.
 */
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

/** A result that named this tree (spec §7 `artifacts`). */
export interface ArtifactBinding {
  schemaVersion: number;
  recordId: string;
  repoId: string;
  aboutArtifactId: ArtifactId;
  createdAt: string;
  taskId: string;
  name: string;
  runId?: string;
}

/** Retention removed this tree's content; its records stay. */
export interface ArtifactExpiry {
  schemaVersion: number;
  recordId: string;
  repoId: string;
  aboutArtifactId: ArtifactId;
  createdAt: string;
  policies: ArtifactRetention[];
  storage: ArtifactStorage;
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
  /** Absent from servers that predate result binding. */
  bindings?: ArtifactBinding[];
  /** Produced, no longer retained: retention collected the content. */
  expired?: boolean;
  expirations?: ArtifactExpiry[];
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

/** A remote ref that was not imported, and why. */
export interface ArtifactRefusedRef {
  ref: string;
  reason: string;
}

/** Result of kanna_push_artifact. `remote` never carries URL credentials. */
export interface ArtifactPushOutcome {
  remote: string;
  artifactId: ArtifactId;
  /** The pushed id, then earlier versions reached through `previous`. */
  artifactIds: ArtifactId[];
  createdRefs: string[];
  upToDateRefs: number;
}

/** Result of kanna_fetch_artifact. A received decision changes no task. */
export interface ArtifactFetchOutcome {
  remote: string;
  artifactId: ArtifactId;
  fetched: ArtifactId[];
  contentRetained: ArtifactId[];
  recordsImported: number;
  refused: ArtifactRefusedRef[];
  /** Earlier versions whose content neither the remote nor this store holds. */
  missing: ArtifactId[];
  detail: ArtifactDetail;
}

/**
 * One file of a retained tree, read through
 * `GET /v1/repos/{repoId}/artifacts/{artifactId}/files?path=` by a client that
 * renders the artifact itself instead of opening the loopback preview.
 */
export interface ArtifactFileContent {
  repoId: string;
  artifactId: ArtifactId;
  path: string;
  mediaType: string;
  size: number;
  dataBase64: string;
}

/**
 * The artifact remote a repository's configuration resolves to on this
 * machine, from `GET /v1/repos/{repoId}/artifact-remote`. A client shows it
 * before a push, because the committed repo config can choose it: whoever
 * wrote `.kanna/config.json` decides where a push goes unless this machine's
 * `.kanna/config.local.json` says otherwise. `remote` never carries URL
 * credentials.
 */
export interface ArtifactRemoteInfo {
  repoId: string;
  configured: boolean;
  remote?: string;
  source?: "committed" | "machine-local";
  configFile?: string;
  /** The configured value is present but unusable. */
  error?: { code: string; message: string };
  /**
   * Opaque identity of this remote and source. Sent back with a push as
   * `remoteFingerprint`, it binds the push to this remote: the server refuses
   * with `artifact_remote_changed` if its configuration now names another.
   */
  fingerprint?: string;
}
