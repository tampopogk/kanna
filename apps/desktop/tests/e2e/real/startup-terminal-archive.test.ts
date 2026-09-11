import { setTimeout as sleep } from "node:timers/promises";
import { spawn } from "node:child_process";
import { writeFile } from "node:fs/promises";
import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { WebDriverClient } from "../helpers/webdriver";
import { resetDatabase, importTestRepo, cleanupWorktrees } from "../helpers/reset";
import { cleanupFixtureRepos, createFixtureRepo } from "../helpers/fixture-repo";
import { resolveAppKannaServer } from "../helpers/kannaServer";
import { tauriInvoke } from "../helpers/vue";
import { localProcessFetch } from "@kanna/local-process-fetch";

/**
 * A retired startup terminal has to stay readable.
 *
 * A launch runs the repository's setup in a startup terminal of its own and
 * starts the agent only once that shell exits — so by the time anybody looks,
 * the shell is gone and the daemon has dropped the session. What the tab shows
 * is the archive: the final frame the daemon captured before dropping it, kept
 * with the task's durable record. This drives the real app end to end, because
 * the failure this covers only exists in the wiring: an attach-only view over
 * a dead session renders an error loop where a failed stage advance's
 * diagnostics should be.
 */

const SENTINEL = "STARTUP_ARCHIVE_SENTINEL";
const PROVIDER_BIN_DIR = ".kanna/test-provider-bin";

interface TaskTerminal {
  role: string;
  state: string;
  archived: boolean;
  daemonSessionId: string | null;
}

const SELECT_SIDEBAR_ITEM_SCRIPT = `
  function selectSidebarItem(ctx, id) {
    const select = ctx.selectSidebarItemById || ctx.handleSelectItem
      || (ctx.store && ctx.store.selectItem && ctx.store.selectItem.bind(ctx.store));
    if (!select) throw new Error("no sidebar selection entry point on setupState");
    return select(id);
  }
`;

describe("startup terminal archive", () => {
  const client = new WebDriverClient();
  let testRepoPath = "";
  let repoId = "";
  let taskId = "";

  beforeAll(async () => {
    await client.createSession();
    await resetDatabase(client);
    await client.reload();
    testRepoPath = await createFixtureRepo("startup-terminal-archive");
    await writeFile(
      `${testRepoPath}/.kanna/config.json`,
      `${JSON.stringify(
        {
          // The startup terminal prints the sentinel and installs a fake agent
          // CLI that parks, so the launch completes and the startup shell
          // exits — which is exactly the moment its output stops being live.
          setup: [
            `printf '%s\\n' '${SENTINEL}'`,
            `mkdir -p ${PROVIDER_BIN_DIR}`,
            `printf '%s\\n' '#!/bin/sh' 'while true; do sleep 60; done' > ${PROVIDER_BIN_DIR}/claude`,
            `chmod +x ${PROVIDER_BIN_DIR}/claude`,
          ],
          workspace: { path: { prepend: [PROVIDER_BIN_DIR] } },
        },
        null,
        2,
      )}\n`,
    );
    // The repository config a launch reads is the committed one.
    await runCommand(["git", "add", ".kanna/config.json"], testRepoPath);
    await runCommand(["git", "commit", "-m", "configure the startup archive fixture"], testRepoPath);
    await runCommand(["git", "push", "origin", "main"], testRepoPath);
    repoId = await importTestRepo(client, testRepoPath, "startup-terminal-archive");
  });

  afterAll(async () => {
    if (taskId) {
      await tauriInvoke(client, "kill_session", { sessionId: taskId }).catch(() => null);
    }
    if (testRepoPath) {
      await cleanupWorktrees(client, testRepoPath);
      await cleanupFixtureRepos([testRepoPath]);
    }
    await client.deleteSession();
  });

  it("renders a retired startup terminal's output, and still does after a restart", async () => {
    taskId = await client.executeAsync<string>(
      `const cb = arguments[arguments.length - 1];
       const ctx = window.__KANNA_E2E__.setupState;
       Promise.resolve(
         ctx.createItem(${JSON.stringify(repoId)}, ${JSON.stringify(testRepoPath)}, "Read the startup terminal", "pty", {
           selectOnCreate: false,
           agentProvider: "claude",
         })
       ).then((id) => cb(id)).catch((error) => cb("__error:" + (error?.message || String(error))));`,
    );
    expect(taskId).toMatch(/^[0-9a-f]{8}$/);

    const server = await resolveAppKannaServer(client);
    const setupSessionId = await waitForRetiredStartupTerminal(server.baseUrl, taskId);

    await selectTask(taskId);
    await waitForTerminalTab(setupSessionId);
    await activateTab(setupSessionId);

    // The process is gone: anything on screen came from the archive.
    expect(await waitForArchivedLine(setupSessionId)).toContain(SENTINEL);

    // A reload rebuilds every view from what the server can still answer, so
    // an archive that only lived in this render would not survive it.
    await sleep(1_200);
    await client.reload();
    await selectTask(taskId);
    await waitForTerminalTab(setupSessionId);
    await activateTab(setupSessionId);
    expect(await waitForArchivedLine(setupSessionId)).toContain(SENTINEL);
  }, 240_000);

  async function waitForRetiredStartupTerminal(
    baseUrl: string,
    id: string,
    timeoutMs = 90_000,
  ): Promise<string> {
    const deadline = Date.now() + timeoutMs;
    let latest = "";
    while (Date.now() < deadline) {
      const response = await localProcessFetch(
        `${baseUrl}/v1/tasks/${encodeURIComponent(id)}/terminals`,
      );
      if (response.ok) {
        const body = (await response.json()) as { terminals: TaskTerminal[] };
        latest = JSON.stringify(body.terminals);
        const startup = body.terminals.find(
          (terminal) =>
            terminal.role === "setup"
            && terminal.state === "retired"
            && terminal.daemonSessionId,
        );
        if (startup?.daemonSessionId) {
          // The list has to say whether there is an archive; a tab that opens
          // on `retired` alone has no way to know there is anything to show.
          expect(startup.archived).toBe(true);
          return startup.daemonSessionId;
        }
      }
      await sleep(500);
    }
    throw new Error(`the startup terminal never retired; terminals=${latest}`);
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
       }`,
    );
    if (typeof result === "string" && result.startsWith("err:")) {
      throw new Error(`could not select ${id}: ${result.slice(4)}`);
    }
  }

  async function waitForTerminalTab(sessionId: string, timeoutMs = 20_000): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const present = await client.executeSync<boolean>(
        `return Boolean(document.querySelector('[data-testid="main-tab-terminal:${sessionId}"]'));`,
      );
      if (present) return;
      await sleep(250);
    }
    throw new Error(`no tab appeared for the startup terminal ${sessionId}`);
  }

  async function activateTab(sessionId: string): Promise<void> {
    await client.executeSync(
      `document.querySelector('[data-testid="main-tab-terminal:${sessionId}"]').click();`,
    );
    await sleep(300);
  }

  async function waitForArchivedLine(sessionId: string, timeoutMs = 20_000): Promise<string> {
    const deadline = Date.now() + timeoutMs;
    let lines: string[] = [];
    while (Date.now() < deadline) {
      lines = await client.executeSync<string[]>(
        `const buffers = window.__KANNA_E2E__.terminalBuffers;
         if (!buffers || !buffers.sessionIds().includes("${sessionId}")) return [];
         return buffers.lines("${sessionId}");`,
      );
      const match = lines.find((line) => line.includes(SENTINEL));
      if (match) return match;
      await sleep(250);
    }
    throw new Error(
      `the retired startup terminal rendered nothing from its archive: ${JSON.stringify(lines)}`,
    );
  }
});

async function runCommand(command: string[], cwd: string): Promise<void> {
  const [file, ...args] = command;
  const proc = spawn(file, args, { cwd, stdio: "pipe" });
  let stderr = "";
  proc.stderr.setEncoding("utf8");
  proc.stderr.on("data", (chunk: string) => {
    stderr += chunk;
  });
  await new Promise<void>((resolve, reject) => {
    proc.once("error", reject);
    proc.once("exit", (code) => {
      if (code === 0) {
        resolve();
        return;
      }
      reject(new Error(`${command.join(" ")} failed (${code}): ${stderr.trim()}`));
    });
  });
}
