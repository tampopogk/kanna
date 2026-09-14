<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";
import { isTauri } from "../tauri-mock";
import { fetchDesktopTaskDetail } from "../services/desktopServerClient";
import { localTaskPreviewUrl } from "../utils/taskPreview";

const props = defineProps<{ taskId: string; portName: string; workspace: string; supported: boolean; visible: boolean }>();
const emit = defineEmits<{ (e: "activate"): void }>();
const frame = ref<HTMLIFrameElement | null>(null);
let focusFrame = 0;
function onFrameFocus() {
  // Events inside a cross-origin frame do not bubble through the host pane.
  cancelAnimationFrame(focusFrame);
  focusFrame = requestAnimationFrame(() => {
    if (props.visible && frame.value && document.activeElement === frame.value) emit("activate");
  });
}
onMounted(() => {
  window.addEventListener("blur", onFrameFocus);
  document.addEventListener("focusout", onFrameFocus);
});
onBeforeUnmount(() => {
  cancelAnimationFrame(focusFrame);
  window.removeEventListener("blur", onFrameFocus);
  document.removeEventListener("focusout", onFrameFocus);
});
const url = ref("");
const error = ref("");
const loading = ref(false);
let generation = 0;
async function reload() {
  const request = ++generation;
  url.value = "";
  error.value = "";
  if (!props.supported) return;
  loading.value = true;
  const { taskId, workspace, portName } = props;
  try {
    // Resolve against the owning local server each time this view is rebuilt.
    // No address or remote snapshot is persisted with the tab.
    const detail = await fetchDesktopTaskDetail(taskId, { localOnly: true });
    const resolved = localTaskPreviewUrl(detail, { taskId, workspace, portName });
    if (request === generation) url.value = resolved;
  } catch (cause) {
    if (request === generation) error.value = cause instanceof Error ? cause.message : String(cause);
  } finally {
    if (request === generation) loading.value = false;
  }
}
watch(() => [props.taskId, props.workspace, props.portName, props.supported], () => {
  ++generation;
  loading.value = false;
  url.value = "";
  if (props.visible) void reload();
}, { immediate: true });
watch(() => props.visible, visible => { if (visible && !url.value && !loading.value) void reload(); });
// A warm preview retains its in-page location. Revalidate its owning workspace
// and port on return without navigating a still-valid frame.
watch(() => props.visible, async visible => {
  if (!visible || !url.value || !props.supported) return;
  const request = ++generation;
  try {
    const detail = await fetchDesktopTaskDetail(props.taskId, { localOnly: true });
    const resolved = localTaskPreviewUrl(detail, props);
    if (request === generation && resolved !== url.value) void reload();
  } catch (cause) {
    if (request === generation) {
      url.value = "";
      error.value = cause instanceof Error ? cause.message : String(cause);
    }
  }
});
onBeforeUnmount(() => { ++generation; });
async function openExternal() {
  if (!url.value) return;
  if (isTauri) await openUrl(url.value);
  else window.open(url.value, "_blank", "noopener,noreferrer");
}
</script>
<template>
  <section class="task-preview" data-testid="task-preview">
    <div class="preview-toolbar">
      <span :title="workspace">{{ portName }} · This machine</span>
      <button v-if="supported" @click="reload">Reload</button>
      <button v-if="url" @click="openExternal">Open in browser ↗</button>
    </div>
    <p v-if="!supported">Preview is available on the desktop owning this task.</p>
    <p v-else-if="loading">Resolving task preview…</p>
    <p v-else-if="error" role="alert">{{ error }}</p>
    <template v-else-if="url">
      <p class="preview-hint">{{ url }} · Start this task’s server first. If embedding is blocked, open in browser for the full app and DevTools.</p>
      <iframe ref="frame" @focus="emit('activate')" :src="url" :title="`${taskId} · ${portName} preview`" sandbox="allow-scripts allow-same-origin allow-forms" referrerpolicy="no-referrer" />
    </template>
  </section>
</template>
<style scoped>
.task-preview { display: flex; flex: 1; flex-direction: column; min-height: 0; min-width: 0; }
.preview-toolbar { display: flex; gap: 8px; align-items: center; padding: 8px 12px; border-bottom: 1px solid var(--kn-border-default); font-size: 12px; }
.preview-toolbar span { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
button { background: var(--kn-bg-panel-raised); color: var(--kn-text-secondary); border: 1px solid var(--kn-border-default); border-radius: 4px; padding: 3px 6px; cursor: pointer; }
p { margin: 8px 12px; font-size: 12px; color: var(--kn-text-muted); }
.preview-hint { font-size: 11px; }
iframe { flex: 1; min-height: 0; width: 100%; border: 0; background: white; }
</style>
