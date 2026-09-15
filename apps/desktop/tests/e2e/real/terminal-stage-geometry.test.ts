import { chmod, mkdir, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { afterAll, beforeAll, expect, it } from "vitest";
import { localProcessFetch } from "@kanna/local-process-fetch";
import { WebDriverClient } from "../helpers/webdriver";
import { getWebDriverPort } from "../helpers/webdriverPort";
import { assertNativeWindowIdentity, resolveExpectedNativeWindowIdentity } from "../helpers/windowIdentity";
import { resetDatabase, importTestRepo, cleanupWorktrees } from "../helpers/reset";
import { createFixtureRepo, cleanupFixtureRepos, publishFixtureChanges } from "../helpers/fixture-repo";
import { resolveAppKannaServer } from "../helpers/kannaServer";
import { callVueMethod, tauriInvoke } from "../helpers/vue";

const client = new WebDriverClient(getWebDriverPort());
const artifactDir = resolve(process.cwd(), "../../.tmp/stage-geometry");
let repoPath = "";
let taskId = "";
let baseUrl = "";
const evidence: Record<string, unknown> = {};

async function verifyWindow() {
  const identity = await resolveExpectedNativeWindowIdentity(resolve(process.cwd(), "../.."));
  await assertNativeWindowIdentity(client, identity, "stage geometry fixture");
  evidence.identity = identity;
}

async function appMethod(method: string, ...args: unknown[]) {
  const result = await callVueMethod(client, method, ...args);
  if (result !== null && typeof result === "object") expect(result).not.toHaveProperty("__error");
  return result;
}

async function windowCommand(command: string) {
  await verifyWindow();
  return client.executeAsync(`
    const done = arguments[arguments.length - 1];
    const internals = window.__TAURI_INTERNALS__;
    const label = internals.metadata.currentWindow.label;
    internals.invoke(${JSON.stringify(`plugin:window|${command}`)}, { label }).then(() => done(null), error => done({failure: String(error)}));
  `).then(result => expect(result).toBeNull());
}

async function foreground() {
  await verifyWindow();
  await windowCommand("show");
  await tauriInvoke(client, "e2e_activate_current_app");
  await windowCommand("set_focus");
  await expect.poll(() => client.executeSync("return document.hasFocus();")).toBe(true);
}

async function dimensions() {
  const state = await tauriInvoke(client, "get_session_recovery_state", { sessionId: taskId }) as {cols: number; rows: number};
  return { cols: state.cols, rows: state.rows };
}

async function measuredPane() {
  return client.executeSync<{cols: number; rows: number}>(`
    const viewport = window.__KANNA_E2E__.terminalBuffers.viewport(${JSON.stringify(`local:${taskId}`)});
    return {cols: viewport.availableCols, rows: viewport.availableRows};
  `);
}

beforeAll(async () => {
  await client.createSession();
  await verifyWindow();
  await resetDatabase(client);
  await mkdir(artifactDir, { recursive: true });
  baseUrl = (await resolveAppKannaServer(client)).baseUrl;
});

afterAll(async () => {
  evidence.taskId = taskId;
  evidence.activeViewTrace = await client.executeSync("return window.__KANNA_E2E__?.activeViewTrace;").catch(() => null);
  await writeFile(join(artifactDir, "evidence.json"), JSON.stringify(evidence, null, 2));
  if (taskId) await tauriInvoke(client, "kill_session", { sessionId: taskId });
  if (repoPath) {
    await cleanupWorktrees(client, repoPath);
    await cleanupFixtureRepos([repoPath]);
  }
  await client.deleteSession();
});

it("sizes a new visible task without input and preserves the applied PTY grid through stage replacement and viewer rebind", async () => {
  repoPath = await createFixtureRepo("stage-geometry");
  await mkdir(join(repoPath, ".kanna/workflows"), { recursive: true });
  // A repository-local Codex executable reports kernel winsize instead of
  // invoking a model. Unlike setup (run before spawn on stage forks), this
  // executes inside each real PTY. No terminal input is needed.
  await writeFile(join(repoPath, "geometry.pl"), `select(STDOUT); $| = 1;
sub draw { my $size = \`stty size\`; open(my $out, ">>", "geometry-sizes.txt") or die $!; print $out $size; close($out); print "STAGE_GEOMETRY:$size"; }
$SIG{WINCH} = sub { draw(); }; draw(); while (1) { sleep 1; }
`);
  await mkdir(join(repoPath, "bin"));
  await writeFile(join(repoPath, "bin/codex"), '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "codex-cli 0.0.0-fixture"; exit 0; fi\nexec /usr/bin/perl geometry.pl\n');
  await chmod(join(repoPath, "bin/codex"), 0o755);
  await writeFile(join(repoPath, ".kanna/config.json"), JSON.stringify({ workspace: { path: { prepend: ["./bin"] } }, agentProviders: { "*": "codex" } }));
  await writeFile(join(repoPath, ".kanna/workflows/geometry.json"), JSON.stringify({ name: "geometry", stages: [
    { name: "in progress", policy: { transition: "manual" } },
    { name: "review", policy: { transition: "manual" } },
    { name: "verified", policy: { transition: "manual" } },
  ] }));
  await publishFixtureChanges(repoPath, "geometry fixture");
  const repoId = await importTestRepo(client, repoPath, "stage-geometry");
  await foreground();
  await client.executeSync(`
    window.__stageGeometryInputCount = 0;
    const send = WebSocket.prototype.send;
    WebSocket.prototype.send = function(data) {
      if (typeof data === "string") {
        try {
          const frame = JSON.parse(data);
          if (["term_input", "term_input_boundary"].includes(frame.type)) window.__stageGeometryInputCount++;
        } catch {}
      }
      return send.call(this, data);
    };
  `);
  const response = await localProcessFetch(`${baseUrl}/v1/tasks`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ repoId, prompt: "Geometry fixture", baseRef: "origin/main", agentProvider: "codex", agentType: "pty", workflowName: "geometry" }),
  });
  expect(response.ok, await response.clone().text()).toBe(true);
  taskId = (await response.json() as { taskId: string }).taskId;
  const initialPath = join(repoPath, ".kanna-worktrees", `task-${taskId}`);
  await appMethod("store.reloadSnapshot");
  await expect.poll(() => client.executeSync<string[]>("return window.__KANNA_E2E__.setupState.store.items.map(item => item.id);"), { timeout: 30_000 }).toContain(taskId);
  await appMethod("store.selectItem", taskId);
  evidence.selectedUi = await client.executeSync(`
    const ctx = window.__KANNA_E2E__.setupState;
    return { selectedItemId: ctx.store.selectedItemId, panels: document.querySelectorAll('.main-panel').length,
      taskIds: ctx.store.items.map(item => item.id) };
  `);
  await appMethod("mainTabs.activateTab", "agent");
  await client.waitForElement(".main-panel .terminal-container .xterm-helper-textarea", 30_000);
  await expect.poll(async () => (await measuredPane()).cols, { timeout: 30_000 }).toBeGreaterThan(80);
  const pane = await measuredPane();
  await expect.poll(dimensions, { timeout: 30_000 }).toEqual(pane);
  evidence.newTaskPane = pane;
  await expect.poll(() => readFile(join(initialPath, "geometry-sizes.txt"), "utf8").catch(() => ""), { timeout: 30_000 }).toContain(`${pane.rows} ${pane.cols}`);

  // Advance from the diff tab while the terminal pane is hidden. Read the
  // replacement's first kernel report: a later foreground correction must
  // not conceal a default-size spawn.
  await verifyWindow();
  await appMethod("mainTabs.openTab", { kind: "diff" });
  await expect.poll(() => client.executeSync("return document.querySelector('.main-panel .terminal-container')?.offsetWidth;")).toBe(0);
  const advance = await localProcessFetch(`${baseUrl}/v1/tasks/${taskId}/actions/advance-stage`, {
    method: "POST", headers: { "content-type": "application/json" }, body: "{}",
  });
  expect(advance.ok, await advance.clone().text()).toBe(true);
  const nextPath = join(repoPath, ".kanna-worktrees", `task-${taskId}-2`);
  const reports = () => readFile(join(nextPath, "geometry-sizes.txt"), "utf8").catch(() => "");
  await expect.poll(reports, { timeout: 30_000 }).not.toBe("");
  evidence.replacementReports = await reports();
  evidence.replacementDimensions = await dimensions();
  expect((await reports()).trim().split("\n")[0]?.trim()).toBe(`${pane.rows} ${pane.cols}`);
  expect(await dimensions()).toEqual(pane);

  await verifyWindow();
  await appMethod("mainTabs.activateTab", "agent");
  await expect.poll(() => client.executeSync("return document.querySelector('.main-panel .terminal-container')?.offsetWidth;")).toBeGreaterThan(0);
  await expect.poll(dimensions, { timeout: 30_000 }).toEqual(await measuredPane());
  evidence.reboundDimensions = await dimensions();
  // Now replace it while continuously foregrounded. The persistent terminal
  // host must rebind without needing a new key, wheel, or pointer gesture.
  const activeClaims = () => client.executeSync<number>(`
    return (window.__KANNA_E2E__.activeViewTrace ?? []).filter(entry => entry.sessionId === ${JSON.stringify(taskId)} && entry.phase === "sent").length;
  `);
  const claimsBefore = await activeClaims();
  const visibleAdvance = await localProcessFetch(`${baseUrl}/v1/tasks/${taskId}/actions/advance-stage`, {
    method: "POST", headers: { "content-type": "application/json" }, body: "{}",
  });
  expect(visibleAdvance.ok, await visibleAdvance.clone().text()).toBe(true);
  const visibleReports = () => readFile(join(repoPath, ".kanna-worktrees", `task-${taskId}-3`, "geometry-sizes.txt"), "utf8").catch(() => "");
  await expect.poll(visibleReports, { timeout: 30_000 }).not.toBe("");
  await expect.poll(dimensions, { timeout: 30_000 }).toEqual(pane);
  await expect.poll(activeClaims, { timeout: 30_000 }).toBeGreaterThan(claimsBefore);
  // One bounded quiet observation catches a resize trailing the successful
  // rebind; this is a test oracle, never an application retry or delay.
  await new Promise(resolve => setTimeout(resolve, 1_000));
  evidence.visibleReplacementReports = await visibleReports();
  expect([...new Set((await visibleReports()).trim().split("\n").map(line => line.trim()))]).toEqual([`${pane.rows} ${pane.cols}`]);
  evidence.inputCount = await client.executeSync("return window.__stageGeometryInputCount;");
  expect(evidence.inputCount).toBe(0);
  evidence.activeViewTrace = await client.executeSync("return window.__KANNA_E2E__.activeViewTrace;");
});
