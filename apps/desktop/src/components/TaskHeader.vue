<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { ArtifactReference } from "@kanna/core";
import { isTauri } from "../tauri-mock";

const { t } = useI18n();

interface TaskHeaderPresentation {
  launchProvider?: string | null;
  launchModel?: string | null;
  display_name: string | null;
  issue_title: string | null;
  prompt: string | null;
  stage: string;
  stage_advance_pending?: boolean;
  stage_advance_from?: string | null;
  branch: string | null;
  port_env: string | null;
  issue_number: number | null;
  pr_number: number | null;
  pr_url: string | null;
}

/**
 * The task's latest recorded result (spec §16.8). A trimmed view of
 * `DesktopTaskLatestRun` — only the fields this header shows — so this
 * component stays decoupled from the full wire shape.
 */
export interface TaskHeaderLatestRun {
  id?: string;
  verdict?: string | null;
  summary: string | null;
  exit?: string | null;
  artifacts?: Record<string, ArtifactReference> | null;
  /** The session identity T2 recorded when this run started (spec §16.8,
   * T11b). Absent on a server predating it. */
  session?: { name?: string | null } | null;
  /** The transition-commit step bound to this run's exit (T3), when one is
   * pending, running, or has finished. */
  commitStep?: { state: string; exit?: string | null } | null;
}

/** One historical stage-run session, oldest first (T11b). A trimmed view of
 * `DesktopTaskSessionHistoryEntry`. */
export interface TaskHeaderSessionHistoryEntry {
  runId: string;
  stage: string;
  session: { name?: string | null };
}

/** One stage-dependency edge into this task (T4). A trimmed view of
 * `DesktopTaskStageDependency`. */
export interface TaskHeaderStageDependency {
  upstreamTaskId: string;
  upstreamStage: string;
  supersededAt?: string | null;
}

/** This task's recorded automatic-advance dependency wait (T4). */
export interface TaskHeaderDependencyWait {
  fromStage: string;
  toStage: string;
}

const props = defineProps<{
  item: TaskHeaderPresentation;
  taskId?: string;
  ownerLabel?: string;
  previewSupported?: boolean;
  latestRun?: TaskHeaderLatestRun | null;
  sessionHistory?: TaskHeaderSessionHistoryEntry[] | null;
  stageDependencies?: TaskHeaderStageDependency[] | null;
  dependencyWait?: TaskHeaderDependencyWait | null;
  /** True when the current stage has no agent role and a person must
   * decide (spec's roleless Gate stage, T3). */
  gateParked?: boolean | null;
}>();

const emit = defineEmits<{
  (e: "preview", portName: string): void;
  (e: "open-artifact", reference: Extract<ArtifactReference, { type: "stored" }>): void;
}>();

const latestResultArtifacts = computed(() => {
  const artifacts = props.latestRun?.artifacts;
  if (!artifacts) return [];
  return Object.entries(artifacts).map(([name, reference]) => ({ name, reference }));
});

// A run still in flight has a non-null `latestRun` with no verdict, summary,
// exit or artifacts recorded yet (mobile_api.rs's `map_task_latest_run`
// yields all `None` until a result lands). Showing the container then would
// be an empty bordered row.
const hasLatestResult = computed(() => {
  const run = props.latestRun;
  if (!run) return false;
  return Boolean(run.verdict || run.summary || run.exit || run.commitStep || latestResultArtifacts.value.length > 0);
});

function openArtifactReference(reference: ArtifactReference) {
  if (reference.type !== "stored") return;
  emit("open-artifact", reference);
}

const sessionName = computed(() => props.latestRun?.session?.name || null);

const priorSessionHistory = computed(() => {
  const history = props.sessionHistory ?? [];
  // The latest run's own session is already shown by `sessionName`; the
  // history list under it is what came before, oldest first as recorded.
  const currentRunId = props.latestRun?.id;
  return history.filter((entry) => entry.runId !== currentRunId && entry.session.name);
});

const supersededDependencies = computed(() => (props.stageDependencies ?? []).filter((dep) => dep.supersededAt));

const stageBadgeLabel = computed(() => {
  const from = props.item.stage_advance_from;
  if (props.item.stage_advance_pending && from && from !== props.item.stage) {
    return `${from} → ${props.item.stage}…`;
  }
  return props.item.stage;
});

function title(item: TaskHeaderPresentation): string {
  return item.display_name || item.issue_title || item.prompt || t('tasks.untitled');
}

function taskPromptTooltip(item: TaskHeaderPresentation): string | undefined {
  return item.prompt || undefined;
}

interface PortBadge {
  envName: string;
  port: number;
}

const ports = computed<PortBadge[]>(() => {
  if (!props.item.port_env) return [];
  try {
    const env = JSON.parse(props.item.port_env) as Record<string, string | number>;
    return Object.entries(env)
      .map(([envName, value]) => ({ envName, port: Number(value) }))
      .filter(({ port }) => Number.isInteger(port) && port > 0 && port <= 65535)
      .sort((a, b) => a.port - b.port || a.envName.localeCompare(b.envName));
  } catch (error) {
    console.debug("[task-header] failed to parse task port_env:", error);
    return [];
  }
});

const copied = ref(false);
function copyBranch() {
  if (!props.item.branch) return;
  navigator.clipboard.writeText(props.item.branch);
  copied.value = true;
  setTimeout(() => { copied.value = false; }, 1500);
}

function openLocalhostPort(port: number) {
  const url = `http://localhost:${port}`;
  if (isTauri) {
    openUrl(url).catch((error) => console.error("[task-header] Failed to open port:", error));
    return;
  }
  window.open(url, "_blank");
}
</script>

<template>
  <div class="task-header" @mousedown.prevent>
    <div class="header-top">
      <span
        class="stage-badge"
        :class="{ 'stage-badge-pending': item.stage_advance_pending }"
        :title="item.stage_advance_pending ? $t('taskHeader.stageAdvancePending') : undefined"
      >{{ stageBadgeLabel }}</span>
      <h2 class="task-title" :title="taskPromptTooltip(item)" @mousedown.stop>{{ title(item) }}</h2>
    </div>
    <div class="header-meta">
      <span v-if="taskId" class="meta-item">{{ taskId }} · {{ ownerLabel }}</span>
      <span v-if="item.branch" class="meta-item branch" @dblclick="copyBranch">
        <span class="meta-label">{{ $t('taskHeader.branchLabel') }}</span> {{ copied ? $t('taskHeader.copied', 'Copied!') : item.branch }}
      </span>
      <span v-if="sessionName" class="meta-item session" data-testid="session-name">
        <span class="meta-label">{{ $t('taskHeader.sessionLabel') }}</span> {{ sessionName }}
      </span>
      <details v-if="priorSessionHistory.length" class="session-history" data-testid="session-history">
        <summary class="meta-item session-history-toggle">{{ $t('taskHeader.sessionHistoryLabel') }} ({{ priorSessionHistory.length }})</summary>
        <ul class="session-history-list">
          <li v-for="entry in priorSessionHistory" :key="entry.runId" class="session-history-entry">
            <span class="meta-label">{{ entry.stage }}</span> {{ entry.session.name }}
          </li>
        </ul>
      </details>
      <button
        v-for="portInfo in ports"
        :key="`${portInfo.envName}:${portInfo.port}`"
        class="meta-item port"
        :title="`${portInfo.envName}=${portInfo.port}${previewSupported ? ' · Preview' : ''}`"
        :disabled="!previewSupported && !!ownerLabel"
        @mousedown.stop
        @click="previewSupported ? emit('preview', portInfo.envName) : openLocalhostPort(portInfo.port)"
      >
        :{{ portInfo.port }}
      </button>
      <a
        v-if="item.issue_number"
        class="meta-item link"
        :href="`#issue-${item.issue_number}`"
        @click.prevent
      >
        #{{ item.issue_number }}
      </a>
      <a
        v-if="item.pr_number && item.pr_url"
        class="meta-item link"
        :href="item.pr_url"
        target="_blank"
      >
        {{ $t('taskHeader.prPrefix') }}{{ item.pr_number }}
      </a>
    </div>
    <div v-if="gateParked" class="gate-parked" data-testid="gate-parked">
      {{ $t('taskHeader.gateParked') }}
    </div>
    <div v-if="dependencyWait" class="meta-item dependency-wait" data-testid="dependency-wait">
      {{ $t('taskHeader.dependencyWait', { fromStage: dependencyWait.fromStage, toStage: dependencyWait.toStage }) }}
    </div>
    <div v-if="supersededDependencies.length" class="meta-item dependency-superseded" data-testid="dependency-superseded">
      <span v-for="dep in supersededDependencies" :key="`${dep.upstreamTaskId}:${dep.upstreamStage}`">
        {{ $t('taskHeader.dependencySuperseded', { upstreamTaskId: dep.upstreamTaskId, upstreamStage: dep.upstreamStage }) }}
      </span>
    </div>
    <div v-if="hasLatestResult && latestRun" class="latest-result" data-testid="latest-result">
      <span v-if="latestRun.verdict" class="verdict-badge" :data-verdict="latestRun.verdict">{{ latestRun.verdict }}</span>
      <span v-if="latestRun.summary" class="latest-result-message">{{ latestRun.summary }}</span>
      <span v-if="latestRun.exit" class="meta-item exit">
        <span class="meta-label">{{ $t('taskHeader.exitLabel') }}</span> {{ latestRun.exit }}
      </span>
      <span v-if="latestRun.commitStep" class="meta-item commit-step" :data-commit-state="latestRun.commitStep.state" data-testid="commit-step">
        <span class="meta-label">{{ $t('taskHeader.commitStepLabel') }}</span> {{ latestRun.commitStep.state }}
      </span>
      <button
        v-for="artifact in latestResultArtifacts"
        :key="artifact.name"
        class="meta-item artifact-chip"
        type="button"
        :disabled="artifact.reference.type !== 'stored'"
        @mousedown.stop
        @click="openArtifactReference(artifact.reference)"
      >
        {{ artifact.name }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.task-header {
  padding: 12px 16px;
  border-bottom: 1px solid var(--kn-border-default);
  background: var(--kn-bg-sidebar);
}

.header-top {
  display: flex;
  align-items: center;
  gap: 10px;
}

.stage-badge {
  display: inline-block;
  padding: 1px 6px;
  border-radius: 3px;
  font-size: 11px;
  font-weight: 600;
  color: var(--kn-accent);
  white-space: nowrap;
  line-height: 1.4;
  background: var(--kn-bg-accent-subtle);
  flex-shrink: 0;
}

.stage-badge-pending {
  color: var(--kn-warning);
}

.task-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--kn-text-primary);
  margin: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  user-select: text;
  cursor: text;
}

.header-meta {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 12px;
  margin-top: 6px;
  font-size: 12px;
}

.meta-item {
  color: var(--kn-text-muted);
}

.meta-label {
  color: var(--kn-text-muted);
}

.branch {
  font-family: "JetBrains Mono", "SF Mono", Menlo, monospace;
  font-size: 11px;
  background: var(--kn-bg-panel-raised);
  padding: 1px 6px;
  border-radius: 3px;
  cursor: default;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.port {
  border: 0;
  font-family: "JetBrains Mono", "SF Mono", Menlo, monospace;
  font-size: 11px;
  background: var(--kn-bg-panel-raised);
  padding: 1px 6px;
  border-radius: 3px;
  color: var(--kn-success);
  cursor: pointer;
  flex-shrink: 0;
}

.link {
  color: var(--kn-accent);
  text-decoration: none;
}

.link:hover {
  text-decoration: underline;
}

.latest-result {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 6px;
  font-size: 12px;
}

.verdict-badge {
  display: inline-block;
  padding: 1px 6px;
  border-radius: 3px;
  font-size: 11px;
  font-weight: 600;
  white-space: nowrap;
  line-height: 1.4;
  flex-shrink: 0;
  color: var(--kn-text-muted);
  background: var(--kn-bg-panel-raised);
}

.verdict-badge[data-verdict="success"] {
  color: var(--kn-success);
  background: color-mix(in srgb, var(--kn-success) 15%, transparent);
}

.verdict-badge[data-verdict="failure"] {
  color: var(--kn-danger);
  background: color-mix(in srgb, var(--kn-danger) 15%, transparent);
}

.verdict-badge[data-verdict="needs-input"],
.verdict-badge[data-verdict="declined"] {
  color: var(--kn-warning);
  background: color-mix(in srgb, var(--kn-warning) 15%, transparent);
}

.latest-result-message {
  color: var(--kn-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.artifact-chip {
  border: 0;
  font-size: 11px;
  background: var(--kn-bg-panel-raised);
  padding: 1px 6px;
  border-radius: 3px;
  color: var(--kn-accent);
  cursor: pointer;
  flex-shrink: 0;
}

.artifact-chip:disabled {
  cursor: default;
  color: var(--kn-text-muted);
}

.session {
  font-family: "JetBrains Mono", "SF Mono", Menlo, monospace;
  font-size: 11px;
}

.session-history {
  font-size: 11px;
}

.session-history-toggle {
  cursor: pointer;
  color: var(--kn-text-muted);
  list-style: none;
}

.session-history-list {
  margin: 4px 0 0;
  padding-left: 14px;
  color: var(--kn-text-muted);
}

.gate-parked {
  margin-top: 6px;
  padding: 4px 8px;
  border-radius: 3px;
  font-size: 12px;
  font-weight: 600;
  color: var(--kn-warning);
  background: color-mix(in srgb, var(--kn-warning) 15%, transparent);
}

.dependency-wait,
.dependency-superseded {
  margin-top: 6px;
  font-size: 12px;
}

.commit-step {
  font-family: "JetBrains Mono", "SF Mono", Menlo, monospace;
}

.commit-step[data-commit-state="failed"] {
  color: var(--kn-danger);
}

.commit-step[data-commit-state="succeeded"] {
  color: var(--kn-success);
}
</style>
