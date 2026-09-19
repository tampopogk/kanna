import { setTimeout as sleep } from "node:timers/promises";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { cleanupFixtureRepos, createFixtureRepo } from "../helpers/fixture-repo";
import {
  cleanupWorktrees,
  importTestRepoDirect,
  resetDatabase,
} from "../helpers/reset";
import { WebDriverClient } from "../helpers/webdriver";

const remoteRepoId = "cloud:remote-terminal-geometry-repo";
const remoteTaskId = "cloud:remote-terminal-geometry-task";
/** The owner's local task id, which is what the remote pane renders under. */
const remoteOwnerTaskId = "remote-terminal-geometry-task";

interface ViewerMeasurement {
  /** What `FitAddon.proposeDimensions()` would register for this viewer. */
  availableCols: number;
  availableRows: number;
  /** The grid this viewer is currently rendering. */
  gridCols: number;
  gridRows: number;
  /** The pane the viewer measures itself against. */
  shellWidth: number;
  shellHeight: number;
}

function remoteSnapshot() {
  return {
    repos: [{
      id: remoteRepoId,
      path: "cloud",
      name: "Remote Terminal Geometry",
      default_branch: "main",
      remote_url: "https://example.invalid/kanna/remote-terminal-geometry.git",
      remote_url_hash: "remote-terminal-geometry-hash",
      hidden: 0,
      sort_order: 1,
      created_at: "2026-09-19T00:00:00.000Z",
      last_opened_at: "2026-09-19T00:00:00.000Z",
    }],
    items: [{
      id: remoteTaskId,
      repo_id: remoteRepoId,
      prompt: "Remote terminal geometry task",
      display_name: "Remote terminal geometry task",
      pipeline: "default",
      pipeline_def: null,
      stage: "in progress",
      branch: "task-remote-terminal-geometry",
      pr_number: null,
      pr_url: null,
      closed_at: null,
      agent_type: "pty",
      agent_provider: "codex",
      activity: "idle",
      activity_revision: 1,
      transition_revision: "run-remote-terminal-geometry-1",
      activity_changed_at: "2026-09-19T00:00:00.000Z",
      unread_at: null,
      port_offset: null,
      port_env: null,
      pinned: 0,
      pin_order: null,
      base_ref: "origin/main",
      agent_session_id: null,
      teardown_started_at: null,
      parent_task_id: null,
      notify_task_id: null,
      issue_number: null,
      issue_title: null,
      last_output_preview: null,
      created_at: "2026-09-19T00:00:00.000Z",
      updated_at: "2026-09-19T00:00:00.000Z",
    }],
    terminalRefs: {
      [remoteTaskId]: {
        ownerDesktopId: "peer-terminal-geometry",
        ownerLocalRepoId: "remote-terminal-geometry-repo",
        ownerLocalTaskId: "remote-terminal-geometry-task",
        transport: "cloud",
      },
    },
    blockedByTaskIds: {},
    transferMachines: [],
  };
}

/**
 * Read what this viewer would register. Reading `clientWidth` flushes pending
 * layout, so the measurement is synchronous; polling covers xterm's renderer
 * updating its own cell dimensions a turn later. Deliberately not
 * `requestAnimationFrame` — an occluded WKWebView stops ticking it.
 */
async function measureViewer(
  client: WebDriverClient,
  timeoutMs = 10_000,
): Promise<ViewerMeasurement> {
  const deadline = Date.now() + timeoutMs;
  let last: unknown = null;
  while (Date.now() < deadline) {
    last = await client.executeSync<ViewerMeasurement | string>(
      `const buffers = window.__KANNA_E2E__ && window.__KANNA_E2E__.terminalBuffers;
       if (!buffers) return "no terminal buffer registry";
       const shell = document.querySelector(".cloud-terminal-shell");
       if (!(shell instanceof HTMLElement)) return "no cloud terminal shell";
       const shellWidth = shell.clientWidth;
       const shellHeight = shell.clientHeight;
       const viewport = buffers.viewport(${JSON.stringify(remoteOwnerTaskId)});
       if (!viewport) {
         const element = buffers.element(${JSON.stringify(remoteOwnerTaskId)});
         const rect = element ? element.getBoundingClientRect() : null;
         return "no viewport measurement yet: sessions=" + JSON.stringify(buffers.sessionIds())
           + " element=" + (rect ? rect.width + "x" + rect.height : "none")
           + " shell=" + shellWidth + "x" + shellHeight;
       }
       const cursor = buffers.cursor(${JSON.stringify(remoteOwnerTaskId)});
       return {
         availableCols: viewport.availableCols,
         availableRows: viewport.availableRows,
         gridCols: cursor.columns,
         gridRows: cursor.rows,
         shellWidth: shellWidth,
         shellHeight: shellHeight,
       };`,
    );
    if (typeof last === "object" && last !== null) return last as ViewerMeasurement;
    await sleep(100);
  }
  throw new Error(`timed out measuring the remote viewer; last=${JSON.stringify(last)}`);
}

describe("remote terminal geometry", () => {
  const client = new WebDriverClient();
  let fixtureRepoPath = "";

  beforeAll(async () => {
    await client.createSession();
    await resetDatabase(client);
    fixtureRepoPath = await createFixtureRepo("remote-terminal-geometry");
    await importTestRepoDirect(client, fixtureRepoPath, "Local Terminal Geometry");
    const setupResult = await client.executeAsync<string>(
      `const cb = arguments[arguments.length - 1];
       const ctx = window.__KANNA_E2E__.setupState;
       Promise.resolve(ctx.refreshAllItems())
         .then(function() {
           ctx.__e2eInjectRemoteSnapshot(
             "cloud",
             ${JSON.stringify(remoteSnapshot())},
           );
           cb("ok");
         })
         .catch(function(error) { cb("err:" + error); });`,
    );
    expect(setupResult).toBe("ok");
  });

  afterAll(async () => {
    if (fixtureRepoPath) {
      await cleanupWorktrees(client, fixtureRepoPath).catch(() => undefined);
      await cleanupFixtureRepos([fixtureRepoPath]);
    }
    await client.deleteSession();
  });

  // The owner watched a remote pane walk the PTY from Studio Display size down
  // to laptop size two columns at a time, one relay round trip per step. The
  // daemon's election is right — it snaps to the controller's grid — but a
  // remote viewer hydrates at the owner's authoritative grid, and if the pane
  // can widen to that grid then the viewer's next measurement describes the
  // grid instead of the pane. The controller registers that measurement, the
  // daemon applies it, the next snapshot hydrates at the new grid, and the
  // loop takes another step. It is a measurement feedback loop, not the
  // network: the relay only sets the step period.
  it("measures its own pane, not the authoritative grid it is rendering", async () => {
    const remoteRowSelector = `.sidebar .workflow-item[data-task-id="${remoteTaskId}"]`;
    const remoteRow = await client.waitForElement(remoteRowSelector, 10_000);
    await client.click(remoteRow);
    await client.waitForElement(`${remoteRowSelector}.selected`, 10_000);
    await client.waitForElement(".cloud-terminal-shell .xterm-helper-textarea", 10_000);

    const before = await measureViewer(client);
    expect(before.availableCols).toBeGreaterThan(0);
    expect(before.availableRows).toBeGreaterThan(0);
    expect(before.shellWidth).toBeGreaterThan(0);

    // Hydrate at an owner grid far larger than this pane in both axes, the way
    // `applyTerminalSnapshot` restores a snapshot's recorded dimensions.
    const ownerCols = before.availableCols * 2 + 40;
    const ownerRows = before.availableRows * 2 + 10;
    await client.executeSync(
      `window.__KANNA_E2E__.terminalBuffers.resize(
         ${JSON.stringify(remoteOwnerTaskId)}, ${ownerCols}, ${ownerRows});`,
    );

    await sleep(500);

    const after = await measureViewer(client);
    expect(after.gridCols).toBe(ownerCols);
    expect(after.gridRows).toBe(ownerRows);

    // The pane clips the oversized grid instead of stretching to it, so the
    // viewer's proposal is the same measurement it made before hydrating.
    // Without this the proposal tracked the grid — one step of the descent.
    expect(after.availableCols).toBe(before.availableCols);
    expect(after.availableRows).toBe(before.availableRows);
    expect(after.shellWidth).toBe(before.shellWidth);
    expect(after.shellHeight).toBe(before.shellHeight);
  });
});
