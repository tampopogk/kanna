/** Local exact-ref collection. The controller is current kd; all build inputs
 * come from an unmodified detached clone of the requested commit. */
import { closeSync, existsSync, fsyncSync, openSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { cleanLinuxSource, collectLinuxRelease } from "./linux-release-artifacts";
import { jsonBytes } from "./linux-release-state";
import type { CommandRunner } from "./process";

export async function prepareLinuxRelease(input: {
  repoRoot: string; env: NodeJS.ProcessEnv; runner: CommandRunner;
  ref: string; stagingIteration: number; outDir?: string;
}) {
  if (!/^[a-f0-9]{40}$/.test(input.ref)) throw new Error("Linux prepare --ref requires an exact 40-hex commit, not a moving ref.");
  if (!Number.isSafeInteger(input.stagingIteration) || input.stagingIteration < 1) throw new Error("Linux prepare requires a positive staging iteration.");
  // Git exports these from hooks and callers. They must not redirect commands
  // in the isolated checkout back into the operator's index or working tree.
  const env = { ...input.env };
  for (const name of Object.keys(env)) if (/^GIT_(?:DIR|WORK_TREE|INDEX_FILE|OBJECT_DIRECTORY|ALTERNATE_OBJECT_DIRECTORIES|COMMON_DIR|NAMESPACE|CONFIG_COUNT|CONFIG_PARAMETERS|CONFIG_KEY_.*|CONFIG_VALUE_.*)$/.test(name)) delete env[name];
  const git = async (args: string[], cwd = input.repoRoot) => {
    const r = await input.runner.run("git", args, { cwd, env });
    if (r.exitCode) throw new Error(`Linux preparation git failed: ${r.stderr || r.stdout}`);
    return r.stdout.trim();
  };
  const revision = await git(["rev-parse", "--verify", `${input.ref}^{commit}`]);
  const tree = await git(["rev-parse", "--verify", `${input.ref}^{tree}`]);
  if (revision !== input.ref || !/^[a-f0-9]{40}$/.test(tree)) throw new Error("Linux preparation source mismatch.");
  const source = { revision, tree };
  const outDir = resolve(input.repoRoot, input.outDir ?? `.build/linux-prepared/${revision}-staging.${input.stagingIteration}`);
  if (existsSync(outDir)) throw new Error("Linux preparation output already exists; preserve it and select a new --out-dir.");
  mkdirSync(join(input.repoRoot, ".tmp"), { recursive: true });
  const scratch = mkdtempSync(join(input.repoRoot, ".tmp/linux-prepare-"));
  const checkout = join(scratch, "source");
  try {
    // No worktree/branch changes, hooks, shared index or mutable alternates.
    await git(["init", checkout]);
    await git(["-c", "protocol.file.allow=always", "fetch", "--no-tags", "--depth=1", "--", input.repoRoot, revision], checkout);
    await git(["-c", "core.hooksPath=/dev/null", "checkout", "--detach", revision], checkout);
    const check = async () => {
      const actual = await cleanLinuxSource(checkout, env, input.runner);
      if (actual.revision !== revision || actual.tree !== tree) throw new Error("Linux preparation source mismatch or changed during collection.");
    };
    await check();
    const graphPath = join(checkout, "packaging/linux/products.bzl");
    const reportPath = join(checkout, "tools/kd/src/runtime/linux-bazel-package.ts");
    const graph = existsSync(graphPath) ? readFileSync(graphPath, "utf8") : "";
    const report = existsSync(reportPath) ? readFileSync(reportPath, "utf8") : "";
    if (!["KANNA_LINUX_BUILD_REVISION", "KANNA_LINUX_BUILD_TREE"].every(s => graph.includes(s)) || !["buildRevision: input.buildRevision", "buildTree: input.buildTree"].every(s => report.includes(s))) {
      throw new Error(`Unsupported historical Linux source ${revision}: its package graph/report lacks revision/tree stamps. Prepare a reviewed commit that includes stamp support and retains the intended product baseline; record that new commit, never the historical SHA.`);
    }
    const version = readFileSync(join(checkout, "VERSION"), "utf8").trim();
    // Batch mode leaves no build server attached to a removed source checkout.
    const runner: CommandRunner = { run: (command, args, options) => input.runner.run(command, command === "bazel" ? ["--batch", ...args] : args, options) };
    const artifacts = await collectLinuxRelease({ repoRoot: checkout, env, runner, source, version, channel: "staging", iteration: input.stagingIteration });
    await check();
    const manifest = { schemaVersion: 1, kind: "linux-local-preparation", source, version, channel: "staging", iteration: input.stagingIteration, preparedAt: new Date().toISOString(), artifacts: artifacts.map(a => a.identity) };
    // Reserve the final directory exclusively; manifest is the completion marker.
    mkdirSync(resolve(outDir, ".."), { recursive: true });
    mkdirSync(outDir);
    for (const a of artifacts) {
      writeFileSync(join(outDir, a.identity.fileName), a.publication.bytes, { flag: "wx", flush: true });
      writeFileSync(join(outDir, `${a.identity.fileName}.json`), a.report, { flag: "wx", flush: true });
    }
    writeFileSync(join(outDir, ".manifest.json"), jsonBytes(manifest), { flag: "wx", flush: true });
    renameSync(join(outDir, ".manifest.json"), join(outDir, "manifest.json"));
    for (const directory of [outDir, resolve(outDir, "..")]) {
      const fd = openSync(directory, "r");
      try { fsyncSync(fd); } finally { closeSync(fd); }
    }
    return { ...manifest, manifestPath: join(outDir, "manifest.json"), outDir };
  } finally { rmSync(scratch, { recursive: true, force: true }); }
}
