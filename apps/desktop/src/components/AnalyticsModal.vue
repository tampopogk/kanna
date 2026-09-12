<script setup lang="ts">
/**
 * Analytics — statistics over a chosen window.
 *
 * Deliberately not a dashboard of charts. Every figure here answers a question
 * an operator asked out loud: how long is work sitting unserviced, how much is
 * the fleet shipping, how much review churn it costs, and what the agent CLIs
 * actually spent. Each one names its denominator, says where its record begins,
 * and opens into the tasks that produced it.
 */
import { computed, nextTick, onMounted, ref, toRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import {
  useEmbeddableView,
  type EmbeddableViewProps,
} from "../composables/useEmbeddableView";
import {
  useAnalytics,
  type AnalyticsDrilldown,
  type AnalyticsRangePreset,
} from "../composables/useAnalytics";
import { useKannaStore } from "../stores/kanna";
import {
  waitForViewReady,
  type DesktopViewOpenOutcome,
} from "../composables/desktopViewOpen";

const props = defineProps<EmbeddableViewProps & {
  repoId: string | null;
}>();

const {
  zIndex,
  bringToFront,
  overlayClass,
  overlayStyle,
  dismissOnScrimClick,
  focusWhenBrought,
} =
  useEmbeddableView(props);
/**
 * Analytics takes no target, so being open is the whole of being ready — but
 * "open and still loading" is not something to report as shown, so the answer
 * waits for the numbers.
 */
async function revealDesktopViewTarget(): Promise<DesktopViewOpenOutcome> {
  const settled = await waitForViewReady(() => !loading.value);
  return settled
    ? { opened: true }
    : { opened: false, code: "renderer_failed", message: "analytics is still loading" };
}

const emit = defineEmits<{ (e: "close"): void }>();

const { t } = useI18n();
const store = useKannaStore();

const {
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
} = useAnalytics(toRef(props, "repoId"));

const overlayRef = ref<HTMLDivElement | null>(null);
focusWhenBrought(overlayRef);
const drilldownRef = ref<HTMLElement | null>(null);
const openDrilldown = ref<AnalyticsDrilldown | null>(null);

// The drilldown opens below the statistics, which on a short window is below
// the fold — so without this, clicking a figure looks like it did nothing.
watch(openDrilldown, (drilldown) => {
  if (!drilldown) return;
  nextTick(() => drilldownRef.value?.scrollIntoView({ block: "nearest" }));
});

const presets: AnalyticsRangePreset[] = ["7d", "30d", "90d", "custom"];
const presetLabels: Record<AnalyticsRangePreset, string> = {
  "7d": "analytics.rangeLast7",
  "30d": "analytics.rangeLast30",
  "90d": "analytics.rangeLast90",
  custom: "analytics.rangeCustom",
};

const drilldownRows = computed(() =>
  openDrilldown.value ? contributionsFor(openDrilldown.value) : [],
);

/** Token drilldowns are counts of tokens; the others are seconds or rounds. */
const drilldownFormat = computed<"tokens" | "duration" | "count">(() => {
  if (openDrilldown.value === "idle") return "duration";
  if (openDrilldown.value === "revisions") return "count";
  return "tokens";
});

onMounted(() => {
  nextTick(() => overlayRef.value?.focus());
});

function dismiss(): boolean {
  if (openDrilldown.value) {
    openDrilldown.value = null;
    return false;
  }
  return true;
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key !== "Escape") return;
  if (dismiss()) emit("close");
}

defineExpose({ zIndex, bringToFront, dismiss, revealDesktopViewTarget });

function toggleDrilldown(drilldown: AnalyticsDrilldown) {
  openDrilldown.value = openDrilldown.value === drilldown ? null : drilldown;
}

async function openTask(taskId: string) {
  if (!isTaskNavigable(taskId)) return;
  await store.selectItem(taskId);
  emit("close");
}

function isTaskNavigable(taskId: string): boolean {
  return !!taskId && store.taskUiSlots.some((slot) => slot.task_id === taskId);
}

function formatDuration(seconds: number): string {
  // A measured zero is not the same as the em-dash this view uses for a
  // figure the forge could not confirm, and must not read as one.
  if (seconds <= 0) return "0m";
  if (seconds < 60) return `${Math.round(seconds)}s`;
  if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.round((seconds % 3600) / 60);
  if (hours < 24) return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
  const days = Math.floor(hours / 24);
  return `${days}d ${hours % 24}h`;
}

/** Token counts run to the millions; full digits are unreadable at a glance. */
function formatTokens(tokens: number): string {
  if (tokens <= 0) return "0";
  if (tokens < 1_000) return String(tokens);
  if (tokens < 1_000_000) return `${(tokens / 1_000).toFixed(1)}k`;
  if (tokens < 1_000_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  return `${(tokens / 1_000_000_000).toFixed(2)}B`;
}

function formatPercent(ratio: number | null): string {
  return ratio == null ? "—" : `${Math.round(ratio * 100)}%`;
}

function formatValue(value: number): string {
  if (drilldownFormat.value === "duration") return formatDuration(value);
  if (drilldownFormat.value === "tokens") return formatTokens(value);
  return String(value);
}

function coverageNote(since: string | null): string {
  return t("analytics.coverageGap", { since: (since ?? "").slice(0, 10) });
}

/** Relative width of one row in a group list, against its largest sibling. */
function shareOf(value: number, rows: { totals: { total: number } }[]): number {
  const largest = rows.reduce((max, row) => Math.max(max, row.totals.total), 0);
  return largest > 0 ? Math.max(2, (value / largest) * 100) : 0;
}
</script>

<template>
  <div
    ref="overlayRef"
    :class="overlayClass"
    :style="overlayStyle"
    tabindex="0"
    @click.self="dismissOnScrimClick(() => emit('close'))"
    @keydown="handleKeydown"
  >
    <div class="analytics-modal" data-testid="analytics-view">
      <header class="modal-header">
        <h2>{{ t('analytics.title') }}</h2>
        <div class="range-picker">
          <button
            v-for="option in presets"
            :key="option"
            type="button"
            class="range-option"
            :data-testid="`analytics-range-${option}`"
            :class="{ active: preset === option }"
            @click="selectPreset(option)"
          >
            {{ t(presetLabels[option]) }}
          </button>
        </div>
      </header>

      <div v-if="preset === 'custom'" class="custom-range">
        <label>
          {{ t('analytics.rangeFrom') }}
          <input v-model="customRange.from" type="date" />
        </label>
        <label>
          {{ t('analytics.rangeTo') }}
          <input v-model="customRange.to" type="date" />
        </label>
      </div>
      <p class="range-summary">{{ range.from }} → {{ range.to }}</p>

      <p v-if="loading" class="empty-state">{{ t('analytics.loading') }}</p>
      <p v-else-if="error" class="empty-state error">
        {{ t('analytics.error', { message: error }) }}
      </p>
      <p v-else-if="!hasAnyData" class="empty-state" data-testid="analytics-empty">
        {{ t('analytics.empty') }}
      </p>

      <template v-else>
        <section class="stat-section">
          <h3>{{ t('analytics.sectionFlow') }}</h3>
          <div class="stat-row">
            <div class="stat">
              <span class="stat-value" data-testid="analytics-tasks-created">{{ analytics.tasks.created }}</span>
              <span class="stat-label">{{ t('analytics.tasksCreated') }}</span>
            </div>
            <div class="stat">
              <span class="stat-value" data-testid="analytics-tasks-closed">{{ analytics.tasks.closed }}</span>
              <span class="stat-label">{{ t('analytics.tasksClosed') }}</span>
            </div>
            <div class="stat">
              <span class="stat-value" data-testid="analytics-tasks-open">{{ analytics.tasks.openNow }}</span>
              <span class="stat-label">{{ t('analytics.tasksOpen') }}</span>
            </div>
            <div class="stat">
              <span class="stat-value" data-testid="analytics-pr-created">{{ analytics.pullRequests.created ?? '—' }}</span>
              <span class="stat-label">{{ t('analytics.prCreated') }}</span>
            </div>
            <div class="stat">
              <span class="stat-value">
                {{ analytics.pullRequests.merged ?? '—' }}
              </span>
              <span class="stat-label">{{ t('analytics.prMerged') }}</span>
            </div>
            <div class="stat">
              <span class="stat-value">{{ analytics.pullRequests.openNow ?? '—' }}</span>
              <span class="stat-label">{{ t('analytics.prOpen') }}</span>
            </div>
          </div>
          <p v-if="analytics.tasks.childTasksCreated > 0" class="note">
            {{ t('analytics.childTasks', { count: analytics.tasks.childTasksCreated }) }}
          </p>
          <p
            v-if="analytics.pullRequests.created === null || !analytics.coverage.pullRequestStateConfirmed"
            class="note warning"
          >
            {{ t('analytics.prUnavailable') }}
          </p>
        </section>

        <section class="stat-section">
          <h3>{{ t('analytics.sectionIdle') }}</h3>
          <div class="stat-row">
            <button
              type="button"
              class="stat clickable"
              data-testid="analytics-idle-total"
              @click="toggleDrilldown('idle')"
            >
              <span class="stat-value">{{ formatDuration(analytics.idle.totalSeconds) }}</span>
              <span class="stat-label">{{ t('analytics.idleTotal') }}</span>
            </button>
            <div class="stat">
              <span class="stat-value">
                {{ formatDuration(analytics.idle.averageSecondsPerTask) }}
              </span>
              <span class="stat-label">{{ t('analytics.idleAverage') }}</span>
            </div>
            <div class="stat">
              <span class="stat-value">{{ formatDuration(analytics.idle.longestSeconds) }}</span>
              <span class="stat-label">{{ t('analytics.idleLongest') }}</span>
            </div>
            <div class="stat">
              <span class="stat-value">{{ formatDuration(analytics.idle.workingSeconds) }}</span>
              <span class="stat-label">{{ t('analytics.idleWorking') }}</span>
            </div>
          </div>
          <p class="note">
            {{ t('analytics.idleDenominator', { count: analytics.idle.taskCount }) }}
          </p>
          <p class="note">{{ t('analytics.idleDefinition') }}</p>
          <p v-if="coverageGaps.idle" class="note warning">
            {{ coverageNote(analytics.coverage.idleSince) }}
          </p>
        </section>

        <section class="stat-section">
          <h3>{{ t('analytics.sectionReview') }}</h3>
          <template v-if="analytics.revisions.cohortTasks > 0">
            <div class="stat-row">
              <button type="button" class="stat clickable" @click="toggleDrilldown('revisions')">
                <span class="stat-value">{{ analytics.revisions.averagePerTask.toFixed(2) }}</span>
                <span class="stat-label">{{ t('analytics.revisionAverage') }}</span>
              </button>
              <div class="stat">
                <span class="stat-value">{{ analytics.revisions.totalRevisions }}</span>
                <span class="stat-label">{{ t('analytics.revisionTotal') }}</span>
              </div>
              <div class="stat">
                <span class="stat-value">
                  {{ formatPercent(analytics.revisions.cleanPassRate) }}
                </span>
                <span class="stat-label">{{ t('analytics.revisionCleanPass') }}</span>
              </div>
            </div>
            <p class="note">
              {{ t('analytics.revisionCohort', { count: analytics.revisions.cohortTasks }) }}
            </p>
            <p v-if="analytics.revisions.parkedRequests > 0" class="note">
              {{ t('analytics.revisionParked', { count: analytics.revisions.parkedRequests }) }}
            </p>
          </template>
          <p v-else class="note">{{ t('analytics.revisionNoCohort') }}</p>
          <p v-if="coverageGaps.revisions" class="note warning">
            {{ coverageNote(analytics.coverage.revisionsSince) }}
          </p>
        </section>

        <section class="stat-section">
          <h3>{{ t('analytics.sectionTokens') }}</h3>
          <div class="stat-row">
            <button type="button" class="stat clickable" @click="toggleDrilldown('tokensByTask')">
              <span class="stat-value">{{ formatTokens(analytics.tokens.total.total) }}</span>
              <span class="stat-label">{{ t('analytics.tokensTotal') }}</span>
            </button>
            <div class="stat">
              <span class="stat-value">{{ formatTokens(analytics.tokens.total.input) }}</span>
              <span class="stat-label">{{ t('analytics.tokensInput') }}</span>
            </div>
            <div class="stat">
              <span class="stat-value">{{ formatTokens(analytics.tokens.total.cachedInput) }}</span>
              <span class="stat-label">{{ t('analytics.tokensCached') }}</span>
            </div>
            <div class="stat">
              <span class="stat-value">
                {{ formatTokens(analytics.tokens.total.cacheCreation) }}
              </span>
              <span class="stat-label">{{ t('analytics.tokensCacheWrite') }}</span>
            </div>
            <div class="stat">
              <span class="stat-value">{{ formatTokens(analytics.tokens.total.output) }}</span>
              <span class="stat-label">{{ t('analytics.tokensOutput') }}</span>
              <span class="stat-sub">
                {{ t('analytics.tokensReasoning') }}
                {{ formatTokens(analytics.tokens.total.reasoning) }}
              </span>
            </div>
          </div>

          <div v-if="analytics.tokens.byModel.length > 0" class="group-list">
            <h4>{{ t('analytics.tokensByModel') }}</h4>
            <div v-for="group in analytics.tokens.byModel" :key="group.key" class="group-row">
              <span class="group-label">{{ group.label }}</span>
              <span class="group-bar">
                <span
                  class="group-bar-fill"
                  :style="{ width: shareOf(group.totals.total, analytics.tokens.byModel) + '%' }"
                />
              </span>
              <span class="group-value">{{ formatTokens(group.totals.total) }}</span>
            </div>
          </div>

          <p v-if="tokenCoverageRatio == null || analytics.coverage.runsWithTokenUsage === 0" class="note warning">
            {{ t('analytics.tokensNoCoverage') }}
          </p>
          <p v-else class="note">
            {{
              t('analytics.tokensCoverage', {
                covered: analytics.coverage.runsWithTokenUsage,
                total: analytics.coverage.runsInRange,
              })
            }}
          </p>
          <p v-if="analytics.coverage.providersWithoutTokenUsage.length > 0" class="note warning">
            {{
              t('analytics.tokensIncomplete', {
                providers: analytics.coverage.providersWithoutTokenUsage.join(', '),
              })
            }}
          </p>
          <p v-if="coverageGaps.tokens" class="note warning">
            {{ coverageNote(analytics.coverage.tokensSince) }}
          </p>
        </section>

        <section
          v-if="openDrilldown"
          ref="drilldownRef"
          class="drilldown"
          data-testid="analytics-drilldown"
        >
          <header class="drilldown-header">
            <h3>{{ t('analytics.drilldownTitle') }}</h3>
            <button type="button" class="drilldown-close" @click="openDrilldown = null">
              {{ t('analytics.close') }}
            </button>
          </header>
          <p v-if="drilldownRows.length === 0" class="note">{{ t('analytics.drilldownEmpty') }}</p>
          <ul v-else class="drilldown-list">
            <li v-for="row in drilldownRows" :key="row.taskId || row.title">
              <button
                type="button"
                class="drilldown-row"
                :class="{ navigable: isTaskNavigable(row.taskId) }"
                :disabled="!isTaskNavigable(row.taskId)"
                @click="openTask(row.taskId)"
              >
                <span class="drilldown-title">{{ row.title }}</span>
                <span class="drilldown-value">{{ formatValue(row.value) }}</span>
              </button>
            </li>
          </ul>
        </section>
      </template>

      <p class="hint">{{ t('analytics.hint') }}</p>
    </div>
  </div>
</template>

<style scoped>
.embedded .analytics-modal {
  width: 100%;
  height: 100%;
  max-width: none;
  border: none;
  border-radius: 0;
}

.modal-overlay {
  position: fixed;
  inset: 0;
  background: var(--kn-overlay-scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  outline: none;
}

.analytics-modal {
  background: var(--kn-bg-panel);
  border: 1px solid var(--kn-border-strong);
  border-radius: 8px;
  width: 760px;
  max-width: 92vw;
  max-height: 84vh;
  padding: 24px;
  display: flex;
  flex-direction: column;
  gap: 16px;
  overflow-y: auto;
}

.modal-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
}

.modal-header h2 {
  font-size: 16px;
  font-weight: 600;
  color: var(--kn-text-secondary);
}

.range-picker {
  display: flex;
  gap: 4px;
}

.range-option {
  background: transparent;
  border: 1px solid var(--kn-border-default);
  border-radius: 5px;
  color: var(--kn-text-muted);
  cursor: pointer;
  font-size: 11px;
  padding: 4px 10px;
}

.range-option.active {
  border-color: var(--kn-accent);
  color: var(--kn-text-secondary);
}

.custom-range {
  display: flex;
  gap: 12px;
  font-size: 11px;
  color: var(--kn-text-muted);
}

.custom-range label {
  display: flex;
  align-items: center;
  gap: 6px;
}

.custom-range input {
  background: var(--kn-bg-sidebar);
  border: 1px solid var(--kn-border-default);
  border-radius: 4px;
  color: var(--kn-text-secondary);
  font-size: 11px;
  padding: 3px 6px;
}

.range-summary {
  font-size: 11px;
  color: var(--kn-text-muted);
  margin-top: -8px;
}

.stat-section {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.stat-section h3 {
  font-size: 12px;
  font-weight: 600;
  color: var(--kn-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.stat-row {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.stat {
  flex: 1 1 100px;
  background: var(--kn-bg-sidebar);
  border: 1px solid var(--kn-border-default);
  border-radius: 6px;
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  text-align: left;
}

.stat.clickable {
  cursor: pointer;
  font: inherit;
  color: inherit;
}

.stat.clickable:hover {
  border-color: var(--kn-accent);
}

.stat-value {
  font-size: 20px;
  font-weight: 600;
  color: var(--kn-text-secondary);
}

.stat-label {
  font-size: 10px;
  color: var(--kn-text-muted);
}

.stat-sub {
  font-size: 10px;
  color: var(--kn-text-muted);
  opacity: 0.8;
}

.note {
  font-size: 11px;
  color: var(--kn-text-muted);
  line-height: 1.4;
}

.note.warning {
  color: var(--kn-warning);
}

.group-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-top: 4px;
}

.group-list h4 {
  font-size: 10px;
  color: var(--kn-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.group-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 11px;
  color: var(--kn-text-muted);
}

.group-label {
  flex: 0 0 160px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.group-bar {
  flex: 1;
  height: 6px;
  background: var(--kn-bg-sidebar);
  border-radius: 3px;
  overflow: hidden;
}

.group-bar-fill {
  display: block;
  height: 100%;
  background: var(--kn-accent);
}

.group-value {
  flex: 0 0 64px;
  text-align: right;
  color: var(--kn-text-secondary);
}

.drilldown {
  border-top: 1px solid var(--kn-border-default);
  padding-top: 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.drilldown-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.drilldown-header h3 {
  font-size: 12px;
  font-weight: 600;
  color: var(--kn-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.drilldown-close {
  background: transparent;
  border: none;
  color: var(--kn-text-muted);
  cursor: pointer;
  font-size: 11px;
}

.drilldown-list {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.drilldown-row {
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  background: transparent;
  border: none;
  border-radius: 4px;
  color: var(--kn-text-muted);
  font: inherit;
  font-size: 12px;
  padding: 5px 8px;
  text-align: left;
}

.drilldown-row.navigable {
  cursor: pointer;
}

.drilldown-row.navigable:hover {
  background: var(--kn-bg-sidebar);
  color: var(--kn-text-secondary);
}

.drilldown-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.drilldown-value {
  flex: 0 0 auto;
  color: var(--kn-text-secondary);
  font-variant-numeric: tabular-nums;
}

.empty-state {
  text-align: center;
  color: var(--kn-text-muted);
  padding: 48px 0;
  font-size: 14px;
}

.empty-state.error {
  color: var(--kn-warning);
}

.hint {
  font-size: 11px;
  color: var(--kn-text-muted);
  text-align: right;
}
</style>
