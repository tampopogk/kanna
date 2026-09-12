import { mkdir } from "node:fs/promises";
import { join, resolve } from "node:path";
import { afterAll, beforeAll, expect, it } from "vitest";
import { localProcessFetch } from "@kanna/local-process-fetch";
import { WebDriverClient } from "../helpers/webdriver";
import { getWebDriverPort } from "../helpers/webdriverPort";
import { assertNativeWindowIdentity, resolveExpectedNativeWindowIdentity } from "../helpers/windowIdentity";
import { resetDatabase, importTestRepo, cleanupWorktrees } from "../helpers/reset";
import { createFixtureRepo, cleanupFixtureRepos } from "../helpers/fixture-repo";
import { resolveAppKannaServer } from "../helpers/kannaServer";
import { callVueMethod, tauriInvoke } from "../helpers/vue";
import { verifyViewerGestures } from "../helpers/viewerGestures";

const client = new WebDriverClient(getWebDriverPort());
const artifactDir = resolve(process.cwd(), "../../.tmp/viewer-gestures");
let repoPath = "";
let taskId: string | null = null;

async function verifyWindow() {
  const identity = await resolveExpectedNativeWindowIdentity(resolve(process.cwd(), "../.."));
  await assertNativeWindowIdentity(client, identity, "viewer gesture fixture");
}

beforeAll(async () => {
  await client.createSession();
  await verifyWindow();
  await resetDatabase(client);
  await mkdir(artifactDir, { recursive: true });
});

afterAll(async () => {
  if (taskId) await tauriInvoke(client, "kill_session", { sessionId: taskId });
  if (repoPath) {
    await cleanupWorktrees(client, repoPath);
    await cleanupFixtureRepos([repoPath]);
  }
  await client.deleteSession();
});

it("hands actual PTY geometry between mobile touch and desktop wheel while replay stays passive", async () => {
  repoPath = await createFixtureRepo("viewer-gestures");
  const repoId = await importTestRepo(client, repoPath, "viewer-gestures");
  const { baseUrl } = await resolveAppKannaServer(client);
  // Bounded fixture: reports the kernel winsize on SIGWINCH, never an agent
  // model and never terminal input as a resize workaround.
  const script = 'select(STDOUT); $| = 1; sub draw { my $size = `stty size`; my ($rows, $cols) = split(/\\s+/, $size); print "ACTIVE_VIEW:${cols}x${rows}\\n"; } $SIG{WINCH} = sub { draw(); }; draw(); while (1) { sleep 1; }';
  const response = await localProcessFetch(`${baseUrl}/v1/tasks`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ repoId, prompt: "Viewer geometry fixture", baseRef: "origin/main", agentProvider: "codex", agentType: "pty", terminalCols: 140, terminalRows: 50, setupCmds: [`/usr/bin/perl -e '${script}'`] }),
  });
  expect(response.ok).toBe(true);
  const created = await response.json() as { taskId: string };
  taskId = created.taskId;
  await verifyWindow();
  await callVueMethod(client, "loadItems", repoId);
  await callVueMethod(client, "store.selectItem", taskId);
  await client.waitForElement(".main-panel .terminal-container .xterm-helper-textarea", 30_000);
  const id = taskId;
  const readPtyOutput = () => client.executeSync<string>(`
    return (window.__KANNA_E2E__?.terminalBuffers?.lines(${JSON.stringify(`local:${id}`)}) ?? [])
      .filter(line => /^ACTIVE_VIEW:\\d+x\\d+$/.test(line.trim())).at(-1)?.trim() ?? "";
  `);
  await expect.poll(readPtyOutput, { timeout: 30_000 }).toContain("ACTIVE_VIEW:");
  const credential = await tauriInvoke(client, "local_control_credential") as string;
  await verifyViewerGestures(baseUrl, id, credential,
    () => tauriInvoke(client, "get_session_recovery_state", { sessionId: id }).then(value => {
      const state = value as { cols: number; rows: number };
      return { cols: state.cols, rows: state.rows };
    }), readPtyOutput, artifactDir, async (grid) => {
      await verifyWindow();
      // Non-activating WKWebView can defer rAF painting. Use the existing
      // synchronous paint hook, as the companion E2E does; this neither writes
      // terminal bytes nor changes geometry, focus, or scroll position.
      await client.executeSync(`window.__KANNA_E2E__?.terminalBuffers?.refresh(${JSON.stringify(`local:${id}`)});`);
      try {
        await expect.poll(() => client.executeSync<string>(`
          return window.__KANNA_E2E__?.terminalBuffers?.element(${JSON.stringify(`local:${id}`)})
            ?.querySelector(".xterm-rows")?.textContent ?? "";
        `), { timeout: 30_000 }).toContain(`ACTIVE_VIEW:${grid.cols}x${grid.rows}`);
      } finally {
        await client.screenshot(join(artifactDir, "native-viewer-final-grid.png"));
      }
    },
  );
});
