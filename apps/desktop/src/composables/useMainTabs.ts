import { computed, reactive, type ComputedRef } from "vue";

import type { TerminalEditorSession } from "../services/desktopServerClient";
import type { ShortcutContext } from "./useShortcutContext";
import { paneLeaves, paneRects, splitRects, resizeSplit, replacePane, removeEmptyPanes, restorePaneLayout, type TaskPaneLayout } from "./taskPaneLayout";

/**
 * The view kinds the main content area can host.
 *
 * `agent` is the task's own session — the terminal (or agent message view, or
 * the cloud terminal for a task another machine owns). It is implicit: every
 * *task* scope has exactly one, it is always first, and it can never be
 * closed. A repo scope has no agent session, so its tab set starts empty.
 */
export type MainTabKind =
  | "agent"
  | "diff"
  | "file"
  | "shell"
  | "editor"
  | "tree"
  | "graph"
  | "analytics"
  | "image"
  | "preview";

/**
 * Which shell a `shell` tab runs: the task's worktree (⌘J) or the repository
 * root (⇧⌘J). Both can be open at once in a task scope, so they are separate
 * tabs rather than one tab that changes directory underneath the reader.
 */
export type ShellTabScope = "worktree" | "repo";

export interface MainTabReadingState {
  workspace: string;
  top?: number;
  diff?: {
    scope?: "branch" | "working";
    scrollPositions?: { branch?: number; working?: number };
    branchInclude?: "none" | "staged" | "all";
  };
}

export interface MainTabDescriptor {
  kind: MainTabKind;
  /** A preview stores a claimed port name, never a URL or entry credential. */
  portName?: string;
  /** Reading coordinates only apply to the workspace that produced them. */
  reading?: MainTabReadingState;
  editorSession?: TerminalEditorSession;
  /** `file` tabs: worktree-relative or absolute path of the file to show. */
  filePath?: string;
  /** `file` tabs: line to scroll to on open. */
  initialLine?: number;
  /**
   * `file` tabs: point-in-time content fetched from another machine. Present
   * only for a file that cannot be re-read locally.
   */
  remoteContent?: string | null;
  /** `shell` tabs: which shell this is. Defaults to the task worktree. */
  shellScope?: ShellTabScope;
  /** `image` tabs: the URL of the image to show. */
  imageUrl?: string;
  /**
   * `file` and `tree` tabs an agent opened through `kanna_open_view`: read
   * this task's content through the server's contained resolution rather than
   * from the worktree path directly.
   *
   * The server validates a target descriptor-relative to the task's worktree,
   * refusing symlinks at every step. A renderer that then reads the same path
   * off the filesystem re-walks it through whatever links exist by then, so a
   * swap after validation would show content from outside the worktree. The
   * task id travels with the tab because the read has to keep asking the
   * component that owns the containment rule.
   */
  containedTaskId?: string;
}

export interface MainTab extends MainTabDescriptor {
  /** Stable within a scope, so re-opening the same view focuses it. */
  id: string;
}

interface MainTabScopeState {
  layout?: TaskPaneLayout;
  focusedPane?: string;
  referenceId?: string;
  split?: boolean;
  tabs: MainTab[];
  activeId: string;
}

/** Bump when the stored shape becomes incompatible; an older payload is discarded, not guessed at. */
export const PERSISTED_MAIN_TABS_VERSION = 1;

export interface PersistedMainTabScope {
  layout?: TaskPaneLayout;
  referenceId?: string;
  split?: boolean;
  tabs: MainTabDescriptor[];
  activeId: string;
}

export interface PersistedMainTabs {
  version: number;
  scopes: Record<string, PersistedMainTabScope>;
}

/**
 * Whether a tab is worth writing down.
 *
 * A restart may only rebuild a view it can rebuild *honestly*. The agent tab
 * is implicit — every task scope re-creates its own — so storing it would only
 * risk contradicting that. An `image` tab holds a URL minted for one session,
 * and a `file` tab carrying `remoteContent` holds bytes read from another
 * machine at a moment that has passed: restoring either would show the reader
 * something the app can no longer stand behind. A shell tab is restorable
 * because its session id is derived, not remembered — the daemon outlives the
 * app, so the tab reattaches to the surviving session and otherwise starts a
 * fresh one in the same directory.
 */
export function isRestorableTab(tab: MainTabDescriptor): boolean {
  switch (tab.kind) {
    case "agent":
    case "image":
      return false;
    case "preview":
      return typeof tab.portName === "string" && tab.portName.length > 0;
    case "editor":
      return Boolean(tab.editorSession && [tab.editorSession.sessionId, tab.editorSession.worktreePath, tab.editorSession.filePath, tab.editorSession.command]
        .every(value => typeof value === "string" && value.length > 0));
    case "file":
      return Boolean(tab.filePath) && tab.remoteContent == null;
    default:
      return true;
  }
}

function persistedDescriptor(tab: MainTabDescriptor): MainTabDescriptor {
  const descriptor: MainTabDescriptor = { kind: tab.kind };
  if (typeof tab.portName === "string") descriptor.portName = tab.portName;
  if (tab.reading && typeof tab.reading.workspace === "string") {
    const { workspace, top, diff } = tab.reading;
    descriptor.reading = { workspace };
    if (typeof top === "number" && Number.isFinite(top) && top >= 0) descriptor.reading.top = top;
    if (diff) {
      descriptor.reading.diff = {};
      if (diff.scope === "branch" || diff.scope === "working") descriptor.reading.diff.scope = diff.scope;
      if (["none", "staged", "all"].includes(diff.branchInclude ?? "")) descriptor.reading.diff.branchInclude = diff.branchInclude;
      for (const scope of ["branch", "working"] as const) {
        const position = diff.scrollPositions?.[scope];
        if (typeof position === "number" && Number.isFinite(position) && position >= 0) {
          (descriptor.reading.diff.scrollPositions ??= {})[scope] = position;
        }
      }
    }
  }
  // Keep server-contained reads contained after a restart.
  if (tab.containedTaskId) descriptor.containedTaskId = tab.containedTaskId;
  if (tab.editorSession !== undefined) descriptor.editorSession = { ...tab.editorSession };
  if (tab.filePath !== undefined) descriptor.filePath = tab.filePath;
  if (tab.initialLine !== undefined) descriptor.initialLine = tab.initialLine;
  if (tab.shellScope !== undefined) descriptor.shellScope = tab.shellScope;
  return descriptor;
}

/**
 * Reads a stored payload back, keeping only what this version understands.
 *
 * Anything unrecognized — a future version, a hand-edited setting, a kind that
 * no longer exists — is dropped rather than trusted, because the alternative
 * is a startup that throws on a value nobody can see to fix.
 */
export function parsePersistedMainTabs(raw: string | null | undefined): PersistedMainTabs | null {
  if (!raw) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const candidate = parsed as Partial<PersistedMainTabs>;
  if (candidate.version !== PERSISTED_MAIN_TABS_VERSION) return null;
  if (typeof candidate.scopes !== "object" || candidate.scopes === null) return null;

  const scopes: Record<string, PersistedMainTabScope> = {};
  for (const [key, value] of Object.entries(candidate.scopes)) {
    if (typeof value !== "object" || value === null) continue;
    const rawTabs = Array.isArray((value as PersistedMainTabScope).tabs)
      ? (value as PersistedMainTabScope).tabs
      : [];
    const tabs = rawTabs.filter((tab): tab is MainTabDescriptor =>
      typeof tab === "object"
      && tab !== null
      && typeof (tab as MainTabDescriptor).kind === "string"
      && (tab as MainTabDescriptor).kind in TAB_SHORTCUT_CONTEXTS
      && isRestorableTab(tab as MainTabDescriptor)
    ).map(persistedDescriptor);
    if (tabs.length === 0) continue;
    const activeId = typeof (value as PersistedMainTabScope).activeId === "string"
      ? (value as PersistedMainTabScope).activeId
      : "";
    scopes[key] = { tabs, activeId,
      layout: value.layout,
      referenceId: typeof value.referenceId === "string" ? value.referenceId : undefined,
      split: typeof value.split === "boolean" ? value.split : undefined,
    };
  }
  return { version: PERSISTED_MAIN_TABS_VERSION, scopes };
}

export const AGENT_TAB_ID = "agent";

const TAB_SHORTCUT_CONTEXTS: Record<MainTabKind, ShortcutContext> = {
  agent: "main",
  diff: "diff",
  file: "file",
  shell: "shell",
  editor: "shell",
  tree: "tree",
  graph: "graph",
  analytics: "main",
  image: "file",
  preview: "preview",
};

/**
 * The id a descriptor claims. Two opens that name the same view produce the
 * same id, which is what makes "open this file" idempotent — the second one
 * focuses the tab the first one created instead of stacking a duplicate.
 */
export function mainTabId(descriptor: MainTabDescriptor): string {
  switch (descriptor.kind) {
    case "agent":
      return AGENT_TAB_ID;
    case "editor":
      return `editor:${descriptor.editorSession?.sessionId ?? ""}`;
    case "shell":
      return descriptor.shellScope === "repo" ? "shell:repo" : "shell";
    case "file":
      return `file:${descriptor.filePath ?? ""}`;
    case "preview":
      return `preview:${descriptor.portName ?? ""}`;
    case "image":
      return `image:${descriptor.imageUrl ?? ""}`;
    default:
      // One per scope: the diff, the tree, the graph, and analytics.
      return descriptor.kind;
  }
}

export function mainTabShortcutContext(kind: MainTabKind): ShortcutContext {
  return TAB_SHORTCUT_CONTEXTS[kind];
}

/** The tab-set key for one task. The single spelling both sides agree on. */
export function mainTabScopeKeyForTask(taskId: string): string {
  return `item:${taskId}`;
}

/**
 * The tab-set key for a repository with no task on screen. Repo-scoped views —
 * the commit graph, analytics, a repo-root shell, the tree explorer — belong
 * to the repository, not to whichever task happened to be selected when they
 * were opened, so they get a tab set of their own.
 */
export function mainTabScopeKeyForRepo(repoId: string): string {
  return `repo:${repoId}`;
}

/**
 * The tab set for a window with no repository selected at all — the first-run
 * state. It exists so the main content area always has exactly one place views
 * open into: with no app scope, the handful of surfaces reachable before a
 * repository is added (a home shell and the tree explorer) would
 * need a second, parallel rendering path that nothing else exercises.
 */
export function mainTabScopeKeyForApp(): string {
  return "app";
}

/** Whether a scope is a task's (and therefore owns an agent session tab). */
export function isTaskScopeKey(key: string | null): boolean {
  return key?.startsWith("item:") === true;
}

interface UseMainTabsOptions {
  /**
   * Identifies the tab set: the selected task, else the selected repository,
   * else the app itself. Scoping views this way is what makes switching tasks
   * restore that task's tabs rather than carry one task's views onto another,
   * and what keeps a repository's commit graph the repository's rather than
   * whichever task happened to be selected when it was opened.
   */
  scopeKey: ComputedRef<string | null>;
  /**
   * Called for every tab that closes, however it was closed — the tab's own
   * button, its shortcut, Escape. Consequences of closing a view belong here
   * rather than at one call site, because they were silently skipped for the
   * other two when they lived in the panel.
   */
  onTabClosed?: (tab: MainTab) => void;
}

export function useMainTabs({ scopeKey, onTabClosed }: UseMainTabsOptions) {
  const scopes = reactive<Record<string, MainTabScopeState>>({});

  function agentTab(): MainTab {
    return { id: AGENT_TAB_ID, kind: "agent" };
  }

  /**
   * A task scope opens on its agent session; a repository scope has none, so
   * it starts empty and the main area keeps showing its own empty state until
   * something is opened there.
   */
  function initialScopeState(key: string): MainTabScopeState {
    return isTaskScopeKey(key)
      ? { tabs: [agentTab()], activeId: AGENT_TAB_ID }
      : { tabs: [], activeId: "" };
  }

  function scopeState(key: string): MainTabScopeState {
    scopes[key] ??= initialScopeState(key);
    return scopes[key];
  }

  const tabs = computed<MainTab[]>(() => {
    const key = scopeKey.value;
    if (!key) return [];
    const state = scopes[key];
    if (!state) return initialScopeState(key).tabs;
    if (!state.layout) return state.tabs;
    const byId = new Map(state.tabs.map(tab => [tab.id, tab]));
    return paneLeaves(state.layout).flatMap(pane => pane.tabs)
      .map(id => byId.get(id)).filter((tab): tab is MainTab => !!tab);
  });

  const activeTabId = computed<string>(() => {
    const key = scopeKey.value;
    if (!key) return "";
    return scopes[key]?.activeId ?? initialScopeState(key).activeId;
  });

  const activeTab = computed<MainTab | null>(() =>
    tabs.value.find((tab) => tab.id === activeTabId.value) ?? null
  );

  function ensureLayout(state: MainTabScopeState): TaskPaneLayout {
    state.layout ??= restorePaneLayout(null, state.tabs.map(tab => tab.id), state.activeId);
    return state.layout;
  }
  const panes = computed(() => {
    const key = scopeKey.value;
    if (!key) return [];
    const state = scopes[key] ?? initialScopeState(key);
    return paneRects(state.layout ?? restorePaneLayout(null, state.tabs.map(tab => tab.id), state.activeId));
  });
  const dividers = computed(() => {
    const layout = scopeKey.value ? scopes[scopeKey.value]?.layout : undefined;
    return layout ? splitRects(layout) : [];
  });
  function resizePane(path: string, ratio: number) {
    if (scopeKey.value && Number.isFinite(ratio)) resizeSplit(ensureLayout(scopeState(scopeKey.value)), path, ratio);
  }
  function focusPane(id: string) {
    if (!scopeKey.value) return;
    const state = scopeState(scopeKey.value);
    const pane = paneLeaves(ensureLayout(state)).find(pane => pane.id === id);
    if (!pane) return;
    state.focusedPane = id;
    if (pane.active) state.activeId = pane.active;
  }
  function splitPane(id: string, axis: 'horizontal' | 'vertical', tabId?: string) {
    if (!scopeKey.value) return;
    const state = scopeState(scopeKey.value), layout = ensureLayout(state);
    const leaves = paneLeaves(layout), pane = leaves.find(pane => pane.id === id);
    if (!pane) return;
    if (tabId && !state.tabs.some(tab => tab.id === tabId)) return;
    let n = 1;
    while (leaves.some(pane => pane.id === `pane-${n}`)) n++;
    const moving = tabId ?? (pane.tabs.length > 1 && pane.active !== AGENT_TAB_ID ? pane.active : '');
    if (moving) for (const source of leaves) {
      source.tabs = source.tabs.filter(tab => tab !== moving);
      if (source.active === moving) source.active = source.tabs[0] ?? '';
    }
    const added = { kind: 'pane' as const, id: `pane-${n}`, tabs: moving ? [moving] : [], active: moving };
    state.layout = replacePane(layout, id, { kind: 'split', axis, ratio: .5, first: pane, second: added });
    focusPane(added.id);
  }
  function moveTab(id: string, paneId: string, beforeId?: string) {
    if (!scopeKey.value) return;
    const state = scopeState(scopeKey.value), layout = ensureLayout(state);
    const leaves = paneLeaves(layout), target = leaves.find(pane => pane.id === paneId);
    if (!target || !state.tabs.some(tab => tab.id === id) || id === beforeId) return;
    for (const pane of leaves) {
      pane.tabs = pane.tabs.filter(tab => tab !== id);
      if (pane.active === id) pane.active = pane.tabs[0] ?? '';
    }
    const index = beforeId ? target.tabs.indexOf(beforeId) : -1;
    target.tabs.splice(index < 0 ? target.tabs.length : index, 0, id);
    target.active = id;
    state.layout = removeEmptyPanes(layout);
    focusPane(paneId);
  }
  function closePane(id: string) {
    if (!scopeKey.value) return;
    const state = scopeState(scopeKey.value);
    const layout = ensureLayout(state);
    function remove(node: TaskPaneLayout): TaskPaneLayout {
      if (node.kind === 'pane') return node;
      const closing = node.first.kind === 'pane' && node.first.id === id ? node.first
        : node.second.kind === 'pane' && node.second.id === id ? node.second : null;
      if (closing) {
        const sibling = node.first === closing ? node.second : node.first;
        const neighbor = paneLeaves(sibling)[0];
        neighbor.tabs.push(...closing.tabs);
        if (closing.tabs.includes(state.activeId) || !neighbor.active) neighbor.active = closing.active || neighbor.tabs[0] || '';
        if (state.focusedPane === id) state.focusedPane = neighbor.id;
        return sibling;
      }
      return { ...node, first: remove(node.first), second: remove(node.second) };
    }
    state.layout = remove(layout);
  }
  function joinPanes() {
    if (!scopeKey.value) return;
    const state = scopeState(scopeKey.value);
    state.layout = restorePaneLayout(null, paneLeaves(ensureLayout(state)).flatMap(pane => pane.tabs), state.activeId);
    state.focusedPane = 'pane-1';
  }

  const activeTabContext = computed<ShortcutContext | null>(() => {
    const tab = activeTab.value;
    return tab ? mainTabShortcutContext(tab.kind) : null;
  });

  /** True when this scope owns an agent session tab, i.e. it is a task's. */
  const hasAgentTab = computed(() => isTaskScopeKey(scopeKey.value));

  const referenceTabId = computed(() => {
    const key = scopeKey.value;
    const id = key ? scopes[key]?.referenceId : undefined;
    return tabs.value.find(tab => tab.id === id && tab.kind !== "agent")?.id ?? "";
  });
  const split = computed(() => panes.value.length > 1);
  function setSplit(value: boolean): void {
    if (!value) joinPanes();
    else if (panes.value.length === 1) splitPane(panes.value[0].pane.id, 'horizontal', referenceTabId.value || undefined);
  }
  function updateReading(id: string, reading: NonNullable<MainTabDescriptor["reading"]>): void {
    const tab = tabs.value.find(tab => tab.id === id);
    if (tab) tab.reading = reading;
  }

  function isOpen(id: string): boolean {
    return tabs.value.some((tab) => tab.id === id);
  }

  function activateTab(id: string): void {
    const key = scopeKey.value;
    if (!key) return;
    // Mounting the initial terminal focuses Agent before persistence hydrates.
    // That no-op must not create a scope that would mask the saved layout.
    if (id === activeTabId.value) return;
    const state = scopeState(key);
    if (!state.tabs.some((tab) => tab.id === id)) return;
    state.activeId = id;
    const pane = paneLeaves(ensureLayout(state)).find(pane => pane.tabs.includes(id));
    if (pane) { pane.active = id; state.focusedPane = pane.id; }
    if (id !== AGENT_TAB_ID) state.referenceId = id;
  }

  /** Opens the view, or focuses it when it is already open. Returns its id. */
  function openTab(descriptor: MainTabDescriptor, options?: { activate?: boolean }): string | null {
    const key = scopeKey.value;
    if (!key) return null;
    return openTabInScope(key, descriptor, options);
  }

  /**
   * Opens a view in a named task's tab set, which need not be the selected
   * one. This is what a server-driven "open this" command uses: it records the
   * view against the task it belongs to instead of pulling the operator's
   * window onto another task, so the tab is simply there when they look.
   */
  function openTabInScope(
    key: string,
    descriptor: MainTabDescriptor,
    options?: { activate?: boolean },
  ): string {
    const state = scopeState(key);
    const layout = ensureLayout(state);
    const id = mainTabId(descriptor);
    const existing = state.tabs.findIndex((tab) => tab.id === id);
    if (existing === -1) {
      state.tabs.push({ ...descriptor, id });
      const leaves = paneLeaves(layout);
      const pane = leaves.find(pane => pane.id === state.focusedPane) ?? leaves[0];
      pane.tabs.push(id);
      if (options?.activate !== false || !pane.active) pane.active = id;
    } else if (descriptor.kind !== "agent") {
      // Re-opening a file at a different line re-aims the tab that is
      // already showing it rather than leaving the reader where they were.
      state.tabs[existing] = {
        reading: descriptor.initialLine === undefined ? state.tabs[existing].reading : undefined,
        ...descriptor,
        id,
      };
    }
    if (options?.activate !== false) {
      state.activeId = id;
      if (id !== AGENT_TAB_ID) state.referenceId = id;
      const pane = paneLeaves(layout).find(pane => pane.tabs.includes(id));
      if (pane) { pane.active = id; state.focusedPane = pane.id; }
    }
    return id;
  }

  function closeTab(id: string): void {
    const key = scopeKey.value;
    if (!key || id === AGENT_TAB_ID) return;
    const state = scopes[key];
    if (!state) return;
    const index = state.tabs.findIndex((tab) => tab.id === id);
    if (index === -1) return;
    const [closed] = state.tabs.splice(index, 1);
    if (state.layout) {
      for (const pane of paneLeaves(state.layout)) {
        pane.tabs = pane.tabs.filter(tab => tab !== id);
        if (pane.active === id) pane.active = pane.tabs[0] ?? '';
      }
      state.layout = removeEmptyPanes(state.layout);
    }
    if (closed) onTabClosed?.(closed);
    if (state.referenceId === id) state.referenceId = state.tabs.find(tab => tab.kind !== "agent")?.id;
    if (state.activeId !== id) return;
    // Closing the active tab moves to the tab that took its place — the one to
    // its right — and falls back to its left neighbour when it was last. In a
    // task scope the agent session is leftmost, so closing the only open view
    // lands there; a repository scope simply runs out of tabs.
    state.activeId = (state.tabs[index] ?? state.tabs[index - 1])?.id ?? "";
    const activePane = state.layout && paneLeaves(state.layout).find(pane => pane.tabs.includes(state.activeId));
    if (activePane) { activePane.active = state.activeId; state.focusedPane = activePane.id; }
    if (state.activeId !== AGENT_TAB_ID) state.referenceId = state.activeId;
  }

  function cycleTab(direction: -1 | 1): void {
    const ordered = tabs.value;
    if (ordered.length < 2) return;
    const current = ordered.findIndex((tab) => tab.id === activeTabId.value);
    const next = (current + direction + ordered.length) % ordered.length;
    activateTab(ordered[next].id);
  }

  /**
   * Close the tab in front, the way ⌘W closes a tab everywhere else. Returns
   * false when there is nothing to close — an agent session, which belongs to
   * the task rather than being a view of it, or an empty scope — and the
   * caller closes the window instead.
   */
  function closeActiveTab(): boolean {
    const id = activeTabId.value;
    if (!id || id === AGENT_TAB_ID) return false;
    closeTab(id);
    return true;
  }

  /** The tab sets worth restoring, in the stored shape. */
  function snapshotScopes(): PersistedMainTabs {
    const persisted: Record<string, PersistedMainTabScope> = {};
    for (const [key, state] of Object.entries(scopes)) {
      const tabs = state.tabs.filter(isRestorableTab).map(persistedDescriptor);
      if (tabs.length === 0) continue;
      // An active tab that is not being stored cannot be restored as active
      // either; the scope reopens on its own default instead of on a tab the
      // reader would find missing.
      const activeId = tabs.some((tab) => mainTabId(tab) === state.activeId) ? state.activeId : "";
      persisted[key] = { tabs, activeId,
        layout: state.layout,
        referenceId: tabs.some(tab => mainTabId(tab) === state.referenceId) ? state.referenceId : undefined,
        split: state.split,
      };
    }
    return { version: PERSISTED_MAIN_TABS_VERSION, scopes: persisted };
  }

  /**
   * Rebuilds stored tab sets, keeping whatever is already open.
   *
   * `isLiveScope` decides which stored scopes still have something to be a
   * view of: a task closed while the app was shut down should not come back as
   * a tab set. Restoring never disturbs a scope the reader has already touched
   * this session.
   */
  function restoreScopes(
    persisted: PersistedMainTabs | null,
    isLiveScope: (key: string) => boolean = () => true,
  ): void {
    if (!persisted) return;
    for (const [key, stored] of Object.entries(persisted.scopes)) {
      if (scopes[key] || !isLiveScope(key)) continue;
      const state = initialScopeState(key);
      for (const descriptor of stored.tabs) {
        const id = mainTabId(descriptor);
        if (state.tabs.some((tab) => tab.id === id)) continue;
        state.tabs.push({ ...descriptor, id });
      }
      const active = state.tabs.find((tab) => tab.id === stored.activeId);
      state.activeId = active?.id ?? state.tabs[0]?.id ?? "";
      state.referenceId = state.tabs.find(tab => tab.id === stored.referenceId && tab.kind !== "agent")?.id
        ?? (active?.kind !== "agent" ? active?.id : undefined);
      state.split = stored.split;
      state.layout = restorePaneLayout(stored.layout, state.tabs.map(tab => tab.id), state.activeId);
      const activePane = paneLeaves(state.layout).find(pane => pane.tabs.includes(state.activeId));
      if (activePane) { activePane.active = state.activeId; state.focusedPane = activePane.id; }
      scopes[key] = state;
    }
  }

  return {
    panes,
    dividers,
    resizePane,
    focusPane,
    splitPane,
    moveTab,
    joinPanes,
    closePane,
    scopeKey,
    referenceTabId,
    split,
    setSplit,
    updateReading,
    tabs,
    activeTabId,
    activeTab,
    activeTabContext,
    hasAgentTab,
    isOpen,
    openTab,
    openTabInScope,
    closeTab,
    closeActiveTab,
    activateTab,
    cycleTab,
    snapshotScopes,
    restoreScopes,
  };
}

export type MainTabsController = ReturnType<typeof useMainTabs>;
