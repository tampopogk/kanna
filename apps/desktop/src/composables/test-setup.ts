// Test preload: set up happy-dom globals for composable tests that need DOM APIs.
import { Window } from "happy-dom";

const win = new Window();
// @ts-ignore
globalThis.document = win.document;
// @ts-ignore
globalThis.window = win;
// @ts-ignore
globalThis.localStorage = win.localStorage;
// @ts-ignore — use happy-dom's Event so dispatchEvent instanceof check passes
globalThis.Event = win.Event;

// This suite's keyboard expectations are macOS's: ⌘ bindings and ⌘ glyphs, the
// platform the app shipped on first. Say so rather than inheriting whatever
// happy-dom reports, which would silently switch every shortcut assertion to
// the Linux mapping. That mapping has its own explicit coverage in
// `shortcutPlatform.test.ts`, which asks for each platform by name.
for (const nav of [globalThis.navigator, win.navigator]) {
  if (nav) Object.defineProperty(nav, "platform", { value: "MacIntel", configurable: true });
}

import {
  setDesktopServerClientHandlersForTests,
  setDesktopSnapshotFetcherForTests,
} from "../services/desktopServerClient";

setDesktopSnapshotFetcherForTests(async () => ({
  entries: [],
  taskBlockers: [],
  worktreePaths: {},
  settings: {},
}));

setDesktopServerClientHandlersForTests({
  getSetting: async () => null,
  putSetting: async (key, value) => ({ key, value }),
  deleteSetting: async () => {},
  postOperatorEvents: async () => {},
  fetchRepoAnalytics: async (_repoId, range) => ({
    range: range ?? { from: "2026-09-01", to: "2026-09-30" },
    coverage: {
      idleSince: null,
      revisionsSince: null,
      tokensSince: null,
      pullRequestStateConfirmed: false,
      providersWithoutTokenUsage: [],
      runsWithTokenUsage: 0,
      runsInRange: 0,
    },
    tasks: { created: 0, closed: 0, openNow: 0, childTasksCreated: 0 },
    pullRequests: { created: 0, merged: null, openNow: null },
    idle: {
      totalSeconds: 0,
      workingSeconds: 0,
      taskCount: 0,
      averageSecondsPerTask: 0,
      longestSeconds: 0,
      contributors: [],
    },
    revisions: {
      cohortTasks: 0,
      totalRevisions: 0,
      averagePerTask: 0,
      cleanPassRate: null,
      parkedRequests: 0,
      contributors: [],
    },
    tokens: {
      total: { input: 0, cachedInput: 0, cacheCreation: 0, reasoning: 0, output: 0, total: 0 },
      byModel: [],
      byTask: [],
    },
  }),
  fetchRepoAgentProviders: async () => ["claude", "copilot", "codex", "opencode", "antigravity"],
  patchRepo: async () => {},
  fetchClosedTaskIdentities: async () => [],
});
