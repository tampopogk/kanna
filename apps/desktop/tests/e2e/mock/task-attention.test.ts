import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { beforeAll, afterAll, describe, expect, it } from "vitest";
import { WebDriverClient } from "../helpers/webdriver";
import { resetDatabase, importTestRepo } from "../helpers/reset";
import { createFixtureRepo, cleanupFixtureRepos } from "../helpers/fixture-repo";
import { callVueMethod, execDb } from "../helpers/vue";
import { dismissStartupShortcutsModal } from "../helpers/startupOverlays";
import { resolveAppKannaServer } from "../helpers/kannaServer";
import { assertNativeWindowIdentity, resolveExpectedNativeWindowIdentity } from "../helpers/windowIdentity";

const execFileAsync = promisify(execFile);
const root = resolve("../..");
const evidence = resolve(root, ".tmp/attention-native");
const client = new WebDriverClient();
let fixture = "";
let repoId = "";
let serverUrl = "";

async function verifyIdentity() {
  await assertNativeWindowIdentity(client, await resolveExpectedNativeWindowIdentity(root), "attention badge");
}
async function tool(name: string, args: Record<string, string>) {
  const { stdout } = await execFileAsync(resolve(root, ".build/debug/kanna-cli"), ["tool", "call", name, "--json", JSON.stringify(args)], {
    cwd: root,
    env: { ...process.env, KANNA_SERVER_BASE_URL: serverUrl },
  });
  return JSON.parse(stdout);
}

describe("task attention through the catalog-backed CLI", () => {
  beforeAll(async () => {
    await client.createSession({ dismissStartupShortcuts: false });
    await verifyIdentity();
    await mkdir(evidence, { recursive: true });
    await writeFile(resolve(evidence, "identity.json"), JSON.stringify({ title: await client.getNativeWindowTitle(), build: await client.getAppBuildInfo(), endpoint: client.getBaseUrl() }, null, 2));
    await resetDatabase(client);
    fixture = await createFixtureRepo("attention");
    repoId = await importTestRepo(client, fixture, "Attention");
    serverUrl = (await resolveAppKannaServer(client)).baseUrl;
    for (const id of ["attention-fixture", "ordinary-fixture"]) {
      await execDb(client, `INSERT INTO pipeline_item (id, repo_id, prompt, display_name, pipeline, stage, branch, agent_type, agent_provider, activity) VALUES (?, ?, ?, ?, 'no-review', 'in progress', ?, 'pty', 'codex', 'idle')`, [id, repoId, id, id, `task-${id}`]);
    }
    await callVueMethod(client, "refreshRepos");
    await client.waitForElement('[data-task-id="attention-fixture"]');
  });
  afterAll(async () => {
    await cleanupFixtureRepos(fixture ? [fixture] : []);
    await client.deleteSession();
  });
  it("updates without refresh, survives reading/reload, and clears without reordering", async () => {
    const rows = () => client.executeSync<string[]>('return Array.from(document.querySelectorAll(".sidebar [data-task-id]")).map(el => el.dataset.taskId)');
    const before = await rows();
    const set = await tool("kanna_set_task_attention", { task_id: "attention-fixture" });
    expect(set.changed).toBe(true);
    await client.waitForElement('[data-task-id="attention-fixture"] .task-attention-marker');
    await client.click(await client.findElement('.workflow-item[data-task-id="attention-fixture"]'));
    await client.waitForElement('.workflow-item.selected[data-task-id="attention-fixture"]');
    expect(await client.executeSync('return document.querySelector(".task-attention-marker").getAttribute("aria-label")')).toBe("Agent requests attention");
    await client.screenshot(resolve(evidence, "selected-badge.png"));
    expect(await rows()).toEqual(before);
    await client.reload({ dismissStartupShortcuts: false });
    await verifyIdentity();
    await dismissStartupShortcutsModal(client);
    await client.waitForElement('[data-task-id="attention-fixture"] .task-attention-marker');
    await client.click(await client.findElement('.workflow-item[data-task-id="ordinary-fixture"]'));
    await client.waitForElement('.workflow-item.selected[data-task-id="ordinary-fixture"]');
    expect(await rows()).toEqual(before);
    const clear = await tool("kanna_clear_task_attention", { task_id: "attention-fixture" });
    expect(clear.attentionRequested).toBe(false);
    await client.waitForNoElement(".task-attention-marker");
    await client.screenshot(resolve(evidence, "cleared-badge.png"));
    expect(await rows()).toEqual(before);
  });
});
