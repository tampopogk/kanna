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
    <span class="stage-arrow" aria-hidden="true">▾</span>
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
.stage-arrow { width: 14px; flex: 0 0 14px; text-align: center; }
.stage-name { overflow: hidden; text-overflow: ellipsis; }
select { position: absolute; inset: 0 0 0 auto; width: 18px; opacity: 0; cursor: pointer; }
.agent-stage:focus-within .stage-arrow { outline: 1px solid var(--kn-accent); border-radius: 2px; }
</style>
