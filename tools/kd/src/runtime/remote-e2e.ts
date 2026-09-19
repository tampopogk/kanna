import { laneTriggerPaths, matchTriggerPaths } from "./lane-inventory";
import type { CommandRunner } from "./process";

/**
 * The lane whose trigger paths gate `--if-changed`.
 *
 * The prefixes themselves were recovered from the pull-request `paths:` filter
 * of the deleted `.github/workflows/remote-e2e.yml` and lived here as a
 * constant. They now live in `docs/verification/lanes.json`, which declares
 * `triggerPaths` for every lane, because two copies of the same answer is two
 * things to keep in step — and because a constant asserted against its own
 * literal is the anti-pattern the lane manifest exists to remove. Update the
 * manifest when the remote surface moves; nothing here needs to change.
 */
export const REMOTE_E2E_LANE_ID = "remote-e2e-dev";

/** The `remote-e2e-dev` lane's trigger prefixes, read from the manifest. */
export function remoteE2eTriggerPaths(repoRoot: string): string[] {
  return laneTriggerPaths(repoRoot, REMOTE_E2E_LANE_ID);
}

/**
 * Remote-tracking refs tried in order when `refs/remotes/origin/HEAD` is not
 * set locally (a fresh clone with `--single-branch`, or a worktree whose origin
 * HEAD was never fetched).
 */
const DEFAULT_BRANCH_FALLBACK_REFS = ["origin/main", "main", "origin/master", "master"] as const;

export interface RemoteE2eGitInput {
  repoRoot: string;
  env: NodeJS.ProcessEnv;
  runner: CommandRunner;
}

export interface RemoteE2eSelection {
  required: boolean;
  defaultBranchRef: string;
  mergeBase: string;
  changedPaths: string[];
  matchedPaths: string[];
  /** The prefixes the selection was made against, as the manifest declared them. */
  triggerPaths: string[];
}

export interface RemoteE2eOptions {
  staging: boolean;
  mobileRelay: boolean;
  mobileRelayTerminalControl?: boolean;
  desktopPairing: boolean;
  ifChanged: boolean;
}

export function matchRemoteE2eTriggerPaths(
  changedPaths: string[],
  triggerPaths: readonly string[]
): string[] {
  return matchTriggerPaths(changedPaths, triggerPaths);
}

async function runGit(input: RemoteE2eGitInput, args: string[]) {
  return input.runner.run("git", args, { cwd: input.repoRoot, env: input.env });
}

export async function resolveDefaultBranchRef(input: RemoteE2eGitInput): Promise<string> {
  const originHead = await runGit(input, ["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]);
  const originHeadRef = originHead.exitCode === 0 ? originHead.stdout.trim() : "";
  if (originHeadRef) return originHeadRef;

  for (const candidate of DEFAULT_BRANCH_FALLBACK_REFS) {
    const verified = await runGit(input, ["rev-parse", "--verify", "--quiet", candidate]);
    if (verified.exitCode === 0 && verified.stdout.trim()) return candidate;
  }
  throw new Error(
    "could not resolve the repo default branch: neither refs/remotes/origin/HEAD nor " +
      `${DEFAULT_BRANCH_FALLBACK_REFS.join(", ")} resolve`
  );
}

function splitPaths(stdout: string): string[] {
  return stdout
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
}

/**
 * Changed paths for this branch: everything between the merge-base with the
 * default branch and the working tree, plus untracked files. Uncommitted work
 * counts because a stage agent is normally still dirty when it decides which
 * lanes to run.
 */
export async function readBranchChangedPaths(
  input: RemoteE2eGitInput
): Promise<{ defaultBranchRef: string; mergeBase: string; changedPaths: string[] }> {
  const defaultBranchRef = await resolveDefaultBranchRef(input);

  const mergeBaseResult = await runGit(input, ["merge-base", defaultBranchRef, "HEAD"]);
  const mergeBase = mergeBaseResult.stdout.trim();
  if (mergeBaseResult.exitCode !== 0 || !mergeBase) {
    throw new Error(
      mergeBaseResult.stderr.trim() || `git merge-base ${defaultBranchRef} HEAD failed`
    );
  }

  const diff = await runGit(input, ["diff", "--name-only", mergeBase]);
  if (diff.exitCode !== 0) {
    throw new Error(diff.stderr.trim() || `git diff --name-only ${mergeBase} failed`);
  }

  const untracked = await runGit(input, ["ls-files", "--others", "--exclude-standard"]);
  if (untracked.exitCode !== 0) {
    throw new Error(untracked.stderr.trim() || "git ls-files --others --exclude-standard failed");
  }

  const changedPaths = [
    ...new Set([...splitPaths(diff.stdout), ...splitPaths(untracked.stdout)]),
  ].sort();
  return { defaultBranchRef, mergeBase, changedPaths };
}

export async function selectRemoteE2eByChangedPaths(
  input: RemoteE2eGitInput
): Promise<RemoteE2eSelection> {
  const triggerPaths = remoteE2eTriggerPaths(input.repoRoot);
  const { defaultBranchRef, mergeBase, changedPaths } = await readBranchChangedPaths(input);
  const matchedPaths = matchRemoteE2eTriggerPaths(changedPaths, triggerPaths);
  return {
    required: matchedPaths.length > 0,
    defaultBranchRef,
    mergeBase,
    changedPaths,
    matchedPaths,
    triggerPaths,
  };
}

export function buildRemoteE2eLaneArgs(
  options: Pick<RemoteE2eOptions, "staging" | "mobileRelay" | "mobileRelayTerminalControl" | "desktopPairing">
): string[] {
  const args = [
    "--dir",
    "tests/remote-e2e",
    "exec",
    "tsx",
    "src/run.ts",
    options.staging ? "--staging" : "--dev",
  ];
  if (options.mobileRelay) args.push("--mobile-relay");
  if (options.mobileRelayTerminalControl) args.push("--mobile-relay-terminal-control");
  if (options.desktopPairing) args.push("--desktop-pairing");
  return args;
}

/**
 * Runs the remote E2E lane, optionally gated on the branch touching the trigger
 * paths. When `ifChanged` is set and nothing matches, no lane command runs at
 * all — no emulators, no relay, no tests.
 */
export async function executeRemoteE2e(input: {
  repoRoot: string;
  env: NodeJS.ProcessEnv;
  runner: CommandRunner;
  options: RemoteE2eOptions;
}) {
  const { options } = input;
  if (options.ifChanged && options.staging) {
    return {
      ok: false,
      message: "remote-e2e --if-changed applies to the dev lane only.",
      data: {},
    };
  }

  if (options.ifChanged) {
    const selection = await selectRemoteE2eByChangedPaths(input);
    if (!selection.required) {
      return {
        ok: true,
        message:
          "remote E2E not required for this branch: no change since " +
          `${selection.defaultBranchRef} touches ${selection.triggerPaths.join(", ")}.`,
        data: { selection },
      };
    }
  }

  const args = buildRemoteE2eLaneArgs(options);
  const result = await input.runner.run("pnpm", args, {
    cwd: input.repoRoot,
    env: input.env,
  });
  return {
    ok: result.exitCode === 0,
    message:
      result.exitCode === 0
        ? result.stdout || `pnpm ${args.join(" ")} completed.`
        : result.stderr || result.stdout,
    data: { command: "pnpm", args, exitCode: result.exitCode },
  };
}
