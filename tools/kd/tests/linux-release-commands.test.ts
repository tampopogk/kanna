import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { afterAll, afterEach, beforeAll, expect, it, vi } from "vitest";
import * as openpgp from "openpgp";
const build = vi.hoisted(() => ({ collectLinuxRelease: vi.fn(), cleanLinuxSource: vi.fn() }));
vi.mock("../src/runtime/linux-release-artifacts", async original => ({ ...await original<typeof import("../src/runtime/linux-release-artifacts")>(), ...build }));
import { shipLinuxRelease, linuxReleaseStatus } from "../src/runtime/linux-release";
import { sha256, type CollectedLinuxArtifact } from "../src/runtime/linux-release-artifacts";
import { FilesystemAptStorage } from "../src/runtime/linux-apt-storage";
import { archiveState, jsonBytes, readJson, releasePath, type LinuxAcceptance } from "../src/runtime/linux-release-state";
import type { CommandRunner } from "../src/runtime/process";

const repo = resolve(import.meta.dirname, "../../..");
mkdirSync(join(repo, ".tmp"), { recursive: true });
const root = mkdtempSync(join(repo, ".tmp/linux-command-tests-"));
const source = { revision: "a".repeat(40), tree: "b".repeat(40) };
const now = new Date(Math.floor(Date.now() / 1000) * 1000);
let keys: { privateKey: string; publicKey: string; fingerprint: string };
beforeAll(async () => {
  const k = await openpgp.generateKey({ type: "rsa", rsaBits: 3072, subkeys: [], format: "object", date: new Date(now.getTime() - 86400000), config: { v6Keys: false }, userIDs: [{ name: "Disposable command integration test", email: "test@example.invalid" }] });
  keys = { privateKey: k.privateKey.armor(), publicKey: k.publicKey.armor(), fingerprint: k.publicKey.getFingerprint() };
}, 30000);
afterAll(() => rmSync(root, { recursive: true, force: true }));
afterEach(() => { vi.useRealTimers(); vi.clearAllMocks(); vi.unstubAllGlobals(); });
function artifacts(channel: "staging" | "production", iteration?: number): CollectedLinuxArtifact[] {
  return (["x86_64", "arm64"] as const).map(architecture => {
    const arch = architecture === "x86_64" ? "amd64" : "arm64";
    const bytes = Buffer.from(`TEST ONLY ${channel} ${architecture} ${iteration}`);
    const report = jsonBytes({ fixture: true });
    const name = channel === "staging" ? "kanna-staging" : "kanna";
    const version = `1.2.3${channel === "staging" ? `~staging.${iteration}` : ""}-1`;
    const fileName = `${name}_${version}_${arch}.deb`;
    return { identity: { architecture, sourceRevision: source.revision, buildRevision: source.revision, buildTree: source.tree, version: "1.2.3", channel, iteration: iteration ?? null, label: `//packaging/linux:deb_${channel}_${architecture}`, sha256: sha256(bytes), reportSha256: sha256(report), sizeBytes: bytes.length, fileName }, report, publication: { bytes, artifact: { architecture: arch, fileName, sha256: sha256(bytes), sizeBytes: bytes.length, controlFields: { Package: name, Version: version, Architecture: arch, Description: "TEST ONLY" } } } };
  });
}
function setup() {
  vi.useFakeTimers({ toFake: ["Date"] }); vi.setSystemTime(now);
  const directory = mkdtempSync(join(root, "fixture-"));
  mkdirSync(join(directory, "archive"));
  writeFileSync(join(directory, "VERSION"), "1.2.3\n");
  writeFileSync(join(directory, "release-policy.json"), JSON.stringify({ productionSoakHours: 0, linux: { productionSoakHours: 24 } }));
  writeFileSync(join(directory, "key.asc"), keys.privateKey, { mode: 0o600 });
  writeFileSync(join(directory, "public.asc"), keys.publicKey);
  const evidencePath = join(directory, "test-evidence.txt");
  const evidence = "TEST ONLY attestation fixture"; writeFileSync(evidencePath, evidence);
  const a = artifacts("staging", 1);
  const acceptance: LinuxAcceptance = { schemaVersion: 1, sourceRevision: source.revision, sourceTree: source.tree, version: "1.2.3", iteration: 1, artifacts: { x86_64: a[0].identity.sha256, arm64: a[1].identity.sha256 }, checks: [] };
  for (const architecture of ["x86_64", "arm64"] as const) for (const kind of ["installed", "upgrade"] as const) acceptance.checks.push({ architecture, kind, status: "pass", testedAt: now.toISOString(), evidencePath, evidenceSha256: sha256(evidence), ...(kind === "upgrade" ? { predecessor: { sourceRevision: "c".repeat(40), version: "1.2.2~staging.1-1", sha256: "d".repeat(64) } } : {}) });
  acceptance.checks.push({ architecture: "both", kind: "system", status: "pass", testedAt: now.toISOString(), evidencePath, evidenceSha256: sha256(evidence) });
  const acceptancePath = join(directory, "acceptance.json"); writeFileSync(acceptancePath, jsonBytes(acceptance));
  const tags = new Map<string, string>();
  const releases = new Map<string, { body: string }>();
  let branchTip = source.revision;
  let failProjection = false;
  const calls: string[][] = [];
  const runner: CommandRunner = { run: async (command, args) => {
    calls.push([command, ...args]);
    let stdout = "";
    if (command === "git") {
      if (args[0] === "fetch") { /* fixture refs already current */ }
      else if (args[0] === "tag") stdout = [...tags.keys()].filter(t => t.includes("-staging.")).join("\n");
      else if (args[0] === "remote") stdout = "https://github.com/example/kanna.git";
      else if (args[0] === "ls-remote" && args[1] === "--heads") stdout = `${branchTip}\t${args[3]}`;
      else if (args[0] === "ls-remote") { const tag = args[2].replace("refs/tags/", ""); stdout = tags.has(tag) ? `${tags.get(tag)}\trefs/tags/${tag}` : ""; }
      else if (args[0] === "merge-base") { /* synthetic forward lineage */ }
      else throw new Error(`unexpected git ${args.join(" ")}`);
    } else if (command === "gh") {
      if (args[0] === "api") {
        const tag = args[1].split("/tags/")[1];
        if (!releases.has(tag)) return { exitCode: 1, stdout: "", stderr: "HTTP 404" };
        stdout = JSON.stringify(releases.get(tag));
      } else if (args[0] === "release") {
        const tag = args[2];
        if (failProjection && tag.startsWith("desktop-linux")) { failProjection = false; return { exitCode: 1, stdout: "", stderr: "fixture projection interrupted" }; }
        const body = readFileSync(args[args.indexOf("--notes-file") + 1], "utf8");
        releases.set(tag, { body });
        if (args[1] === "create") tags.set(tag, args[args.indexOf("--target") + 1]);
      } else throw new Error("unexpected gh");
    } else throw new Error(`macOS/unsupported tool: ${command}`);
    return { exitCode: 0, stdout, stderr: "" };
  } };
  const env = { KANNA_LINUX_ARCHIVE_BACKEND: "filesystem", KANNA_LINUX_ARCHIVE_ROOT: join(directory, "archive"), KANNA_LINUX_ARCHIVE_BASE_URL: "https://archive.example.invalid", KANNA_LINUX_ARCHIVE_VALID_HOURS: "72", KANNA_LINUX_APT_PUBLIC_KEY_PATH: join(directory, "public.asc"), KANNA_LINUX_APT_PRIVATE_KEY_PATH: join(directory, "key.asc"), KANNA_LINUX_APT_FINGERPRINT: keys.fingerprint };
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {
    const path = decodeURIComponent(new URL(url).pathname).slice(1);
    try { return new Response(readFileSync(join(env.KANNA_LINUX_ARCHIVE_ROOT, path))); }
    catch { return new Response("missing", { status: 404 }); }
  }));
  const context = { repoRoot: directory, env, runner };
  build.cleanLinuxSource.mockResolvedValue(source);
  build.collectLinuxRelease.mockImplementation(async input => artifacts(input.channel, input.iteration));
  return { context, acceptancePath, acceptance, calls, tags, releases, storage: new FilesystemAptStorage(env.KANNA_LINUX_ARCHIVE_ROOT), moveBranch: () => { branchTip = "f".repeat(40); }, interruptProjection: () => { failProjection = true; } };
}
it("ships Linux staging, enforces the exact 24h edge and promotes production identity on the same source", async () => {
  const f = setup();
  const shipped = await shipLinuxRelease({ ...f.context, staging: true, release: true, acceptance: f.acceptancePath });
  expect(shipped.published).toBe(true);
  vi.setSystemTime(new Date(now.getTime() + 24 * 3600000 - 1));
  let status = await linuxReleaseStatus({ ...f.context, acceptance: f.acceptancePath });
  expect(status.promotion.allowed).toBe(false);
  expect(status.promotion.blockers.join()).toMatch(/24h/);
  await expect(shipLinuxRelease({ ...f.context, promoteFrom: "1.2.3-staging.1", release: true, acceptance: f.acceptancePath })).rejects.toThrow(/24h/);
  vi.setSystemTime(new Date(now.getTime() + 24 * 3600000));
  status = await linuxReleaseStatus({ ...f.context, acceptance: f.acceptancePath });
  expect(status.promotion.blockers).toEqual([]);
  const stagingBytes = await f.storage.read("dists/staging/InRelease");
  const promoted = await shipLinuxRelease({ ...f.context, promoteFrom: "1.2.3-staging.1", release: true, acceptance: f.acceptancePath });
  expect(promoted).toMatchObject({ published: true, tag: "linux-v1.2.3", channel: "desktop-linux" });
  expect(build.collectLinuxRelease).toHaveBeenLastCalledWith(expect.objectContaining({ channel: "production", source }));
  expect(await f.storage.read("dists/staging/InRelease")).toEqual(stagingBytes);
  expect(Buffer.from((await f.storage.read("dists/stable/main/binary-amd64/Packages"))!).toString()).toContain("Package: kanna\n");
  expect(f.calls.some(call => call.includes("desktop") || call.includes("desktop-staging") || call.some(a => a.includes("apple-darwin")))).toBe(false);
});
it("retains a committed candidate across channel projection interruption and retry", async () => {
  const f = setup(); f.interruptProjection();
  await expect(shipLinuxRelease({ ...f.context, staging: true, release: true, acceptance: f.acceptancePath })).rejects.toThrow(/interrupted/);
  const original = await readJson(f.storage, releasePath("linux-v1.2.3-staging.1", "publication.json"));
  vi.setSystemTime(new Date(now.getTime() + 3600000));
  await shipLinuxRelease({ ...f.context, staging: true, release: true, acceptance: f.acceptancePath });
  expect(await readJson(f.storage, releasePath("linux-v1.2.3-staging.1", "publication.json"))).toEqual(original);
  expect((await archiveState(f.storage)).pending).toBeNull();
});
it("reports stale/missing acceptance and moved promotion base, and dry-run writes no publication", async () => {
  const f = setup();
  const dry = await shipLinuxRelease({ ...f.context, staging: true, dryRun: true });
  expect(dry.publication.allowed).toBe(false);
  expect(await f.storage.read("linux/state.json")).toBeNull();
  expect(f.releases.size).toBe(0);
  await shipLinuxRelease({ ...f.context, staging: true, release: true, acceptance: f.acceptancePath });
  vi.setSystemTime(new Date(now.getTime() + 25 * 3600000));
  f.acceptance.artifacts.arm64 = "e".repeat(64); writeFileSync(f.acceptancePath, jsonBytes(f.acceptance));
  let status = await linuxReleaseStatus({ ...f.context, acceptance: f.acceptancePath });
  expect(status.promotion.blockers.join()).toMatch(/stale/);
  f.moveBranch(); status = await linuxReleaseStatus(f.context);
  expect(status.promotion.allowed).toBe(false);
  expect(status.promotion.blockers.join()).toMatch(/promotion base/);
});

it("does not start soak until the public archive serves the committed bytes", async () => {
  const f = setup();
  const fetchPublic = globalThis.fetch;
  vi.stubGlobal("fetch", vi.fn(async () => new Response("not served", { status: 404 })));
  await expect(shipLinuxRelease({ ...f.context, staging: true, release: true, acceptance: f.acceptancePath })).rejects.toThrow(/public archive readback/);
  expect(await f.storage.read("dists/staging/InRelease")).not.toBeNull();
  expect(await readJson(f.storage, releasePath("linux-v1.2.3-staging.1", "publication.json"))).toBeNull();
  vi.stubGlobal("fetch", fetchPublic);
  vi.setSystemTime(new Date(now.getTime() + 3600000));
  await shipLinuxRelease({ ...f.context, staging: true, release: true, acceptance: f.acceptancePath });
  expect(await readJson(f.storage, releasePath("linux-v1.2.3-staging.1", "publication.json"))).toMatchObject({ verifiedAt: new Date(now.getTime() + 3600000).toISOString() });
});
