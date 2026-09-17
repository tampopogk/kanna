import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const invokeMock = vi.fn()

vi.mock("../invoke", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

import { createTerminalClipboardBridge } from "./terminalClipboardBridge"

function bridge() {
  return createTerminalClipboardBridge({
    sessionId: "session-1",
    instanceId: "instance-1",
    options: { agentTerminal: true },
    outputDecoder: new TextDecoder(),
    sendInputBytes: vi.fn(async () => {}),
  })
}

beforeEach(() => {
  invokeMock.mockReset()
})

afterEach(() => {
  vi.restoreAllMocks()
})

/**
 * The native clipboard text read, which is the only one Linux has.
 *
 * `navigator.clipboard.readText()` is denied by WebKitGTK policy however the
 * paste chord arrives, so the terminal's Ctrl+Shift+V handler asks the app for
 * the clipboard instead. What matters here is the contract that handler is
 * written against: text or `null`, never a rejection, and never silence about
 * a refusal.
 */
describe("the terminal clipboard bridge's text read", () => {
  it("reads the clipboard through the native command", async () => {
    invokeMock.mockResolvedValue("pasted text")

    await expect(bridge().readClipboardText()).resolves.toBe("pasted text")
    expect(invokeMock).toHaveBeenCalledWith("read_clipboard_text", {})
  })

  it("keeps the newlines a multi-line paste carries", async () => {
    invokeMock.mockResolvedValue("first\nsecond\nthird")

    await expect(bridge().readClipboardText()).resolves.toBe("first\nsecond\nthird")
  })

  it("reports an empty clipboard as nothing to paste", async () => {
    invokeMock.mockResolvedValue("")

    await expect(bridge().readClipboardText()).resolves.toBeNull()
  })

  it("reports a clipboard that holds no text as nothing to paste", async () => {
    invokeMock.mockResolvedValue(null)

    await expect(bridge().readClipboardText()).resolves.toBeNull()
  })

  it("logs a refused read rather than swallowing it, and pastes nothing", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    invokeMock.mockRejectedValue(new Error("failed to open clipboard: no X11 display"))

    await expect(bridge().readClipboardText()).resolves.toBeNull()
    expect(warn).toHaveBeenCalledWith(
      "[terminal][clipboard] failed to read clipboard text",
      expect.objectContaining({
        sessionId: "session-1",
        instanceId: "instance-1",
        error: expect.stringContaining("failed to open clipboard"),
      }),
    )
  })
})
