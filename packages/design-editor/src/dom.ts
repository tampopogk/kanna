/**
 * Give Node the browser globals BlockNote touches when it builds an editor.
 * Only for scripts and tests that run the headless editor outside a browser.
 */
export async function installDom(): Promise<void> {
  if (typeof globalThis.document !== "undefined") return;
  const { JSDOM } = await import("jsdom");
  const dom = new JSDOM("<!doctype html><html><body></body></html>");
  const g = globalThis as Record<string, unknown>;
  g.window = dom.window;
  g.document = dom.window.document;
  for (const key of [
    "navigator",
    "HTMLElement",
    "Node",
    "DOMParser",
    "getComputedStyle",
    "MutationObserver",
    "Element",
    "DocumentFragment",
  ]) {
    g[key] ??= (dom.window as unknown as Record<string, unknown>)[key];
  }
}
