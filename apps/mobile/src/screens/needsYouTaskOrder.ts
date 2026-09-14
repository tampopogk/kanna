import type { TaskSummary } from "../lib/api/types";

export const DETECTED_PROMPT_LABEL = "Detected question / input prompt";

/**
 * A task needs the human only when one of the two positive signals says so.
 * Read state, display activity, recent output, and ordinary idle/busy runtime
 * are deliberately irrelevant.
 */
export function taskNeedsYou(task: TaskSummary): boolean {
  return task.closedAt == null &&
    (Boolean(task.attentionReason?.trim()) || task.runtimeState === "waiting");
}

export function needsYouReason(task: TaskSummary): string | null {
  const explicitReason = task.attentionReason?.trim();
  if (explicitReason) return explicitReason;
  return task.runtimeState === "waiting" ? DETECTED_PROMPT_LABEL : null;
}

export function visibleNeedsYouTasks(
  tasks: readonly TaskSummary[]
): TaskSummary[] {
  return tasks.filter(taskNeedsYou);
}

export function needsYouCount(tasks: readonly TaskSummary[]): number {
  return visibleNeedsYouTasks(tasks).length;
}
