import { onBeforeUnmount, ref, watch } from 'vue';

export interface TabDropTarget { paneId: string; beforeId?: string }

/** In-app pointer dragging: native OS file drops keep their separate Tauri path. */
export function usePaneTabDrag(options: {
  scope: () => string | null | undefined;
  target: (x: number, y: number) => TabDropTarget | null;
  move: (id: string, target: TabDropTarget) => void;
}) {
  const dragging = ref<string | null>(null);
  const target = ref<TabDropTarget | null>(null);
  let gesture: { id: string; pointerId: number; x: number; y: number; source: HTMLElement; scope: string } | null = null;
  let suppressClick: HTMLElement | null = null;
  function click(event: MouseEvent) {
    if (event.target instanceof Node && suppressClick?.contains(event.target)) { event.preventDefault(); event.stopImmediatePropagation(); }
    suppressClick = null;
    document.removeEventListener('click', click, true);
  }
  function stop() {
    const prior = gesture;
    gesture = null;
    dragging.value = null;
    target.value = null;
    document.removeEventListener('pointermove', move);
    document.removeEventListener('pointerup', end);
    document.removeEventListener('pointercancel', cancel);
    document.removeEventListener('keydown', key);
    window.removeEventListener('blur', cancel);
    if (prior?.source.hasPointerCapture?.(prior.pointerId)) prior.source.releasePointerCapture(prior.pointerId);
  }
  function move(event: PointerEvent) {
    if (!gesture || event.pointerId !== gesture.pointerId) return;
    if (options.scope() !== gesture.scope) { stop(); return; }
    if (!dragging.value && Math.hypot(event.clientX - gesture.x, event.clientY - gesture.y) < 5) return;
    dragging.value = gesture.id;
    event.preventDefault();
    target.value = options.target(event.clientX, event.clientY);
  }
  function end(event: PointerEvent) {
    if (!gesture || event.pointerId !== gesture.pointerId) return;
    const id = dragging.value;
    const destination = options.scope() === gesture.scope ? options.target(event.clientX, event.clientY) : null;
    suppressClick = id ? gesture.source : null;
    if (id) document.addEventListener('click', click, true);
    stop();
    if (id && destination) options.move(id, destination);
  }
  function cancel() { stop(); }
  function key(event: KeyboardEvent) { if (event.key === 'Escape') { event.preventDefault(); stop(); } }
  function start(event: PointerEvent, id: string) {
    if (event.button !== 0 || event.isPrimary === false || !options.scope()) return;
    if (event.target instanceof Element && event.target.closest('button,select,input')) return;
    const source = event.currentTarget;
    if (!(source instanceof HTMLElement)) return;
    stop();
    // A fresh press cannot be the browser's trailing click from the last drag.
    suppressClick = null;
    document.removeEventListener('click', click, true);
    gesture = { id, pointerId: event.pointerId, x: event.clientX, y: event.clientY, source, scope: options.scope()! };
    if (event.isTrusted) source.setPointerCapture?.(event.pointerId);
    document.addEventListener('pointermove', move);
    document.addEventListener('pointerup', end);
    document.addEventListener('pointercancel', cancel);
    document.addEventListener('keydown', key);
    window.addEventListener('blur', cancel);
  }
  watch(options.scope, stop);
  onBeforeUnmount(() => { stop(); document.removeEventListener('click', click, true); });
  return { start, dragging, target };
}
