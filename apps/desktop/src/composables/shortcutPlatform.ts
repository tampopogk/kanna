/**
 * How a shortcut is spelled on this platform, for dispatch and for display.
 *
 * The shortcut table is authored in macOS terms — `meta` is ⌘, and `display`
 * is the glyph string. On Linux both halves were simply wrong: nothing has a
 * Command key, so every ⌘ binding was unreachable by a person, while the hints
 * still said "⌘I". Dispatch and display have to move together or the app tells
 * you to press something that does nothing, which is why one module answers
 * both questions.
 *
 * The Linux mapping is not a glyph substitution. A terminal owns plain
 * `Ctrl+<letter>`: `Ctrl+C`, `Ctrl+D`, `Ctrl+Z`, `Ctrl+W`, and the rest of
 * readline. Mapping ⌘ to Ctrl the obvious way would have taken all of them
 * away from every agent session in the app. So:
 *
 * - `⌘X`  becomes `Ctrl+Shift+X` — the GNOME Terminal convention for exactly
 *   this reason.
 * - `⇧⌘X` becomes `Ctrl+Alt+X`, because its `Ctrl+Shift` form is taken by the
 *   unshifted binding above.
 * - Anything the desktop environment has already claimed, or that the mapping
 *   would put on a chord Linux spells differently, gets a named exception
 *   below rather than a silent collision.
 *
 * An exception is a whole binding, not a modifier swap. Some actions are the
 * *same action* under a different key on Linux: history is Alt+←/→ there, not
 * a Ctrl-punctuation chord, and tab cycling is Ctrl+Page Up/Down rather than
 * bracket keys. A table that could only move modifiers had to leave those on
 * chords that read as macOS habits transliterated, which is how the owner's
 * test drive found a listed hint sitting on a dead chord. So an exception may
 * also replace the key, and may declare that its Linux form must be *listed*
 * where the macOS one is a convention too well known to advertise.
 */
export type ShortcutPlatform = "mac" | "linux"

export interface ShortcutModifiers {
  meta?: boolean
  shift?: boolean
  alt?: boolean
  ctrl?: boolean
}

export interface ShortcutBinding extends ShortcutModifiers {
  /** Keys matched against `KeyboardEvent.key`. */
  key: string | string[]
  /** Physical codes, for keys whose `key` a modifier rewrites. */
  code?: string | string[]
  /** What the hints say. */
  display: string
  /** Whether the shortcuts modal leaves this one out, on this platform. */
  hidden: boolean
}

export function resolveShortcutPlatform(
  platform: string | undefined = typeof navigator === "undefined" ? undefined : navigator.platform,
): ShortcutPlatform {
  return /mac|iphone|ipad/i.test(platform ?? "") ? "mac" : "linux"
}

const MODIFIER_LABELS: Record<ShortcutPlatform, { ctrl: string; shift: string; alt: string; meta: string }> = {
  mac: { ctrl: "⌃", shift: "⇧", alt: "⌥", meta: "⌘" },
  linux: { ctrl: "Ctrl", shift: "Shift", alt: "Alt", meta: "Super" },
}

/** Modifier tokens a display string can start with, for splitting hints into keys. */
export function shortcutModifierTokens(platform: ShortcutPlatform): string[] {
  const labels = MODIFIER_LABELS[platform]
  return [labels.shift, labels.ctrl, labels.alt, labels.meta]
}

/**
 * US-layout shifted punctuation, so a hint can name the key you press rather
 * than the character Shift produces. `["_", "-"]` is one key, and "Ctrl+Shift+-"
 * is what a person can act on; "Ctrl+Shift+_" is not.
 */
const UNSHIFTED_PUNCTUATION: Record<string, string> = {
  _: "-",
  "{": "[",
  "}": "]",
  ":": ";",
  '"': "'",
  "<": ",",
  ">": ".",
  "?": "/",
  "|": "\\",
  "+": "=",
  "~": "`",
}

/** The key a hint should name, out of everything the binding matches. */
function preferredKey(keys: string[]): string {
  const letter = keys.find((key) => /^[A-Za-z]$/.test(key))
  if (letter) return letter
  const unshifted = keys.filter((key) => !(key in UNSHIFTED_PUNCTUATION && keys.includes(UNSHIFTED_PUNCTUATION[key]!)))
  return unshifted[0] ?? keys[0] ?? ""
}

/** The key name a hint shows, given the `KeyboardEvent.key` values it matches. */
function displayKey(key: string | string[], platform: ShortcutPlatform): string {
  const first = Array.isArray(key) ? preferredKey(key) : key
  const named: Record<string, string> = {
    ArrowUp: "↑",
    ArrowDown: "↓",
    ArrowLeft: "←",
    ArrowRight: "→",
    Backspace: platform === "mac" ? "⌫" : "Backspace",
    Escape: "Escape",
    PageUp: "Page Up",
    PageDown: "Page Down",
  }
  return named[first] ?? (first.length === 1 ? first.toUpperCase() : first)
}

function renderDisplay(binding: ShortcutModifiers & { key: string | string[] }, platform: ShortcutPlatform): string {
  const labels = MODIFIER_LABELS[platform]
  const parts: string[] = []
  // macOS orders modifiers ⌃⌥⇧⌘; Linux writes them Ctrl+Alt+Shift+Super.
  if (binding.ctrl) parts.push(labels.ctrl)
  if (binding.alt) parts.push(labels.alt)
  if (binding.shift) parts.push(labels.shift)
  if (binding.meta) parts.push(labels.meta)
  parts.push(displayKey(binding.key, platform))
  return platform === "mac" ? parts.join("") : parts.join("+")
}

/** What a Linux exception may replace: the chord, the key, and whether it is listed. */
export interface LinuxException extends ShortcutModifiers {
  /**
   * The Linux key, when the action belongs on a different key there and not
   * merely under different modifiers.
   */
  key?: string | string[]
  code?: string | string[]
  /**
   * List this one in the shortcuts modal even though the authored entry is
   * hidden. For a chord that is a platform convention on macOS and an
   * invention on Linux, hiding it is the difference between "everyone already
   * knows this" and "nobody can find it".
   */
  listed?: boolean
}

/**
 * Linux bindings that the systematic mapping would put somewhere already
 * taken, or somewhere it should not go. Keyed by action name.
 *
 * Every chord here was checked against a stock GNOME/Ubuntu desktop's own
 * bindings — `org.gnome.desktop.wm.keybindings`, `org.gnome.mutter`,
 * `org.gnome.settings-daemon.plugins.media-keys` and `org.freedesktop.ibus.*`.
 * The input method counts: IBus is on by default and takes its chords before
 * any application sees them.
 */
const LINUX_EXCEPTIONS: Record<string, LinuxException> = {
  // Ctrl+Shift+I is WebKitGTK's built-in inspector shortcut. Let the native
  // webview keep it instead of opening both devtools and Create Repository.
  createRepo: { ctrl: true, alt: true, shift: true },
  // Vertical arrows are the most contested chords on this desktop, and the
  // three obvious candidates are all gone. Ctrl+Alt+↑/↓ switches GNOME
  // workspaces. Alt+↑/↓ and Ctrl+Shift+↑/↓ are taken by something above the
  // app that `gsettings` does not name: injecting them below the compositor on
  // a stock Ubuntu 26.04 GNOME/Wayland session delivers the modifier keydowns
  // to the webview and never the arrow (see `tests/e2e/linux/`). Ctrl+↑/↓
  // arrives intact and nothing else claims it, so task navigation takes it —
  // the closest surviving transform of ⌥⌘↑/↓, and a chord that still works
  // from the sidebar search field, because it selects nothing and so is not
  // conceded by `belongsToTextEditing`.
  navigateUp: { ctrl: true },
  navigateDown: { ctrl: true },
  // Ctrl+Alt+←/→ is `switch-to-workspace-left`/`-right`, and
  // Ctrl+Alt+Shift+←/→ is `move-to-workspace-*`. Alt+←/→ is history, which is
  // what goBack/goForward below take, so panes keep the free Ctrl+Shift pair.
  previousPane: { ctrl: true, shift: true },
  nextPane: { ctrl: true, shift: true },
  // Repo navigation keeps the Shift tier its ⇧⌘↑/↓ authoring gives it, one
  // modifier over task navigation, on the other arrow chord measured to reach
  // the webview: Alt+Shift+↑/↓. Ctrl+Shift+↑/↓ reads as the natural pair with
  // the panes' Ctrl+Shift+←/→ and is exactly the chord the desktop eats — and
  // Ctrl+Shift+arrow is word selection, which `belongsToTextEditing` concedes
  // to the composer the caret usually sits in. Alt+Shift+arrow selects nothing
  // in a GTK field, so it survives there.
  navigateRepoUp: { alt: true, shift: true },
  navigateRepoDown: { alt: true, shift: true },
  // ⌥⌘P would land on Ctrl+Alt+P, which ⇧⌘P (the command palette) already has.
  toggleFilePreview: { ctrl: true, alt: true, shift: true },
  // Ctrl+Alt+Backspace is the historical "kill the X server" chord. Disabled by
  // default on modern systems, but not something to bind an app action to.
  // Nothing else claims Ctrl+Shift+Backspace, because no ⌘⌫ shortcut exists.
  closeTask: { ctrl: true, shift: true },
  // ⇧⌘[ / ⇧⌘] is a macOS convention so widespread it needs no advertising, and
  // its Ctrl+Alt transliteration is nothing at all: an owner looking for tab
  // navigation on Linux read the listed Ctrl+Shift+←/→ pane hint as tab
  // navigation and reported it dead. Ctrl+Page Up / Ctrl+Page Down is what
  // every Linux browser, editor and terminal uses, GNOME claims neither, and
  // being listed is the point.
  prevTab: { ctrl: true, key: "PageUp", listed: true },
  nextTab: { ctrl: true, key: "PageDown", listed: true },
  // ⌃- / ⌃⇧- carry straight across as Ctrl+- / Ctrl+Shift+-, which on Linux is
  // zoom out and zoom in — the app would own both and the desktop would have
  // no zoom left. History on Linux is Alt+←/→, unclaimed by GNOME and the
  // chord a person already reaches for.
  goBack: { alt: true, key: "ArrowLeft" },
  goForward: { alt: true, key: "ArrowRight" },
  // Ctrl+Shift+U is IBus' Unicode code-point entry
  // (`org.freedesktop.ibus.panel.emoji unicode-hotkey`), on by default. The
  // input method consumes it, so the owner saw a literal "u" appear instead of
  // the app moving; no application can win it. Both unread jumps move up one
  // tier, keeping the mnemonic and their "one repo / all repos" ordering.
  goToOldestUnread: { ctrl: true, alt: true },
  goToOldestUnreadAllRepos: { ctrl: true, alt: true, shift: true },
}

/** The named Linux exception for an action, if it has one. */
export function linuxException(action: string): LinuxException | undefined {
  return LINUX_EXCEPTIONS[action]
}

/** The modifiers this action actually dispatches on, for this platform. */
export function platformModifiers(
  action: string,
  authored: ShortcutModifiers,
  platform: ShortcutPlatform,
): ShortcutModifiers {
  if (platform === "mac") {
    return {
      meta: authored.meta ?? false,
      shift: authored.shift ?? false,
      alt: authored.alt ?? false,
      ctrl: authored.ctrl ?? false,
    }
  }
  const exception = linuxException(action)
  if (exception) {
    return {
      meta: exception.meta ?? false,
      shift: exception.shift ?? false,
      alt: exception.alt ?? false,
      ctrl: exception.ctrl ?? false,
    }
  }
  if (!authored.meta) {
    // Already a Ctrl-only or bare binding; nothing to remap.
    return {
      meta: false,
      shift: authored.shift ?? false,
      alt: authored.alt ?? false,
      ctrl: authored.ctrl ?? false,
    }
  }
  return {
    meta: false,
    ctrl: true,
    shift: !authored.shift,
    alt: Boolean(authored.shift),
  }
}

/** The full binding — modifiers, key and hint — for this platform. */
export function platformBinding(
  action: string,
  authored: ShortcutModifiers & {
    key: string | string[]
    code?: string | string[]
    display: string
    hidden?: boolean
  },
  platform: ShortcutPlatform,
): ShortcutBinding {
  const modifiers = platformModifiers(action, authored, platform)
  const exception = platform === "linux" ? linuxException(action) : undefined
  const key = exception?.key ?? authored.key
  // An exception that moves the key carries its own code, or none: the
  // authored code belongs to the authored key, and `KeyP` left behind on a
  // binding that now matches `PageUp` would fire the action on the wrong key.
  const code = exception?.key ? exception.code : authored.code
  return {
    ...modifiers,
    key,
    code,
    // macOS keeps its authored glyph string byte for byte; every existing
    // expectation is written against it.
    display: platform === "mac" ? authored.display : renderDisplay({ ...modifiers, key }, platform),
    hidden: exception?.listed ? false : (authored.hidden ?? false),
  }
}

/**
 * The clipboard chord a terminal should act on, or `null` for a key that
 * belongs to the PTY.
 *
 * macOS can use ⌘C/⌘V because the terminal has no claim on the Command key.
 * Linux cannot: plain `Ctrl+C` is SIGINT and plain `Ctrl+V` is readline's
 * quoted-insert, so taking either would break the agent sessions the app
 * exists to run. `Ctrl+Shift+C` / `Ctrl+Shift+V` is what GNOME Terminal, and
 * every other Linux terminal, settled on for the same reason.
 */
export function terminalClipboardAction(
  event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey">,
  platform: ShortcutPlatform,
): "copy" | "paste" | null {
  const key = event.key.toLowerCase()
  if (key !== "c" && key !== "v") return null
  const wanted =
    platform === "mac"
      ? event.metaKey && !event.altKey && !event.ctrlKey
      : event.ctrlKey && event.shiftKey && !event.altKey && !event.metaKey
  if (!wanted) return null
  return key === "c" ? "copy" : "paste"
}

/**
 * Keys that move the caret. Held with Shift they mean "extend the selection"
 * in every text field on every platform, which is the one thing an app-level
 * shortcut must never take.
 *
 * The deletion keys are deliberately absent: Shift+Backspace and Shift+Delete
 * extend nothing, and conceding them would hand a focused field the listed
 * `closeTask` chord (⇧⌘⌫ / Ctrl+Shift+Backspace), which is exactly the kind of
 * silent no-op this guard exists to prevent.
 */
const TEXT_NAVIGATION_KEYS = new Set([
  "ArrowLeft",
  "ArrowRight",
  "ArrowUp",
  "ArrowDown",
  "Home",
  "End",
  "PageUp",
  "PageDown",
])

/** Input types that hold text a person edits, as opposed to a widget. */
const EDITABLE_INPUT_TYPES = new Set([
  "text",
  "search",
  "email",
  "url",
  "tel",
  "password",
  "number",
  "",
])

/**
 * Whether this element is somewhere a person is editing text.
 *
 * xterm's hidden helper textarea is deliberately not one. It is an editable
 * element in the DOM sense only — what is typed into it goes to the PTY, there
 * is no selection to protect, and navigating tasks from a focused agent
 * terminal has to keep working.
 */
export function isEditableElement(target: EventTarget | null): boolean {
  if (typeof HTMLElement === "undefined" || !(target instanceof HTMLElement)) return false
  if (target.classList.contains("xterm-helper-textarea")) return false
  if (target.isContentEditable) return true
  if (typeof HTMLTextAreaElement !== "undefined" && target instanceof HTMLTextAreaElement) {
    return !target.readOnly && !target.disabled
  }
  if (typeof HTMLInputElement !== "undefined" && target instanceof HTMLInputElement) {
    return EDITABLE_INPUT_TYPES.has(target.type) && !target.readOnly && !target.disabled
  }
  return false
}

/**
 * Whether this keystroke belongs to the text field it landed in rather than to
 * the app.
 *
 * The global handler listens in the capture phase and calls `preventDefault()`,
 * so anything it claims is gone from every input, textarea and contenteditable
 * in the app. On Linux that took Ctrl+Shift+←/→ — *the* word-selection chord —
 * away from the task search field and every other one: Shift+← still selected a
 * character and Ctrl+Shift+← silently did nothing at all.
 *
 * Only selection chords are conceded, not caret motion and not the whole app
 * tier. Ctrl+Shift+S has no meaning inside a text field and should still
 * advance a stage from one. Plain Ctrl+↑/↓ is deliberately *not* conceded
 * either, even though GTK moves the caret by paragraph with it: that is task
 * navigation on Linux, and typing a few characters into the sidebar search and
 * then walking the results is the whole point of the field. Nothing is
 * selected, so nothing is lost.
 *
 * Which Shift chords a field genuinely owns is a platform question, and
 * conceding one it does not own is not free. The agent view's composer
 * textarea holds the caret nearly all the time, so a chord conceded here is a
 * chord that does nothing in the view a person spends the day in — which is
 * how repo navigation was measured dead on Linux even after it moved to a
 * chord the desktop delivers. A GTK or WebKit text field builds a selection
 * out of Shift and Ctrl only; Alt is the mnemonic modifier and Super belongs
 * to the desktop, and neither extends a selection. macOS is different — ⌥⇧←
 * is word-wise selection and ⇧⌘↑ selects to the top — so it concedes every
 * Shift+caret chord, exactly as it always has.
 */
export function belongsToTextEditing(
  event: Pick<KeyboardEvent, "key" | "shiftKey" | "altKey" | "metaKey">,
  target: EventTarget | null,
  platform: ShortcutPlatform = resolveShortcutPlatform(),
): boolean {
  if (!event.shiftKey || !TEXT_NAVIGATION_KEYS.has(event.key)) return false
  if (platform === "linux" && (event.altKey || event.metaKey)) return false
  return isEditableElement(target)
}

/**
 * A hint for a binding a component dispatches on `metaKey || ctrlKey` itself —
 * an in-view find, say. Those already work on Linux; only their labels said ⌘.
 */
export function metaOrControlHint(key: string, platform: ShortcutPlatform = resolveShortcutPlatform()): string {
  const display = key.length === 1 ? key.toUpperCase() : key
  return platform === "mac" ? `⌘${display}` : `Ctrl+${display}`
}
