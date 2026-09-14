<script setup lang="ts">
import { computed } from "vue";
import type { AgentTerminalAttempt } from "../services/desktopServerClient";
const props = defineProps<{ attempts: AgentTerminalAttempt[]; selected: string; currentStage?: string }>();
const emit = defineEmits<{ select: [id: string] }>();
function selectorKey(event: KeyboardEvent) {
  // Native option navigation stays local; app shortcuts still cycle tabs.
  if (!event.metaKey && !event.ctrlKey) event.stopPropagation();
}
const stage = computed(() => (props.selected ? props.attempts.find(attempt => attempt.id === props.selected)?.stage : props.currentStage) ?? 'Agent');
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
      <option v-for="(attempt, index) in attempts" :key="attempt.id" :value="attempt.id">
        {{ attempt.stage }} · attempt {{ index + 1 }} · {{ attempt.startedAt }}{{ attempt.archived ? '' : ' · history unavailable' }}
      </option>
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
