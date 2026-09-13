import { chromium } from "playwright";
import { expect } from "vitest";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { StreamClient } from "../../../../../packages/stream-client/src/index";
import { build } from "vite";
import { fileURLToPath } from "node:url";
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
  /** Every `stty size` the fixture has reported, oldest first. Asserting the
   * final dimensions cannot detect oscillation; the sequence can. */
  readPtyHistory: () => Promise<string[]>,
): Promise<void> {
  // Bundle the source module, including its module-scoped values and imports.
  // Function.toString() silently loses those dependencies in the browser.
  const bundled = await build({
    configFile: false,
    logLevel: "error",
    build: {
      write: false,
      minify: false,
      lib: {
        entry: fileURLToPath(new URL("../../../src/composables/terminalViewerInteraction.ts", import.meta.url)),
        name: "KannaViewerInteraction",
        formats: ["iife"],
      },
    },
  });
  const output = Array.isArray(bundled) ? bundled[0] : bundled;
  if (!("output" in output)) throw new Error("Expected a single observer bundle");
  const observerBundle = output.output.find(chunk => chunk.type === "chunk");
  if (!observerBundle || observerBundle.type !== "chunk") throw new Error("Missing observer bundle");
  const browser = await chromium.launch({ headless: true });
  const clients: StreamClient[] = [];
  const errors: string[] = [];
  try {
    const makeViewer = async (local: boolean) => {
      const page = await browser.newPage({ viewport: { width: local ? 1200 : 390, height: 720 }, hasTouch: !local });
      page.on("pageerror", error => errors.push(`browser ${local ? "desktop" : "mobile"}: ${error.stack ?? error.message}`));
      let capacity = { cols: 0, rows: 0 };
      let client: StreamClient | null = null;
      let activations = 0;
      let wheels = 0;
      await page.exposeFunction("recordWheel", () => { wheels++; });
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
        await page.evaluate('document.getElementById("viewport").addEventListener("wheel", event => { if (event.isTrusted) window.recordWheel(); }, { capture: true, passive: true });');
        await page.addScriptTag({ content: observerBundle.code });
        await page.evaluate('window.KannaViewerInteraction.observeTerminalViewerInteraction(document.getElementById("viewport"), () => window.claimViewer());');
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
      return { page, client, get capacity() { return capacity; }, activations: () => activations, wheels: () => wheels, flush: async () => { await paints; if (paintError) throw paintError; } };
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
    const stability: Array<{ phase: string; reports: string[] }> = [];
    /** Assert how the PTY actually got to `grid` since `mark`: one direct
     * transition, with no obsolete intermediate dimensions on the way.
     *
     * The kernel coalesces rapid SIGWINCHes, so a missing report never fails
     * this; a *present* obsolete report is the real signal, and it is exactly
     * what a stale claim or a replayed size history produces. */
    const assertDirectTransition = (
      phase: string,
      mark: number,
      history: string[],
      grid: { cols: number; rows: number },
    ) => {
      const since = history.slice(mark);
      stability.push({ phase, reports: since });
      const distinct = [...new Set(since)];
      expect(
        distinct,
        `${phase}: PTY walked through obsolete dimensions on its way to ${grid.cols}x${grid.rows}`,
      ).toEqual([`ACTIVE_VIEW:${grid.cols}x${grid.rows}`]);
    };
    /** No further dimension change once control and paint work has drained.
     * This is a stability observation, not a setup-performance deadline. */
    const assertNoFurtherResize = async (phase: string) => {
      const settled = await readPtyHistory();
      await new Promise(resolve => setTimeout(resolve, 5_000));
      const after = await readPtyHistory();
      stability.push({ phase, reports: after.slice(settled.length) });
      expect(after, `${phase}: PTY kept resizing after the viewer settled`).toEqual(settled);
    };
    let mark = (await readPtyHistory()).length;
    await mobile.page.touchscreen.tap(100, 250);
    await expect.poll(mobile.activations).toBeGreaterThan(0);
    await assertGrid(mobile.capacity);
    assertDirectTransition("mobile-claim", mark, await readPtyHistory(), mobile.capacity);
    await assertNoFurtherResize("mobile-quiet-reading");

    await desktop.page.evaluate('document.getElementById("viewport").dispatchEvent(new Event("scroll")); document.getElementById("viewport").dispatchEvent(new WheelEvent("wheel", { deltaY: -80 }));');
    expect(desktop.activations()).toBe(0);
    mark = (await readPtyHistory()).length;
    await desktop.page.mouse.move(100, 250);
    await desktop.page.mouse.wheel(0, -80);
    await assertGrid(desktop.capacity);
    assertDirectTransition("desktop-claim", mark, await readPtyHistory(), desktop.capacity);

    // Repeated claims by the existing owner must cost the PTY nothing.
    mark = (await readPtyHistory()).length;
    const beforeBurst = desktop.activations();
    for (let tick = 0; tick < 40; tick += 1) {
      await desktop.page.mouse.wheel(0, -20);
    }
    await assertGrid(desktop.capacity);
    await expect.poll(desktop.activations).toBe(beforeBurst + 40);
    expect(
      (await readPtyHistory()).slice(mark),
      "a sustained scroll by the existing owner resized the PTY",
    ).toEqual([]);
    await assertNoFurtherResize("desktop-scroll-burst");

    // Freeze A's browser time: real trusted wheel events execute the bundled
    // production observer, while KSP and the real daemon drain independently.
    // This establishes A@0, A@10, B@50 without any wall-clock race/deadline.
    await desktop.page.clock.install({ time: 0 });
    await desktop.page.clock.pauseAt(1000);
    const beforeInterleave = desktop.activations();
    const beforeWheels = desktop.wheels();
    mark = (await readPtyHistory()).length;
    await desktop.page.mouse.wheel(0, -20); // A@0
    await expect.poll(desktop.wheels).toBe(beforeWheels + 1);
    await desktop.page.clock.runFor(10);
    await desktop.page.mouse.wheel(0, -20); // A@10
    await expect.poll(desktop.wheels).toBe(beforeWheels + 2);
    await assertGrid(desktop.capacity);
    expect((await readPtyHistory()).slice(mark)).toEqual([]);
    await desktop.page.clock.runFor(40);
    await mobile.page.touchscreen.tap(100, 250); // B@50
    await assertGrid(mobile.capacity);
    assertDirectTransition("interleaved-newer-mobile", mark, await readPtyHistory(), mobile.capacity);
    const newerViewerHistory = await readPtyHistory();
    await desktop.page.clock.runFor(50); // Former A trailing claim at 100
    await desktop.page.clock.runFor(5000);
    expect(desktop.activations()).toBe(beforeInterleave + 2);
    await assertGrid(mobile.capacity);
    await assertNoFurtherResize("interleaved-no-stale-reclaim");
    expect(await readPtyHistory()).toEqual(newerViewerHistory);
    mark = newerViewerHistory.length;
    await desktop.page.mouse.wheel(0, -20); // A's legitimate later handoff
    await expect.poll(desktop.activations).toBe(beforeInterleave + 3);
    await assertGrid(desktop.capacity);
    assertDirectTransition("interleaved-legitimate-desktop-return", mark, await readPtyHistory(), desktop.capacity);
    await desktop.page.clock.resume();

    // Two viewers actually contending: each handoff is worth exactly one
    // direct transition. Anything more is the reported oscillation.
    for (let round = 0; round < 3; round += 1) {
      mark = (await readPtyHistory()).length;
      await mobile.page.touchscreen.tap(100, 250);
      await assertGrid(mobile.capacity);
      assertDirectTransition(`alternation-${round}-mobile`, mark, await readPtyHistory(), mobile.capacity);
      mark = (await readPtyHistory()).length;
      await desktop.page.mouse.wheel(0, -20);
      await assertGrid(desktop.capacity);
      assertDirectTransition(`alternation-${round}-desktop`, mark, await readPtyHistory(), desktop.capacity);
    }
    await assertNoFurtherResize("alternation-settled");
    const mobileActivations = mobile.activations();
    const initialMobileCols = mobile.capacity.cols;
    await mobile.page.setViewportSize({ width: 410, height: 720 });
    await expect.poll(() => mobile.capacity.cols).toBeGreaterThan(initialMobileCols);
    await mobile.page.evaluate('document.getElementById("viewport").dispatchEvent(new Event("scroll"));');
    expect(mobile.activations()).toBe(mobileActivations);
    await assertGrid(desktop.capacity);
    mark = (await readPtyHistory()).length;
    await mobile.page.touchscreen.tap(100, 250);
    await assertGrid(mobile.capacity);
    assertDirectTransition("mobile-reclaim", mark, await readPtyHistory(), mobile.capacity);
    await assertNoFurtherResize("mobile-reclaim-quiet");
    await mobile.flush();
    await expect.poll(() => mobile.page.locator(".xterm-rows").innerText()).toContain(`ACTIVE_VIEW:${mobile.capacity.cols}x${mobile.capacity.rows}`);
    await verifyNativeRendering(mobile.capacity);
    if (artifactDir) {
      await mobile.page.screenshot({ path: join(artifactDir, "mobile-gesture-real-pty.png") });
      await writeFile(join(artifactDir, "viewer-gestures.json"), JSON.stringify({ desktop: desktop.capacity, mobile: mobile.capacity, desktopGestures: desktop.activations(), mobileGestures: mobile.activations(), ptyReport: await readPtyOutput(), ptyHistory: await readPtyHistory(), stability }, null, 2));
    }
    expect(errors, "browser handlers and KSP must complete without errors").toEqual([]);
  } finally {
    for (const client of clients) client.close();
    await browser.close();
  }
}
