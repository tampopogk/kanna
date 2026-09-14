import { describe, expect, it, vi } from "vitest";
import type { PersistedMainTabs } from "./useMainTabs";

import {
  mainTabDescriptorForCommand,
  parseDesktopViewOpenCommand,
  performDesktopViewOpen,
  diffViewTarget,
  fileViewTarget,
  type DesktopViewOpenCommand,
  type DesktopViewOpenDeps,
} from "./desktopViewOpen";

function command(overrides: Partial<DesktopViewOpenCommand> = {}): DesktopViewOpenCommand {
  return { requestId: "view-1", taskId: "task-a", view: "file", ...overrides };
}

function deps(overrides: Partial<DesktopViewOpenDeps> = {}): DesktopViewOpenDeps {
  return {
    findTaskSlotId: (taskId) => (taskId === "task-a" ? "slot-a" : null),
    refreshTasks: async () => {},
    selectTask: async () => {},
    openTab: () => "file:src/main.rs",
    revealTab: async () => ({ opened: true }),
    ...overrides,
  };
}

describe("parseDesktopViewOpenCommand", () => {
  it("refuses a command it cannot act on rather than guessing", () => {
    expect(() => parseDesktopViewOpenCommand(null)).toThrow();
    expect(() => parseDesktopViewOpenCommand({ taskId: "task-a", view: "file" })).toThrow();
    expect(() => parseDesktopViewOpenCommand({ requestId: "r", taskId: "task-a", view: "shell" }))
      .toThrow();
    expect(() => parseDesktopViewOpenCommand({
      requestId: "r",
      taskId: "task-a",
      view: "file",
      target: "src/main.rs",
    })).toThrow();
  });

  it("reads the whitelisted views and their target object", () => {
    expect(parseDesktopViewOpenCommand({
      requestId: "view-7",
      taskId: "task-a",
      view: "diff",
      target: { scope: "working", path: "src/main.rs", side: "new", line: 4 },
    })).toEqual({
      requestId: "view-7",
      taskId: "task-a",
      view: "diff",
      target: { scope: "working", path: "src/main.rs", side: "new", line: 4 },
    });
  });
});

describe("mainTabDescriptorForCommand", () => {
  it("identifies a file tab by its path, so a second open re-aims it", () => {
    expect(mainTabDescriptorForCommand(command({
      target: { path: "src/main.rs", line: 12 },
    }))).toEqual({
      kind: "file",
      filePath: "src/main.rs",
      initialLine: 12,
      // The view reads back through the server's contained resolution, so the
      // task whose worktree bounds it travels with the tab.
      containedTaskId: "task-a",
    });
  });

  it("gives every other view one tab per task", () => {
    expect(mainTabDescriptorForCommand(command({ view: "diff", target: { scope: "branch" } })))
      .toEqual({ kind: "diff" });
    expect(mainTabDescriptorForCommand(command({ view: "tree", target: { path: "src" } })))
      .toEqual({ kind: "tree", containedTaskId: "task-a" });
    expect(mainTabDescriptorForCommand(command({ view: "agent", target: undefined })))
      .toEqual({ kind: "agent" });
  });
});

describe("target readers", () => {
  it("keeps only positions that are positions", () => {
    expect(fileViewTarget(command({ target: { path: "a.txt", line: 0, column: -3 } })))
      .toEqual({ path: "a.txt", line: undefined, column: undefined, endLine: undefined, endColumn: undefined });
    expect(fileViewTarget(command({ target: {} }))).toBeNull();
  });

  it("defaults a diff target to the branch scope", () => {
    expect(diffViewTarget(command({ view: "diff", target: undefined })).scope).toBe("branch");
    expect(diffViewTarget(command({ view: "diff", target: { scope: "working" } })).scope)
      .toBe("working");
  });
});

describe("performDesktopViewOpen", () => {
  it("selects the task, opens its tab, and reports what the view says", async () => {
    const selected: string[] = [];
    const opened: Array<[string, unknown]> = [];
    const outcome = await performDesktopViewOpen(
      command({ target: { path: "src/main.rs", line: 3 } }),
      deps({
        selectTask: async (slotId) => {
          selected.push(slotId);
        },
        openTab: (scopeKey, descriptor) => {
          opened.push([scopeKey, descriptor]);
          return "file:src/main.rs";
        },
      }),
    );
    expect(outcome).toEqual({ opened: true });
    expect(selected).toEqual(["slot-a"]);
    expect(opened).toEqual([[
      "item:task-a",
      {
        kind: "file",
        filePath: "src/main.rs",
        initialLine: 3,
        containedTaskId: "task-a",
      },
    ]]);
  });

  it("reloads once for a task this window has not heard of, then gives up honestly", async () => {
    const refreshTasks = vi.fn(async () => {});
    const outcome = await performDesktopViewOpen(
      command({ taskId: "task-unknown" }),
      deps({ refreshTasks }),
    );
    expect(refreshTasks).toHaveBeenCalledTimes(1);
    expect(outcome.opened).toBe(false);
    expect(outcome.code).toBe("task_not_found");
  });

  it("takes the task a reload turned up", async () => {
    let known = false;
    const outcome = await performDesktopViewOpen(
      command({ taskId: "task-late", target: { path: "a.txt" } }),
      deps({
        findTaskSlotId: (taskId) => (known && taskId === "task-late" ? "slot-late" : null),
        refreshTasks: async () => {
          known = true;
        },
      }),
    );
    expect(outcome).toEqual({ opened: true });
  });

  it("turns a throwing step into a failure the caller can read", async () => {
    const outcome = await performDesktopViewOpen(
      command({ target: { path: "a.txt" } }),
      deps({
        revealTab: async () => {
          throw new Error("the view exploded");
        },
      }),
    );
    expect(outcome.opened).toBe(false);
    expect(outcome.code).toBe("renderer_failed");
    expect(outcome.message).toContain("the view exploded");
  });

  it("passes a view's own refusal through unchanged", async () => {
    const outcome = await performDesktopViewOpen(
      command({ view: "graph", target: { commit: "a".repeat(40) } }),
      deps({
        openTab: () => "graph",
        revealTab: async () => ({ opened: false, code: "commit_not_found", message: "gone" }),
      }),
    );
    expect(outcome).toEqual({ opened: false, code: "commit_not_found", message: "gone" });
  });
});

// Exercise the command parser and production pane controller together; view
// readiness is held explicitly so a queued/loading view cannot be acknowledged.
describe("workspace command/controller contract", () => {
  async function fixture(persisted?: PersistedMainTabs) {
    const { computed, ref, nextTick } = await import("vue");
    const { useMainTabs } = await import("./useMainTabs");
    const selected = ref("task-a");
    const branch = ref("task-task-a");
    const tabs = useMainTabs({ scopeKey: computed(() => `item:${selected.value}`) });
    tabs.restoreScopes(persisted ?? null);
    const dependencies = deps({
      selectTask: async () => { selected.value = "task-a"; },
      openTab: (key, descriptor) => tabs.openTabInScope(key, descriptor),
      workspace: {
        tabs, windowId: "main", currentBranch: () => branch.value, rendered: nextTick,
        presentation: () => tabs.panes.value.map(({ pane, ...rect }) => ({ id: pane.id, activeTabId: pane.active, ...rect })),
      },
    });
    const run = (overrides: Partial<DesktopViewOpenCommand>) => performDesktopViewOpen(parseDesktopViewOpenCommand(command({
      branch: branch.value, windowId: "main", view: "agent", ...overrides,
    })), dependencies);
    const initial = await run({ operation: "inspect" });
    const address = { windowId: "main", workspaceId: initial.workspace!.workspaceId, paneId: initial.workspace!.panes[0].id };
    return { tabs, run, address, selected, branch, dependencies };
  }

  it.each(["horizontal", "vertical"] as const)("inspect -> split %s -> open second pane -> move preserves identity/state", async direction => {
    const { tabs, run, address, dependencies } = await fixture();
    const split = await run({ operation: "split", direction, ...address });
    expect(split.opened).toBe(true);
    const second = split.workspace!.panes[1];
    expect(second.order).toBe(2);
    expect(direction === "horizontal" ? second.left : second.top).toBe(50);
    // Focus the first pane to prove explicit destination outranks focus.
    tabs.focusPane(address.paneId);
    let ready!: () => void;
    dependencies.revealTab = async () => { await new Promise<void>(resolve => { ready = resolve; }); return { opened: true }; };
    const pending = run({ view: "file", target: { path: "AGENTS.md" }, ...address, paneId: second.id });
    await vi.waitFor(() => expect(ready).toBeTypeOf("function"));
    expect(tabs.panes.value[1].pane.tabs).toEqual(["file:AGENTS.md"]);
    ready();
    const opened = await pending;
    expect(opened).toMatchObject({ opened: true, paneId: second.id, tabId: "file:AGENTS.md" });
    expect(opened.workspace?.activeTabId).toBe("file:AGENTS.md");
    const file = tabs.tabs.value.find(tab => tab.id === opened.tabId)!;
    tabs.updateReading(file.id, { workspace: "task-task-a", top: 123 });
    const moved = await run({ operation: "move", tabId: file.id, ...address });
    expect(moved).toMatchObject({ opened: true, paneId: address.paneId, tabId: file.id });
    expect(tabs.tabs.value.find(tab => tab.id === file.id)).toBe(file);
    expect(file.reading?.top).toBe(123);
    expect(moved.workspace?.panes).toHaveLength(1); // empty source removed
    const resplit = await run({ operation: "split", direction, tabId: file.id, ...address });
    expect(resplit.paneId).not.toBe(second.id); // stale pane ids never alias a replacement
    expect(tabs.tabs.value.filter(tab => tab.id === file.id)).toHaveLength(1);
    expect(tabs.tabs.value.find(tab => tab.id === file.id)).toBe(file);
  });

  it.each(["open", "move"] as const)("reserves restored pane IDs before closure and refuses stale %s without mutation", async operation => {
    const saved: PersistedMainTabs = {
      version: 1,
      scopes: { "item:task-a": {
        tabs: [{ kind: "file", filePath: "kept.md" }], activeId: "agent",
        layout: { kind: "split", axis: "horizontal", ratio: .5,
          first: { kind: "pane", id: "pane-1", tabs: ["agent"], active: "agent" },
          second: { kind: "pane", id: "pane-2", tabs: ["file:kept.md"], active: "file:kept.md" },
        },
      } },
    };
    const { tabs, run, address } = await fixture(saved);
    const inspected = (await run({ operation: "inspect" })).workspace!;
    const staleAddress = { ...address, paneId: inspected.panes[1].id };
    tabs.closePane(staleAddress.paneId);
    const replacement = await run({ operation: "split", direction: "vertical", ...address });
    expect(replacement.opened).toBe(true);
    const before = (await run({ operation: "inspect" })).workspace!;
    const existingTab = tabs.tabs.value.find(tab => tab.id === "file:kept.md");
    const outcome = await run(operation === "open"
      ? { view: "file", target: { path: "AGENTS.md" }, ...staleAddress }
      : { operation: "move", tabId: "file:kept.md", ...staleAddress });
    expect(outcome).toMatchObject({ opened: false, code: "pane_not_found" });
    expect(replacement.paneId).not.toBe(staleAddress.paneId);
    expect((await run({ operation: "inspect" })).workspace).toEqual(before);
    expect(tabs.tabs.value.find(tab => tab.id === "file:kept.md")).toBe(existingTab);

    // The new destination still opens/selects normally and moves the same tab.
    const opened = await run({ view: "file", target: { path: "AGENTS.md" }, ...address, paneId: replacement.paneId });
    expect(opened).toMatchObject({ opened: true, paneId: replacement.paneId, tabId: "file:AGENTS.md" });
    expect(tabs.activeTabId.value).toBe(opened.tabId);
    const file = tabs.tabs.value.find(tab => tab.id === opened.tabId);
    expect(await run({ operation: "move", tabId: opened.tabId, ...address }))
      .toMatchObject({ opened: true, paneId: address.paneId, tabId: opened.tabId });
    expect(tabs.tabs.value.find(tab => tab.id === opened.tabId)).toBe(file);
    expect(tabs.activeTabId.value).toBe(opened.tabId);

    // Reservations are local to a renderer incarnation; restoring persisted
    // state must still issue a fresh workspace fence and reject old addresses.
    const reloaded = await fixture(JSON.parse(JSON.stringify(tabs.snapshotScopes())));
    expect(reloaded.address.workspaceId).not.toBe(address.workspaceId);
    const reloadBefore = (await reloaded.run({ operation: "inspect" })).workspace;
    expect(await reloaded.run({ operation: "move", tabId: "file:kept.md", ...address }))
      .toMatchObject({ opened: false, code: "stale_workspace" });
    expect((await reloaded.run({ operation: "inspect" })).workspace).toEqual(reloadBefore);
  });

  it("rejects missing/stale/cross-task identities before changing tabs", async () => {
    const { tabs, run, address, dependencies, branch } = await fixture();
    for (const [overrides, code] of [
      [{ paneId: "gone" }, "pane_not_found"],
      [{ workspaceId: tabs.workspaceIdentity("item:task-b", branch.value) }, "stale_workspace"],
      [{ windowId: "other-window" }, "window_not_found"],
      [{ branch: "previous-branch" }, "workspace_unavailable"],
      [{ expiresAt: 1 }, "request_expired"],
    ] as const) {
      expect(await run({ view: "file", target: { path: "AGENTS.md" }, ...address, ...overrides })).toMatchObject({ opened: false, code });
    }
    expect(await run({ operation: "move", ...address, tabId: "file:other-task.md" })).toMatchObject({ opened: false, code: "tab_not_found" });
    expect(tabs.tabs.value.map(tab => tab.id)).toEqual(["agent"]);
    dependencies.revealTab = async () => { branch.value = "next-stage"; return { opened: true }; };
    expect(await run({ view: "file", target: { path: "AGENTS.md" }, ...address })).toMatchObject({ opened: false, code: "workspace_unavailable" });
  });

  it("does not confirm a tab hidden or moved while its content loads", async () => {
    const { run, tabs, address, dependencies } = await fixture();
    const split = await run({ operation: "split", direction: "horizontal", ...address });
    dependencies.revealTab = async id => { tabs.moveTab(id, address.paneId); return { opened: true }; };
    expect(await run({ view: "file", target: { path: "AGENTS.md" }, ...address, paneId: split.paneId })).toMatchObject({ opened: false, code: "pane_not_found" });
    dependencies.workspace!.presentation = () => [];
    expect(await run({ view: "agent" })).toMatchObject({ opened: false, code: "renderer_failed" });
  });
});
