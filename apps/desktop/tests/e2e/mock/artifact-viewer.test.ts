import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as sleep } from "node:timers/promises";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { localProcessFetch } from "@kanna/local-process-fetch";
import { WebDriverClient } from "../helpers/webdriver";
import { cleanupFixtureRepos, createSeedFixtureRepo } from "../helpers/fixture-repo";
import { resolveAppKannaServer } from "../helpers/kannaServer";
import { importTestRepo, resetDatabase } from "../helpers/reset";
import { callVueMethod, execDb } from "../helpers/vue";

/**
 * The artifact viewer in the real desktop webview: an HTML mockup published to
 * the repository's artifact store, opened by tree id, framed from the store's
 * loopback preview listener with its relative assets, beside the comment
 * anchors and decision recorded on that exact version.
 *
 * The mockup is hostile on purpose. It tries to reach the control API, read
 * its host, find a native bridge, open a window and navigate the top-level
 * window, and prints what happened inside itself; the screenshot is the
 * visual record, and the assertions check the host survived all of it.
 */

const WORKTREE_ROOT = fileURLToPath(new URL("../../../../../", import.meta.url));
const SCREENSHOT_DIR = join(WORKTREE_ROOT, ".tmp", "visual");

function probeHtml(controlBaseUrl: string, heading: string): string {
  return `<!doctype html>
<html><head>
<meta charset="utf-8">
<link rel="stylesheet" href="css/site.css">
<script src="js/app.js"></script>
</head><body>
<header><img src="img/logo.svg" alt="logo" width="40" height="40"><h1>${heading}</h1></header>
<p id="asset-check">Relative assets: <span id="script-check">script NOT loaded</span></p>
<p><a href="pages/about.html">In-tree page</a></p>
<h2>Unprivileged HTML probe</h2>
<ul id="probe"></ul>
<script>
const probe = document.getElementById("probe");
function report(name, outcome) {
  const item = document.createElement("li");
  item.dataset.probe = name;
  item.textContent = name + ": " + outcome;
  item.className = /^(blocked|absent|opaque)/.test(outcome) ? "ok" : "bad";
  probe.appendChild(item);
}
function attempt(name, action, blockedWhen) {
  try {
    const value = action();
    report(name, blockedWhen(value) ? "blocked (" + String(value) + ")" : "ALLOWED (" + String(value) + ")");
  } catch (error) {
    report(name, "blocked (" + error.name + ")");
  }
}
report("origin", self.origin === "null" ? "opaque (null)" : "NOT OPAQUE " + self.origin);
attempt("read host document", () => window.parent.document.title, () => false);
attempt("Tauri bridge", () => typeof window.__TAURI_INTERNALS__ + "/" + typeof window.__TAURI__ + "/" + typeof window.ipc,
  (value) => value === "undefined/undefined/undefined");
attempt("React Native bridge", () => typeof window.ReactNativeWebView, (value) => value === "undefined");
attempt("cookies", () => document.cookie, () => false);
attempt("localStorage", () => window.localStorage.length, () => false);
attempt("window.open", () => window.open("https://example.com/", "_blank"), (value) => value === null);
fetch(${JSON.stringify(`${controlBaseUrl}/v1/status`)}).then(
  (response) => report("fetch control API", "ALLOWED (" + response.status + ")"),
  (error) => report("fetch control API", "blocked (" + error.name + ")")
);
setTimeout(() => {
  attempt("navigate top window", () => { window.top.location.href = "https://example.com/"; return "assigned"; }, () => false);
}, 300);
</script>
</body></html>`;
}

async function writeMockup(directory: string, controlBaseUrl: string, heading: string, accent: string) {
  await mkdir(join(directory, "css"), { recursive: true });
  await mkdir(join(directory, "js"), { recursive: true });
  await mkdir(join(directory, "img"), { recursive: true });
  await mkdir(join(directory, "pages"), { recursive: true });
  await writeFile(join(directory, "index.html"), probeHtml(controlBaseUrl, heading));
  await writeFile(join(directory, "css/site.css"),
    `body { font: 14px -apple-system, sans-serif; margin: 16px; color: #1b2230; }
header { display: flex; gap: 12px; align-items: center; border-bottom: 4px solid ${accent}; }
h1 { color: ${accent}; }
li.ok { color: #16723a; } li.bad { color: #b3261e; font-weight: 700; }
`);
  await writeFile(join(directory, "js/app.js"),
    `addEventListener("DOMContentLoaded", () => { document.getElementById("script-check").textContent = "css, js/app.js and img/logo.svg loaded"; });`);
  await writeFile(join(directory, "img/logo.svg"),
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><circle cx="20" cy="20" r="18" fill="${accent}"/></svg>`);
  await writeFile(join(directory, "pages/about.html"),
    `<!doctype html><link rel="stylesheet" href="../css/site.css"><h1>About (same tree)</h1>`);
}

describe("artifact viewer", () => {
  const client = new WebDriverClient();
  const taskId = "artifact-viewer-producer";
  let repoPath = "";
  let repoId = "";

  beforeAll(async () => {
    await client.createSession();
    await resetDatabase(client);
    repoPath = await createSeedFixtureRepo("task-switch-minimal");
    // Keep the artifact store inside this worktree's scratch space.
    const store = join(WORKTREE_ROOT, ".tmp", "e2e-artifacts", `${Date.now()}`, "artifacts.git");
    await mkdir(join(repoPath, ".kanna"), { recursive: true });
    await writeFile(join(repoPath, ".kanna", "config.local.json"),
      JSON.stringify({ artifacts: { repositoryPath: store } }));
    repoId = await importTestRepo(client, repoPath, "artifact-viewer-fixture");
    await execDb(client,
      `INSERT OR REPLACE INTO pipeline_item (id, repo_id, prompt, stage, branch, agent_type, created_at, updated_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
      [taskId, repoId, "Artifact producer", "in progress", null, "agent", "2026-09-23T00:00:00.000Z", "2026-09-23T00:00:00.000Z"]);
    await execDb(client,
      "INSERT OR REPLACE INTO worktree (id, pipeline_item_id, path, branch) VALUES (?, ?, ?, ?)",
      [`wt-${taskId}`, taskId, repoPath, "main"]);
  });

  afterAll(async () => {
    await cleanupFixtureRepos(repoPath ? [repoPath] : []);
    await client.deleteSession();
  });

  it("frames a published mockup with its assets, shows exact-version anchors, and contains the page", async () => {
    const server = await resolveAppKannaServer(client);
    const api = async (method: string, path: string, body?: unknown) => {
      const response = await localProcessFetch(`${server.baseUrl}${path}`, {
        method,
        headers: { "content-type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      const text = await response.text();
      expect(response.ok, `${method} ${path}: ${text}`).toBe(true);
      return JSON.parse(text) as Record<string, unknown>;
    };

    const mockup = join(repoPath, "artifact-probe");
    await writeMockup(mockup, server.baseUrl, "Checkout mockup v1", "#6b7280");
    const v1 = await api("POST", `/v1/tasks/${taskId}/artifacts`, { path: "artifact-probe", kind: "mockup" });
    await writeMockup(mockup, server.baseUrl, "Checkout mockup v2", "#2563eb");
    const v2 = await api("POST", `/v1/tasks/${taskId}/artifacts`, {
      path: "artifact-probe", kind: "mockup", previous: v1.artifactId,
    });
    const v2Id = v2.artifactId as string;
    await api("POST", `/v1/repos/${repoId}/artifacts/${v2Id}/comments`, {
      author: "stakeholder", body: "Header accent is too loud on v2",
      anchor: { path: "css/site.css", position: "line 2", excerpt: "border-bottom: 4px solid #2563eb" },
    });
    await api("POST", `/v1/repos/${repoId}/artifacts/${v1.artifactId}/comments`, {
      author: "stakeholder", body: "v1 note that must not show on v2",
    });
    await api("POST", `/v1/repos/${repoId}/artifacts/${v2Id}/decisions`, { who: "owner", what: "approved with the accent change" });

    const opened = await callVueMethod(client, "appModals.openArtifact", repoId, v2Id);
    expect(opened).not.toMatchObject({ __error: expect.anything() });
    await client.waitForElement('[data-testid="artifact-frame"]', 10_000);
    const frame = await client.executeSync<{ src: string; sandbox: string }>(
      `const frame = document.querySelector('[data-testid="artifact-frame"]');
       return { src: frame.getAttribute("src"), sandbox: frame.getAttribute("sandbox") };`);
    expect(frame.sandbox).toBe("allow-scripts");
    expect(frame.src).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/a\/[0-9a-f]{32}\/index\.html$/);
    expect(await client.executeSync<string[]>(
      `return Array.from(document.querySelectorAll('[data-testid="artifact-comment"]')).map((node) => node.textContent);`,
    )).toEqual([expect.stringContaining("Header accent is too loud")]);

    // Give the probe time to try everything, including the delayed top navigation.
    await sleep(2_500);
    await mkdir(SCREENSHOT_DIR, { recursive: true });
    await client.screenshot(join(SCREENSHOT_DIR, "desktop-artifact-viewer.png"));

    // The host is still the host: same document, one window, frame intact.
    expect(await client.executeSync<string>("return location.href")).not.toContain("example.com");
    expect(await client.getWindowHandles()).toHaveLength(1);
    expect(await client.executeSync<boolean>(
      `return Boolean(document.querySelector('[data-testid="artifact-frame"]'));`,
    )).toBe(true);

    // The anchor shows its file of the same tree, with the anchored line marked.
    await client.executeSync(`document.querySelector('[data-testid="artifact-anchor"]').click();`);
    await client.waitForElement('[data-testid="artifact-source"] .anchored', 10_000);
    expect(await client.executeSync<string>(
      `return document.querySelector('[data-testid="artifact-source"] .anchored').textContent;`,
    )).toContain("border-bottom: 4px solid #2563eb");
    await client.screenshot(join(SCREENSHOT_DIR, "desktop-artifact-anchor.png"));
  });
});
