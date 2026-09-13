<script setup lang="ts">
import type { AgentTerminalAttempt } from "../services/desktopServerClient";
defineProps<{ attempts: AgentTerminalAttempt[]; selected: string }>();
const emit = defineEmits<{ select: [id: string] }>();
</script>
<template>
  <select aria-label="Agent stage output" :value="selected" @click.stop @keydown.stop @change="emit('select', ($event.target as HTMLSelectElement).value)">
    <option value="">Latest</option>
    <option v-for="(attempt, index) in attempts" :key="attempt.id" :value="attempt.id">
      {{ attempt.stage }} · attempt {{ index + 1 }} · {{ attempt.startedAt }}{{ attempt.archived ? '' : ' · history unavailable' }}
    </option>
  </select>
</template>
<style scoped>
select { flex: 1 1 0; width: 100%; min-width: 0; max-width: 260px; color: inherit; background: var(--kn-bg-secondary); border: 0; font: inherit; }
</style>
