/** Disposable disk + test-key proof. Fixtures are synthetic publication inputs;
 * these tests establish storage/lifecycle behavior, never installed acceptance. */
import { mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { execFileSync, spawn } from "node:child_process";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import * as openpgp from "openpgp";
import { FilesystemAptStorage } from "../src/runtime/linux-apt-storage";
import { createAptPublicationSigner } from "../src/runtime/linux-apt-signature";
import { acceptanceBlockers, archiveState, candidatePath, jsonBytes, publishLinuxCandidate, readJson, releasePath, verifyLinuxPublication, type LinuxAcceptance, type LinuxCandidate, type LinuxPublicationReceipt } from "../src/runtime/linux-release-state";
import { sha256, verifyLinuxArtifact, type CollectedLinuxArtifact } from "../src/runtime/linux-release-artifacts";
import { evaluateStagingPublishGate } from "../src/runtime/release-lineage";
import { linuxReleaseStatus, shipLinuxRelease } from "../src/runtime/linux-release";
import { linuxReleaseConfig } from "../src/runtime/linux-release-config";
import { channelIdentity, packageLayout } from "../src/runtime/linux-package";

const repo = resolve(import.meta.dirname, "../../..");
mkdirSync(join(repo, ".tmp"), { recursive: true });
const root = mkdtempSync(join(repo, ".tmp/linux-lifecycle-tests-"));
afterAll(() => rmSync(root, { recursive: true, force: true }));
const scratch = () => mkdtempSync(join(root, "archive-"));
const now = new Date(Math.floor(Date.now() / 1000) * 1000);
let key: { publicKey: string; privateKey: string; fingerprint: string };
beforeAll(async () => {
  const generated = await openpgp.generateKey({ type: "rsa", rsaBits: 3072, subkeys: [], format: "object", date: new Date(now.getTime() - 86400000), config: { v6Keys: false }, userIDs: [{ name: "Disposable Linux lifecycle test", email: "test@example.invalid" }] });
  key = { publicKey: generated.publicKey.armor(), privateKey: generated.privateKey.armor(), fingerprint: generated.publicKey.getFingerprint() };
}, 30000);
function fixture() {
  const storage = new FilesystemAptStorage(scratch());
  const source = { revision: "a".repeat(40), tree: "b".repeat(40) };
  const artifacts: CollectedLinuxArtifact[] = (["x86_64", "arm64"] as const).map(architecture => {
    const arch = architecture === "x86_64" ? "amd64" : "arm64";
    const bytes = Buffer.from(`TEST ONLY ${architecture}`);
    const report = jsonBytes({ fixture: true, sha256: sha256(bytes) });
    const fileName = `kanna-staging_1.2.3~staging.1-1_${arch}.deb`;
    return { identity: { architecture, sourceRevision: source.revision, buildRevision: source.revision, buildTree: source.tree, version: "1.2.3", channel: "staging", iteration: 1, label: `//packaging/linux:deb_staging_${architecture}`, sha256: sha256(bytes), sizeBytes: bytes.length, reportSha256: sha256(report), fileName }, report, publication: { bytes, artifact: { architecture: arch, fileName, sha256: sha256(bytes), sizeBytes: bytes.length, controlFields: { Package: "kanna-staging", Version: "1.2.3~staging.1-1", Architecture: arch, Description: "TEST ONLY" } } } };
  });
  const candidate: LinuxCandidate = { schemaVersion: 1, platform: "linux", tag: "linux-v1.2.3-staging.1", channel: "desktop-linux-staging", source, promotionBase: { branch: "release/linux/1.2", revision: source.revision }, version: "1.2.3", iteration: 1, artifacts: artifacts.map(a => a.identity), aptArtifacts: artifacts.map(a => a.publication.artifact), preparedAt: now.toISOString(), validForHours: 72, baseUrl: "https://archive.example.invalid/linux", fingerprint: key.fingerprint, previousTag: null, previousInReleaseSha256: null, promotedFrom: null, acceptance: null };
  return { storage, candidate, artifacts };
}
async function publish(f: ReturnType<typeof fixture>, project = async (_c: LinuxCandidate, _r: LinuxPublicationReceipt) => {}, clock = now) {
  const signer = await createAptPublicationSigner({ ...key, now: () => clock });
  return f.storage.withExclusivePublication(() => publishLinuxCandidate({ ...f, acceptance: null, signer, key, now: () => clock, project }));
}
describe("concrete archive ownership", () => {
  it("serializes the whole archive across independent adapters and releases after failure", async () => {
    const { storage } = fixture();
    await storage.withExclusivePublication(async () => {
      const competitor = new FilesystemAptStorage(storage.root);
      await expect(competitor.withExclusivePublication(async () => {})).rejects.toThrow(/already owned/);
      await expect(storage.create("pool/item", Buffer.from("old"))).resolves.toBe(true);
      await expect(storage.create("pool/item", Buffer.from("new"))).resolves.toBe(false);
      expect(Buffer.from((await storage.read("pool/item"))!).toString()).toBe("old");
    });
    await expect(storage.withExclusivePublication(async () => { throw new Error("interrupted"); })).rejects.toThrow("interrupted");
    await expect(new FilesystemAptStorage(storage.root).read("pool/item")).resolves.toEqual(Buffer.from("old"));
    await expect(storage.replace("pool/item", Buffer.from("outside lock"))).rejects.toThrow(/ownership/);
  });
  it("releases kernel ownership when the publisher process is killed", async () => {
    const { storage } = fixture();
    const program = `import { FilesystemAptStorage } from ${JSON.stringify(join(repo, "tools/kd/src/runtime/linux-apt-storage.ts"))}; const storage = new FilesystemAptStorage(${JSON.stringify(storage.root)}); await storage.withExclusivePublication(async () => { process.stdout.write("owned\\n"); await new Promise(() => {}); });`;
    const child = spawn(process.execPath, ["--import", "tsx", "--input-type=module", "-e", program], { cwd: join(repo, "tools/kd"), stdio: ["ignore", "pipe", "pipe"] });
    let errors = ""; child.stderr.on("data", b => { errors += String(b); });
    const exited = new Promise<void>(resolve => child.once("close", () => resolve()));
    try {
      await new Promise<void>((resolve, reject) => {
        child.stdout.once("data", () => resolve());
        child.once("error", reject);
        child.once("exit", () => reject(new Error(errors || "publisher exited before ownership")));
      });
      await expect(new FilesystemAptStorage(storage.root).read("absent")).rejects.toThrow(/already owned/);
      child.kill("SIGKILL");
      await exited;
      // Wait on the kernel lock (bounded), not a timing-based retry: the
      // helper must observe stdin EOF and release ownership after parent death.
      execFileSync("/usr/bin/python3", ["-c", "import sys, fcntl; f = open(sys.argv[1], 'r+'); fcntl.flock(f, fcntl.LOCK_EX)", join(storage.root, ".publication.lock")], { timeout: 2000 });
      await expect(new FilesystemAptStorage(storage.root).read("absent")).resolves.toBeNull();
    } finally { child.kill("SIGKILL"); await exited; }
  });
  it("refuses traversal, symlinks and ownership loss before writing", async () => {
    const { storage } = fixture();
    await expect(storage.read("../escape")).rejects.toThrow(/Invalid archive/);
    symlinkSync(root, join(storage.root, "escape"));
    await expect(storage.withExclusivePublication(() => storage.create("escape/forbidden", Buffer.from("x")))).rejects.toThrow();
    await storage.withExclusivePublication(async () => {
      renameSync(join(storage.root, ".publication.lock"), join(storage.root, ".old-lock"));
      writeFileSync(join(storage.root, ".publication.lock"), "replacement");
      await expect(storage.replace("object", Buffer.from("x"))).rejects.toThrow(/ownership lost/);
    }).catch(error => expect(error.message).toMatch(/ownership lost/));
  });
});
describe("InRelease commit and recovery", () => {
  it("reads back actual signed bytes and leaves the other suite untouched", async () => {
    const f = fixture();
    const receipt = await publish(f);
    const result = await f.storage.withExclusivePublication(() => verifyLinuxPublication(f.storage, f.candidate, key, now));
    expect(result).toEqual(receipt);
    expect(await f.storage.read("dists/stable/InRelease")).toBeNull();
    expect((await archiveState(f.storage)).staging).toBe(f.candidate.tag);
  });
  it("repairs a failed GitHub projection without replacing inputs or restarting soak", async () => {
    const f = fixture();
    await expect(publish(f, async () => { throw new Error("GitHub interruption"); })).rejects.toThrow("GitHub interruption");
    const receipt = await readJson(f.storage, releasePath(f.candidate.tag, "publication.json"));
    const signed = await f.storage.read("dists/staging/InRelease");
    const retried = await publish(f, async () => {}, new Date(now.getTime() + 25 * 3600000));
    expect(retried).toEqual(receipt);
    expect(await f.storage.read("dists/staging/InRelease")).toEqual(signed);
    expect((await archiveState(f.storage)).pending).toBeNull();
  });
  it.each(["before", "after"])("recovers an interruption %s InRelease replacement with the same bytes", async phase => {
    const f = fixture();
    const replace = f.storage.replace.bind(f.storage);
    let interrupted = false;
    f.storage.replace = async (path, bytes) => {
      if (!interrupted && path === "dists/staging/InRelease") {
        interrupted = true;
        if (phase === "after") await replace(path, bytes);
        throw new Error("lost commit acknowledgement");
      }
      await replace(path, bytes);
    };
    await expect(publish(f)).rejects.toThrow(/lost commit/);
    const intended = await f.storage.read(releasePath(f.candidate.tag, "InRelease"));
    expect(await f.storage.read("dists/staging/InRelease")).toEqual(phase === "before" ? null : intended);
    f.storage.replace = replace;
    await publish(f);
    expect(await f.storage.read("dists/staging/InRelease")).toEqual(intended);
  });
  it("refuses differing retry inputs and out-of-band channel changes", async () => {
    const f = fixture();
    await expect(publish(f, async () => { throw new Error("projection"); })).rejects.toThrow();
    const changed = { ...f, candidate: { ...f.candidate, source: { ...f.candidate.source, revision: "c".repeat(40) } } };
    await expect(publish(changed)).rejects.toThrow(/Immutable/);
    await f.storage.withExclusivePublication(() => f.storage.replace("dists/staging/InRelease", Buffer.from("foreign")));
    await expect(publish(f)).rejects.toThrow(/outside/);
  });
  it("rejects corrupted pool bytes on status even when signatures and receipt remain", async () => {
    const f = fixture(); await publish(f);
    const a = f.artifacts[0].publication.artifact;
    writeFileSync(join(f.storage.root, `pool/main/k/kanna-staging/${a.fileName}`), "corrupted");
    await expect(f.storage.withExclusivePublication(() => verifyLinuxPublication(f.storage, f.candidate, key, now))).rejects.toThrow(/package/);
  });
});
describe("acceptance and platform gates", () => {
  function acceptance(c: LinuxCandidate): LinuxAcceptance {
    return { schemaVersion: 1, sourceRevision: c.source.revision, sourceTree: c.source.tree, version: c.version, iteration: c.iteration!, artifacts: Object.fromEntries(c.artifacts.map(a => [a.architecture, a.sha256])) as LinuxAcceptance["artifacts"], checks: (["x86_64", "arm64"] as const).map(architecture => ({ kind: "installed", architecture, status: "pass", testedAt: now.toISOString(), evidencePath: "fixture", evidenceSha256: "d".repeat(64) })) };
  }
  it("cannot use install-only or stale-source/hash acceptance for promotion", () => {
    const { candidate } = fixture(); const a = acceptance(candidate);
    expect(acceptanceBlockers(a, candidate, false, now)).toEqual([]);
    expect(acceptanceBlockers(a, candidate, true, now)).toHaveLength(3);
    expect(acceptanceBlockers({ ...a, sourceRevision: "f".repeat(40) }, candidate, false, now).join()).toMatch(/stale/);
    expect(acceptanceBlockers({ ...a, artifacts: { ...a.artifacts, arm64: "e".repeat(64) } }, candidate, false, now).join()).toMatch(/stale/);
    a.checks.push(...(["arm64", "x86_64"] as const).map(architecture => ({ kind: "upgrade" as const, architecture, status: "pass" as const, testedAt: now.toISOString(), evidencePath: "fixture", evidenceSha256: "d".repeat(64), predecessor: { sourceRevision: candidate.source.revision, version: "1.2.2~staging.1-1", sha256: "e".repeat(64) } })));
    expect(acceptanceBlockers(a, candidate, true, now).join()).toMatch(/distinct product source/);
  });
  it("freezes main behind Linux release branches without changing macOS branch semantics", () => {
    const args = { proposedSourceBranch: "main", proposedCommit: "b", active: { version: "1.2.3-staging.1", tag: "linux-v1.2.3-staging.1", commit: "a", sourceBranch: "release/linux/1.2", publishedAt: null }, relationship: "descendant" as const, activeProductionTagExists: false, activeMetadataError: null, reset: null, postPromotion: null };
    expect(evaluateStagingPublishGate({ ...args, platform: "linux" }).frozenBy).toBe("release/linux/1.2");
    expect(evaluateStagingPublishGate(args).frozenBy).toBeNull();
  });
  it("fails closed without config and refuses macOS selectors before any build or remote operation", async () => {
    const calls: unknown[] = [];
    const context = { repoRoot: repo, env: {}, runner: { run: async (...args: unknown[]) => { calls.push(args); throw new Error("must not run"); } } };
    expect(() => linuxReleaseConfig({})).toThrow(/configuration/);
    expect(await linuxReleaseStatus(context)).toMatchObject({ platform: "linux", promotion: { allowed: false } });
    await expect(shipLinuxRelease({ ...context, staging: true, arm64: true })).rejects.toThrow(/both architectures/);
    await expect(shipLinuxRelease({ ...context, production: true })).rejects.toThrow(/promote/);
    expect(calls).toEqual([]);
  });
});
describe("actual package bytes and stamped report", () => {
  it("rejects prototypes, wrong stamps, architecture/channel/control and executable mismatches", () => {
    const directory = scratch();
    const tree = join(directory, "tree");
    mkdirSync(join(tree, "DEBIAN"), { recursive: true });
    const report = JSON.parse(readFileSync(join(repo, "docs/evidence/2026-09-14-linux-products/native-arm64.json"), "utf8"));
    const source = { revision: "a".repeat(40), tree: "b".repeat(40) };
    report.buildRevision = source.revision; report.buildTree = source.tree;
    const lib = join(tree, packageLayout({ channel: "staging" }).libDir);
    mkdirSync(lib, { recursive: true });
    for (const fact of report.executables) {
      const bytes = Buffer.from(`TEST ${fact.path}`); writeFileSync(join(lib, fact.path), bytes); fact.sha256 = sha256(bytes);
    }
    writeFileSync(join(tree, "DEBIAN/control"), `Package: ${channelIdentity("staging").packageName}\nVersion: ${report.debianVersion}\nArchitecture: arm64\nDepends: ${report.depends.join(", ")}\nDescription: TEST ONLY\n`);
    const debPath = join(directory, "kanna-staging_0.2.0~staging.1-1_arm64.deb");
    execFileSync("/usr/bin/python3", [join(repo, "packaging/linux/artifact_tool.py"), "deb", tree, debPath]);
    const bytes = readFileSync(debPath); report.sha256 = sha256(bytes);
    const input = { repoRoot: repo, source, version: "0.2.0", channel: "staging" as const, iteration: 1, architecture: "arm64" as const, debPath, bytes, report: jsonBytes(report) };
    expect(verifyLinuxArtifact(input).identity.buildTree).toBe(source.tree);
    for (const change of [{ builder: "prototype" }, { auditOverridden: true }, { buildRevision: "c".repeat(40) }, { channel: "production" }, { architecture: "x86_64" }, { sha256: "d".repeat(64) }]) expect(() => verifyLinuxArtifact({ ...input, report: jsonBytes({ ...report, ...change }) })).toThrow();
    report.executables[0].sha256 = "e".repeat(64);
    expect(() => verifyLinuxArtifact({ ...input, report: jsonBytes(report) })).toThrow(/executable/);
  });
});

describe("explicit same-candidate renewal", () => {
  async function renew(f: ReturnType<typeof fixture>, time: Date, sequence = 1, hours = 72, observeCommit = async () => {}) {
    const { renewLinuxCandidate } = await import("../src/runtime/linux-release-renewal");
    const signer = await createAptPublicationSigner({ ...key, now: () => time });
    return f.storage.withExclusivePublication(() => renewLinuxCandidate({ ...f, sequence, validForHours: hours, signer, key, now: () => time, observeCommit, project: async () => {} }));
  }
  it("renews expired metadata while retaining original receipt, source, packages and soak timestamp", async () => {
    const f = fixture();
    const original = await publish(f);
    const candidate = await f.storage.read(candidatePath(f.candidate.tag));
    const signed = await f.storage.read(releasePath(f.candidate.tag, "InRelease"));
    const later = new Date(now.getTime() + 80 * 3600000);
    await expect(f.storage.withExclusivePublication(() => verifyLinuxPublication(f.storage, f.candidate, key, later))).rejects.toThrow(/expired/);
    const result = await renew(f, later);
    expect(result.receipt).toEqual(original);
    expect(await f.storage.read(candidatePath(f.candidate.tag))).toEqual(candidate);
    expect(await f.storage.read(releasePath(f.candidate.tag, "InRelease"))).toEqual(signed);
    expect(await f.storage.read("dists/staging/InRelease")).not.toEqual(signed);
    expect(await f.storage.withExclusivePublication(() => verifyLinuxPublication(f.storage, f.candidate, key, later))).toEqual(original);
    expect(await f.storage.read("dists/stable/InRelease")).toBeNull();
    const retry = await renew(f, new Date(later.getTime() + 3600000));
    expect(retry).toEqual(result);
    await expect(renew(f, later, 1, 96)).rejects.toThrow(/validity differs/);
  });
  it.each(["before", "after", "public"])("recovers renewal %s commit failure, including expired pending renewal", async phase => {
    const f = fixture(); const original = await publish(f);
    const later = new Date(now.getTime() + 80 * 3600000);
    const replace = f.storage.replace.bind(f.storage);
    if (phase !== "public") f.storage.replace = async (path, bytes) => {
      if (path === "dists/staging/InRelease") {
        if (phase === "after") await replace(path, bytes);
        throw new Error("disconnect");
      }
      await replace(path, bytes);
    };
    await expect(renew(f, later, 1, 72, async () => { if (phase === "public") throw new Error("disconnect"); })).rejects.toThrow(/disconnect/);
    f.storage.replace = replace;
    expect((await archiveState(f.storage)).pendingRenewal).toEqual({ tag: f.candidate.tag, sequence: 1 });
    const expired = new Date(later.getTime() + 80 * 3600000);
    await expect(renew(f, expired)).rejects.toThrow(/expired/);
    const result = await renew(f, expired, 2);
    expect(result.receipt).toEqual(original);
    expect((await archiveState(f.storage)).pendingRenewal).toBeNull();
    expect(await f.storage.withExclusivePublication(() => verifyLinuxPublication(f.storage, f.candidate, key, expired))).toEqual(original);
  });
  it("recovers an expired publication with no public receipt conservatively", async () => {
    const f = fixture();
    const signer = await createAptPublicationSigner({ ...key, now: () => now });
    await expect(f.storage.withExclusivePublication(() => publishLinuxCandidate({ ...f, acceptance: null, signer, key, now: () => now, observeCommit: async () => { throw new Error("public unavailable"); }, project: async () => {} }))).rejects.toThrow(/public unavailable/);
    const later = new Date(now.getTime() + 80 * 3600000);
    const result = await renew(f, later);
    expect(result.receipt.verifiedAt).toBe(later.toISOString());
    expect(result.receipt.inReleaseSha256).toBe(sha256((await f.storage.read("dists/staging/InRelease"))!));
    expect((await archiveState(f.storage)).pending).toBeNull();
    expect(await f.storage.withExclusivePublication(() => verifyLinuxPublication(f.storage, f.candidate, key, later))).toEqual(result.receipt);
  });
  it("retains a renewal's durable first observation when the initial receipt acknowledgement is lost", async () => {
    const f = fixture();
    const signer = await createAptPublicationSigner({ ...key, now: () => now });
    await expect(f.storage.withExclusivePublication(() => publishLinuxCandidate({ ...f, acceptance: null, signer, key, now: () => now, observeCommit: async () => { throw new Error("not public"); }, project: async () => {} }))).rejects.toThrow(/not public/);
    const later = new Date(now.getTime() + 80 * 3600000);
    const create = f.storage.create.bind(f.storage);
    f.storage.create = async (path, bytes) => {
      if (path === releasePath(f.candidate.tag, "publication.json")) throw new Error("receipt interruption");
      return create(path, bytes);
    };
    await expect(renew(f, later)).rejects.toThrow(/receipt interruption/);
    f.storage.create = create;
    const recovered = await renew(f, new Date(later.getTime() + 3600000));
    expect(recovered.receipt.verifiedAt).toBe(later.toISOString());
  });
  it("adopts an immutable renewal envelope after lost journal acknowledgement, without silently changing its date", async () => {
    const f = fixture(); const original = await publish(f);
    const later = new Date(now.getTime() + 80 * 3600000);
    const create = f.storage.create.bind(f.storage);
    f.storage.create = async (path, bytes) => {
      const result = await create(path, bytes);
      if (path.endsWith("/renewal.json")) throw new Error("envelope acknowledgement lost");
      return result;
    };
    await expect(renew(f, later)).rejects.toThrow(/envelope acknowledgement/);
    expect((await archiveState(f.storage)).pendingRenewal).toBeUndefined();
    f.storage.create = create;
    const recovered = await renew(f, new Date(later.getTime() + 3600000));
    expect(recovered.renewal.date).toBe(later.toISOString());
    expect(recovered.receipt).toEqual(original);
  });
  it("rejects altered artifacts and out-of-band metadata before renewal", async () => {
    const f = fixture(); await publish(f);
    await f.storage.withExclusivePublication(() => f.storage.replace("dists/staging/InRelease", Buffer.from("foreign")));
    await expect(renew(f, now)).rejects.toThrow(/outside/);
    const g = fixture(); await publish(g);
    writeFileSync(join(g.storage.root, releasePath(g.candidate.tag, "arm64.report.json")), "changed");
    await expect(renew(g, now)).rejects.toThrow(/report/);
  });
});
