import { getCurrentScope, nextTick, onScopeDispose } from "vue";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isTauri } from "../tauri-mock";
import { nextFrameOrTimeout } from "../utils/animationFrame";

interface FocusableTerminal {
  focus(): void;
}

interface TerminalFocusWhenActiveOptions {
  isActive: () => boolean;
  getTerminal: () => FocusableTerminal | null;
}

function shouldPreserveCurrentFocus(): boolean {
  if (document.querySelector(".modal-overlay")) return true;
  const activeElement = document.activeElement;
  return activeElement instanceof HTMLElement
    && activeElement.matches(".sidebar input, .sidebar textarea");
}

async function restoreNativeWebviewFocus(): Promise<void> {
  if (!isTauri) return;
  try {
    await getCurrentWebview().setFocus();
  } catch (error: unknown) {
    console.warn("[terminal] failed to restore native webview focus:", error);
  }
}

/**
 * Every live terminal's own focus request. A terminal that asked for focus
 * while an ancestor was `inert` — which is what the startup screen makes of the
 * workspace behind it — never got it, and nothing about that terminal changes
 * when the screen lifts: it does not remount, and its `active` prop does not
 * move. The readiness edge therefore has to ask again. Each request still
 * guards on its own `isActive()` and on the modal/sidebar rules below, so
 * asking all of them focuses at most the active one, and only when nothing
 * else has a better claim on the caret.
 */
const terminalFocusRequests = new Set<() => Promise<void>>();

/** Re-run every live terminal's focus request. Call after `inert` is gone. */
export function refocusActiveTerminal(): void {
  for (const request of terminalFocusRequests) {
    void request();
  }
}

export function useTerminalFocusWhenActive({
  isActive,
  getTerminal,
}: TerminalFocusWhenActiveOptions) {
  let focusGeneration = 0;

  function cancelPendingFocus(): void {
    focusGeneration += 1;
  }

  async function focusWhenActive(): Promise<void> {
    const generation = ++focusGeneration;
    if (!isActive() || !getTerminal() || shouldPreserveCurrentFocus()) return;
    await nextTick();
    if (
      generation !== focusGeneration
      || !isActive()
      || !getTerminal()
      || shouldPreserveCurrentFocus()
    ) return;
    await restoreNativeWebviewFocus();
    await nextFrameOrTimeout();
    const terminal = getTerminal();
    if (
      generation !== focusGeneration
      || !isActive()
      || !terminal
      || shouldPreserveCurrentFocus()
    ) return;
    terminal.focus();
  }

  // Registration is tied to the owning scope, so a terminal's request leaves
  // the set with the terminal. A caller with no scope registers nothing rather
  // than leaking a request that outlives whatever created it.
  if (getCurrentScope()) {
    terminalFocusRequests.add(focusWhenActive);
    onScopeDispose(() => {
      terminalFocusRequests.delete(focusWhenActive);
    });
  }

  return {
    cancelPendingFocus,
    focusWhenActive,
  };
}
