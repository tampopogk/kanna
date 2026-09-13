/** One continuous gesture is one viewing action. A trackpad scroll or a key
 * repeat delivers dozens of events per second, and an ownership claim per
 * event turns a single flick into a burst of ordered KSP control frames — each
 * one a daemon election, and, whenever another viewer claims in between, a real
 * PTY resize and snapshot cutover. That is what several seconds of resizing
 * looks like from the terminal's side.
 *
 * The claim still leads the gesture, so a legitimate handoff is immediate; only
 * the repeats behind it are collapsed into one trailing claim that keeps a
 * continuing gesture's ownership current within this window. */
const VIEWER_ACTIVITY_COALESCE_MS = 100

/** Observe intent before xterm handles it, without consuming keys or gestures.
 * Scroll/selection notifications also come from replay, reflow and API calls;
 * only their user gesture producers may claim the daemon's geometry. */
export function observeTerminalViewerInteraction(
  container: HTMLElement,
  activate: () => void,
): () => void {
  const events = ["wheel", "pointerdown", "touchstart", "keydown"] as const
  let coalesceTimer: ReturnType<typeof setTimeout> | null = null
  let pending = false
  const openWindow = () => {
    coalesceTimer = setTimeout(() => {
      coalesceTimer = null
      if (!pending) return
      pending = false
      activate()
      openWindow()
    }, VIEWER_ACTIVITY_COALESCE_MS)
  }
  const onInteraction = (event: Event) => {
    if (!event.isTrusted) return
    if (coalesceTimer !== null) {
      pending = true
      return
    }
    activate()
    openWindow()
  }
  for (const name of events) {
    container.addEventListener(name, onInteraction, { capture: true, passive: true })
  }
  return () => {
    for (const name of events) container.removeEventListener(name, onInteraction, true)
    if (coalesceTimer !== null) clearTimeout(coalesceTimer)
    coalesceTimer = null
    pending = false
  }
}
