/** A task owns views; panes arrange them without owning their sessions. */
export interface TaskPane { kind: 'pane'; id: string; tabs: string[]; active: string }
export interface TaskSplit { kind: 'split'; axis: 'horizontal' | 'vertical'; ratio: number; first: TaskPaneLayout; second: TaskPaneLayout }
export type TaskPaneLayout = TaskPane | TaskSplit;
export interface PaneRect { pane: TaskPane; left: number; top: number; width: number; height: number }
export function paneLeaves(node: TaskPaneLayout): TaskPane[] {
  return node.kind === 'pane' ? [node] : [...paneLeaves(node.first), ...paneLeaves(node.second)];
}
export function paneRects(node: TaskPaneLayout, left = 0, top = 0, width = 100, height = 100): PaneRect[] {
  if (node.kind === 'pane') return [{ pane: node, left, top, width, height }];
  return node.axis === 'horizontal'
    ? [...paneRects(node.first, left, top, width * node.ratio, height), ...paneRects(node.second, left + width * node.ratio, top, width * (1 - node.ratio), height)]
    : [...paneRects(node.first, left, top, width, height * node.ratio), ...paneRects(node.second, left, top + height * node.ratio, width, height * (1 - node.ratio))];
}
export function replacePane(node: TaskPaneLayout, id: string, replacement: TaskPaneLayout): TaskPaneLayout {
  if (node.kind === 'pane') return node.id === id ? replacement : node;
  return { ...node, first: replacePane(node.first, id, replacement), second: replacePane(node.second, id, replacement) };
}
export function removeEmptyPanes(node: TaskPaneLayout): TaskPaneLayout {
  if (node.kind === 'pane') return node;
  const first = removeEmptyPanes(node.first), second = removeEmptyPanes(node.second);
  if (first.kind === 'pane' && !first.tabs.length) return second;
  if (second.kind === 'pane' && !second.tabs.length) return first;
  return { ...node, first, second };
}
/** Reject malformed layouts and duplicate ownership; append newly introduced views. */
export function restorePaneLayout(raw: unknown, tabs: string[], active: string): TaskPaneLayout {
  const usedTabs = new Set<string>(), usedPanes = new Set<string>();
  function read(value: unknown, depth: number): TaskPaneLayout | null {
    if (!value || typeof value !== 'object' || depth > 16) return null;
    const node = value as Partial<TaskPane & { axis: TaskSplit['axis']; ratio: number; first: unknown; second: unknown }>;
    if (node.kind === 'pane' && typeof node.id === 'string' && !usedPanes.has(node.id) && Array.isArray(node.tabs)) {
      usedPanes.add(node.id);
      const owned = node.tabs.filter((id): id is string => typeof id === 'string' && tabs.includes(id) && !usedTabs.has(id) && Boolean(usedTabs.add(id)));
      return { kind: 'pane', id: node.id, tabs: owned, active: typeof node.active === 'string' && owned.includes(node.active) ? node.active : owned[0] ?? '' };
    }
    const split = value as Partial<TaskSplit>;
    if (split.kind !== 'split' || !['horizontal', 'vertical'].includes(split.axis ?? '') || typeof split.ratio !== 'number' || !Number.isFinite(split.ratio)) return null;
    const first = read(split.first, depth + 1), second = read(split.second, depth + 1);
    return first && second ? { kind: 'split', axis: split.axis as TaskSplit['axis'], ratio: Math.max(.15, Math.min(.85, split.ratio)), first, second } : null;
  }
  const result = read(raw, 0);
  if (!result) return { kind: 'pane', id: 'pane-1', tabs: [...tabs], active };
  const target = paneLeaves(result)[0];
  target.tabs.push(...tabs.filter(id => !usedTabs.has(id)));
  if (!target.active) target.active = target.tabs[0] ?? '';
  return result;
}

export interface SplitRect { path: string; axis: TaskSplit['axis']; ratio: number; left: number; top: number; width: number; height: number }
export function splitRects(node: TaskPaneLayout, path = '', left = 0, top = 0, width = 100, height = 100): SplitRect[] {
  if (node.kind === 'pane') return [];
  return [{ path, axis: node.axis, ratio: node.ratio, left, top, width, height },
    ...(node.axis === 'horizontal'
      ? [...splitRects(node.first, path + '0', left, top, width * node.ratio, height), ...splitRects(node.second, path + '1', left + width * node.ratio, top, width * (1 - node.ratio), height)]
      : [...splitRects(node.first, path + '0', left, top, width, height * node.ratio), ...splitRects(node.second, path + '1', left, top + height * node.ratio, width, height * (1 - node.ratio))])];
}
export function resizeSplit(node: TaskPaneLayout, path: string, ratio: number): void {
  if (node.kind === 'pane') return;
  if (!path) { node.ratio = Math.max(.15, Math.min(.85, ratio)); return; }
  resizeSplit(path[0] === '0' ? node.first : node.second, path.slice(1), ratio);
}
