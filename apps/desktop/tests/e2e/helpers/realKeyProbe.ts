/**
 * What the webview actually received.
 *
 * Injecting a real chord (see `realKeys.ts`) answers half the question. The
 * other half is which of two different bugs you are looking at when nothing
 * happens:
 *
 * - the chord never arrived, because the compositor or the input method took
 *   it first — no amount of handler work can fix that, the binding has to move;
 * - the chord arrived and the app did nothing with it, because the binding is
 *   wrong.
 *
 * The owner hit both at once and could only report "it doesn't work". This
 * probe separates them by recording every keydown the page sees, in the capture
 * phase, alongside whether the app's own handler claimed it.
 */

export interface ObservedKey {
  key: string;
  code: string;
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  meta: boolean;
  /** Whether something claimed it — the app's handler runs before this probe. */
  defaultPrevented: boolean;
  /** Where it landed, as `tagName.class`, so a terminal is distinguishable. */
  target: string;
}

/**
 * The app's global handler registers on `window` in the capture phase during
 * startup, so a listener installed later in the same phase on the same target
 * runs after it — which is what makes `defaultPrevented` meaningful here.
 */
export const INSTALL_KEY_PROBE_SCRIPT = `(() => {
  const existing = window.__KANNA_REAL_KEY_PROBE__;
  if (existing) window.removeEventListener("keydown", existing.listener, true);
  const observed = [];
  const listener = (event) => {
    const target = event.target;
    const tag = target && target.tagName ? target.tagName.toLowerCase() : "";
    const cls = target && target.className && typeof target.className === "string"
      ? "." + target.className.trim().split(/\\s+/).join(".")
      : "";
    observed.push({
      key: event.key,
      code: event.code,
      ctrl: event.ctrlKey,
      shift: event.shiftKey,
      alt: event.altKey,
      meta: event.metaKey,
      defaultPrevented: event.defaultPrevented,
      target: tag + cls,
    });
  };
  window.__KANNA_REAL_KEY_PROBE__ = { observed, listener };
  window.addEventListener("keydown", listener, true);
  return true;
})()`;

export const READ_KEY_PROBE_SCRIPT = `(() => {
  const probe = window.__KANNA_REAL_KEY_PROBE__;
  return probe ? probe.observed : null;
})()`;

export const CLEAR_KEY_PROBE_SCRIPT = `(() => {
  const probe = window.__KANNA_REAL_KEY_PROBE__;
  if (probe) probe.observed.length = 0;
  return true;
})()`;

export const REMOVE_KEY_PROBE_SCRIPT = `(() => {
  const probe = window.__KANNA_REAL_KEY_PROBE__;
  if (probe) window.removeEventListener("keydown", probe.listener, true);
  window.__KANNA_REAL_KEY_PROBE__ = undefined;
  return true;
})()`;

/** Whether one observed keydown is the chord that was injected. */
export function matchesChord(observed: ObservedKey, chord: string): boolean {
  const parts = chord.split("+").map((part) => part.trim()).filter(Boolean);
  const keyName = parts[parts.length - 1];
  const modifiers = new Set(parts.slice(0, -1));
  if (observed.ctrl !== modifiers.has("Ctrl")) return false;
  if (observed.shift !== modifiers.has("Shift")) return false;
  if (observed.alt !== modifiers.has("Alt")) return false;
  if (observed.meta !== (modifiers.has("Super") || modifiers.has("Meta"))) return false;
  // Matched on `code`, not `key`: Shift rewrites `key` for letters and
  // punctuation ("u" becomes "U", "/" becomes "?"), and the injected chord
  // names the physical key.
  return observed.code === physicalCode(keyName);
}

/** The `KeyboardEvent.code` a physical key name produces. */
export function physicalCode(keyName: string): string {
  if (/^[A-Za-z]$/.test(keyName)) return `Key${keyName.toUpperCase()}`;
  if (/^[0-9]$/.test(keyName)) return `Digit${keyName}`;
  return keyName;
}
