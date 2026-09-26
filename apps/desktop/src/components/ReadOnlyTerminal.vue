<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { useThemeRuntime } from "../theme/runtime";
import { getTerminalTheme } from "../theme/theme";
import { registerE2ETerminalBuffer } from "../e2eTerminalBuffers";

const props = defineProps<{ output: string; identity: string }>();
const host = ref<HTMLElement | null>(null);
const { effectiveCodeTheme } = useThemeRuntime();
let terminal: Terminal | undefined;
let observer: ResizeObserver | undefined;
let unregister: (() => void) | undefined;
let written = "";
let writes = Promise.resolve();
let disposed = false;

function render(output: string) {
  writes = writes.then(async () => {
    const term = terminal;
    if (!term || disposed || output === written) return;
    const follow = term.buffer.active.viewportY >= term.buffer.active.baseY;
    // Reconnection or the output cap can replace a snapshot. Normally this
    // writes only the newly teed bytes, preserving selection and scrollback.
    const append = output.startsWith(written);
    if (!append) term.reset();
    const next = append ? output.slice(written.length) : output;
    await new Promise<void>(resolve => term.write(next, resolve));
    written = output;
    if (!disposed && follow) term.scrollToBottom();
  });
}
onMounted(() => {
  const term = new Terminal({
    disableStdin: true,
    cursorBlink: false,
    convertEol: true,
    fontFamily: '"JetBrains Mono", "SF Mono", Menlo, monospace',
    fontSize: 13,
    lineHeight: 1,
    scrollback: 10000,
    theme: getTerminalTheme(effectiveCodeTheme.value),
  });
  terminal = term;
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(host.value!);
  // No PTY connection, input listener, or backend resize is installed.
  term.attachCustomKeyEventHandler(event => {
    if (event.type === "keydown" && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "c" && term.hasSelection()) {
      void navigator.clipboard.writeText(term.getSelection());
      event.preventDefault();
      return false;
    }
    return true;
  });
  const resize = () => {
    if (host.value && host.value.clientWidth > 0 && host.value.clientHeight > 0) fit.fit();
  };
  observer = new ResizeObserver(resize);
  observer.observe(host.value!);
  resize();
  unregister = registerE2ETerminalBuffer(props.identity, term, () => fit.proposeDimensions());
  render(props.output);
});
watch(() => props.output, render);
watch(effectiveCodeTheme, theme => { if (terminal) terminal.options.theme = getTerminalTheme(theme); });
onBeforeUnmount(() => {
  disposed = true;
  observer?.disconnect();
  unregister?.();
  terminal?.dispose();
});
</script>
<template>
  <div ref="host" class="read-only-terminal" role="region" aria-label="Read-only creation terminal" data-testid="creation-terminal" />
</template>
<style scoped>
.read-only-terminal { height: 100%; min-height: 0; overflow: hidden; }
</style>
