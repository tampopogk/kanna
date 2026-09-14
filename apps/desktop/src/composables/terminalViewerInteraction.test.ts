import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { observeTerminalViewerInteraction } from "./terminalViewerInteraction"

describe("terminal viewer interaction", () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  function harness(onActivate?: () => void) {
    const container = document.createElement("div")
    const child = document.createElement("div")
    container.appendChild(child)
    const activate = vi.fn(onActivate)
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
    expect(activate).toHaveBeenCalledTimes(4)
    vi.advanceTimersByTime(500)
    expect(activate).toHaveBeenCalledTimes(4)

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

  it("preserves interleaved intent A@0, A@10, B@50 and a later handoff to A", () => {
    vi.setSystemTime(0)
    const claims: Array<[string, number]> = []
    const a = harness(() => claims.push(["A", Date.now()]))
    const b = harness(() => claims.push(["B", Date.now()]))
    a.gesture("wheel")
    vi.advanceTimersByTime(10)
    a.gesture("wheel")
    vi.advanceTimersByTime(40)
    b.gesture("wheel")
    expect(claims).toEqual([["A", 0], ["A", 10], ["B", 50]])
    vi.advanceTimersByTime(50)
    expect(claims.at(-1)).toEqual(["B", 50])
    vi.advanceTimersByTime(5000)
    expect(claims).toHaveLength(3)
    a.gesture("wheel")
    expect(claims.at(-1)).toEqual(["A", 5100])
    vi.advanceTimersByTime(5000)
    expect(claims).toHaveLength(4)
    a.stop()
    b.stop()
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
