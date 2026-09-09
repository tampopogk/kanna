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
      "terminal:setup-task-1-1",
      "terminal:setup-task-1-2",
    ]);
    // A stage's startup terminal appearing must not pull the reader off
    // whatever they were looking at.
    expect(tabs.activeTabId.value).toBe("agent");
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
