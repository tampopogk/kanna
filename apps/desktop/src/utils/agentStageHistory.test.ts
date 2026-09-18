import { describe, expect, it } from "vitest";
import {
  parseStageHistorySelection,
  setupSelectionValue,
  stageHistoryItems,
} from "./agentStageHistory";
import type { AgentTerminalAttempt, WorkspaceSetupRun } from "../services/desktopServerClient";

const attempt = (
  id: string, stage: string, over: Partial<AgentTerminalAttempt> = {},
): AgentTerminalAttempt => ({
  id, stage, kind: "main", startedAt: id, cwd: "/work",
  live: false, archived: true, recordedLaunch: true, observedExitCode: 0, ...over,
});

const setupRun = (runId: string, over: Partial<WorkspaceSetupRun> = {}): WorkspaceSetupRun => ({
  runId, status: "succeeded", exitCode: 0, timedOut: false, truncated: false,
  commands: ["pnpm install"], output: "ok", durationMs: 12, finishedAt: `${runId}-finished`, ...over,
});

describe("stageHistoryItems", () => {
  it("numbers agent attempts without letting a teardown consume an ordinal", () => {
    const items = stageHistoryItems([
      attempt("run-1", "in progress"),
      attempt("td-1", "in progress", { kind: "teardown", startedAt: "td-1-started" }),
      attempt("run-2", "review"),
    ], []);
    expect(items.map(item => [item.kind, item.label])).toEqual([
      ["attempt", "review · attempt 2 · run-2"],
      ["teardown", "Teardown · in progress · td-1-started"],
      ["attempt", "in progress · attempt 1 · run-1"],
    ]);
  });

  it("labels a teardown by the stage whose workspace it tore down and marks a missing archive", () => {
    const [teardown] = stageHistoryItems(
      [attempt("td-1", "plan", { kind: "teardown", archived: false })], [],
    );
    expect(teardown).toMatchObject({
      value: "td-1", kind: "teardown", stage: "plan", title: "Teardown · plan",
      label: "Teardown · plan · td-1 · history unavailable",
    });
  });

  it("offers each run's setup stream beside it, including the live session's", () => {
    const items = stageHistoryItems(
      [attempt("run-1", "in progress"), attempt("run-2", "review", { live: true })],
      [setupRun("run-1"), setupRun("run-2")],
    );
    // Newest first, and a run's setup sits just below the session it prepared.
    expect(items.map(item => [item.value, item.label])).toEqual([
      ["setup:run-2", "Setup · review · run-2-finished"],
      ["run-1", "in progress · attempt 1 · run-1"],
      ["setup:run-1", "Setup · in progress · run-1-finished"],
    ]);
  });

  it("marks a failed setup and keeps a record whose run left no launch row", () => {
    const items = stageHistoryItems(
      [attempt("run-1", "in progress")],
      [setupRun("run-1", { status: "failed", exitCode: 1 }), setupRun("orphan")],
    );
    expect(items.map(item => item.label)).toEqual([
      "Setup · orphan-finished",
      "in progress · attempt 1 · run-1",
      "Setup · in progress · run-1-finished · failed",
    ]);
    expect(items[0]).toMatchObject({ title: "Setup", stage: "Setup" });
  });

  it("leaves an all-live list with no history but its setup", () => {
    expect(stageHistoryItems([attempt("run-1", "build", { live: true })], [])).toEqual([]);
  });
});

describe("parseStageHistorySelection", () => {
  it("reads the live session, an archive run and a setup run apart", () => {
    expect(parseStageHistorySelection("")).toEqual({ kind: "latest" });
    // A teardown is an ordinary terminal archive addressed by its run id.
    expect(parseStageHistorySelection("td-1")).toEqual({ kind: "attempt", runId: "td-1" });
    expect(parseStageHistorySelection(setupSelectionValue("run-1")))
      .toEqual({ kind: "setup", runId: "run-1" });
  });
});
