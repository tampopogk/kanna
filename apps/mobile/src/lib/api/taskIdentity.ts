import type { TaskSummary } from "./types";

/**
 * The owner-local durable task id that `parentTaskId` and `blockedByTaskIds`
 * reference. Cloud-merged tasks display under a cloud-canonical id while
 * `ownerLocalTaskId` keeps the desktop-local id; direct LAN tasks use the
 * local id as their display id.
 */
export function taskLocalId(task: TaskSummary): string {
  return task.ownerLocalTaskId ?? task.id;
}

/** Local task ids are only unique per desktop; undefined owners match any. */
export function sameTaskDesktop(left: TaskSummary, right: TaskSummary): boolean {
  return (
    left.ownerDesktopId === undefined ||
    right.ownerDesktopId === undefined ||
    left.ownerDesktopId === right.ownerDesktopId
  );
}

/**
 * A blocker reference paired with its visible task summary when one is in
 * the current collections. Blockers can be cross-repo (a task may wait on
 * work in another repository), so resolution only matches owner-local id
 * within the same desktop — never repo.
 */
export interface BlockerTaskRef {
  blockerTaskId: string;
  task: TaskSummary | null;
}

export function isTaskBlocked(task: TaskSummary): boolean {
  return (task.blockedByTaskIds?.length ?? 0) > 0;
}

/**
 * True once a session has reported anything about this task's agent.
 * `runtimeState` starts absent and is only ever set once a run has started
 * (spec §16.8, T11b) — the server-provided signal to use here, not
 * `blockedByTaskIds` alone. A task blocked by a T4 stage-dependency edge
 * into a *later* stage, or a T5 subtask-join wait, can already be running
 * its current stage; only a task blocked before its first session has none.
 */
export function taskHasLiveSession(task: TaskSummary): boolean {
  return task.runtimeState != null;
}

/**
 * Blocked with nothing running yet — the case that actually has no agent
 * session to attach. `isTaskBlocked` alone conflates this with a task
 * blocked at a later stage whose current stage is already live.
 */
export function isTaskBlockedWithoutSession(task: TaskSummary): boolean {
  return isTaskBlocked(task) && !taskHasLiveSession(task);
}

export function resolveBlockerTasks(
  task: TaskSummary,
  tasks: readonly TaskSummary[]
): BlockerTaskRef[] {
  return (task.blockedByTaskIds ?? []).map((blockerTaskId) => ({
    blockerTaskId,
    task:
      tasks.find(
        (candidate) =>
          taskLocalId(candidate) === blockerTaskId &&
          sameTaskDesktop(candidate, task)
      ) ?? null
  }));
}

export function buildCloudTaskId({
  ownerDesktopId,
  localRepoId,
  ownerLocalTaskId
}: {
  ownerDesktopId: string;
  localRepoId: string;
  ownerLocalTaskId: string;
}): string {
  return `cloud:${ownerDesktopId}:${localRepoId}:${ownerLocalTaskId}`;
}

// Cloud-sourced tasks carry a synthetic canonical id ("cloud:<desktop>:<repo>:<task>").
// User-facing surfaces must show the desktop-local task id, matching the desktop app.
export function displayTaskId(task: {
  id: string;
  ownerLocalTaskId?: string;
}): string {
  return task.ownerLocalTaskId?.trim() || task.id;
}

export function canonicalizeTaskActionId({
  canonicalTaskId,
  ownerDesktopId,
  localRepoId,
  sourceLocalTaskId,
  responseLocalTaskId
}: {
  canonicalTaskId: string;
  ownerDesktopId: string;
  localRepoId: string;
  sourceLocalTaskId: string;
  responseLocalTaskId: string;
}): string {
  if (responseLocalTaskId === sourceLocalTaskId) {
    return canonicalTaskId;
  }

  return buildCloudTaskId({
    ownerDesktopId,
    localRepoId,
    ownerLocalTaskId: responseLocalTaskId
  });
}
