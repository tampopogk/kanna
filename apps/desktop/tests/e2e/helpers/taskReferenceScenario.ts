import assert from "node:assert/strict";
import { createServer } from "node:http";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { cleanupFixtureRepos, createFixtureRepo } from "./fixture-repo";
import { cleanupWorktrees, importTestRepo, resetDatabase } from "./reset";
import { callVueMethod, execDb, tauriInvoke } from "./vue";
import { WebDriverClient } from "./webdriver";
import { assertNativeWindowIdentity, resolveExpectedNativeWindowIdentity } from "./windowIdentity";
import { localProcessFetch } from "@kanna/local-process-fetch";
import { resolveAppKannaServer } from "./kannaServer";
import { buildGlobalKeydownScript } from "./keyboard";
import { dismissStartupShortcutsModal } from "./startupOverlays";

/** Real UI/server/PTY exercise; cat supplies harmless terminal bytes without provider quota. */
export async function taskReferenceScenario(client: WebDriverClient, repoRoot: string) {
  const verifyPainting = process.env.KANNA_E2E_NO_ACTIVATE === "0";
  const output = join(repoRoot, ".tmp", "task-reference");
  await mkdir(output, { recursive: true });
  const expected = await resolveExpectedNativeWindowIdentity(repoRoot);
  await client.createSession({ dismissStartupShortcuts: false });
  await assertNativeWindowIdentity(client, expected, "task reference");
  const scale = await client.executeSync<number>(`return window.devicePixelRatio || 1`);
  let repo = "";
  const ids = ["ref00001", "ref00002"];
  const ownedSessions = new Set(ids);
  let frameRequests = 0;
  const server = createServer((_request, response) => {
    ++frameRequests;
    response.setHeader("content-type", "text/html");
    response.end('<html><body style="font:20px system-ui;padding:30px"><h1>Task preview</h1><p>Owned by this test task.</p><input placeholder="Keep this draft"><div style="height:1800px"></div></body></html>');
  });
  async function waitFor(check: () => Promise<boolean>, label: string) {
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline) { if (await check()) return; await sleep(100); }
    throw new Error(`Timed out: ${label}`);
  }
  async function click(selector: string) { await client.click(await client.waitForElement(selector)); }
  async function select(id: string) {
    await click(`.workflow-item[data-task-id="${id}"]`);
    await waitFor(async () => await client.executeSync<string>(`return window.__KANNA_E2E__.setupState.mainTabs.scopeKey.value`) === `item:${id}`, "task selection");
  }
  async function active() { return client.executeSync<string>(`return window.__KANNA_E2E__.setupState.mainTabs.activeTabId.value`); }
  async function sessions() { return await tauriInvoke(client, "list_sessions") as Array<{ session_id: string; pid: number; cwd: string }>; }
  try {
    await resetDatabase(client);
    await client.reload({ dismissStartupShortcuts: false });
    await assertNativeWindowIdentity(client, expected, "task reference after reset");
    await dismissStartupShortcutsModal(client);
    repo = await createFixtureRepo("task-reference-real-test");
    const repoId = await importTestRepo(client, repo, "Reference UI test");
    for (const [index, id] of ids.entries()) {
      const branch = `task-${id}`;
      const worktree = `${repo}/.kanna-worktrees/${branch}`;
      await tauriInvoke(client, "git_worktree_add", { repoPath: repo, branch, path: worktree });
      await writeFile(join(worktree, "reading.txt"), Array.from({ length: 350 }, (_, line) => `Line ${line + 1}: reference reading context`).join("\n"));
      await execDb(client, "INSERT INTO pipeline_item (id, repo_id, prompt, display_name, stage, branch, agent_type, agent_provider) VALUES (?, ?, ?, ?, ?, ?, ?, ?)", [id, repoId, "Reference UI fixture", index === 0 ? "Inspect changes with the agent" : "Another task", "in progress", branch, "pty", "codex"]);
      await execDb(client, "INSERT INTO worktree (id, pipeline_item_id, path, branch) VALUES (?, ?, ?, ?)", [`wt-${id}`, id, worktree, branch]);
      await tauriInvoke(client, "spawn_session", { sessionId: id, cwd: worktree, executable: "/bin/cat", args: [], env: {}, cols: 100, rows: 30 });
    }
    await callVueMethod(client, "store.refreshAllItems");
    await select(ids[0]);
    await client.setWindowRect({ width: 1500 * scale, height: 900 * scale });
    await client.waitForElement('[data-testid="main-tab-panel-agent"] .xterm-helper-textarea');
    const agentPid = (await sessions()).find(session => session.session_id === ids[0])?.pid;
    assert.ok(agentPid);
    assert.equal(await active(), "agent");
    await client.sendKeys(await client.waitForElement('[data-testid="main-tab-panel-agent"] .xterm-helper-textarea'), "TASK_REFERENCE_AGENT\r");
    await waitFor(async () => client.executeSync<boolean>(`return (window.__KANNA_E2E__.terminalBuffers?.lines?.('${ids[0]}') ?? []).join(' ').includes('TASK_REFERENCE_AGENT')`), "primary terminal bytes");
    await callVueMethod(client, "mainTabs.openTab", { kind: "diff" });
    await waitFor(async () => client.executeSync<boolean>(`return (document.querySelector('.diff-container')?.scrollHeight ?? 0) > 1500`), "render long working diff");
    await client.executeSync(`const el = document.querySelector('.diff-container'); el.scrollTop = 500; el.dispatchEvent(new Event('scroll'));`);
    await select(ids[1]);
    await select(ids[0]);
    await waitFor(async () => client.executeSync<boolean>(`return document.querySelector('.diff-container')?.scrollTop === 500`), "restore diff position");
    await client.screenshot(join(output, "wide-diff.png"));
    await client.executeSync(`Array.from(document.querySelectorAll('.workspace-actions button')).find(el => el.textContent === 'Full width').click()`);
    await waitFor(async () => client.executeSync<boolean>(`return !document.querySelector('.work-area').classList.contains('split')`), "reference full width");
    await client.executeSync(`Array.from(document.querySelectorAll('.workspace-actions button')).find(el => el.textContent === 'Side by side').click()`);
    await callVueMethod(client, "openFilePreview", "reading.txt");
    await client.waitForElement(".preview-content");
    await waitFor(async () => client.executeSync<boolean>(`return document.querySelector('.work-area').classList.contains('split')`), "wide split");
    await client.executeSync(`const el = document.querySelector('.preview-content'); el.scrollTop = 600; el.dispatchEvent(new Event('scroll'));`);
    await waitFor(async () => client.executeSync<boolean>(`return window.__KANNA_E2E__.setupState.mainTabs.activeTab.value.reading?.top === 600`), "record file position");
    await select(ids[1]);
    assert.equal(await active(), "agent");
    await select(ids[0]);
    await waitFor(async () => client.executeSync<boolean>(`return document.querySelector('.preview-content')?.scrollTop === 600`), "restore reading position");
    await client.screenshot(join(output, "wide-file.png"));
    await client.executeSync(`Array.from(document.querySelectorAll('.workspace-actions button')).find(el => el.textContent === 'Return to agent').click()`);
    await waitFor(async () => client.executeSync<boolean>(`return !!document.activeElement?.closest('[data-testid="main-tab-panel-agent"]')`), "return focus to agent");
    assert.equal(await active(), "agent");
    await client.sendKeys(await client.waitForElement('[data-testid="main-tab-panel-agent"] .xterm-helper-textarea'), "AGENT_FOCUS_CHECK");
    await click('[data-testid="main-tab-file:reading.txt"]');
    await client.setWindowRect({ width: 900 * scale, height: 800 * scale });
    await waitFor(async () => client.executeSync<boolean>(`return !document.querySelector('.work-area').classList.contains('split')`), "narrow single view");
    assert.equal(await client.executeSync<number>(`return document.querySelector('[data-testid="main-tab-panel-agent"]').getBoundingClientRect().width`), 0);
    await client.screenshot(join(output, "narrow-file.png"));
    await callVueMethod(client, "mainTabPersistence.flush");
    await client.reload({ dismissStartupShortcuts: false });
    await assertNativeWindowIdentity(client, expected, "task reference after restart");
    await dismissStartupShortcutsModal(client);
    await select(ids[0]);
    await waitFor(async () => client.executeSync<boolean>(`return document.querySelector('.preview-content')?.scrollTop === 600`), "restore file position after webview restart");
    assert.equal((await sessions()).find(session => session.session_id === ids[0])?.pid, agentPid);
    await click('[data-testid="main-tab-diff"]');
    await waitFor(async () => client.executeSync<boolean>(`return document.querySelector('.diff-container')?.scrollTop === 500`), "restore diff after webview restart");
    await click('[data-testid="main-tab-file:reading.txt"]');
    await client.setWindowRect({ width: 1500 * scale, height: 900 * scale });
    await callVueMethod(client, "store.savePreference", "terminalEditorCommand", "/usr/bin/vim -u NONE -n -i NONE");
    await click('[data-testid="edit-in-terminal"]');
    await click('[data-testid="start-terminal-editor"]:not(:disabled)');
    await waitFor(async () => (await active()).startsWith("editor:"), "editor opens beside agent");
    const editorId = (await active()).slice("editor:".length);
    ownedSessions.add(editorId);
    await waitFor(async () => client.executeSync<boolean>(`return (window.__KANNA_E2E__.terminalBuffers?.lines?.('${editorId}') ?? []).join(' ').includes('reading.txt')`), "editor terminal bytes");
    const editorPid = (await sessions()).find(session => session.session_id === editorId)?.pid;
    await select(ids[1]);
    await select(ids[0]);
    assert.equal(await active(), `editor:${editorId}`);
    assert.equal((await sessions()).find(session => session.session_id === editorId)?.pid, editorPid);
    if (verifyPainting) {
      const activation = await tauriInvoke(client, "e2e_activate_current_app");
      assert.ok(!activation || typeof activation !== "object" || !("__error" in activation), JSON.stringify(activation));
      await tauriInvoke(client, "plugin:window|set_focus");
    }

    if (verifyPainting) await waitFor(async () => client.executeSync<boolean>(`return (document.querySelector('.editor-view .xterm-rows')?.textContent ?? '').includes('reading.txt')`), "painted editor rows after task return");
    const editorInput = await client.waitForElement('.editor-view .xterm-helper-textarea');
    await client.sendKeys(editorInput, "\rgg0iSPLIT_EDITOR_SAVE\u001b:w\r");
    await waitFor(async () => (await readFile(`${repo}/.kanna-worktrees/task-${ids[0]}/reading.txt`, "utf8")).includes("SPLIT_EDITOR_SAVE"), "native editor save in original workspace");
    await client.executeSync(buildGlobalKeydownScript({ key: "s", meta: true }));
    await sleep(300);
    assert.equal(await client.executeSync<boolean>(`return !!window.__KANNA_E2E__.setupState.store.currentItem.has_running_post`), false, "editor Cmd+S must not dispatch a stage post");
    assert.equal(await active(), `editor:${editorId}`);
    assert.equal(await client.executeSync<string>(`return window.__KANNA_E2E__.setupState.store.currentItem.stage`), "in progress");
    await client.screenshot(join(output, "wide-editor.png"));
    await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve));
    const address = server.address();
    assert.ok(address && typeof address !== "string");
    await new Promise<void>(resolve => server.close(() => resolve()));
    const owner = await resolveAppKannaServer(client);
    const claim = await localProcessFetch(`${owner.baseUrl}/v1/tasks/${ids[0]}/ports`, {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ ports: { PREVIEW_PORT: address.port } }),
    });
    assert.equal(claim.status, 200);
    const claimed = await claim.json() as { portEnv?: Record<string, string>; port_env?: Record<string, string> };
    const port = Number((claimed.portEnv ?? claimed.port_env)?.PREVIEW_PORT);
    assert.ok(port > 0);
    await new Promise<void>(resolve => server.listen(port, "127.0.0.1", resolve));
    await callVueMethod(client, "store.refreshAllItems");
    await click('.task-header .port');
    await waitFor(async () => frameRequests > 0, "iframe reaches claimed task port");
    await client.executeSync(`Array.from(document.querySelectorAll('.workspace-actions button')).find(el => el.textContent === 'Return to agent').click()`);
    await waitFor(async () => client.executeSync<boolean>(`return !!document.activeElement?.closest('[data-testid="main-tab-panel-agent"]')`), "focus agent beside preview");
    // The plugin's pointer actions dispatch synthetic MouseEvents and do not
    // focus cross-origin frames. Exercise actual WebKit focus explicitly.
    await client.executeSync(`document.querySelector('.task-preview iframe').focus()`);
    await waitFor(async () => (await active()) === "preview:PREVIEW_PORT", "iframe focus owns preview context");
    await client.screenshot(join(output, "wide-preview.png"));
    const requestsBeforeSwitch = frameRequests;
    await select(ids[1]);
    await select(ids[0]);
    await sleep(500);
    assert.equal(frameRequests, requestsBeforeSwitch, "warm preview must not reload on task switch");
    assert.equal(await active(), "preview:PREVIEW_PORT");
    await writeFile(join(output, "evidence.json"), JSON.stringify({ title: await client.getNativeWindowTitle(), build: await client.getAppBuildInfo(), endpoint: client.getBaseUrl(), agentPid, editorPid, frameRequests, checks: ["file and diff switch/restart scroll", "split, full width and narrow layout", "native editor save and Cmd+S ownership", "agent and iframe focus", "surviving agent/editor PID", "task-owned preview route", "warm iframe continuity"] }, null, 2));
  } catch (error) {
    await client.screenshot(join(output, "failure.png"));
    await writeFile(join(output, "failure.txt"), String(error) + "\n" + await client.executeSync<string>(`return document.body.innerText + '\\n' + JSON.stringify({ activeElement: document.activeElement?.outerHTML.slice(0, 500), worktree: window.__KANNA_E2E__.setupState.appModals.activeWorktreePath.value, terminalNodes: Array.from(document.querySelectorAll('.terminal-container')).map(el => ({width: el.clientWidth, height: el.clientHeight, text: el.innerText})) })`));
    throw error;
  } finally {
    server.closeAllConnections();
    await new Promise<void>(resolve => server.close(() => resolve()));
    for (const sessionId of ownedSessions) await tauriInvoke(client, "kill_session", { sessionId });
    if (repo) {
      await cleanupWorktrees(client, repo);
      await cleanupFixtureRepos([repo]);
    }
    await client.deleteSession();
  }
}
