<script setup lang="ts">
import { computed, ref, watch, onBeforeUnmount } from "vue";
import { stripAnsi } from "@kanna/core";
import { readWorkspaceSetupRun, type WorkspaceSetupRun } from "../services/desktopServerClient";
/**
 * A stage run's stored workspace-setup output, read only.
 *
 * Sibling of `AgentHistoryView`, which renders a terminal archive's frames.
 * Setup ran on the server-side workspace command runner with no PTY, so what
 * it left is captured text: a plain scrollable transcript, not a rendered
 * terminal — so the colour codes the commands wrote have no terminal to
 * interpret them and are stripped rather than printed. Only the owner machine
 * serves this stream today, so the selector
 * offers no Setup item for a task presented from another desktop and this view
 * is never mounted for one.
 */
const props = defineProps<{ taskId: string; runId: string }>();
const body = ref<HTMLElement | null>(null);
const output = ref<HTMLElement | null>(null);
const run = ref<WorkspaceSetupRun | null>(null);
const status = ref("Loading setup output…");
let request = 0;
watch(() => [props.taskId, props.runId] as const, async ([task, runId]) => {
  const token = ++request;
  run.value = null;
  status.value = "Loading setup output…";
  try {
    const loaded = await readWorkspaceSetupRun(task, runId);
    if (token !== request) return;
    if (!loaded) { status.value = "Setup output unavailable — this stage run recorded none."; return; }
    run.value = loaded;
    status.value = "";
  } catch (error) {
    if (token === request) status.value = `Setup output unavailable: ${String(error)}`;
  }
}, { immediate: true });
onBeforeUnmount(() => { request++; });
const transcript = computed(() => stripAnsi(run.value?.output ?? ""));
const summary = computed(() => {
  const loaded = run.value;
  if (!loaded) return "";
  const outcome = loaded.timedOut
    ? "Timed out"
    : loaded.exitCode == null ? "Exit status unknown" : `Exit ${loaded.exitCode}`;
  const parts = [loaded.status === "succeeded" ? "Setup succeeded" : "Setup failed", outcome, `${loaded.durationMs} ms`];
  if (loaded.truncated) parts.push("output truncated");
  return parts.join(" · ");
});
function focusContent(): boolean {
  (output.value ?? body.value)?.focus({ preventScroll: true });
  return document.activeElement === (output.value ?? body.value);
}
defineExpose({ focusContent });
</script>
<template>
  <section ref="body" class="setup-log" aria-label="Read-only workspace setup output" data-testid="setup-log">
    <ol v-if="run && run.commands.length" class="commands">
      <li v-for="(command, index) in run.commands" :key="index"><code>{{ command }}</code></li>
    </ol>
    <pre ref="output" tabindex="0">{{ transcript }}</pre>
    <p role="status">{{ status || summary }}</p>
  </section>
</template>
<style scoped>
.setup-log { height: 100%; overflow: auto; padding: 12px; box-sizing: border-box; }
.commands { margin: 0 0 8px; padding-left: 18px; color: var(--kn-text-secondary); }
.commands code { font-family: monospace; }
pre { font-family: monospace; margin: 0; white-space: pre-wrap; }
p { color: var(--kn-text-muted); }
</style>
