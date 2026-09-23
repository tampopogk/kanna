/**
 * Remotes this desktop has already pushed artifacts to, per repository and
 * config source. The first push to a remote waits for the reader to see where
 * it goes and which config file chose it: a committed `.kanna/config.json`
 * can name any remote, so a cloned repository must not send artifacts
 * somewhere nobody on this machine looked at.
 *
 * Kept in this window's local storage: a convenience that only ever adds a
 * confirmation step when it is missing (a private window, cleared storage).
 */
const STORAGE_KEY = "kanna.artifactRemotesConfirmed";

function key(repoId: string, remote: string, source: string): string {
  return JSON.stringify([repoId, remote, source]);
}

function read(): Set<string> {
  try {
    const parsed: unknown = JSON.parse(globalThis.localStorage?.getItem(STORAGE_KEY) ?? "[]");
    return new Set(Array.isArray(parsed) ? parsed.filter((entry): entry is string => typeof entry === "string") : []);
  } catch {
    return new Set();
  }
}

export function isArtifactRemoteConfirmed(repoId: string, remote: string, source: string): boolean {
  return read().has(key(repoId, remote, source));
}

export function rememberArtifactRemoteConfirmed(repoId: string, remote: string, source: string): void {
  const confirmed = read();
  confirmed.add(key(repoId, remote, source));
  try {
    globalThis.localStorage?.setItem(STORAGE_KEY, JSON.stringify([...confirmed]));
  } catch {
    // Without storage the next push simply asks again.
  }
}
