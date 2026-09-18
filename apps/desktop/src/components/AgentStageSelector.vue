<script setup lang="ts">
import { computed } from "vue";
import type { AgentTerminalAttempt, WorkspaceSetupRun } from "../services/desktopServerClient";
import { stageHistoryItems } from "../utils/agentStageHistory";
const props = defineProps<{
  attempts: AgentTerminalAttempt[];
  setupRuns?: WorkspaceSetupRun[];
  selected: string;
  currentStage?: string;
  historyStatus?: string;
}>();
const emit = defineEmits<{ select: [id: string] }>();
// The list is history only: the server reports which attempt the daemon still
// runs the terminal for, and that session is what "Latest" already shows.
// Everything else the task left behind belongs here — the agent's own earlier
// sessions, the teardown that cleaned a departed workspace up, and the setup
// that prepared each spawn. `stageHistoryItems` owns the labelling and the
// attempt numbering, which counts agent sessions only.
const items = computed(() => stageHistoryItems(props.attempts, props.setupRuns ?? []));
function selectorKey(event: KeyboardEvent) {
  // Native option navigation stays local; app shortcuts still cycle tabs.
  if (!event.metaKey && !event.ctrlKey) event.stopPropagation();
}
const stage = computed(() => (props.selected
  ? items.value.find(item => item.value === props.selected)?.title
  : props.currentStage) ?? 'Agent');
</script>
<template>
  <span class="agent-stage">
    <span class="stage-name">{{ stage }}</span>
    <span class="stage-arrow" aria-hidden="true">
      <svg width="16" height="16" viewBox="0 0 16 16">
        <path d="m3.5 6 4.5 4 4.5-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
    </span>
    <select aria-label="Agent stage output" :value="selected" @click.stop @keydown="selectorKey" @change="emit('select', ($event.target as HTMLSelectElement).value)">
      <option value="">Latest{{ currentStage ? ` · ${currentStage}` : '' }}</option>
      <option v-for="item in items" :key="item.value" :value="item.value">{{ item.label }}</option>
      <option v-if="historyStatus" value="" disabled>{{ historyStatus }}</option>
    </select>
  </span>
</template>
<style scoped>
.agent-stage { display: inline-flex; position: relative; align-items: center; gap: 5px; min-width: 0; max-width: 220px; user-select: none; -webkit-user-select: none; }
.stage-arrow { display: inline-flex; width: 22px; height: 18px; flex: 0 0 22px; align-items: center; justify-content: center; color: var(--kn-text-secondary); }
.stage-name { overflow: hidden; text-overflow: ellipsis; }
select { position: absolute; inset: -3px 0 -3px auto; width: 24px; opacity: 0; cursor: pointer; }
.agent-stage:focus-within .stage-arrow { outline: 1px solid var(--kn-accent); border-radius: 2px; }
</style>
