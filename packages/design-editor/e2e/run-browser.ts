/**
 * Browser tests of the design editor's interaction details (docs/specs/
 * app-design.md §4, the owner's): run in real Chromium and WebKit, because
 * selection, focus and key handling are exactly what DOM stand-ins get wrong.
 *
 *   pnpm --filter @kanna/design-editor test:browser
 */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, webkit, type Browser, type Page } from "playwright";

const packageDir = join(dirname(fileURLToPath(import.meta.url)), "..");
execFileSync("npx", ["vite", "build", "--config", "vite.harness.config.ts", "--logLevel", "warn"], {
  cwd: packageDir,
  stdio: "inherit",
});
const script = readFileSync(join(packageDir, "dist-harness", "harness.js"), "utf8").replace(/<\/script/gi, "<\\/script");
const css = readFileSync(join(packageDir, "dist-harness", "harness.css"), "utf8");
const html = `<!doctype html><html><head><meta charset="utf-8"><style>${css}</style></head>
<body><div id="root" style="padding:40px"></div><script>${script}</script></body></html>`;

type Created = { kind: string; body: string; anchor?: { quotedText: string; blockId: string } };

async function open(browser: Browser): Promise<Page> {
  const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
  page.on("pageerror", (error) => console.error(`page error: ${error.message}`));
  await page.setContent(html);
  await page.waitForFunction(() => window.__harness?.ready === true);
  await page.waitForSelector(".bn-editor [contenteditable='true'], .bn-editor[contenteditable='true']");
  return page;
}

const created = (page: Page) => page.evaluate(() => window.__harness.created() as Created[]);
const blockTexts = (page: Page) =>
  page.$$eval(".bn-block-content", (nodes) => nodes.map((node) => (node.textContent ?? "").trim()));
const menuOpen = (page: Page) => page.locator(".bn-suggestion-menu").isVisible().catch(() => false);
const modifier = process.platform === "darwin" ? "Meta" : "Control";

async function focusEditorEnd(page: Page) {
  await page.locator(".bn-block-content").last().click();
  await page.keyboard.press("End");
}

const cases: Array<[string, (page: Page) => Promise<void>]> = [
  [
    "/agent completes in place with Tab, focus stays on the line, and the next Enter sends once",
    async (page) => {
      await focusEditorEnd(page);
      await page.keyboard.type("/ag");
      await page.waitForSelector(".bn-suggestion-menu");
      await page.keyboard.press("Tab");
      await page.waitForFunction(() => !document.querySelector(".bn-suggestion-menu"));
      assert.deepEqual((await blockTexts(page)).at(-1), "/agent");
      // Focus stayed in the line: typing continues it.
      await page.keyboard.type("make it shorter");
      assert.equal((await blockTexts(page)).at(-1), "/agent make it shorter");
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => window.__harness.created().length === 1);
      await page.keyboard.press("Enter");
      await page.waitForTimeout(300);
      const threads = await created(page);
      assert.equal(threads.length, 1, "the second Enter does not send again");
      assert.deepEqual(
        { kind: threads[0].kind, body: threads[0].body, anchor: threads[0].anchor ?? null },
        { kind: "message", body: "make it shorter", anchor: null },
      );
      assert.ok(!(await blockTexts(page)).some((text) => text.startsWith("/agent")), "the line is cleared");
    },
  ],
  [
    "Enter also picks /agent from the menu, and /agent alone does not start a new line",
    async (page) => {
      await focusEditorEnd(page);
      await page.keyboard.type("/agent");
      await page.waitForSelector(".bn-suggestion-menu");
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => !document.querySelector(".bn-suggestion-menu"));
      const before = (await blockTexts(page)).length;
      await page.keyboard.press("Enter");
      assert.equal((await blockTexts(page)).length, before, "no new block for an empty /agent");
      assert.equal((await created(page)).length, 0);
    },
  ],
  [
    "Tab picks any highlighted slash-menu item",
    async (page) => {
      await focusEditorEnd(page);
      await page.keyboard.type("/heading");
      await page.waitForSelector(".bn-suggestion-menu");
      await page.keyboard.press("Tab");
      await page.waitForFunction(() => !document.querySelector(".bn-suggestion-menu"));
      assert.ok(await page.locator("[data-content-type='heading']").count(), "the block became a heading");
      assert.equal(await menuOpen(page), false);
    },
  ],
  [
    "selecting text shows one toolbar with Comment first; cmd-Enter comments; the anchor is tinted and underlined",
    async (page) => {
      await focusEditorEnd(page);
      await page.keyboard.type("The feedback panel lists threads");
      await page.keyboard.down("Shift");
      for (let index = 0; index < "threads".length; index += 1) await page.keyboard.press("ArrowLeft");
      await page.keyboard.up("Shift");
      await page.waitForSelector(".bn-formatting-toolbar");
      assert.equal(await page.locator(".bn-formatting-toolbar").count(), 1, "one toolbar");
      const first = page.locator(".bn-formatting-toolbar > *").first();
      assert.match((await first.textContent()) ?? "", /Comment/);
      await page.keyboard.press(`${modifier}+Enter`);
      await page.waitForSelector(".bn-comment-editor [contenteditable='true'], .bn-thread .bn-editor [contenteditable='true']");
      await page.keyboard.type("Number them by creation");
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => window.__harness.created().length === 1);
      const [thread] = await created(page);
      assert.equal(thread.kind, "comment");
      assert.equal(thread.body, "Number them by creation");
      assert.equal(thread.anchor?.quotedText, "threads");
      const mark = page.locator(".bn-thread-mark").first();
      await mark.waitFor();
      const style = await mark.evaluate((node) => {
        const computed = getComputedStyle(node);
        return { line: `${computed.borderBottomStyle} ${computed.borderBottomWidth}`, background: computed.backgroundColor };
      });
      assert.equal(style.line, "solid 2px", "underlined");
      assert.notEqual(style.background, "rgba(0, 0, 0, 0)");
      assert.match(await page.evaluate(() => window.__harness.serverText()), /comment--/, "the anchor reached the server");
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
      const page = await open(browser);
      try {
        await run(page);
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
  console.error(`${failures} browser test(s) failed`);
  process.exit(1);
}
console.log("all browser tests passed");
