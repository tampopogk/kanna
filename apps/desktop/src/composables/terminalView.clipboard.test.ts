import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { ref } from "vue"
import { Terminal } from "@xterm/xterm"
import { FitAddon } from "@xterm/addon-fit"
import { initializeTerminalView, type InitializedTerminalView } from "./terminalView"
import { createTerminalRuntimeState } from "./terminalRuntimeState"

vi.mock("./terminalRenderer", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./terminalRenderer")>()),
  requestedTerminalRenderer: () => "dom",
}))

/**
 * The whole file is the Linux keyboard. `terminalView` resolves the platform
 * once at module load, and the suite's own setup declares macOS, so asking for
 * Linux here has to happen in the module graph rather than per test.
 */
vi.mock("./shortcutPlatform", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./shortcutPlatform")>()),
  resolveShortcutPlatform: () => "linux" as const,
}))

/**
 * Terminal paste, on the platform where it has to be read natively.
 *
 * `Ctrl+Shift+V` is what the shortcuts modal advertises on Linux, and the
 * chord was arriving and being claimed while pasting nothing at all:
 * `navigator.clipboard.readText()` is denied by WebKitGTK policy through this
 * path whatever gesture produced the keystroke, so the promise could only
 * reject. These press the chord through the production key handler against a
 * real xterm and assert what reaches the PTY.
 */
const views: InitializedTerminalView[] = []
let keyHandler: ((event: KeyboardEvent) => boolean) | null = null

beforeEach(() => {
  // The one thing this lane cannot have is a browser painting surface.
  vi.spyOn(Terminal.prototype, "open").mockImplementation(() => {})
  keyHandler = null
  vi.spyOn(Terminal.prototype, "attachCustomKeyEventHandler").mockImplementation(function (
    this: Terminal,
    handler: (event: KeyboardEvent) => boolean,
  ) {
    keyHandler = handler
    return this
  })
})

afterEach(() => {
  for (const view of views.splice(0)) {
    view.cleanupContainerEvents?.()
    view.stopThemeWatch()
    view.stopFileLinkAvailabilityWatch()
    view.unregisterE2ETerminalBuffer()
    view.unregisterFileLinkProvider()
    view.term.dispose()
  }
  vi.restoreAllMocks()
})

interface Harness {
  term: Terminal
  sent: string[]
  readClipboardText: ReturnType<typeof vi.fn>
  press: (key: string, modifiers?: { ctrl?: boolean; shift?: boolean; meta?: boolean }) => {
    allowed: boolean
    preventDefault: ReturnType<typeof vi.fn>
  }
}

function harness(clipboard: () => Promise<string | null>): Harness {
  const sent: string[] = []
  const readClipboardText = vi.fn(clipboard)
  const el = document.createElement("div")
  const view = initializeTerminalView({
    el,
    state: createTerminalRuntimeState(),
    sessionId: `clipboard-${views.length}`,
    instanceId: `clipboard-${views.length}`,
    options: { agentProvider: "claude", agentTerminal: true },
    effectiveCodeTheme: ref("dark"),
    fitAddon: new FitAddon(),
    getContainer: () => el,
    isDisposed: () => false,
    isAttached: () => false,
    getStreamClient: () => null,
    handleLinkActivate: vi.fn(),
    sendInputBytes: vi.fn(async (bytes: Uint8Array) => {
      sent.push(new TextDecoder().decode(bytes))
    }),
    maybeReadClipboardImage: vi.fn(async () => {}),
    readClipboardText,
    sendDroppedPaths: vi.fn(),
    onNativeDropCleanupReady: vi.fn(),
    onTerminalInteraction: vi.fn(),
    setTerminal: vi.fn(),
  })
  views.push(view)

  function press(key: string, modifiers: { ctrl?: boolean; shift?: boolean; meta?: boolean } = {}) {
    if (!keyHandler) throw new Error("the terminal view attached no key handler")
    const preventDefault = vi.fn()
    const event = {
      type: "keydown",
      key,
      code: `Key${key.toUpperCase()}`,
      ctrlKey: modifiers.ctrl ?? false,
      shiftKey: modifiers.shift ?? false,
      altKey: false,
      metaKey: modifiers.meta ?? false,
      isComposing: false,
      preventDefault,
    } as unknown as KeyboardEvent
    return { allowed: keyHandler(event), preventDefault }
  }

  return { term: view.term, sent, readClipboardText, press }
}

async function write(term: Terminal, data: string): Promise<void> {
  await new Promise<void>((resolve) => term.write(data, resolve))
}

/** Let the clipboard read and xterm's input queue settle. */
async function settle(): Promise<void> {
  for (let tick = 0; tick < 4; tick += 1) await Promise.resolve()
  await new Promise((resolve) => setTimeout(resolve, 0))
}

describe("Ctrl+Shift+V on Linux", () => {
  it("pastes what the native clipboard read returned", async () => {
    const { sent, readClipboardText, press } = harness(async () => "native clipboard text")

    const { allowed, preventDefault } = press("v", { ctrl: true, shift: true })
    await settle()

    expect(allowed, "the chord was handed to the PTY as a keystroke").toBe(false)
    expect(preventDefault).toHaveBeenCalled()
    expect(readClipboardText).toHaveBeenCalledTimes(1)
    expect(sent.join("")).toBe("native clipboard text")
  })

  it("brackets a multi-line paste when the program asked for bracketed paste", async () => {
    const { term, sent, press } = harness(async () => "first\nsecond")

    // What a shell or an agent CLI sends when it wants pastes marked.
    await write(term, "\x1b[?2004h")
    press("v", { ctrl: true, shift: true })
    await settle()

    // xterm's own paste path: CR line endings inside the markers, so the
    // program on the other end reads one paste rather than two submissions.
    expect(sent.join("")).toBe("\x1b[200~first\rsecond\x1b[201~")
  })

  it("sends a multi-line paste unbracketed when the program did not ask", async () => {
    const { sent, press } = harness(async () => "first\nsecond")

    press("v", { ctrl: true, shift: true })
    await settle()

    expect(sent.join("")).toBe("first\rsecond")
  })

  it("pastes nothing when the clipboard read came back empty", async () => {
    const { sent, readClipboardText, press } = harness(async () => null)

    press("v", { ctrl: true, shift: true })
    await settle()

    expect(readClipboardText).toHaveBeenCalledTimes(1)
    expect(sent).toEqual([])
  })

  it("leaves plain Ctrl+V to the PTY, where it is readline's quoted insert", async () => {
    const { readClipboardText, press } = harness(async () => "native clipboard text")

    const { allowed, preventDefault } = press("v", { ctrl: true })

    expect(allowed).toBe(true)
    expect(preventDefault).not.toHaveBeenCalled()
    expect(readClipboardText).not.toHaveBeenCalled()
  })
})

describe("Ctrl+Shift+C on Linux", () => {
  const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard")

  afterEach(() => {
    if (originalClipboard) Object.defineProperty(navigator, "clipboard", originalClipboard)
    else Reflect.deleteProperty(navigator, "clipboard")
  })

  it("logs a refused clipboard write instead of swallowing it", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    const writeText = vi.fn(async () => {
      throw new Error("NotAllowedError: write permission denied")
    })
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    })

    const { term, press } = harness(async () => null)
    await write(term, "selected output")
    vi.spyOn(term, "getSelection").mockReturnValue("selected output")

    const { allowed, preventDefault } = press("c", { ctrl: true, shift: true })
    await settle()

    expect(allowed).toBe(false)
    expect(preventDefault).toHaveBeenCalled()
    expect(writeText).toHaveBeenCalledWith("selected output")
    expect(warn).toHaveBeenCalledWith(
      "[terminal] clipboard copy failed:",
      expect.objectContaining({ message: expect.stringContaining("NotAllowedError") }),
    )
  })
})
