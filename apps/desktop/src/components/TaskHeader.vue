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
  verdict?: string | null;
  summary: string | null;
  exit?: string | null;
  artifacts?: Record<string, ArtifactReference> | null;
}

const props = defineProps<{
  item: TaskHeaderPresentation;
  taskId?: string;
  ownerLabel?: string;
  previewSupported?: boolean;
  latestRun?: TaskHeaderLatestRun | null;
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

function openArtifactReference(reference: ArtifactReference) {
  if (reference.type !== "stored") return;
  emit("open-artifact", reference);
}

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
    <div v-if="latestRun" class="latest-result" data-testid="latest-result">
      <span v-if="latestRun.verdict" class="verdict-badge" :data-verdict="latestRun.verdict">{{ latestRun.verdict }}</span>
      <span v-if="latestRun.summary" class="latest-result-message">{{ latestRun.summary }}</span>
      <span v-if="latestRun.exit" class="meta-item exit">
        <span class="meta-label">{{ $t('taskHeader.exitLabel') }}</span> {{ latestRun.exit }}
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
</style>
