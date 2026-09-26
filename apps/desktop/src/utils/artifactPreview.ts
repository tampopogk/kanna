import type { OpenedArtifactPreview } from "@kanna/core";
import { closeArtifactPreview, openArtifactPreview } from "../services/artifactClient";

/**
 * The server keeps one preview listener per repository and tree id. Two
 * viewers of the same id share it, so closing it is counted: the listener
 * stops when the last viewer lets go, not when the first one does.
 */
const holders = new Map<string, number>();

function key(repoId: string, artifactId: string): string {
  return `${repoId}\u0000${artifactId}`;
}

export async function acquireArtifactPreview(repoId: string, artifactId: string): Promise<OpenedArtifactPreview> {
  const id = key(repoId, artifactId);
  holders.set(id, (holders.get(id) ?? 0) + 1);
  try {
    const opened = await openArtifactPreview(repoId, artifactId);
    assertArtifactPreviewUrl(opened.url);
    return opened;
  } catch (error) {
    await releaseArtifactPreview(repoId, artifactId);
    throw error;
  }
}

export async function releaseArtifactPreview(repoId: string, artifactId: string): Promise<void> {
  const id = key(repoId, artifactId);
  const remaining = (holders.get(id) ?? 0) - 1;
  if (remaining > 0) {
    holders.set(id, remaining);
    return;
  }
  holders.delete(id);
  // The listener also expires on idle; a failed close costs nothing.
  await closeArtifactPreview(repoId, artifactId).catch(() => undefined);
}

export function resetArtifactPreviewHoldersForTests(): void {
  holders.clear();
}

const PREVIEW_PATH = /^\/a\/[0-9a-f]{32}\//;

/**
 * Refuse any address that is not the artifact store's own loopback listener:
 * the frame must never be pointed at the control API, a dev server, or a
 * remote origin, whatever a response claims.
 */
export function assertArtifactPreviewUrl(url: string): URL {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    throw new Error("The artifact preview address is not a URL.");
  }
  if (
    parsed.protocol !== "http:"
    || parsed.hostname !== "127.0.0.1"
    || !parsed.port
    || !PREVIEW_PATH.test(parsed.pathname)
    || parsed.username
    || parsed.password
    || parsed.search
    || parsed.hash
  ) {
    throw new Error("The artifact preview address is not this machine's artifact listener.");
  }
  return parsed;
}

function encodePath(path: string): string {
  return path.split("/").map(encodeURIComponent).join("/");
}

/**
 * Asks the listener for its sandboxing shell rather than the content: the
 * shell frames the content, and its `frame-src` keeps every navigation of
 * that frame on the listener. This webview sets no `frame-src` of its own,
 * and an iframe load never gets the shell by fetch metadata alone.
 */
export const ARTIFACT_SHELL_QUERY = "kanna-shell";

/**
 * The frame address for one file of the open tree: the entrypoint by default,
 * or the file a comment is anchored to. Resolved under the capability segment,
 * exactly as the page's own relative references are, and always the shell
 * that frames that file.
 */
export function artifactFrameUrl(previewUrl: string, path: string | null): string {
  const parsed = assertArtifactPreviewUrl(previewUrl);
  const capability = parsed.pathname.match(PREVIEW_PATH)?.[0] ?? "/";
  const frame = path ? new URL(`${capability}${encodePath(path)}`, parsed.origin) : parsed;
  frame.search = `?${ARTIFACT_SHELL_QUERY}`;
  return frame.toString();
}
