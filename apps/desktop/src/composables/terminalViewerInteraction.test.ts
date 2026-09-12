import { describe, expect, it, vi } from "vitest"
import { observeTerminalViewerInteraction } from "./terminalViewerInteraction"

describe("terminal viewer interaction", () => {
  it("observes trusted gesture producers without consuming them, and removes listeners", () => {
    const container = document.createElement("div")
    const child = document.createElement("div")
    container.appendChild(child)
    const activate = vi.fn()
    const stop = observeTerminalViewerInteraction(container, activate)
    const gesture = (name: string) => {
      const event = new Event(name, { bubbles: true, cancelable: true })
      Object.defineProperty(event, "isTrusted", { value: true })
      child.dispatchEvent(event)
      expect(event.defaultPrevented).toBe(false)
    }
    for (const name of ["wheel", "pointerdown", "touchstart", "keydown"]) gesture(name)
    expect(activate).toHaveBeenCalledTimes(4)
    for (const name of ["scroll", "selectionchange", "mousemove", "focusin"]) gesture(name)
    child.dispatchEvent(new Event("wheel", { bubbles: true }))
    expect(activate).toHaveBeenCalledTimes(4)
    stop()
    gesture("wheel")
    expect(activate).toHaveBeenCalledTimes(4)
  })
})
