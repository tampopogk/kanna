import { describe, expect, it } from "vitest"
import { bindingsFor, secondaryBindingsFor, shortcuts } from "./useKeyboardShortcuts"
import {
  belongsToTerminalInput,
  belongsToTextEditing,
  isEditableElement,
  linuxException,
  metaOrControlHint,
  platformModifiers,
  resolveShortcutPlatform,
  secondaryBinding,
  shortcutModifierTokens,
  terminalClipboardAction,
} from "./shortcutPlatform"

const mac = bindingsFor("mac")
const linux = bindingsFor("linux")

function chord(binding: { ctrl?: boolean; shift?: boolean; alt?: boolean; meta?: boolean }): string {
  return [
    binding.ctrl ? "Ctrl" : "",
    binding.alt ? "Alt" : "",
    binding.shift ? "Shift" : "",
    binding.meta ? "Super" : "",
  ]
    .filter(Boolean)
    .join("+")
}

describe("resolveShortcutPlatform", () => {
  it("reads a Mac as a Mac and everything else as Linux", () => {
    expect(resolveShortcutPlatform("MacIntel")).toBe("mac")
    expect(resolveShortcutPlatform("iPhone")).toBe("mac")
    expect(resolveShortcutPlatform("Linux aarch64")).toBe("linux")
    expect(resolveShortcutPlatform("")).toBe("linux")
    // Omitting the argument reads `navigator.platform`, which this suite
    // declares as a Mac in its setup file.
    expect(resolveShortcutPlatform()).toBe("mac")
  })
})

describe("macOS bindings", () => {
  it("dispatches and displays exactly what the table authored", () => {
    for (const def of shortcuts) {
      const binding = mac.get(def.action)
      expect(binding, def.action).toBeDefined()
      expect(binding!.display, def.action).toBe(def.display)
      expect(binding!.meta ?? false, def.action).toBe(def.meta ?? false)
      expect(binding!.shift ?? false, def.action).toBe(def.shift ?? false)
      expect(binding!.alt ?? false, def.action).toBe(def.alt ?? false)
      expect(binding!.ctrl ?? false, def.action).toBe(def.ctrl ?? false)
    }
  })
})

describe("Linux bindings", () => {
  it("never asks for a Command key nobody has", () => {
    for (const [action, binding] of linux) {
      expect(binding.meta ?? false, action).toBe(false)
      expect(binding.display, action).not.toContain("⌘")
    }
  })

  it("never takes a plain Ctrl+letter, because the terminal owns those", () => {
    // Ctrl+C, Ctrl+D, Ctrl+Z, Ctrl+W and the rest of readline belong to the
    // agent session running inside the app, not to the app's chrome.
    for (const [action, binding] of linux) {
      const keys = Array.isArray(binding.key) ? binding.key : [binding.key]
      const isSingleLetter = keys.some((k) => /^[A-Za-z]$/.test(k))
      if (!isSingleLetter) continue
      const plainCtrl = binding.ctrl && !binding.shift && !binding.alt
      expect(plainCtrl, `${action} would steal Ctrl+${keys[0]?.toUpperCase()} from the terminal`).toBe(false)
    }
  })

  it("binds no two actions to the same chord", () => {
    const seen = new Map<string, string>()
    for (const [action, binding] of linux) {
      const keys = Array.isArray(binding.key) ? binding.key : [binding.key]
      // `["I", "i"]` is one binding written twice, not two.
      for (const id of new Set(keys.map((key) => `${chord(binding)}+${key.toLowerCase()}`))) {
        const previous = seen.get(id)
        expect(previous, `${action} collides with ${previous} on ${id}`).toBeUndefined()
        seen.set(id, action)
      }
    }
  })

  it("stays off the chords GNOME has already claimed", () => {
    // Ctrl+Alt+Arrow switches workspaces; an app cannot win that.
    for (const [action, binding] of linux) {
      const keys = Array.isArray(binding.key) ? binding.key : [binding.key]
      const isArrow = keys.some((k) => k.startsWith("Arrow"))
      if (!isArrow) continue
      expect(binding.ctrl && binding.alt, `${action} is on a GNOME workspace chord`).toBeFalsy()
    }
    // Ctrl+Alt+Backspace is the historical "kill the X server" chord.
    for (const [action, binding] of linux) {
      const keys = Array.isArray(binding.key) ? binding.key : [binding.key]
      if (!keys.includes("Backspace")) continue
      expect(binding.ctrl && binding.alt, `${action} is on the X-server-zap chord`).toBeFalsy()
    }
  })

  it("moves an unshifted Command binding to Ctrl+Shift and a shifted one to Ctrl+Alt", () => {
    expect(linux.get("createRepo")).toMatchObject({ ctrl: true, shift: true, alt: true, display: "Ctrl+Alt+Shift+I" })
    expect(linux.get("importRepo")).toMatchObject({ ctrl: true, shift: false, alt: true, display: "Ctrl+Alt+I" })
    expect(linux.get("closeTabOrWindow")).toMatchObject({ ctrl: true, shift: true, display: "Ctrl+Shift+W" })
    expect(linux.get("closeWindow")).toMatchObject({ ctrl: true, alt: true, display: "Ctrl+Alt+W" })
  })

  it("leaves a binding that was already Ctrl-only alone", () => {
    // The rule, not a binding that happens to exercise it: every authored
    // Ctrl chord in the table is now a named exception, and the transform
    // still has to pass an un-excepted one through untouched.
    expect(platformModifiers("notAnAction", { ctrl: true }, "linux")).toMatchObject({
      ctrl: true, shift: false, alt: false, meta: false,
    })
    expect(platformModifiers("notAnAction", { ctrl: true, shift: true }, "linux")).toMatchObject({
      ctrl: true, shift: true, alt: false, meta: false,
    })
  })

  it("keeps the vertical arrows off every chord the desktop swallows", () => {
    // Measured, not reasoned about: on a stock Ubuntu 26.04 GNOME/Wayland
    // session, keys injected below the compositor for Ctrl+Shift+↑/↓ and
    // Alt+↑/↓ deliver their modifiers to the webview and never the arrow, the
    // same way Ctrl+Alt+↑/↓ goes to the workspace switcher. `gsettings` names
    // no owner for the first two, which changes nothing: a chord that does not
    // arrive cannot be bound. See `tests/e2e/linux/shortcut-keys.test.ts`.
    const VERTICAL_DEAD_CHORDS: { ctrl: boolean; alt: boolean; shift: boolean; why: string }[] = [
      { ctrl: true, alt: true, shift: false, why: "GNOME switch-to-workspace" },
      { ctrl: true, alt: false, shift: true, why: "never delivered to the webview" },
      { ctrl: false, alt: true, shift: false, why: "never delivered to the webview" },
    ]
    for (const [action, binding] of linux) {
      const keys = Array.isArray(binding.key) ? binding.key : [binding.key]
      if (!keys.some((key) => key === "ArrowUp" || key === "ArrowDown")) continue
      for (const dead of VERTICAL_DEAD_CHORDS) {
        const sits = binding.ctrl === dead.ctrl && binding.alt === dead.alt && binding.shift === dead.shift
        expect(sits, `${action} sits on a vertical-arrow chord the desktop takes (${dead.why})`).toBe(false)
      }
    }
    // And the two that do arrive, each in the tier its authoring gives it:
    // task navigation one modifier lighter than repo navigation.
    expect(linux.get("navigateUp")).toMatchObject({ ctrl: true, alt: false, shift: false, key: "ArrowUp" })
    expect(linux.get("navigateDown")).toMatchObject({ ctrl: true, alt: false, shift: false, key: "ArrowDown" })
    expect(linux.get("navigateRepoUp")).toMatchObject({ ctrl: false, alt: true, shift: true, key: "ArrowUp" })
    expect(linux.get("navigateRepoDown")).toMatchObject({ ctrl: false, alt: true, shift: true, key: "ArrowDown" })
  })

  it("keeps navigation off the desktop's zoom chords", () => {
    // ⌃- / ⌃⇧- would have landed on Ctrl+- and Ctrl+Shift+-, which is zoom out
    // in the webview and in every browser beside it. Back/forward is Alt+Arrow
    // on Linux anyway, which is what a GTK app is expected to answer.
    expect(linux.get("goBack")).toMatchObject({
      ctrl: false, alt: true, shift: false, meta: false, key: "ArrowLeft", display: "Alt+←",
    })
    expect(linux.get("goForward")).toMatchObject({
      ctrl: false, alt: true, shift: false, meta: false, key: "ArrowRight", display: "Alt+→",
    })
    for (const [action, binding] of linux) {
      const keys = Array.isArray(binding.key) ? binding.key : [binding.key]
      const isZoomKey = keys.some((key) => key === "-" || key === "_" || key === "=" || key === "+" || key === "0")
      if (!isZoomKey) continue
      expect(binding.ctrl && !binding.alt, `${action} sits on a zoom chord`).toBeFalsy()
    }
  })

  it("keeps every chord off the ones IBus takes before the app sees them", () => {
    // Ctrl+Shift+U is IBus' unicode entry on a stock GNOME desktop. It never
    // reaches the webview — the owner pressed it and got a literal "u" typed —
    // so no amount of handler work can win it back; the binding has to move.
    for (const [action, binding] of linux) {
      const keys = Array.isArray(binding.key) ? binding.key : [binding.key]
      const claimsU = keys.some((key) => key.toLowerCase() === "u" || key.toLowerCase() === "e")
      if (!claimsU) continue
      expect(
        binding.ctrl && binding.shift && !binding.alt,
        `${action} is on an IBus chord the app cannot receive`,
      ).toBe(false)
    }
    expect(linux.get("goToOldestUnread")).toMatchObject({ ctrl: true, alt: true, shift: false, display: "Ctrl+Alt+U" })
    expect(linux.get("goToOldestUnreadAllRepos")).toMatchObject({
      ctrl: true, alt: true, shift: true, display: "Ctrl+Alt+Shift+U",
    })
  })

  it("gives tab cycling a listed chord instead of an unguessable hidden one", () => {
    // The owner read the shortcuts list, pressed what it showed at
    // Ctrl+Shift+←/→ (pane focus, a no-op without a split) and concluded tab
    // navigation was broken. Ctrl+Page Up / Page Down is what GNOME Terminal,
    // Firefox and VS Code all cycle tabs with, and it is now in the list.
    expect(linux.get("prevTab")).toMatchObject({
      ctrl: true, shift: false, alt: false, key: "PageUp", hidden: false, display: "Ctrl+Page Up",
    })
    expect(linux.get("nextTab")).toMatchObject({
      ctrl: true, shift: false, alt: false, key: "PageDown", hidden: false, display: "Ctrl+Page Down",
    })
  })

  it("drops the authored code when an exception moves the key", () => {
    // A `code` belongs to the key it was authored beside. Left behind on a
    // binding whose exception moved the key, it would fire the action on the
    // wrong physical key.
    for (const [action, binding] of linux) {
      const exception = linuxException(action)
      if (!exception?.key) continue
      expect(binding.code, `${action} kept a code from its authored key`).toBe(exception.code)
    }
  })

  it("keeps the VS Code chords that cost the terminal nothing", () => {
    // The whole table sits one tier above its VS Code twin, which is the
    // price of leaving readline alone. Ctrl+, is not part of that price: it
    // produces no control code and no terminal claims it.
    expect(linux.get("openPreferences")).toMatchObject({
      ctrl: true, shift: false, alt: false, meta: false, key: ",", code: "Comma", display: "Ctrl+,",
    })
  })

  it("puts the command palette on the chord a VS Code user's fingers already know", () => {
    // Ctrl+Shift+P was not dead here before — it opened the *file picker*,
    // which is harder to diagnose than a no-op, because the app answers and
    // answers with the wrong dialog. The picker takes the tier the palette
    // vacated, and file preview keeps the one above it.
    expect(linux.get("commandPalette")).toMatchObject({
      ctrl: true, shift: true, alt: false, meta: false, display: "Ctrl+Shift+P",
    })
    expect(linux.get("openFile")).toMatchObject({
      ctrl: true, shift: false, alt: true, meta: false, display: "Ctrl+Alt+P",
    })
    expect(linux.get("toggleFilePreview")).toMatchObject({
      ctrl: true, shift: true, alt: true, meta: false, display: "Ctrl+Alt+Shift+P",
    })
  })

  it("keeps Escape bare", () => {
    expect(linux.get("dismiss")).toMatchObject({ ctrl: false, shift: false, alt: false, meta: false, display: "Escape" })
  })

  it("spells arrows and Backspace so a hint reads as a key", () => {
    expect(linux.get("navigateUp")?.display).toBe("Ctrl+↑")
    expect(linux.get("navigateRepoDown")?.display).toBe("Alt+Shift+↓")
    expect(linux.get("closeTask")?.display).toBe("Ctrl+Shift+Backspace")
    expect(linux.get("previousPane")?.display).toBe("Ctrl+Shift+←")
    expect(linux.get("nextPane")?.display).toBe("Ctrl+Shift+→")
  })
})

describe("shortcutModifierTokens", () => {
  it("gives each platform the vocabulary its hints are written in", () => {
    expect(shortcutModifierTokens("mac")).toEqual(["⇧", "⌃", "⌥", "⌘"])
    expect(shortcutModifierTokens("linux")).toEqual(["Shift", "Ctrl", "Alt", "Super"])
  })
})

describe("terminalClipboardAction", () => {
  const key = (over: Partial<KeyboardEvent>) =>
    ({ key: "c", metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, ...over }) as KeyboardEvent

  it("acts on the Command chord on macOS", () => {
    expect(terminalClipboardAction(key({ key: "c", metaKey: true }), "mac")).toBe("copy")
    expect(terminalClipboardAction(key({ key: "v", metaKey: true }), "mac")).toBe("paste")
  })

  it("leaves plain Ctrl+C to the PTY on Linux, because it is SIGINT", () => {
    expect(terminalClipboardAction(key({ key: "c", ctrlKey: true }), "linux")).toBeNull()
    // Ctrl+V is readline's quoted-insert; also not ours.
    expect(terminalClipboardAction(key({ key: "v", ctrlKey: true }), "linux")).toBeNull()
  })

  it("acts on Ctrl+Shift+C/V on Linux", () => {
    expect(terminalClipboardAction(key({ key: "C", ctrlKey: true, shiftKey: true }), "linux")).toBe("copy")
    expect(terminalClipboardAction(key({ key: "V", ctrlKey: true, shiftKey: true }), "linux")).toBe("paste")
  })

  it("does not claim the Command chord on Linux, where it is the Super key", () => {
    expect(terminalClipboardAction(key({ key: "c", metaKey: true }), "linux")).toBeNull()
  })

  it("ignores every other key", () => {
    expect(terminalClipboardAction(key({ key: "d", metaKey: true }), "mac")).toBeNull()
    expect(terminalClipboardAction(key({ key: "c", metaKey: true, altKey: true }), "mac")).toBeNull()
  })
})

describe("hints for keys a modifier rewrites", () => {
  it("names the key you press, not the character Shift makes", () => {
    // `["I", "i"]` is one key; a hint saying "Ctrl+Alt+i" is not.
    expect(linux.get("closeWindow")?.display).toBe("Ctrl+Alt+W")
    expect(linux.get("showShortcuts")?.display).toBe("Ctrl+Shift+/")
    expect(linux.get("showAllShortcuts")?.display).toBe("Ctrl+Alt+/")
  })
})

describe("bindings whose Linux chord holds Shift over punctuation", () => {
  it("carries the physical code so a US layout's '?' still matches showShortcuts", () => {
    // showShortcuts' Linux chord is Ctrl+Shift+/, and Shift rewrites
    // KeyboardEvent.key from "/" to "?" on a US layout — matching by key alone
    // can never fire. The physical code is the only reliable match.
    expect(linux.get("showShortcuts")).toMatchObject({ ctrl: true, shift: true, alt: false, code: "Slash" })
  })

  it("leaves showAllShortcuts unaffected, since its Linux chord never holds Shift", () => {
    // ⇧⌘/ maps to Ctrl+Alt+/ on Linux (see the module doc comment), so the
    // authored Shift never reaches the actual dispatch and the key stays "/".
    expect(linux.get("showAllShortcuts")).toMatchObject({ ctrl: true, shift: false, alt: true, code: "Slash" })
  })

  it("keeps mac dispatch and display byte-identical after adding the Linux-only code", () => {
    expect(mac.get("showShortcuts")).toMatchObject({ meta: true, shift: false, ctrl: false, alt: false, display: "⌘/" })
    expect(mac.get("showAllShortcuts")).toMatchObject({ meta: true, shift: true, ctrl: false, alt: false, display: "⇧⌘/" })
  })
})

describe("metaOrControlHint", () => {
  it("labels a binding the view already dispatches on either modifier", () => {
    expect(metaOrControlHint("f", "mac")).toBe("⌘F")
    expect(metaOrControlHint("f", "linux")).toBe("Ctrl+F")
    expect(metaOrControlHint("Enter", "linux")).toBe("Ctrl+Enter")
  })
})

describe("hidden is a per-platform decision", () => {
  it("keeps tab cycling unlisted on macOS, where ⇧⌘[ / ⇧⌘] is the convention", () => {
    expect(mac.get("prevTab")?.hidden).toBe(true)
    expect(mac.get("nextTab")?.hidden).toBe(true)
  })

  it("resolves hidden for every action on both platforms", () => {
    for (const def of shortcuts) {
      expect(typeof mac.get(def.action)?.hidden, def.action).toBe("boolean")
      expect(typeof linux.get(def.action)?.hidden, def.action).toBe("boolean")
    }
  })
})

describe("isEditableElement", () => {
  const make = (html: string): HTMLElement => {
    const host = document.createElement("div")
    host.innerHTML = html
    return host.firstElementChild as HTMLElement
  }

  it("recognises the fields a person types into", () => {
    expect(isEditableElement(make(`<input type="text">`))).toBe(true)
    expect(isEditableElement(make(`<input type="search">`))).toBe(true)
    expect(isEditableElement(make(`<input>`))).toBe(true)
    expect(isEditableElement(make(`<textarea></textarea>`))).toBe(true)
  })

  it("is not fooled by a widget that happens to be an input", () => {
    expect(isEditableElement(make(`<input type="checkbox">`))).toBe(false)
    expect(isEditableElement(make(`<input type="range">`))).toBe(false)
    expect(isEditableElement(make(`<input type="text" readonly>`))).toBe(false)
    expect(isEditableElement(make(`<textarea disabled></textarea>`))).toBe(false)
    expect(isEditableElement(make(`<div></div>`))).toBe(false)
    expect(isEditableElement(null)).toBe(false)
  })

  it("does not count xterm's hidden helper textarea", () => {
    // It is an editable element in the DOM only: what is typed into it goes
    // to the PTY, and navigating tasks from a focused agent terminal has to
    // keep working.
    expect(isEditableElement(make(`<textarea class="xterm-helper-textarea"></textarea>`))).toBe(false)
  })
})

describe("belongsToTextEditing", () => {
  const field = document.createElement("input")
  const plain = document.createElement("div")
  const event = (key: string, shiftKey: boolean, extra: { altKey?: boolean; metaKey?: boolean } = {}) => ({
    key,
    shiftKey,
    altKey: extra.altKey ?? false,
    metaKey: extra.metaKey ?? false,
  })

  it("concedes a selection chord to the field it landed in", () => {
    // This is the bug the owner hit: Ctrl+Shift+← is *the* word-selection
    // chord, and the capture-phase handler was calling preventDefault() on it
    // in every text field in the app.
    for (const key of ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"]) {
      expect(belongsToTextEditing(event(key, true), field), key).toBe(true)
    }
  })

  it("keeps the deletion keys, which extend no selection", () => {
    // Shift+Backspace and Shift+Delete select nothing on either platform, and
    // conceding them handed a focused field the listed closeTask chord
    // (⇧⌘⌫ / Ctrl+Shift+Backspace) — which then silently closed nothing,
    // while on macOS ⌘⌫ went on to eat the composer draft.
    for (const platform of ["mac", "linux"] as const) {
      expect(belongsToTextEditing(event("Backspace", true), field, platform), platform).toBe(false)
      expect(belongsToTextEditing(event("Delete", true), field, platform), platform).toBe(false)
    }
  })

  it("concedes nothing outside a text field", () => {
    expect(belongsToTextEditing(event("ArrowLeft", true), plain)).toBe(false)
    expect(belongsToTextEditing(event("ArrowLeft", true), null)).toBe(false)
  })

  it("keeps the app tier, which a text field has no use for", () => {
    // Ctrl+Shift+S means nothing inside an input; advancing a stage from a
    // focused search field must still work. Only Shift+caret is conceded.
    expect(belongsToTextEditing(event("s", true), field)).toBe(false)
    expect(belongsToTextEditing(event("S", true), field)).toBe(false)
    expect(belongsToTextEditing(event("Escape", true), field)).toBe(false)
    // Without Shift there is no selection to extend: Ctrl+↑/↓ still moves
    // between tasks while the search field has focus, which is how search is
    // used at all — typing two characters and then walking the results.
    expect(belongsToTextEditing(event("ArrowUp", false), field)).toBe(false)
    expect(belongsToTextEditing(event("PageUp", false), field)).toBe(false)
  })

  it("concedes only the chords the platform's text fields actually select with", () => {
    // Alt+Shift+↑/↓ is repo navigation on Linux, and the agent composer holds
    // the caret almost all the time — so conceding it there is the difference
    // between a working binding and one that is dead in the app's main view.
    // A GTK field selects with Shift and Ctrl; Alt is the mnemonic modifier and
    // selects nothing, so there is nothing to concede.
    expect(belongsToTextEditing(event("ArrowDown", true, { altKey: true }), field, "linux")).toBe(false)
    expect(belongsToTextEditing(event("ArrowLeft", true, { metaKey: true }), field, "linux")).toBe(false)
    // Ctrl+Shift+arrow *is* word selection on Linux, and stays conceded: that
    // is why pane focus sits on a chord a text field eats, and why repo
    // navigation could not.
    expect(belongsToTextEditing(event("ArrowLeft", true), field, "linux")).toBe(true)
    // macOS is untouched — ⌥⇧← selects by word and ⇧⌘↑ selects to the top, so
    // every Shift+caret chord still belongs to the field.
    expect(belongsToTextEditing(event("ArrowDown", true, { altKey: true }), field, "mac")).toBe(true)
    expect(belongsToTextEditing(event("ArrowUp", true, { metaKey: true }), field, "mac")).toBe(true)
  })
})

describe("secondary bindings", () => {
  const linuxSecondary = secondaryBindingsFor("linux")

  it("gives the worktree shell VS Code's Ctrl+J without moving either shell chord", () => {
    // Ctrl+J is LF. The pair could not simply shift down a tier — that byte
    // belongs to the agent composers this app exists to run — so the listed
    // chords stay exactly where the systematic mapping put them and Ctrl+J is
    // an extra way in.
    expect(linux.get("openShell")).toMatchObject({ ctrl: true, shift: true, alt: false, display: "Ctrl+Shift+J" })
    expect(linux.get("openShellRepoRoot")).toMatchObject({ ctrl: true, alt: true, shift: false, display: "Ctrl+Alt+J" })
    expect(linuxSecondary.get("openShell")).toMatchObject({ ctrl: true, key: "j", activeInTerminal: false })
  })

  it("is the only plain Ctrl+letter, and only where no PTY has the caret", () => {
    // The invariant the resolved table keeps absolutely: a secondary binding
    // is the one way to be on a terminal key at all, and it is bought by
    // being inactive exactly where the terminal is.
    for (const [action, secondary] of linuxSecondary) {
      const keys = Array.isArray(secondary.key) ? secondary.key : [secondary.key]
      if (!keys.some((key) => /^[A-Za-z]$/.test(key))) continue
      if (secondary.ctrl && !secondary.shift && !secondary.alt) {
        expect(secondary.activeInTerminal, `${action} would take a terminal key from the PTY`).toBe(false)
      }
    }
  })

  it("collides with no resolved Linux binding", () => {
    const taken = new Set<string>()
    for (const binding of linux.values()) {
      const keys = Array.isArray(binding.key) ? binding.key : [binding.key]
      for (const key of keys) taken.add(`${chord(binding)}+${key.toLowerCase()}`)
    }
    for (const [action, secondary] of linuxSecondary) {
      const keys = Array.isArray(secondary.key) ? secondary.key : [secondary.key]
      for (const key of keys) {
        expect(taken.has(`${chord(secondary)}+${key.toLowerCase()}`), `${action}'s extra chord is taken`).toBe(false)
      }
    }
  })

  it("stays off macOS entirely, where every ⌘ chord already outranks the PTY", () => {
    expect(secondaryBindingsFor("mac").size).toBe(0)
    expect(secondaryBinding("openShell", "mac")).toBeUndefined()
    expect(secondaryBinding("openShell", "linux")).toBeDefined()
  })

  it("is never listed, because the modal must not advertise a chord that is dead in the agent view", () => {
    // The failure this module exists to prevent: a hint pressed in the view a
    // person spends the day in, which did nothing. The modal renders resolved
    // bindings only, so no secondary chord may appear among their hints.
    const hints = new Set([...linux.values()].map((binding) => binding.display))
    for (const [action, secondary] of linuxSecondary) {
      const keys = Array.isArray(secondary.key) ? secondary.key : [secondary.key]
      for (const key of keys) {
        const hint = `${chord(secondary)}+${key.toUpperCase()}`
        expect(hints.has(hint), `${action}'s unlisted ${hint} is being advertised`).toBe(false)
      }
    }
  })
})

describe("belongsToTerminalInput", () => {
  const make = (html: string): HTMLElement => {
    const host = document.createElement("div")
    host.innerHTML = html
    return host.firstElementChild as HTMLElement
  }

  it("recognises the one element a PTY types through", () => {
    expect(belongsToTerminalInput(make(`<textarea class="xterm-helper-textarea"></textarea>`))).toBe(true)
  })

  it("is not an editing question — it is the same element, asked the opposite way", () => {
    // `isEditableElement` answers false for the helper textarea so that task
    // navigation works from a focused terminal; this answers true for it so
    // that a chord the PTY owns is left alone there. Both are needed.
    const helper = make(`<textarea class="xterm-helper-textarea"></textarea>`)
    expect(isEditableElement(helper)).toBe(false)
    expect(belongsToTerminalInput(helper)).toBe(true)
  })

  it("claims nothing else", () => {
    expect(belongsToTerminalInput(make(`<textarea></textarea>`))).toBe(false)
    expect(belongsToTerminalInput(make(`<input type="text">`))).toBe(false)
    expect(belongsToTerminalInput(make(`<div></div>`))).toBe(false)
    expect(belongsToTerminalInput(null)).toBe(false)
  })
})
