import { resolve } from "node:path";
import { mkdir, writeFile } from "node:fs/promises";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { WebDriverClient } from "../helpers/webdriver";
import { resetDatabase, importTestRepo } from "../helpers/reset";
import { createFixtureRepo, cleanupFixtureRepos } from "../helpers/fixture-repo";
import { callVueMethod, execDb, queryDb } from "../helpers/vue";
import { assertNativeWindowIdentity, resolveExpectedNativeWindowIdentity } from "../helpers/windowIdentity";

// A real WebKit selector and real pinned-workflow write, with inventory only
// stubbed: this test must never start an agent or perform inference.
describe("OpenCode model selection before a stage post", () => {
  const client = new WebDriverClient();
  const taskId = "opencode-selector-fixture";
  const evidence = resolve("../../.tmp/handoff");
  let fixture = "";
  const workflow = {
    name: "model-selection-fixture",
    stages: [
      { name: "in progress", agent: "implement", prompt: "$TASK_PROMPT", policy: { transition: "manual" }, post: { name: "commit", agent: "commit", prompt: "Keep this post" } },
      { name: "review", agent: "implement", prompt: "Keep this review", policy: { transition: "manual" } },
    ],
  };
  beforeAll(async () => {
    await client.createSession({ dismissStartupShortcuts: false });
    const expected = await resolveExpectedNativeWindowIdentity(resolve("../.."));
    await assertNativeWindowIdentity(client, expected, "OpenCode selector");
    await mkdir(evidence, { recursive: true });
    await writeFile(resolve(evidence, "native-identity.json"), JSON.stringify({ expected, observedTitle: await client.getNativeWindowTitle(), build: await client.getAppBuildInfo(), endpoint: client.getBaseUrl() }, null, 2));
    await resetDatabase(client);
    fixture = await createFixtureRepo("opencode-model-selection");
    const repoId = await importTestRepo(client, fixture, "opencode-model-selection");
    await execDb(client, `INSERT INTO pipeline_item (id, repo_id, prompt, display_name, pipeline, pipeline_def, stage, agent_type, agent_provider, activity) VALUES (?, ?, ?, ?, ?, ?, 'in progress', 'pty', 'opencode', 'idle')`, [taskId, repoId, "Selector fixture", "Selector fixture", workflow.name, JSON.stringify(workflow)]);
    await client.executeSync(`const original = window.fetch.bind(window); window.fetch = (input, init) => String(typeof input === 'string' ? input : input.url).includes('/opencode-models') ? Promise.resolve(new Response(JSON.stringify([{id:'local/qwen',name:'Local fixture',local:true,connection:'http://127.0.0.1:5235',context:32768},{id:'cloud/coder',name:'Cloud fixture',local:false,connection:'https://provider.example',context:65536}]),{headers:{'Content-Type':'application/json'}})) : original(input, init);`);
    await callVueMethod(client, "refreshAllItems");
    await callVueMethod(client, "store.selectItem", taskId);
  });
  afterEach(async () => {
    await writeFile(resolve(evidence, "native-dom.txt"), await client.executeSync<string>("return document.body.innerText"));
    await client.screenshot(resolve(evidence, "native-model-selector.png"));
  });
  afterAll(async () => {
    try {
      if (fixture) await cleanupFixtureRepos([fixture]);
    } finally {
      await client.deleteSession();
    }
  });
  it("shows local metadata and saves a cloud choice without dispatching the post", async () => {
    await client.click(await client.waitForElement(".stage-model-control > button"));
    const input = await client.waitForElement('.stage-model-panel input[aria-label="OpenCode model"]');
    await client.sendKeys(input, "local/qwen");
    await client.waitForText(".stage-model-panel", "Local connection · http://127.0.0.1:5235");
    await client.waitForText(".stage-model-panel", "Server readiness has not been checked.");
    await client.clear(input);
    await client.sendKeys(input, "cloud/coder");
    await client.click(await client.findElement(".stage-model-panel > button"));
    await client.waitForText(".stage-model-panel", "Saved for review");
    const rows = await queryDb(client, "SELECT stage, pipeline_def FROM pipeline_item WHERE id = ?", [taskId]) as Array<{stage: string; pipeline_def: string}>;
    expect(rows[0]?.stage).toBe("in progress");
    const saved = JSON.parse(rows[0]?.pipeline_def ?? "null");
    expect(saved).toMatchObject({ ...workflow, stages: [workflow.stages[0], { ...workflow.stages[1], agent_provider: ["opencode-cloud/coder"] }] });
    // The server canonicalizes selectors; a second save must use its returned
    // snapshot as the concurrency fence rather than our original request.
    await client.clear(input);
    await client.sendKeys(input, "local/qwen");
    await client.click(await client.findElement(".stage-model-panel > button"));
    await client.waitForText(".stage-model-panel", "Saved for review");
    const changed = await queryDb(client, "SELECT pipeline_def FROM pipeline_item WHERE id = ?", [taskId]) as Array<{pipeline_def: string}>;
    expect(JSON.parse(changed[0]?.pipeline_def ?? "null").stages[1].agent_provider).toEqual(["opencode-local/qwen"]);
    expect(await queryDb(client, "SELECT id FROM stage_run WHERE task_id = ?", [taskId])).toEqual([]);
    await client.screenshot(resolve(evidence, "native-model-selector.png"));
  });
});
