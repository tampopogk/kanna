<script setup lang="ts">
import { computed, ref, watch, onBeforeUnmount } from "vue";
import ReadOnlyTerminal from "./ReadOnlyTerminal.vue";
import { readTaskCreationProgress, type TaskCreationProgress } from "../services/desktopServerClient";
const props = defineProps<{ taskId: string; creating: boolean; error?: string; selected?: boolean }>();
const progress = ref<TaskCreationProgress | null>(null);
const loadError = ref("");
const emit = defineEmits<{ completed: [status: string | null] }>();
let generation = 0;
let timer: ReturnType<typeof setTimeout> | undefined;
watch(() => props.taskId, () => {
  const token = ++generation;
  clearTimeout(timer);
  progress.value = null;
  loadError.value = "";
  emit("completed", null);
  async function poll() {
    try {
      const loaded = await readTaskCreationProgress(props.taskId);
      if (token !== generation) return;
      progress.value = loaded;
      loadError.value = "";
    } catch (error) {
      if (token !== generation) return;
      loadError.value = `Unable to read creation progress: ${String(error)}`;
    }
    if (token === generation && !props.error && (progress.value?.status === "running" || (props.creating && !progress.value))) {
      timer = setTimeout(poll, 250);
    }
  }
  void poll();
}, { immediate: true });
watch(() => progress.value?.status, status => {
  if (status && status !== "running") emit("completed", status);
});
// The create response can beat the last poll. Fetch the terminal snapshot once
// so the final stderr bytes remain in the transcript, not just the error alert.
watch(() => props.error, async error => {
  if (!error) return;
  const token = ++generation;
  clearTimeout(timer);
  try {
    const loaded = await readTaskCreationProgress(props.taskId);
    if (token === generation && loaded) progress.value = loaded;
  } catch { /* The create response itself still carries the diagnostic. */ }
});
onBeforeUnmount(() => { generation++; clearTimeout(timer); });
const transcript = computed(() => {
  let output = progress.value?.output ?? "";
  const diagnostic = props.error || progress.value?.error;
  if (diagnostic && !output.includes(diagnostic)) output += `\r\n${diagnostic}\r\n`;
  if (loadError.value) output += `\r\n${loadError.value}\r\n`;
  return output;
});
</script>
<template>
  <div v-show="creating || selected || (!!error && !progress)" class="creation-progress" data-testid="creation-progress">
    <ReadOnlyTerminal :key="taskId" :identity="`creation:${taskId}`" :output="transcript" />
  </div>
</template>
<style scoped>
.creation-progress { flex: 1; min-height: 0; padding: 4px 8px; }
</style>
