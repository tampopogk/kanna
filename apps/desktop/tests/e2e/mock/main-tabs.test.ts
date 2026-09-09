import { setTimeout as sleep } from "node:timers/promises";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { localProcessFetch } from "@kanna/local-process-fetch";
import { buildGlobalKeydownScript } from "../helpers/keyboard";
import { WebDriverClient } from "../helpers/webdriver";
import { cleanupFixtureRepos, createSeedFixtureRepo } from "../helpers/fixture-repo";
import { resolveAppKannaServer } from "../helpers/kannaServer";
import { cleanupWorktrees, importTestRepo, resetDatabase } from "../helpers/reset";
import { callVueMethod, getVueState, tauriInvoke } from "../helpers/vue";

/**
 * The main content area hosts a task's views as tabs: the agent session plus
 * whichever of the diff, a file, and the task shell the operator (or an agent
 * through `kanna_open_file`) has opened. These are the boundary-crossing parts
 * that unit tests cannot prove — the real keyboard path, the real xterm buffer
 * surviving a tab switch, and a server route reaching a live window.
 */
async function openTabIds(client: WebDriverClient): Promise<string[]> {
  return await client.executeSync<string[]>(
    `return Array.from(document.querySelectorAll('[data-testid="main-tab-bar"] [role="tab"]'))
      .map((tab) => (tab.getAttribute("data-testid") || "").replace(/^main-tab-/, ""));`
  );
}

async function activeTabId(client: WebDriverClient): Promise<string | null> {
  return await client.executeSync<string | null>(
    `const active = document.querySelector('[data-testid="main-tab-bar"] [role="tab"][aria-selected="true"]');
     return active ? (active.getAttribute("data-testid") || "").replace(/^main-tab-/, "") : null;`
  );
}

async function waitForActiveTab(
  client: WebDriverClient,
  id: string,
  timeoutMs = 8000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let latest: string | null = null;
  while (Date.now() < deadline) {
    latest = await activeTabId(client);
    if (latest === id) return;
    await sleep(150);
  }
  throw new Error(`expected active tab ${id}, got ${latest} of ${JSON.stringify(await openTabIds(client))}`);
}

/**
 * The app has renamed its selection entry point before, and an e2e helper that
 * silently missed it selects nothing while every later assertion still looks
 * plausible. Resolve it by trying each spelling.
 */
const SELECT_SIDEBAR_ITEM_SCRIPT = `
  function selectSidebarItem(ctx, id) {
    const select = ctx.selectSidebarItemById || ctx.handleSelectItem
      || (ctx.store && ctx.store.selectItem && ctx.store.selectItem.bind(ctx.store));
    if (!select) throw new Error("no sidebar selection entry point on setupState");
    return select(id);
  }
`;

async function pressShortcut(
  client: WebDriverClient,
  options: { key: string; meta?: boolean; shift?: boolean; alt?: boolean },
): Promise<void> {
  await client.executeSync(buildGlobalKeydownScript(options));
}

/**
 * ⌘W itself belongs to the native File menu in the packaged app, which
 * dispatches to this action — so a test presses it the way the menu does
 * rather than synthesising a keydown the web handler deliberately ignores.
 */
async function pressCloseTab(client: WebDriverClient): Promise<void> {
  const result = await callVueMethod(client, "keyboardActions.closeTabOrWindow");
  if (result && typeof result === "object" && "__error" in result) {
    throw new Error(String((result as { __error: string }).__error));
  }
  await sleep(200);
}

/** Tabs persist per task, so each test starts from the agent session alone. */
async function closeViewTabs(client: WebDriverClient): Promise<void> {
  for (let attempt = 0; attempt < 8; attempt += 1) {
    const closed = await client.executeSync<boolean>(
      `const close = document.querySelector('[data-testid^="main-tab-close-"]');
       if (!close) return false;
       close.click();
       return true;`
    );
    if (!closed) return;
    await sleep(120);
  }
  throw new Error(`tabs would not close: ${JSON.stringify(await openTabIds(client))}`);
}


/**
 * Arm a one-shot record of the sidebar task search taking focus. The global
 * ⌘F focuses it synchronously while the view focuses its own input a tick
 * later, so the end state hides the second binding having fired at all.
 */
async function watchSidebarSearchFocus(client: WebDriverClient): Promise<void> {
  await client.executeSync(
    `window.__sidebarSearchFocused = false;
     const input = document.querySelector(".sidebar .search-input");
     if (!input) throw new Error("sidebar search input is missing");
     input.addEventListener("focus", function () { window.__sidebarSearchFocused = true; }, { once: true });
     return true;`
  );
}

async function sidebarSearchWasFocused(client: WebDriverClient): Promise<boolean> {
  return client.executeSync<boolean>(`return window.__sidebarSearchFocused === true;`);
}

/**
 * Where the focused element lives, which is how these tests tell the view's own
 * find from the sidebar's task search: both render `.search-input`.
 */
async function focusedSearchOwner(client: WebDriverClient): Promise<string> {
  return client.executeSync<string>(
    `const active = document.activeElement;
     if (!active || !active.classList.contains("search-input")) return "none";
     return active.closest(".sidebar") ? "sidebar" : "view";`
  );
}

describe("main content area tabs", () => {
  const client = new WebDriverClient();
  let fixtureRepoRoot = "";
  let testRepoPath = "";
  let taskId = "";
  let secondTaskId = "";

  async function createTask(prompt: string): Promise<string> {
    const repoId = await getVueState(client, "selectedRepoId") as string;
    const id = crypto.randomUUID();
    const branch = `task-${id}`;
    const worktreePath = `${testRepoPath}/.kanna-worktrees/${branch}`;

    await tauriInvoke(client, "git_worktree_add", {
      repoPath: testRepoPath,
      branch,
      path: worktreePath,
    });
    await tauriInvoke(client, "run_script", {
      script: "printf '\\n# main tabs e2e\\n' >> README.md",
      cwd: worktreePath,
      env: {},
    });

    const created = await client.executeAsync<string>(
      `const cb = arguments[arguments.length - 1];
       const ctx = window.__KANNA_E2E__.setupState;
       ${SELECT_SIDEBAR_ITEM_SCRIPT}
       const db = ctx.db.value || ctx.db;
       db.execute("INSERT INTO pipeline_item (id, repo_id, prompt, stage, branch, agent_type) VALUES (?, ?, ?, ?, ?, ?)",
         ["${id}", "${repoId}", "${prompt}", "in progress", "${branch}", "agent"])
         // kanna_open_file resolves the file through the task's recorded
         // workspace, so the row has to exist as well as the directory.
         .then(function() {
           return db.execute("INSERT INTO worktree (id, pipeline_item_id, path, branch) VALUES (?, ?, ?, ?)",
             ["wt-${id}", "${id}", "${worktreePath}", "${branch}"]);
         })
         .then(function() { return ctx.loadItems("${repoId}"); })
         .then(function() { selectSidebarItem(ctx, "${id}"); return ctx.refreshAllItems ? ctx.refreshAllItems() : null; })
         .then(function() { cb("ok"); })
         .catch(function(e) { cb("err:" + (e && e.message ? e.message : String(e))); });`
    );
    if (typeof created === "string" && created.startsWith("err:")) {
      throw new Error(`creating task ${prompt} failed: ${created.slice(4)}`);
    }
    await client.waitForText(".sidebar", prompt);
    return id;
  }

  async function selectTask(id: string): Promise<void> {
    const result = await client.executeAsync<string>(
      `const cb = arguments[arguments.length - 1];
       try {
         const ctx = window.__KANNA_E2E__.setupState;
         ${SELECT_SIDEBAR_ITEM_SCRIPT}
         selectSidebarItem(ctx, "${id}");
         setTimeout(function() { cb("ok"); }, 100);
       } catch (e) {
         cb("err:" + (e && e.message ? e.message : String(e)));
       }`
    );
    if (typeof result === "string" && result.startsWith("err:")) {
      throw new Error(`selecting task ${id} failed: ${result.slice(4)}`);
    }
  }

  beforeAll(async () => {
    await client.createSession();
    await resetDatabase(client);

    fixtureRepoRoot = await createSeedFixtureRepo("task-switch-minimal");
    testRepoPath = fixtureRepoRoot;
    await importTestRepo(client, testRepoPath, "main-tabs-test");

    taskId = await createTask("Main tabs task");
    secondTaskId = await createTask("Main tabs other task");
    await selectTask(taskId);
  });

  afterAll(async () => {
    if (testRepoPath) {
      await cleanupWorktrees(client, testRepoPath);
    }
    await cleanupFixtureRepos(fixtureRepoRoot ? [fixtureRepoRoot] : []);
    await client.deleteSession();
  });

  it("opens the diff and the task shell as tabs beside the agent session", async () => {
    await client.waitForElement('[data-testid="main-tab-bar"]', 5_000);
    expect(await openTabIds(client)).toEqual(["agent"]);

    await pressShortcut(client, { key: "d", meta: true });
    await waitForActiveTab(client, "diff");
    await client.waitForElement(".diff-view", 8_000);
    // The agent session is still mounted behind it, not torn down.
    expect(await openTabIds(client)).toEqual(["agent", "diff"]);

    await pressShortcut(client, { key: "j", meta: true });
    await waitForActiveTab(client, "shell");
    expect(await openTabIds(client)).toEqual(["agent", "diff", "shell"]);

    // The shortcut that opened a view raises it again rather than toggling it
    // shut — closing is ⌘W's job.
    await pressShortcut(client, { key: "d", meta: true });
    await waitForActiveTab(client, "diff");
    await pressShortcut(client, { key: "d", meta: true });
    await waitForActiveTab(client, "diff");
    expect(await openTabIds(client)).toEqual(["agent", "diff", "shell"]);

    // ⌘W closes the tab in front and hands over to the one that takes its
    // place.
    await pressCloseTab(client);
    await waitForActiveTab(client, "shell");
    expect(await openTabIds(client)).toEqual(["agent", "shell"]);

    // Escape belongs to whatever runs in the shell, so it does not close it.
    await pressShortcut(client, { key: "Escape" });
    await sleep(400);
    expect(await openTabIds(client)).toEqual(["agent", "shell"]);

    await pressCloseTab(client);
    await waitForActiveTab(client, "agent");
    expect(await openTabIds(client)).toEqual(["agent"]);
  });

  it("closes a diff tab with Escape once no modal wants the key", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);

    await pressShortcut(client, { key: "d", meta: true });
    await waitForActiveTab(client, "diff");

    await pressShortcut(client, { key: "Escape" });
    await waitForActiveTab(client, "agent");
    expect(await openTabIds(client)).toEqual(["agent"]);
  });

  it("keeps each tab's view alive while another tab is in front", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);
    await waitForActiveTab(client, "agent");

    await pressShortcut(client, { key: "j", meta: true });
    await waitForActiveTab(client, "shell");
    const shellTerminals = await client.executeSync<number>(
      `return document.querySelectorAll(".shell-modal .xterm").length;`
    );
    expect(shellTerminals).toBeGreaterThan(0);

    await pressShortcut(client, { key: "d", meta: true });
    await waitForActiveTab(client, "diff");

    // Hidden, but still in the DOM: the xterm buffer is not rebuilt when the
    // shell comes back, which is why tabs use v-show rather than v-if.
    const hiddenShell = await client.executeSync<boolean>(
      `const shell = document.querySelector(".shell-modal");
       if (!shell) return false;
       const overlay = shell.closest(".embedded-view");
       return Boolean(overlay) && getComputedStyle(overlay).display === "none";`
    );
    expect(hiddenShell).toBe(true);

    await pressShortcut(client, { key: "j", meta: true });
    await waitForActiveTab(client, "shell");
  });

  it("gives every task its own tabs", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);
    await waitForActiveTab(client, "agent");
    await pressShortcut(client, { key: "d", meta: true });
    await waitForActiveTab(client, "diff");

    await selectTask(secondTaskId);
    await closeViewTabs(client);
    await waitForActiveTab(client, "agent");
    expect(await openTabIds(client)).toEqual(["agent"]);

    // Coming back restores what that task had open — the point of tabs over
    // the ephemeral modals they replaced.
    await selectTask(taskId);
    await waitForActiveTab(client, "diff");
    expect(await openTabIds(client)).toEqual(["agent", "diff"]);
  });

  it("closes the tab in front from the native File menu, and spares the window", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);
    await pressShortcut(client, { key: "d", meta: true });
    await waitForActiveTab(client, "diff");

    // ⌘W is the menu's accelerator in the packaged app, so this is the path
    // the key actually takes: the item emits, and the action closes the tab.
    // A synthesized keydown proves nothing here — the web handler skips it.
    const emitNativeClose = async () => {
      const result = await client.executeAsync<string>(
        `const cb = arguments[arguments.length - 1];
         import("/src/emit.ts")
           .then((module) => module.emit("kanna://native-close-window", {}))
           .then(() => cb("ok"))
           .catch((error) => cb("err:" + String(error && error.message ? error.message : error)));`
      );
      if (typeof result === "string" && result.startsWith("err:")) {
        throw new Error(`native close-window emit failed: ${result.slice(4)}`);
      }
      await sleep(500);
    };

    await emitNativeClose();
    await waitForActiveTab(client, "agent");
    expect(await openTabIds(client)).toEqual(["agent"]);
    expect(await client.getWindowHandles()).toHaveLength(1);

    // With a view open behind the agent session, ⌘W declines rather than
    // closing the window out from under it.
    await pressShortcut(client, { key: "d", meta: true });
    await waitForActiveTab(client, "diff");
    await client.executeSync(
      `document.querySelector('[data-testid="main-tab-agent"]')?.click(); return true;`,
    );
    await waitForActiveTab(client, "agent");

    await emitNativeClose();
    expect(await openTabIds(client)).toEqual(["agent", "diff"]);
    expect(await client.getWindowHandles()).toHaveLength(1);

    await closeViewTabs(client);
  });

  it("gives a repository with no task selected a tab set of its own", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);

    // Deselect the task: the main area belongs to the repository now.
    await client.executeAsync<string>(
      `const cb = arguments[arguments.length - 1];
       const ctx = window.__KANNA_E2E__.setupState;
       const store = ctx.store;
       if (store?.selectedItemId?.__v_isRef) store.selectedItemId.value = null;
       else store.selectedItemId = null;
       setTimeout(function() { cb("ok"); }, 200);`
    );

    // A repository has no agent session, so its tab set starts empty. The
    // deselect is reactive, so this waits for it to land rather than for a
    // fixed interval — on a loaded machine the old sleep expired first and
    // read the task's tabs.
    let repoTabs: string[] = ["agent"];
    const deselected = Date.now() + 10_000;
    while (Date.now() < deselected) {
      repoTabs = await openTabIds(client);
      if (repoTabs.length === 0) break;
      await sleep(100);
    }
    expect(repoTabs).toEqual([]);

    await pressShortcut(client, { key: "g", meta: true });
    await waitForActiveTab(client, "graph");
    await pressShortcut(client, { key: "A", meta: true, shift: true });
    await waitForActiveTab(client, "analytics");
    expect(await openTabIds(client)).toEqual(["graph", "analytics"]);

    // ⌘J has no worktree to run in here, so it opens the repo-root shell.
    await pressShortcut(client, { key: "j", meta: true });
    await waitForActiveTab(client, "shell:repo");

    await closeViewTabs(client);
    expect(await openTabIds(client)).toEqual([]);

    // Back on the task, its own tabs are untouched by any of that.
    await selectTask(taskId);
    await sleep(400);
    expect(await openTabIds(client)).toEqual(["agent"]);
  });

  it("opens the remaining views as tabs of the selected task", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);
    await waitForActiveTab(client, "agent");

    await pressShortcut(client, { key: "E", meta: true, shift: true });
    await waitForActiveTab(client, "tree");
    await pressShortcut(client, { key: ",", meta: true });
    await waitForActiveTab(client, "preferences");
    await pressShortcut(client, { key: "J", meta: true, shift: true });
    await waitForActiveTab(client, "shell:repo");

    // The worktree shell and the repo-root shell are separate tabs.
    await pressShortcut(client, { key: "j", meta: true });
    await waitForActiveTab(client, "shell");
    expect(await openTabIds(client)).toEqual([
      "agent",
      "tree",
      "preferences",
      "shell:repo",
      "shell",
    ]);

    await closeViewTabs(client);
    await waitForActiveTab(client, "agent");
  });

  it("opens a file an agent asked for through kanna_open_file", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);
    await waitForActiveTab(client, "agent");

    const server = await resolveAppKannaServer(client);
    const response = await localProcessFetch(`${server.baseUrl}/v1/desktop/views/open`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ taskId, path: "README.md" }),
    });
    expect(response.ok).toBe(true);
    // Requested, never shown: the response says only that a window was asked.
    expect(await response.json()).toMatchObject({ requested: true, path: "README.md" });

    await waitForActiveTab(client, "file:README.md");
    await client.waitForText(".preview-modal .file-path", "README.md", 8_000);

    const refused = await localProcessFetch(`${server.baseUrl}/v1/desktop/views/open`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ taskId, path: "../outside.txt" }),
    });
    // A path outside the task's workspace fails at the route, so a mistyped
    // path is an error the agent can act on rather than a silent no-op.
    expect(refused.ok).toBe(false);

    await pressShortcut(client, { key: "Escape" });
    await waitForActiveTab(client, "agent");
    expect(await openTabIds(client)).toEqual(["agent"]);
  });

  it("shows a launch's startup terminal as its own tab, beside the agent session", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);
    await waitForActiveTab(client, "agent");

    const server = await resolveAppKannaServer(client);
    const setupSessionId = `setup-${taskId}-1`;
    // A launch records its startup terminal when the daemon acknowledges it;
    // the desktop reads that record rather than being told about it, which is
    // what makes a tab appear after a missed event or a restart too.
    const recorded = await client.executeAsync<string>(
      `const cb = arguments[arguments.length - 1];
       const ctx = window.__KANNA_E2E__.setupState;
       const db = ctx.db.value || ctx.db;
       db.execute("INSERT INTO terminal_session (id, repo_id, pipeline_item_id, label, cwd, daemon_session_id, role, stage, attempt, state, title) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
         ["${setupSessionId}", "${await getVueState(client, "selectedRepoId")}", "${taskId}", "setup", "${testRepoPath}", "${setupSessionId}", "setup", "in progress", 1, "live", "Startup · in progress"])
         .then(function() { cb("ok"); })
         .catch(function(e) { cb("err:" + (e && e.message ? e.message : String(e))); });`
    );
    if (typeof recorded === "string" && recorded.startsWith("err:")) {
      throw new Error(`recording the startup terminal failed: ${recorded.slice(4)}`);
    }
    // The server is the source of truth for which terminals a task has, and a
    // task now has more than one — only the agent one answers to the task id.
    const terminals = await localProcessFetch(
      `${server.baseUrl}/v1/tasks/${taskId}/terminals`,
    );
    expect(terminals.ok).toBe(true);
    const listed = await terminals.json() as {
      agentSessionId: string | null;
      terminals: { role: string; daemonSessionId: string | null }[];
    };
    expect(listed.agentSessionId).toBe(taskId);
    expect(listed.terminals.some((terminal) =>
      terminal.role === "setup" && terminal.daemonSessionId === setupSessionId
    )).toBe(true);

    // Re-selecting is what re-reads the task's terminals.
    await selectTask(secondTaskId);
    await selectTask(taskId);

    const deadline = Date.now() + 10_000;
    let tabs: string[] = [];
    while (Date.now() < deadline) {
      tabs = await openTabIds(client);
      if (tabs.includes(`terminal:${setupSessionId}`)) break;
      await sleep(200);
    }
    expect(tabs).toEqual(["agent", `terminal:${setupSessionId}`]);
    // A startup terminal appearing must not pull the reader off the agent.
    expect(await activeTabId(client)).toBe("agent");

    const label = await client.executeSync<string | null>(
      `const tab = document.querySelector('[data-testid="main-tab-terminal:${setupSessionId}"] .main-tab-label');
       return tab ? tab.textContent.trim() : null;`
    );
    expect(label).toBe("Startup · in progress");

    // It is a view of a session the launch owns, so closing it hides the view
    // and leaves the record alone: reopening shows the same terminal.
    await client.executeSync(
      `const close = document.querySelector('[data-testid="main-tab-close-terminal:${setupSessionId}"]');
       if (!close) throw new Error("the startup terminal tab has no close button");
       close.click();
       return true;`
    );
    await sleep(300);
    expect(await openTabIds(client)).toEqual(["agent"]);

    const still = await localProcessFetch(`${server.baseUrl}/v1/tasks/${taskId}/terminals`);
    const stillListed = await still.json() as { terminals: { role: string }[] };
    expect(stillListed.terminals.some((terminal) => terminal.role === "setup")).toBe(true);

    await client.executeAsync<string>(
      `const cb = arguments[arguments.length - 1];
       const ctx = window.__KANNA_E2E__.setupState;
       const db = ctx.db.value || ctx.db;
       db.execute("DELETE FROM terminal_session WHERE id = ?", ["${setupSessionId}"])
         .then(function() { cb("ok"); })
         .catch(function(e) { cb("err:" + (e && e.message ? e.message : String(e))); });`
    );
    await sleep(1_200);
  });

  /**
   * A stage advance records its startup terminal before that setup runs, and
   * writes nothing to the task row until the transition lands — so the tab
   * cannot wait on the snapshot revision the reader's task otherwise changes
   * on. The edge it waits on instead is the terminal's own session being
   * created, which is what this drives: the record, then the daemon session,
   * with the reader sitting on the task the whole time.
   *
   * The advance itself is a server concern and is covered there; what has to
   * be proven here is that the desktop reacts to a terminal appearing without
   * the reader reselecting and without a poll.
   */
  it("shows a stage's startup terminal while its setup is still running", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);
    await waitForActiveTab(client, "agent");

    const stageSetupSessionId = `setup-${taskId}-2`;
    const repoId = await getVueState(client, "selectedRepoId") as string;
    const recorded = await client.executeAsync<string>(
      `const cb = arguments[arguments.length - 1];
       const ctx = window.__KANNA_E2E__.setupState;
       const db = ctx.db.value || ctx.db;
       db.execute("INSERT INTO terminal_session (id, repo_id, pipeline_item_id, label, cwd, daemon_session_id, role, stage, attempt, state, title) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
         ["${stageSetupSessionId}", "${repoId}", "${taskId}", "setup", "${testRepoPath}", "${stageSetupSessionId}", "setup", "review", 2, "live", "Startup · review"])
         .then(function() { cb("ok"); })
         .catch(function(e) { cb("err:" + (e && e.message ? e.message : String(e))); });`
    );
    if (typeof recorded === "string" && recorded.startsWith("err:")) {
      throw new Error(`recording the stage startup terminal failed: ${recorded.slice(4)}`);
    }

    // The daemon session the launch starts next. Nothing selects, reselects,
    // or touches the task row from here on.
    const spawned = await tauriInvoke(client, "spawn_session", {
      sessionId: stageSetupSessionId,
      cwd: testRepoPath,
      executable: "/bin/zsh",
      args: ["-c", "printf 'STAGE_SETUP_RUNNING\\n'; while true; do sleep 60; done"],
      env: {},
      cols: 80,
      rows: 24,
    });
    if (spawned && typeof spawned === "object" && "__error" in spawned) {
      throw new Error(`spawning the stage startup terminal failed: ${String((spawned as { __error: unknown }).__error)}`);
    }

    const deadline = Date.now() + 15_000;
    let tabs: string[] = [];
    while (Date.now() < deadline) {
      tabs = await openTabIds(client);
      if (tabs.includes(`terminal:${stageSetupSessionId}`)) break;
      await sleep(200);
    }
    expect(tabs).toContain(`terminal:${stageSetupSessionId}`);
    // A startup terminal appearing must not pull the reader off the agent.
    expect(await activeTabId(client)).toBe("agent");

    // And it shows the setup that is running: a live startup terminal is a
    // session in its own right, so the tab attaches to it directly rather
    // than resolving the task's agent session and finding nothing.
    await client.executeSync(
      `document.querySelector('[data-testid="main-tab-terminal:${stageSetupSessionId}"]').click();`
    );
    const liveDeadline = Date.now() + 20_000;
    let liveLines: string[] = [];
    while (Date.now() < liveDeadline) {
      liveLines = await client.executeSync<string[]>(
        `const buffers = window.__KANNA_E2E__.terminalBuffers;
         if (!buffers || !buffers.sessionIds().includes("${stageSetupSessionId}")) return [];
         return buffers.lines("${stageSetupSessionId}");`
      );
      if (liveLines.some((line) => line.includes("STAGE_SETUP_RUNNING"))) break;
      await sleep(200);
    }
    expect(liveLines.some((line) => line.includes("STAGE_SETUP_RUNNING"))).toBe(true);

    await tauriInvoke(client, "kill_session", { sessionId: stageSetupSessionId }).catch(() => null);
    await client.executeAsync<string>(
      `const cb = arguments[arguments.length - 1];
       const ctx = window.__KANNA_E2E__.setupState;
       const db = ctx.db.value || ctx.db;
       db.execute("DELETE FROM terminal_session WHERE id = ?", ["${stageSetupSessionId}"])
         .then(function() { cb("ok"); })
         .catch(function(e) { cb("err:" + (e && e.message ? e.message : String(e))); });`
    );
    await closeViewTabs(client);
    await sleep(1_200);
  });

  /**
   * `kanna_open_terminal` on a terminal that has already finished has to show
   * what it printed. The command carries what the server knows about that
   * terminal — including whether its final frame was kept — because a tab that
   * assumed the worst told the reader the output was gone while the archive
   * sat beside it.
   */
  it("renders the archived frame of a retired terminal opened through the tab surface", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);
    await waitForActiveTab(client, "agent");

    const server = await resolveAppKannaServer(client);
    const retiredSessionId = `setup-${taskId}-3`;
    const repoId = await getVueState(client, "selectedRepoId") as string;
    const archived = await client.executeAsync<string>(
      `const cb = arguments[arguments.length - 1];
       const ctx = window.__KANNA_E2E__.setupState;
       const db = ctx.db.value || ctx.db;
       db.execute("INSERT INTO terminal_session (id, repo_id, pipeline_item_id, label, cwd, daemon_session_id, role, stage, attempt, state, title, exit_code, retired_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))",
         ["${retiredSessionId}", "${repoId}", "${taskId}", "setup", "${testRepoPath}", "${retiredSessionId}", "setup", "in progress", 3, "retired", "Startup · in progress", 0])
         .then(function() {
           return db.execute("INSERT INTO terminal_session_archive (session_id, cols, rows, vt) VALUES (?, ?, ?, ?)",
             ["${retiredSessionId}", 80, 24, "ARCHIVED_FRAME_SENTINEL\\r\\n"]);
         })
         .then(function() { cb("ok"); })
         .catch(function(e) { cb("err:" + (e && e.message ? e.message : String(e))); });`
    );
    if (typeof archived === "string" && archived.startsWith("err:")) {
      throw new Error(`recording the retired terminal failed: ${archived.slice(4)}`);
    }

    const opened = await localProcessFetch(`${server.baseUrl}/v1/desktop/views/open-terminal`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ taskId, sessionId: retiredSessionId }),
    });
    expect(opened.ok).toBe(true);

    await waitForActiveTab(client, `terminal:${retiredSessionId}`);

    const deadline = Date.now() + 15_000;
    let lines: string[] = [];
    while (Date.now() < deadline) {
      lines = await client.executeSync<string[]>(
        `const buffers = window.__KANNA_E2E__.terminalBuffers;
         if (!buffers || !buffers.sessionIds().includes("${retiredSessionId}")) return [];
         return buffers.lines("${retiredSessionId}");`
      );
      if (lines.some((line) => line.includes("ARCHIVED_FRAME_SENTINEL"))) break;
      await sleep(200);
    }
    expect(lines.some((line) => line.includes("ARCHIVED_FRAME_SENTINEL"))).toBe(true);

    // And it never claims the output was not kept.
    const banner = await client.executeSync<string>(
      `const status = document.querySelector('[data-testid="task-terminal-finished"]');
       return status ? status.textContent.trim() : "";`
    );
    expect(banner).not.toContain("was not kept");

    await client.executeAsync<string>(
      `const cb = arguments[arguments.length - 1];
       const ctx = window.__KANNA_E2E__.setupState;
       const db = ctx.db.value || ctx.db;
       db.execute("DELETE FROM terminal_session_archive WHERE session_id = ?", ["${retiredSessionId}"])
         .then(function() {
           return db.execute("DELETE FROM terminal_session WHERE id = ?", ["${retiredSessionId}"]);
         })
         .then(function() { cb("ok"); })
         .catch(function(e) { cb("err:" + (e && e.message ? e.message : String(e))); });`
    );
    await closeViewTabs(client);
    await sleep(1_200);
  });

  it("brings a task's tabs back after the app restarts, and forgets a closed task's", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);

    await pressShortcut(client, { key: "d", meta: true });
    await waitForActiveTab(client, "diff");
    await callVueMethod(client, "openFilePreview", "README.md");
    await waitForActiveTab(client, "file:README.md");
    await pressShortcut(client, { key: "d", meta: true });
    await waitForActiveTab(client, "diff");

    // The tab state is written on a debounce, so a reload that beat the write
    // would prove nothing. Reload rather than open a second WebDriver session:
    // a new session attaches to the app that is still running, so the tabs
    // would never have left memory and this would pass with no storage at all.
    await sleep(1_200);

    await client.reload();
    await selectTask(taskId);

    // The agent session is rebuilt rather than stored, and the tab that was in
    // front is the tab that comes back in front.
    await waitForActiveTab(client, "diff");
    expect(await openTabIds(client)).toEqual(["agent", "diff", "file:README.md"]);
    await client.waitForText(".preview-modal .file-path", "README.md", 8_000);

    // The other task never opened a view, so it comes back as it was.
    await selectTask(secondTaskId);
    expect(await openTabIds(client)).toEqual(["agent"]);

    await selectTask(taskId);
    await closeViewTabs(client);
    await sleep(1_200);
  });

  it("gives the less keys to the tab in front, not to the ones behind it", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);

    await pressShortcut(client, { key: "d", meta: true });
    await waitForActiveTab(client, "diff");
    await pressShortcut(client, { key: "g", meta: true });
    await waitForActiveTab(client, "graph");

    // `q` is a window-level binding, and a tab behind another one is still
    // mounted: without a foreground gate the hidden diff answered this too and
    // closed itself along with the graph.
    await pressShortcut(client, { key: "q" });
    await sleep(400);

    expect(await openTabIds(client)).toEqual(["agent", "diff"]);
    await waitForActiveTab(client, "diff");

    // Same again with a file in front, which has always gated its own keys —
    // what has to survive here is the diff behind it.
    await callVueMethod(client, "openFilePreview", "README.md");
    await waitForActiveTab(client, "file:README.md");
    await pressShortcut(client, { key: "q" });
    await sleep(400);

    expect(await openTabIds(client)).toEqual(["agent", "diff"]);

    await closeViewTabs(client);
  });

  it("runs the IDE once, on the file, when a file tab is in front", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);

    // The same path createTask made the worktree at.
    const worktreePath = `${testRepoPath}/.kanna-worktrees/task-${taskId}`;
    const ideLog = `${worktreePath}/.ide-invocations`;
    const ideRecorder = `${worktreePath}/.record-ide.sh`;
    // The app runs `${ideCommand} "<path>"`, so a recorder standing in for the
    // editor captures the exact path each binding passed.
    await tauriInvoke(client, "run_script", {
      script: [
        `cat > "${ideRecorder}" <<'SH'`,
        "#!/bin/sh",
        `printf '%s\\n' "$1" >> "${ideLog}"`,
        "SH",
        `chmod +x "${ideRecorder}"`,
        `rm -f "${ideLog}"`,
      ].join("\n"),
      cwd: worktreePath,
      env: {},
    });
    await client.executeSync(
      `const ctx = window.__KANNA_E2E__.setupState;
       const ide = ctx.store.ideCommand;
       if (ide && ide.__v_isRef) ide.value = ${JSON.stringify(JSON.stringify(ideRecorder))};
       else ctx.store.ideCommand = ${JSON.stringify(JSON.stringify(ideRecorder))};
       return true;`
    );

    await callVueMethod(client, "openFilePreview", "README.md");
    await waitForActiveTab(client, "file:README.md");

    await pressShortcut(client, { key: "o", meta: true });

    // macOS resolves the temp fixture path through /private; compare the real
    // paths rather than the spelling each side happened to use.
    const realPath = (path: string) => path.replace(/^\/private/, "");
    // The recorder is a separate process, so this waits for it to have written
    // rather than for a fixed interval, then asserts what it wrote. A second,
    // wrong invocation would still be there to see.
    let lines: string[] = [];
    const recordedBy = Date.now() + 10_000;
    while (Date.now() < recordedBy) {
      const recorded = await tauriInvoke(client, "run_script", {
        script: `cat "${ideLog}" 2>/dev/null || true`,
        cwd: worktreePath,
        env: {},
      }) as string;
      lines = String(recorded)
        .split("\n")
        .map((line) => realPath(line.trim()))
        .filter(Boolean);
      if (lines.length > 0) break;
      await sleep(100);
    }
    // Give a second invocation, if the binding fired twice, time to land.
    await sleep(500);
    const settled = await tauriInvoke(client, "run_script", {
      script: `cat "${ideLog}" 2>/dev/null || true`,
      cwd: worktreePath,
      env: {},
    }) as string;
    lines = String(settled)
      .split("\n")
      .map((line) => realPath(line.trim()))
      .filter(Boolean);
    // Both the global shortcut and the file view bind ⌘O, and a matched global
    // only calls preventDefault, so the keydown reached both: the editor opened
    // twice, once on the worktree and once on the file.
    expect(lines).toEqual([realPath(`${worktreePath}/README.md`)]);

    await closeViewTabs(client);
  });

  it("gives ⌘F to a file tab's own find, leaving the sidebar search alone", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);

    // A plain-text file: the preview disables its own find while it is
    // rendering markdown, so README.md would not claim the key.
    await callVueMethod(client, "openFilePreview", "src/index.txt");
    await waitForActiveTab(client, "file:src/index.txt");

    await watchSidebarSearchFocus(client);
    await pressShortcut(client, { key: "f", meta: true });
    await sleep(400);

    // Focus alone is not the assertion: the sidebar focuses synchronously and
    // the view focuses on the next tick, so whoever ends up focused says
    // nothing about whether both fired.
    expect(await sidebarSearchWasFocused(client)).toBe(false);
    expect(await focusedSearchOwner(client)).toBe("view");

    await closeViewTabs(client);
  });

  it("gives ⌘F to a diff tab's own find, leaving the sidebar search alone", async () => {
    await selectTask(taskId);
    await closeViewTabs(client);

    await pressShortcut(client, { key: "d", meta: true });
    await waitForActiveTab(client, "diff");

    await watchSidebarSearchFocus(client);
    await pressShortcut(client, { key: "f", meta: true });
    await sleep(400);

    expect(await sidebarSearchWasFocused(client)).toBe(false);
    expect(await focusedSearchOwner(client)).toBe("view");

    await closeViewTabs(client);
  });
});
