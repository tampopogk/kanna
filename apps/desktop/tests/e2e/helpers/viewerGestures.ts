import { chromium } from "playwright";
import { expect } from "vitest";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { StreamClient } from "../../../../../packages/stream-client/src/index";
import { observeTerminalViewerInteraction } from "../../../src/composables/terminalViewerInteraction";
import { buildTerminalDocument } from "../../../../mobile/src/screens/buildTerminalDocument";

/** Real browser gesture producers -> shared KSP client -> isolated daemon/PTY.
 * The mobile document supplies rendering. The wide page hosts the desktop
 * gesture observer; native useTerminal rendering is checked by the caller.
 * This is not a physical iOS or native WKWebView wheel test. */
export async function verifyViewerGestures(
  baseUrl: string,
  taskId: string,
  credential: string,
  readDimensions: () => Promise<{ cols: number; rows: number }>,
  readPtyOutput: () => Promise<string>,
  artifactDir: string,
  verifyNativeRendering: (grid: { cols: number; rows: number }) => Promise<void>,
): Promise<void> {
  const browser = await chromium.launch({ headless: true });
  const clients: StreamClient[] = [];
  const errors: string[] = [];
  try {
    const makeViewer = async (local: boolean) => {
      const page = await browser.newPage({ viewport: { width: local ? 1200 : 390, height: 720 }, hasTouch: !local });
      let capacity = { cols: 0, rows: 0 };
      let client: StreamClient | null = null;
      let activations = 0;
      const activate = () => {
        activations++;
        client?.setTerminalViewerVisibility(taskId, true);
        client?.activateTerminalViewer(taskId);
      };
      await page.exposeFunction("claimViewer", activate);
      await page.exposeFunction("terminalBridge", (message: string) => {
        const payload = JSON.parse(message) as { type: string; cols: number; rows: number };
        if (payload.type === "terminal-capacity") {
          capacity = { cols: payload.cols, rows: payload.rows };
          client?.sendTermResize(taskId, payload.cols, payload.rows);
        }
        if (!local && payload.type === "terminal-viewer-interaction") activate();
      });
      await page.setContent(buildTerminalDocument({ bottomInset: 0 }).replace(
        "<head>",
        '<head><script>window.ReactNativeWebView = { postMessage: message => window.terminalBridge(message) };</script>',
      ));
      await expect.poll(() => capacity.cols).toBeGreaterThan(0);
      if (local) {
        await page.evaluate(`(() => { const __name = fn => fn; (${observeTerminalViewerInteraction.toString()})(document.getElementById("viewport"), () => window.claimViewer()); })()`);
      }
      client = new StreamClient({ url: baseUrl.replace(/^http/, "ws") + "/v1/stream", credential, terminalViewerRole: local ? "local" : "remote" });
      clients.push(client);
      client.registerTerminalViewer(taskId, capacity.cols, capacity.rows);
      let paints = Promise.resolve();
      let paintError: unknown;
      const paint = (script: string) => {
        paints = paints.then(async () => { await page.evaluate(script); }).catch(error => { paintError = error; });
      };
      client.attachTerminal(taskId, {
        onError(code, message) { errors.push(`${code}: ${message}`); },
        onSnapshot(cols, rows, dataB64) {
          paint(`window.__setTerminalDims(${JSON.stringify({ cols, rows })}); window.__replaceTerminalState(${JSON.stringify({ chunksB64: [dataB64] })});`);
        },
        onOutput(dataB64) { paint(`window.__appendTerminalChunk(${JSON.stringify({ chunksB64: [dataB64] })});`); },
      });
      return { page, client, get capacity() { return capacity; }, activations: () => activations, flush: async () => { await paints; if (paintError) throw paintError; } };
    };
    const desktop = await makeViewer(true);
    const mobile = await makeViewer(false);
    const assertGrid = async (grid: { cols: number; rows: number }) => {
      await expect.poll(async () => {
        if (errors.length) throw new Error(errors.join("; "));
        return readDimensions();
      }, { timeout: 30_000 }).toEqual(grid);
      // Fixture reports `stty size` on SIGWINCH: not just the daemon registry.
      await expect.poll(readPtyOutput, { timeout: 30_000 }).toBe(`ACTIVE_VIEW:${grid.cols}x${grid.rows}`);
    };
    await mobile.page.touchscreen.tap(100, 250);
    await expect.poll(mobile.activations).toBeGreaterThan(0);
    await assertGrid(mobile.capacity);
    await desktop.page.evaluate('document.getElementById("viewport").dispatchEvent(new Event("scroll")); document.getElementById("viewport").dispatchEvent(new WheelEvent("wheel", { deltaY: -80 }));');
    expect(desktop.activations()).toBe(0);
    await desktop.page.mouse.move(100, 250);
    await desktop.page.mouse.wheel(0, -80);
    await assertGrid(desktop.capacity);
    const mobileActivations = mobile.activations();
    const initialMobileCols = mobile.capacity.cols;
    await mobile.page.setViewportSize({ width: 410, height: 720 });
    await expect.poll(() => mobile.capacity.cols).toBeGreaterThan(initialMobileCols);
    await mobile.page.evaluate('document.getElementById("viewport").dispatchEvent(new Event("scroll"));');
    expect(mobile.activations()).toBe(mobileActivations);
    await assertGrid(desktop.capacity);
    await mobile.page.touchscreen.tap(100, 250);
    await assertGrid(mobile.capacity);
    await mobile.flush();
    await expect.poll(() => mobile.page.locator(".xterm-rows").innerText()).toContain(`ACTIVE_VIEW:${mobile.capacity.cols}x${mobile.capacity.rows}`);
    await verifyNativeRendering(mobile.capacity);
    if (artifactDir) {
      await mobile.page.screenshot({ path: join(artifactDir, "mobile-gesture-real-pty.png") });
      await writeFile(join(artifactDir, "viewer-gestures.json"), JSON.stringify({ desktop: desktop.capacity, mobile: mobile.capacity, desktopGestures: desktop.activations(), mobileGestures: mobile.activations(), ptyReport: await readPtyOutput() }, null, 2));
    }
  } finally {
    for (const client of clients) client.close();
    await browser.close();
  }
}
