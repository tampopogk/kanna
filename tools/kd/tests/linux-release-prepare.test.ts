/** Real isolated Git checkout and real collector/deb byte validation; only
 * native Bazel compilation is replaced with explicitly synthetic payloads. */
import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs";
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
    const channel = args.at(-1)!.includes("deb_production_") ? "production" : "staging";
    const packageName = channel === "production" ? "kanna" : "kanna-staging";
    const debianVersion = channel === "production" ? "0.2.0-1" : "0.2.0~staging.2-1";
    const name = `${packageName}_${debianVersion}_${arch}.deb`;
    const deb = join(cwd, ".build", name);
    if (args[1] === "cquery") return { exitCode: 0, stdout: `.build/${name}\n.build/${name}.json\n`, stderr: "" };
    const staged = join(cwd, ".build", `tree-${arch}`);
    mkdirSync(join(staged, "DEBIAN"), { recursive: true });
    const lib = join(staged, packageLayout({ channel }).libDir);
    mkdirSync(lib, { recursive: true });
    const report = JSON.parse(readFileSync(join(repo, "docs/evidence/2026-09-14-linux-products/native-arm64.json"), "utf8"));
    const policy = JSON.parse(readFileSync(join(cwd, "packaging/linux/runtime-policy.json"), "utf8"));
    Object.assign(report, { architecture, channel, debianVersion, buildRevision: mode === "stamp" ? "f".repeat(40) : ref, buildTree: tree });
    for (const fact of report.executables) {
      const bytes = Buffer.from(`SYNTHETIC PREPARATION TEST ${channel} ${architecture} ${fact.path}`);
      writeFileSync(join(lib, fact.path), bytes);
      Object.assign(fact, { sha256: sha256(bytes), machine: policy.architectures[architecture].elfMachine, interpreter: policy.architectures[architecture].interpreter });
    }
    writeFileSync(join(staged, "DEBIAN/control"), `Package: ${packageName}\nVersion: ${report.debianVersion}\nArchitecture: ${arch}\nDepends: ${report.depends.join(", ")}\nDescription: SYNTHETIC TEST ONLY\n`);
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
  const ship = parseCliArgs(["release", "ship", "--platform", "linux", "--staging", "--prepared-manifest", "/owned/manifest.json", "--source-ref", "a".repeat(40), "--promotion-base", "a".repeat(40), "--staging-iteration", "2"]);
  expect(getTaskDefinition(ship.taskId).inputSchema.parse(ship.input)).toMatchObject({ preparedManifest: "/owned/manifest.json", sourceRef: "a".repeat(40), promotionBase: "a".repeat(40) });
  expect(() => getTaskDefinition(ship.taskId).inputSchema.parse({ ...ship.input, sourceRef: "main" })).toThrow();
  const renewed = parseCliArgs(["release", "renew", "--platform", "linux", "--candidate", "linux-v0.2.0-staging.2", "--renewal", "1", "--valid-for-hours", "96"]);
  expect(getTaskDefinition(renewed.taskId).inputSchema.parse(renewed.input)).toMatchObject({ renewal: 1, validForHours: 96 });
  for (const id of ["release.prepare", "release.renew"]) expect(() => getTaskDefinition(id).inputSchema.parse({ platform: "macos" })).toThrow();
  expect(() => getTaskDefinition("release.prepare").inputSchema.parse({ ...prepared.input, ref: "main" })).toThrow();
});

it("ships retained prepared bytes from a pinned product after controller/main advance, with refusal fences", async () => {
  const { shipLinuxRelease, linuxReleaseStatus } = await import("../src/runtime/linux-release");
  const { FilesystemAptStorage } = await import("../src/runtime/linux-apt-storage");
  const { readCandidate } = await import("../src/runtime/linux-release-state");
  const { readLinuxPrepared } = await import("../src/runtime/linux-release-prepared");
  const openpgp = await import("openpgp");
  const { vi } = await import("vitest");
  const f = fixture();
  const prepared = await prepareLinuxRelease(f.input);
  const git = (args: string[]) => execFileSync("git", args, { cwd: f.directory, encoding: "utf8" }).trim();
  // Controller is deliberately a newer commit with a different VERSION. No
  // product build cache survives prepare's isolated checkout.
  git(["add", "VERSION"]);
  git(["-c", "user.name=Fixture", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "commit", "-m", "Advance controller"]);
  const controller = git(["rev-parse", "HEAD"]);
  let tip = controller;
  const dir = join(f.directory, ".tmp/publication");
  mkdirSync(dir, { recursive: true });
  const key = await openpgp.generateKey({ type: "rsa", rsaBits: 3072, subkeys: [], format: "object", config: { v6Keys: false }, userIDs: [{ name: "Disposable prepared publication" }] });
  writeFileSync(join(dir, "public.asc"), key.publicKey.armor());
  writeFileSync(join(dir, "private.asc"), key.privateKey.armor(), { mode: 0o600 });
  const evidencePath = join(dir, "evidence.txt");
  writeFileSync(evidencePath, "SYNTHETIC integration evidence, not native acceptance");
  const acceptance = {
    schemaVersion: 1, sourceRevision: f.ref, sourceTree: f.tree, version: "0.2.0", iteration: 2,
    artifacts: Object.fromEntries(prepared.artifacts.map(a => [a.architecture, a.sha256])),
    checks: (["x86_64", "arm64"] as const).map(architecture => ({ architecture, kind: "installed", status: "pass", testedAt: new Date().toISOString(), evidencePath, evidenceSha256: sha256(readFileSync(evidencePath)) })),
  };
  const acceptancePath = join(dir, "acceptance.json");
  writeFileSync(acceptancePath, JSON.stringify(acceptance));
  const env = { ...process.env, KANNA_LINUX_ARCHIVE_BACKEND: "filesystem", KANNA_LINUX_ARCHIVE_ROOT: join(dir, "archive"), KANNA_LINUX_ARCHIVE_BASE_URL: "https://prepared.example.invalid", KANNA_LINUX_ARCHIVE_VALID_HOURS: "72", KANNA_LINUX_APT_PUBLIC_KEY_PATH: join(dir, "public.asc"), KANNA_LINUX_APT_PRIVATE_KEY_PATH: join(dir, "private.asc"), KANNA_LINUX_APT_FINGERPRINT: key.publicKey.getFingerprint() };
  mkdirSync(env.KANNA_LINUX_ARCHIVE_ROOT);
  const tags = new Map<string, string>();
  const releases = new Map<string, { body: string }>();
  const calls: string[][] = [];
  let productionBuild = false;
  const runner: CommandRunner = { run: async (command, args, options) => {
    calls.push([command, ...args]);
    if (command === "bazel") {
      if (!productionBuild) throw new Error("Prepared publication must not build");
      expect(args.at(-1)).toContain("deb_production_");
      return f.input.runner.run(command, args, options);
    }
    if (command === "git") {
      if (args[0] === "fetch" && args.includes("origin")) return { exitCode: 0, stdout: "", stderr: "" };
      if (args[0] === "remote") return { exitCode: 0, stdout: "https://github.com/example/kanna.git", stderr: "" };
      if (args[0] === "ls-remote") {
        const tag = args[2]?.replace("refs/tags/", "");
        return { exitCode: 0, stdout: args[1] === "--heads" ? tip + "\t" + args[3] : tags.has(tag) ? tags.get(tag) + "\trefs/tags/" + tag : "", stderr: "" };
      }
      return nodeCommandRunner.run(command, args, options);
    }
    if (command === "gh") {
      if (args[0] === "api") {
        const value = releases.get(args[1].split("/tags/")[1]);
        return value ? { exitCode: 0, stdout: JSON.stringify(value), stderr: "" } : { exitCode: 1, stdout: "", stderr: "HTTP 404" };
      }
      const tag = args[2];
      releases.set(tag, { body: readFileSync(args[args.indexOf("--notes-file") + 1], "utf8") });
      if (args[1] === "create") tags.set(tag, args[args.indexOf("--target") + 1]);
      return { exitCode: 0, stdout: "", stderr: "" };
    }
    throw new Error("Unexpected tool " + command);
  } };
  const input = { repoRoot: f.directory, env, runner, staging: true, stagingIteration: 2, preparedManifest: prepared.manifestPath, sourceRef: f.ref, promotionBase: f.ref, acceptance: acceptancePath };
  const originalManifest = readFileSync(prepared.manifestPath);
  const firstPath = join(prepared.outDir, prepared.artifacts[0].fileName);
  const originalDeb = readFileSync(firstPath);
  try {
    vi.stubGlobal("fetch", async (url: string) => {
      expect(new URL(url).origin).toBe(env.KANNA_LINUX_ARCHIVE_BASE_URL);
      try { return new Response(readFileSync(join(env.KANNA_LINUX_ARCHIVE_ROOT, decodeURIComponent(new URL(url).pathname)))); }
      catch { return new Response("missing", { status: 404 }); }
    });
    await expect(shipLinuxRelease({ ...input, promotionBase: controller })).rejects.toThrow(/matching --promotion-base/);
    for (const change of [
      (m: typeof prepared) => { m.artifacts[1] = m.artifacts[0]; },
      (m: typeof prepared) => { m.artifacts[0].fileName = "../outside.deb"; },
      (m: typeof prepared) => { m.artifacts[0].reportSha256 = "f".repeat(64); },
      (m: typeof prepared) => { m.iteration = 3; },
    ]) {
      const changed = JSON.parse(originalManifest.toString()); change(changed);
      writeFileSync(prepared.manifestPath, JSON.stringify(changed));
      await expect(shipLinuxRelease({ ...input, dryRun: true })).rejects.toThrow();
    }
    writeFileSync(prepared.manifestPath, originalManifest);
    const reportPath = firstPath + ".json";
    const reportBytes = readFileSync(reportPath);
    const linkedReport = join(dir, "linked-report.json"); writeFileSync(linkedReport, reportBytes);
    unlinkSync(reportPath); symlinkSync(linkedReport, reportPath);
    await expect(shipLinuxRelease({ ...input, dryRun: true })).rejects.toThrow(/symlink/);
    unlinkSync(reportPath); writeFileSync(reportPath, reportBytes);
    writeFileSync(join(f.directory, "VERSION"), "dirty\n");
    await expect(shipLinuxRelease({ ...input, dryRun: true })).rejects.toThrow(/clean committed/);
    writeFileSync(join(f.directory, "VERSION"), "9.9.9\n");
    const tampered = JSON.parse(originalManifest.toString());
    tampered.source.tree = "f".repeat(40);
    writeFileSync(prepared.manifestPath, JSON.stringify(tampered));
    await expect(shipLinuxRelease({ ...input, dryRun: true })).rejects.toThrow(/source\/tree/);
    writeFileSync(prepared.manifestPath, originalManifest);
    writeFileSync(firstPath, Buffer.concat([originalDeb, Buffer.from("changed")]));
    await expect(shipLinuxRelease({ ...input, dryRun: true })).rejects.toThrow();
    writeFileSync(firstPath, originalDeb);
    const wrongAcceptance = { ...acceptance, artifacts: { ...acceptance.artifacts, arm64: "f".repeat(64) } };
    writeFileSync(acceptancePath, JSON.stringify(wrongAcceptance));
    await expect(shipLinuxRelease({ ...input, release: true })).rejects.toThrow(/acceptance/i);
    writeFileSync(acceptancePath, JSON.stringify(acceptance));
    expect(releases.size).toBe(0);
    const result = await shipLinuxRelease({ ...input, release: true });
    expect(result).toMatchObject({ published: true, source: prepared.source, controller: { revision: controller }, version: "0.2.0", promotionBase: { kind: "commit", branch: "main", revision: f.ref } });
    expect(result.artifacts).toEqual(prepared.artifacts);
    const storage = new FilesystemAptStorage(env.KANNA_LINUX_ARCHIVE_ROOT);
    const candidate = await readCandidate(storage, "linux-v0.2.0-staging.2");
    expect(candidate.artifacts).toEqual(prepared.artifacts);
    const status = await linuxReleaseStatus(input);
    expect(status.promotion.blockers.join("\n")).toMatch(/24h/);
    expect(status.promotion.blockers.join("\n")).not.toMatch(/promotion base/);
    await expect(shipLinuxRelease({ repoRoot: f.directory, env, runner, promoteFrom: "0.2.0-staging.2", release: true })).rejects.toThrow(/24h/);
    tip = "f".repeat(40);
    const moved = await linuxReleaseStatus(input);
    expect(moved.promotion.allowed).toBe(false);
    expect(moved.promotion.blockers.join("\n")).toMatch(/ancestry/);
    expect(calls.some(c => c[0] === "bazel")).toBe(false);
    expect(calls.filter(c => c[0] === "gh").some(c => c.includes("desktop-staging"))).toBe(false);
    expect(readLinuxPrepared(prepared.manifestPath, { repoRoot: f.directory, source: prepared.source, version: "0.2.0", iteration: 2 }).map(a => a.identity)).toEqual(prepared.artifacts);
    tip = controller;
    // Promotion must rebuild the pinned source as production, never relabel
    // retained staging bytes. Its acceptance and clock are synthetic fixtures.
    const fullAcceptance = { ...acceptance, checks: [
      ...acceptance.checks,
      ...acceptance.checks.map(c => ({ ...c, kind: "upgrade", predecessor: { sourceRevision: "c".repeat(40), version: "0.2.0~staging.1-1", sha256: "d".repeat(64) } })),
      { ...acceptance.checks[0], architecture: "both", kind: "system" },
    ] };
    writeFileSync(acceptancePath, JSON.stringify(fullAcceptance));
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(Date.now() + 24 * 3600000 + 1000);
    productionBuild = true;
    const promoted = await shipLinuxRelease({ repoRoot: f.directory, env, runner, promoteFrom: "0.2.0-staging.2", release: true, acceptance: acceptancePath });
    expect(promoted).toMatchObject({ published: true, version: "0.2.0", source: prepared.source, controller: { revision: controller }, channel: "desktop-linux" });
    expect(promoted.artifacts.every(a => a.channel === "production" && a.buildRevision === f.ref && !prepared.artifacts.some(b => b.sha256 === a.sha256))).toBe(true);
    expect(calls.filter(c => c[0] === "bazel" && c[2] === "build")).toHaveLength(2);

  } finally {
    writeFileSync(prepared.manifestPath, originalManifest);
    writeFileSync(firstPath, originalDeb);
    vi.useRealTimers();
    vi.unstubAllGlobals();
  }
}, 30000);
