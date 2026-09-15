import { withLinuxSource } from "./linux-release-source";
/** Local exact-ref collection. The controller is current kd; all build inputs
 * come from an unmodified detached clone of the requested commit. */
import { closeSync, existsSync, fsyncSync, openSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { collectLinuxRelease } from "./linux-release-artifacts";
import { jsonBytes } from "./linux-release-state";
import type { CommandRunner } from "./process";

export async function prepareLinuxRelease(input: {
  repoRoot: string; env: NodeJS.ProcessEnv; runner: CommandRunner;
  ref: string; stagingIteration: number; outDir?: string;
}) {
  if (!/^[a-f0-9]{40}$/.test(input.ref)) throw new Error("Linux prepare --ref requires an exact 40-hex commit, not a moving ref.");
  if (!Number.isSafeInteger(input.stagingIteration) || input.stagingIteration < 1) throw new Error("Linux prepare requires a positive staging iteration.");
  const outDir = resolve(input.repoRoot, input.outDir ?? `.build/linux-prepared/${input.ref}-staging.${input.stagingIteration}`);
  if (existsSync(outDir)) throw new Error("Linux preparation output already exists; preserve it and select a new --out-dir.");
  return withLinuxSource(input, async ({ repoRoot: checkout, env, runner, source }) => {
    const revision = source.revision;
    const graphPath = join(checkout, "packaging/linux/products.bzl");
    const reportPath = join(checkout, "tools/kd/src/runtime/linux-bazel-package.ts");
    const graph = existsSync(graphPath) ? readFileSync(graphPath, "utf8") : "";
    const report = existsSync(reportPath) ? readFileSync(reportPath, "utf8") : "";
    if (!["KANNA_LINUX_BUILD_REVISION", "KANNA_LINUX_BUILD_TREE"].every(s => graph.includes(s)) || !["buildRevision: input.buildRevision", "buildTree: input.buildTree"].every(s => report.includes(s))) {
      throw new Error(`Unsupported historical Linux source ${revision}: its package graph/report lacks revision/tree stamps. Prepare a reviewed commit that includes stamp support and retains the intended product baseline; record that new commit, never the historical SHA.`);
    }
    const version = readFileSync(join(checkout, "VERSION"), "utf8").trim();
    const artifacts = await collectLinuxRelease({ repoRoot: checkout, env, runner, source, version, channel: "staging", iteration: input.stagingIteration });
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
  });
}
