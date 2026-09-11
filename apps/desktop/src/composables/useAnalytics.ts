import { computed, ref, watch, type Ref } from "vue";
import {
  fetchDesktopRepoAnalytics,
  type DesktopAnalyticsContribution,
  type DesktopAnalyticsRange,
  type DesktopRepoAnalytics,
} from "../services/desktopServerClient";

/** Selectable windows, plus the custom one the date inputs drive. */
export type AnalyticsRangePreset = "7d" | "30d" | "90d" | "custom";

export const ANALYTICS_RANGE_PRESET_DAYS: Record<Exclude<AnalyticsRangePreset, "custom">, number> = {
  "7d": 7,
  "30d": 30,
  "90d": 90,
};

/** A statistic a reader can open into the rows that produced it. */
export type AnalyticsDrilldown = "idle" | "revisions" | "tokensByTask" | "tokensByModel";

export function isoDate(date: Date): string {
  return date.toISOString().slice(0, 10);
}

export function rangeForPreset(preset: Exclude<AnalyticsRangePreset, "custom">): DesktopAnalyticsRange {
  const to = new Date();
  const from = new Date(to);
  // Inclusive of both ends: a "7 days" window is today plus the six before it,
  // not today plus seven.
  from.setUTCDate(from.getUTCDate() - (ANALYTICS_RANGE_PRESET_DAYS[preset] - 1));
  return { from: isoDate(from), to: isoDate(to) };
}

function emptyAnalytics(range: DesktopAnalyticsRange): DesktopRepoAnalytics {
  return {
    range,
    coverage: {
      idleSince: null,
      revisionsSince: null,
      tokensSince: null,
      pullRequestStateConfirmed: false,
      providersWithoutTokenUsage: [],
      runsWithTokenUsage: 0,
      runsInRange: 0,
    },
    tasks: { created: 0, closed: 0, openNow: 0, childTasksCreated: 0 },
    pullRequests: { created: 0, merged: null, openNow: null },
    idle: {
      totalSeconds: 0,
      workingSeconds: 0,
      taskCount: 0,
      averageSecondsPerTask: 0,
      longestSeconds: 0,
      contributors: [],
    },
    revisions: {
      cohortTasks: 0,
      totalRevisions: 0,
      averagePerTask: 0,
      cleanPassRate: null,
      parkedRequests: 0,
      contributors: [],
    },
    tokens: {
      total: { input: 0, cachedInput: 0, cacheCreation: 0, reasoning: 0, output: 0, total: 0 },
      byModel: [],
      byTask: [],
    },
  };
}

export function useAnalytics(repoId: Ref<string | null>) {
  const preset = ref<AnalyticsRangePreset>("30d");
  const customRange = ref<DesktopAnalyticsRange>(rangeForPreset("30d"));
  const loading = ref(false);
  const error = ref<string | null>(null);

  const range = computed<DesktopAnalyticsRange>(() =>
    preset.value === "custom" ? customRange.value : rangeForPreset(preset.value),
  );
  const analytics = ref<DesktopRepoAnalytics>(emptyAnalytics(range.value));
  let refreshGeneration = 0;

  /**
   * Whether the selected window reaches back before a statistic started being
   * recorded. The view says so rather than presenting the unrecorded part as
   * a stretch in which nothing happened.
   */
  const coverageGaps = computed(() => {
    const { coverage } = analytics.value;
    const from = range.value.from;
    const startsAfter = (since: string | null) => since != null && since.slice(0, 10) > from;
    return {
      idle: startsAfter(coverage.idleSince),
      revisions: startsAfter(coverage.revisionsSince),
      tokens: startsAfter(coverage.tokensSince),
    };
  });

  /** Share of the window's runs the token figures actually account for. */
  const tokenCoverageRatio = computed(() => {
    const { runsInRange, runsWithTokenUsage } = analytics.value.coverage;
    return runsInRange > 0 ? runsWithTokenUsage / runsInRange : null;
  });

  const hasAnyData = computed(() => {
    const { tasks, pullRequests, idle, revisions, tokens } = analytics.value;
    return (
      tasks.created > 0 ||
      tasks.closed > 0 ||
      tasks.openNow > 0 ||
      pullRequests.created === null ||
      pullRequests.created > 0 ||
      idle.totalSeconds > 0 ||
      revisions.cohortTasks > 0 ||
      tokens.total.total > 0
    );
  });

  function contributionsFor(drilldown: AnalyticsDrilldown): DesktopAnalyticsContribution[] {
    switch (drilldown) {
      case "idle":
        return analytics.value.idle.contributors;
      case "revisions":
        return analytics.value.revisions.contributors;
      case "tokensByTask":
        return analytics.value.tokens.byTask.map((group) => ({
          taskId: group.key,
          title: group.label,
          value: group.totals.total,
        }));
      case "tokensByModel":
        // Grouped by model, so there is no task to open — the key is the
        // model name and the row is not navigable.
        return analytics.value.tokens.byModel.map((group) => ({
          taskId: "",
          title: group.label,
          value: group.totals.total,
        }));
    }
  }

  async function refresh() {
    const generation = ++refreshGeneration;
    const selectedRepoId = repoId.value;
    const selectedRange = { ...range.value };
    if (!selectedRepoId) {
      analytics.value = emptyAnalytics(selectedRange);
      error.value = null;
      loading.value = false;
      return;
    }
    loading.value = true;
    error.value = null;
    try {
      const response = await fetchDesktopRepoAnalytics(selectedRepoId, selectedRange);
      if (generation === refreshGeneration) {
        analytics.value = response;
      }
    } catch (e) {
      if (generation === refreshGeneration) {
        error.value = e instanceof Error ? e.message : String(e);
        console.error("[analytics] refresh failed:", e);
        analytics.value = emptyAnalytics(selectedRange);
      }
    } finally {
      if (generation === refreshGeneration) {
        loading.value = false;
      }
    }
  }

  function selectPreset(next: AnalyticsRangePreset) {
    if (next === "custom" && preset.value !== "custom") {
      // Carry the window currently on screen into the custom inputs so the
      // first thing a reader sees is the range they were already looking at.
      customRange.value = { ...range.value };
    }
    preset.value = next;
  }

  watch([repoId, range], refresh, { immediate: true, deep: true });

  return {
    analytics,
    range,
    preset,
    customRange,
    loading,
    error,
    coverageGaps,
    tokenCoverageRatio,
    hasAnyData,
    contributionsFor,
    selectPreset,
    refresh,
  };
}
