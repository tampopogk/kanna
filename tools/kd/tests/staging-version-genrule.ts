import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

/**
 * Runs the real `//:staging_version_file` genrule over a VERSION / VERSION_RC
 * pair, the way Bazel would.
 *
 * Shared rather than duplicated because the fault it guards is an interaction
 * between two halves that each look correct alone: kd decides what to write
 * into the worktree, Bazel decides what to stamp from it, and a unit test of
 * either half passes while a bundle ships a version its own feed can never
 * match. Both the wiring cases and the ship cases drive this same command.
 */
const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
const rootBuildPath = join(repoRoot, "BUILD.bazel");

/** Resolve Starlark string escapes so the text matches what Bazel hands bash. */
export function unescapeStarlark(text: string): string {
  return text.replace(/\\(.)/gs, (_, character: string) => {
    if (character === "n") return "\n";
    if (character === "t") return "\t";
    if (character === "r") return "\r";
    return character;
  });
}

/**
 * The genrule's `cmd`, ready for bash. It is a single-line Starlark string
 * rather than a heredoc, so the quotes, the trailing comma and Bazel's `$$`
 * escape all have to come off first.
 */
export function stagingVersionGenruleCommand(): string {
  const source = readFileSync(rootBuildPath, "utf8");
  const rule = /\ngenrule\(\n {4}name = "staging_version_file",\n([\s\S]*?)\n\)\n/.exec(source);
  if (!rule) throw new Error("BUILD.bazel must declare the staging_version_file genrule");
  const cmd = /\n {4}cmd = "((?:[^"\\]|\\.)*)",/.exec(rule[1] ?? "");
  if (!cmd) throw new Error("staging_version_file must declare a single-line cmd");
  return unescapeStarlark(cmd[1] ?? "").replaceAll("$$", "$");
}

/** The staging version Bazel would stamp for these two committed values. */
export function runStagingVersionGenrule(version: string, candidate: string): string {
  const dir = mkdtempSync(join(tmpdir(), "kd-staging-version-genrule-"));
  try {
    const versionPath = join(dir, "VERSION");
    const candidatePath = join(dir, "VERSION_RC");
    const outputPath = join(dir, "VERSION_staging");
    writeFileSync(versionPath, `${version}\n`);
    writeFileSync(candidatePath, `${candidate}\n`);

    const command = stagingVersionGenruleCommand()
      .replace("$(location VERSION_RC)", candidatePath)
      .replace("$(location VERSION)", versionPath)
      .replaceAll("$@", outputPath);
    execFileSync("bash", ["-c", command], { cwd: dir, encoding: "utf8" });
    return readFileSync(outputPath, "utf8").trim();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/**
 * The staging version Bazel would stamp for a worktree on disk — reading the
 * two files exactly as the build's `srcs` do.
 */
export function stagingVersionForWorktree(worktreeRoot: string): string {
  return runStagingVersionGenrule(
    readFileSync(join(worktreeRoot, "VERSION"), "utf8").trim(),
    readFileSync(join(worktreeRoot, "VERSION_RC"), "utf8").trim()
  );
}
