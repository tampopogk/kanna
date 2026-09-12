import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { beforeAll, afterAll, afterEach, describe, expect, it } from "vitest";
import { cleanupFixtureRepos, createFixtureRepo } from "../helpers/fixture-repo";
import { cleanupWorktrees, importTestRepo, resetDatabase } from "../helpers/reset";
import { callVueMethod, execDb, tauriInvoke } from "../helpers/vue";
import { WebDriverClient } from "../helpers/webdriver";
import { assertNativeWindowIdentity, resolveExpectedNativeWindowIdentity } from "../helpers/windowIdentity";
import { dismissStartupShortcutsModal } from "../helpers/startupOverlays";

describe("local terminal editor", () => {
  const client = new WebDriverClient();
  const taskId = "ed170001";
  let repo = "";
  let worktree = "";
  let editorSession = "";
  async function activeTab() {
    return client.executeSync<{ id: string; kind: string; editorSession?: { sessionId: string } }>(`return JSON.parse(JSON.stringify(window.__KANNA_E2E__.setupState.mainTabs.activeTab.value));`);
  }
  async function click(selector: string) { await client.click(await client.waitForElement(selector)); }
  async function sessions() { return await tauriInvoke(client, "list_sessions") as Array<{ session_id: string; pid: number; cwd: string }>; }
  beforeAll(async () => {
    await mkdir(resolve("../../.tmp"), { recursive: true });
    await client.createSession({ dismissStartupShortcuts: false });
    await assertNativeWindowIdentity(client, await resolveExpectedNativeWindowIdentity(resolve("../..")), "terminal editor");
    await writeFile(resolve("../../.tmp/editor-identity.json"), JSON.stringify({ title: await client.getNativeWindowTitle(), build: await client.getAppBuildInfo(), endpoint: client.getBaseUrl() }, null, 2));
    await resetDatabase(client);
    await client.reload({ dismissStartupShortcuts: false });
    await assertNativeWindowIdentity(client, await resolveExpectedNativeWindowIdentity(resolve("../..")), "terminal editor after reload");
    await dismissStartupShortcutsModal(client);
    repo = await createFixtureRepo("terminal-editor-real-test");
    const repoId = await importTestRepo(client, repo, "terminal-editor-real-test");
    worktree = `${repo}/.kanna-worktrees/task-${taskId}`;
    await tauriInvoke(client, "git_worktree_add", { repoPath: repo, branch: `task-${taskId}`, path: worktree });
    await execDb(client, "INSERT INTO pipeline_item (id, repo_id, prompt, stage, branch, agent_type, agent_provider) VALUES (?, ?, ?, ?, ?, ?, ?)", [taskId, repoId, "Terminal editor fixture", "in progress", `task-${taskId}`, "pty", "codex"]);
    await execDb(client, "INSERT INTO worktree (id, pipeline_item_id, path, branch) VALUES (?, ?, ?, ?)", [`wt-${taskId}`, taskId, worktree, `task-${taskId}`]);
    // A harmless PTY stands in for the primary agent; no live provider quota.
    await tauriInvoke(client, "spawn_session", { sessionId: taskId, cwd: worktree, executable: "/bin/cat", args: [], env: {}, cols: 80, rows: 24 });
    await callVueMethod(client, "store.savePreference", "terminalEditorCommand", "/usr/bin/vim -u NONE -n -i NONE");
    await click(`.workflow-item[data-task-id="${taskId}"]`);
    await client.waitForElement('[data-testid="main-tab-panel-agent"]');
  });
  afterEach(async ({ task }) => {
    if (task.result?.state === "fail") {
      await client.screenshot(resolve("../../.tmp/editor-failure.png"));
      await writeFile(resolve("../../.tmp/editor-failure.json"), JSON.stringify({ page: await client.executeSync(`return { text: document.body.innerText };`), sessions: await sessions(), editorSnapshot: editorSession ? await tauriInvoke(client, "get_session_recovery_state", { sessionId: editorSession }) : null }, null, 2));
    }
  });
  afterAll(async () => {
    const editors = (await sessions().catch(error => { console.warn("[terminal-editor cleanup]", error); return []; })).filter(s => s.session_id.startsWith(`shell-editor-${taskId.length}-${taskId}-`)).map(s => s.session_id);
    for (const id of [...editors, editorSession, taskId, `shell-wt-${taskId}`].filter(Boolean)) {
      await tauriInvoke(client, "kill_session", { sessionId: id }).catch(error => console.warn("[terminal-editor cleanup]", error));
    }
    if (repo) {
      await cleanupWorktrees(client, repo).catch(error => console.warn("[terminal-editor cleanup]", error));
      await cleanupFixtureRepos([repo]).catch(error => console.warn("[terminal-editor cleanup]", error));
    }
    await client.deleteSession();
  });
  it("edits and saves in the real worktree, survives hiding, and leaves the agent PTY alone", async () => {
    const agent = (await sessions()).find(s => s.session_id === taskId);
    if (!agent) throw new Error("Fixture agent session missing");
    const agentPid = agent.pid;
    await callVueMethod(client, "openFilePreview", "README.md");
    await click('[data-testid="edit-in-terminal"]');
    await client.waitForElement('[data-testid="start-terminal-editor"]:not(:disabled)');
    await client.screenshot(resolve("../../.tmp/editor-picker.png"));
    await click('[data-testid="start-terminal-editor"]:not(:disabled)');
    await expect.poll(async () => (await activeTab()).kind, { timeout: 30_000 }).toBe("editor");
    const descriptor = (await activeTab()).editorSession;
    if (!descriptor) throw new Error("Editor tab descriptor missing");
    editorSession = descriptor.sessionId;
    expect(editorSession).toMatch(/^shell-editor-/);
    const editor = (await sessions()).find(s => s.session_id === editorSession);
    if (!editor) throw new Error("Launched editor session missing");
    expect(editor.cwd).toBe(worktree);
    await writeFile(resolve("../../.tmp/editor-live.json"), JSON.stringify({ editor, session: await activeTab(), snapshot: await tauriInvoke(client, "get_session_recovery_state", { sessionId: editorSession }) }, null, 2));
    await client.waitForElement(".editor-view .terminal-container");
    await expect.poll(async () => await client.executeSync<string>(`return (window.__KANNA_E2E__?.terminalBuffers?.lines?.(${JSON.stringify(editorSession)}) ?? []).join('\\n');`), { timeout: 30_000 }).toContain("README.md");
    const textarea = await client.waitForElement(".editor-view .xterm-helper-textarea");
    await client.click(textarea);
    // Element Send Keys exercises xterm's input path. Native key actions in
    // this WebDriver build duplicate punctuation and mis-map some Vim keys.
    await client.sendKeys(textarea, "\u001bGoEDITOR_SAVED_FROM_KANNA\u001b:w\r");
    await expect.poll(async () => readFile(`${worktree}/README.md`, "utf8"), { timeout: 30_000 }).toContain("EDITOR_SAVED_FROM_KANNA");
    await client.pressShortcut(["Meta", "s"]);
    expect(await client.executeSync(`return window.__KANNA_E2E__.setupState.store.currentItem.stage;`)).toBe("in progress");
    await expect.poll(async () => client.executeSync<string>(`return document.querySelector(".editor-view .xterm-rows")?.textContent ?? "";`), { timeout: 10_000 }).toContain("EDITOR_SAVED_FROM_KANNA");
    await client.screenshot(resolve("../../.tmp/editor-view.png"));
    const id = (await activeTab()).id;
    await callVueMethod(client, "mainTabs.activateTab", "agent");
    await callVueMethod(client, "mainTabs.activateTab", id);
    expect((await sessions()).find(s => s.session_id === editorSession)?.pid).toBe(editor.pid);
    await callVueMethod(client, "mainTabs.closeTab", id);
    expect((await sessions()).find(s => s.session_id === editorSession)?.pid).toBe(editor.pid);
    await callVueMethod(client, "openFilePreview", "README.md");
    await click('[data-testid="edit-in-terminal"]');
    await click('[data-testid="start-terminal-editor"]:not(:disabled)');
    await expect.poll(async () => (await activeTab()).kind, { timeout: 30_000 }).toBe("editor");
    expect((await sessions()).find(s => s.session_id === editorSession)?.pid).toBe(editor.pid);
    expect((await sessions()).find(s => s.session_id === taskId)?.pid).toBe(agentPid);
    await client.sendKeys(await client.waitForElement(".editor-view .xterm-helper-textarea"), "\u001b:q\r");
  });
});
