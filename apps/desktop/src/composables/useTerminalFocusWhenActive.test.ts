// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { effectScope, nextTick } from "vue";
import { refocusActiveTerminal, useTerminalFocusWhenActive } from "./useTerminalFocusWhenActive";

const setWebviewFocusMock = vi.fn(async () => {});

vi.mock("../tauri-mock", () => ({
  isTauri: true,
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    setFocus: setWebviewFocusMock,
  }),
}));

describe("useTerminalFocusWhenActive", () => {
  const originalRequestAnimationFrame = globalThis.requestAnimationFrame;
  const originalCancelAnimationFrame = globalThis.cancelAnimationFrame;

  beforeEach(() => {
    setWebviewFocusMock.mockClear();
    globalThis.requestAnimationFrame = (callback: FrameRequestCallback) => {
      callback(0);
      return 1;
    };
    globalThis.cancelAnimationFrame = vi.fn();
  });

  afterEach(() => {
    vi.useRealTimers();
    document.body.innerHTML = "";
    globalThis.requestAnimationFrame = originalRequestAnimationFrame;
    globalThis.cancelAnimationFrame = originalCancelAnimationFrame;
  });

  it("restores native focus before focusing an active terminal", async () => {
    const focus = vi.fn();
    const { focusWhenActive } = useTerminalFocusWhenActive({
      isActive: () => true,
      getTerminal: () => ({ focus }),
    });

    await focusWhenActive();
    await nextTick();

    expect(setWebviewFocusMock).toHaveBeenCalledTimes(1);
    expect(focus).toHaveBeenCalledTimes(1);
    expect(setWebviewFocusMock.mock.invocationCallOrder[0]).toBeLessThan(
      focus.mock.invocationCallOrder[0] ?? Number.POSITIVE_INFINITY,
    );
  });

  it("preserves focus in sidebar search and rename inputs", async () => {
    const sidebar = document.createElement("aside");
    sidebar.className = "sidebar";
    const input = document.createElement("input");
    sidebar.appendChild(input);
    document.body.appendChild(sidebar);
    input.focus();
    const focus = vi.fn();
    const { focusWhenActive } = useTerminalFocusWhenActive({
      isActive: () => true,
      getTerminal: () => ({ focus }),
    });

    await focusWhenActive();

    expect(document.activeElement).toBe(input);
    expect(setWebviewFocusMock).not.toHaveBeenCalled();
    expect(focus).not.toHaveBeenCalled();
  });

  it("focuses through the bounded fallback when animation frames are suspended", async () => {
    vi.useFakeTimers();
    globalThis.requestAnimationFrame = vi.fn(() => 7);
    const focus = vi.fn();
    const { focusWhenActive } = useTerminalFocusWhenActive({
      isActive: () => true,
      getTerminal: () => ({ focus }),
    });

    const pendingFocus = focusWhenActive();
    await nextTick();
    await vi.advanceTimersByTimeAsync(50);
    await pendingFocus;

    expect(focus).toHaveBeenCalledTimes(1);
  });
});

async function flushFocusChain(): Promise<void> {
  // The request awaits a tick, the native webview focus and an animation frame.
  for (let attempt = 0; attempt < 10; attempt++) {
    await nextTick();
  }
}

describe("refocusActiveTerminal", () => {
  const originalRequestAnimationFrame = globalThis.requestAnimationFrame;

  beforeEach(() => {
    setWebviewFocusMock.mockClear();
    globalThis.requestAnimationFrame = (callback: FrameRequestCallback) => {
      callback(0);
      return 1;
    };
  });

  afterEach(() => {
    document.body.innerHTML = "";
    globalThis.requestAnimationFrame = originalRequestAnimationFrame;
  });

  it("asks a live terminal for focus again, and only the active one", async () => {
    const activeFocus = vi.fn();
    const inactiveFocus = vi.fn();
    const scope = effectScope();
    scope.run(() => {
      useTerminalFocusWhenActive({ isActive: () => true, getTerminal: () => ({ focus: activeFocus }) });
      useTerminalFocusWhenActive({ isActive: () => false, getTerminal: () => ({ focus: inactiveFocus }) });
    });

    refocusActiveTerminal();
    await flushFocusChain();

    // A terminal that asked for focus behind `inert` gets asked again once the
    // startup screen lifts; a background tab still does not steal the caret.
    expect(activeFocus).toHaveBeenCalledTimes(1);
    expect(inactiveFocus).not.toHaveBeenCalled();

    scope.stop();
  });

  it("keeps the terminal's own modal and sidebar rules when asked again", async () => {
    const focus = vi.fn();
    const overlay = document.createElement("div");
    overlay.className = "modal-overlay";
    document.body.appendChild(overlay);
    const scope = effectScope();
    scope.run(() => {
      useTerminalFocusWhenActive({ isActive: () => true, getTerminal: () => ({ focus }) });
    });

    refocusActiveTerminal();
    await flushFocusChain();

    expect(focus).not.toHaveBeenCalled();

    scope.stop();
  });

  it("forgets a terminal's request when its scope is torn down", async () => {
    const focus = vi.fn();
    const scope = effectScope();
    scope.run(() => {
      useTerminalFocusWhenActive({ isActive: () => true, getTerminal: () => ({ focus }) });
    });
    scope.stop();

    refocusActiveTerminal();
    await flushFocusChain();

    expect(focus).not.toHaveBeenCalled();
  });
});
