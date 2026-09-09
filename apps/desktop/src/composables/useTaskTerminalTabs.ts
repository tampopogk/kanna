import { onScopeDispose, watch, type ComputedRef } from "vue";

import {
  fetchDesktopTaskTerminals,
  type DesktopTaskTerminal,
} from "../services/desktopServerClient";
import { mainTabScopeKeyForTask, type MainTabsController } from "./useMainTabs";
import { listen } from "../listen";

/**
 * Which of a task's terminals get a tab of their own.
 *
 * The agent's session is already the `agent` tab, so it is not repeated here;
 * so is a pre-split `legacy_agent` session, which *is* that task's agent
 * terminal and would otherwise show up twice. What is left is the startup
 * shell each launch runs its setup in, and the teardown of a workspace the
 * task has left — the terminals that had nowhere to be shown before, because
 * their output used to be mixed into the agent's scrollback or, worse for
 * teardown and for a stage's setup, printed nowhere at all.
 */
export function isOwnTabTerminal(terminal: DesktopTaskTerminal): boolean {
  return (
    Boolean(terminal.daemonSessionId)
    && (terminal.role === "setup" || terminal.role === "teardown")
  );
}

/**
 * How a terminal is named in the tab bar.
 *
 * A task has one startup terminal per launch, so the stage is what tells them
 * apart; without one — a launch on a task with no stage recorded — the role
 * alone still says what it is.
 */
export function terminalTabTitle(terminal: DesktopTaskTerminal): string {
  if (terminal.title) return terminal.title;
  const role = terminal.role === "teardown" ? "Teardown" : "Startup";
  return terminal.stage ? `${role} · ${terminal.stage}` : role;
}

interface UseTaskTerminalTabsOptions {
  tabs: MainTabsController;
  /** The task whose terminals should be on screen, or null for none. */
  taskId: ComputedRef<string | null>;
  /**
   * Changes whenever the server may have recorded a new terminal. The store
   * replaces its snapshot on every state change, so re-reading on that edge is
   * what makes a stage advance's startup terminal appear without polling.
   */
  revision: ComputedRef<unknown>;
  fetchTerminals?: typeof fetchDesktopTaskTerminals;
}

/**
 * Whether a daemon session id names one of a task's own-tab terminals.
 *
 * A startup terminal is `setup-{task}-{attempt}` and a teardown is
 * `td-{branch}`; the agent's session is the task id itself. This is the edge
 * that says "the server has just recorded a terminal", which the task row does
 * not: a stage advance writes nothing to `pipeline_item` until its transition
 * lands, so for the whole of that stage's setup the snapshot revision the tabs
 * otherwise reconcile on never moves.
 */
function isOwnTabTerminalSessionId(sessionId: string): boolean {
  return sessionId.startsWith("setup-") || sessionId.startsWith("td-");
}

/**
 * Keep a task's terminal tabs in step with the terminals the server says it
 * has.
 *
 * Tabs are *opened*, never closed, from this reconciliation. A launch's
 * startup terminal stays readable after it exits — that scrollback is the
 * record of what the stage's setup did, and a stage that has moved on is
 * exactly when someone wants it — and a tab the reader closed themselves is
 * their decision, which a later refresh must not undo. Closing one hides the
 * view; the session's record is untouched and reopening it shows the same
 * terminal.
 */
export function useTaskTerminalTabs({
  tabs,
  taskId,
  revision,
  fetchTerminals = fetchDesktopTaskTerminals,
}: UseTaskTerminalTabsOptions) {
  const openedByReconciliation = new Set<string>();

  async function reconcile(): Promise<void> {
    const id = taskId.value;
    if (!id) return;
    let terminals: DesktopTaskTerminal[];
    try {
      terminals = (await fetchTerminals(id)).terminals;
    } catch (error: unknown) {
      // A task whose terminals cannot be read keeps whatever tabs it has. The
      // alternative — dropping them — would make a momentary server hiccup
      // look like the launch never happened.
      console.warn("[task-terminals] could not read the task's terminals:", error);
      return;
    }
    if (taskId.value !== id) return;
    const scope = mainTabScopeKeyForTask(id);
    for (const terminal of terminals) {
      if (!isOwnTabTerminal(terminal)) continue;
      const sessionId = terminal.daemonSessionId;
      if (!sessionId) continue;
      const key = `${id}:${sessionId}`;
      const descriptor = {
        kind: "terminal" as const,
        terminalSessionId: sessionId,
        terminalTitle: terminalTabTitle(terminal),
        terminalLive: terminal.state === "live",
        terminalArchived: terminal.archived,
        terminalTaskId: id,
      };
      if (openedByReconciliation.has(key) && !tabs.isOpen(`terminal:${sessionId}`)) {
        // The reader closed it. Leave it closed.
        continue;
      }
      openedByReconciliation.add(key);
      // Never steal focus: a startup terminal appearing must not pull the
      // reader off whatever they were looking at, in this task or another.
      tabs.openTabInScope(scope, descriptor, { activate: false });
    }
  }

  watch([taskId, revision], () => void reconcile(), { immediate: true });

  // The other edge: a terminal the server has just started. Reconciling here
  // is what makes a stage advance's startup tab appear while its setup is
  // still running, instead of only when the reader happens to reselect the
  // task. It is an event, not a timer — a session that is never created
  // costs nothing.
  const listening = listen("session_created", (event: unknown) => {
    const sessionId = (event as { payload?: { session_id?: string } } | undefined)
      ?.payload?.session_id;
    if (!sessionId || !isOwnTabTerminalSessionId(sessionId)) return;
    void reconcile();
  }).catch((error: unknown) => {
    console.warn("[task-terminals] could not watch for new terminals:", error);
    return null;
  });
  onScopeDispose(() => {
    void listening.then((unlisten) => unlisten?.());
  });

  return { reconcile };
}
