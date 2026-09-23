export interface TaskSessionIdentity {
  id: string;
  branch?: string | null;
  /**
   * The `stage_workspace` identity T2 records for the task's current stage
   * run. A daemon that reports this as its session id names the exact
   * workspace it opened rather than the branch name, which changes at every
   * stage transition and can go stale mid-run. Absent on a peer that
   * predates T2's session identity, or before any run has started — the
   * resolver falls back to `branch` for those.
   */
  workspace_id?: string | null;
}

export function resolveTaskItemForDaemonSession<T extends TaskSessionIdentity>(
  items: readonly T[],
  sessionId: string,
): T | null {
  return items.find((candidate) => candidate.id === sessionId)
    ?? items.find((candidate) => candidate.workspace_id === sessionId)
    ?? items.find((candidate) => candidate.branch === sessionId)
    ?? null;
}
