/** Observe intent before xterm handles it, without consuming keys or gestures.
 * Scroll/selection notifications also come from replay, reflow and API calls;
 * only their user gesture producers may claim the daemon's geometry. */
export function observeTerminalViewerInteraction(
  container: HTMLElement,
  activate: () => void,
): () => void {
  const events = ["wheel", "pointerdown", "touchstart", "keydown"] as const
  const onInteraction = (event: Event) => {
    if (event.isTrusted) activate()
  }
  for (const name of events) {
    container.addEventListener(name, onInteraction, { capture: true, passive: true })
  }
  return () => {
    for (const name of events) container.removeEventListener(name, onInteraction, true)
  }
}
