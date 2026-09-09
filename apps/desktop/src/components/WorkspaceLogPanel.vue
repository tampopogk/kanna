<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from "vue";
import {
  fetchDesktopTaskActivity,
  type DesktopTaskActivityEntry,
} from "../services/desktopServerClient";
import { getAppErrorMessage } from "../appError";

/**
 * A task's workspace, as a log rather than a shell.
 *
 * Workspace creation, the startup script, the agent starting and finishing,
 * teardown, then the next stage's startup — one read-only chronological view
 * of what happened around the agent, so a person can see setup and teardown
 * without an agent's screen repainting over them. It is a viewer of operations
 * this task performed, not an interactive terminal shared with anyone, and it
 * is not one shell: the entries come from different sessions and different
 * workspaces, which is why it can span stages at all.
 *
 * A finished agent's own output is deliberately absent — that is its own tab,
 * kept per stage and attempt. Repeating it here would be the cross-stage
 * chaining this replaces.
 */
const props = defineProps<{
  taskId: string;
  /** Changes whenever the server may have recorded something new. */
  revision?: unknown;
  active?: boolean;
}>();

const emit = defineEmits<{
  (event: "open-terminal", terminalSessionId: string): void;
}>();

const entries = ref<DesktopTaskActivityEntry[]>([]);
const error = ref<string | null>(null);
const loading = ref(true);
let disposed = false;

async function refresh(): Promise<void> {
  const taskId = props.taskId;
  if (!taskId) return;
  try {
    const activity = await fetchDesktopTaskActivity(taskId);
    if (disposed || props.taskId !== taskId) return;
    entries.value = activity.entries;
    error.value = null;
  } catch (cause: unknown) {
    if (disposed || props.taskId !== taskId) return;
    // Keep whatever the log already shows: a momentary server hiccup must not
    // make a task's history look as though it never happened.
    error.value = getAppErrorMessage(cause);
  } finally {
    if (!disposed) loading.value = false;
  }
}

watch(
  [() => props.taskId, () => props.revision],
  () => {
    void refresh();
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  disposed = true;
});

function entryStatus(entry: DesktopTaskActivityEntry): string {
  if (entry.kind === "agent") return entry.status ?? "";
  if (entry.status === "live") return "running";
  return entry.exitCode === null || entry.exitCode === 0
    ? "finished"
    : `exited ${entry.exitCode}`;
}
</script>

<template>
  <div class="workspace-log" data-testid="workspace-log">
    <div v-if="error" class="workspace-log-error">{{ error }}</div>
    <div v-else-if="loading" class="workspace-log-empty">{{ $t('common.loading') }}</div>
    <div v-else-if="entries.length === 0" class="workspace-log-empty">
      {{ $t('workspaceLog.empty') }}
    </div>
    <ol v-else class="workspace-log-entries">
      <li
        v-for="(entry, index) in entries"
        :key="`${entry.kind}-${entry.at}-${index}`"
        class="workspace-log-entry"
        :class="`workspace-log-entry-${entry.kind}`"
      >
        <span class="workspace-log-at">{{ entry.at }}</span>
        <span class="workspace-log-title">{{ entry.title }}</span>
        <span class="workspace-log-status">{{ entryStatus(entry) }}</span>
        <button
          v-if="entry.terminalSessionId && entry.archived"
          type="button"
          class="workspace-log-open"
          :data-testid="`workspace-log-open-${entry.terminalSessionId}`"
          @click="emit('open-terminal', entry.terminalSessionId)"
        >
          {{ $t('workspaceLog.openOutput') }}
        </button>
      </li>
    </ol>
  </div>
</template>

<style scoped>
.workspace-log {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 8px 12px;
  font-size: 12px;
}

.workspace-log-entries {
  list-style: none;
  margin: 0;
  padding: 0;
}

.workspace-log-entry {
  display: flex;
  gap: 10px;
  align-items: baseline;
  padding: 3px 0;
  border-bottom: 1px solid var(--kn-border);
}

.workspace-log-entry-agent .workspace-log-title {
  font-weight: 600;
}

.workspace-log-at {
  color: var(--kn-text-muted);
  font-family: "JetBrains Mono", "SF Mono", Menlo, monospace;
  white-space: nowrap;
}

.workspace-log-title {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.workspace-log-status {
  color: var(--kn-text-muted);
  white-space: nowrap;
}

.workspace-log-open {
  background: none;
  border: 1px solid var(--kn-border);
  border-radius: 4px;
  color: inherit;
  cursor: pointer;
  font-size: 11px;
  padding: 1px 6px;
}

.workspace-log-empty,
.workspace-log-error {
  color: var(--kn-text-muted);
  padding: 6px 0;
}
</style>
