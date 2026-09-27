/**
 * The mockup pin script (crates/kanna-server/resources/design-mockup-pins.js)
 * in real Chromium and WebKit, nested the way the desktop shows a mockup:
 * the Kanna window frames the preview listener's shell, which frames the
 * mockup sandboxed (`allow-scripts` only, an opaque origin). The listener is
 * stood in for by routed pages; the script is the server's own file, added
 * the way the listener adds it.
 *
 *   pnpm --filter @kanna/design-editor test:pins
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, webkit, type Browser, type Frame, type Page } from "playwright";

const root = join(dirname(fileURLToPath(import.meta.url)), "../../..");
const pinScript = readFileSync(join(root, "crates/kanna-server/resources/design-mockup-pins.js"), "utf8");

const KANNA = "http://kanna.test";
const LISTENER = "http://127.0.0.1:47999";
const CAP = "c".repeat(32);

const mockupHtml = `<!doctype html><html><head><style>
  body { font: 14px system-ui; margin: 24px }
  button { padding: 8px 16px }
</style></head><body>
  <main aria-label="Tasks">
    <h1>Tasks</h1>
    <ul><li>First</li><li class="row selected">Review design</li></ul>
    <button id="save" onclick="document.title = 'clicked'">Save</button>
  </main>
</body></html>`;

// The Kanna side does what DesignMockupFrame does: accept a message only
// from the mockup window inside its shell, and answer with pins.
const kannaHtml = `<!doctype html><html><body style="margin:0">
<iframe id="shell" style="width:800px;height:500px;border:0" sandbox="allow-scripts allow-same-origin"
  src="${LISTENER}/p/${CAP}/index.html?kanna-shell"></iframe>
<script>
  window.received = [];
  window.forged = [];
  const shell = document.getElementById("shell");
  window.mockup = () => shell.contentWindow && shell.contentWindow.frames[0];
  window.addEventListener("message", (event) => {
    if (!event.data || event.data.kind !== "kanna-mockup") return;
    if (event.source !== window.mockup()) { window.forged.push(event.data); return; }
    window.received.push(event.data);
  });
  window.sendPins = (pins, selected) =>
    window.mockup().postMessage({ kind: "kanna-mockup", type: "pins", pins, selected, reveal: true }, "*");
</script></body></html>`;

const shellHtml = `<!doctype html><meta charset=utf-8>
<style>html,body,iframe{margin:0;border:0;width:100%;height:100%;display:block}</style>
<iframe sandbox="allow-scripts" referrerpolicy="no-referrer" src="/p/${CAP}/index.html"></iframe>`;

async function open(browser: Browser): Promise<{ page: Page; mockup: Frame }> {
  const page = await browser.newPage({ viewport: { width: 900, height: 600 } });
  page.on("pageerror", (error) => console.error(`page error: ${error.message}`));
  await page.route(`${KANNA}/**`, (route) => route.fulfill({ contentType: "text/html", body: kannaHtml }));
  await page.route(`${LISTENER}/**`, (route) => {
    const url = new URL(route.request().url());
    if (url.search === "?kanna-shell") return route.fulfill({ contentType: "text/html", body: shellHtml });
    return route.fulfill({
      contentType: "text/html",
      headers: { "content-security-policy": "sandbox allow-scripts; connect-src 'none'" },
      body: `${mockupHtml}\n<script data-kanna-pins>${pinScript}</script>\n`,
    });
  });
  await page.goto(`${KANNA}/`);
  await page.waitForFunction(() => (window as unknown as { received: Array<{ type: string }> }).received.some((m) => m.type === "ready"));
  const mockup = page.frames().find((frame) => frame.url().endsWith(`/p/${CAP}/index.html`));
  assert.ok(mockup, "the mockup frame loaded");
  return { page, mockup };
}

type Received = { type: string; page?: string; number?: number; pin?: Record<string, unknown> };
const received = (page: Page) => page.evaluate(() => (window as unknown as { received: Received[] }).received);

const cases: Array<[string, (page: Page, mockup: Frame) => Promise<void>]> = [
  [
    "a click pins the element and tells Kanna what it is, without using the mockup",
    async (page, mockup) => {
      const ready = (await received(page)).find((message) => message.type === "ready");
      assert.equal(ready?.page, "index.html");
      await mockup.locator("#save").click();
      await page.waitForFunction(() => (window as unknown as { received: Received[] }).received.some((m) => m.type === "pin"));
      const pin = (await received(page)).find((message) => message.type === "pin")!.pin!;
      assert.equal(pin.selector, "#save");
      assert.equal(pin.tag, "button");
      assert.equal(pin.elementId, "save");
      assert.equal(pin.text, "Save");
      assert.equal(pin.page, "index.html");
      assert.match(String(pin.container), /main/);
      assert.match(String(pin.html), /^<button id="save"/);
      assert.notEqual(await mockup.evaluate(() => document.title), "clicked", "the mockup's own handler did not run");
      const forged = await page.evaluate(() => (window as unknown as { forged: unknown[] }).forged);
      assert.deepEqual(forged, []);
    },
  ],
  [
    "an element without an id gets a selector that finds it again",
    async (page, mockup) => {
      await mockup.locator("li.selected").click();
      await page.waitForFunction(() => (window as unknown as { received: Received[] }).received.some((m) => m.type === "pin"));
      const pin = (await received(page)).find((message) => message.type === "pin")!.pin!;
      assert.equal(pin.classes, "row selected");
      const found = await mockup.evaluate((selector) => document.querySelector(selector)?.textContent, String(pin.selector));
      assert.equal(found, "Review design");
    },
  ],
  [
    "Option/Alt-click uses the mockup instead of pinning",
    async (page, mockup) => {
      await mockup.locator("#save").click({ modifiers: ["Alt"] });
      assert.equal(await mockup.evaluate(() => document.title), "clicked");
      await page.waitForTimeout(200);
      assert.ok(!(await received(page)).some((message) => message.type === "pin"));
    },
  ],
  [
    "pins Kanna sends are drawn as numbered markers, and clicking one selects its thread",
    async (page, mockup) => {
      await page.evaluate(() =>
        (window as unknown as { sendPins: (pins: unknown[], selected: number) => void }).sendPins(
          [
            { number: 3, page: "index.html", selector: "#save" },
            { number: 4, page: "other.html", selector: "#save" },
            { number: 5, page: "index.html", selector: "#missing" },
          ],
          3,
        ),
      );
      const marker = mockup.locator("[data-kanna-pins] button");
      await marker.first().waitFor();
      assert.deepEqual(await marker.allTextContents(), ["3"], "only this page's pins whose element exists");
      await marker.first().click();
      await page.waitForFunction(() => (window as unknown as { received: Received[] }).received.some((m) => m.type === "select"));
      const select = (await received(page)).find((message) => message.type === "select");
      assert.equal(select?.number, 3);
      assert.ok(!(await received(page)).some((message) => message.type === "pin"), "a marker click is not a pin");
    },
  ],
  [
    "the mockup's own scripts cannot draw pins by posting to themselves",
    async (_page, mockup) => {
      await mockup.evaluate(() =>
        window.postMessage({ kind: "kanna-mockup", type: "pins", pins: [{ number: 9, page: "index.html", selector: "#save" }] }, "*"),
      );
      await new Promise((resolve) => setTimeout(resolve, 200));
      assert.equal(await mockup.locator("[data-kanna-pins] button").count(), 0);
    },
  ],
];

let failures = 0;
for (const [name, launcher] of [
  ["chromium", chromium],
  ["webkit", webkit],
] as const) {
  const browser = await launcher.launch();
  try {
    for (const [title, run] of cases) {
      const { page, mockup } = await open(browser);
      try {
        await run(page, mockup);
        console.log(`✓ [${name}] ${title}`);
      } catch (error) {
        failures += 1;
        console.error(`✗ [${name}] ${title}\n  ${error instanceof Error ? error.stack : error}`);
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
  }
}
if (failures) {
  console.error(`${failures} mockup pin test(s) failed`);
  process.exit(1);
}
console.log("all mockup pin tests passed");
