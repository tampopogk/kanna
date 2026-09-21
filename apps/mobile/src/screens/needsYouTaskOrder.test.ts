import { describe, expect, it } from "vitest";
import type { TaskSummary } from "../lib/api/types";
import {
  ATTENTION_REQUESTED_LABEL,
  DETECTED_PROMPT_LABEL,
  needsYouCount,
  needsYouReason,
  visibleNeedsYouTasks
} from "./needsYouTaskOrder";

function task(id: string, overrides: Partial<TaskSummary> = {}): TaskSummary {
  return {
    id,
    repoId: "repo-1",
    title: id,
    stage: "in progress",
    ...overrides
  };
}

describe("visibleNeedsYouTasks", () => {
  it("includes each open explicit request or detected wait once", () => {
    const tasks = [
      task("explicit", { attentionRequested: true, runtimeState: "idle" }),
      task("waiting", { runtimeState: "waiting", readState: "read" }),
      task("both", { attentionRequested: true, runtimeState: "waiting" }),
      task("unread", { activity: "unread", readState: "unread", runtimeState: "idle" }),
      task("busy", { activity: "working", runtimeState: "busy" }),
      task("output", { waitingPromptSnippet: "Recent output", runtimeState: "idle" }),
      task("closed", {
        attentionRequested: true,
        runtimeState: "waiting",
        closedAt: "2026-09-14T12:00:00.000Z"
      })
    ];

    expect(visibleNeedsYouTasks(tasks).map(({ id }) => id)).toEqual([
      "explicit",
      "waiting",
      "both"
    ]);
    expect(needsYouCount(tasks)).toBe(3);
  });

  it("prefers the recorded request and labels detected waiting truthfully", () => {
    expect(needsYouReason(task("explicit", {
      attentionRequested: true,
      runtimeState: "waiting"
    }))).toBe(ATTENTION_REQUESTED_LABEL);
    expect(needsYouReason(task("waiting", { runtimeState: "waiting" }))).toBe(
      DETECTED_PROMPT_LABEL
    );
    expect(needsYouReason(task("idle", { runtimeState: "idle" }))).toBeNull();
  });
});
