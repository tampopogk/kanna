<script setup lang="ts">
import { ref, watch, type CSSProperties } from "vue";
import TaskPreviewView from "./TaskPreviewView.vue";

interface PreviewEntry { key: string; taskId: string; portName: string; workspace: string; supported: boolean; style?: CSSProperties }
const props = defineProps<{ visibleEntries: PreviewEntry[]; workspaces: Record<string, string> }>();
const emit = defineEmits<{ (e: "activate", key: string): void }>();
const entries = ref<PreviewEntry[]>([]);
const recency = new Map<string, number>();
let sequence = 0;
watch(() => props.visibleEntries, visible => {
  for (const entry of visible) {
    // Keep frames in the DOM: moving an iframe through KeepAlive's detached
    // container can reload it and lose its in-page location.
    recency.set(entry.key, ++sequence);
    const existing = entries.value.findIndex(candidate => candidate.key === entry.key);
    if (existing >= 0) entries.value[existing] = entry;
    else entries.value.push(entry);
  }
  if (entries.value.length > Math.max(5, visible.length)) {
    const oldest = entries.value.filter(entry => !visible.some(current => current.key === entry.key)).sort((a, b) => (recency.get(a.key) ?? 0) - (recency.get(b.key) ?? 0))[0];
    if (oldest) discard(oldest.key);
  }
}, { immediate: true });
// Closed, transferred, and superseded workspaces must not keep hidden pages alive.
watch(() => props.workspaces, workspaces => {
  for (const cached of entries.value) {
    if (workspaces[cached.taskId] !== cached.workspace) discard(cached.key);
  }
}, { deep: true });
function discard(key: string) { recency.delete(key); entries.value = entries.value.filter(entry => entry.key !== key); }
defineExpose({ discard });
</script>
<template>
  <div class="preview-cache">
    <TaskPreviewView
      v-for="cached in entries" :key="cached.key"
      v-show="visibleEntries.some(entry => entry.key === cached.key)"
      :style="visibleEntries.find(entry => entry.key === cached.key)?.style"
      :task-id="cached.taskId" :port-name="cached.portName"
      :workspace="cached.workspace" :supported="cached.supported"
      :visible="visibleEntries.some(entry => entry.key === cached.key)"
      @activate="emit('activate', cached.key)"
      @pointerdown.capture="emit('activate', cached.key)"
      @focusin="emit('activate', cached.key)"
    />
  </div>
</template>

<style scoped>
.preview-cache { display: contents; }
</style>
