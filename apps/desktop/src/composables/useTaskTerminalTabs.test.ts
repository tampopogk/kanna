import { computed, ref } from "vue";
import { describe, expect, it, vi } from "vitest";

import type { DesktopTaskTerminal, DesktopTaskTerminals } from "../services/desktopServerClient";
import { useMainTabs } from "./useMainTabs";
import { isOwnTabTerminal, terminalTabTitle, useTaskTerminalTabs } from "./useTaskTerminalTabs";

function terminal(overrides: Partial<DesktopTaskTerminal>): DesktopTaskTerminal {
  return {
    id: "setup-task-1-1",
    taskId: "task-1",
    repoId: "repo-1",
    daemonSessionId: "setup-task-1-1",
    role: "setup",
    stage: "in progress",
    attempt: 1,
    state: "live",
    stageRunId: null,
    title: null,
    cwd: "/tmp/wt",
    exitCode: null,
    createdAt: "2026-09-08T00:00:00Z",
    retiredAt: null,
    ...overrides,
  };
}

function tabsForTask(taskId: string) {
  const scopeKey = ref<string | null>(`item:${taskId}`);
  return useMainTabs({ scopeKey: computed(() => scopeKey.value) });
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

describe("which terminals get a tab", () => {
  it("gives the launch's startup and teardown shells their own tabs", () => {
    expect(isOwnTabTerminal(terminal({ role: "setup" }))).toBe(true);
    expect(isOwnTabTerminal(terminal({ role: "teardown" }))).toBe(true);
  });

  it("leaves the agent session to the agent tab, including a pre-split one", () => {
    // Repeating the agent's own session here would show one terminal twice —
    // once as the agent tab, once as a terminal tab pointing at the same PTY.
    expect(isOwnTabTerminal(terminal({ role: "agent" }))).toBe(false);
    expect(isOwnTabTerminal(terminal({ role: "legacy_agent" }))).toBe(false);
  });

  it("keeps a finished agent attempt as history of its own", () => {
    // A stage advance or a retry respawns the same session id, so without a
    // tab per attempt the previous stage's output is simply gone — which is
    // the chaining this architecture replaces.
    expect(isOwnTabTerminal(terminal({ role: "agent", state: "retired" }))).toBe(true);
    expect(
      terminalTabTitle(terminal({ role: "agent", state: "retired", title: null, stage: "review", attempt: 2 })),
    ).toBe("Agent · review · attempt 2");
  });

  it("names a terminal by the stage that launched it", () => {
    expect(terminalTabTitle(terminal({ title: "Startup · review" }))).toBe("Startup · review");
    expect(terminalTabTitle(terminal({ title: null, stage: "review" }))).toBe("Startup · review");
    expect(terminalTabTitle(terminal({ title: null, stage: null, role: "teardown" })))
      .toBe("Teardown");
  });
});

describe("keeping a task's terminal tabs in step with the server", () => {
  it("opens a tab for each launch's startup terminal without stealing focus", async () => {
    const tabs = tabsForTask("task-1");
    const fetchTerminals = vi.fn(async (): Promise<DesktopTaskTerminals> => ({
      taskId: "task-1",
      agentSessionId: "task-1",
      terminals: [
        terminal({ id: "setup-task-1-1", daemonSessionId: "setup-task-1-1", stage: "in progress" }),
        terminal({
          id: "setup-task-1-2",
          daemonSessionId: "setup-task-1-2",
          stage: "review",
          attempt: 2,
        }),
        terminal({ id: "agent-task-1", daemonSessionId: "task-1", role: "agent" }),
      ],
    }));

    const { reconcile } = useTaskTerminalTabs({
      tabs,
      taskId: computed(() => "task-1"),
      revision: computed(() => 1),
      fetchTerminals,
    });
    await reconcile();

    expect(tabs.tabs.value.map((tab) => tab.id)).toEqual([
      "agent",
      // The workspace log comes with the task's own terminals; it is the
      // chronological view they are entries in.
      "workspace",
      "terminal:setup-task-1-1",
      "terminal:setup-task-1-2",
    ]);
    // A stage's startup terminal appearing must not pull the reader off
    // whatever they were looking at.
    expect(tabs.activeTabId.value).toBe("agent");
  });

  it("carries a finished startup terminal's exit status onto its tab", async () => {
    // A startup shell that failed is the whole reason its output is kept, so
    // the status has to reach the tab that renders the banner: without it a
    // setup that exited 23 read as an ordinary finish.
    const tabs = tabsForTask("task-1");
    const state = ref<DesktopTaskTerminal>(
      terminal({ state: "live", exitCode: null, archived: false }),
    );
    const fetchTerminals = vi.fn(async (): Promise<DesktopTaskTerminals> => ({
      taskId: "task-1",
      agentSessionId: "task-1",
      terminals: [state.value],
    }));
    const { reconcile } = useTaskTerminalTabs({
      tabs,
      taskId: computed(() => "task-1"),
      revision: computed(() => 1),
      fetchTerminals,
    });

    await reconcile();
    const live = tabs.tabs.value.find((tab) => tab.id === "terminal:setup-task-1-1");
    expect(live?.terminalLive).toBe(true);
    expect(live?.terminalExitCode).toBeNull();

    // The same terminal, once its shell has exited: re-reconciling has to
    // update the tab that is already open rather than leave it saying the
    // startup is still running.
    state.value = terminal({ state: "retired", exitCode: 23, archived: true });
    await reconcile();

    const retired = tabs.tabs.value.find((tab) => tab.id === "terminal:setup-task-1-1");
    expect(retired?.terminalLive).toBe(false);
    expect(retired?.terminalArchived).toBe(true);
    expect(retired?.terminalExitCode).toBe(23);
  });

  it("gives each finished agent attempt its own tab rather than one shared id", async () => {
    const tabs = tabsForTask("task-1");
    const fetchTerminals = vi.fn(async (): Promise<DesktopTaskTerminals> => ({
      taskId: "task-1",
      agentSessionId: "task-1",
      terminals: [
        // Both attempts ran in the same daemon session; only their records
        // tell them apart.
        terminal({
          id: "agent-task-1-1",
          daemonSessionId: "task-1",
          role: "agent",
          state: "retired",
          stage: "in progress",
          attempt: 1,
          exitCode: 0,
        }),
        terminal({
          id: "agent-task-1-2",
          daemonSessionId: "task-1",
          role: "agent",
          state: "retired",
          stage: "review",
          attempt: 2,
          exitCode: 0,
        }),
      ],
    }));

    const { reconcile } = useTaskTerminalTabs({
      tabs,
      taskId: computed(() => "task-1"),
      revision: computed(() => 1),
      fetchTerminals,
    });
    await reconcile();

    expect(tabs.tabs.value.map((tab) => tab.id)).toEqual([
      "agent",
      "terminal:agent-task-1-1",
      "terminal:agent-task-1-2",
    ]);
  });

  it("opens one read-only workspace log for the task's operations", async () => {
    // The log is what replaces a permanent startup tab per stage: workspace
    // creation, the startup script, the agent, teardown, then the next
    // stage's startup, in one place.
    const tabs = tabsForTask("task-1");
    const fetchTerminals = vi.fn(async (): Promise<DesktopTaskTerminals> => ({
      taskId: "task-1",
      agentSessionId: "task-1",
      terminals: [
        terminal({ id: "setup-task-1-1", daemonSessionId: "setup-task-1-1" }),
        terminal({
          id: "setup-task-1-2",
          daemonSessionId: "setup-task-1-2",
          stage: "review",
          attempt: 2,
        }),
      ],
    }));
    const { reconcile } = useTaskTerminalTabs({
      tabs,
      taskId: computed(() => "task-1"),
      revision: computed(() => 1),
      fetchTerminals,
    });

    await reconcile();
    await reconcile();

    expect(tabs.tabs.value.filter((tab) => tab.kind === "workspace")).toHaveLength(1);
    expect(tabs.activeTabId.value).toBe("agent");
  });

  it("brings the workspace log back after the reader closes it", async () => {
    // The log is the task's index, not one of its documents. Closing a
    // retained attempt is only safe because the log is where it is found
    // again, and nothing else in the app opens the log — so a closed log that
    // stayed closed stranded every attempt behind it.
    const tabs = tabsForTask("task-1");
    const fetchTerminals = vi.fn(async (): Promise<DesktopTaskTerminals> => ({
      taskId: "task-1",
      agentSessionId: "task-1",
      terminals: [terminal({ id: "setup-task-1-1", daemonSessionId: "setup-task-1-1" })],
    }));
    const { reconcile } = useTaskTerminalTabs({
      tabs,
      taskId: computed(() => "task-1"),
      revision: computed(() => 1),
      fetchTerminals,
    });

    await reconcile();
    expect(tabs.isOpen("workspace")).toBe(true);

    tabs.closeTab("workspace");
    expect(tabs.isOpen("workspace")).toBe(false);

    await reconcile();
    expect(tabs.isOpen("workspace")).toBe(true);
    // And it comes back where it was, without taking the reader off the agent.
    expect(tabs.activeTabId.value).toBe("agent");
  });

  it("reports the server's answer on whether a launch can still start the agent", async () => {
    // A failed launch leaves no runtime state — it never had a session to
    // report one — so the agent view cannot tell it from a startup terminal
    // that is still running. This is the fact it waits on instead.
    const tabs = tabsForTask("task-1");
    const pending = ref<boolean | undefined>(true);
    const fetchTerminals = vi.fn(async (): Promise<DesktopTaskTerminals> => ({
      taskId: "task-1",
      agentSessionId: "task-1",
      terminals: [terminal({})],
      agentLaunchPending: pending.value,
    }));
    const { reconcile, agentLaunchPending } = useTaskTerminalTabs({
      tabs,
      taskId: computed(() => "task-1"),
      revision: computed(() => 1),
      fetchTerminals,
    });

    await reconcile();
    expect(agentLaunchPending.value).toBe(true);

    pending.value = false;
    await reconcile();
    expect(agentLaunchPending.value).toBe(false);
  });

  it("reopens a closed attempt tab when the workspace log asks for it", async () => {
    // Reconciliation never reopens a tab the reader closed — that is their
    // decision — which is exactly why the log must be able to, and why the
    // reopened tab has to come back labelled and with its status rather than
    // as an anonymous terminal that finished for no stated reason.
    const tabs = tabsForTask("task-1");
    const fetchTerminals = vi.fn(async (): Promise<DesktopTaskTerminals> => ({
      taskId: "task-1",
      agentSessionId: "task-1",
      terminals: [
        terminal({
          id: "agent-task-1-1",
          daemonSessionId: "task-1",
          role: "agent",
          state: "retired",
          stage: "in progress",
          attempt: 1,
          exitCode: 0,
          title: "Agent · in progress · attempt 1",
        }),
      ],
    }));
    const { reconcile } = useTaskTerminalTabs({
      tabs,
      taskId: computed(() => "task-1"),
      revision: computed(() => 1),
      fetchTerminals,
    });

    await reconcile();
    expect(tabs.isOpen("terminal:agent-task-1-1")).toBe(true);

    tabs.closeTab("terminal:agent-task-1-1");
    await reconcile();
    expect(tabs.isOpen("terminal:agent-task-1-1")).toBe(false);

    // What the Workspace log's Open button does, with what its entry carries.
    tabs.openTab({
      kind: "terminal",
      terminalSessionId: "agent-task-1-1",
      terminalTitle: "Agent · in progress · attempt 1",
      terminalTaskId: "task-1",
      terminalLive: false,
      terminalArchived: true,
      terminalExitCode: 0,
    });

    const reopened = tabs.tabs.value.find((tab) => tab.id === "terminal:agent-task-1-1");
    expect(reopened?.terminalTitle).toBe("Agent · in progress · attempt 1");
    expect(reopened?.terminalLive).toBe(false);
    expect(reopened?.terminalArchived).toBe(true);
  });

  it("does not reopen a terminal tab the reader closed", async () => {
    const tabs = tabsForTask("task-1");
    const fetchTerminals = vi.fn(async (): Promise<DesktopTaskTerminals> => ({
      taskId: "task-1",
      agentSessionId: "task-1",
      terminals: [terminal({})],
    }));
    const { reconcile } = useTaskTerminalTabs({
      tabs,
      taskId: computed(() => "task-1"),
      revision: computed(() => 1),
      fetchTerminals,
    });

    await reconcile();
    tabs.closeTab("terminal:setup-task-1-1");
    await reconcile();

    expect(tabs.isOpen("terminal:setup-task-1-1")).toBe(false);
  });

  it("keeps the tabs it has when the terminals cannot be read", async () => {
    const tabs = tabsForTask("task-1");
    let fail = false;
    const fetchTerminals = vi.fn(async (): Promise<DesktopTaskTerminals> => {
      if (fail) throw new Error("server unreachable");
      return { taskId: "task-1", agentSessionId: "task-1", terminals: [terminal({})] };
    });
    const { reconcile } = useTaskTerminalTabs({
      tabs,
      taskId: computed(() => "task-1"),
      revision: computed(() => 1),
      fetchTerminals,
    });

    await reconcile();
    fail = true;
    await reconcile();

    // A momentary server hiccup must not read as "the launch never happened".
    expect(tabs.isOpen("terminal:setup-task-1-1")).toBe(true);
  });

  it("records terminals against their own task, not the selected one", async () => {
    const selected = ref<string | null>("item:task-other");
    const tabs = useMainTabs({ scopeKey: computed(() => selected.value) });
    const fetchTerminals = vi.fn(async (): Promise<DesktopTaskTerminals> => ({
      taskId: "task-1",
      agentSessionId: "task-1",
      terminals: [terminal({})],
    }));
    const { reconcile } = useTaskTerminalTabs({
      tabs,
      taskId: computed(() => "task-1"),
      revision: computed(() => 1),
      fetchTerminals,
    });

    await reconcile();
    await settle();

    expect(tabs.isOpen("terminal:setup-task-1-1")).toBe(false);
    selected.value = "item:task-1";
    expect(tabs.isOpen("terminal:setup-task-1-1")).toBe(true);
  });
});
