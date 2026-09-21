import type { CommandRunner } from "./process";
import { mustRun, releaseRepoSlug, stagingTag, type ReleaseCommandContext } from "./release-command";
import { readAbandonedSeries, releaseSeriesBranch, releaseSeriesFromVersion, type AbandonedSeriesRecord } from "./release-version";
import {
  STAGING_CHANNEL_TAG,
  activeProductionTagExists,
  readStagingChannel,
  readVerifiedStagingCandidate,
  verifyImmutableStagingCandidate,
  type StagingChannelRead
} from "./release-channel";
import { EMPTY_LINEAGE_AUDIT, readLineageAudit, assessPromotionCandidate } from "./release-gate";
import {
  parseUnmergedReleaseCommits,
  UNMERGED_COMMIT_REPORT_LIMIT,
  type ReleaseBranchCommit
} from "./release-cut";
import {
  evaluateSoak,
  evaluateStagingFreeze,
  normalizeStagingVersion,
  type LineageRecutApplicationRecord,
  type LineageRecutRecord,
  type LineageResetRecord,
  type PostPromotionTrunkRecord,
  type SoakEvaluation,
  type StagingCandidate,
  type StagingLineageRelationship
} from "./release-lineage";
import { readReleasePolicy, type ReleasePolicy } from "./release-policy";

export interface ReleaseStatusInput {
  repoRoot: string;
  env: NodeJS.ProcessEnv;
  runner: CommandRunner;
  /** Fixed clock for soak arithmetic; defaults to `Date.now()`. */
  now?: number;
  /** Exact historical RC to assess. The staging field still reports the live pointer. */
  candidateVersion?: string;
}

export interface ReleaseStatusStaging {
  version: string;
  tag: string;
  commit: string | null;
  sourceBranch: string | null;
  commitsBehindMain: number | null;
  publishedAt: string | null;
  ageHours: number | null;
}

export interface ReleaseStatusReleaseBranch {
  name: string;
  commit: string;
  /** Set when the series was deliberately abandoned instead of released. */
  abandoned: AbandonedSeriesRecord | null;
  /**
   * Commits on the release branch with no patch-equivalent on main. This is
   * provable ancestry/patch-id provenance — un-backported release-only work —
   * not a semantic claim that the branch carries only bugfixes.
   */
  unmergedCommits: ReleaseBranchCommit[] | null;
  unmergedCommitCount: number | null;
  /** Archived recut tags for this release branch, newest ordinal last. */
  recuts: ReleaseStatusRecut[] | null;
}

export interface ReleaseStatusRecut {
  id: string;
  archiveTag: string;
  status: "pending" | "applied" | "incomplete" | "superseded";
  oldTip: string | null;
  newTip: string | null;
}

export interface ReleaseStatusLineage {
  relationship: StagingLineageRelationship;
  previous: { version: string; tag: string; commit: string | null } | null;
  valid: boolean;
  authorizedByReset: boolean;
  authorizedByPromotion: boolean;
  authorizedByRecut: boolean;
  recut: LineageRecutRecord | null;
  reset: LineageResetRecord | null;
  postPromotion: PostPromotionTrunkRecord | null;
  detail: string;
}

export interface ReleaseStatusPromotion {
  /** The immutable candidate this decision assesses (live pointer by default). */
  candidate: StagingCandidate | null;
  /** True when the selected RC's immutable source identity is verified. */
  mechanicallyPromotable: boolean;
  /** Exact immutable source commit; retained as `base` for response compatibility. */
  base: string | null;
  mechanicalReason: string | null;
  soak: SoakEvaluation;
  /** True only when identity, lineage, soak, abandonment, and production-version gates hold. */
  allowed: boolean;
  blockers: string[];
}

export interface ReleaseStatusResult {
  production: { version: string; tag: string; publishedAt: string } | null;
  staging: ReleaseStatusStaging | null;
  releaseBranch: ReleaseStatusReleaseBranch | null;
  commitsOnMainSinceProduction: number | null;
  policy: ReleasePolicy;
  lineage: ReleaseStatusLineage | null;
  freeze: { active: boolean; branch: string | null; reason: string | null; waivedByReset: boolean };
  promotion: ReleaseStatusPromotion;
  promoteCommand: string | null;
}

function parseProductionReleaseView(raw: string): { tag: string; publishedAt: string } | null {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== "object" || parsed === null) return null;
    const record = parsed as { tagName?: unknown; publishedAt?: unknown };
    if (typeof record.tagName !== "string" || record.tagName.length === 0) return null;
    return { tag: record.tagName, publishedAt: typeof record.publishedAt === "string" ? record.publishedAt : "" };
  } catch {
    return null;
  }
}

async function countCommits(input: ReleaseCommandContext, range: string): Promise<number | null> {
  const result = await input.runner.run("git", ["rev-list", "--count", range], { cwd: input.repoRoot, env: input.env });
  if (result.exitCode !== 0) return null;
  const count = Number.parseInt(result.stdout.trim(), 10);
  return Number.isNaN(count) ? null : count;
}

function roundHours(hours: number): number {
  return Math.round(Math.max(0, hours) * 100) / 100;
}

/**
 * Release-branch hygiene kd can actually prove: commits on the branch with no
 * patch-equivalent on main. `--cherry-pick` compares by patch id, so a fix
 * cherry-picked onto the branch from main does not show up, while a fix that
 * only ever landed on the branch does — that is the regression the "fix on main
 * first, then backport" rule exists to prevent. Merges are excluded: a merge
 * carries no patch of its own, so it can be neither backported nor missing.
 * This says nothing about whether the remaining commits are bugfixes; only a
 * human review can claim that.
 */
async function resolveUnmergedReleaseCommits(
  input: ReleaseCommandContext,
  branchName: string
): Promise<{ commits: ReleaseBranchCommit[] | null; count: number | null }> {
  const fetched = await input.runner.run(
    "git",
    ["fetch", "origin", `+refs/heads/${branchName}:refs/remotes/origin/${branchName}`],
    { cwd: input.repoRoot, env: input.env }
  );
  if (fetched.exitCode !== 0) return { commits: null, count: null };
  const log = await input.runner.run(
    "git",
    ["log", "--no-merges", "--cherry-pick", "--right-only", "--format=%H %s", `origin/main...origin/${branchName}`],
    { cwd: input.repoRoot, env: input.env }
  );
  if (log.exitCode !== 0) return { commits: null, count: null };
  const commits = parseUnmergedReleaseCommits(log.stdout);
  return { commits: commits.slice(0, UNMERGED_COMMIT_REPORT_LIMIT), count: commits.length };
}

async function resolveRecutTags(input: ReleaseCommandContext, branchName: string): Promise<string[] | null> {
  const result = await input.runner.run(
    "git",
    ["ls-remote", "--tags", "origin", `recut/${branchName}-*`],
    { cwd: input.repoRoot, env: input.env }
  );
  if (result.exitCode !== 0) return null;
  return result.stdout
    .split(/\r?\n/)
    .map((line) => line.trim().split(/\s+/).at(-1) ?? "")
    .filter((ref) => ref.startsWith("refs/tags/recut/") && !ref.endsWith("^{}"))
    .map((ref) => ref.slice("refs/tags/".length));
}

function describeRecutTags(
  tags: string[] | null,
  record: LineageRecutRecord | null,
  application: LineageRecutApplicationRecord | null
): ReleaseStatusRecut[] | null {
  if (!tags) return null;
  const described: ReleaseStatusRecut[] = tags.map((archiveTag) => {
    const id = archiveTag.slice("recut/".length).replace(/^release\//, "");
    const isApplied = application?.recutId === id;
    const isPending = record?.recutId === id || record?.archiveTag === archiveTag;
    return {
      id,
      archiveTag,
      status: isApplied ? "applied" : isPending ? "pending" : record ? "superseded" : "incomplete",
      oldTip: isPending ? record?.oldTip ?? null : null,
      newTip: isPending ? record?.newTip ?? null : null
    };
  });
  if (record && !tags.includes(record.archiveTag)) {
    described.unshift({ id: record.recutId, archiveTag: record.archiveTag, status: "incomplete", oldTip: record.oldTip, newTip: record.newTip });
  }
  return described;
}

export async function releaseStatus(input: ReleaseStatusInput): Promise<ReleaseStatusResult> {
  const nowMs = input.now ?? Date.now();
  let policy = readReleasePolicy(input.repoRoot);
  const remoteUrl = await mustRun(input.runner, "git", ["remote", "get-url", "origin"], input.repoRoot, input.env);
  const repoSlug = releaseRepoSlug(remoteUrl);
  await mustRun(input.runner, "git", ["fetch", "--tags", "origin", "main"], input.repoRoot, input.env);

  let production: ReleaseStatusResult["production"] = null;
  const productionView = await input.runner.run("gh", ["release", "view", "--repo", repoSlug, "--json", "tagName,publishedAt"], {
    cwd: input.repoRoot,
    env: input.env
  });
  if (productionView.exitCode === 0) {
    const parsed = parseProductionReleaseView(productionView.stdout);
    if (parsed) {
      production = { version: parsed.tag.replace(/^v/, ""), tag: parsed.tag, publishedAt: parsed.publishedAt };
    }
  }

  let staging: ReleaseStatusStaging | null = null;
  let activeCandidate: StagingCandidate | null = null;
  const channel = await readStagingChannel(input, repoSlug);
  // An unreadable channel must never be reported as an empty one: "no candidate
  // is active" reads as a calm all-clear, and the operator would act on it.
  const channelError = channel.state === "unreadable" ? channel.error : null;
  let activeCandidateIntegrityError: string | null = channelError;
  const version = channel.state === "active" ? channel.version : null;
  if (version) {
    const lookup = await readVerifiedStagingCandidate(input, repoSlug, version);
    activeCandidate = lookup.candidate;
    activeCandidateIntegrityError = lookup.error;
    if (activeCandidate && !activeCandidateIntegrityError) {
      try {
        await verifyImmutableStagingCandidate(input, repoSlug, activeCandidate);
      } catch (error) {
        activeCandidateIntegrityError = error instanceof Error ? error.message : String(error);
      }
    }
    const publishedAt = activeCandidate?.publishedAt ?? null;
    const publishedMs = publishedAt ? Date.parse(publishedAt) : Number.NaN;
    staging = {
      version,
      tag: stagingTag(version),
      commit: activeCandidate?.commit ?? null,
      sourceBranch: activeCandidate?.sourceBranch ?? null,
      commitsBehindMain: activeCandidate?.commit
        ? await countCommits(input, `${activeCandidate.commit}..origin/main`)
        : null,
      publishedAt,
      ageHours: Number.isNaN(publishedMs) ? null : roundHours((nowMs - publishedMs) / 3_600_000)
    };
  }

  const requestedCandidateVersion = input.candidateVersion
    ? normalizeStagingVersion(input.candidateVersion)
    : null;
  let promotionCandidate = activeCandidate;
  let promotionCandidateIntegrityError = activeCandidateIntegrityError;
  if (requestedCandidateVersion && requestedCandidateVersion !== activeCandidate?.version) {
    const lookup = await readVerifiedStagingCandidate(input, repoSlug, requestedCandidateVersion);
    promotionCandidate = lookup.candidate;
    promotionCandidateIntegrityError = lookup.error;
    if (promotionCandidate && !promotionCandidateIntegrityError) {
      try {
        await verifyImmutableStagingCandidate(input, repoSlug, promotionCandidate);
      } catch (error) {
        promotionCandidateIntegrityError = error instanceof Error ? error.message : String(error);
      }
    }
  }

  let releaseBranch: ReleaseStatusReleaseBranch | null = null;
  if (promotionCandidate) {
    const branchName = releaseSeriesBranch(releaseSeriesFromVersion(promotionCandidate.version));
    const branchRefs = await input.runner.run("git", ["ls-remote", "origin", `refs/heads/${branchName}`], {
      cwd: input.repoRoot,
      env: input.env
    });
    const branchSha = branchRefs.exitCode === 0 ? branchRefs.stdout.trim().split(/\s+/)[0] ?? "" : "";
    if (branchSha) {
      const unmerged = await resolveUnmergedReleaseCommits(input, branchName);
      const audit = staging && activeCandidate ? await readLineageAudit(input, repoSlug) : EMPTY_LINEAGE_AUDIT;
      releaseBranch = {
        name: branchName,
        commit: branchSha,
        abandoned: await readAbandonedSeries(input, branchName),
        unmergedCommits: unmerged.commits,
        unmergedCommitCount: unmerged.count,
        recuts: describeRecutTags(await resolveRecutTags(input, branchName), audit.recut, audit.recutApplication)
      };
    }
  }

  const commitsOnMainSinceProduction = production ? await countCommits(input, `${production.tag}..origin/main`) : null;

  let lineage: ReleaseStatusLineage | null = null;
  let freeze: ReleaseStatusResult["freeze"] = {
    active: false,
    branch: null,
    reason: null,
    waivedByReset: false
  };
  const promotion: ReleaseStatusPromotion = {
    candidate: promotionCandidate,
    mechanicallyPromotable: false,
    base: null,
    mechanicalReason: null,
    soak: evaluateSoak({ requiredHours: policy.productionSoakHours, publishedAt: null, nowMs }),
    allowed: false,
    blockers: [
      !requestedCandidateVersion && channelError
        ? `The ${STAGING_CHANNEL_TAG} channel could not be read (${channelError}), so no promotion decision can be made.`
        : promotionCandidateIntegrityError
        ? `The selected staging candidate failed immutable identity verification: ${promotionCandidateIntegrityError}`
        : requestedCandidateVersion
        ? `The selected staging candidate v${requestedCandidateVersion} could not be resolved.`
        : "No staging release candidate is active on the channel."
    ]
  };

  if (promotionCandidate) {
    const audit = await readLineageAudit(input, repoSlug);
    if (releaseBranch?.recuts && audit.recut) {
      releaseBranch.recuts = releaseBranch.recuts.map((recut) =>
        recut.archiveTag === audit.recut?.archiveTag
          ? { ...recut, id: audit.recut.recutId, status: audit.recutApplication?.recutId === audit.recut.recutId ? "applied" : "pending", oldTip: audit.recut.oldTip, newTip: audit.recut.newTip }
          : recut
      );
    }
    const assessment = await assessPromotionCandidate(input, repoSlug, promotionCandidate);
    policy = assessment.policy;
    lineage = assessment.lineage;

    promotion.mechanicallyPromotable = Boolean(promotionCandidate.commit) && !promotionCandidateIntegrityError;
    promotion.base = promotion.mechanicallyPromotable ? promotionCandidate.commit : null;
    promotion.mechanicalReason = promotionCandidateIntegrityError ?? (promotionCandidate.commit
      ? null
      : `${promotionCandidate.tag} records no target commit, so its immutable source cannot be resolved.`);
    promotion.soak = assessment.soak;
    promotion.allowed = assessment.gate.allowed;
    promotion.blockers = assessment.gate.blockers;
    if (promotionCandidateIntegrityError) {
      promotion.allowed = false;
      promotion.blockers = [
        `The selected staging candidate failed immutable identity verification: ${promotionCandidateIntegrityError}`,
        ...promotion.blockers
      ];
    }
  }

  if (activeCandidate) {
    const audit = await readLineageAudit(input, repoSlug);
    const promoted = await activeProductionTagExists(input, activeCandidate.version);
    const freezeDecision = evaluateStagingFreeze({
      proposedSourceBranch: "main",
      active: activeCandidate,
      activeProductionTagExists: promoted,
      reset: audit.reset
    });
    freeze = {
      active: freezeDecision.active,
      branch: freezeDecision.branch,
      reason: freezeDecision.reason,
      waivedByReset: freezeDecision.waivedByReset
    };
  }

  return {
    production,
    staging,
    releaseBranch,
    commitsOnMainSinceProduction,
    policy,
    lineage,
    freeze,
    promotion,
    promoteCommand: promotion.allowed && promotionCandidate ? `kd release promote ${promotionCandidate.version}` : null
  };
}
