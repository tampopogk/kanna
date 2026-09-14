/**
 * Honouring one `kanna_open_view` command.
 *
 * The server has already resolved the target against the task's worktree and
 * is waiting for this window to say whether it is on screen, so the shape of
 * the work is fixed: select the task, open (or re-aim) its tab, wait for the
 * view to render its target, and answer. Every step that can fail answers with
 * a code rather than throwing, because a command nobody answers is reported to
 * the caller as an unavailable desktop — which would be a lie about a window
 * that was right there and simply could not find the task.
 */

import { mainTabScopeKeyForTask, type MainTabsController, type MainTabDescriptor } from "./useMainTabs";

/**
 * The native event carrying one command. Addressed to a single window rather
 * than broadcast, so it is listened for on this webview rather than globally.
 */
export const DESKTOP_VIEW_OPEN_EVENT = "desktop-view-open";

export const DESKTOP_VIEW_KINDS = [
  "agent",
  "file",
  "diff",
  "tree",
  "graph",
  "analytics",
] as const;

export type DesktopViewKind = (typeof DESKTOP_VIEW_KINDS)[number];

export interface DesktopViewOpenCommand {
  requestId: string;
  taskId: string;
  view: DesktopViewKind;
  target?: Record<string, unknown>;
  operation?: "inspect" | "split" | "move";
  branch?: string;
  windowId?: string;
  workspaceId?: string;
  paneId?: string;
  tabId?: string;
  direction?: "horizontal" | "vertical";
  expiresAt?: number;
}

export interface DesktopViewOpenOutcome {
  opened: boolean;
  workspace?: DesktopWorkspaceSnapshot;
  paneId?: string;
  tabId?: string;
  code?: string;
  message?: string;
}

export interface DesktopWorkspaceSnapshot {
  taskId: string;
  branch: string;
  windowId: string;
  workspaceId: string;
  focusedPaneId: string | null;
  activeTabId: string;
  displayedPanes: Array<{ id: string; left: number; top: number; width: number; height: number; activeTabId: string }>;
  panes: Array<{
    id: string;
    order: number;
    left: number; top: number; width: number; height: number;
    activeTabId: string;
    tabs: Array<{ id: string; kind: string; filePath?: string }>;
  }>;
}

function isViewKind(value: unknown): value is DesktopViewKind {
  return DESKTOP_VIEW_KINDS.includes(value as DesktopViewKind);
}

/**
 * Read a command off the native event, refusing anything this window does not
 * understand rather than guessing at it. A window that cannot read the command
 * cannot acknowledge it either — it has no request id to answer with — so the
 * caller learns of it as an unavailable desktop.
 */
export function parseDesktopViewOpenCommand(payload: unknown): DesktopViewOpenCommand {
  const command = payload as Partial<DesktopViewOpenCommand> | null;
  if (
    !command
    || typeof command.requestId !== "string"
    || command.requestId.length === 0
    || typeof command.taskId !== "string"
    || command.taskId.length === 0
    || !isViewKind(command.view)
  ) {
    throw new Error("malformed desktop view open command");
  }
  for (const key of ["branch", "windowId", "workspaceId", "paneId", "tabId"] as const) {
    if (command[key] !== undefined && (typeof command[key] !== "string" || !command[key])) throw new Error(`malformed ${key}`);
  }
  if (command.operation !== undefined && !["inspect", "split", "move"].includes(command.operation)) throw new Error("malformed workspace operation");
  if (command.direction !== undefined && !["horizontal", "vertical"].includes(command.direction)) throw new Error("malformed split direction");
  if (command.expiresAt !== undefined && (typeof command.expiresAt !== "number" || !Number.isFinite(command.expiresAt))) throw new Error("malformed expiry");
  const target = command.target;
  if (target !== undefined && (typeof target !== "object" || target === null || Array.isArray(target))) {
    throw new Error("malformed desktop view open target");
  }
  return {
    requestId: command.requestId,
    taskId: command.taskId,
    view: command.view,
    target: target as Record<string, unknown> | undefined,
    ...Object.fromEntries(["operation", "branch", "windowId", "workspaceId", "paneId", "tabId", "direction", "expiresAt"]
      .filter(key => key in command).map(key => [key, command[key as keyof DesktopViewOpenCommand]])),
  };
}

/**
 * The tab a command asks for. A `file` tab is identified by its path, so
 * re-opening the same file at another line re-aims the tab that is already
 * showing it; every other view is one per task.
 */
export function mainTabDescriptorForCommand(command: DesktopViewOpenCommand): MainTabDescriptor {
  if (command.view === "file") {
    const path = command.target?.path;
    const line = command.target?.line;
    return {
      kind: "file",
      filePath: typeof path === "string" ? path : "",
      initialLine: typeof line === "number" ? line : undefined,
      // The server validated this path inside the task's worktree; the view
      // must read it back the same way rather than off the filesystem.
      containedTaskId: command.taskId,
    };
  }
  if (command.view === "tree") {
    return { kind: "tree", containedTaskId: command.taskId };
  }
  return { kind: command.view };
}

export interface DesktopViewOpenDeps {
  /** Production controller, with the selected local workspace checked at each async boundary. */
  workspace?: {
    tabs: MainTabsController;
    windowId: string;
    currentBranch: (taskId: string) => string | null;
    rendered: () => Promise<void>;
    presentation: () => DesktopWorkspaceSnapshot["displayedPanes"];
  };
  /** The selectable sidebar row for a task id, if this window has one. */
  findTaskSlotId: (taskId: string) => string | null;
  /** One reload, for a task this window has not heard about yet. */
  refreshTasks: () => Promise<void>;
  selectTask: (slotId: string) => Promise<void>;
  /** Open or re-aim the tab in the task's own scope; returns its id. */
  openTab: (scopeKey: string, descriptor: MainTabDescriptor) => string;
  /**
   * Wait for that tab's view to be showing, and for its target to be revealed
   * inside it. This is the step that makes `opened: true` a statement about a
   * screen rather than about a queue.
   */
  revealTab: (tabId: string, command: DesktopViewOpenCommand) => Promise<DesktopViewOpenOutcome>;
}

export async function performDesktopViewOpen(
  command: DesktopViewOpenCommand,
  deps: DesktopViewOpenDeps,
): Promise<DesktopViewOpenOutcome> {
  if (command.expiresAt && Date.now() >= command.expiresAt) return { opened: false, code: "request_expired", message: "the desktop request expired" };
  if (command.windowId && deps.workspace && command.windowId !== deps.workspace.windowId)
    return { opened: false, code: "window_not_found", message: "the command was delivered to a different window" };
  let slotId = deps.findTaskSlotId(command.taskId);
  if (slotId === null) {
    // The server resolved this task a moment ago, so a window that has not
    // heard of it is behind rather than wrong. One reload, then take the
    // answer: retrying past that would keep the caller waiting for a task this
    // window is never going to show.
    try {
      await deps.refreshTasks();
    } catch (error: unknown) {
      console.error("[desktop-view-open] refreshing tasks failed:", error);
    }
    slotId = deps.findTaskSlotId(command.taskId);
  }
  if (slotId === null) {
    return {
      opened: false,
      code: "task_not_found",
      message: `this window has no task ${command.taskId}`,
    };
  }

  try {
    await deps.selectTask(slotId);
  } catch (error: unknown) {
    return {
      opened: false,
      code: "renderer_failed",
      message: `selecting the task failed: ${error instanceof Error ? error.message : String(error)}`,
    };
  }

  const workspace = deps.workspace;
  const scope = mainTabScopeKeyForTask(command.taskId);
  const failure = (code: string, message: string): DesktopViewOpenOutcome => ({ opened: false, code, message });
  function check(): DesktopViewOpenOutcome | null {
    if (command.expiresAt && Date.now() >= command.expiresAt) return failure("request_expired", "the desktop request expired");
    if (!workspace) return command.operation || command.paneId
      ? failure("renderer_failed", "this desktop does not support pane controls") : null;
    if (command.windowId && command.windowId !== workspace.windowId) return failure("window_not_found", "the requested window is unavailable");
    if (!command.branch || workspace.currentBranch(command.taskId) !== command.branch || workspace.tabs.scopeKey.value !== scope)
      return failure("workspace_unavailable", "the task's current local workspace is not selected");
    if (command.workspaceId && command.workspaceId !== workspace.tabs.workspaceIdentity(scope, command.branch))
      return failure("stale_workspace", "inspect the current workspace before addressing its panes");
    if (command.paneId && !workspace.tabs.panes.value.some(({ pane }) => pane.id === command.paneId))
      return failure("pane_not_found", "the requested pane no longer exists in this workspace");
    return null;
  }
  function snapshot(): DesktopWorkspaceSnapshot {
    const tabs = workspace!.tabs;
    const panes = tabs.panes.value;
    return {
      taskId: command.taskId, branch: command.branch!, windowId: workspace!.windowId,
      workspaceId: tabs.workspaceIdentity(scope, command.branch!),
      focusedPaneId: tabs.focusedPaneId.value,
      activeTabId: tabs.activeTabId.value,
      displayedPanes: workspace!.presentation(),
      // Same depth-first order and percentage rectangles MainPanel renders.
      panes: panes.map(({ pane, ...rect }, index) => ({
        id: pane.id, order: index + 1, ...rect, activeTabId: pane.active,
        tabs: pane.tabs.map(id => {
          const tab = tabs.tabs.value.find(tab => tab.id === id)!;
          return { id, kind: tab.kind, ...(tab.filePath ? { filePath: tab.filePath } : {}) };
        }),
      })),
    };
  }
  try {
    if (workspace) await workspace.rendered();
    const invalid = check();
    if (invalid) return invalid;
    let paneId = command.paneId;
    let tabId = command.tabId;
    if (command.operation) {
      const tabs = workspace!.tabs;
      if (tabId && !tabs.tabs.value.some(tab => tab.id === tabId)) return failure("tab_not_found", "the requested tab does not belong to this workspace");
      if (command.operation === "split") {
        if (!paneId || !command.direction) return failure("invalid_target", "split needs a pane and direction");
        paneId = tabs.splitPane(paneId, command.direction, tabId);
        tabId = tabs.panes.value.find(({ pane }) => pane.id === paneId)?.pane.active || undefined;
      } else if (command.operation === "move") {
        if (!paneId || !tabId) return failure("invalid_target", "move needs a pane and tab");
        tabs.moveTab(tabId, paneId);
      }
    } else {
      // Validate the destination before creating or re-aiming anything.
      tabId = deps.openTab(scope, mainTabDescriptorForCommand(command));
      if (paneId) workspace!.tabs.moveTab(tabId, paneId);
      const outcome = await deps.revealTab(tabId, command);
      if (!outcome.opened) return outcome;
    }
    if (!workspace) return { opened: true };
    await workspace.rendered();
    // A task switch/stage transition during a view's load must never confirm a different screen.
    const changed = check();
    if (changed) return changed;
    const result = snapshot();
    if (tabId) {
      const destination = result.panes.find(pane => pane.tabs.some(tab => tab.id === tabId));
      if (!destination || destination.activeTabId !== tabId || (paneId && destination.id !== paneId)
        || !result.displayedPanes.some(pane => pane.id === destination.id && pane.activeTabId === tabId))
        return failure("renderer_failed", "the requested tab is no longer showing in the destination pane");
      paneId = destination.id;
    }
    if (paneId && !result.panes.some(pane => pane.id === paneId)) return failure("pane_not_found", "the destination pane disappeared");
    return { opened: true, workspace: result, ...(paneId ? { paneId } : {}), ...(tabId ? { tabId } : {}) };
  } catch (error: unknown) {
    return failure("renderer_failed", `showing the workspace failed: ${error instanceof Error ? error.message : String(error)}`);
  }
}

/**
 * Wait for a view to be ready, with a ceiling.
 *
 * The caller is holding an HTTP request open on the answer, so a view that
 * never finishes loading has to become a "no" rather than an indefinite wait.
 * The ceiling is below the route's own timeout, so a slow view is reported as
 * a slow view instead of arriving after the caller has already given up.
 */
export async function waitForViewReady(
  isReady: () => boolean,
  { timeoutMs = 8_000, stepMs = 50 }: { timeoutMs?: number; stepMs?: number } = {},
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (!isReady()) {
    if (Date.now() >= deadline) return false;
    await new Promise((resolve) => setTimeout(resolve, stepMs));
  }
  return true;
}

/** The target of a `file` view command, as the viewer needs it. */
export interface FileViewTarget {
  path: string;
  line?: number;
  column?: number;
  endLine?: number;
  endColumn?: number;
}

export function fileViewTarget(command: DesktopViewOpenCommand): FileViewTarget | null {
  const target = command.target;
  if (!target || typeof target.path !== "string") return null;
  const number = (value: unknown): number | undefined =>
    typeof value === "number" && Number.isFinite(value) && value > 0 ? value : undefined;
  return {
    path: target.path,
    line: number(target.line),
    column: number(target.column),
    endLine: number(target.endLine),
    endColumn: number(target.endColumn),
  };
}

/** The target of a `diff` view command: one line of one side of one file. */
export interface DiffViewTarget {
  scope: "branch" | "working";
  path?: string;
  side?: "old" | "new";
  /** Which side(s) number the anchored line, resolved by the server. */
  anchorKind?: "context" | "addition" | "deletion";
  oldLine?: number;
  newLine?: number;
  /**
   * A short piece of the anchored line as the server read it. The view checks
   * the rendered row still carries it, because line numbers survive an edit
   * that replaces the line.
   */
  excerpt?: string;
}

export function diffViewTarget(command: DesktopViewOpenCommand): DiffViewTarget {
  const target = command.target ?? {};
  const number = (value: unknown): number | undefined =>
    typeof value === "number" && Number.isFinite(value) && value > 0 ? value : undefined;
  const side = target.side === "old" || target.side === "new" ? target.side : undefined;
  const anchorKind = target.anchorKind === "context"
    || target.anchorKind === "addition"
    || target.anchorKind === "deletion"
    ? target.anchorKind
    : undefined;
  return {
    scope: target.scope === "working" ? "working" : "branch",
    path: typeof target.path === "string" ? target.path : undefined,
    side,
    anchorKind,
    oldLine: number(target.oldLine),
    newLine: number(target.newLine),
    excerpt: typeof target.excerpt === "string" && target.excerpt.length > 0
      ? target.excerpt
      : undefined,
  };
}
