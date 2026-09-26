<script setup lang="ts">
import {
  computed,
  nextTick,
  onMounted,
  onBeforeUnmount,
  ref,
  watch,
  type ComponentPublicInstance,
  type Ref,
} from "vue";
import { AGENT_PROVIDERS, getAgentProviderSpec } from "@kanna/agent-protocol";
import type { ArtifactReference } from "@kanna/core";
import type { AgentProvider, BlockerDisplayItem } from "../types/kanna";
import type { TaskUiSlot } from "../types/taskUi";
import {
  fetchDesktopTaskDetail,
  listDesktopTaskDirectory,
  listAgentTerminalAttempts,
  listWorkspaceSetupRuns,
  readAgentTerminalArchive,
  readDesktopTaskFile,
  type AgentTerminalArchive,
  type AgentTerminalAttempt,
  type DesktopTaskDetail,
  type WorkspaceSetupRun,
} from "../services/desktopServerClient";
import { CREATION_SELECTION, parseStageHistorySelection } from "../utils/agentStageHistory";
import { isBlockerResolved } from "../utils/blockerResolution";
import { isRemotePresentationTaskId } from "../utils/remoteTaskIdentity";
import { invoke } from "../invoke";
import TaskPreviewCache from "./TaskPreviewCache.vue";
import TaskHeader from "./TaskHeader.vue";
import TerminalTabs from "./TerminalTabs.vue";
import AgentHistoryView from "./AgentHistoryView.vue";
import TaskCreationProgress from "./TaskCreationProgress.vue";
import WorkspaceSetupLogView from "./WorkspaceSetupLogView.vue";
import MainTabBar from "./MainTabBar.vue";
import { usePaneTabDrag } from "../composables/usePaneTabDrag";
import DiffModal from "./DiffModal.vue";
import FilePreviewModal from "./FilePreviewModal.vue";
import ShellModal from "./ShellModal.vue";
import TerminalEditorView from "./TerminalEditorView.vue";
import { openTerminalEditor } from "../services/desktopServerClient";
import TreeExplorerModal from "./TreeExplorerModal.vue";
import CommitGraphModal from "./CommitGraphModal.vue";
import AnalyticsModal from "./AnalyticsModal.vue";
import ImageUrlPreviewModal from "./ImageUrlPreviewModal.vue";
import ArtifactViewer from "./ArtifactViewer.vue";
import { AGENT_TAB_ID, mainTabScopeKeyForTask, type MainTab } from "../composables/useMainTabs";
import type { RemoteDirectoryEntry } from "../composables/useTreeExplorer";
import type { SplitRect } from "../composables/taskPaneLayout";
import {
  waitForViewReady,
  type DesktopViewOpenCommand,
  type DesktopViewOpenOutcome,
} from "../composables/desktopViewOpen";
import type { MainTabViewsController } from "./MainPanel.types";
import type { BranchInclude, DiffScope, DiffScrollPositions } from "../composables/useAppModals";
import type { MarkdownPreviewMode } from "../stores/markdownPreviewMode";
import { shortcutHint, shortcutHintKeys } from "../composables/useKeyboardShortcuts";
import { refocusActiveTerminal } from "../composables/useTerminalFocusWhenActive";
import CloudTerminalCache, {
  type CloudTerminalCacheEntry,
} from "./CloudTerminalCache.vue";

const props = defineProps<{
  uiSlot: TaskUiSlot | null;
  repoPath?: string;
  spawnPtySession?: (sessionId: string, cwd: string, prompt: string, cols: number, rows: number) => Promise<void>;
  recoverTaskSession?: (sessionId: string, options?: { cols?: number; rows?: number }) => Promise<void>;
  blockers?: BlockerDisplayItem[];
  blocked?: boolean;
  hasRepos?: boolean;
  cloudTask?: boolean;
  cloudTerminalRef?: {
    ownerDesktopId: string;
    ownerLocalTaskId: string;
    transport?: "cloud" | "lan";
  } | null;
  /**
   * Present in the app; absent in isolated tests, where the panel is just the
   * agent session it has always been.
   */
  views?: MainTabViewsController;
}>();

const emit = defineEmits<{
  (e: "back"): void;
}>();

const isMobile = __KANNA_MOBILE__;
const COMMAND_HINT_STORAGE_KEY = "kanna:hide-command-hint";
const TERMINAL_EDITOR_NOTICE_SETTING_KEY = "hideTerminalEditorNotice";
const item = computed(() => props.uiSlot?.task ?? null);
const selectedAttempt = ref("");
const agentAttempts = ref<AgentTerminalAttempt[]>([]);
const completedCreationTask = ref("");
const creationComplete = computed(() => !props.cloudTask && !!completedCreationTask.value
  && completedCreationTask.value === (props.uiSlot?.draft.creation_task_id || props.uiSlot?.task_id));
function creationCompleted(status: string | null) {
  completedCreationTask.value = status ? (props.uiSlot?.draft.creation_task_id || props.uiSlot?.task_id || "") : "";
  if (status === "failed") selectedAttempt.value = CREATION_SELECTION;
}
const agentSetupRuns = ref<WorkspaceSetupRun[]>([]);
const agentHistoryStatus = ref("");
let attemptsRequest = 0;
let setupRunsRequest = 0;
const agentHistoryRemoteRoute = computed(() => {
  const ref = props.cloudTerminalRef;
  if (!item.value || !ref || (!props.cloudTask && !isRemotePresentationTaskId(item.value.id))) return null;
  return {
    desktopId: ref.ownerDesktopId,
    taskId: ref.ownerLocalTaskId,
    transport: ref.transport ?? "cloud",
  };
});
const agentHistoryBindingTaskId = computed(() =>
  agentHistoryRemoteRoute.value?.taskId ?? item.value?.id ?? ""
);
const agentHistorySourceKey = computed(() => {
  const task = item.value;
  if (!task) return "none";
  const route = agentHistoryRemoteRoute.value;
  if (route) return `${route.transport}:${route.desktopId}:${route.taskId}`;
  return taskDetailIsLocal.value ? `local:${task.id}` : `remote-unavailable:${task.id}`;
});
function historyError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  return `History unavailable · ${message}`;
}
async function loadAgentAttempts(presentationTaskId: string, sourceKey: string) {
  const request = ++attemptsRequest;
  agentHistoryStatus.value = "Loading history…";
  try {
    const route = agentHistoryRemoteRoute.value;
    const attempts = route
      ? await props.views?.modals.listRemoteAgentTerminalAttempts(route)
      : taskDetailIsLocal.value
        ? await listAgentTerminalAttempts(presentationTaskId)
        : undefined;
    if (!attempts) throw new Error(route ? "remote task views unsupported" : "owner route unavailable");
    if (
      request === attemptsRequest
      && item.value?.id === presentationTaskId
      && agentHistorySourceKey.value === sourceKey
    ) {
      agentAttempts.value = attempts;
      agentHistoryStatus.value = "";
    }
  } catch (error) {
    if (
      request === attemptsRequest
      && item.value?.id === presentationTaskId
      && agentHistorySourceKey.value === sourceKey
    ) agentHistoryStatus.value = historyError(error);
    console.debug("[agent-history] attempt list unavailable", error);
  }
}
async function loadSelectedAgentArchive(runId: string): Promise<AgentTerminalArchive | null> {
  const route = agentHistoryRemoteRoute.value;
  if (route) {
    const loader = props.views?.modals.readRemoteAgentTerminalArchive;
    if (!loader) throw new Error("remote task views unsupported");
    return loader(route, runId);
  }
  if (!taskDetailIsLocal.value) throw new Error("owner route unavailable");
  return readAgentTerminalArchive(agentHistoryBindingTaskId.value, runId);
}
/**
 * Setup streams are served by the owner machine's local API alone: no LAN or
 * relay route carries them yet. A task presented from another desktop keeps an
 * empty list, so the selector simply offers no Setup item rather than showing
 * one that cannot be read.
 */
async function loadAgentSetupRuns(presentationTaskId: string, sourceKey: string) {
  const request = ++setupRunsRequest;
  if (agentHistoryRemoteRoute.value || !taskDetailIsLocal.value) return;
  try {
    const runs = await listWorkspaceSetupRuns(presentationTaskId);
    if (
      request === setupRunsRequest
      && item.value?.id === presentationTaskId
      && agentHistorySourceKey.value === sourceKey
    ) agentSetupRuns.value = runs;
  } catch (error) {
    console.debug("[agent-history] setup log list unavailable", error);
  }
}
function selectAttempt(id: string) { selectedAttempt.value = id; selectTab(AGENT_TAB_ID); }
// One selection token drives the agent tab: empty is the live session, a setup
// token names a stored setup stream, and anything else is a run id whose
// terminal archive the history view reads — an agent attempt or a teardown.
const historySelection = computed(() => parseStageHistorySelection(selectedAttempt.value));
const selectedArchiveRunId = computed(() =>
  historySelection.value.kind === "attempt" ? historySelection.value.runId : ""
);
const selectedSetupRunId = computed(() =>
  historySelection.value.kind === "setup" ? historySelection.value.runId : ""
);


const tabs = computed<MainTab[]>(() => props.views?.tabs.tabs.value ?? []);
const activeTabId = computed(() => props.views?.tabs.activeTabId.value ?? AGENT_TAB_ID);
const agentTabActive = computed(() => activeTabId.value === AGENT_TAB_ID);
const workArea = ref<HTMLElement | null>(null);
const workAreaWidth = ref(0);
let workAreaObserver: ResizeObserver | undefined;
onMounted(() => {
  workAreaObserver = new ResizeObserver(([entry]) => { workAreaWidth.value = entry.contentRect.width; });
  if (workArea.value) workAreaObserver.observe(workArea.value);
});
onBeforeUnmount(() => workAreaObserver?.disconnect());
const paneRects = computed(() => props.views?.tabs.panes.value ?? []);
const narrowLayout = computed(() => isMobile || workAreaWidth.value < 800);
const visiblePanes = computed(() => {
  // A creating slot has no task tab scope yet. Show its progress instead of
  // the previous repository's panes until the durable task is acknowledged.
  if (props.uiSlot?.state === "creating") return [];
  const maximizedPaneId = props.views?.tabs.maximizedPaneId.value;
  if (maximizedPaneId) {
    const maximized = paneRects.value.find((rect) => rect.pane.id === maximizedPaneId);
    return maximized
      ? [{ ...maximized, left: 0, top: 0, width: 100, height: 100 }]
      : paneRects.value;
  }
  if (!narrowLayout.value) return paneRects.value;
  const active = paneRects.value.find(rect => rect.pane.tabs.includes(activeTabId.value)) ?? paneRects.value[0];
  return active ? [{ ...active, pane: { ...active.pane, tabs: tabs.value.map(tab => tab.id), active: activeTabId.value }, left: 0, top: 0, width: 100, height: 100 }] : [];
});
const splitVisible = computed(() => visiblePanes.value.length > 1);
function viewVisible(id: string) {
  return !props.views ? id === AGENT_TAB_ID : visiblePanes.value.some(rect => rect.pane.active === id);
}
const agentVisible = computed(() => props.uiSlot?.state === "creating" || viewVisible(AGENT_TAB_ID));
// Pane geometry changes without reparenting content: moving an iframe in the
// DOM reloads it, and remounting terminal views loses their local reading state.
function tabStyle(id: string) {
  const rect = visiblePanes.value.find(rect => rect.pane.active === id);
  if (!rect) return {};
  return { position: 'absolute' as const, left: `${rect.left}%`, top: `calc(${rect.top}% + 34px)`, width: `${rect.width}%`, height: `calc(${rect.height}% - 34px)`, padding: '2px', boxSizing: 'border-box' as const, boxShadow: activeTabId.value === id ? 'inset 0 2px var(--kn-accent)' : undefined };
}
function paneStyle(rect: typeof visiblePanes.value[number]) {
  return { left: `${rect.left}%`, top: `${rect.top}%`, width: `${rect.width}%`, height: `${rect.height}%` };
}
function dividerStyle(rect: SplitRect) {
  return rect.axis === 'horizontal'
    ? { left: `calc(${rect.left + rect.width * rect.ratio}% - 3px)`, top: `${rect.top}%`, width: '6px', height: `${rect.height}%`, cursor: 'col-resize' }
    : { left: `${rect.left}%`, top: `calc(${rect.top + rect.height * rect.ratio}% - 3px)`, width: `${rect.width}%`, height: '6px', cursor: 'row-resize' };
}
function resizeDivider(event: PointerEvent, rect: SplitRect) {
  if (!(event.currentTarget instanceof HTMLElement) || !event.currentTarget.hasPointerCapture(event.pointerId)) return;
  const bounds = workArea.value?.getBoundingClientRect();
  if (!bounds) return;
  const ratio = rect.axis === 'horizontal'
    ? ((event.clientX - bounds.left) / bounds.width * 100 - rect.left) / rect.width
    : ((event.clientY - bounds.top) / bounds.height * 100 - rect.top) / rect.height;
  props.views?.tabs.resizePane(rect.path, ratio);
}
function captureDivider(event: PointerEvent) {
  if (event.currentTarget instanceof HTMLElement) event.currentTarget.setPointerCapture(event.pointerId);
}
function openPaneView(paneId: string, id: string, tabId?: string) {
  props.views?.tabs.focusPane(paneId);
  if (id === 'split-horizontal' || id === 'split-vertical') props.views?.tabs.splitPane(paneId, id === 'split-horizontal' ? 'horizontal' : 'vertical', tabId);
  else openNewView(id);
}
const tabDrag = usePaneTabDrag({
  scope: () => props.views?.tabs.scopeKey.value,
  target: (x, y) => {
    const bounds = workArea.value?.getBoundingClientRect();
    if (!bounds || narrowLayout.value) return null;
    const rect = visiblePanes.value.find(rect => x >= bounds.left + bounds.width * rect.left / 100
      && x < bounds.left + bounds.width * (rect.left + rect.width) / 100
      && y >= bounds.top + bounds.height * rect.top / 100
      && y < bounds.top + bounds.height * (rect.top + rect.height) / 100);
    if (!rect) return null;
    const bar = workArea.value?.querySelector(`[data-pane-id="${rect.pane.id}"]`);
    const overTab = Array.from(bar?.querySelectorAll<HTMLElement>('[data-tab-id]') ?? [])
      .find(tab => { const r = tab.getBoundingClientRect(); return y >= r.top && y < r.bottom && x < r.right; });
    // Midpoint decides before/after, so dragging the last tab right can reorder too.
    const index = overTab ? rect.pane.tabs.indexOf(overTab.dataset.tabId!) : -1;
    const box = overTab?.getBoundingClientRect();
    const beforeId = box && x >= box.left + box.width / 2
      ? rect.pane.tabs[index + 1] : overTab?.dataset.tabId;
    return { paneId: rect.pane.id, beforeId };
  },
  move: (id, target) => props.views?.tabs.moveTab(id, target.paneId, target.beforeId),
});
const ownerLabel = computed(() => props.cloudTerminalRef?.ownerDesktopId
  ?? (props.cloudTask ? "Owner unavailable" : "This machine"));
const previewCache = ref<InstanceType<typeof TaskPreviewCache> | null>(null);
const agentHistoryRef = ref<InstanceType<typeof AgentHistoryView> | null>(null);
const setupLogRef = ref<InstanceType<typeof WorkspaceSetupLogView> | null>(null);
const previewWorkspaces = computed(() => Object.fromEntries(
  (props.views?.store.items ?? []).filter(task => task.closed_at == null)
    .map(task => [task.id, props.views?.store.worktreePaths?.[task.id] ?? ""]),
));

const openViewTabs = computed(() => tabs.value.filter((tab) => tab.kind !== "agent"));
const visiblePreviews = computed(() => tabs.value.filter(tab => tab.kind === 'preview' && viewVisible(tab.id)).map(tab => ({
  key: tabKey(tab), taskId: item.value?.id ?? '', portName: tab.portName ?? '',
  workspace: props.views?.modals.activeWorktreePath.value ?? '',
  supported: taskDetailIsLocal.value && !isMobile && !props.views?.modals.activeTaskViewIsRemote.value,
  style: tabStyle(tab.id),
})));
const newViews = computed(() => [
  ...(props.uiSlot ? [{ id: "diff", label: "Diff", shortcut: shortcutHint("showDiff") }] : []),
  { id: "shell", label: "Shell", shortcut: shortcutHint("openShell") },
  { id: "tree", label: "File explorer", shortcut: shortcutHint("toggleTreeExplorer") },
  ...(scopeRepoPath.value ? [{ id: "graph", label: "Commit graph", shortcut: shortcutHint("showCommitGraph") }] : []),
]);
const paneActions = computed(() => narrowLayout.value ? [] : [
  { id: "split-horizontal", label: "Split side by side" },
  { id: "split-vertical", label: "Split top and bottom" },
]);
function openNewView(id: string) {
  if (id === "diff" || id === "shell" || id === "tree" || id === "graph") props.views?.tabs.openTab({ kind: id });
}

/** Opens a named artifact reference from the task's latest result (spec §16.8) in the existing artifact viewer. */
function openLatestResultArtifact(reference: Extract<ArtifactReference, { type: "stored" }>) {
  props.views?.tabs.openTab({ kind: "artifact", artifactRepoId: reference.repoId, artifactId: reference.artifactId });
}
/**
 * The panel's own empty state — "no task selected", or the agent-install help
 * when there are no repositories — belongs to a main area with nothing in it.
 * A repository scope with tabs open is not empty.
 */
const showEmptyState = computed(() => !props.uiSlot && tabs.value.length === 0);
const scopeRepoId = computed(() =>
  item.value?.repo_id ?? props.views?.store.selectedRepoId ?? null
);
const scopeRepoPath = computed(() =>
  props.repoPath ?? props.views?.store.selectedRepo?.path ?? ""
);
const taskWorktreePath = computed(() =>
  item.value ? (props.views?.store.worktreePaths?.[item.value.id]
    ?? (item.value.branch ? `${props.repoPath}/.kanna-worktrees/${item.value.branch}` : undefined)) : undefined
);

function selectTab(id: string) {
  props.views?.tabs.activateTab(id);
}

function closeTab(id: string) {
  props.views?.tabs.closeTab(id);
}

/** Move the caret with pane selection without manufacturing terminal input. */
async function focusActivePaneContent(paneId?: string) {
  await nextTick();
  const pane = visiblePanes.value.find((rect) => rect.pane.id === paneId)
    ?? visiblePanes.value.find((rect) => rect.pane.tabs.includes(activeTabId.value));
  const id = pane?.pane.active;
  const focusPaneChrome = () => {
    workArea.value?.querySelector<HTMLElement>(`[data-workspace-pane-id="${pane?.pane.id ?? paneId ?? ""}"]`)?.focus({ preventScroll: true });
  };
  if (!id) {
    focusPaneChrome();
    return;
  }
  if (id === AGENT_TAB_ID) {
    if (selectedAttempt.value) {
      const view = selectedSetupRunId.value ? setupLogRef.value : agentHistoryRef.value;
      if (!view?.focusContent()) focusPaneChrome();
      return;
    }
    refocusActiveTerminal();
    return;
  }
  const tab = tabs.value.find((candidate) => candidate.id === id);
  if (tab?.kind === "preview") {
    if (!previewCache.value?.focus(tabKey(tab))) focusPaneChrome();
    return;
  }
  // Embedded views with their own activation watcher get first refusal. The
  // extra tick lets that watcher focus a terminal, filter, or reading surface.
  await nextTick();
  const panel = Array.from(workArea.value?.querySelectorAll<HTMLElement>("[data-main-tab-id]") ?? [])
    .find((candidate) => candidate.dataset.mainTabId === id);
  if (!panel || (document.activeElement instanceof Node && panel.contains(document.activeElement))) return;
  (panel.querySelector<HTMLElement>('[tabindex="-1"]') ?? panel).focus({ preventScroll: true });
}

/**
 * Consequences of a tab closing that belong to the panel. Wired into the tab
 * store by App.vue so they run however the tab was closed.
 */
function onTabClosed(tab: MainTab) {
  // The shell is how an operator installs an agent CLI before they have any
  // repositories, so closing it is the moment to look again.
  if (tab.kind === "preview") previewCache.value?.discard(tabKey(tab));
  if (tab.kind === "shell" && !props.hasRepos) void checkAllClis();
}

/**
 * A tab id is only unique inside its task's tab set, and the same file path
 * can be open in two tasks. Keying the rendered view by scope as well makes a
 * task switch remount it against the new worktree instead of leaving the
 * previous task's content in a reused node.
 */
function tabKey(tab: MainTab): string {
  return `${props.views?.tabs.scopeKey.value ?? ""}:${tab.id}:${tab.kind === "editor" ? tab.editorSession?.worktreePath : props.views?.modals.readingWorkspace?.value ?? props.views?.modals.activeWorktreePath?.value}`;
}

const diffViewProps = computed(() => {
  const modals = props.views?.modals;
  if (!modals) return null;
  const state = modals.currentDiffViewState.value;
  const route = modals.activeRemoteTaskRoute.value;
  return {
    repoPath: modals.activeRepoPath.value || props.repoPath || "",
    worktreePath: modals.activeDiffWorktreePath.value,
    initialScope: state?.scope,
    initialScrollPositions: state?.scrollPositions,
    initialBranchInclude: state?.branchInclude,
    baseRef: item.value?.base_ref ?? undefined,
    viewKey: modals.currentDiffViewKey.value,
    remoteDiffLoader: modals.activeTaskViewIsRemote.value ? modals.readRemoteTaskDiff : undefined,
    remoteDesktopId: route?.desktopId,
    remoteTaskId: route?.taskId,
    remoteTransport: route?.transport,
  };
});

/**
 * The contained readers a view an agent opened uses, one stable function per
 * task.
 *
 * These are read from the template, so a fresh closure here would be a new
 * function identity on every parent render — and the views downstream treat a
 * new loader as a new place to be looking at. The explorer resets breadcrumb,
 * cursor, filter and visibility on it, and the file preview reloads: switching
 * to the agent tab and back would silently throw away the very location an
 * agent asked a human to read. Keying the cache by task keeps a genuine task
 * change resetting the view, which is what that reset is for.
 */
const containedFileLoaders = new Map<string, (path: string) => Promise<string>>();
const containedDirectoryLoaders = new Map<
  string,
  (path: string, showAllFiles: boolean) => Promise<{ entries: RemoteDirectoryEntry[] }>
>();

function containedFileLoader(taskId: string | undefined) {
  if (!taskId) return undefined;
  const existing = containedFileLoaders.get(taskId);
  if (existing) return existing;
  const loader = (path: string) => readDesktopTaskFile(taskId, path);
  containedFileLoaders.set(taskId, loader);
  return loader;
}

function containedDirectoryLoader(taskId: string | undefined) {
  if (!taskId) return undefined;
  const existing = containedDirectoryLoaders.get(taskId);
  if (existing) return existing;
  const loader = (path: string, showAllFiles: boolean) =>
    listDesktopTaskDirectory(taskId, path, showAllFiles);
  containedDirectoryLoaders.set(taskId, loader);
  return loader;
}

function fileViewProps(tab: MainTab) {
  const modals = props.views?.modals;
  const views = props.views;
  const taskId = item.value?.id;
  const local = !isMobile && !props.cloudTask && !modals?.activeTaskViewIsRemote.value && taskId && !isRemotePresentationTaskId(taskId);
  const worktreePath = local ? props.views?.store.worktreePaths?.[taskId] : undefined;
  const editInTerminal = local && worktreePath && tab.remoteContent == null && item.value?.closed_at == null
    ? async (command: string) => {
      const session = await openTerminalEditor(taskId, worktreePath, tab.filePath ?? "", command);
      props.views?.tabs.openTabInScope(mainTabScopeKeyForTask(taskId), { kind: "editor", editorSession: session });
    } : undefined;
  return {
    filePath: tab.filePath ?? "",
    editInTerminal,
    worktreePath: worktreePath ?? modals?.activeWorktreePath.value ?? taskWorktreePath.value ?? "",
    remoteContent: tab.remoteContent ?? null,
    remoteContentLoader: modals?.activeTaskViewIsRemote.value
      ? modals.readRemoteTaskFile
      : undefined,
    // A tab an agent opened reads through the server's contained resolution,
    // so a symlink swapped in after validation cannot put outside content on
    // screen under this task's name.
    contentLoader: containedFileLoader(tab.containedTaskId),
    ideCommand: props.views?.store.ideCommand,
    terminalEditorNoticeDismissed:
      props.views?.store.snapshotSettings?.[TERMINAL_EDITOR_NOTICE_SETTING_KEY] === "true",
    dismissTerminalEditorNotice: views
      ? () => views.store.savePreference(TERMINAL_EDITOR_NOTICE_SETTING_KEY, "true")
      : undefined,
    initialLine: tab.initialLine,
    initialScrollTop: tab.reading?.workspace === modals?.readingWorkspace?.value ? tab.reading?.top : undefined,
    initialMarkdownMode: modals?.currentPreviewMarkdownMode.value,
  };
}

function shellSessionId(tab: MainTab): string {
  if (tab.shellScope === "repo") {
    const repoId = scopeRepoId.value;
    return repoId ? `shell-repo-${repoId}` : "shell-home";
  }
  return item.value ? `shell-wt-${item.value.id}` : "";
}

function shellCwd(tab: MainTab): string {
  if (tab.shellScope === "repo") {
    return scopeRepoPath.value || (props.views?.modals.homePath.value ?? "");
  }
  return taskWorktreePath.value ?? scopeRepoPath.value;
}

function treeViewProps(tab: MainTab) {
  const modals = props.views?.modals;
  const route = modals?.activeRemoteTaskRoute.value;
  const containedTaskId = tab.containedTaskId;
  return {
    worktreePath: modals?.treeExplorerRoot.value ?? taskWorktreePath.value ?? scopeRepoPath.value,
    repoRoot: scopeRepoPath.value || (modals?.treeExplorerRoot.value ?? ""),
    homePath: modals?.homePath.value,
    // Same containment reason as the file view: the explorer asks the server
    // rather than walking the worktree path itself.
    remoteDirectoryLoader: containedDirectoryLoader(containedTaskId)
      ?? (modals?.activeTaskViewIsRemote.value ? modals.listRemoteTaskDirectory : undefined),
    // The preview column reads files the same way the file view does, so a
    // browsed-elsewhere explorer previews the task's file rather than this
    // machine's copy of that path.
    remoteContentLoader: containedFileLoader(containedTaskId)
      ?? (modals?.activeTaskViewIsRemote.value ? modals.readRemoteTaskFile : undefined),
    remoteDesktopId: route?.desktopId,
    remoteTaskId: route?.taskId,
    remoteTransport: route?.transport,
  };
}

function onDiffScopeChange(scope: DiffScope) {
  props.views?.modals.updateCurrentDiffViewState({ scope });
}

function onDiffScrollStateChange(scrollPositions: DiffScrollPositions) {
  props.views?.modals.updateCurrentDiffViewState({ scrollPositions });
}

function onDiffBranchIncludeChange(branchInclude: BranchInclude) {
  props.views?.modals.updateCurrentDiffViewState({ branchInclude });
}

function onMarkdownModeChange(mode: MarkdownPreviewMode) {
  props.views?.modals.updateCurrentPreviewMarkdownMode(mode);
}

interface DismissableView {
  dismiss?: () => boolean;
  /**
   * Show the view's content and whatever the command aimed it at, and say
   * whether that succeeded. The contract every whitelisted view implements for
   * `kanna_open_view`: the route reports `opened` to the agent that asked, so
   * "rendered" here means rendered, not "mounted and loading".
   */
  revealDesktopViewTarget?: (
    command: DesktopViewOpenCommand,
  ) => Promise<DesktopViewOpenOutcome>;
}

const viewRefs = new Map<string, DismissableView>();

function setViewRef(id: string, component: Element | ComponentPublicInstance | null) {
  if (component) {
    viewRefs.set(id, component as unknown as DismissableView);
  } else {
    viewRefs.delete(id);
  }
}

/**
 * Aim one tab at what an agent asked a human to look at.
 *
 * The tab must be the one in front — a view that is behind another one is not
 * showing anybody anything — and then the view itself decides when its content
 * and target are up. Views with nothing to load and nothing to aim (the agent
 * session, analytics) are ready as soon as they are the active tab.
 */
async function revealTabTarget(
  tabId: string,
  command: DesktopViewOpenCommand,
): Promise<DesktopViewOpenOutcome> {
  const controller = props.views?.tabs;
  if (!controller) {
    return {
      opened: false,
      code: "renderer_failed",
      message: "this window is not hosting task views",
    };
  }
  // Selecting the task is what moves this panel onto that task's tab set, and
  // it lands through the store rather than in the same tick — so activation is
  // retried until the scope catches up rather than giving up on the first one.
  const activated = await waitForViewReady(() => {
    if (controller.scopeKey.value !== `item:${command.taskId}`) return false;
    controller.activateTab(tabId);
    return controller.activeTabId.value === tabId;
  }, { timeoutMs: 3_000 });
  await nextTick();
  if (!activated) {
    return {
      opened: false,
      code: "renderer_failed",
      message: `the ${command.view} view could not be brought to the front`,
    };
  }
  const reveal = viewRefs.get(tabId)?.revealDesktopViewTarget;
  if (!reveal) {
    if (command.target === undefined) return { opened: true };
    return {
      opened: false,
      code: "unsupported_target",
      message: `this window's ${command.view} view cannot be aimed at a target`,
    };
  }
  return await reveal(command);
}

/**
 * Escape's share of the tab surface. The centralized dismiss handler calls
 * this once every open modal has declined, because a modal is always above the
 * tabs. Returns true when the key was consumed.
 */
function dismissActiveTab(): boolean {
  const controller = props.views?.tabs;
  const tab = controller?.activeTab.value;
  if (!controller || !tab || tab.kind === "agent") return false;
  // A shell tab is a live terminal; Escape belongs to whatever runs in it.
  if (tab.kind === "shell" || tab.kind === "editor") return false;
  // A view with its own layered dismiss — a file's search, the tree's filter,
  // the graph's detail pane — gets to close that first.
  if (viewRefs.get(tab.id)?.dismiss?.() === false) return true;
  controller.closeTab(tab.id);
  return true;
}
const headerItem = computed(() => {
  const slot = props.uiSlot;
  if (!slot) return null;
  const task = slot.task;
  return {
    display_name: task?.display_name ?? slot.draft.display_name,
    issue_title: task?.issue_title ?? null,
    prompt: task?.prompt ?? slot.draft.prompt,
    stage: task?.stage ?? slot.draft.stage,
    branch: task?.branch ?? null,
    port_env: task?.port_env ?? null,
    issue_number: task?.issue_number ?? null,
    pr_number: task?.pr_number ?? null,
    pr_url: task?.pr_url ?? null,
  };
});

const isBlocked = computed(() => {
  if (props.blocked !== undefined) return props.blocked;
  if (!props.blockers || props.blockers.length === 0) return false;
  return props.blockers.some(b => !isBlockerResolved(b));
});

const activeCloudTerminal = computed<CloudTerminalCacheEntry | null>(() => {
  const task = item.value;
  const terminalRef = props.cloudTerminalRef;
  if (
    !task
    || props.uiSlot?.state !== "ready"
    || !props.cloudTask
    || isBlocked.value
    || !terminalRef
  ) {
    return null;
  }
  return {
    key: task.id,
    ownerDesktopId: terminalRef.ownerDesktopId,
    ownerTaskId: terminalRef.ownerLocalTaskId,
    worktreeBranch: task.branch,
    transport: terminalRef.transport,
    sessionRevision: task.transition_revision ?? null,
  };
});

const discardedCloudTerminalKey = computed(() => {
  const task = item.value;
  if (!task || props.uiSlot?.state !== "ready" || !props.cloudTask) return null;
  return isBlocked.value || !props.cloudTerminalRef ? task.id : null;
});

const commandHintDismissed = ref(readCommandHintDismissed());
const showCommandHint = computed(() => !commandHintDismissed.value);
const taskDetail = ref<DesktopTaskDetail | null>(null);

const revisionBudgetExhausted = computed(() => {
  const task = item.value;
  const detail = taskDetail.value;
  if (!task || !detail || task.closed_at != null || detail.closedAt != null) return false;
  if (detail.id !== task.id || detail.revisionLimit <= 0) return false;
  if (detail.revisionRounds < detail.revisionLimit) return false;
  const latestRun = detail.latestRun;
  return latestRun?.status === "failed"
    && latestRun.summary?.startsWith("Parked for human review:") === true;
});

/**
 * Task detail comes from the server that owns the task. A task running on
 * another machine is shown here under a `cloud:` presentation id this server
 * has never heard of, so asking for its detail is a guaranteed 404 — and the
 * watcher below re-fires on every activity, stage and `updated_at` change the
 * cloud index syncs, so it asked tens of thousands of times for one selected
 * remote task. Every miss also fans out over the relay to each reachable
 * peer, so the noise lands in the other machine's log too.
 *
 * Nothing is lost by not asking: the detail-derived exhausted-budget status
 * and review context were never populated for a remote task anyway — the
 * fetch always failed.
 */
const taskDetailIsLocal = computed(() => {
  const taskId = item.value?.id;
  if (!taskId) return false;
  return !props.cloudTask && !isRemotePresentationTaskId(taskId);
});

/** Read-only PR identity and decision status, never inferred from the task title. */
const reviewContext = computed(() => {
  const task = item.value;
  const detail = taskDetail.value;
  if (!task || !detail || detail.id !== task.id) return null;
  return detail.reviewContext ?? null;
});

const humanReviewDecision = computed(() => {
  const task = item.value;
  const detail = taskDetail.value;
  if (!task || !detail || detail.id !== task.id) return null;
  return detail.humanReviewDecision ?? null;
});

/**
 * A decision already taken for the exact head on screen. A decision recorded
 * against an older head is deliberately not treated as this one: the PR moved,
 * and what the reviewer authorized was a commit that is no longer the head.
 */
const decisionForCurrentHead = computed(() => {
  const decision = humanReviewDecision.value;
  const context = reviewContext.value;
  if (!decision || !context) return null;
  return decision.headSha.toLowerCase() === context.headSha.toLowerCase() ? decision : null;
});

const shortReviewedHead = computed(() => reviewContext.value?.headSha.slice(0, 12) ?? "");

const reviewedHeadLabel = computed(() => {
  const context = reviewContext.value;
  if (!context) return "";
  const branch = context.headRepo && context.headRef
    ? `${context.headRepo}:${context.headRef}`
    : context.headRef ?? "";
  return branch ? `${branch} @ ${shortReviewedHead.value}` : shortReviewedHead.value;
});

let taskDetailRequest = 0;
async function loadTaskDetail(taskId: string): Promise<void> {
  const request = ++taskDetailRequest;
  try {
    const detail = await fetchDesktopTaskDetail(taskId);
    if (request === taskDetailRequest && item.value?.id === taskId) {
      taskDetail.value = detail;
    }
  } catch (error) {
    console.error(`[main-panel] failed to load task detail for ${taskId}:`, error);
  }
}

watch(
  () => [
    item.value?.id ?? null,
    item.value?.activity_revision ?? 0,
    item.value?.stage ?? null,
    item.value?.updated_at ?? null,
    item.value?.has_running_post ?? 0,
    // A task that transfers in stops being remote without changing id, and
    // must pick up the detail it can now be asked for.
    taskDetailIsLocal.value,
  ] as const,
  ([taskId], previous) => {
    if (taskId !== previous?.[0]) {
      taskDetailRequest += 1;
      taskDetail.value = null;
    }
    if (taskId && taskDetailIsLocal.value) {
      void loadTaskDetail(taskId);
    } else if (taskDetail.value) {
      taskDetail.value = null;
    }
  },
  { immediate: true },
);

watch(
  () => [item.value?.id ?? null, item.value?.stage ?? null, agentHistorySourceKey.value] as const,
  ([taskId, _stage, sourceKey], previous) => {
    const identityChanged = taskId !== previous?.[0] || sourceKey !== previous?.[2];
    if (identityChanged) {
      attemptsRequest += 1;
      setupRunsRequest += 1;
      agentAttempts.value = [];
      agentSetupRuns.value = [];
      agentHistoryStatus.value = "";
      // A failed snapshot can arrive before the durable task is hydrated.
      // Preserve its selection across that acknowledgement, not task/source switches.
      const hydratingSelectedCreation = previous?.[0] === null
        && taskId === completedCreationTask.value
        && !props.cloudTask && !agentHistoryRemoteRoute.value
        && selectedAttempt.value === CREATION_SELECTION;
      if (!hydratingSelectedCreation) selectedAttempt.value = "";
    }
    if (taskId) {
      void loadAgentAttempts(taskId, sourceKey);
      void loadAgentSetupRuns(taskId, sourceKey);
    }
  },
  { immediate: true },
);

// --- Agent CLI detection ---

interface AgentCliStatus {
  installed: boolean;
  version?: string;
}

const claude = ref<AgentCliStatus>({ installed: false });
const copilot = ref<AgentCliStatus>({ installed: false });
const codex = ref<AgentCliStatus>({ installed: false });
const opencode = ref<AgentCliStatus>({ installed: false });
const antigravity = ref<AgentCliStatus>({ installed: false });
const copiedAgent = ref<string | null>(null);

interface AgentSetupCard {
  key: AgentProvider;
  nameKey: string;
  sortName: string;
  installCommand: string;
  status: AgentCliStatus;
}

interface AgentCardMetadata {
  nameKey: string;
  sortName: string;
  installCommand: string;
}

const AGENT_CARD_METADATA: Record<AgentProvider, AgentCardMetadata> = {
  claude: {
    nameKey: "mainPanel.agentClaudeName",
    sortName: "Claude Code",
    installCommand: "curl -fsSL https://claude.ai/install.sh | bash",
  },
  copilot: {
    nameKey: "mainPanel.agentCopilotName",
    sortName: "GitHub Copilot",
    installCommand: "curl -fsSL https://gh.io/copilot-install | bash",
  },
  codex: {
    nameKey: "mainPanel.agentCodexName",
    sortName: "OpenAI Codex",
    installCommand: "npm install -g @openai/codex",
  },
  opencode: {
    nameKey: "mainPanel.agentOpenCodeName",
    sortName: "OpenCode",
    installCommand: "curl -fsSL https://opencode.ai/install | bash",
  },
  antigravity: {
    nameKey: "mainPanel.agentAntigravityName",
    sortName: "Google Antigravity",
    installCommand: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
  },
};

const statusByProvider: Record<AgentProvider, Ref<AgentCliStatus>> = {
  claude,
  copilot,
  codex,
  opencode,
  antigravity,
};

const agentCards = computed<AgentSetupCard[]>(() => AGENT_PROVIDERS.map((provider) => ({
  key: provider,
  ...AGENT_CARD_METADATA[provider],
  status: statusByProvider[provider].value,
})));

const agentSetupGroups = computed(() => {
  const sorted = [...agentCards.value].sort((a, b) =>
    a.sortName.localeCompare(b.sortName, undefined, { sensitivity: "base" })
  );
  return [
    {
      key: "installed",
      titleKey: "mainPanel.agentInstalled",
      cards: sorted.filter(agent => agent.status.installed),
    },
    {
      key: "not-installed",
      titleKey: "mainPanel.agentNotInstalled",
      cards: sorted.filter(agent => !agent.status.installed),
    },
  ].filter(group => group.cards.length > 0);
});

function parseSemver(output: string): string | undefined {
  const match = output.match(/\b(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)\b/);
  return match?.[1];
}

async function readE2eCliVersion(name: string): Promise<string | undefined> {
  if (!import.meta.env.DEV) return undefined;
  const envName = `KANNA_E2E_AGENT_CLI_VERSION_${name.toUpperCase().replace(/[^A-Z0-9_]/g, "_")}`;
  try {
    return await invoke<string>("read_env_var", { name: envName });
  } catch (error) {
    console.debug(`[main-panel] E2E CLI version override not set for ${name}:`, error);
    return undefined;
  }
}

async function checkCli(provider: AgentProvider): Promise<AgentCliStatus> {
  const binary = getAgentProviderSpec(provider).executable;
  const e2eVersionOutput = await readE2eCliVersion(binary);
  if (e2eVersionOutput !== undefined) {
    return { installed: true, version: parseSemver(e2eVersionOutput) };
  }

  try {
    await invoke("which_binary", { name: binary });
  } catch (error) {
    console.debug(`[main-panel] CLI binary not found: ${binary}`, error);
    return { installed: false };
  }
  try {
    const output = await invoke("run_script", {
      script: `${binary} --version`,
      cwd: "/",
      env: {},
    }) as string;
    return { installed: true, version: parseSemver(output) };
  } catch (error) {
    console.debug(`[main-panel] failed to read CLI version for ${provider}:`, error);
    return { installed: true };
  }
}

async function checkAllClis() {
  const statuses = await Promise.all(
    AGENT_PROVIDERS.map(async (provider) => [provider, await checkCli(provider)] as const),
  );
  for (const [provider, status] of statuses) {
    statusByProvider[provider].value = status;
  }
}

watch(() => props.hasRepos, (has) => {
  if (!has) checkAllClis();
}, { immediate: true });

function workspacePresentation() {
  return visiblePanes.value.map(({ pane, left, top, width, height }) => ({
    id: pane.id, left, top, width, height, activeTabId: pane.active,
  }));
}

defineExpose({
  workspacePresentation,
  recheckClis: checkAllClis,
  dismissActiveTab,
  revealTabTarget,
  onTabClosed,
  focusActivePaneContent,
});

async function copyCommand(agent: AgentProvider) {
  const cmd = AGENT_CARD_METADATA[agent].installCommand;
  await navigator.clipboard.writeText(cmd);
  copiedAgent.value = agent;
  setTimeout(() => { copiedAgent.value = null; }, 1500);
}

function readCommandHintDismissed(): boolean {
  if (typeof window === "undefined") return false;
  return window.localStorage.getItem(COMMAND_HINT_STORAGE_KEY) === "1";
}

function dismissCommandHint() {
  commandHintDismissed.value = true;
  if (typeof window !== "undefined") {
    window.localStorage.setItem(COMMAND_HINT_STORAGE_KEY, "1");
  }
}
</script>

<template>
  <main class="main-panel">
    <template v-if="uiSlot">
      <div v-if="isMobile" class="mobile-back-bar" @click="emit('back')">
        <span class="mobile-back-arrow">&larr;</span>
        <span>Tasks</span>
      </div>
      <TaskHeader v-if="headerItem" :item="headerItem" :owner-label="ownerLabel" :task-id="item?.id" :preview-supported="taskDetailIsLocal && !isMobile && !views?.modals.activeTaskViewIsRemote?.value && !!views" :latest-run="taskDetail?.latestRun ?? null" :session-history="taskDetail?.sessionHistory ?? null" :stage-dependencies="taskDetail?.stageDependencies ?? null" :dependency-wait="taskDetail?.dependencyWait ?? null" :gate-parked="taskDetail?.gateParked ?? null" @preview="(portName) => views?.tabs.openTab({ kind: 'preview', portName })" @open-artifact="openLatestResultArtifact" />
      <section v-if="revisionBudgetExhausted" class="revision-exhausted" data-testid="revision-exhausted-status">
        <div>
          <p class="revision-exhausted-title">{{ $t('mainPanel.revisionExhaustedTitle') }}</p>
          <p class="revision-exhausted-hint">
            {{ $t('mainPanel.revisionExhaustedHint', {
              rounds: taskDetail?.revisionRounds,
              limit: taskDetail?.revisionLimit,
            }) }}
          </p>
        </div>
      </section>
    </template>
    <div ref="workArea" class="work-area" v-show="!showEmptyState" :class="{ split: splitVisible }" data-testid="task-work-area">
      <div v-for="rect in visiblePanes" :key="rect.pane.id" class="pane-chrome" :style="paneStyle(rect)" :data-workspace-pane-id="rect.pane.id" tabindex="-1">
        <MainTabBar
          :tabs="tabs.filter(tab => rect.pane.tabs.includes(tab.id)).sort((a, b) => rect.pane.tabs.indexOf(a.id) - rect.pane.tabs.indexOf(b.id))"
          :active-tab-id="rect.pane.active"
          :worktree-path="taskWorktreePath"
          :pane-id="narrowLayout ? undefined : rect.pane.id"
          :scope-key="views?.tabs.scopeKey.value"
          :new-views="newViews"
          :pane-actions="paneActions"
          :can-close-pane="!narrowLayout && paneRects.length > 1"
          @close-pane="views?.tabs.closePane(rect.pane.id)"
          :agent-attempts="item ? agentAttempts : undefined"
          :agent-setup-runs="agentSetupRuns"
          :creation-output="creationComplete"
          :agent-history-status="agentHistoryStatus"
          :selected-attempt="selectedAttempt"
          :current-stage="item?.stage"
          @select-attempt="selectAttempt"
          @select="selectTab"
          @close="closeTab"
          @new="id => openPaneView(rect.pane.id, id)"
          @layout="(id, tabId) => openPaneView(rect.pane.id, id, tabId)"
          @drag-tab="tabDrag.start"
          :dragged-tab="tabDrag.dragging.value"
          :drop-active="tabDrag.target.value?.paneId === rect.pane.id"
          :drop-before="tabDrag.target.value?.paneId === rect.pane.id ? tabDrag.target.value?.beforeId : undefined"
        />
        <div v-if="!rect.pane.tabs.length" class="empty-pane" @click="views?.tabs.focusPane(rect.pane.id)">Drop a tab here or use + to open a view.</div>
      </div>
      <template v-if="!narrowLayout && !views?.tabs.maximizedPaneId.value">
        <div
          v-for="divider in views?.tabs.dividers.value" :key="divider.path"
          class="pane-divider" :style="dividerStyle(divider)"
          role="separator" tabindex="0" aria-label="Resize panes"
          :aria-orientation="divider.axis === 'horizontal' ? 'vertical' : 'horizontal'"
          :aria-valuenow="Math.round(divider.ratio * 100)"
          @pointerdown.prevent="captureDivider"
          @pointermove="resizeDivider($event, divider)"
          @keydown.left.prevent="views?.tabs.resizePane(divider.path, divider.ratio - .05)"
          @keydown.right.prevent="views?.tabs.resizePane(divider.path, divider.ratio + .05)"
          @keydown.up.prevent="views?.tabs.resizePane(divider.path, divider.ratio - .05)"
          @keydown.down.prevent="views?.tabs.resizePane(divider.path, divider.ratio + .05)"
        />
      </template>
    <template v-if="uiSlot">
      <div v-show="agentVisible" :style="views ? tabStyle(AGENT_TAB_ID) : {}" @pointerdown.capture="selectTab(AGENT_TAB_ID)" @focusin="selectTab(AGENT_TAB_ID)" :class="{ 'pane-focused': agentTabActive }" class="main-tab-panel" data-testid="main-tab-panel-agent">
        <section
          v-if="reviewContext"
          class="review-merge"
          data-testid="review-merge-status"
        >
          <div class="review-merge-copy">
            <p class="review-merge-title">{{ $t('mainPanel.reviewMergeTitle') }}</p>
            <p class="review-merge-detail" data-testid="review-merge-head">
              {{ $t('mainPanel.reviewMergeReviewed', {
                pr: reviewContext.prUrl,
                head: reviewedHeadLabel,
                base: reviewContext.baseRef,
              }) }}
            </p>
            <p
              v-if="reviewContext.relatedPrUrls && reviewContext.relatedPrUrls.length > 0"
              class="review-merge-warning"
              data-testid="review-merge-overlap"
            >
              {{ $t('mainPanel.reviewMergeOverlap', {
                prs: reviewContext.relatedPrUrls.join(', '),
              }) }}
            </p>
            <p
              v-if="decisionForCurrentHead"
              class="review-merge-detail"
              data-testid="review-merge-decision"
            >
              {{ $t(`mainPanel.reviewMergeDelivery.${decisionForCurrentHead.deliveryStatus}`, {
                mergeTask: decisionForCurrentHead.mergeTaskId ?? '',
                machine: decisionForCurrentHead.ownerDesktopId ?? '',
                detail: decisionForCurrentHead.deliveryDetail ?? '',
              }) }}
            </p>
          </div>
        </section>
        <TaskCreationProgress
          v-if="(uiSlot.draft.creation_task_id || uiSlot.task_id) && !cloudTask"
          :task-id="uiSlot.draft.creation_task_id || uiSlot.task_id!"
          :creating="uiSlot.state === 'creating'"
          :error="uiSlot.draft.creation_error"
          :selected="historySelection.kind === 'creation'"
          @completed="creationCompleted"
        />
        <AgentHistoryView
          ref="agentHistoryRef"
          v-if="selectedArchiveRunId && item"
          :task-id="agentHistoryBindingTaskId"
          :attempt-id="selectedArchiveRunId"
          :source-key="agentHistorySourceKey"
          :load-archive="loadSelectedAgentArchive"
        />
        <WorkspaceSetupLogView
          ref="setupLogRef"
          v-else-if="selectedSetupRunId && item"
          :task-id="agentHistoryBindingTaskId"
          :run-id="selectedSetupRunId"
        />
        <div v-show="!selectedAttempt && uiSlot.state !== 'creating'" class="agent-live-content">
        <CloudTerminalCache
          :active-terminal="activeCloudTerminal"
          :focused="agentTabActive && !selectedAttempt"
          :visible="agentVisible && !selectedAttempt"
          :discard-key="discardedCloudTerminalKey"
        />
        <template v-if="uiSlot.state !== 'ready' || !item">
          <div v-if="!uiSlot.draft.creation_task_id" class="setup-placeholder">
            <p class="setup-title">{{ $t('mainPanel.taskSettingUp') }}</p>
          </div>
        </template>
        <template v-else-if="isBlocked">
          <div class="blocked-placeholder">
            <p class="blocked-title">{{ $t('mainPanel.taskBlocked') }}</p>
            <p class="blocked-hint">{{ $t('mainPanel.taskBlockedHint') }}</p>
            <div v-if="blockers && blockers.length > 0" class="blocked-by">
              <p class="blocked-by-label">{{ $t('mainPanel.waitingOn') }}</p>
              <div v-for="b in blockers" :key="b.id" class="blocker-item">
                <span
                  class="blocker-status"
                  :style="{ color: b.closed_at != null ? 'var(--kn-text-muted)' : 'var(--kn-accent)' }"
                >{{ b.closed_at != null ? $t('mainPanel.blockerDone') : $t('mainPanel.blockerActive') }}</span>
                <span class="blocker-name">{{
                  b.display_name
                    || b.issue_title
                    || (b.prompt ? b.prompt.slice(0, 60) : null)
                    || (b.fallback_task_id
                      ? $t('tasks.taskId', { id: b.fallback_task_id })
                      : $t('tasks.untitled'))
                }}</span>
              </div>
            </div>
          </div>
        </template>
        <template v-else-if="cloudTask">
          <div v-if="!cloudTerminalRef" class="cloud-task-placeholder">
            <p class="cloud-task-title">Task is running on another machine</p>
            <p class="cloud-task-hint">Cloud sync is showing the task here, but terminal routing information is unavailable.</p>
          </div>
        </template>
        <template v-else>
          <TerminalTabs
            :session-id="item.id"
            :active="agentTabActive && !selectedAttempt"
            :visible="agentVisible && !selectedAttempt"
            :agent-type="item.agent_type || 'pty'"
            :agent-provider="item.agent_provider"
            :repo-path="repoPath"
            :worktree-path="taskWorktreePath"
            :prompt="item.prompt || ''"
            :spawn-pty-session="spawnPtySession"
            :recover-task-session="recoverTaskSession"
          />
        </template>
        </div>
      </div>
    </template>
    <div v-if="views" class="reference-area" data-testid="reference-area">
      <div v-for="tab in openViewTabs.filter(tab => tab.kind !== 'preview')" :key="tabKey(tab)" v-show="viewVisible(tab.id)" :style="tabStyle(tab.id)" class="reference-view" :data-main-tab-id="tab.id" tabindex="-1" @pointerdown.capture="selectTab(tab.id)" @focusin="selectTab(tab.id)">
        <DiffModal
          v-if="tab.kind === 'diff' && diffViewProps"
          :ref="(component) => setViewRef(tab.id, component)"
          v-show="viewVisible(tab.id)"
          v-bind="diffViewProps"
          :is-visible="() => viewVisible(tab.id)"
          embedded
          :active="activeTabId === tab.id"
          @scope-change="onDiffScopeChange"
          @scroll-state-change="onDiffScrollStateChange"
          @branch-include-change="onDiffBranchIncludeChange"
          @close="closeTab(tab.id)"
        />
        <FilePreviewModal
          v-else-if="tab.kind === 'file'"
          :ref="(component) => setViewRef(tab.id, component)"
          v-show="viewVisible(tab.id)"
          v-bind="fileViewProps(tab)"
          embedded
          :active="activeTabId === tab.id"
          @update-markdown-mode="onMarkdownModeChange"
          @scroll-position="(top: number) => views?.tabs.updateReading(tab.id, { workspace: views.modals.readingWorkspace.value, top })"
          @close="closeTab(tab.id)"
        />
        <TerminalEditorView
          v-else-if="tab.kind === 'editor' && tab.editorSession && taskDetailIsLocal && !views?.modals.activeTaskViewIsRemote.value"
          v-show="viewVisible(tab.id)"
          :session="tab.editorSession"
          :visible="viewVisible(tab.id)"
          :active="activeTabId === tab.id"
        />
        <div v-else-if="tab.kind === 'editor'" v-show="viewVisible(tab.id)" class="cloud-task-placeholder">
          Terminal editing is available only on the desktop holding this workspace. Remote editor sessions are not transported.
        </div>
        <ShellModal
          v-else-if="tab.kind === 'shell' && shellSessionId(tab)"
          v-show="viewVisible(tab.id)"
          :session-id="shellSessionId(tab)"
          :visible="viewVisible(tab.id)"
          :cwd="shellCwd(tab)"
          :fallback-cwd="tab.shellScope === 'repo' ? undefined : scopeRepoPath"
          :port-env="tab.shellScope === 'repo' ? undefined : item?.port_env"
          embedded
          :active="activeTabId === tab.id"
          @close="closeTab(tab.id)"
        />
        <TreeExplorerModal
          v-else-if="tab.kind === 'tree'"
          :ref="(component) => setViewRef(tab.id, component)"
          v-show="viewVisible(tab.id)"
          v-bind="treeViewProps(tab)"
          embedded
          :active="activeTabId === tab.id"
          @open-file="(filePath: string) => views?.modals.openFilePreview(filePath)"
          @close="closeTab(tab.id)"
        />
        <CommitGraphModal
          v-else-if="tab.kind === 'graph' && scopeRepoPath"
          :ref="(component) => setViewRef(tab.id, component)"
          v-show="viewVisible(tab.id)"
          :repo-path="scopeRepoPath"
          :worktree-path="taskWorktreePath"
          :remote-graph-loader="views?.modals.activeTaskViewIsRemote.value ? views.modals.readRemoteTaskGraph : undefined"
          embedded
          :active="activeTabId === tab.id"
          @close="closeTab(tab.id)"
        />
        <AnalyticsModal
          v-else-if="tab.kind === 'analytics'"
          :ref="(component) => setViewRef(tab.id, component)"
          v-show="viewVisible(tab.id)"
          :repo-id="scopeRepoId"
          embedded
          :active="activeTabId === tab.id"
          @close="closeTab(tab.id)"
        />
        <ImageUrlPreviewModal
          v-else-if="tab.kind === 'image'"
          v-show="viewVisible(tab.id)"
          :image-url="tab.imageUrl ?? ''"
          embedded
          :active="activeTabId === tab.id"
          @close="closeTab(tab.id)"
        />
        <ArtifactViewer
          v-else-if="tab.kind === 'artifact' && tab.artifactRepoId"
          v-show="viewVisible(tab.id)"
          :repo-id="tab.artifactRepoId"
          :artifact-id="tab.artifactShownId ?? tab.artifactId"
          :visible="viewVisible(tab.id)"
          @navigate="(artifactId: string) => views?.tabs.updateArtifactShown(tab.id, artifactId)"
        />
      </div>
      <TaskPreviewCache
        ref="previewCache"
        :workspaces="previewWorkspaces"
        :visible-entries="visiblePreviews"
        @activate="key => { const tab = tabs.find(tab => tabKey(tab) === key); if (tab) selectTab(tab.id); }"
      />
    </div>
    </div>
    <div v-if="showEmptyState" class="empty-state">
      <template v-if="!hasRepos">
        <div class="agent-setup">
          <p class="setup-title">{{ $t('mainPanel.agentSetupTitle') }}</p>
          <div class="agent-cards">
            <section v-for="group in agentSetupGroups" :key="group.key" class="agent-group">
              <p class="agent-group-title">{{ $t(group.titleKey) }}</p>
              <div v-for="agent in group.cards" :key="agent.key" class="agent-card">
                <div class="agent-header">
                  <span class="agent-name">{{ $t(agent.nameKey) }}</span>
                  <span v-if="agent.status.installed" class="agent-badge installed">
                    <span class="checkmark">✓</span>
                    {{ $t('mainPanel.agentVersion', { version: agent.status.version || '?' }) }}
                  </span>
                  <span v-else class="agent-badge not-installed">
                    {{ $t('mainPanel.agentNotInstalled') }}
                  </span>
                </div>
                <div v-if="!agent.status.installed" class="install-block">
                  <code class="install-cmd">{{ agent.installCommand }}</code>
                  <button
                    class="copy-btn"
                    :title="copiedAgent === agent.key ? $t('mainPanel.agentCopied') : 'Copy'"
                    @click="copyCommand(agent.key)"
                  >
                    {{ copiedAgent === agent.key ? '✓' : '⧉' }}
                  </button>
                </div>
              </div>
            </section>
          </div>
          <p class="setup-hint">
            {{ $t('mainPanel.agentInstallHint', { shellShortcut: shortcutHint('openShellRepoRoot') }) }}
          </p>
          <p class="empty-hint">{{ $t('mainPanel.noReposHint', { shortcut: shortcutHint('createRepo') }) }}</p>
        </div>
      </template>
      <template v-else>
        <p class="empty-title">{{ $t('mainPanel.noTaskSelected') }}</p>
        <p class="empty-hint">{{ $t('mainPanel.noTaskHint', { shortcut: shortcutHint('newTask') }) }}</p>
      </template>
    </div>
    <div
      v-if="showCommandHint"
      data-testid="command-hint"
      class="command-hint"
    >
      <span class="command-hint-copy">
        <span v-if="$t('mainPanel.commandHintPrefix')" class="command-hint-text">
          {{ $t('mainPanel.commandHintPrefix') }}
        </span>
        <span class="command-hint-shortcut">
          <kbd v-for="key in shortcutHintKeys('showShortcuts')" :key="key">{{ key }}</kbd>
        </span>
        <span class="command-hint-text">
          {{ $t('mainPanel.commandHintSuffix') }}
        </span>
      </span>
      <button
        data-testid="command-hint-dismiss"
        type="button"
        class="command-hint-dismiss"
        :aria-label="$t('actions.dismiss')"
        @click="dismissCommandHint"
      >
        ×
      </button>
    </div>
  </main>
</template>

<style scoped>
.agent-live-content { display: flex; flex-direction: column; flex: 1; min-height: 0; height: 100%; }
.action-spacer { flex: 1; }
.work-area { display: flex; flex: 1; min-height: 0; min-width: 0; overflow: hidden; }
.reference-area, .reference-view { display: flex; flex-direction: column; flex: 1; min-width: 0; min-height: 0; overflow: hidden; }
.work-area > .main-tab-panel { min-width: 0; }


.work-area.split > .pane-focused { box-shadow: inset 0 2px var(--kn-accent); }



.main-panel {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  background: var(--kn-bg-app);
}

.main-tab-panel {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
}

.review-merge {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  padding: 9px 12px;
  border-bottom: 1px solid var(--kn-border);
  background: var(--kn-bg-subtle, var(--kn-bg-app));
}

.review-merge-copy {
  min-width: 0;
}

.review-merge-title {
  margin: 0;
  color: var(--kn-text-primary);
  font-size: 13px;
  font-weight: 600;
}

.review-merge-detail {
  margin: 2px 0 0;
  color: var(--kn-text-secondary);
  font-size: 12px;
  overflow-wrap: anywhere;
}

.review-merge-warning {
  margin: 2px 0 0;
  color: var(--kn-warning);
  font-size: 12px;
  overflow-wrap: anywhere;
}

.revision-exhausted {
  display: flex;
  align-items: center;
  padding: 10px 16px;
  border-bottom: 1px solid var(--kn-warning);
  background: var(--kn-warning-bg);
}

.revision-exhausted-title {
  margin: 0;
  color: var(--kn-text-primary);
  font-size: 13px;
  font-weight: 600;
}

.revision-exhausted-hint {
  margin: 2px 0 0;
  color: var(--kn-text-muted);
  font-size: 11px;
}

.empty-state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
}

.terminal-policy-loading {
  flex: 1;
  min-height: 0;
}

.setup-placeholder {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--kn-text-muted);
  font-size: 13px;
}

.cloud-task-placeholder {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  color: var(--kn-text-muted);
  font-size: 13px;
  text-align: center;
}

.cloud-task-title {
  margin: 0;
  color: var(--kn-text-primary);
  font-size: 14px;
}

.cloud-task-hint {
  margin: 0;
  max-width: 360px;
  line-height: 1.4;
}

.empty-title {
  font-size: 15px;
  font-weight: 500;
  color: var(--kn-text-muted);
}

.empty-hint {
  font-size: 12px;
  color: var(--kn-text-muted);
}

.empty-hint kbd {
  background: var(--kn-bg-panel-raised);
  border: 1px solid var(--kn-border-strong);
  border-radius: 3px;
  padding: 1px 5px;
  font-family: inherit;
  font-size: 11px;
  color: var(--kn-text-muted);
}

.empty-hint kbd + kbd {
  margin-left: 2px;
}

.blocked-placeholder {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 32px;
  max-width: 600px;
  margin: 0 auto;
}

.blocked-title {
  font-size: 18px;
  font-weight: 600;
  color: var(--kn-text-muted);
}

.blocked-prompt {
  font-size: 13px;
  color: var(--kn-text-muted);
  text-align: center;
  white-space: pre-wrap;
  max-height: 200px;
  overflow-y: auto;
}

.blocked-by {
  width: 100%;
  margin-top: 8px;
}

.blocked-by-label {
  font-size: 12px;
  color: var(--kn-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  margin-bottom: 6px;
}

.blocker-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  background: var(--kn-bg-panel);
  border-radius: 4px;
  margin-bottom: 4px;
}

.blocker-status {
  font-size: 11px;
  font-weight: 600;
  min-width: 80px;
}

.blocker-name {
  font-size: 12px;
  color: var(--kn-text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.blocked-hint {
  font-size: 11px;
  color: var(--kn-text-muted);
  margin-top: 8px;
}

.command-hint {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 14px;
  border-top: 1px solid var(--kn-border-default);
  background: var(--kn-bg-app);
  color: var(--kn-text-muted);
  font-size: 12px;
}

.command-hint-copy {
  display: inline-flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
}

.command-hint-shortcut {
  display: inline-flex;
  align-items: center;
}

.command-hint-copy kbd {
  background: var(--kn-bg-panel);
  border: 1px solid var(--kn-border-strong);
  border-radius: 4px;
  padding: 1px 5px;
  font-family: inherit;
  font-size: 11px;
  color: var(--kn-text-secondary);
}

.command-hint-copy kbd + kbd {
  margin-left: 2px;
}

.command-hint-dismiss {
  border: 0;
  background: transparent;
  color: var(--kn-text-muted);
  font-size: 16px;
  line-height: 1;
  cursor: pointer;
  padding: 2px;
}

.command-hint-dismiss:hover {
  color: var(--kn-text-muted);
}

.agent-setup {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 16px;
  max-width: 480px;
  margin: 0 auto;
  padding: 32px;
}

.setup-title {
  font-size: 15px;
  font-weight: 500;
  color: var(--kn-text-muted);
  margin-bottom: 4px;
}

.agent-cards {
  display: flex;
  flex-direction: column;
  gap: 14px;
  width: 100%;
}

.agent-group {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.agent-group-title {
  margin: 0;
  font-size: 11px;
  font-weight: 600;
  color: var(--kn-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.agent-card {
  background: var(--kn-bg-panel-raised);
  border: 1px solid var(--kn-border-default);
  border-radius: 8px;
  padding: 14px 16px;
}

.agent-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.agent-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--kn-text-secondary);
}

.agent-badge {
  font-size: 11px;
  padding: 2px 8px;
  border-radius: 4px;
}

.agent-badge.installed {
  color: var(--kn-success);
  background: var(--kn-success-bg);
}

.agent-badge.not-installed {
  color: var(--kn-text-muted);
  background: var(--kn-bg-panel-raised);
}

.checkmark {
  margin-right: 4px;
}

.install-block {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 10px;
}

.install-cmd {
  flex: 1;
  font-size: 11px;
  font-family: monospace;
  color: var(--kn-text-muted);
  background: var(--kn-bg-input);
  border: 1px solid var(--kn-border-default);
  border-radius: 4px;
  padding: 6px 10px;
  overflow-x: auto;
  white-space: nowrap;
}

.copy-btn {
  background: var(--kn-bg-panel-raised);
  border: 1px solid var(--kn-border-strong);
  border-radius: 4px;
  color: var(--kn-text-muted);
  font-size: 13px;
  padding: 4px 8px;
  cursor: pointer;
  flex-shrink: 0;
}

.copy-btn:hover {
  background: var(--kn-bg-hover);
  color: var(--kn-text-secondary);
}

.setup-hint {
  font-size: 12px;
  color: var(--kn-text-muted);
}

.mobile-back-bar {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 10px 14px;
  background: var(--kn-bg-panel-raised);
  border-bottom: 1px solid var(--kn-border-default);
  color: var(--kn-accent);
  font-size: 14px;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.mobile-back-arrow {
  font-size: 18px;
}
</style>

<style scoped>
.work-area { position: relative; }
.reference-area { display: contents; }
.pane-chrome { position: absolute; pointer-events: none; box-sizing: border-box; border: 1px solid var(--kn-border-default); }
.pane-chrome :deep(.main-tab-bar) { pointer-events: auto; height: 34px; box-sizing: border-box; }
.empty-pane { pointer-events: auto; height: calc(100% - 34px); display: grid; place-items: center; color: var(--kn-text-muted); font-size: 12px; }
.pane-divider { position: absolute; z-index: 3; touch-action: none; }
.pane-divider:hover, .pane-divider:focus-visible { background: var(--kn-accent); }
</style>
