<script setup lang="ts">
import { ref, watch } from "vue";
import TaskPreviewView from "./TaskPreviewView.vue";

interface PreviewEntry { key: string; taskId: string; portName: string; workspace: string; supported: boolean }
const props = defineProps<{ entry: PreviewEntry | null; workspaces: Record<string, string> }>();
const emit = defineEmits<{ (e: "activate"): void }>();
const entries = ref<PreviewEntry[]>([]);
const recency = new Map<string, number>();
let sequence = 0;
watch(() => props.entry, entry => {
  if (!entry) return;
  // Keep frames in the DOM: moving an iframe through KeepAlive's detached
  // container can reload it and lose its in-page location.
  recency.set(entry.key, ++sequence);
  const existing = entries.value.findIndex(candidate => candidate.key === entry.key);
  if (existing >= 0) entries.value[existing] = entry;
  else entries.value.push(entry);
  if (entries.value.length > 5) {
    const oldest = [...entries.value].sort((a, b) => (recency.get(a.key) ?? 0) - (recency.get(b.key) ?? 0))[0];
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
  <div v-show="entry" class="preview-cache">
    <TaskPreviewView v-for="cached in entries" :key="cached.key" v-show="entry?.key === cached.key" :task-id="cached.taskId" :port-name="cached.portName" :workspace="cached.workspace" :supported="cached.supported" :visible="entry?.key === cached.key" @activate="emit('activate')" />
  </div>
</template>

<style scoped>
.preview-cache { display: flex; flex: 1; min-height: 0; min-width: 0; }
</style>
