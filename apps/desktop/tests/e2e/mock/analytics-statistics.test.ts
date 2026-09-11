import { setTimeout as sleep } from "node:timers/promises";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { buildGlobalKeydownScript } from "../helpers/keyboard";
import { WebDriverClient } from "../helpers/webdriver";
import { cleanupFixtureRepos, createSeedFixtureRepo } from "../helpers/fixture-repo";
import { cleanupWorktrees, importTestRepo, resetDatabase } from "../helpers/reset";
import { getVueState } from "../helpers/vue";

/**
 * Analytics reads statistics nothing else in the app reads: durable activity
 * spans, the revision record, pull-request facts, and token usage collected
 * from the agent CLIs' own session files. Every one of those crosses the
 * renderer → server → SQLite boundary and back, and the composable tests mock
 * exactly that boundary away. This drives the real one: rows written into the
 * real database, read by the real HTTP route, rendered by the real view.
 */

/** Inclusive `YYYY-MM-DD`, `daysAgo` days before today in UTC. */
function isoDaysAgo(daysAgo: number): string {
  const date = new Date();
  date.setUTCDate(date.getUTCDate() - daysAgo);
  return date.toISOString().slice(0, 10);
}

/** The SQLite timestamp spelling every row in this database uses. */
function timestampDaysAgo(daysAgo: number, time = "12:00:00"): string {
  return `${isoDaysAgo(daysAgo)} ${time}`;
}

/**
 * Read one statistic, failing with what the view actually rendered instead of
 * with an empty string — a missing tile and a zero look identical otherwise.
 */
async function statText(client: WebDriverClient, testId: string): Promise<string> {
  const found = await client.executeSync<string | null>(
    `const element = document.querySelector('[data-testid="${testId}"]');
     return element ? element.textContent.trim() : null;`,
  );
  if (found === null) {
    const rendered = await client.executeSync<string>(
      `const view = document.querySelector('[data-testid="analytics-view"]');
       return view ? view.textContent.replace(/\s+/g, " ").trim().slice(0, 600) : "no analytics view";`,
    );
    throw new Error(`no [data-testid="${testId}"] in the analytics view; it rendered: ${rendered}`);
  }
  return found;
}

/**
 * Wait for the statistics themselves, not merely for the view: the first read
 * of a repository also collects token usage from the provider session files,
 * so the tiles appear a moment after the frame does.
 */
async function waitForStatistics(client: WebDriverClient, timeoutMs = 30_000): Promise<void> {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const state = await client.executeSync<string>(
      `const view = document.querySelector('[data-testid="analytics-view"]');
       if (!view) return "absent";
       if (view.querySelector('[data-testid="analytics-tasks-created"]')) return "ready";
       if (view.querySelector('[data-testid="analytics-empty"]')) return "empty";
       const state = view.querySelector(".empty-state");
       return state ? state.textContent.trim() : "unknown";`,
    );
    if (state === "ready") return;
    if (state === "empty") throw new Error("analytics reported no activity for the seeded window");
    if (state === "absent") throw new Error("the analytics view left the DOM while loading");
    await sleep(200);
  }
  const requests = await client.executeSync<string>(
    `return JSON.stringify(window.__analyticsRequests || "fetch was never wrapped");`,
  );
  const views = await client.executeSync<string>(
    `return JSON.stringify(Array.from(document.querySelectorAll('[data-testid="analytics-view"]'))
      .map(function (view) {
        return {
          visible: view.offsetParent !== null,
          text: view.textContent.replace(/[ \\t\\n]+/g, " ").trim().slice(0, 300),
        };
      }));`,
  );
  console.log(`[analytics-e2e] views: ${views}`);
  throw new Error(
    `analytics never rendered its statistics within ${timeoutMs}ms; requests: ${requests}`,
  );
}

describe("analytics statistics", () => {
  const client = new WebDriverClient();
  let fixtureRepoRoot = "";
  let testRepoPath = "";

  beforeAll(async () => {
    await client.createSession();
    await resetDatabase(client);

    fixtureRepoRoot = await createSeedFixtureRepo("task-switch-minimal");
    testRepoPath = fixtureRepoRoot;
    await importTestRepo(client, testRepoPath, "analytics-test");

    const repoId = (await getVueState(client, "selectedRepoId")) as string;
    const seeded = await client.executeAsync<string>(
      `const cb = arguments[arguments.length - 1];
       const ctx = window.__KANNA_E2E__.setupState;
       const db = ctx.db.value || ctx.db;
       const repoId = "${repoId}";
       const statements = [
         ["INSERT INTO pipeline_item (id, repo_id, prompt, stage, branch, agent_type, activity, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
          ["task-a", repoId, "Analytics task A", "review", "task-a", "agent", "idle", "${timestampDaysAgo(20, "08:00:00")}", "${timestampDaysAgo(20, "08:00:00")}"]],
         ["INSERT INTO pipeline_item (id, repo_id, prompt, stage, branch, agent_type, activity, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
          ["task-b", repoId, "Analytics task B", "review", "task-b", "agent", "idle", "${timestampDaysAgo(18, "08:00:00")}", "${timestampDaysAgo(18, "08:00:00")}"]],
         // Two hours idle and one hour unread: Analytics counts both as
         // nobody servicing the task.
         ["INSERT INTO task_activity_interval (task_id, activity, started_at, ended_at) VALUES (?, ?, ?, ?)",
          ["task-a", "idle", "${timestampDaysAgo(20, "09:00:00")}", "${timestampDaysAgo(20, "11:00:00")}"]],
         ["INSERT INTO task_activity_interval (task_id, activity, started_at, ended_at) VALUES (?, ?, ?, ?)",
          ["task-a", "unread", "${timestampDaysAgo(20, "12:00:00")}", "${timestampDaysAgo(20, "13:00:00")}"]],
         // Both tasks reached review; only one was revised.
         ["INSERT INTO stage_run (id, task_id, stage, kind, status, agent_provider, started_at) VALUES (?, ?, ?, 'main', 'succeeded', 'claude', ?)",
          ["run-a", "task-a", "review", "${timestampDaysAgo(20, "14:00:00")}"]],
         ["INSERT INTO stage_run (id, task_id, stage, kind, status, agent_provider, started_at) VALUES (?, ?, ?, 'main', 'succeeded', 'claude', ?)",
          ["run-b", "task-b", "review", "${timestampDaysAgo(18, "14:00:00")}"]],
         ["INSERT INTO task_revision (task_id, origin, target_stage, applied, created_at) VALUES (?, 'agent', 'in progress', 1, ?)",
          ["task-a", "${timestampDaysAgo(20, "15:00:00")}"]],
         ["INSERT INTO task_pull_request (repo_id, pr_key, pr_number, pr_url, first_seen_at) VALUES (?, ?, ?, ?, ?)",
          [repoId, "github.com/owner/repo/pull/1", 1, "https://github.com/owner/repo/pull/1", "${timestampDaysAgo(18, "16:00:00")}"]],
         ["INSERT INTO provider_token_usage (usage_key, provider, repo_id, task_id, run_id, model, occurred_at, input_tokens, cached_input_tokens, cache_creation_tokens, output_tokens, reasoning_tokens, total_tokens) VALUES (?, 'claude', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
          ["claude:message:msg_e2e", repoId, "task-a", "run-a", "claude-opus-5", "${timestampDaysAgo(20, "14:30:00")}", 1000, 9000, 500, 2000, 100, 12500]],
       ];
       statements.reduce(function (chain, statement) {
         return chain.then(function () { return db.execute(statement[0], statement[1]); });
       }, Promise.resolve())
         .then(function () { return ctx.loadItems(repoId); })
         .then(function () { cb("ok"); })
         .catch(function (e) { cb("err:" + (e && e.message ? e.message : String(e))); });`,
    );
    if (typeof seeded === "string" && seeded.startsWith("err:")) {
      throw new Error(`seeding analytics rows failed: ${seeded.slice(4)}`);
    }
    // Let the seeded tasks reach the sidebar before anything opens a view:
    // Analytics is one tab per scope, and the app drops it when the scope
    // changes underneath it.
    await client.waitForText(".sidebar", "Analytics task A");
    await client.waitForText(".sidebar", "Analytics task B");

    // Record what the view actually asks the server for. A tile that never
    // arrives is either a request that was never made, one the server
    // refused, or one nobody awaited — and the rendered frame looks the same
    // in all three cases.
    await client.executeSync(
      `window.__analyticsRequests = [];
       const original = window.fetch;
       window.fetch = function (input, init) {
         const url = String(input && input.url ? input.url : input);
         if (url.indexOf("/v1/analytics/") === -1) return original.call(this, input, init);
         const entry = { url: url, state: "pending" };
         window.__analyticsRequests.push(entry);
         return original.call(this, input, init).then(
           function (response) { entry.state = "status " + response.status; return response; },
           function (error) { entry.state = "rejected " + error; throw error; }
         );
       };
       return true;`,
    );

  });

  /**
   * Analytics is one tab per scope, so the app drops it whenever the scope
   * changes underneath it. Each step re-opens it rather than assuming the one
   * an earlier step left behind is still mounted.
   */
  async function ensureAnalyticsOpen(): Promise<void> {
    const present = await client.executeSync<boolean>(
      `return !!document.querySelector('[data-testid="analytics-view"]');`,
    );
    if (!present) {
      // ⇧⌘A is the app's own way in, so the test opens the view the way a
      // person does rather than mounting the component itself.
      await client.executeSync(buildGlobalKeydownScript({ key: "A", meta: true, shift: true }));
      await client.waitForElement('[data-testid="analytics-view"]');
    }
    await waitForStatistics(client);
  }

  beforeEach(ensureAnalyticsOpen);

  afterAll(async () => {
    if (testRepoPath) {
      await cleanupWorktrees(client, testRepoPath);
    }
    await cleanupFixtureRepos(fixtureRepoRoot ? [fixtureRepoRoot] : []);
    await client.deleteSession();
  });

  async function selectRange(preset: "7d" | "30d"): Promise<void> {
    await ensureAnalyticsOpen();
    await client.executeSync(
      `document.querySelector('[data-testid="analytics-range-${preset}"]').click(); return true;`,
    );
    await waitForStatistics(client);
  }

  it("counts only what the chosen window contains", async () => {
    // The seeded history is ~20 days old, so what the thirty-day window holds
    // and the seven-day one does not is exactly this test's own rows —
    // whatever else the harness has created today.
    await selectRange("30d");
    const createdIn30 = Number(await statText(client, "analytics-tasks-created"));
    const pullRequestsIn30 = Number(await statText(client, "analytics-pr-created"));
    const openNow = Number(await statText(client, "analytics-tasks-open"));

    await selectRange("7d");
    const createdIn7 = Number(await statText(client, "analytics-tasks-created"));
    const pullRequestsIn7 = Number(await statText(client, "analytics-pr-created"));

    expect(createdIn30 - createdIn7).toBe(2);
    expect(pullRequestsIn30 - pullRequestsIn7).toBe(1);
    // The backlog is "right now", so a narrower window does not shrink it.
    expect(Number(await statText(client, "analytics-tasks-open"))).toBe(openNow);
    expect(openNow).toBeGreaterThanOrEqual(2);

    await selectRange("30d");
  });

  it("counts idle and unread together as waiting", async () => {
    // Two hours idle plus one hour unread.
    expect(await statText(client, "analytics-idle-total")).toContain("3h");
  });

  it("says merge state is unconfirmed rather than reporting zero merges", async () => {
    // The fixture repository has no forge to ask, which is exactly the case
    // that must not read as "nothing merged".
    const merged = await client.executeSync<string>(
      `const stats = Array.from(document.querySelectorAll('[data-testid="analytics-view"] .stat'));
       const card = stats.find(function (stat) {
         const label = stat.querySelector(".stat-label");
         return label && /merged/i.test(label.textContent || "");
       });
       return card ? card.querySelector(".stat-value").textContent.trim() : "missing";`,
    );
    expect(merged).toBe("—");
  });

  it("opens a statistic into the tasks that produced it", async () => {
    await client.executeSync(
      `document.querySelector('[data-testid="analytics-idle-total"]').click(); return true;`,
    );
    await client.waitForElement('[data-testid="analytics-drilldown"]');
    const rows = await client.executeSync<string[]>(
      `return Array.from(document.querySelectorAll('[data-testid="analytics-drilldown"] .drilldown-title'))
        .map(function (row) { return row.textContent.trim(); });`,
    );
    expect(rows).toContain("Analytics task A");
  });

});
