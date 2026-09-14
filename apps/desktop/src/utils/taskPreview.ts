import type { DesktopTaskDetail } from "../services/desktopServerClient";
import { isRemotePresentationTaskId } from "./remoteTaskIdentity";

/** Local-only: never turn an owning peer's claimed port into this machine's URL. */
export function localTaskPreviewUrl(
  detail: DesktopTaskDetail,
  expected: { taskId: string; workspace: string; portName: string },
): string {
  if (isRemotePresentationTaskId(expected.taskId) || detail.id !== expected.taskId) {
    throw new Error("Preview must be opened on the task’s owning desktop.");
  }
  if (detail.closedAt != null || !expected.workspace || detail.worktreePath !== expected.workspace) {
    throw new Error("The task workspace changed or is unavailable. Reopen Preview from the current task.");
  }
  const port = detail.ports?.find(candidate => candidate.name === expected.portName)?.port;
  if (!port || !Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error("This task no longer holds the selected preview port.");
  }
  return `http://localhost:${port}`;
}
