function isScrollbarPress(event: PointerEvent): boolean {
  return event.button === 0
    && event.target instanceof Element
    && event.target.closest(".xterm-scrollbar") !== null
}

function isScrollbackKey(event: KeyboardEvent): boolean {
  return event.shiftKey && (event.key === "PageUp" || event.key === "PageDown")
}

/** Observe deliberate scrolling before xterm handles it, without consuming the
 * gesture. Scroll notifications themselves also come from output, replay,
 * reflow, snapshot application and API calls, so they are never ownership
 * signals. The trusted producers below are the paths xterm uses for wheel /
 * trackpad scrolling, touch scrolling, scrollbar presses and keyboard
 * scrollback. Ordinary pointer, focus and key events are deliberately absent. */
export function observeTerminalViewerInteraction(
  container: HTMLElement,
  activate: () => void,
): () => void {
  const events = ["wheel", "touchmove", "pointerdown", "keydown"] as const
  const onInteraction = (event: Event) => {
    if (!event.isTrusted) return
    if (event instanceof WheelEvent && event.deltaX === 0 && event.deltaY === 0) return
    if (event instanceof PointerEvent && !isScrollbarPress(event)) return
    if (event instanceof KeyboardEvent && !isScrollbackKey(event)) return
    // Claims must preserve gesture order across viewers. Delaying a repeat
    // can reclaim ownership after another viewer's newer intentional action.
    activate()
  }
  for (const name of events) {
    container.addEventListener(name, onInteraction, { capture: true, passive: true })
  }
  return () => {
    for (const name of events) container.removeEventListener(name, onInteraction, true)
  }
}
