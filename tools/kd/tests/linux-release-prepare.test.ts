/** Real isolated Git checkout and real collector/deb byte validation; only
 * native Bazel compilation is replaced with explicitly synthetic payloads. */
import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { afterAll, expect, it } from "vitest";
import { prepareLinuxRelease } from "../src/runtime/linux-release-prepare";
import { nodeCommandRunner, type CommandRunner } from "../src/runtime/process";
import { sha256 } from "../src/runtime/linux-release-artifacts";
import { packageLayout } from "../src/runtime/linux-package";
import { parseCliArgs } from "../src/cli";
import { getTaskDefinition } from "../src/tasks/registry";

const repo = resolve(import.meta.dirname, "../../..");
mkdirSync(join(repo, ".tmp"), { recursive: true });
const root = mkdtempSync(join(repo, ".tmp/linux-prepare-tests-"));
afterAll(() => rmSync(root, { recursive: true, force: true }));
function fixture(mode: "normal" | "historical" | "stamp" | "dirty" = "normal") {
  const directory = mkdtempSync(join(root, "repo-"));
  const git = (args: string[]) => execFileSync("git", args, { cwd: directory, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  git(["init"]);
  for (const path of ["packaging/linux/products.bzl", "packaging/linux/runtime-policy.json", "tools/kd/src/runtime/linux-bazel-package.ts"]) {
    mkdirSync(dirname(join(directory, path)), { recursive: true }); copyFileSync(join(repo, path), join(directory, path));
  }
  if (mode === "historical") writeFileSync(join(directory, "packaging/linux/products.bzl"), "# Product graph without source stamp support\n");
  writeFileSync(join(directory, ".gitignore"), ".tmp/\n.build/\n");
  writeFileSync(join(directory, "VERSION"), "0.2.0\n");
  git(["add", "."]);
  git(["-c", "user.name=Preparation fixture", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "commit", "-m", "Synthetic source fixture"]);
  const ref = git(["rev-parse", "HEAD"]);
  const tree = git(["rev-parse", "HEAD^{tree}"]);
  // Dirty controller input must never become part of the isolated source.
  writeFileSync(join(directory, "VERSION"), "9.9.9\n");
  const calls: string[][] = [];
  const runner: CommandRunner = { run: async (command, args, options) => {
    calls.push([command, ...args]);
    if (command !== "bazel") return nodeCommandRunner.run(command, args, options);
    expect(args[0]).toBe("--batch");
    const cwd = options!.cwd!;
    expect(cwd).not.toBe(directory);
    const architecture = args.at(-1)!.endsWith("_x86_64") ? "x86_64" : "arm64";
    const arch = architecture === "arm64" ? "arm64" : "amd64";
    const name = `kanna-staging_0.2.0~staging.2-1_${arch}.deb`;
    const deb = join(cwd, ".build", name);
    if (args[1] === "cquery") return { exitCode: 0, stdout: `.build/${name}\n.build/${name}.json\n`, stderr: "" };
    const staged = join(cwd, ".build", `tree-${arch}`);
    mkdirSync(join(staged, "DEBIAN"), { recursive: true });
    const lib = join(staged, packageLayout({ channel: "staging" }).libDir);
    mkdirSync(lib, { recursive: true });
    const report = JSON.parse(readFileSync(join(repo, "docs/evidence/2026-09-14-linux-products/native-arm64.json"), "utf8"));
    const policy = JSON.parse(readFileSync(join(cwd, "packaging/linux/runtime-policy.json"), "utf8"));
    Object.assign(report, { architecture, debianVersion: "0.2.0~staging.2-1", buildRevision: mode === "stamp" ? "f".repeat(40) : ref, buildTree: tree });
    for (const fact of report.executables) {
      const bytes = Buffer.from(`SYNTHETIC PREPARATION TEST ${architecture} ${fact.path}`);
      writeFileSync(join(lib, fact.path), bytes);
      Object.assign(fact, { sha256: sha256(bytes), machine: policy.architectures[architecture].elfMachine, interpreter: policy.architectures[architecture].interpreter });
    }
    writeFileSync(join(staged, "DEBIAN/control"), `Package: kanna-staging\nVersion: ${report.debianVersion}\nArchitecture: ${arch}\nDepends: ${report.depends.join(", ")}\nDescription: SYNTHETIC TEST ONLY\n`);
    execFileSync("/usr/bin/python3", [join(repo, "packaging/linux/artifact_tool.py"), "deb", staged, deb]);
    report.sha256 = sha256(readFileSync(deb));
    writeFileSync(`${deb}.json`, JSON.stringify(report));
    if (mode === "dirty" && architecture === "arm64") writeFileSync(join(cwd, "VERSION"), "0.2.1\n");
    return { exitCode: 0, stdout: "", stderr: "" };
  } };
  return { directory, ref, tree, calls, input: { repoRoot: directory, ref, stagingIteration: 2, env: process.env, runner } };
}
it("prepares without release configuration, remote tip or key gates and retains both measured outputs", async () => {
  const f = fixture();
  // No archive/key environment loader is called; no remote exists in this repo.
  const result = await prepareLinuxRelease({ ...f.input, env: { PATH: process.env.PATH, GIT_DIR: join(f.directory, ".git"), GIT_WORK_TREE: f.directory, GIT_INDEX_FILE: join(f.directory, ".git/index") } });
  expect(result.source).toEqual({ revision: f.ref, tree: f.tree });
  expect(result.artifacts.map(a => a.architecture)).toEqual(["x86_64", "arm64"]);
  for (const a of result.artifacts) {
    expect(sha256(readFileSync(join(result.outDir, a.fileName)))).toBe(a.sha256);
    expect(sha256(readFileSync(join(result.outDir, `${a.fileName}.json`)))).toBe(a.reportSha256);
  }
  expect(JSON.parse(readFileSync(result.manifestPath, "utf8")).source).toEqual(result.source);
  expect(readFileSync(join(f.directory, "VERSION"), "utf8")).toBe("9.9.9\n");
  expect(readdirSync(join(f.directory, ".tmp"))).toEqual([]);
  expect(f.calls.some(c => c[0] === "gh" || c.includes("ls-remote"))).toBe(false);
  await expect(prepareLinuxRelease(f.input)).rejects.toThrow(/output already exists/);
});
it.each(["stamp", "dirty", "historical"] as const)("rejects %s source without a completed manifest or leaked checkout", async mode => {
  const f = fixture(mode);
  await expect(prepareLinuxRelease(f.input)).rejects.toThrow(mode === "stamp" ? /audit report/ : mode === "dirty" ? /clean committed/ : /Unsupported historical/);
  expect(existsSync(join(f.directory, ".build/linux-prepared"))).toBe(false);
  expect(readdirSync(join(f.directory, ".tmp"))).toEqual([]);
  if (mode === "historical") expect(f.calls.some(c => c[0] === "bazel")).toBe(false);
});
it("requires exact commits and exposes Linux-only strict CLI/registry surfaces", () => {
  const prepared = parseCliArgs(["release", "prepare", "--platform", "linux", "--ref", "a".repeat(40), "--staging-iteration", "2"]);
  expect(prepared.taskId).toBe("release.prepare");
  expect(getTaskDefinition(prepared.taskId).inputSchema.parse(prepared.input)).toMatchObject({ stagingIteration: 2 });
  const renewed = parseCliArgs(["release", "renew", "--platform", "linux", "--candidate", "linux-v0.2.0-staging.2", "--renewal", "1", "--valid-for-hours", "96"]);
  expect(getTaskDefinition(renewed.taskId).inputSchema.parse(renewed.input)).toMatchObject({ renewal: 1, validForHours: 96 });
  for (const id of ["release.prepare", "release.renew"]) expect(() => getTaskDefinition(id).inputSchema.parse({ platform: "macos" })).toThrow();
  expect(() => getTaskDefinition("release.prepare").inputSchema.parse({ ...prepared.input, ref: "main" })).toThrow();
});
