/**
 * Real key events, injected below the compositor.
 *
 * Every other keyboard helper in this tree builds a `KeyboardEvent` in the
 * page and dispatches it. That proves the app's own handler wiring and nothing
 * else — it was green while three Linux chords were dead on a real desktop,
 * because the chords never reached the webview at all: IBus takes Ctrl+Shift+U
 * for Unicode entry, and GNOME takes Ctrl+Alt+Arrow for workspaces, both
 * before any application sees a key.
 *
 * `ydotool` writes to `/dev/uinput`, so what it sends enters the same kernel
 * input pipeline a keyboard does and travels the whole way up: kernel →
 * compositor → IBus → GTK → WebKit → the page. A chord that something up that
 * chain has claimed simply never arrives, which is exactly the failure this
 * lane exists to catch.
 *
 * It follows that this injects into whatever session owns the seat, not into
 * the test process — the window under test must be focused, and the lane
 * checks that rather than assuming it (see `linux/README.md`).
 */
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const run = promisify(execFile);

/**
 * Linux input event codes, from `linux/input-event-codes.h`.
 *
 * `ydotool key` speaks these, not key names: a code is the physical key, which
 * is the level injection has to happen at. The names on the left are
 * `KeyboardEvent.key` / `KeyboardEvent.code` spellings so a chord in a test
 * reads like the chord in the shortcuts table.
 */
const KEY_CODES: Record<string, number> = {
  Escape: 1,
  Backspace: 14,
  Tab: 15,
  Enter: 28,
  Space: 57,
  Minus: 12,
  Equal: 13,
  BracketLeft: 26,
  BracketRight: 27,
  Semicolon: 39,
  Quote: 40,
  Backquote: 41,
  Backslash: 43,
  Comma: 51,
  Period: 52,
  Slash: 53,
  ArrowUp: 103,
  ArrowLeft: 105,
  ArrowRight: 106,
  ArrowDown: 108,
  Home: 102,
  End: 107,
  PageUp: 104,
  PageDown: 109,
  Insert: 110,
  Delete: 111,
  a: 30, b: 48, c: 46, d: 32, e: 18, f: 33, g: 34, h: 35, i: 23,
  j: 36, k: 37, l: 38, m: 50, n: 49, o: 24, p: 25, q: 16, r: 19,
  s: 31, t: 20, u: 22, v: 47, w: 17, x: 45, y: 21, z: 44,
  "1": 2, "2": 3, "3": 4, "4": 5, "5": 6, "6": 7, "7": 8, "8": 9, "9": 10, "0": 11,
};

const MODIFIER_CODES: Record<string, number> = {
  Ctrl: 29,
  Control: 29,
  Shift: 42,
  Alt: 56,
  Super: 125,
  Meta: 125,
};

export interface RealKeyboardStatus {
  usable: boolean;
  /** Why not, in a sentence a person reading a skipped lane can act on. */
  reason: string;
}

/**
 * The keycode sequence for one chord, as `ydotool key` arguments.
 *
 * Modifiers go down in the order written and come up in reverse, which is what
 * a person's hand does and what the compositor's own grab matching expects.
 */
export function chordToKeyArgs(chord: string): string[] {
  const parts = chord.split("+").map((part) => part.trim()).filter(Boolean);
  if (parts.length === 0) throw new Error(`empty chord: ${JSON.stringify(chord)}`);
  const keyName = parts[parts.length - 1];
  const modifiers = parts.slice(0, -1);

  const keyCode = KEY_CODES[keyName] ?? KEY_CODES[keyName.toLowerCase()];
  if (keyCode === undefined) throw new Error(`no Linux keycode for ${JSON.stringify(keyName)} in ${chord}`);

  const modifierCodes = modifiers.map((modifier) => {
    const code = MODIFIER_CODES[modifier];
    if (code === undefined) throw new Error(`no Linux keycode for modifier ${JSON.stringify(modifier)} in ${chord}`);
    return code;
  });

  return [
    ...modifierCodes.map((code) => `${code}:1`),
    `${keyCode}:1`,
    `${keyCode}:0`,
    ...[...modifierCodes].reverse().map((code) => `${code}:0`),
  ];
}

async function ydotool(args: string[]): Promise<void> {
  await run("ydotool", args, {
    env: process.env,
    timeout: 10_000,
  });
}

/**
 * Whether this machine can inject real keys at all.
 *
 * A lane that cannot must skip with a reason, never pass: evidence that says
 * nothing is worse here than no evidence, because the bug it is looking for is
 * invisible to every other layer.
 */
export async function inspectRealKeyboard(): Promise<RealKeyboardStatus> {
  if (process.platform !== "linux") {
    return { usable: false, reason: `real key injection is Linux-only; this host is ${process.platform}` };
  }
  try {
    // One probe, not two. Shift alone, pressed and released: it reaches the
    // socket or it does not, and it cannot disturb anything if it does.
    // `ydotool --version` looks like the cheaper presence check and is not one
    // — Ubuntu's build answers it with "Unknown command" and exit 1, so asking
    // reports a missing binary on a machine where injection works perfectly.
    await ydotool(["key", "42:1", "42:0"]);
  } catch (error) {
    // A spawn that never found the binary fails with ENOENT; anything else
    // ran it and it refused, which on this tool means the daemon.
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      return { usable: false, reason: "ydotool is not on PATH (apt install ydotool)" };
    }
    const detail = error instanceof Error ? error.message : String(error);
    return {
      usable: false,
      reason:
        "ydotoold is not reachable — start it with access to /dev/uinput and export YDOTOOL_SOCKET " +
        `to the socket it created (${detail.split("\n")[0]})`,
    };
  }
  return { usable: true, reason: "" };
}

/** Press one chord for real, e.g. `Ctrl+Shift+ArrowLeft`. */
export async function pressRealChord(chord: string): Promise<void> {
  await ydotool(["key", ...chordToKeyArgs(chord)]);
}

/** Type literal text for real, one keystroke per character. */
export async function typeRealText(text: string): Promise<void> {
  await ydotool(["type", "--key-delay", "12", "--", text]);
}

/**
 * Dismiss whatever holds the keyboard grab, without going near an application.
 *
 * A GNOME overview or panel menu takes the keyboard from every window, and
 * while one is open nothing injected reaches the app. Escape closes it. This is
 * only ever sent while the window under test does *not* have focus, so it
 * cannot reach a dialog the lane itself opened.
 */
export async function dismissDesktopGrab(): Promise<void> {
  await pressRealChord("Escape");
}

/**
 * Move the desktop's focus to the next window.
 *
 * Wayland deliberately has no "activate that window" call, and a client cannot
 * raise itself: `set_focus` returns `ok` and changes nothing. Clicking is no
 * better, because a Wayland client is not told where it is on screen — the
 * window under test here reports its rect as `0,0`, so a click computed from it
 * lands on the desktop's own top panel. What is left is the switcher the
 * desktop does own, pressed until the window under test answers that it has the
 * keyboard. The caller checks after every step; this only takes one.
 */
export async function focusNextWindow(): Promise<void> {
  await pressRealChord("Alt+Tab");
}
