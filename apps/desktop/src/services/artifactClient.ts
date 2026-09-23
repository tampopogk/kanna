import type {
  ArtifactComment,
  ArtifactFileContent,
  ArtifactDecision,
  ArtifactDetail,
  ArtifactFetchOutcome,
  ArtifactPushOutcome,
  ArtifactRemoteInfo,
  OpenedArtifactPreview,
} from "@kanna/core";
import { DesktopServerRequestError, requestDesktopServerJson } from "./desktopServerClient";

/**
 * The desktop's client for one repository's artifact store (spec §8).
 *
 * Everything is addressed by repository and exact tree id. Nothing here names
 * a task or a dev-server port: an artifact outlives the task that produced it,
 * and a preview listener is minted per open rather than remembered.
 *
 * Recording a comment or a decision writes a record about a tree id and
 * nothing else. There is deliberately no gate, stage or task operation on this
 * client; whoever owns the gate reads the decision and acts on it.
 */

function artifactPath(repoId: string, artifactId: string, suffix = ""): string {
  return `/v1/repos/${encodeURIComponent(repoId)}/artifacts/${encodeURIComponent(artifactId)}${suffix}`;
}

/** Why an artifact cannot be shown, as the server classified it. */
export type ArtifactUnavailableReason =
  | "not-found"
  | "invalid-id"
  | "content-missing"
  | "no-entrypoint"
  | "repo-not-found";

export class ArtifactUnavailableError extends Error {
  constructor(readonly reason: ArtifactUnavailableReason, message: string) {
    super(message);
    this.name = "ArtifactUnavailableError";
  }
}

const REASON_BY_CODE: Record<string, ArtifactUnavailableReason> = {
  artifact_not_found: "not-found",
  invalid_artifact_id: "invalid-id",
  artifact_content_missing: "content-missing",
  no_entrypoint: "no-entrypoint",
  repo_not_found: "repo-not-found",
};

function classify(error: unknown): never {
  if (error instanceof DesktopServerRequestError) {
    const body = error.parsedBody();
    const code = typeof body?.error === "string" ? body.error : "";
    const reason = REASON_BY_CODE[code];
    if (reason) {
      const message = typeof body?.message === "string" ? body.message : error.message;
      throw new ArtifactUnavailableError(reason, message);
    }
  }
  throw error;
}

export async function fetchArtifact(repoId: string, artifactId: string): Promise<ArtifactDetail> {
  return requestDesktopServerJson<ArtifactDetail>(artifactPath(repoId, artifactId)).catch(classify);
}

export async function readArtifactFile(
  repoId: string,
  artifactId: string,
  path: string,
  signal?: AbortSignal,
): Promise<ArtifactFileContent> {
  return requestDesktopServerJson<ArtifactFileContent>(
    artifactPath(repoId, artifactId, `/files?path=${encodeURIComponent(path)}`),
    { signal },
  ).catch(classify);
}

export async function openArtifactPreview(repoId: string, artifactId: string): Promise<OpenedArtifactPreview> {
  return requestDesktopServerJson<OpenedArtifactPreview>(
    artifactPath(repoId, artifactId, "/preview"),
    { method: "POST" },
  ).catch(classify);
}

export async function closeArtifactPreview(repoId: string, artifactId: string): Promise<void> {
  await requestDesktopServerJson<unknown>(artifactPath(repoId, artifactId, "/preview/close"), { method: "POST" });
}

export interface ArtifactCommentInput {
  author: string;
  body: string;
  anchor?: { path?: string; position?: string; excerpt?: string };
}

export async function recordArtifactComment(
  repoId: string,
  artifactId: string,
  input: ArtifactCommentInput,
): Promise<ArtifactComment> {
  return requestDesktopServerJson<ArtifactComment>(artifactPath(repoId, artifactId, "/comments"), {
    method: "POST",
    body: input,
  });
}

export async function recordArtifactDecision(
  repoId: string,
  artifactId: string,
  input: { who: string; what: string },
): Promise<ArtifactDecision> {
  return requestDesktopServerJson<ArtifactDecision>(artifactPath(repoId, artifactId, "/decisions"), {
    method: "POST",
    body: input,
  });
}

/**
 * A push or fetch the server refused. `refs` names the remote refs that
 * already hold different objects (a conflict); `remote` is redacted.
 */
export class ArtifactRemoteError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly remote: string | null,
    readonly refs: string[],
  ) {
    super(message);
    this.name = "ArtifactRemoteError";
  }
}

function classifyRemote(error: unknown): never {
  if (error instanceof DesktopServerRequestError) {
    const body = error.parsedBody();
    const code = typeof body?.error === "string" ? body.error : "";
    if (code.startsWith("artifact_remote") || code === "artifact_not_on_remote") {
      throw new ArtifactRemoteError(
        code,
        typeof body?.message === "string" ? body.message : error.message,
        typeof body?.remote === "string" ? body.remote : null,
        Array.isArray(body?.refs) ? body.refs.filter((ref: unknown): ref is string => typeof ref === "string") : [],
      );
    }
  }
  return classify(error);
}

/** Where a push of this repository's artifacts would go, and which config file chose it. */
export async function fetchArtifactRemoteInfo(repoId: string): Promise<ArtifactRemoteInfo> {
  return requestDesktopServerJson<ArtifactRemoteInfo>(`/v1/repos/${encodeURIComponent(repoId)}/artifact-remote`);
}

/** Push one artifact, its earlier versions and all their records to the configured remote. */
export async function pushArtifact(repoId: string, artifactId: string): Promise<ArtifactPushOutcome> {
  return requestDesktopServerJson<ArtifactPushOutcome>(artifactPath(repoId, artifactId, "/push"), { method: "POST" })
    .catch(classifyRemote);
}

/** Fetch one artifact by hash from the configured remote. Received decisions change no task. */
export async function fetchArtifactFromRemote(repoId: string, artifactId: string): Promise<ArtifactFetchOutcome> {
  return requestDesktopServerJson<ArtifactFetchOutcome>(artifactPath(repoId, artifactId, "/fetch"), { method: "POST" })
    .catch(classifyRemote);
}
