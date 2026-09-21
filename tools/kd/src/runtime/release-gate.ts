import { mustRun, releaseRepoSlug, stagingTag, type ReleaseCommandContext } from "./release-command";
import {
  compareVersions,
  parsePromotionVersions,
  readAbandonedSeries,
  readGreatestProductionVersion,
  releaseSeriesBranch,
  releaseSeriesFromVersion
} from "./release-version";
import {
  STAGING_CHANNEL_TAG,
  activeProductionTagExists,
  listStagingCandidateTags,
  parseRemoteTagCommit,
  productionVersionForStaging,
  readStagingCandidate,
  readStagingChannelBody,
  resolveActiveStagingCandidate
} from "./release-channel";
import {
  evaluateCandidateLineage,
  evaluatePromotionGate,
  evaluateSoak,
  evaluateStagingPublishGate,
  isReleaseBranchName,
  normalizeStagingVersion,
  parseLineageRecutApplicationRecords,
  parseLineageRecutRecords,
  parseLineageResetRecords,
  parsePostPromotionTrunkRecords,
  promotionAuthorizes,
  recutApplicationAuthorizes,
  resetAuthorizes,
  type CandidateLineage,
  type LineageRecutApplicationRecord,
  type LineageRecutRecord,
  type LineageResetRecord,
  type PostPromotionTrunkRecord,
  type SoakEvaluation,
  type StagingCandidate,
  type StagingLineageRelationship
} from "./release-lineage";
import { DEFAULT_RELEASE_POLICY, parseReleasePolicy, readReleasePolicy, type ReleasePolicy } from "./release-policy";

interface LineageAudit {
  /** Newest records remain the live staging-publication state. */
  reset: LineageResetRecord | null;
  recut: LineageRecutRecord | null;
  recutApplication: LineageRecutApplicationRecord | null;
  postPromotion: PostPromotionTrunkRecord | null;
  /** Full history is used only to assess an already-published immutable RC. */
  resets: LineageResetRecord[];
  recuts: LineageRecutRecord[];
  recutApplications: LineageRecutApplicationRecord[];
  postPromotions: PostPromotionTrunkRecord[];
}

export const EMPTY_LINEAGE_AUDIT: LineageAudit = {
  reset: null,
  recut: null,
  recutApplication: null,
  postPromotion: null,
  resets: [],
  recuts: [],
  recutApplications: [],
  postPromotions: []
};

export async function readLineageAudit(
  context: ReleaseCommandContext,
  repoSlug: string
): Promise<LineageAudit> {
  const body = await readStagingChannelBody(context, repoSlug);
  const resets = parseLineageResetRecords(body);
  const recuts = parseLineageRecutRecords(body);
  const recutApplications = parseLineageRecutApplicationRecords(body);
  const postPromotions = parsePostPromotionTrunkRecords(body);
  return {
    reset: resets[0] ?? null,
    recut: recuts[0] ?? null,
    recutApplication: recutApplications[0] ?? null,
    postPromotion: postPromotions[0] ?? null,
    resets,
    recuts,
    recutApplications,
    postPromotions
  };
}

async function commitRelationship(
  context: ReleaseCommandContext,
  base: string | null,
  candidate: string | null
): Promise<StagingLineageRelationship> {
  if (!base || !candidate) return "unknown";
  if (base === candidate) return "same-commit";
  const forward = await context.runner.run("git", ["merge-base", "--is-ancestor", base, candidate], {
    cwd: context.repoRoot,
    env: context.env
  });
  if (forward.exitCode === 0) return "descendant";
  // git exits 1 for "not an ancestor" and something else (128) when a commit is
  // missing locally; only the former is a real answer.
  if (forward.exitCode !== 1) return "unknown";
  const backward = await context.runner.run("git", ["merge-base", "--is-ancestor", candidate, base], {
    cwd: context.repoRoot,
    env: context.env
  });
  if (backward.exitCode === 0) return "behind";
  if (backward.exitCode !== 1) return "unknown";
  return "diverged";
}

async function fetchStagingHistory(context: ReleaseCommandContext): Promise<void> {
  // Best effort: lineage comparison needs the RC commits locally, and every RC
  // is tagged. A failure here degrades a relationship to "unknown", which the
  // gates already treat as a refusal rather than a pass.
  await context.runner.run("git", ["fetch", "--tags", "origin"], { cwd: context.repoRoot, env: context.env });
}

/**
 * Resolve the narrow post-promotion hand-back to main. Every fact is checked
 * here before the pure lineage gate receives an authorization record:
 * production release metadata and both tag resolutions prove the tag is the RC
 * promotion (directly, or at kd's exact version-bump child), the recorded
 * release branch still resolves, and the proposed trunk commit is forward of
 * that branch's merge-base with origin/main.
 */
async function resolvePostPromotionTrunkRecord(
  context: ReleaseCommandContext & { now?: number },
  repoSlug: string,
  active: StagingCandidate,
  proposed: { sourceBranch: string; commit: string }
): Promise<PostPromotionTrunkRecord | null> {
  const activeSourceBranch = active.sourceBranch;
  if (proposed.sourceBranch !== "main" || !active.commit || !activeSourceBranch || !isReleaseBranchName(activeSourceBranch)) {
    return null;
  }
  const productionVersion = productionVersionForStaging(active.version);
  if (!productionVersion) return null;
  const productionTag = `v${productionVersion}`;

  try {
    const releaseView = await context.runner.run(
      "gh",
      ["release", "view", productionTag, "--repo", repoSlug, "--json", "tagName,targetCommitish,isPrerelease"],
      { cwd: context.repoRoot, env: context.env }
    );
    if (releaseView.exitCode !== 0) return null;
    const parsed = JSON.parse(releaseView.stdout) as unknown;
    if (typeof parsed !== "object" || parsed === null) return null;
    const metadata = parsed as { tagName?: unknown; targetCommitish?: unknown; isPrerelease?: unknown };
    if (
      metadata.tagName !== productionTag ||
      metadata.isPrerelease !== false ||
      typeof metadata.targetCommitish !== "string" ||
      metadata.targetCommitish.trim().length === 0
    ) {
      return null;
    }

    const tagRefs = await mustRun(
      context.runner,
      "git",
      ["ls-remote", "--tags", "origin", `refs/tags/${productionTag}`, `refs/tags/${productionTag}^{}`],
      context.repoRoot,
      context.env
    );
    const productionTagCommit = parseRemoteTagCommit(tagRefs, productionTag)?.toLowerCase() ?? null;
    if (!productionTagCommit) return null;
    await mustRun(
      context.runner,
      "git",
      ["fetch", "--no-tags", "origin", `refs/tags/${productionTag}`],
      context.repoRoot,
      context.env
    );
    const fetchedCommit = await mustRun(
      context.runner,
      "git",
      ["rev-parse", "FETCH_HEAD^{commit}"],
      context.repoRoot,
      context.env
    );
    if (fetchedCommit.toLowerCase() !== productionTagCommit) return null;
    if (productionTagCommit !== active.commit.toLowerCase()) {
      const parentLine = await mustRun(
        context.runner,
        "git",
        ["show", "-s", "--format=%P", "FETCH_HEAD"],
        context.repoRoot,
        context.env
      );
      const parents = parentLine.split(/\s+/).filter(Boolean);
      const subject = await mustRun(
        context.runner,
        "git",
        ["show", "-s", "--format=%s", "FETCH_HEAD"],
        context.repoRoot,
        context.env
      );
      if (parents.length !== 1 || parents[0]?.toLowerCase() !== active.commit.toLowerCase() || subject !== `release: ${productionTag}`) {
        return null;
      }
    }

    await mustRun(
      context.runner,
      "git",
      ["fetch", "origin", "main", activeSourceBranch],
      context.repoRoot,
      context.env
    );
    const originMain = await mustRun(context.runner, "git", ["rev-parse", "origin/main"], context.repoRoot, context.env);
    const originRelease = await mustRun(
      context.runner,
      "git",
      ["rev-parse", `origin/${activeSourceBranch}`],
      context.repoRoot,
      context.env
    );
    const mergeBase = await mustRun(
      context.runner,
      "git",
      ["merge-base", originMain, originRelease],
      context.repoRoot,
      context.env
    );
    if (!/^[0-9a-f]{40}$/i.test(mergeBase)) return null;
    const releaseContainsCandidate = await context.runner.run(
      "git",
      ["merge-base", "--is-ancestor", active.commit, originRelease],
      { cwd: context.repoRoot, env: context.env }
    );
    if (releaseContainsCandidate.exitCode !== 0) return null;
    const mainContainsBase = await context.runner.run(
      "git",
      ["merge-base", "--is-ancestor", mergeBase, originMain],
      { cwd: context.repoRoot, env: context.env }
    );
    if (mainContainsBase.exitCode !== 0) return null;
    const proposedDescendsFromBase = await context.runner.run(
      "git",
      ["merge-base", "--is-ancestor", mergeBase, proposed.commit],
      { cwd: context.repoRoot, env: context.env }
    );
    if (proposedDescendsFromBase.exitCode !== 0) return null;

    return {
      resumedAt: new Date(context.now ?? Date.now()).toISOString(),
      promotedVersion: productionVersion,
      promotedTag: productionTag,
      promotedCommit: active.commit.toLowerCase(),
      productionTagCommit,
      newCommit: proposed.commit.toLowerCase(),
      newBranch: proposed.sourceBranch
    };
  } catch (error) {
    throw new Error(
      `Could not verify whether ${productionTag} authorizes post-promotion trunk resumption: ` +
        (error instanceof Error ? error.message : String(error))
    );
  }
}

/**
 * The candidate published immediately before `tag` on the same channel. This is
 * what makes a divergence visible: the incident's v0.1.0-staging.8 was
 * mechanically aligned to its branch while sharing only an ancient merge base
 * with v0.1.0-staging.7.
 */
async function resolvePreviousCandidate(
  context: ReleaseCommandContext,
  repoSlug: string,
  tag: string
): Promise<{ previous: StagingCandidate | null; found: boolean }> {
  const tags = await listStagingCandidateTags(context, repoSlug);
  const index = tags.indexOf(tag);
  if (index < 0) return { previous: null, found: false };
  const previousTag = tags[index + 1];
  if (!previousTag) return { previous: null, found: true };
  const lookup = await readStagingCandidate(context, repoSlug, previousTag.replace(/^v/, ""));
  return { previous: lookup.candidate, found: true };
}

async function resolveCandidateLineage(
  context: ReleaseCommandContext,
  repoSlug: string,
  candidate: StagingCandidate,
  audit: LineageAudit
): Promise<CandidateLineage> {
  const candidateRecutApplication = audit.recutApplications.find((application) =>
    normalizeStagingVersion(application.version) === normalizeStagingVersion(candidate.version) &&
    Boolean(candidate.commit) && application.commit.toLowerCase() === candidate.commit?.toLowerCase()
  ) ?? null;
  const recut = candidateRecutApplication
    ? audit.recuts.find((record) => record.recutId === candidateRecutApplication.recutId) ?? null
    : audit.recut;
  // Consumption belongs to the grant, not to whichever candidate is currently
  // being assessed. Keep an exact application as durable evidence for its RC,
  // but never let a later candidate reuse that grant after the branch advances.
  const recutApplication = candidateRecutApplication ?? (recut
    ? audit.recutApplications.find((application) => application.recutId === recut.recutId) ?? null
    : null);
  let recutDestinationRelationship: StagingLineageRelationship | undefined;
  let recutDestinationIsBranchTip: boolean | undefined;
  // The unused publication grant remains bound to today's branch tip. Once
  // applied, its exact candidate record is historical evidence and must not be
  // invalidated merely because that branch has advanced to a newer RC.
  if (!recutApplication && recut && candidate.sourceBranch === recut.branch && candidate.commit) {
    const branchRefs = await context.runner.run(
      "git",
      ["ls-remote", "origin", `refs/heads/${recut.branch}`],
      { cwd: context.repoRoot, env: context.env }
    );
    const branchTip = branchRefs.exitCode === 0 ? branchRefs.stdout.trim().split(/\s+/)[0] ?? "" : "";
    recutDestinationIsBranchTip = branchTip.length > 0 && branchTip.toLowerCase() === candidate.commit.toLowerCase();
    if (recutDestinationIsBranchTip) {
      recutDestinationRelationship = await commitRelationship(context, recut.newTip, candidate.commit);
    }
  }
  const recutPrevious = recut && recut.fromVersion && candidate.version !== recut.fromVersion &&
      candidate.sourceBranch === recut.branch &&
      (recutApplicationAuthorizes(recut, recutApplication, candidate) || (!recutApplication && recutDestinationIsBranchTip))
    ? {
        version: recut.fromVersion,
        tag: stagingTag(recut.fromVersion),
        commit: recut.fromCommit
      }
    : null;
  if (recutPrevious) {
    const recutRelationship = await commitRelationship(context, recutPrevious.commit, candidate.commit);
    return evaluateCandidateLineage({
      candidate,
      previous: recutPrevious,
      relationship: recutRelationship,
      reset: audit.reset,
      recut,
      recutApplication,
      recutDestinationRelationship,
      recutDestinationIsBranchTip,
      postPromotion: audit.postPromotion
    });
  }
  const { previous, found } = await resolvePreviousCandidate(context, repoSlug, candidate.tag);
  if (!found) {
    return {
      relationship: "unknown",
      previous: null,
      valid: false,
      authorizedByReset: false,
      authorizedByPromotion: false,
      authorizedByRecut: false,
      reset: audit.reset,
      recut,
      recutApplication,
      postPromotion: audit.postPromotion,
      detail: `${candidate.tag} is not listed among this repository's staging prereleases, so its lineage cannot be established.`
    };
  }
  if (!previous) {
    return evaluateCandidateLineage({
      candidate,
      previous: null,
      relationship: "initial",
      reset: audit.reset,
      recut,
      recutApplication,
      postPromotion: audit.postPromotion
    });
  }
  const relationship = await commitRelationship(context, previous.commit, candidate.commit);
  const reset = audit.resets.find((record) => resetAuthorizes(record, {
    fromVersion: previous.version,
    toBranch: candidate.sourceBranch ?? ""
  })) ?? audit.reset;
  const postPromotion = audit.postPromotions.find((record) => promotionAuthorizes(record, {
    fromVersion: previous.version,
    fromCommit: previous.commit,
    toCommit: candidate.commit,
    toBranch: candidate.sourceBranch
  })) ?? audit.postPromotion;
  return evaluateCandidateLineage({
    candidate,
    previous: { version: previous.version, tag: previous.tag, commit: previous.commit },
    relationship,
    reset,
    recut,
    recutApplication,
    recutDestinationRelationship,
    recutDestinationIsBranchTip,
    postPromotion
  });
}

/**
 * Refuses a staging publish that would move the channel non-linearly or ship
 * against unverifiable channel metadata. Runs
 * before any build so a refusal costs seconds, not a signed build.
 */
export async function assertStagingPublishAllowed(
  input: ReleaseCommandContext,
  proposed: { sourceBranch: string; commit: string; branchTip: string | null }
): Promise<{ active: StagingCandidate | null; postPromotion: PostPromotionTrunkRecord | null; recut: LineageRecutRecord | null; repoSlug: string }> {
  const remoteUrl = await mustRun(input.runner, "git", ["remote", "get-url", "origin"], input.repoRoot, input.env);
  const repoSlug = releaseRepoSlug(remoteUrl);
  const active = await resolveActiveStagingCandidate(input, repoSlug);
  // A candidate we could not read still has to reach the gate: only a channel
  // positively known to be empty skips the comparison.
  if (!active.candidate && !active.error) return { active: null, postPromotion: null, recut: null, repoSlug };

  if (active.candidate) await fetchStagingHistory(input);
  const relationship = active.candidate
    ? await commitRelationship(input, active.candidate.commit, proposed.commit)
    : "unknown";
  const productionTagPresent = active.candidate
    ? await activeProductionTagExists(input, active.candidate.version)
    : false;
  const postPromotion = active.candidate && relationship === "diverged" && productionTagPresent
    ? await resolvePostPromotionTrunkRecord(input, repoSlug, active.candidate, proposed)
    : null;
  const audit = active.candidate ? await readLineageAudit(input, repoSlug) : EMPTY_LINEAGE_AUDIT;
  const recutDestinationRelationship = audit.recut && proposed.sourceBranch === audit.recut.branch
    ? await commitRelationship(input, audit.recut.newTip, proposed.commit)
    : undefined;
  const decision = evaluateStagingPublishGate({
    proposedSourceBranch: proposed.sourceBranch,
    proposedCommit: proposed.commit,
    active: active.candidate,
    relationship,
    activeProductionTagExists: productionTagPresent,
    activeMetadataError: active.error,
    reset: audit.reset,
    recut: audit.recut,
    recutApplication: audit.recutApplication,
    recutDestinationRelationship,
    recutDestinationIsBranchTip: proposed.branchTip === null ? undefined : proposed.branchTip === proposed.commit,
    postPromotion
  });
  if (!decision.allowed) {
    throw new Error(decision.reason ?? `Refusing to repoint ${STAGING_CHANNEL_TAG}.`);
  }
  return {
    active: active.candidate,
    postPromotion: decision.authorizedByPromotion ? postPromotion : null,
    recut: decision.authorizedByRecut ? audit.recut : null,
    repoSlug
  };
}

interface PromotionAssessment {
  lineage: CandidateLineage;
  soak: SoakEvaluation;
  gate: ReturnType<typeof evaluatePromotionGate>;
  policy: ReleasePolicy;
}

async function readCandidateReleasePolicy(
  input: ReleaseCommandContext,
  candidate: StagingCandidate
): Promise<ReleasePolicy> {
  if (!candidate.commit) return readReleasePolicy(input.repoRoot);
  const sourceLabel = `${candidate.tag}:release-policy.json`;
  const present = await input.runner.run(
    "git",
    ["cat-file", "-e", `${candidate.commit}:release-policy.json`],
    { cwd: input.repoRoot, env: input.env }
  );
  if (present.exitCode !== 0) {
    return { ...DEFAULT_RELEASE_POLICY, linux: { ...DEFAULT_RELEASE_POLICY.linux } };
  }
  const result = await input.runner.run(
    "git",
    ["show", `${candidate.commit}:release-policy.json`],
    { cwd: input.repoRoot, env: input.env }
  );
  if (result.exitCode !== 0) {
    throw new Error(`Could not read ${sourceLabel}: ${result.stderr.trim() || "git show failed"}`);
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(result.stdout) as unknown;
  } catch (error) {
    throw new Error(`${sourceLabel} is not valid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
  return parseReleasePolicy(parsed, sourceLabel);
}

/**
 * The one promotion decision used by status, dry-run, and a real publication.
 * The selected versioned prerelease supplies identity, historical lineage and
 * publication time; no mutable branch or channel pointer participates.
 */
export async function assessPromotionCandidate(
  input: ReleaseCommandContext & { now?: number; soakOverrideReason?: string },
  repoSlug: string,
  candidate: StagingCandidate
): Promise<PromotionAssessment> {
  const { productionVersion } = parsePromotionVersions(candidate.version);
  await fetchStagingHistory(input);
  const audit = await readLineageAudit(input, repoSlug);
  const lineage = await resolveCandidateLineage(input, repoSlug, candidate, audit);
  const policy = await readCandidateReleasePolicy(input, candidate);
  const soak = evaluateSoak({
    requiredHours: policy.productionSoakHours,
    publishedAt: candidate.publishedAt,
    nowMs: input.now ?? Date.now(),
    overrideReason: input.soakOverrideReason ?? null
  });
  const greatestProductionVersion = await readGreatestProductionVersion(input);
  const productionAdvances =
    greatestProductionVersion === null || compareVersions(productionVersion, greatestProductionVersion) > 0;
  const seriesBranch = releaseSeriesBranch(releaseSeriesFromVersion(productionVersion));
  const abandonedRecord = await readAbandonedSeries(input, seriesBranch);
  const abandonedSeries = abandonedRecord ? { branch: seriesBranch, ...abandonedRecord } : null;
  const gate = evaluatePromotionGate({
    rcTag: candidate.tag,
    rcVersion: candidate.version,
    sourceIdentity: {
      commit: candidate.commit,
      reason: candidate.commit ? null : `${candidate.tag} records no target commit, so its immutable source cannot be resolved.`
    },
    lineage,
    soak,
    abandonedSeries,
    productionVersion: {
      selected: productionVersion,
      greatestPublished: greatestProductionVersion,
      advances: productionAdvances
    }
  });
  return { lineage, soak, gate, policy };
}
