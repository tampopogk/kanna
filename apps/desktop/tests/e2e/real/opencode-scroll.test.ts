import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { beforeAll, afterAll, expect, it } from "vitest";
import { StreamClient } from "@kanna/stream-client";
import { WebDriverClient } from "../helpers/webdriver";
import { assertNativeWindowIdentity, resolveExpectedNativeWindowIdentity } from "../helpers/windowIdentity";
import { resetDatabase, importTestRepo, cleanupWorktrees } from "../helpers/reset";
import { createFixtureRepo, cleanupFixtureRepos } from "../helpers/fixture-repo";
import { callVueMethod, execDb, tauriInvoke } from "../helpers/vue";
import { dismissStartupShortcutsModal } from "../helpers/startupOverlays";
import { resolveAppKannaServer } from "../helpers/kannaServer";

const client = new WebDriverClient();
const run = promisify(execFile);
const artifacts = resolve("../../.tmp/opencode-scroll");
const opencodeTask = "0c5c0001";
const shellTask = "0c5c0002";
let repo = "";
let repoId = "";
let viewer: StreamClient | undefined;

async function verifyWindow(label: string) {
  await assertNativeWindowIdentity(client, await resolveExpectedNativeWindowIdentity(resolve("../..")), label);
}

beforeAll(async () => {
  await client.createSession({ dismissStartupShortcuts: false });
  await verifyWindow("OpenCode scroll");
  await mkdir(artifacts, { recursive: true });
  await writeFile(`${artifacts}/identity.json`, JSON.stringify({
    title: await client.getNativeWindowTitle(), build: await client.getAppBuildInfo(), endpoint: client.getBaseUrl(),
  }));
  await dismissStartupShortcutsModal(client);
  await resetDatabase(client);
  repo = await createFixtureRepo("opencode-scroll");
  repoId = await importTestRepo(client, repo, "opencode-scroll");
});

afterAll(async () => {
  viewer?.close();
  for (const sessionId of [opencodeTask, shellTask]) {
    await tauriInvoke(client, "kill_session", { sessionId }).catch(error => console.warn("scroll fixture cleanup", error));
  }
  if (repo) { await cleanupWorktrees(client, repo); await cleanupFixtureRepos([repo]); }
  await client.deleteSession();
});

async function createTask(id: string, provider: string) {
  const worktree = `${repo}/.kanna-worktrees/task-${id}`;
  await tauriInvoke(client, "git_worktree_add", { repoPath: repo, branch: `task-${id}`, path: worktree });
  await execDb(client, "INSERT INTO pipeline_item (id, repo_id, prompt, stage, branch, agent_type, agent_provider) VALUES (?, ?, ?, ?, ?, ?, ?)",
    [id, repoId, "Offline scrolling fixture", "in progress", `task-${id}`, "pty", provider]);
  await execDb(client, "INSERT INTO worktree (id, pipeline_item_id, path, branch) VALUES (?, ?, ?, ?)",
    [`wt-${id}`, id, worktree, `task-${id}`]);
  return worktree;
}

async function select(id: string) {
  await callVueMethod(client, "loadItems", repoId);
  await callVueMethod(client, "store.selectItem", id);
  await client.waitForElement(".main-panel .terminal-container .xterm-helper-textarea", 30_000);
}

function read(id: string) {
  return client.executeSync<string>(`return window.__KANNA_E2E__.terminalBuffers.lines("local:${id}").join("\\n");`);
}

async function painted(id: string) {
  // Non-activating WKWebView may defer rAF. This existing hook only paints
  // parsed cells; it does not inject output, input, modes, focus or geometry.
  return client.executeSync<string>(`
    const buffers = window.__KANNA_E2E__.terminalBuffers;
    buffers.refresh("local:${id}");
    return buffers.element("local:${id}").querySelector(".xterm-rows").textContent;
  `);
}

async function wheel(id: string, deltaY: number) {
  // tauri-plugin-webdriver 0.2.1's wheel action also dispatches DOM WheelEvent.
  // Exercise xterm's normal wheel producer, not Terminal.input/scrollLines.
  // Trusted active-view intent is covered by terminal-viewer-gestures.test.ts.
  await client.executeSync(`
    const el = window.__KANNA_E2E__.terminalBuffers.element("local:${id}");
    const screen = el.querySelector(".xterm-screen");
    const r = screen.getBoundingClientRect();
    for (let i = 0; i < 20; i++) screen.dispatchEvent(new WheelEvent("wheel", {
      bubbles: true, cancelable: true, clientX: r.x + 100, clientY: r.y + 100, deltaY: ${deltaY}
    }));
  `);
}

async function observeInput(id: string) {
  await client.executeSync(`
    window.scrollInputs = [];
    window.scrollXterm = [];
    if (!window.scrollOriginalSend) {
      window.scrollOriginalSend = WebSocket.prototype.send;
      WebSocket.prototype.send = function(data) {
        try { const f = JSON.parse(data); if (f.type?.startsWith("term_input")) window.scrollInputs.push(f); } catch {}
        return window.scrollOriginalSend.call(this, data);
      };
    }
    const el = window.__KANNA_E2E__.terminalBuffers.element("local:${id}");
    let component = el.parentElement.__vueParentComponent;
    while (component) {
      const term = component.setupState?.terminal;
      if (term?.onBinary) {
        window.scrollBuffer = term.buffer.active.type;
        if (!term.scrollObserved) {
          term.scrollObserved = true;
          term.onBinary(data => window.scrollXterm.push({ kind: "binary", bytes: Array.from(data, c => c.charCodeAt(0)) }));
          term.onData(data => window.scrollXterm.push({ kind: "data", data }));
        }
        break;
      }
      component = component.parent;
    }
  `);
}

async function verifyConversationScroll(label: string) {
  await expect.poll(() => painted(opencodeTask), { timeout: 30_000 }).toContain("SCROLL_TURN_029");
  await observeInput(opencodeTask);
  await writeFile(`${artifacts}/${label}-before.txt`, await read(opencodeTask));
  await client.screenshot(`${artifacts}/${label}-before.png`);
  try {
    await wheel(opencodeTask, -100);
    // Require earlier conversation on painted rows, not an empty grid or a
    // scrollbar/geometry change. No typing or inference occurs in this test.
    await expect.poll(() => painted(opencodeTask), { timeout: 30_000 }).toMatch(/SCROLL_TURN_0[01]\d/);
    expect(await painted(opencodeTask)).not.toContain("SCROLL_TURN_029");
    const trace = await client.executeSync<{ buffer: string; inputs: Array<{ type: string; data_b64: string }>; xterm: Array<{ kind: string }> }>(
      `return { buffer: window.scrollBuffer, inputs: window.scrollInputs, xterm: window.scrollXterm };`);
    expect(trace.buffer).toBe("alternate");
    expect(trace.xterm.some(event => event.kind === "binary")).toBe(false);
    expect(trace.inputs.some(frame => frame.type === "term_input_control" && Buffer.from(frame.data_b64, "base64").toString().startsWith("\x1b[<64;"))).toBe(true);
    await writeFile(`${artifacts}/${label}-inputs.json`, JSON.stringify(trace));
  } finally {
    await writeFile(`${artifacts}/${label}-after.txt`, await read(opencodeTask));
    await painted(opencodeTask);
    await client.screenshot(`${artifacts}/${label}-after.png`);
  }
  await wheel(opencodeTask, 100);
  await expect.poll(() => painted(opencodeTask), { timeout: 30_000 }).toContain("SCROLL_TURN_029");
}

it("scrolls ordinary shell scrollback through the same desktop wheel path", async () => {
  const cwd = await createTask(shellTask, "codex");
  await tauriInvoke(client, "spawn_session", { sessionId: shellTask, cwd, executable: "/bin/sh",
    args: ["-c", "i=0; while [ $i -lt 120 ]; do printf 'SHELL_ROW_%03d\\n' $i; i=$((i+1)); done; exec /bin/cat"], env: {}, cols: 100, rows: 35 });
  await select(shellTask);
  await expect.poll(() => painted(shellTask), { timeout: 30_000 }).toContain("SHELL_ROW_119");
  await wheel(shellTask, -100);
  await expect.poll(() => painted(shellTask), { timeout: 30_000 }).not.toContain("SHELL_ROW_119");
  expect(await painted(shellTask)).toContain("SHELL_ROW_");
});

it("keeps actual OpenCode conversation scrolling after attach and active-view resize snapshots", async () => {
  const worktree = await createTask(opencodeTask, "opencode");
  const executable = process.env.KANNA_SCROLL_OPENCODE || (await run("which", ["opencode"])).stdout.trim();
  const version = (await run(executable, ["--version"])).stdout.trim();
  const storage = `${artifacts}/run-${Date.now()}`;
  const env = {
    XDG_DATA_HOME: `${storage}/data`, XDG_CONFIG_HOME: `${storage}/config`,
    XDG_CACHE_HOME: `${storage}/cache`, XDG_STATE_HOME: `${storage}/state`,
    OPENCODE_DISABLE_AUTOUPDATE: "true", OPENCODE_DISABLE_EXTERNAL_PLUGINS: "true",
  };
  await mkdir(storage, { recursive: true });
  const sessionID = "ses_scroll14932c39";
  const now = Date.now();
  // Import conversation data through OpenCode's supported CLI. All screen
  // output and scrolling are produced by the installed TUI, with no model.
  const messages = Array.from({ length: 30 }, (_, i) => {
    const messageID = `msg_scroll${String(i).padStart(4, "0")}`;
    return {
      info: { id: messageID, sessionID, role: "user", time: { created: now + i }, agent: "build", model: { providerID: "opencode", modelID: "big-pickle" } },
      parts: [{ id: `prt_scroll${i}`, sessionID, messageID, type: "text", text: `SCROLL_TURN_${String(i).padStart(3, "0")}\nOffline conversation fixture. This is stored conversation content rendered by OpenCode.` }],
    };
  });
  await writeFile(`${storage}/conversation.json`, JSON.stringify({
    info: { id: sessionID, slug: "scroll-fixture", projectID: "global", directory: worktree, title: "Offline scrolling fixture", version, time: { created: now, updated: now } }, messages,
  }));
  await run(executable, ["import", `${storage}/conversation.json`], { cwd: worktree, env: { ...process.env, ...env } });
  await writeFile(`${artifacts}/provider.json`, JSON.stringify({ executable, version, storage }));
  await tauriInvoke(client, "spawn_session", { sessionId: opencodeTask, cwd: worktree, executable,
    args: ["--pure", "--session", sessionID], env, cols: 100, rows: 35 });
  await select(opencodeTask);
  await verifyConversationScroll("fresh");

  await client.reload({ dismissStartupShortcuts: false });
  await verifyWindow("OpenCode after reattach");
  await dismissStartupShortcutsModal(client);
  await select(opencodeTask);
  await verifyConversationScroll("reattach");

  const { baseUrl } = await resolveAppKannaServer(client);
  const credential = await tauriInvoke(client, "local_control_credential") as string;
  viewer = new StreamClient({ url: baseUrl.replace(/^http/, "ws") + "/v1/stream", credential, terminalViewerRole: "remote" });
  viewer.registerTerminalViewer(opencodeTask, 90, 30);
  viewer.attachTerminal(opencodeTask, { onOutput() {} });
  viewer.setTerminalViewerVisibility(opencodeTask, true);
  viewer.activateTerminalViewer(opencodeTask);
  const recovery = () => tauriInvoke(client, "get_session_recovery_state", { sessionId: opencodeTask }) as Promise<{ cols: number; rows: number; serialized: string }>;
  await expect.poll(async () => { const s = await recovery(); return [s.cols, s.rows]; }, { timeout: 30_000 }).toEqual([90, 30]);
  await expect.poll(() => client.executeSync(`
    const cursor = window.__KANNA_E2E__.terminalBuffers.cursor("local:${opencodeTask}");
    return [cursor.columns, cursor.rows];
  `), { timeout: 30_000 }).toEqual([90, 30]);
  const snapshot = await recovery();
  expect(snapshot.serialized).toContain("\x1b[?1006h");
  await writeFile(`${artifacts}/resize-snapshot.json`, JSON.stringify(snapshot));
  await verifyConversationScroll("resize");
});
