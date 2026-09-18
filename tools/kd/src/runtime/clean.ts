import { lstatSync, rmSync } from "node:fs";
import { appCacheDir } from "../context";
import { homedir } from "node:os";
import { isAbsolute, join, parse, resolve } from "node:path";
import type { CommandResult, CommandRunner } from "./process";
import {
  EXTERNAL_WORKSPACE_BUILD_RECORD,
  resolveExternalWorkspaceBuild,
  WORKSPACE_BUILD_DIRECTORY
} from "./workspace-build";

export interface CleanInput {
  repoRoot: string;
  homeDir?: string;
  env?: NodeJS.ProcessEnv;
  runner: CommandRunner;
  all: boolean;
  dry: boolean;
  sharedRustBuild: boolean;
}

export type CleanOutcome = "removed" | "would-remove" | "absent" | "failed";

export interface CleanRemoval {
  path: string;
  outcome: CleanOutcome;
  error?: string;
}

export interface CleanResult {
  removals: CleanRemoval[];
  /** Absent when `bazel info output_base` could not be resolved; see the "failed" removal for why. */
  bazelOutputBase?: string;
}

export async function resolveBazelOutputBase(input: {
  repoRoot: string;
  env?: NodeJS.ProcessEnv;
  runner: CommandRunner;
}): Promise<string> {
  let result: CommandResult;
  try {
    result = await input.runner.run("bazel", ["info", "output_base"], {
      cwd: input.repoRoot,
      env: input.env
    });
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(`[kd] Cannot resolve Bazel output base: \`bazel info output_base\` could not run (${detail}).`);
  }

  if (result.exitCode !== 0) {
    const detail = result.stderr.trim() || result.stdout.trim() || `exit code ${result.exitCode}`;
    throw new Error(`[kd] Cannot resolve Bazel output base: \`bazel info output_base\` failed (${detail}).`);
  }

  const lines = result.stdout
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  if (lines.length === 0) {
    throw new Error("[kd] Cannot resolve Bazel output base: `bazel info output_base` returned no path.");
  }
  if (lines.length !== 1 || !isAbsolute(lines[0] ?? "")) {
    throw new Error("[kd] Cannot resolve Bazel output base: `bazel info output_base` did not return one absolute path.");
  }

  const outputBase = resolve(lines[0] ?? "");
  if (outputBase === parse(outputBase).root || outputBase === resolve(input.repoRoot)) {
    throw new Error(`[kd] Refusing to clean unsafe Bazel output base ${outputBase}.`);
  }
  return outputBase;
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function removePath(path: string, dry: boolean, requirePresent = false): CleanRemoval {
  try {
    lstatSync(path);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") {
      return { path, outcome: "failed", error: describeError(error) };
    }
    if (requirePresent) {
      return { path, outcome: "failed", error: "the resolved target became unavailable" };
    }
    return { path, outcome: "absent" };
  }
  if (dry) {
    return { path, outcome: "would-remove" };
  }
  try {
    rmSync(path, { recursive: true, force: !requirePresent });
  } catch (error) {
    return { path, outcome: "failed", error: describeError(error) };
  }
  return { path, outcome: "removed" };
}

/**
 * Every candidate is removed independently: a failure resolving or deleting
 * one (a full disk, an unreachable Bazel daemon, a permissions error) is
 * reported and never stops the rest of the sweep, since `kd clean --all`
 * runs unattended as the second half of repo teardown.
 */
export async function cleanWorkspace(input: CleanInput): Promise<CleanResult> {
  const homeDir = input.homeDir ?? homedir();
  const removals: CleanRemoval[] = [];
  const localBuildDirectory = join(input.repoRoot, WORKSPACE_BUILD_DIRECTORY);

  let bazelOutputBase: string | undefined;
  try {
    bazelOutputBase = await resolveBazelOutputBase(input);
  } catch (error) {
    removals.push({ path: "Bazel output base", outcome: "failed", error: describeError(error) });
  }

  // `.build` and its external-target record describe the same pointer as the
  // resolved external build: when resolution fails (an unreachable volume, a
  // mismatched sibling workspace), leave both alone rather than guess at
  // whether it is safe to unlink them.
  let externalWorkspaceBuild: string | undefined;
  let keepLocalBuildPointer = false;
  try {
    externalWorkspaceBuild = resolveExternalWorkspaceBuild(input.repoRoot);
  } catch (error) {
    keepLocalBuildPointer = true;
    removals.push({ path: localBuildDirectory, outcome: "failed", error: describeError(error) });
  }

  if (externalWorkspaceBuild) {
    removals.push(removePath(externalWorkspaceBuild, input.dry, true));
  }
  if (!keepLocalBuildPointer) {
    removals.push(removePath(localBuildDirectory, input.dry));
    removals.push(removePath(join(input.repoRoot, EXTERNAL_WORKSPACE_BUILD_RECORD), input.dry));
  }

  removals.push(removePath(join(input.repoRoot, "apps", "desktop", "src-tauri", "target"), input.dry));

  if (bazelOutputBase !== undefined) {
    removals.push(removePath(bazelOutputBase, input.dry));
  }

  if (input.sharedRustBuild) {
    // Same directory `resolveKdContext` treats as the legacy shared build
    // dir; if these two disagree, `kd clean` silently leaves it behind.
    removals.push(
      removePath(join(appCacheDir(homeDir, process.env, process.platform), "kanna", "rust-build"), input.dry)
    );
  }

  if (input.all) {
    for (const segments of [
      ["apps", "desktop", "dist"],
      ["node_modules"],
      ["apps", "desktop", "node_modules"],
      ["packages", "core", "node_modules"],
      ["packages", "db", "node_modules"],
      [".turbo"]
    ]) {
      removals.push(removePath(join(input.repoRoot, ...segments), input.dry));
    }
  }

  return { bazelOutputBase, removals };
}
