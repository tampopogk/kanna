import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { observeTerminalViewerInteraction } from "./terminalViewerInteraction"

describe("terminal viewer interaction", () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  function harness() {
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
    return { child, activate, stop, gesture }
  }

  it("observes trusted gesture producers without consuming them, and removes listeners", () => {
    const { child, activate, stop, gesture } = harness()
    for (const name of ["wheel", "pointerdown", "touchstart", "keydown"]) gesture(name)
    // Each of these is a producer, but they arrived inside one window.
    expect(activate).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(500)
    expect(activate).toHaveBeenCalledTimes(2)

    activate.mockClear()
    for (const name of ["scroll", "selectionchange", "mousemove", "focusin"]) gesture(name)
    child.dispatchEvent(new Event("wheel", { bubbles: true }))
    vi.advanceTimersByTime(500)
    expect(activate).not.toHaveBeenCalled()

    stop()
    gesture("wheel")
    vi.advanceTimersByTime(500)
    expect(activate).not.toHaveBeenCalled()
  })

  it("claims on the leading edge so a handoff is not delayed", () => {
    const { activate, gesture, stop } = harness()
    gesture("wheel")
    expect(activate).toHaveBeenCalledTimes(1)
    stop()
  })

  it("collapses a scroll burst into a bounded number of claims", () => {
    const { activate, gesture, stop } = harness()
    // A one-second trackpad flick at ~60 wheel events per second.
    for (let tick = 0; tick < 60; tick += 1) {
      gesture("wheel")
      vi.advanceTimersByTime(16)
    }
    vi.advanceTimersByTime(500)
    // One leading claim plus one per elapsed coalescing window, not sixty.
    expect(activate.mock.calls.length).toBeLessThanOrEqual(12)
    expect(activate.mock.calls.length).toBeGreaterThan(0)
    stop()
  })

  it("stops claiming once a gesture ends", () => {
    const { activate, gesture, stop } = harness()
    gesture("wheel")
    gesture("wheel")
    vi.advanceTimersByTime(100)
    const afterGesture = activate.mock.calls.length
    vi.advanceTimersByTime(5000)
    expect(activate.mock.calls.length).toBe(afterGesture)
    stop()
  })
})
