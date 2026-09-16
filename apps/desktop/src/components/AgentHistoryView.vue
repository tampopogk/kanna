<script setup lang="ts">
import { ref, watch, onBeforeUnmount, nextTick } from "vue";
import { readAgentTerminalArchive } from "../services/desktopServerClient";
import type { AgentTerminalArchive } from "../services/desktopServerClient";
import { renderTerminalArchive } from "../composables/renderTerminalArchive";
const props = defineProps<{
  taskId: string;
  attemptId: string;
  sourceKey?: string;
  loadArchive?: (attemptId: string) => Promise<AgentTerminalArchive | null>;
}>();
const body = ref<HTMLElement | null>(null);
const output = ref<HTMLElement | null>(null);
const text = ref("");
const status = ref("Loading historical output…");
let request = 0;
watch(() => [props.taskId, props.attemptId, props.sourceKey ?? ""] as const, async ([task, attempt]) => {
  const token = ++request;
  text.value = "";
  status.value = "Loading historical output…";
  try {
    const archive = props.loadArchive
      ? await props.loadArchive(attempt)
      : await readAgentTerminalArchive(task, attempt);
    if (token !== request) return;
    if (!archive) { status.value = "Historical output unavailable — this attempt has no captured archive."; return; }
    if (archive.binding.task_id !== task || archive.binding.spawned_run_id !== attempt) throw new Error("Archive identity mismatch");
    const rendered = archive.snapshot ? await renderTerminalArchive(archive.snapshot) : "";
    if (token !== request) return;
    text.value = rendered;
    const exit = archive.observed_exit_code == null ? "Exit status unknown" : `Exit ${archive.observed_exit_code}`;
    status.value = archive.snapshot ? exit : `${archive.unavailable_reason ?? 'Historical output unavailable'} · ${exit}`;
    await nextTick();
    if (token === request && body.value) body.value.scrollTop = body.value.scrollHeight;
  } catch (error) {
    if (token === request) status.value = `Historical output unavailable: ${String(error)}`;
  }
}, { immediate: true });
onBeforeUnmount(() => { request++; });
function focusContent(): boolean {
  (output.value ?? body.value)?.focus({ preventScroll: true });
  return document.activeElement === (output.value ?? body.value);
}
defineExpose({ focusContent });
</script>
<template>
  <section ref="body" class="agent-history" aria-label="Read-only historical agent output" data-testid="agent-history">
    <pre ref="output" tabindex="0">{{ text }}</pre>
    <p role="status">{{ status }}</p>
  </section>
</template>
<style scoped>
.agent-history { height: 100%; overflow: auto; padding: 12px; box-sizing: border-box; }
pre { font-family: monospace; margin: 0; white-space: pre; }
p { color: var(--kn-text-muted); }
</style>
