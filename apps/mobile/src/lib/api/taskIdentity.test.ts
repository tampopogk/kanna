import { describe, expect, it } from "vitest";
import type { TaskSummary } from "./types";
import {
  buildCloudTaskId,
  displayTaskId,
  isTaskBlocked,
  isTaskBlockedWithoutSession,
  resolveBlockerTasks,
  sameTaskDesktop,
  taskHasLiveSession,
  taskLocalId
} from "./taskIdentity";

function task(overrides: Partial<TaskSummary> & { id: string }): TaskSummary {
  return {
    repoId: "repo-1",
    title: `Task ${overrides.id}`,
    stage: "in progress",
    ...overrides
  };
}

describe("taskLocalId", () => {
  it("prefers the owner-local id over the display id", () => {
    expect(taskLocalId(task({ id: "cloud-1", ownerLocalTaskId: "local-1" }))).toBe(
      "local-1"
    );
    expect(taskLocalId(task({ id: "local-1" }))).toBe("local-1");
  });
});

describe("sameTaskDesktop", () => {
  it("treats an undefined owner as matching any desktop", () => {
    expect(
      sameTaskDesktop(task({ id: "a" }), task({ id: "b", ownerDesktopId: "d1" }))
    ).toBe(true);
    expect(
      sameTaskDesktop(
        task({ id: "a", ownerDesktopId: "d1" }),
        task({ id: "b", ownerDesktopId: "d2" })
      )
    ).toBe(false);
  });
});

describe("isTaskBlocked", () => {
  it("is blocked only while unresolved blocker ids remain", () => {
    expect(isTaskBlocked(task({ id: "a" }))).toBe(false);
    expect(isTaskBlocked(task({ id: "a", blockedByTaskIds: [] }))).toBe(false);
    expect(isTaskBlocked(task({ id: "a", blockedByTaskIds: ["b"] }))).toBe(true);
  });
});

describe("taskHasLiveSession", () => {
  it("is false before any session has reported anything", () => {
    expect(taskHasLiveSession(task({ id: "a" }))).toBe(false);
    expect(taskHasLiveSession(task({ id: "a", runtimeState: null }))).toBe(false);
  });

  it("is true once a session has reported a runtime state (T11b)", () => {
    expect(taskHasLiveSession(task({ id: "a", runtimeState: "busy" }))).toBe(true);
    expect(taskHasLiveSession(task({ id: "a", runtimeState: "idle" }))).toBe(true);
  });
});

describe("isTaskBlockedWithoutSession (T11b)", () => {
  it("is false when there is no blocker at all", () => {
    expect(isTaskBlockedWithoutSession(task({ id: "a" }))).toBe(false);
  });

  it("is true for a task blocked before its first session — the case with nothing to attach", () => {
    expect(
      isTaskBlockedWithoutSession(task({ id: "a", blockedByTaskIds: ["b"] }))
    ).toBe(true);
  });

  it("is false for a task blocked at a later stage (T4) whose current stage already has a live session", () => {
    const laterStageWait = task({
      id: "a",
      blockedByTaskIds: ["b"],
      runtimeState: "waiting"
    });
    expect(isTaskBlocked(laterStageWait)).toBe(true);
    expect(isTaskBlockedWithoutSession(laterStageWait)).toBe(false);
  });

  it("is false for a subtask waiting on a T5 join whose own session is running", () => {
    const joinWait = task({
      id: "parent",
      blockedByTaskIds: ["child-1", "child-2"],
      runtimeState: "busy"
    });
    expect(isTaskBlockedWithoutSession(joinWait)).toBe(false);
  });
});

describe("resolveBlockerTasks", () => {
  it("resolves blockers by owner-local id across repos on the same desktop", () => {
    const blocked = task({
      id: "kd-task",
      repoId: "repo-kanna",
      ownerDesktopId: "d1",
      blockedByTaskIds: ["kanache-task", "missing-task"]
    });
    const blocker = task({
      id: "cloud-kanache-task",
      repoId: "repo-kanache",
      ownerDesktopId: "d1",
      ownerLocalTaskId: "kanache-task",
      title: "Rust-input-hash donor matching"
    });
    const otherDesktop = task({
      id: "missing-task",
      ownerDesktopId: "d2"
    });

    expect(resolveBlockerTasks(blocked, [blocked, blocker, otherDesktop])).toEqual([
      { blockerTaskId: "kanache-task", task: blocker },
      { blockerTaskId: "missing-task", task: null }
    ]);
  });

  it("returns empty for a task with no blockers", () => {
    expect(resolveBlockerTasks(task({ id: "a" }), [])).toEqual([]);
  });
});
describe("displayTaskId", () => {
  it("prefers the desktop-local task id for cloud-sourced tasks", () => {
    const cloudId = buildCloudTaskId({
      ownerDesktopId: "desktop-1",
      localRepoId: "repo-1",
      ownerLocalTaskId: "task-local"
    });

    expect(displayTaskId({ id: cloudId, ownerLocalTaskId: "task-local" })).toBe(
      "task-local"
    );
  });

  it("falls back to the canonical id when no local id is present", () => {
    expect(displayTaskId({ id: "task-lan" })).toBe("task-lan");
    expect(displayTaskId({ id: "task-lan", ownerLocalTaskId: "  " })).toBe(
      "task-lan"
    );
  });
});
