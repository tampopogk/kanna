import { describe, expect, it, vi } from "vitest"
import { observeTerminalViewerInteraction } from "./terminalViewerInteraction"

describe("terminal viewer interaction", () => {
  function harness(onActivate?: () => void) {
    const container = document.createElement("div")
    const child = document.createElement("div")
    container.appendChild(child)
    const activate = vi.fn(onActivate)
    const stop = observeTerminalViewerInteraction(container, activate)
    const gesture = (event: Event, target: Element = child) => {
      Object.defineProperty(event, "isTrusted", { value: true })
      target.dispatchEvent(event)
      expect(event.defaultPrevented).toBe(false)
    }
    return { container, child, activate, stop, gesture }
  }

  it("claims each trusted wheel or trackpad event without consuming it", () => {
    const { activate, stop, gesture } = harness()
    gesture(new WheelEvent("wheel", { bubbles: true, cancelable: true, deltaY: -20 }))
    gesture(new WheelEvent("wheel", { bubbles: true, cancelable: true, deltaX: 12 }))
    expect(activate).toHaveBeenCalledTimes(2)

    gesture(new WheelEvent("wheel", { bubbles: true, deltaX: 0, deltaY: 0 }))
    expect(activate).toHaveBeenCalledTimes(2)
    stop()
  })

  it("claims scrollbar, touch-move and xterm keyboard scrollback paths", () => {
    const { container, activate, stop, gesture } = harness()
    const scrollbar = document.createElement("div")
    scrollbar.className = "xterm-scrollbar"
    const slider = document.createElement("div")
    scrollbar.appendChild(slider)
    container.appendChild(scrollbar)

    gesture(new PointerEvent("pointerdown", { bubbles: true, cancelable: true, button: 0 }), slider)
    gesture(new TouchEvent("touchmove", { bubbles: true, cancelable: true }))
    gesture(new KeyboardEvent("keydown", { bubbles: true, key: "PageUp", shiftKey: true }))
    gesture(new KeyboardEvent("keydown", { bubbles: true, key: "PageDown", shiftKey: true }))
    expect(activate).toHaveBeenCalledTimes(4)
    stop()
  })

  it("keeps focus, ordinary input, selection presses and passive scrolling inert", () => {
    const { child, activate, stop, gesture } = harness()
    for (const event of [
      new FocusEvent("focusin", { bubbles: true }),
      new Event("scroll", { bubbles: true }),
      new Event("selectionchange", { bubbles: true }),
      new PointerEvent("pointerdown", { bubbles: true, button: 0 }),
      new KeyboardEvent("keydown", { bubbles: true, key: "x" }),
      new KeyboardEvent("keydown", { bubbles: true, key: "PageUp" }),
    ]) gesture(event)
    child.dispatchEvent(new WheelEvent("wheel", { bubbles: true, deltaY: -20 }))
    expect(activate).not.toHaveBeenCalled()
    stop()
  })

  it("preserves interleaved intent A@0, A@10, B@50 and a later handoff to A", () => {
    const claims: string[] = []
    const a = harness(() => claims.push("A"))
    const b = harness(() => claims.push("B"))
    const wheel = () => new WheelEvent("wheel", { bubbles: true, deltaY: -20 })
    a.gesture(wheel())
    a.gesture(wheel())
    b.gesture(wheel())
    expect(claims).toEqual(["A", "A", "B"])
    expect(claims.at(-1)).toBe("B")
    expect(claims).toHaveLength(3)
    a.gesture(wheel())
    expect(claims.at(-1)).toBe("A")
    expect(claims).toHaveLength(4)
    a.stop()
    b.stop()
  })

  it("stops observing immediately when disposed", () => {
    const { activate, gesture, stop } = harness()
    stop()
    gesture(new WheelEvent("wheel", { bubbles: true, deltaY: -20 }))
    expect(activate).not.toHaveBeenCalled()
  })
})
