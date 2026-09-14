import { describe, expect, it } from "vitest";
import type { TaskSummary } from "../lib/api/types";
import {
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
      task("explicit", { attentionReason: "Choose approach", runtimeState: "idle" }),
      task("waiting", { runtimeState: "waiting", readState: "read" }),
      task("both", { attentionReason: "Review result", runtimeState: "waiting" }),
      task("unread", { activity: "unread", readState: "unread", runtimeState: "idle" }),
      task("busy", { activity: "working", runtimeState: "busy" }),
      task("output", { waitingPromptSnippet: "Recent output", runtimeState: "idle" }),
      task("closed", {
        attentionReason: "No longer actionable",
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
      attentionReason: "  Pick a release window  ",
      runtimeState: "waiting"
    }))).toBe("Pick a release window");
    expect(needsYouReason(task("waiting", { runtimeState: "waiting" }))).toBe(
      DETECTED_PROMPT_LABEL
    );
    expect(needsYouReason(task("idle", { runtimeState: "idle" }))).toBeNull();
  });
});
