import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  REMOTE_E2E_LANE_ID,
  buildRemoteE2eLaneArgs,
  executeRemoteE2e,
  matchRemoteE2eTriggerPaths,
  remoteE2eTriggerPaths,
  selectRemoteE2eByChangedPaths,
} from "../src/runtime/remote-e2e";
import { LANE_MANIFEST_PATH, laneTriggerPaths } from "../src/runtime/lane-inventory";
import type { CommandResult, CommandRunner } from "../src/runtime/process";

/**
 * The trigger prefixes live in `docs/verification/lanes.json`, so these tests
 * run against the real repository root: the selection reads the manifest and
 * only git is a fixture. The previous shape asserted the module's own literal
 * against the same literal, which nothing in the repository could break.
 */
const repoRoot = resolve(import.meta.dirname, "..", "..", "..");

interface RecordedCall {
  command: string;
  args: string[];
}

function gitRunner(
  responses: Record<string, Partial<CommandResult>>,
  calls: RecordedCall[]
): CommandRunner {
  return {
    async run(command, args) {
      calls.push({ command, args });
      const key = `${command} ${args.join(" ")}`;
      const response = responses[key];
      if (!response) {
        return { exitCode: 1, stdout: "", stderr: `unexpected command: ${key}` };
      }
      return { exitCode: 0, stdout: "", stderr: "", ...response };
    },
  };
}

const ORIGIN_HEAD = "git symbolic-ref --short refs/remotes/origin/HEAD";
const MERGE_BASE = "git merge-base origin/main HEAD";
const DIFF = "git diff --name-only abc123";
const UNTRACKED = "git ls-files --others --exclude-standard";

function gitResponses(diff: string, untracked = ""): Record<string, Partial<CommandResult>> {
  return {
    [ORIGIN_HEAD]: { stdout: "origin/main\n" },
    [MERGE_BASE]: { stdout: "abc123\n" },
    [DIFF]: { stdout: diff },
    [UNTRACKED]: { stdout: untracked },
  };
}

const DEV_LANE_OPTIONS = {
  staging: false,
  mobileRelay: false,
  desktopPairing: false,
  ifChanged: true,
};

describe("remote E2E trigger paths", () => {
  it("keeps the deleted remote-e2e.yml path filter in the lane manifest", () => {
    expect(remoteE2eTriggerPaths(repoRoot)).toEqual([
      "services/relay/",
      "crates/kanna-server/",
      "services/firebase-functions/",
      "apps/mobile/src/lib/",
      "tests/remote-e2e/",
      "tools/kd/",
    ]);
  });

  /**
   * Two copies of the same answer is two things to keep in step, and the
   * manifest is the one that the inventory test holds to repository state.
   */
  it("keeps no second copy of the prefixes beside the manifest", () => {
    const module = readFileSync(resolve(repoRoot, "tools/kd/src/runtime/remote-e2e.ts"), "utf8");
    for (const prefix of remoteE2eTriggerPaths(repoRoot)) {
      expect(module, `${prefix} is declared in two places`).not.toContain(`"${prefix}"`);
    }
    expect(remoteE2eTriggerPaths(repoRoot)).toEqual(
      laneTriggerPaths(repoRoot, REMOTE_E2E_LANE_ID),
    );
  });

  /**
   * An empty prefix list answers "not required" for every branch — a gate that
   * has silently detached, which is the failure the manifest exists to remove.
   * It must be an error, not a quiet pass.
   */
  it("refuses a lane the manifest does not declare", () => {
    expect(() => laneTriggerPaths(repoRoot, "no-such-lane")).toThrow(LANE_MANIFEST_PATH);
  });

  it("matches by path prefix without matching sibling directories", () => {
    expect(
      matchRemoteE2eTriggerPaths(
        [
          "crates/kanna-server/src/mobile_api.rs",
          "crates/kanna-daemon/src/lib.rs",
          "apps/mobile/src/lib/transports/relayClient.ts",
          "apps/mobile/src/screens/TaskList.tsx",
          "docs/specs/remote-task-e2e.md",
        ],
        remoteE2eTriggerPaths(repoRoot)
      )
    ).toEqual([
      "crates/kanna-server/src/mobile_api.rs",
      "apps/mobile/src/lib/transports/relayClient.ts",
    ]);
  });
});

describe("kd test remote-e2e --if-changed", () => {
  it("reports the branch changed paths against the default branch merge-base", async () => {
    const calls: RecordedCall[] = [];
    const runner = gitRunner(
      gitResponses("services/relay/src/index.ts\nREADME.md\n", "tests/remote-e2e/src/new.ts\n"),
      calls
    );

    const selection = await selectRemoteE2eByChangedPaths({
      repoRoot,
      env: {},
      runner,
    });

    expect(selection).toEqual({
      required: true,
      defaultBranchRef: "origin/main",
      mergeBase: "abc123",
      changedPaths: ["README.md", "services/relay/src/index.ts", "tests/remote-e2e/src/new.ts"],
      matchedPaths: ["services/relay/src/index.ts", "tests/remote-e2e/src/new.ts"],
      triggerPaths: remoteE2eTriggerPaths(repoRoot),
    });
    expect(calls.map((call) => `${call.command} ${call.args.join(" ")}`)).toEqual([
      ORIGIN_HEAD,
      MERGE_BASE,
      DIFF,
      UNTRACKED,
    ]);
  });

  it("falls back to a known default branch ref when origin/HEAD is unset", async () => {
    const calls: RecordedCall[] = [];
    const runner = gitRunner(
      {
        [ORIGIN_HEAD]: { exitCode: 128, stderr: "ref refs/remotes/origin/HEAD is not a symbolic ref" },
        "git rev-parse --verify --quiet origin/main": { stdout: "deadbeef\n" },
        [MERGE_BASE]: { stdout: "abc123\n" },
        [DIFF]: { stdout: "docs/specs/remote-task-e2e.md\n" },
        [UNTRACKED]: { stdout: "" },
      },
      calls
    );

    const selection = await selectRemoteE2eByChangedPaths({
      repoRoot,
      env: {},
      runner,
    });

    expect(selection.defaultBranchRef).toBe("origin/main");
    expect(selection.required).toBe(false);
  });

  it("runs the unchanged dev lane when a trigger path changed", async () => {
    const calls: RecordedCall[] = [];
    const responses = {
      ...gitResponses("crates/kanna-server/src/mobile_api.rs\n"),
      "pnpm --dir tests/remote-e2e exec tsx src/run.ts --dev": { stdout: "remote e2e passed" },
    };

    const result = await executeRemoteE2e({
      repoRoot,
      env: { KANNA_DEV_PORT: "1421" },
      runner: gitRunner(responses, calls),
      options: DEV_LANE_OPTIONS,
    });

    expect(calls.filter((call) => call.command !== "git")).toEqual([
      { command: "pnpm", args: buildRemoteE2eLaneArgs(DEV_LANE_OPTIONS) },
    ]);
    expect(result).toEqual({
      ok: true,
      message: "remote e2e passed",
      data: {
        command: "pnpm",
        args: ["--dir", "tests/remote-e2e", "exec", "tsx", "src/run.ts", "--dev"],
        exitCode: 0,
      },
    });
  });

  it("selects the focused mobile terminal-control relay lane", () => {
    expect(buildRemoteE2eLaneArgs({
      staging: false, mobileRelay: false, mobileRelayTerminalControl: true, desktopPairing: false,
    })).toEqual([
      "--dir", "tests/remote-e2e", "exec", "tsx", "src/run.ts", "--dev", "--mobile-relay-terminal-control",
    ]);
  });

  it("keeps the extra Layer C and Layer D lanes when a trigger path changed", async () => {
    const calls: RecordedCall[] = [];
    const options = { ...DEV_LANE_OPTIONS, mobileRelay: true, desktopPairing: true };
    const responses = {
      ...gitResponses("tools/kd/src/runtime/remote-e2e.ts\n"),
      "pnpm --dir tests/remote-e2e exec tsx src/run.ts --dev --mobile-relay --desktop-pairing": {
        stdout: "ok",
      },
    };

    const result = await executeRemoteE2e({
      repoRoot,
      env: {},
      runner: gitRunner(responses, calls),
      options,
    });

    expect(result.ok).toBe(true);
    expect(calls.filter((call) => call.command !== "git")).toEqual([
      { command: "pnpm", args: buildRemoteE2eLaneArgs(options) },
    ]);
  });

  it("does no emulator or test work when no trigger path changed", async () => {
    const calls: RecordedCall[] = [];
    const runner = gitRunner(
      gitResponses("docs/specs/remote-task-e2e.md\napps/desktop/src/App.vue\n"),
      calls
    );

    const result = await executeRemoteE2e({
      repoRoot,
      env: {},
      runner,
      options: DEV_LANE_OPTIONS,
    });

    expect(result.ok).toBe(true);
    expect(result.message).toContain("remote E2E not required for this branch");
    expect(calls.every((call) => call.command === "git")).toBe(true);
    expect(calls.map((call) => `${call.command} ${call.args.join(" ")}`)).toEqual([
      ORIGIN_HEAD,
      MERGE_BASE,
      DIFF,
      UNTRACKED,
    ]);
  });

  it("runs the lane unconditionally when --if-changed is absent", async () => {
    const calls: RecordedCall[] = [];
    const runner = gitRunner(
      { "pnpm --dir tests/remote-e2e exec tsx src/run.ts --dev": { stdout: "ok" } },
      calls
    );

    const result = await executeRemoteE2e({
      repoRoot,
      env: {},
      runner,
      options: { ...DEV_LANE_OPTIONS, ifChanged: false },
    });

    expect(result.ok).toBe(true);
    expect(calls).toEqual([
      { command: "pnpm", args: ["--dir", "tests/remote-e2e", "exec", "tsx", "src/run.ts", "--dev"] },
    ]);
  });

  it("refuses to gate the staging lane", async () => {
    const calls: RecordedCall[] = [];
    const result = await executeRemoteE2e({
      repoRoot,
      env: {},
      runner: gitRunner({}, calls),
      options: { ...DEV_LANE_OPTIONS, staging: true },
    });

    expect(result.ok).toBe(false);
    expect(result.message).toBe("remote-e2e --if-changed applies to the dev lane only.");
    expect(calls).toEqual([]);
  });

  it("propagates a failing lane exit code", async () => {
    const calls: RecordedCall[] = [];
    const responses = {
      ...gitResponses("services/firebase-functions/src/index.ts\n"),
      "pnpm --dir tests/remote-e2e exec tsx src/run.ts --dev": {
        exitCode: 1,
        stderr: "remote e2e failed",
      },
    };

    const result = await executeRemoteE2e({
      repoRoot,
      env: {},
      runner: gitRunner(responses, calls),
      options: DEV_LANE_OPTIONS,
    });

    expect(result).toEqual({
      ok: false,
      message: "remote e2e failed",
      data: {
        command: "pnpm",
        args: ["--dir", "tests/remote-e2e", "exec", "tsx", "src/run.ts", "--dev"],
        exitCode: 1,
      },
    });
  });
});
