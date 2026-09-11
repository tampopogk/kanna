import { nextTick, ref } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  setDesktopServerClientHandlersForTests,
  type DesktopAnalyticsRange,
  type DesktopRepoAnalytics,
} from "../services/desktopServerClient";
import { rangeForPreset, useAnalytics } from "./useAnalytics";

async function flushWatchers(): Promise<void> {
  await nextTick();
  await Promise.resolve();
  await Promise.resolve();
  await nextTick();
}

function analyticsFixture(
  range: DesktopAnalyticsRange,
  overrides: Partial<DesktopRepoAnalytics> = {},
): DesktopRepoAnalytics {
  return {
    range,
    coverage: {
      idleSince: "2026-08-01 00:00:00",
      revisionsSince: "2026-08-01 00:00:00",
      tokensSince: "2026-08-01 00:00:00",
      pullRequestStateConfirmed: true,
      providersWithoutTokenUsage: [],
      runsWithTokenUsage: 8,
      runsInRange: 10,
    },
    tasks: { created: 12, closed: 9, openNow: 4, childTasksCreated: 3 },
    pullRequests: { created: 7, merged: 5, openNow: 2 },
    idle: {
      totalSeconds: 7_200,
      workingSeconds: 3_600,
      taskCount: 6,
      averageSecondsPerTask: 1_200,
      longestSeconds: 5_400,
      contributors: [{ taskId: "task-a", title: "Task A", value: 5_400 }],
    },
    revisions: {
      cohortTasks: 4,
      totalRevisions: 3,
      averagePerTask: 0.75,
      cleanPassRate: 0.5,
      parkedRequests: 1,
      contributors: [{ taskId: "task-b", title: "Task B", value: 2 }],
    },
    tokens: {
      total: {
        input: 100,
        cachedInput: 900,
        cacheCreation: 50,
        reasoning: 20,
        output: 200,
        total: 1_250,
      },
      byModel: [
        {
          key: "claude-opus-5",
          label: "claude-opus-5",
          totals: {
            input: 100,
            cachedInput: 900,
            cacheCreation: 50,
            reasoning: 20,
            output: 200,
            total: 1_250,
          },
        },
      ],
      byTask: [
        {
          key: "task-a",
          label: "Task A",
          totals: {
            input: 100,
            cachedInput: 900,
            cacheCreation: 50,
            reasoning: 20,
            output: 200,
            total: 1_250,
          },
        },
      ],
    },
    ...overrides,
  };
}

describe("useAnalytics", () => {
  beforeEach(() => {
    setDesktopServerClientHandlersForTests({});
  });

  it("asks the desktop server for the selected window", async () => {
    const fetchRepoAnalytics = vi.fn(async (_repoId: string, range?: DesktopAnalyticsRange) =>
      analyticsFixture(range ?? { from: "2026-09-01", to: "2026-09-30" }),
    );
    setDesktopServerClientHandlersForTests({ fetchRepoAnalytics });

    const analytics = useAnalytics(ref<string | null>("repo-1"));
    await flushWatchers();

    expect(fetchRepoAnalytics).toHaveBeenCalledWith("repo-1", rangeForPreset("30d"));
    expect(analytics.analytics.value.tasks.created).toBe(12);
    expect(analytics.analytics.value.pullRequests.merged).toBe(5);
    expect(analytics.hasAnyData.value).toBe(true);
  });

  it("refetches when the window changes", async () => {
    const fetchRepoAnalytics = vi.fn(async (_repoId: string, range?: DesktopAnalyticsRange) =>
      analyticsFixture(range ?? { from: "2026-09-01", to: "2026-09-30" }),
    );
    setDesktopServerClientHandlersForTests({ fetchRepoAnalytics });

    const analytics = useAnalytics(ref<string | null>("repo-1"));
    await flushWatchers();
    analytics.selectPreset("7d");
    await flushWatchers();

    expect(fetchRepoAnalytics).toHaveBeenLastCalledWith("repo-1", rangeForPreset("7d"));
  });

  it("carries the window on screen into the custom inputs rather than resetting it", async () => {
    setDesktopServerClientHandlersForTests({
      fetchRepoAnalytics: async (_repoId, range) =>
        analyticsFixture(range ?? { from: "2026-09-01", to: "2026-09-30" }),
    });

    const analytics = useAnalytics(ref<string | null>("repo-1"));
    await flushWatchers();
    analytics.selectPreset("7d");
    await flushWatchers();
    const shown = { ...analytics.range.value };
    analytics.selectPreset("custom");
    await flushWatchers();

    expect(analytics.customRange.value).toEqual(shown);
  });

  it("flags a window that reaches back before a statistic was being recorded", async () => {
    setDesktopServerClientHandlersForTests({
      fetchRepoAnalytics: async (_repoId, range) =>
        analyticsFixture(range ?? { from: "2026-09-01", to: "2026-09-30" }, {
          coverage: {
            idleSince: "2999-01-01 00:00:00",
            revisionsSince: "1970-01-01 00:00:00",
            tokensSince: null,
            pullRequestStateConfirmed: true,
            providersWithoutTokenUsage: [],
            runsWithTokenUsage: 0,
            runsInRange: 0,
          },
        }),
    });

    const analytics = useAnalytics(ref<string | null>("repo-1"));
    await flushWatchers();

    expect(analytics.coverageGaps.value.idle).toBe(true);
    expect(analytics.coverageGaps.value.revisions).toBe(false);
    // An unknown start is not a gap to advertise; it is simply unknown.
    expect(analytics.coverageGaps.value.tokens).toBe(false);
    expect(analytics.tokenCoverageRatio.value).toBeNull();
  });

  it("reports a failed load instead of presenting stale or invented numbers", async () => {
    setDesktopServerClientHandlersForTests({
      fetchRepoAnalytics: async () => {
        throw new Error("server down");
      },
    });
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});

    const analytics = useAnalytics(ref<string | null>("repo-1"));
    await flushWatchers();

    expect(analytics.error.value).toBe("server down");
    expect(analytics.analytics.value.tasks.created).toBe(0);
    expect(analytics.hasAnyData.value).toBe(false);
    consoleError.mockRestore();
  });

  it("opens each statistic into the rows that produced it", async () => {
    setDesktopServerClientHandlersForTests({
      fetchRepoAnalytics: async (_repoId, range) =>
        analyticsFixture(range ?? { from: "2026-09-01", to: "2026-09-30" }),
    });

    const analytics = useAnalytics(ref<string | null>("repo-1"));
    await flushWatchers();

    expect(analytics.contributionsFor("idle")).toEqual([
      { taskId: "task-a", title: "Task A", value: 5_400 },
    ]);
    expect(analytics.contributionsFor("revisions")).toEqual([
      { taskId: "task-b", title: "Task B", value: 2 },
    ]);
    expect(analytics.contributionsFor("tokensByTask")).toEqual([
      { taskId: "task-a", title: "Task A", value: 1_250 },
    ]);
    // A model is not a task, so its row carries no navigation target.
    expect(analytics.contributionsFor("tokensByModel")[0]?.taskId).toBe("");
  });

  it("clears everything when no repository is selected", async () => {
    const fetchRepoAnalytics = vi.fn(async (_repoId: string, range?: DesktopAnalyticsRange) =>
      analyticsFixture(range ?? { from: "2026-09-01", to: "2026-09-30" }),
    );
    setDesktopServerClientHandlersForTests({ fetchRepoAnalytics });

    const repoId = ref<string | null>("repo-1");
    const analytics = useAnalytics(repoId);
    await flushWatchers();
    repoId.value = null;
    await flushWatchers();

    expect(analytics.hasAnyData.value).toBe(false);
    expect(analytics.analytics.value.tokens.total.total).toBe(0);
  });
});

describe("rangeForPreset", () => {
  it("counts both ends of the window", () => {
    const range = rangeForPreset("7d");
    const days =
      (Date.parse(`${range.to}T00:00:00Z`) - Date.parse(`${range.from}T00:00:00Z`)) / 86_400_000;
    expect(days).toBe(6);
  });
});
