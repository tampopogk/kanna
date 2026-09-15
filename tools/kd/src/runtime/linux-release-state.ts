/** Durable Linux candidate transaction. Apt is authoritative; GitHub is a
 * recoverable projection. All state changes share the archive's writer lock. */
import { linuxMetadataHistory, verifyLinuxRenewalReceipt } from "./linux-release-renewal";
import { readFileSync } from "node:fs";
import { z } from "zod";
import { channelIdentity, debianArchitecture, debianVersion, debFileName } from "./linux-package";
import { releasePlatform } from "./release-platform";
import { publishAptArchive, type AptPublicationStorage, type AptPublicationSigner } from "./linux-apt-publication";
import { buildPackagesIndex, buildReleaseIndex, inReleasePath, packagesByHashPath, packagesIndexPath, poolPath, type AptChannel } from "./linux-apt";
import { sha256, type CollectedLinuxArtifact, type LinuxArtifactIdentity, type LinuxSource } from "./linux-release-artifacts";
import { verifyAptRelease, type AptVerificationKey } from "./linux-apt-signature";

const digestSchema = z.string().regex(/^[a-f0-9]{64}$/);
const revisionSchema = z.string().regex(/^[a-f0-9]{40}$/);
const checkSchema = z.object({
  kind: z.enum(["installed", "upgrade", "system"]), architecture: z.enum(["x86_64", "arm64", "both"]),
  status: z.literal("pass"), testedAt: z.iso.datetime(), evidencePath: z.string(), evidenceSha256: digestSchema,
  predecessor: z.object({ sourceRevision: revisionSchema, version: z.string().regex(/^\d+\.\d+\.\d+~staging\.\d+-1$/), sha256: digestSchema }).strict().optional(),
}).strict();
export const linuxAcceptanceSchema = z.object({
  schemaVersion: z.literal(1), sourceRevision: revisionSchema, sourceTree: revisionSchema,
  version: z.string().regex(/^\d+\.\d+\.\d+$/), iteration: z.number().int().positive(),
  artifacts: z.object({ x86_64: digestSchema, arm64: digestSchema }).strict(), checks: z.array(checkSchema),
}).strict();
export type LinuxAcceptance = z.infer<typeof linuxAcceptanceSchema>;
export interface AcceptanceEvidence { acceptance: LinuxAcceptance; evidence: Record<string, Uint8Array> }
export function readLinuxAcceptance(path?: string): AcceptanceEvidence | null {
  if (!path) return null;
  const acceptance = linuxAcceptanceSchema.parse(JSON.parse(readFileSync(path, "utf8")));
  const evidence: Record<string, Uint8Array> = {};
  for (const check of acceptance.checks) {
    const bytes = readFileSync(check.evidencePath);
    if (sha256(bytes) !== check.evidenceSha256) throw new Error("Linux acceptance evidence bytes do not match the declared digest.");
    evidence[check.evidenceSha256] = bytes;
  }
  return { acceptance, evidence };
}
export function acceptanceBlockers(acceptance: LinuxAcceptance | null, candidate: Pick<LinuxCandidate, "source" | "version" | "iteration" | "artifacts">, production: boolean, now: Date): string[] {
  if (!acceptance) return ["Missing exact Linux installed acceptance" + (production ? ", genuine two-version upgrade and system/cross-machine acceptance." : ".")];
  const parsed = linuxAcceptanceSchema.safeParse(acceptance);
  if (!parsed.success) return ["Malformed Linux acceptance."];
  if (acceptance.sourceRevision !== candidate.source.revision || acceptance.sourceTree !== candidate.source.tree || acceptance.version !== candidate.version || acceptance.iteration !== candidate.iteration || candidate.artifacts.some(a => acceptance.artifacts[a.architecture] !== a.sha256)) return ["Linux acceptance is stale: source, tree, version, iteration or package hashes differ."];
  const blockers: string[] = [];
  const required = [["installed", "x86_64"], ["installed", "arm64"], ...(production ? [["upgrade", "x86_64"], ["upgrade", "arm64"], ["system", "both"]] : [])];
  for (const [kind, architecture] of required) {
    const checks = acceptance.checks.filter(c => c.kind === kind && c.architecture === architecture);
    if (checks.length !== 1) { blockers.push(`Missing or duplicate ${kind} acceptance for ${architecture}.`); continue; }
    const check = checks[0];
    if (Date.parse(check.testedAt) > now.getTime()) blockers.push(`Future-dated ${kind} acceptance for ${architecture}.`);
    if (kind === "upgrade") {
      const old = check.predecessor;
      // Numeric tuple covers this archive's deliberately bounded Debian version
      // grammar; no lexical version comparison or fabricated predecessor.
      const tuple = (v: string) => v.match(/\d+/g)!.map(Number);
      const older = old && tuple(old.version).some((n, i, a) => n < tuple(`${candidate.version}~staging.${candidate.iteration}-1`)[i] && a.slice(0, i).every((p, j) => p === tuple(`${candidate.version}~staging.${candidate.iteration}-1`)[j]));
      if (!old || old.sourceRevision === candidate.source.revision || old.sha256 === acceptance.artifacts[architecture as "arm64" | "x86_64"] || !older) blockers.push(`Upgrade ${architecture} requires a distinct product source and older package identity.`);
    }
  }
  return blockers;
}
export interface LinuxCandidate {
  schemaVersion: 1;
  platform: "linux";
  tag: string;
  channel: AptChannel;
  source: LinuxSource;
  promotionBase: { branch: string; revision: string };
  version: string;
  iteration: number | null;
  artifacts: LinuxArtifactIdentity[];
  aptArtifacts: CollectedLinuxArtifact["publication"]["artifact"][];
  preparedAt: string;
  validForHours: number;
  baseUrl: string;
  fingerprint: string;
  previousTag: string | null;
  previousInReleaseSha256: string | null;
  promotedFrom: string | null;
  acceptance: LinuxAcceptance | null;
}
export interface LinuxPublicationReceipt { tag: string; candidateSha256: string; inReleaseSha256: string; verifiedAt: string }
export interface LinuxArchiveState { schemaVersion: 1; staging: string | null; production: string | null; pending: string | null; renewals?: Record<string, number>; pendingRenewal?: { tag: string; sequence: number } | null }
export const statePath = "linux/state.json";
export const candidatePath = (tag: string) => {
  if (!/^linux-v\d+\.\d+\.\d+(?:-staging\.[1-9]\d*)?$/.test(tag)) throw new Error("Invalid Linux candidate tag.");
  return `linux/releases/${tag}/candidate.json`;
};
export const releasePath = (tag: string, name: string) => candidatePath(tag).replace("candidate.json", name);
export const jsonBytes = (value: unknown): Buffer => Buffer.from(JSON.stringify(value, null, 2) + "\n");
export async function readJson<T>(storage: AptPublicationStorage, path: string): Promise<T | null> {
  const bytes = await storage.read(path);
  return bytes === null ? null : JSON.parse(Buffer.from(bytes).toString()) as T;
}
export async function archiveState(storage: AptPublicationStorage): Promise<LinuxArchiveState> {
  const state = await readJson<LinuxArchiveState>(storage, statePath);
  if (!state) {
    if (await storage.read(inReleasePath("desktop-linux-staging")) || await storage.read(inReleasePath("desktop-linux"))) throw new Error("Apt archive exists without Linux provenance; refusing to initialize over it.");
    return { schemaVersion: 1, staging: null, production: null, pending: null };
  }
  if (state.schemaVersion !== 1 || !["staging", "production", "pending"].every(k => Object.hasOwn(state, k))) throw new Error("Invalid Linux archive state.");
  for (const tag of [state.staging, state.production, state.pending]) if (tag !== null) candidatePath(tag);
  for (const [tag, sequence] of Object.entries(state.renewals ?? {})) {
    candidatePath(tag);
    if (!Number.isSafeInteger(sequence) || sequence < 1) throw new Error("Invalid Linux renewal state.");
  }
  if (state.pendingRenewal) {
    candidatePath(state.pendingRenewal.tag);
    if (!Number.isSafeInteger(state.pendingRenewal.sequence) || state.pendingRenewal.sequence < 1) throw new Error("Invalid pending Linux renewal.");
  }
  return state;
}
export async function immutable(storage: AptPublicationStorage, path: string, bytes: Uint8Array): Promise<void> {
  await storage.create(path, bytes);
  const stored = await storage.read(path);
  if (!stored || sha256(stored) !== sha256(bytes)) throw new Error(`Immutable archive object differs: ${path}.`);
}
export function candidateRelease(candidate: LinuxCandidate): Buffer {
  const indexes: Record<string, string> = {};
  for (const arch of ["amd64", "arm64"]) indexes[packagesIndexPath(candidate.channel, arch)] = buildPackagesIndex(candidate.aptArtifacts.filter(a => a.architecture === arch));
  return Buffer.from(buildReleaseIndex({ channel: candidate.channel, architectures: ["amd64", "arm64"], date: new Date(candidate.preparedAt), validForHours: candidate.validForHours, indexes }));
}
export async function readCandidate(storage: AptPublicationStorage, tag: string): Promise<LinuxCandidate> {
  const c = await readJson<LinuxCandidate>(storage, candidatePath(tag));
  if (!c || c.schemaVersion !== 1 || c.platform !== "linux" || c.tag !== tag || !["desktop-linux-staging", "desktop-linux"].includes(c.channel) || !/^[a-f0-9]{40}$/.test(c.source?.revision) || !/^[a-f0-9]{40}$/.test(c.source?.tree) || !Array.isArray(c.artifacts) || c.artifacts.length !== 2 || new Set(c.artifacts.map(a => a.architecture)).size !== 2 || !Array.isArray(c.aptArtifacts) || c.aptArtifacts.length !== 2) throw new Error("Missing or malformed Linux candidate provenance.");
  const channel = c.channel === "desktop-linux" ? "production" : "staging";
  const platform = releasePlatform("linux");
  if ((channel === "staging" ? (!Number.isSafeInteger(c.iteration) || c.iteration! < 1 || tag !== platform.stagingTag(c.version, c.iteration!)) : (c.iteration !== null || tag !== platform.productionTag(c.version))) || !Number.isFinite(Date.parse(c.preparedAt)) || !Number.isFinite(c.validForHours) || c.validForHours <= 0 || c.promotionBase?.revision !== c.source.revision || (c.promotionBase.branch !== "main" && c.promotionBase.branch !== platform.seriesBranch(c.version))) throw new Error("Invalid Linux candidate version, date or promotion base.");
  for (const a of c.artifacts) {
    if (!["arm64", "x86_64"].includes(a.architecture) || a.sourceRevision !== c.source.revision || a.buildRevision !== c.source.revision || a.buildTree !== c.source.tree || a.version !== c.version || a.channel !== channel || a.iteration !== c.iteration || a.label !== `//packaging/linux:deb_${channel}_${a.architecture}` || !digestSchema.safeParse(a.sha256).success || !digestSchema.safeParse(a.reportSha256).success || a.fileName !== debFileName({ architecture: a.architecture, version: c.version, channel, stagingIteration: c.iteration ?? undefined })) throw new Error("Incoherent Linux artifact/source provenance.");
    const apt = c.aptArtifacts.filter(p => p.architecture === debianArchitecture(a.architecture));
    if (apt.length !== 1 || apt[0].sha256 !== a.sha256 || apt[0].sizeBytes !== a.sizeBytes || apt[0].fileName !== a.fileName || apt[0].controlFields.Package !== channelIdentity(channel).packageName || apt[0].controlFields.Version !== debianVersion(c.version, channel, c.iteration ?? undefined) || apt[0].controlFields.Architecture !== apt[0].architecture) throw new Error("Linux apt metadata differs from artifact provenance.");
  }
  return c;
}
export async function verifyLinuxPublication(storage: AptPublicationStorage, c: LinuxCandidate, key: AptVerificationKey, now: Date): Promise<LinuxPublicationReceipt> {
  const receipt = await readJson<LinuxPublicationReceipt>(storage, releasePath(c.tag, "publication.json"));
  const signed = await storage.read(inReleasePath(c.channel));
  const state = await archiveState(storage);
  const metadata = await linuxMetadataHistory(storage, c, key, now, state.renewals?.[c.tag] ?? 0);
  if (!receipt || receipt.tag !== c.tag || receipt.candidateSha256 !== sha256(jsonBytes(c)) || !signed || (!metadata.hashes.includes(receipt.inReleaseSha256) || sha256(metadata.signed) !== sha256(signed)) || !Number.isFinite(Date.parse(receipt.verifiedAt)) || Date.parse(receipt.verifiedAt) > now.getTime()) throw new Error("Missing or inconsistent verified Linux publication receipt.");
  await verifyLinuxRenewalReceipt(storage, c, state.renewals?.[c.tag] ?? 0, now, metadata.signed);
  await verifyLinuxClosure(storage, c);
  return receipt;
}
export async function verifyLinuxClosure(storage: AptPublicationStorage, c: LinuxCandidate): Promise<void> {
  for (const a of c.aptArtifacts) {
    const bytes = await storage.read(poolPath(a));
    if (!bytes || sha256(bytes) !== a.sha256 || bytes.byteLength !== a.sizeBytes) throw new Error("Published Linux package is missing or changed.");
    const index = buildPackagesIndex([a]);
    const storedIndex = await storage.read(packagesByHashPath(c.channel, a.architecture, index));
    if (!storedIndex || sha256(storedIndex) !== sha256(index)) throw new Error("Published Linux by-hash index is missing or changed.");
  }
  for (const a of c.artifacts) {
    const report = await storage.read(releasePath(c.tag, `${a.architecture}.report.json`));
    if (!report || sha256(report) !== a.reportSha256) throw new Error("Published Linux report is missing or changed.");
  }
}
/** Caller holds archive ownership across gates, apt, receipt and projections.
 * The scoped wrapper lets the existing apt transaction share that ownership. */
export async function publishLinuxCandidate(input: {
  candidate: LinuxCandidate; artifacts: CollectedLinuxArtifact[]; acceptance: AcceptanceEvidence | null;
  storage: AptPublicationStorage; signer: AptPublicationSigner; key: AptVerificationKey;
  observeCommit?: (candidate: LinuxCandidate) => Promise<void>;
  now: () => Date; project: (candidate: LinuxCandidate, receipt: LinuxPublicationReceipt) => Promise<void>;
}): Promise<LinuxPublicationReceipt> {
  const { storage, candidate: c } = input;
  if (JSON.stringify(c.artifacts) !== JSON.stringify(input.artifacts.map(a => a.identity)) || JSON.stringify(c.aptArtifacts) !== JSON.stringify(input.artifacts.map(a => a.publication.artifact))) throw new Error("Linux publication artifacts differ from immutable candidate inputs.");
  const state = await archiveState(storage);
  if (state.pendingRenewal) throw new Error("Recover pending Linux metadata renewal first.");
  if (state.pending && state.pending !== c.tag) throw new Error(`Recover pending Linux publication ${state.pending} first.`);
  await immutable(storage, candidatePath(c.tag), jsonBytes(c));
  await readCandidate(storage, c.tag);
  for (const artifact of input.artifacts) await immutable(storage, releasePath(c.tag, `${artifact.identity.architecture}.report.json`), artifact.report);
  if (input.acceptance) for (const [hash, bytes] of Object.entries(input.acceptance.evidence)) await immutable(storage, `linux/evidence/${hash}`, bytes);
  const slot = c.channel === "desktop-linux" ? "production" : "staging";
  if (state[slot] !== c.previousTag && state[slot] !== c.tag) throw new Error("Linux channel changed since the candidate was prepared.");
  if (state.renewals?.[c.tag]) {
    const receipt = await verifyLinuxPublication(storage, c, input.key, input.now());
    await input.observeCommit?.(c);
    await input.project(c, receipt);
    await storage.replace(statePath, jsonBytes({ ...state, [slot]: c.tag, pending: null }));
    return receipt;
  }
  await storage.replace(statePath, jsonBytes({ ...state, pending: c.tag }));
  let receipt = await readJson<LinuxPublicationReceipt>(storage, releasePath(c.tag, "publication.json"));
  const live = await storage.read(inReleasePath(c.channel));
  const cachedSignature = await storage.read(releasePath(c.tag, "InRelease"));
  const alreadyCommitted = live && cachedSignature && sha256(live) === sha256(cachedSignature);
  if (!alreadyCommitted) {
    if (receipt || (live ? sha256(live) : null) !== c.previousInReleaseSha256) throw new Error("Linux InRelease moved outside this pending transaction.");
    await publishAptArchive({ channel: c.channel, date: new Date(c.preparedAt), validForHours: c.validForHours, artifacts: input.artifacts.map(a => a.publication) }, {
      withExclusivePublication: work => work(), read: p => storage.read(p), create: (p, b) => storage.create(p, b), replace: (p, b) => storage.replace(p, b),
    }, { sign: async release => {
      const signed = cachedSignature ?? await input.signer.sign(release);
      await verifyAptRelease({ ...input.key, signedRelease: signed, expectedRelease: release, now: input.now() });
      await immutable(storage, releasePath(c.tag, "InRelease"), signed);
      return signed;
    } });
  }
  const committed = await storage.read(inReleasePath(c.channel));
  if (!committed) throw new Error("Linux InRelease commit readback failed.");
  await verifyAptRelease({ ...input.key, signedRelease: committed, expectedRelease: candidateRelease(c), now: input.now() });
  await input.observeCommit?.(c);
  if (!receipt) {
    // This is the first *verified* observation, never the earlier preparation or
    // signature time. A lost acknowledgement without a receipt cannot prove an
    // earlier soak start; recovery records a conservative first observation.
    receipt = { tag: c.tag, candidateSha256: sha256(jsonBytes(c)), inReleaseSha256: sha256(committed), verifiedAt: input.now().toISOString() };
    await immutable(storage, releasePath(c.tag, "publication.json"), jsonBytes(receipt));
  }
  await verifyLinuxPublication(storage, c, input.key, input.now());
  await storage.replace(statePath, jsonBytes({ ...state, [slot]: c.tag, pending: c.tag }));
  await input.project(c, receipt);
  await storage.replace(statePath, jsonBytes({ ...state, [slot]: c.tag, pending: null }));
  return receipt;
}
