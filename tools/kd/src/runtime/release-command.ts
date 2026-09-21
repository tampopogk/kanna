import { join } from "node:path";
import type { CommandRunner } from "./process";

export type ReleaseEnvironment = "production" | "staging";

/**
 * The git/GitHub facts every release command needs. `ReleaseShipInput`,
 * `ReleaseStatusInput`, and `ReleaseResetStagingInput` all satisfy it, so the
 * lineage and soak helpers run identically from ship, status, and promote.
 */
export interface ReleaseCommandContext {
  repoRoot: string;
  env: NodeJS.ProcessEnv;
  runner: CommandRunner;
}

export function releaseEnvironment(input: ReleaseEnvironment | undefined): ReleaseEnvironment {
  return input ?? "production";
}

export function releaseOutputDir(repoRoot: string, environment: ReleaseEnvironment): string {
  return environment === "staging" ? join(repoRoot, ".build", "release", "staging") : join(repoRoot, ".build", "release");
}

export function releaseRepoSlug(remoteUrl: string): string {
  let normalized = remoteUrl.trim();
  if (normalized.startsWith("git@github.com:")) normalized = normalized.slice("git@github.com:".length);
  else if (normalized.startsWith("ssh://git@github.com/")) normalized = normalized.slice("ssh://git@github.com/".length);
  else if (normalized.startsWith("https://github.com/")) normalized = normalized.slice("https://github.com/".length);
  else throw new Error(`Unsupported GitHub remote URL: ${remoteUrl}`);
  return normalized.replace(/\.git$/, "");
}

export function stagingTag(version: string): string {
  return version.startsWith("v") ? version : `v${version}`;
}

export async function mustRun(runner: CommandRunner, command: string, args: string[], cwd: string, env?: NodeJS.ProcessEnv, stdin?: string): Promise<string> {
  const result = await runner.run(command, args, { cwd, env, ...(stdin === undefined ? {} : { stdin }) });
  if (result.exitCode !== 0) {
    throw new Error(result.stderr || result.stdout || `${command} ${args.join(" ")} failed`);
  }
  return result.stdout.trim();
}

export async function assertCleanGitWorktree(repoRoot: string, runner: CommandRunner, env?: NodeJS.ProcessEnv): Promise<void> {
  const status = await mustRun(runner, "git", ["status", "--porcelain"], repoRoot, env);
  if (status.trim().length > 0) {
    throw new Error(
      "Refusing to ship a release from a dirty git worktree. Commit or stash changes first."
    );
  }
}
