import { withLinuxSource } from "./linux-release-source";
import { readLinuxPrepared } from "./linux-release-prepared";
import { renewLinuxCandidate, renewalPath } from "./linux-release-renewal";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { CommandRunner } from "./process";
import { releasePlatform } from "./release-platform";
import { releaseRepoSlug } from "./release-command";
import { compareVersions } from "./release-version";
import { evaluateStagingPublishGate, type StagingLineageRelationship } from "./release-lineage";
import { readReleasePolicy } from "./release-policy";
import { linuxArchiveStorage } from "./linux-apt-storage";
import { linuxReleaseConfig, readLinuxKeyFile, type LinuxReleaseConfig } from "./linux-release-config";
import { createAptPublicationSigner, type AptVerificationKey } from "./linux-apt-signature";
import { cleanLinuxSource, collectLinuxRelease, sha256 } from "./linux-release-artifacts";
import { buildPackagesIndex, inReleasePath, packagesByHashPath, poolPath } from "./linux-apt";
import { acceptanceBlockers, archiveState, candidatePath, immutable, jsonBytes, publishLinuxCandidate, readCandidate, readJson, readLinuxAcceptance, releasePath, verifyLinuxPublication, type AcceptanceEvidence, type LinuxCandidate, type LinuxPublicationReceipt } from "./linux-release-state";
import type { AptPublicationStorage } from "./linux-apt-publication";

interface LinuxContext { repoRoot: string; env: NodeJS.ProcessEnv; runner: CommandRunner }
export interface LinuxReleaseInput extends LinuxContext {
  staging?: boolean; production?: boolean; release?: boolean; dryRun?: boolean; skipBuild?: boolean;
  preparedManifest?: string; sourceRef?: string; promotionBase?: string;
  branch?: string; stagingIteration?: number; acceptance?: string; promoteFrom?: string;
  major?: boolean; minor?: boolean; patch?: boolean; arm64?: boolean; x86_64?: boolean;
  rollbackTo?: string; overrideSoak?: string;
}
const platform = releasePlatform("linux");
async function run(c: LinuxContext, command: string, args: string[]): Promise<string> {
  const result = await c.runner.run(command, args, { cwd: c.repoRoot, env: c.env });
  if (result.exitCode) throw new Error(`Linux release ${command} failed: ${result.stderr || result.stdout}`);
  return result.stdout.trim();
}
async function fetchLinux(c: LinuxContext): Promise<void> {
  await run(c, "git", ["fetch", "--no-tags", "origin", "+refs/heads/main:refs/remotes/origin/main", "+refs/heads/release/linux/*:refs/remotes/origin/release/linux/*", "+refs/tags/linux-v*:refs/tags/linux-v*", "+refs/tags/abandoned/release/linux/*:refs/tags/abandoned/release/linux/*"]);
}
async function relationship(c: LinuxContext, previous: string | null, next: string): Promise<StagingLineageRelationship> {
  if (!previous) return "initial";
  if (previous === next) return "same-commit";
  const ancestor = async (a: string, b: string) => {
    const r = await c.runner.run("git", ["merge-base", "--is-ancestor", a, b], { cwd: c.repoRoot, env: c.env });
    if (r.exitCode !== 0 && r.exitCode !== 1) throw new Error("Cannot resolve Linux candidate ancestry.");
    return r.exitCode === 0;
  };
  if (await ancestor(previous, next)) return "descendant";
  if (await ancestor(next, previous)) return "behind";
  return "diverged";
}
async function tagCommit(c: LinuxContext, tag: string): Promise<string | null> {
  const result = await run(c, "git", ["ls-remote", "origin", `refs/tags/${tag}`, `refs/tags/${tag}^{}`]);
  const refs = result.split("\n").filter(Boolean).map(l => l.split(/\s+/));
  return refs.find(r => r[1].endsWith("^{}"))?.[0] ?? refs[0]?.[0] ?? null;
}
async function remoteBranch(c: LinuxContext, branch: string): Promise<string> {
  if (branch !== "main" && !platform.isSeriesBranch(branch)) throw new Error("Linux source branch must be main or release/linux/X.Y.");
  const result = await run(c, "git", ["ls-remote", "--heads", "origin", `refs/heads/${branch}`]);
  const sha = result.split(/\s+/)[0];
  if (!/^[a-f0-9]{40}$/.test(sha)) throw new Error(`Missing Linux promotion base ${branch}.`);
  return sha;
}
async function matchesPromotionBase(context: LinuxContext, base: LinuxCandidate["promotionBase"]): Promise<boolean> {
  const tip = await remoteBranch(context, base.branch);
  if (tip === base.revision) return true;
  if (base.kind !== "commit") return false;
  // Fetch the observed tip explicitly; never use a stale tracking ref to
  // authorize a branch that has been rewritten away from the pinned source.
  await run(context, "git", ["fetch", "--no-tags", "origin", tip]);
  return await relationship(context, base.revision, tip) === "descendant";
}
function publicKey(config: LinuxReleaseConfig): AptVerificationKey {
  return { publicKey: readLinuxKeyFile(config.publicKeyPath), fingerprint: config.fingerprint };
}
function projection(c: LinuxCandidate, receipt: LinuxPublicationReceipt) {
  return { platform: "linux", tag: c.tag, source: c.source, version: c.version, iteration: c.iteration,
    candidateSha256: receipt.candidateSha256, publication: receipt,
    provenanceUrl: `${c.baseUrl.replace(/\/$/, "")}/${candidatePath(c.tag)}`,
    artifacts: c.aptArtifacts.map(a => ({ architecture: a.architecture, sha256: a.sha256, url: `${c.baseUrl.replace(/\/$/, "")}/${poolPath(a)}` })) };
}
async function releaseView(c: LinuxContext, slug: string, tag: string): Promise<{ body: string } | null> {
  const result = await c.runner.run("gh", ["api", `repos/${slug}/releases/tags/${tag}`], { cwd: c.repoRoot, env: c.env });
  if (result.exitCode) {
    if (/\b404\b/.test(result.stderr)) return null;
    throw new Error(`Cannot read Linux GitHub release ${tag}: ${result.stderr}`);
  }
  return JSON.parse(result.stdout);
}
async function projectGithub(context: LinuxContext, candidate: LinuxCandidate, receipt: LinuxPublicationReceipt): Promise<void> {
  const slug = releaseRepoSlug(await run(context, "git", ["remote", "get-url", "origin"]));
  const body = jsonBytes(projection(candidate, receipt)).toString();
  mkdirSync(join(context.repoRoot, ".tmp"), { recursive: true });
  const scratch = mkdtempSync(join(context.repoRoot, ".tmp/linux-release-"));
  const notes = join(scratch, "notes.json");
  writeFileSync(notes, body);
  try {
    const existingTag = await tagCommit(context, candidate.tag);
    if (existingTag && existingTag !== candidate.source.revision) throw new Error("Immutable Linux tag names a different source.");
    const existing = await releaseView(context, slug, candidate.tag);
    if (existing && existing.body !== body) throw new Error("Immutable Linux GitHub provenance differs.");
    if (!existing) await run(context, "gh", ["release", "create", candidate.tag, "--repo", slug, "--target", candidate.source.revision, "--title", candidate.tag, "--notes-file", notes, "--latest=false", ...(candidate.channel === platform.stagingChannelTag ? ["--prerelease"] : [])]);
    if (await tagCommit(context, candidate.tag) !== candidate.source.revision || (await releaseView(context, slug, candidate.tag))?.body !== body) throw new Error("Linux immutable GitHub projection readback failed.");
    const channel = await releaseView(context, slug, candidate.channel);
    if (channel) {
      const prior = JSON.parse(channel.body);
      if (prior.platform !== "linux" || ![candidate.previousTag, candidate.tag].includes(prior.tag)) throw new Error("Linux GitHub channel changed outside the pending publication.");
      if (channel.body !== body) await run(context, "gh", ["release", "edit", candidate.channel, "--repo", slug, "--notes-file", notes, "--latest=false"]);
    } else await run(context, "gh", ["release", "create", candidate.channel, "--repo", slug, "--target", candidate.source.revision, "--title", candidate.channel, "--notes-file", notes, "--prerelease", "--latest=false"]);
    if ((await releaseView(context, slug, candidate.channel))?.body !== body) throw new Error("Linux GitHub channel projection readback failed.");
  } finally { rmSync(scratch, { recursive: true, force: true }); }
}
/** A local upload is not a published staging candidate until the configured
 * public archive serves those exact bytes. No credentials or provisioning here. */
async function publicReadback(storage: AptPublicationStorage, c: LinuxCandidate, extraPaths: string[] = []): Promise<void> {
  const renewal = (await archiveState(storage)).renewals?.[c.tag] ?? 0;
  const paths = [...new Set([...extraPaths, ...Array.from({ length: renewal }, (_, n) => renewalPath(c.tag, n + 1, "renewal.json"))]), inReleasePath(c.channel), candidatePath(c.tag), ...c.artifacts.map(a => releasePath(c.tag, `${a.architecture}.report.json`))];
  for (const a of c.aptArtifacts) paths.push(poolPath(a), packagesByHashPath(c.channel, a.architecture, buildPackagesIndex([a])));
  for (const path of paths) {
    const expected = await storage.read(path);
    const url = `${c.baseUrl.replace(/\/$/, "")}/${path.split("/").map(encodeURIComponent).join("/")}`;
    const response = await fetch(url, { cache: "no-store", redirect: "error", signal: AbortSignal.timeout(30_000) });
    if (!response.ok) throw new Error(`Linux public archive readback failed for ${path}: HTTP ${response.status}.`);
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (!expected || bytes.byteLength !== expected.byteLength || sha256(bytes) !== sha256(expected)) throw new Error(`Linux public archive bytes differ at ${path}.`);
  }
}
async function checkProjection(context: LinuxContext, c: LinuxCandidate, receipt: LinuxPublicationReceipt): Promise<void> {
  const slug = releaseRepoSlug(await run(context, "git", ["remote", "get-url", "origin"]));
  const expected = jsonBytes(projection(c, receipt)).toString();
  if (await tagCommit(context, c.tag) !== c.source.revision || (await releaseView(context, slug, c.tag))?.body !== expected || (await releaseView(context, slug, c.channel))?.body !== expected) throw new Error("Linux GitHub tag/provenance/channel projection is missing or inconsistent; retry the pending publication.");
}
async function verifyEvidence(storage: AptPublicationStorage, candidate: LinuxCandidate, acceptance: AcceptanceEvidence | null): Promise<void> {
  const record = acceptance?.acceptance ?? candidate.acceptance;
  if (!record) return;
  for (const check of record.checks) {
    const bytes = acceptance?.evidence[check.evidenceSha256] ?? await storage.read(`linux/evidence/${check.evidenceSha256}`);
    if (!bytes || sha256(bytes) !== check.evidenceSha256) throw new Error("Linux acceptance evidence is missing or changed.");
  }
}
async function promotionStatus(context: LinuxContext, storage: AptPublicationStorage, config: LinuxReleaseConfig, acceptance: AcceptanceEvidence | null, now: Date) {
  const blockers: string[] = [];
  const state = await archiveState(storage);
  let candidate: LinuxCandidate | null = null;
  let receipt: LinuxPublicationReceipt | null = null;
  const requiredHours = Math.max(24, readReleasePolicy(context.repoRoot).linux.productionSoakHours);
  if (!config.privateKeyPath) blockers.push("Missing KANNA_LINUX_APT_PRIVATE_KEY_PATH on the trusted release host.");
  else try {
    await createAptPublicationSigner({ ...publicKey(config), privateKey: readLinuxKeyFile(config.privateKeyPath, true), passphrase: config.passphrasePath ? readLinuxKeyFile(config.passphrasePath, true).replace(/\r?\n$/, "") : undefined, now: () => now });
  } catch (error) { blockers.push(`Linux apt key preflight failed: ${(error as Error).message}`); }
  if (state.pendingRenewal) blockers.push(`Incomplete Linux metadata renewal ${state.pendingRenewal.tag}/${state.pendingRenewal.sequence}; retry release renew.`);
  if (state.pending) blockers.push(`Incomplete Linux publication ${state.pending}; retry it before any new candidate.`);
  if (!state.staging) blockers.push("No verified Linux staging candidate.");
  else {
    candidate = await readCandidate(storage, state.staging);
    try { receipt = await verifyLinuxPublication(storage, candidate, publicKey(config), now); } catch (e) { blockers.push((e as Error).message); }
    if (receipt) try { await publicReadback(storage, candidate); } catch (e) { blockers.push((e as Error).message); }
    if (receipt) try { await checkProjection(context, candidate, receipt); } catch (e) { blockers.push((e as Error).message); }
    if (candidate.baseUrl !== config.baseUrl || candidate.validForHours !== config.validForHours) blockers.push("Linux archive configuration differs from the candidate's immutable configuration.");
    if (!(await matchesPromotionBase(context, candidate.promotionBase)) || candidate.promotionBase.revision !== candidate.source.revision) blockers.push("Linux candidate no longer matches its exact promotion base.");
    if (await tagCommit(context, `abandoned/${platform.seriesBranch(candidate.version)}`)) blockers.push("Linux release series is abandoned.");
    if (await tagCommit(context, platform.productionTag(candidate.version))) blockers.push("Linux production version already exists.");
    if (candidate.previousTag) {
      const previous = await readCandidate(storage, candidate.previousTag);
      const relation = await relationship(context, previous.source.revision, candidate.source.revision);
      if (!["same-commit", "descendant"].includes(relation)) blockers.push("Linux candidate lineage is not forward.");
    }
    blockers.push(...acceptanceBlockers(acceptance?.acceptance ?? candidate.acceptance, candidate, true, now));
    try { await verifyEvidence(storage, candidate, acceptance); } catch (e) { blockers.push((e as Error).message); }
  }
  const elapsedHours = receipt ? (now.getTime() - Date.parse(receipt.verifiedAt)) / 3_600_000 : null;
  const satisfied = elapsedHours !== null && elapsedHours >= requiredHours;
  if (!satisfied) blockers.push(`Linux final candidate requires ${requiredHours}h verified soak (${elapsedHours === null ? "unverified" : elapsedHours.toFixed(3) + "h"}).`);
  return { platform: "linux" as const, channels: { staging: platform.stagingChannelTag, production: platform.productionChannelTag }, archive: { backend: config.backend, baseUrl: config.baseUrl, suites: ["staging", "stable"] }, state, candidate, promotion: { allowed: blockers.length === 0, blockers, soak: { requiredHours, elapsedHours, publishedAt: receipt?.verifiedAt ?? null, satisfied } } };
}
export async function linuxReleaseStatus(input: LinuxContext & { acceptance?: string }) {
  try {
    const config = linuxReleaseConfig(input.env);
    const acceptance = readLinuxAcceptance(input.acceptance);
    const storage = linuxArchiveStorage(config);
    await fetchLinux(input);
    return await storage.withExclusivePublication(() => promotionStatus(input, storage, config, acceptance, new Date()));
  } catch (e) {
    return { platform: "linux", channels: { staging: platform.stagingChannelTag, production: platform.productionChannelTag }, promotion: { allowed: false, blockers: [(e as Error).message] } };
  }
}
export async function shipLinuxRelease(input: LinuxReleaseInput) {
  if (input.major || input.minor || input.patch || input.arm64 || input.x86_64 || input.rollbackTo || input.overrideSoak || (input.promoteFrom && input.stagingIteration !== undefined)) throw new Error("Linux requires both architectures, committed VERSION and its own candidate; bump, architecture-only, rollback and soak override selectors are unsupported.");
  if (!input.promoteFrom && (!input.staging || input.production)) throw new Error("Linux ship requires --staging. Production requires release promote --platform linux <exact-staging-version>.");
  const prepared = input.preparedManifest !== undefined || input.sourceRef !== undefined || input.promotionBase !== undefined;
  if (prepared && (!input.preparedManifest || !/^[a-f0-9]{40}$/.test(input.sourceRef ?? "") || input.promotionBase !== input.sourceRef || !input.stagingIteration || input.skipBuild || input.promoteFrom)) throw new Error("Prepared Linux ship requires --prepared-manifest, exact --source-ref and matching --promotion-base, plus --staging-iteration; skip-build and promotion selectors are incompatible.");
  const config = linuxReleaseConfig(input.env);
  const key = publicKey(config);
  const acceptance = readLinuxAcceptance(input.acceptance);
  let signer: Awaited<ReturnType<typeof createAptPublicationSigner>> | undefined;
  let signerProblem: string | null = null;
  try {
    if (!config.privateKeyPath) throw new Error("Missing KANNA_LINUX_APT_PRIVATE_KEY_PATH on the trusted release host.");
    signer = await createAptPublicationSigner({ ...key, privateKey: readLinuxKeyFile(config.privateKeyPath, true), passphrase: config.passphrasePath ? readLinuxKeyFile(config.passphrasePath, true).replace(/\r?\n$/, "") : undefined, now: () => new Date() });
  } catch (error) { signerProblem = (error as Error).message; }
  if (input.release && !input.dryRun && signerProblem) throw new Error(`Linux apt preflight failed: ${signerProblem}`);
  const controller = await cleanLinuxSource(input.repoRoot, input.env, input.runner);
  await fetchLinux(input);
  const storage = linuxArchiveStorage(config);
  return storage.withExclusivePublication(async () => {
    const state = await archiveState(storage);
    if (state.pendingRenewal) throw new Error("Recover pending Linux metadata renewal with release renew first.");
    let staging: LinuxCandidate | null = state.staging ? await readCandidate(storage, state.staging) : null;
    const pinned = prepared || (!!input.promoteFrom && staging?.promotionBase.kind === "commit");
    if (pinned && input.skipBuild) throw new Error("Pinned Linux promotion requires a fresh production build.");
    const execute = async (product: LinuxContext & { source: typeof controller }) => {
      const source = product.source;
      const version = readFileSync(join(product.repoRoot, "VERSION"), "utf8").trim();
      if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error("Linux VERSION must be X.Y.Z.");
      let promoteFrom: string | null = null;
      let branch = input.branch ?? "main";
      let iteration: number | undefined;
      let tag: string;
      const now = new Date();
      const pending = state.pending ? await readCandidate(storage, state.pending) : null;
      if (input.promoteFrom) {
        promoteFrom = input.promoteFrom.startsWith("linux-v") ? input.promoteFrom : `linux-v${input.promoteFrom}`;
        if (!staging || promoteFrom !== staging.tag || source.revision !== staging.source.revision || source.tree !== staging.source.tree || version !== staging.version) throw new Error("Linux promotion must rebuild the exact active soaked source/tree/version.");
        const status = await promotionStatus(input, storage, config, acceptance, now);
        // A pending production retry may have already committed apt or its tag.
        // Its immutable provenance is checked below; waive only recovery blockers.
        const blockers = status.promotion.blockers.filter(b => !(pending?.promotedFrom === staging!.tag && (b.startsWith("Incomplete Linux publication") || b === "Linux production version already exists.")));
        if (blockers.length) throw new Error(blockers.join("\n"));
        branch = staging.promotionBase.branch;
        tag = platform.productionTag(version);
      } else {
        if (input.branch && input.branch !== "main" && input.branch !== platform.seriesBranch(version)) throw new Error("Linux release branch series must match VERSION.");
        const tags = (await run(input, "git", ["tag", "--list", `linux-v${version}-staging.*`])).split("\n");
        const numbers = tags.map(t => Number(t.match(/-staging\.(\d+)$/)?.[1] ?? 0));
        iteration = input.stagingIteration ?? (pending?.channel === platform.stagingChannelTag ? pending.iteration! : Math.max(0, ...numbers, staging?.version === version ? staging.iteration! : 0) + 1);
        if (!Number.isSafeInteger(iteration) || iteration < 1) throw new Error("Linux staging iteration must be a positive integer.");
        tag = platform.stagingTag(version, iteration);
        if (staging && staging.tag !== tag) {
          // InRelease can commit before state.staging advances. Only the matching
          // pending successor's exact cached signature may replace the predecessor
          // here; publishLinuxCandidate revalidates its inputs, signature and archive
          // closure before clearing pending. Public readback still owns the receipt.
          const recovering = pending?.tag === tag && pending.previousTag === staging.tag;
          const intended = recovering ? await storage.read(releasePath(tag, "InRelease")) : null;
          const live = intended ? await storage.read(inReleasePath(staging.channel)) : null;
          if (!intended || !live || sha256(intended) !== sha256(live)) await verifyLinuxPublication(storage, staging, key, now);
          await checkProjection(input, staging, await readJson<LinuxPublicationReceipt>(storage, releasePath(staging.tag, "publication.json")) as LinuxPublicationReceipt);
          const gate = evaluateStagingPublishGate({ platform: "linux", proposedSourceBranch: branch, proposedCommit: source.revision, active: { version: `${staging.version}-staging.${staging.iteration}`, tag: staging.tag, commit: staging.source.revision, sourceBranch: staging.promotionBase.branch, publishedAt: null }, relationship: await relationship(input, staging.source.revision, source.revision), activeProductionTagExists: !!await tagCommit(input, platform.productionTag(staging.version)), activeMetadataError: null, reset: null, postPromotion: null });
          if (!gate.allowed) throw new Error(gate.frozenBy
            ? `Linux staging is frozen to unpromoted ${gate.frozenBy}; ship from that branch or, after acceptance and soak, use kd release promote ${staging.version}-staging.${staging.iteration} --platform linux.`
            : `Linux staging lineage refuses ${source.revision}: it must contain active ${staging.tag} (${staging.source.revision}). No rollback/reset is supported.`);
          if (compareVersions(version, staging.version) < 0 || (version === staging.version && iteration <= staging.iteration!)) throw new Error("Linux staging version/iteration must advance.");
        }
        const production = state.production ? await readCandidate(storage, state.production) : null;
        if (production && compareVersions(version, production.version) <= 0) throw new Error("Linux candidate must advance beyond the Linux production version.");
      }
      if (pending && pending.tag !== tag) throw new Error(`Recover pending Linux publication ${pending.tag} first.`);
      const promotionBase: LinuxCandidate["promotionBase"] = { branch, revision: source.revision, ...(pinned ? { kind: "commit" as const } : {}) };
      if (!(await matchesPromotionBase(input, promotionBase))) throw new Error(`Linux source no longer matches origin/${branch} promotion base.`);
      if (await tagCommit(input, `abandoned/${platform.seriesBranch(version)}`)) throw new Error("Linux release series is abandoned.");
      const slug = releaseRepoSlug(await run(input, "git", ["remote", "get-url", "origin"]));
      const selectedChannel = input.promoteFrom ? platform.productionChannelTag : platform.stagingChannelTag;
      const remoteChannel = await releaseView(input, slug, selectedChannel);
      const expectedTag = input.promoteFrom ? state.production : state.staging;
      if (remoteChannel) {
        const projected = JSON.parse(remoteChannel.body);
        if (projected.platform !== "linux" || ![expectedTag, pending?.tag].filter(Boolean).includes(projected.tag)) throw new Error("Linux GitHub channel does not match archive provenance.");
      } else if (expectedTag && expectedTag !== pending?.tag) throw new Error("Linux GitHub channel projection is missing.");
      if (!input.promoteFrom) {
        const productionVersions = (await run(input, "git", ["tag", "--list", "linux-v*"])).split("\n").flatMap(t => /^linux-v(\d+\.\d+\.\d+)$/.exec(t)?.slice(1) ?? []);
        if (productionVersions.some(v => compareVersions(version, v) <= 0)) throw new Error("Linux candidate must advance beyond existing Linux production tags.");
      }
      const channel = input.promoteFrom ? "production" : "staging";
      const artifacts = prepared
        ? readLinuxPrepared(input.preparedManifest!, { repoRoot: product.repoRoot, source, version, iteration: iteration! })
        : await collectLinuxRelease({ ...input, ...product, source, version, channel, iteration });
      const existingCandidate = await storage.read(candidatePath(tag));
      const old = existingCandidate ? await readCandidate(storage, tag) : null;
      const aptChannel = channel === "staging" ? "desktop-linux-staging" : "desktop-linux";
      const live = await storage.read(inReleasePath(aptChannel));
      const candidate: LinuxCandidate = old ?? {
        schemaVersion: 1, platform: "linux", tag, channel: aptChannel, source, version, iteration: iteration ?? null,
        promotionBase, artifacts: artifacts.map(a => a.identity), aptArtifacts: artifacts.map(a => a.publication.artifact),
        preparedAt: new Date().toISOString(), validForHours: config.validForHours, fingerprint: config.fingerprint, baseUrl: config.baseUrl,
        previousTag: channel === "staging" ? state.staging : state.production, previousInReleaseSha256: live ? sha256(live) : null, promotedFrom: promoteFrom,
        acceptance: acceptance?.acceptance ?? (input.promoteFrom ? staging!.acceptance : null),
      };
      if (JSON.stringify(candidate.source) !== JSON.stringify(source) || JSON.stringify(candidate.artifacts) !== JSON.stringify(artifacts.map(a => a.identity)) || candidate.baseUrl !== config.baseUrl || candidate.fingerprint !== config.fingerprint || candidate.validForHours !== config.validForHours || JSON.stringify(candidate.promotionBase) !== JSON.stringify(promotionBase) || candidate.promotedFrom !== promoteFrom || (acceptance && JSON.stringify(candidate.acceptance) !== JSON.stringify(acceptance.acceptance))) throw new Error("Retry differs from immutable Linux candidate inputs; refusing replacement.");
      const subject = input.promoteFrom ? staging! : candidate;
      await verifyEvidence(storage, subject, acceptance);
      const blockers = acceptanceBlockers(acceptance?.acceptance ?? subject.acceptance, subject, !!input.promoteFrom, new Date());
      if (signerProblem) blockers.push(signerProblem);
      const plan = { platform: "linux", tag, controller, source, version, iteration, channel: aptChannel, artifacts: artifacts.map(a => a.identity), promotionBase: candidate.promotionBase, publication: { allowed: blockers.length === 0, blockers }, published: false };
      if (input.dryRun || !input.release) return plan;
      if (blockers.length) throw new Error(blockers.join("\n"));
      if (!signer) throw new Error("Linux apt signer is unavailable.");
      if (!(await matchesPromotionBase(input, promotionBase)) || JSON.stringify(await cleanLinuxSource(input.repoRoot, input.env, input.runner)) !== JSON.stringify(controller) || JSON.stringify(await cleanLinuxSource(product.repoRoot, product.env, product.runner)) !== JSON.stringify(source)) throw new Error("Linux promotion base/source moved during build.");
      if (input.promoteFrom && acceptance) for (const [hash, bytes] of Object.entries(acceptance.evidence)) await immutable(storage, `linux/evidence/${hash}`, bytes);
      const receipt = await publishLinuxCandidate({ candidate, artifacts, acceptance, storage, signer, key, observeCommit: c => publicReadback(storage, c), now: () => new Date(), project: (c, r) => projectGithub(input, c, r) });
      return { ...plan, published: true, receipt };
    };
    return pinned
      ? withLinuxSource({ ...input, ref: prepared ? input.sourceRef! : staging!.source.revision }, execute)
      : execute({ ...input, source: controller });
  });
}

/** Explicit metadata-only publication; no source checkout, build or tag change. */
export async function renewLinuxRelease(input: LinuxContext & { candidate: string; renewal: number; validForHours: number }) {
  const config = linuxReleaseConfig(input.env);
  if (!config.privateKeyPath) throw new Error("Missing KANNA_LINUX_APT_PRIVATE_KEY_PATH on the trusted release host.");
  const key = publicKey(config);
  const signer = await createAptPublicationSigner({ ...key, privateKey: readLinuxKeyFile(config.privateKeyPath, true), passphrase: config.passphrasePath ? readLinuxKeyFile(config.passphrasePath, true).replace(/\r?\n$/, "") : undefined, now: () => new Date() });
  const storage = linuxArchiveStorage(config);
  return storage.withExclusivePublication(async () => {
    const candidate = await readCandidate(storage, input.candidate);
    if (candidate.baseUrl !== config.baseUrl || candidate.fingerprint.toLowerCase() !== config.fingerprint.toLowerCase()) throw new Error("Renewal archive URL/key differs from candidate configuration.");
    return renewLinuxCandidate({ storage, candidate, sequence: input.renewal, validForHours: input.validForHours, signer, key, now: () => new Date(), observeCommit: (c, paths) => publicReadback(storage, c, [releasePath(c.tag, "InRelease"), ...paths]), project: (c, r) => projectGithub(input, c, r) });
  });
}
