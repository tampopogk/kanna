import type {
  ArtifactComment,
  ArtifactFileContent,
  ArtifactDecision,
  ArtifactDetail,
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

export async function readArtifactFile(repoId: string, artifactId: string, path: string): Promise<ArtifactFileContent> {
  return requestDesktopServerJson<ArtifactFileContent>(
    artifactPath(repoId, artifactId, `/files?path=${encodeURIComponent(path)}`),
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
