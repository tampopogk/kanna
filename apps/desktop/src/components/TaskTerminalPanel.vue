<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import TerminalView from "./TerminalView.vue";

/**
 * One of a task's non-agent terminals: the startup shell a launch ran its
 * setup in, or a departing workspace's teardown.
 *
 * It attaches and never spawns. These sessions belong to a launch the server
 * started; a view that could create one would be able to conjure a terminal
 * for a launch that never happened, and a startup shell that has exited is
 * finished — re-running it is a rerun, not a repaint.
 */
const props = defineProps<{
  sessionId: string;
  title: string;
  /** False once the process behind this terminal has exited. */
  live?: boolean;
  /** The status it exited with, when it has exited. */
  exitCode?: number | null;
  active?: boolean;
}>();

const termRef = ref<InstanceType<typeof TerminalView> | null>(null);

watch(
  () => props.active,
  async (active) => {
    if (!active) return;
    await nextTick();
    termRef.value?.fit?.();
  },
);
</script>

<template>
  <div class="task-terminal-panel">
    <div v-if="live === false" class="task-terminal-status" data-testid="task-terminal-finished">
      {{
        exitCode === 0 || exitCode === null || exitCode === undefined
          ? $t('taskTerminal.finished', { title })
          : $t('taskTerminal.finishedWithStatus', { title, status: exitCode })
      }}
    </div>
    <TerminalView
      ref="termRef"
      :key="sessionId"
      :session-id="sessionId"
      :active="active !== false"
    />
  </div>
</template>

<style scoped>
.task-terminal-panel {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
}

.task-terminal-status {
  padding: 4px 10px;
  font-size: 12px;
  color: var(--kn-text-muted);
  border-bottom: 1px solid var(--kn-border);
}
</style>
