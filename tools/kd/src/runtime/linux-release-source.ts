/** Controller and product checkouts have separate identities. Never run hooks
 * or inherit Git redirection variables into the isolated product snapshot. */
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { cleanLinuxSource, type LinuxSource } from "./linux-release-artifacts";
import type { CommandRunner } from "./process";

export function linuxSourceEnv(input: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const env = { ...input };
  for (const name of Object.keys(env)) if (/^GIT_(?:DIR|WORK_TREE|INDEX_FILE|OBJECT_DIRECTORY|ALTERNATE_OBJECT_DIRECTORIES|COMMON_DIR|NAMESPACE|CONFIG_COUNT|CONFIG_PARAMETERS|CONFIG_KEY_.*|CONFIG_VALUE_.*)$/.test(name)) delete env[name];
  return env;
}
/** The pinned snapshot is a build *input*; Bazel must never write to it.
 * `--lockfile_mode=error` stops Bazel refreshing the tracked `MODULE.bazel.lock`
 * mid-build, which otherwise dirties the checkout and is only noticed by the
 * post-build cleanliness check — after both architectures have been built, with
 * every artifact discarded and nothing naming the cause. Failing closed instead
 * reports a stale lock in seconds. `--batch` is a startup option and must lead;
 * `--lockfile_mode` is a command option and must follow the command word. */
export function linuxSourceBazelArgs(args: string[]): string[] {
  if (!args.length) throw new Error("Linux product source requires an explicit Bazel command.");
  return ["--batch", args[0], "--lockfile_mode=error", ...args.slice(1)];
}
export async function withLinuxSource<T>(input: {
  repoRoot: string; env: NodeJS.ProcessEnv; runner: CommandRunner; ref: string;
}, use: (product: { repoRoot: string; env: NodeJS.ProcessEnv; runner: CommandRunner; source: LinuxSource }) => Promise<T>): Promise<T> {
  if (!/^[a-f0-9]{40}$/.test(input.ref)) throw new Error("Linux product source requires an exact 40-hex commit.");
  const env = linuxSourceEnv(input.env);
  const git = async (args: string[], cwd = input.repoRoot) => {
    const result = await input.runner.run("git", args, { cwd, env });
    if (result.exitCode) throw new Error(`Linux source snapshot failed: ${result.stderr || result.stdout}`);
    return result.stdout.trim();
  };
  const revision = await git(["rev-parse", "--verify", `${input.ref}^{commit}`]);
  const tree = await git(["rev-parse", "--verify", `${input.ref}^{tree}`]);
  if (revision !== input.ref || !/^[a-f0-9]{40}$/.test(tree)) throw new Error("Linux product source identity mismatch.");
  mkdirSync(join(input.repoRoot, ".tmp"), { recursive: true });
  const scratch = mkdtempSync(join(input.repoRoot, ".tmp/linux-source-"));
  const checkout = join(scratch, "source");
  try {
    await git(["init", checkout]);
    await git(["-c", "protocol.file.allow=always", "fetch", "--no-tags", "--depth=1", "--", input.repoRoot, revision], checkout);
    await git(["-c", "core.hooksPath=/dev/null", "checkout", "--detach", revision], checkout);
    const source = { revision, tree };
    const check = async () => {
      if (JSON.stringify(await cleanLinuxSource(checkout, env, input.runner)) !== JSON.stringify(source)) throw new Error("Linux product source snapshot changed.");
    };
    await check();
    const runner: CommandRunner = { run: (command, args, options) => input.runner.run(command, command === "bazel" ? linuxSourceBazelArgs(args) : args, options) };
    const result = await use({ repoRoot: checkout, env, runner, source });
    await check();
    return result;
  } finally { rmSync(scratch, { recursive: true, force: true }); }
}
