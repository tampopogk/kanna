<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, watch } from "vue";
import { nextFrameOrTimeout } from "../utils/animationFrame";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import TerminalView from "./TerminalView.vue";
import { getTerminalTheme } from "../theme/theme";
import { useThemeRuntime } from "../theme/runtime";
import { registerE2ETerminalBuffer } from "../e2eTerminalBuffers";
import {
  DesktopServerRequestError,
  fetchDesktopTerminalArchive,
} from "../services/desktopServerClient";
import { getAppErrorMessage } from "../appError";

/**
 * One of a task's non-agent terminals: the startup shell a launch ran its
 * setup in, or a departing workspace's teardown.
 *
 * While the process is alive this attaches and never spawns. These sessions
 * belong to a launch the server started; a view that could create one would be
 * able to conjure a terminal for a launch that never happened, and a startup
 * shell that has exited is finished — re-running it is a rerun, not a repaint.
 *
 * Once it has exited there is nothing to attach to: the daemon drops the
 * session seconds after the process dies, and an attach-only view would sit in
 * a retry loop forever showing an error where the stage's startup output
 * should be. What it renders instead is the archive — the final frame the
 * daemon captured on its way out, kept with the task's durable record — as a
 * read-only terminal. A terminal that finished without one says so rather than
 * pretending to be readable.
 */
const props = defineProps<{
  taskId: string;
  sessionId: string;
  title: string;
  /**
   * Whether the process behind this terminal is still running.
   *
   * Undefined means *not yet known* — a restored tab remembers the launch's
   * session id but never its liveness, which would outlive the process it
   * described. Nothing attaches until the server has answered, because the
   * alternative is attaching to a session the daemon dropped long ago and
   * retrying forever.
   */
  live?: boolean;
  /**
   * Whether the server kept this terminal's final frame.
   *
   * Undefined means *unknown*, not "no": a tab opened by an agent through
   * `kanna_open_terminal` carries what the command said and nothing more, and
   * treating silence as "not kept" told the reader the output was gone while
   * the archive sat beside it. Unknown fetches, and a 404 is what decides.
   */
  archived?: boolean;
  /** The status it exited with, when it has exited. */
  exitCode?: number | null;
  active?: boolean;
}>();

const termRef = ref<InstanceType<typeof TerminalView> | null>(null);
const archiveEl = ref<HTMLElement | null>(null);
const archiveError = ref<string | null>(null);
const archiveMissing = ref(false);
const { effectiveCodeTheme } = useThemeRuntime();

let archiveTerminal: Terminal | null = null;
let archiveFitAddon: FitAddon | null = null;
let unregisterArchiveBuffer: (() => void) | null = null;
let renderedArchiveFor: string | null = null;
let archiveResizeObserver: ResizeObserver | null = null;

/**
 * Wait until the container has a settled, non-zero size.
 *
 * xterm measures its character cell when the terminal is opened, and a
 * terminal opened into a box that has not been laid out — a tab reconciliation
 * opened behind the one on screen, or the first frame of an already-active
 * one — measures zero and paints empty rows. `fit()` afterwards resizes the
 * buffer but does not re-measure, so the frame stayed invisible while its
 * bytes sat in the buffer. The live view solves this the same way before it
 * connects.
 */
async function waitForStableLayout(el: HTMLElement): Promise<void> {
  let last = { width: 0, height: 0 };
  for (let i = 0; i < 10; i++) {
    const current = { width: el.offsetWidth, height: el.offsetHeight };
    if (
      current.width > 0 &&
      current.height > 0 &&
      current.width === last.width &&
      current.height === last.height
    ) {
      return;
    }
    last = current;
    await nextFrameOrTimeout();
  }
}

/** A 404 from the archive route means the frame was not kept, not a failure. */
function isMissingArchiveError(error: unknown): boolean {
  return error instanceof DesktopServerRequestError && error.status === 404;
}

function disposeArchiveTerminal(): void {
  archiveResizeObserver?.disconnect();
  archiveResizeObserver = null;
  unregisterArchiveBuffer?.();
  unregisterArchiveBuffer = null;
  archiveTerminal?.dispose();
  archiveTerminal = null;
  archiveFitAddon = null;
  renderedArchiveFor = null;
}

async function renderArchive(): Promise<void> {
  const el = archiveEl.value;
  if (!el || renderedArchiveFor === props.sessionId) return;
  // Nothing is painted into a hidden tab: xterm would measure its cell against
  // a box with no size and render blank rows over a full buffer. A tab opened
  // behind the one on screen renders when the reader comes to it.
  if (props.active === false) return;
  disposeArchiveTerminal();
  archiveError.value = null;
  renderedArchiveFor = props.sessionId;

  let archive;
  try {
    archive = await fetchDesktopTerminalArchive(props.taskId, props.sessionId);
  } catch (error: unknown) {
    renderedArchiveFor = null;
    // A terminal that kept no frame answers 404, which is not a fault to
    // report — it is the answer, and the banner already says the output was
    // not kept.
    archiveMissing.value = isMissingArchiveError(error);
    archiveError.value = archiveMissing.value ? null : getAppErrorMessage(error);
    return;
  }
  archiveMissing.value = false;
  if (renderedArchiveFor !== props.sessionId || !archiveEl.value) return;

  const term = new Terminal({
    fontFamily: '"JetBrains Mono", "SF Mono", Menlo, monospace',
    fontSize: 13,
    lineHeight: 1,
    theme: getTerminalTheme(effectiveCodeTheme.value),
    scrollback: 10000,
    cursorBlink: false,
    // A retired terminal has no PTY behind it: nothing typed here could go
    // anywhere, and a caret would say otherwise.
    disableStdin: true,
    cursorStyle: "bar",
    cols: archive.cols,
    rows: archive.rows,
  });
  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);
  await nextTick();
  await waitForStableLayout(archiveEl.value);
  if (renderedArchiveFor !== props.sessionId || !archiveEl.value) {
    term.dispose();
    return;
  }
  term.open(archiveEl.value);
  term.write(archive.vt);
  archiveTerminal = term;
  archiveFitAddon = fitAddon;
  unregisterArchiveBuffer = registerE2ETerminalBuffer(props.sessionId, term);
  fitAddon.fit();
  // The panel is laid out by the tab area around it, so its size changes
  // without anything here being told; refitting keeps the frame filling it.
  archiveResizeObserver = new ResizeObserver(() => {
    if (archiveEl.value?.offsetWidth && archiveEl.value.offsetHeight) archiveFitAddon?.fit();
  });
  archiveResizeObserver.observe(archiveEl.value);
}

watch(effectiveCodeTheme, (theme) => {
  if (archiveTerminal) archiveTerminal.options.theme = getTerminalTheme(theme);
});

watch(
  [() => props.live, () => props.archived, () => props.sessionId, archiveEl],
  async () => {
    // `archived === false` is the one answer that means "there is nothing to
    // fetch"; undefined is unknown and asks the server.
    if (props.live !== false || props.archived === false) {
      disposeArchiveTerminal();
      return;
    }
    await renderArchive();
  },
  { immediate: true },
);

watch(
  () => props.active,
  async (active) => {
    if (!active) return;
    await nextTick();
    termRef.value?.fit?.();
    // A retired tab that was opened behind this one has nothing rendered yet.
    if (props.live === false && props.archived !== false) await renderArchive();
    archiveFitAddon?.fit();
  },
);

onBeforeUnmount(disposeArchiveTerminal);
</script>

<template>
  <div class="task-terminal-panel">
    <div v-if="live === false" class="task-terminal-status" data-testid="task-terminal-finished">
      <template v-if="archiveError">
        {{ $t('taskTerminal.archiveFailed', { title }) }} {{ archiveError }}
      </template>
      <template v-else-if="archived === false || archiveMissing">
        {{ $t('taskTerminal.noArchive', { title }) }}
      </template>
      <template v-else-if="exitCode === 0 || exitCode === null || exitCode === undefined">
        {{ $t('taskTerminal.finished', { title }) }}
      </template>
      <template v-else>
        {{ $t('taskTerminal.finishedWithStatus', { title, status: exitCode }) }}
      </template>
    </div>
    <div
      v-if="live === false"
      ref="archiveEl"
      class="task-terminal-archive"
      data-testid="task-terminal-archive"
    />
    <div v-else-if="live === undefined" class="task-terminal-status">
      {{ $t('common.loading') }}
    </div>
    <TerminalView
      v-else
      ref="termRef"
      :key="sessionId"
      :session-id="sessionId"
      :active="active !== false"
    />
  </div>
</template>

<style scoped>
.task-terminal-panel {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
}

.task-terminal-status {
  padding: 4px 10px;
  font-size: 12px;
  color: var(--kn-text-muted);
  border-bottom: 1px solid var(--kn-border);
}

.task-terminal-archive {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}
</style>
