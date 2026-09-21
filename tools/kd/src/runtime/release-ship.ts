import { cpSync, existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import type { CommandRunner } from "./process";
import {
  assertCleanGitWorktree,
  mustRun,
  releaseEnvironment,
  releaseOutputDir,
  releaseRepoSlug,
  stagingTag,
  type ReleaseCommandContext,
  type ReleaseEnvironment
} from "./release-command";
import {
  RELEASE_CANDIDATE_FILE,
  SOURCE_BRANCH_TRAILER,
  bumpVersion,
  compareVersions,
  deriveMainStagingBaseVersion,
  nextSeriesPatchVersion,
  parsePromotionVersions,
  parseReleaseBranchSeries,
  readAbandonedSeries,
  readCurrentVersion,
  readGreatestProductionVersion,
  readReleaseCandidateNumber,
  releaseSeriesBranch,
  releaseSeriesFromVersion,
  restoreVersionFiles,
  snapshotVersionFiles,
  splitPublishedVersion,
  writeReleaseVersionFiles,
  type MainStagingVersionFloor,
  type ReleaseBump
} from "./release-version";
import {
  STAGING_CHANNEL_TAG,
  STAGING_MANIFEST_NAME,
  activeProductionTagExists,
  ensureStagingGithubRelease,
  listStagingCandidateTags,
  parseRemoteTagCommit,
  productionVersionForStaging,
  readStagingCandidate,
  readStagingChannelBody,
  readVerifiedStagingCandidate,
  verifyImmutableStagingCandidate
} from "./release-channel";
import { assertStagingPublishAllowed, assessPromotionCandidate } from "./release-gate";
import {
  type ReleaseArchLabel,
  bazelTargetForLabel,
  createUpdaterBundleWithSigningKey,
  releaseAssetName,
  resolveBazelOutput,
  updaterAssetName,
  updaterBundleTargetForLabel,
  updaterPlatformKey,
  updaterSignatureName,
  validateDmgImageResources,
  writeLatestJson
} from "./release-artifacts";
import {
  composePostPromotionTrunkBody,
  composeStagingChannelRecutApplicationBody,
  type LineageRecutApplicationRecord,
  type LineageRecutRecord,
  type PostPromotionTrunkRecord,
  type StagingCandidate
} from "./release-lineage";
import { preflightUpdaterSigningKey } from "./updater-key";

export interface ReleaseShipInput {
  repoRoot: string;
  bump: ReleaseBump;
  /** Whether the operator explicitly selected a bump instead of accepting the staging-series default. */
  bumpExplicit?: boolean;
  archLabels: ReleaseArchLabel[];
  environment?: ReleaseEnvironment;
  release: boolean;
  dryRun: boolean;
  rollbackTo?: string;
  promoteFrom?: string;
  sourceBranch?: string;
  /** Explicit human reason for promoting before the policy soak window elapses. */
  soakOverrideReason?: string;
  /** Fixed clock for soak arithmetic; defaults to `Date.now()`. */
  now?: number;
  env: NodeJS.ProcessEnv;
  runner: CommandRunner;
}

export interface ReleaseShipResult {
  version: string;
  dmgPaths: string[];
  updaterPaths: string[];
  latestJson: string;
  /** Present when stale trunk VERSION was raised to the production floor. */
  versionFloor?: MainStagingVersionFloor;
  /** Present on a published production release: the series branch it left behind. */
  seriesBranch?: ReleaseSeriesBranchOutcome;
}

/** What a published production release did about its own `release/X.Y` branch. */
export interface ReleaseSeriesBranchOutcome {
  branch: string;
  /** Where the branch points now. Equals the released commit unless it already existed elsewhere. */
  commit: string;
  /** Whether this release created the branch or found it already on origin. */
  created: boolean;
  /** Set when an existing branch does not hold the released commit; a release never moves one. */
  detail: string | null;
}

async function createOrReuseStagingCandidate(
  input: ReleaseShipInput,
  repoSlug: string,
  version: string,
  targetCommit: string,
  notes: string,
  assets: string[]
): Promise<void> {
  const tag = stagingTag(version);
  const created = await input.runner.run(
    "gh",
    [
      "release",
      "create",
      tag,
      "--repo",
      repoSlug,
      "--title",
      `Kanna Staging v${version}`,
      "--notes",
      notes,
      "--target",
      targetCommit,
      "--prerelease",
      ...assets
    ],
    { cwd: input.repoRoot, env: input.env }
  );
  if (created.exitCode === 0) return;

  // A retry after the versioned release was created but before the channel
  // pointer moved must finish that same candidate. Reuse is permitted only
  // when GitHub still identifies the existing release with this exact commit;
  // a different tag target is never an idempotent retry.
  const existing = await readStagingCandidate(input, repoSlug, version);
  if (existing.candidate?.commit?.toLowerCase() === targetCommit.toLowerCase()) return;
  throw new Error(
    created.stderr.trim() || created.stdout.trim() ||
      `Could not create or verify staging prerelease ${tag}.`
  );
}

function parseExistingStagingNumbers(output: string, baseVersion: string): number[] {
  const pattern = new RegExp(`(?:refs/tags/)?v${baseVersion.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}-staging\\.(\\d+)(?:\\^\\{\\})?$`);
  const numbers = new Set<number>();
  for (const line of output.split(/\r?\n/)) {
    const ref = line.trim().split(/\s+/).at(-1) ?? "";
    const match = pattern.exec(ref);
    if (!match) continue;
    const value = Number.parseInt(match[1] ?? "", 10);
    if (!Number.isNaN(value)) numbers.add(value);
  }
  return [...numbers];
}

async function resolveNextStagingVersion(
  input: ReleaseShipInput,
  baseVersion: string,
  activeVersion: string | null = null
): Promise<string> {
  const tags = await mustRun(input.runner, "git", ["ls-remote", "--tags", "origin", `v${baseVersion}-staging.*`], input.repoRoot, input.env);
  const activeMatch = activeVersion
    ? new RegExp(`^${baseVersion.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}-staging\\.(\\d+)$`).exec(activeVersion)
    : null;
  const activeNumber = Number.parseInt(activeMatch?.[1] ?? "0", 10);
  const highest = Math.max(activeNumber, ...parseExistingStagingNumbers(tags, baseVersion));
  return `${baseVersion}-staging.${highest + 1}`;
}

interface StagingContext {
  baseVersion: string;
  sourceBranch: string;
  commit: string;
  branchTip: string | null;
  versionFloor: MainStagingVersionFloor | null;
  /**
   * The exact candidate version, when the branch already commits it.
   *
   * A release branch carries VERSION and VERSION_RC, so its candidate is read
   * rather than derived, and the build takes the same two files: kd never
   * writes a version into the worktree to build one.
   */
  committedCandidateVersion?: string;
}

async function resolveStagingContext(input: ReleaseShipInput): Promise<StagingContext> {
  const requested = input.sourceBranch?.trim();
  let branchName: string;
  if (requested) {
    if (requested !== "main" && !parseReleaseBranchSeries(requested)) {
      throw new Error(`Invalid --branch ${requested}. Expected main or release/X.Y.`);
    }
    branchName = requested;
  } else {
    const currentBranch = await mustRun(input.runner, "git", ["rev-parse", "--abbrev-ref", "HEAD"], input.repoRoot, input.env);
    branchName = parseReleaseBranchSeries(currentBranch) ? currentBranch : "main";
  }

  const head = await mustRun(input.runner, "git", ["rev-parse", "HEAD"], input.repoRoot, input.env);

  if (branchName === "main") {
    const sourceVersion = readCurrentVersion(input.repoRoot);
    // Main starts the next feature series by default. Patch releases in the
    // production series come from its release branch; an explicit bump still
    // lets an operator override derivation when the release plan requires it.
    const bump = input.bumpExplicit ? input.bump : "minor";
    const derivation = deriveMainStagingBaseVersion(sourceVersion, await readGreatestProductionVersion(input), bump);
    return { ...derivation, sourceBranch: "main", commit: head, branchTip: null };
  }

  const series = parseReleaseBranchSeries(branchName);
  if (!series) {
    throw new Error(`Invalid release branch: ${branchName}`);
  }
  const branchRefs = await mustRun(input.runner, "git", ["ls-remote", "origin", `refs/heads/${branchName}`], input.repoRoot, input.env);
  const branchSha = branchRefs.trim().split(/\s+/)[0] ?? "";
  if (!branchSha) {
    throw new Error(`${branchName} does not exist on origin. Cut it first (kd release cut).`);
  }
  const abandoned = await readAbandonedSeries(input, branchName);
  if (abandoned) {
    throw new Error(
      `${branchName} was abandoned${abandoned.abandonedAt ? ` on ${abandoned.abandonedAt}` : ""}` +
        `${abandoned.reason ? `: ${abandoned.reason}` : "."} No release candidate ships from an abandoned series. ` +
        `Ship from the current series branch instead, or cut one (kd release cut --version X.Y.0).`
    );
  }
  await mustRun(input.runner, "git", ["fetch", "origin", branchName], input.repoRoot, input.env);
  // Exact provenance, not containment: a branch RC must be a build of the
  // remote branch tip itself. Containment let a worktree ship an RC carrying
  // commits that were never on the branch it claims as its promotion base, so
  // the recorded Source-Branch and the artifact could disagree.
  if (branchSha !== head) {
    throw new Error(
      `${branchName} tip (${branchSha}) is not HEAD (${head}). A release-branch RC must build the branch tip exactly. ` +
        `Push backports to ${branchName} first, then check this worktree out at that commit ` +
        `(git fetch origin ${branchName} && git checkout --detach FETCH_HEAD).`
    );
  }
  const tags = await mustRun(input.runner, "git", ["ls-remote", "--tags", "origin", `v${series.major}.${series.minor}.*`], input.repoRoot, input.env);

  // The branch states the version it ships under; kd does not invent one. A
  // stale VERSION is refused here rather than at promotion, where the forward
  // production-version gate would reject it much later and far less clearly.
  const committedVersion = readCurrentVersion(input.repoRoot);
  const committedSeries = releaseSeriesFromVersion(committedVersion);
  if (committedSeries.major !== series.major || committedSeries.minor !== series.minor) {
    throw new Error(
      `${branchName} has VERSION ${committedVersion}, which is not in series ${series.major}.${series.minor}. ` +
        `A release branch carries its own series' version; commit the right one before shipping a candidate.`
    );
  }
  const expectedVersion = nextSeriesPatchVersion(tags, series);
  if (compareVersions(committedVersion, expectedVersion) < 0) {
    throw new Error(
      `${branchName} has VERSION ${committedVersion}, but v${committedVersion} is already released. ` +
        `Setting the version is part of starting a candidate line: commit VERSION ${expectedVersion} ` +
        `(and ${RELEASE_CANDIDATE_FILE} 1) onto ${branchName} with the backport, then ship.`
    );
  }
  const candidate = readReleaseCandidateNumber(input.repoRoot);
  return {
    baseVersion: committedVersion,
    committedCandidateVersion: `${committedVersion}-staging.${candidate}`,
    sourceBranch: branchName,
    commit: head,
    branchTip: branchSha,
    versionFloor: null
  };
}

async function findReusableStagingCandidate(
  input: ReleaseShipInput,
  repoSlug: string,
  baseVersion: string,
  activeVersion: string | null,
  targetCommit: string,
  sourceBranch: string
): Promise<string | null> {
  const tags = await listStagingCandidateTags(input, repoSlug);
  for (const tag of tags) {
    const version = tag.replace(/^v/, "");
    if (!version.startsWith(`${baseVersion}-staging.`)) continue;
    if (activeVersion && compareVersions(version, activeVersion) <= 0) continue;
    const lookup = await readStagingCandidate(input, repoSlug, version);
    if (
      lookup.candidate?.commit?.toLowerCase() === targetCommit.toLowerCase() &&
      lookup.candidate.sourceBranch === sourceBranch
    ) {
      return version;
    }
  }
  return null;
}

function assertStagingVersionAdvances(
  version: string,
  active: StagingCandidate | null,
  committedFrom?: string
): void {
  if (!active || compareVersions(version, active.version) > 0) return;
  if (committedFrom) {
    // A release-branch candidate is stated by its two committed files, so the
    // generic guidance below — continue the series, or pass --minor/--major —
    // cannot move it. Nothing advances VERSION_RC on its own, so say which
    // file is the lever and what to put in it.
    const serving = splitPublishedVersion(active.version);
    const proposed = splitPublishedVersion(version);
    const remedy = serving.base === proposed.base
      ? `Commit ${RELEASE_CANDIDATE_FILE} ${serving.candidate + 1} onto ${committedFrom} — alongside the backport it ` +
        `is a candidate for — and ship again.`
      : `${STAGING_CHANNEL_TAG} is serving v${active.version}, a different version line, so no candidate number ` +
        `for v${proposed.base} advances it. Ship that line's next patch, or release the channel deliberately ` +
        `(kd release reset-staging).`;
    throw new Error(
      `Refusing to republish v${version}: ${STAGING_CHANNEL_TAG}/${STAGING_MANIFEST_NAME} already serves ` +
        `v${active.version}. ${committedFrom} states its candidate in ${RELEASE_CANDIDATE_FILE} and nothing ` +
        `advances it for you. ${remedy}`
    );
  }
  throw new Error(
    `Refusing to roll the staging channel version back or republish it: derived v${version}, but ` +
      `${STAGING_CHANNEL_TAG}/${STAGING_MANIFEST_NAME} currently serves v${active.version}. ` +
      `A staging publish must be strictly greater by semantic-version ordering. Run \`kd release status\`; ` +
      `use a bare \`kd release ship --staging\` to continue the active series, or pass --minor/--major to start a newer series.`
  );
}

interface ResolvedPromotion {
  version: string;
  /** Exact immutable RC source used for notes, build provenance, and the release commit parent. */
  sourceCommit: string;
}

async function resolvePromotion(input: ReleaseShipInput, promoteFrom: string): Promise<ResolvedPromotion> {
  const { stagingVersion, stagingTag, productionVersion } = parsePromotionVersions(promoteFrom);
  const remoteUrl = await mustRun(input.runner, "git", ["remote", "get-url", "origin"], input.repoRoot, input.env);
  const repoSlug = releaseRepoSlug(remoteUrl);
  const lookup = await readVerifiedStagingCandidate(input, repoSlug, stagingVersion);
  if (!lookup.candidate || lookup.error) {
    throw new Error(lookup.error ?? `Staging prerelease not found: ${stagingTag}`);
  }
  await verifyImmutableStagingCandidate(input, repoSlug, lookup.candidate);
  const { commit, sourceBranch, publishedAt } = lookup.candidate;
  if (!commit || !sourceBranch) {
    throw new Error(`${stagingTag} has incomplete immutable release metadata.`);
  }

  const existingTags = await mustRun(input.runner, "git", ["ls-remote", "--tags", "origin", `v${productionVersion}`], input.repoRoot, input.env);
  if (existingTags.trim().length > 0) {
    throw new Error(`Production tag v${productionVersion} already exists. ${stagingTag} was already promoted, or the next version needs a fresh staging RC.`);
  }

  const head = await mustRun(input.runner, "git", ["rev-parse", "HEAD"], input.repoRoot, input.env);
  if (head !== commit) {
    throw new Error(
      `HEAD (${head}) is not the commit ${stagingTag} was built from (${commit}). Check out that commit and rerun.`
    );
  }

  // The versioned prerelease is the promotion base. Branch tips and the staging
  // pointer are deliberately absent from this decision: both may advance while
  // this immutable candidate accumulates its own soak history.
  const assessment = await assessPromotionCandidate(
    input,
    repoSlug,
    { version: stagingVersion, tag: stagingTag, commit, sourceBranch, publishedAt }
  );
  if (!assessment.gate.allowed) {
    throw new Error(`Cannot promote ${stagingTag}:\n- ${assessment.gate.blockers.join("\n- ")}`);
  }
  return { version: productionVersion, sourceCommit: commit };
}

interface GithubAsset {
  name?: string;
}

interface GithubReleaseView {
  assets?: GithubAsset[];
}

function parseGithubReleaseView(raw: string): GithubReleaseView {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== "object" || parsed === null) return {};
    const assets = "assets" in parsed ? (parsed as { assets?: unknown }).assets : undefined;
    if (!Array.isArray(assets)) return {};
    return {
      assets: assets
        .filter((asset): asset is { name?: unknown } => typeof asset === "object" && asset !== null)
        .map((asset) => ({ name: typeof asset.name === "string" ? asset.name : undefined }))
    };
  } catch {
    return {};
  }
}

async function pruneStagingChannelAssets(input: ReleaseShipInput, repoSlug: string): Promise<void> {
  const raw = await mustRun(input.runner, "gh", ["release", "view", STAGING_CHANNEL_TAG, "--repo", repoSlug, "--json", "assets"], input.repoRoot, input.env);
  const view = parseGithubReleaseView(raw);
  for (const asset of view.assets ?? []) {
    if (!asset.name || asset.name === STAGING_MANIFEST_NAME) continue;
    await mustRun(input.runner, "gh", ["release", "delete-asset", STAGING_CHANNEL_TAG, asset.name, "--repo", repoSlug, "--yes"], input.repoRoot, input.env);
  }
}

async function rollbackStagingRelease(input: ReleaseShipInput, version: string): Promise<ReleaseShipResult> {
  if (releaseEnvironment(input.environment) !== "staging") {
    throw new Error("--rollback-to is only supported with --staging.");
  }
  await assertCleanGitWorktree(input.repoRoot, input.runner, input.env);
  const normalizedVersion = version.replace(/^v/, "");
  const releaseTag = stagingTag(normalizedVersion);
  const releaseDir = releaseOutputDir(input.repoRoot, "staging");
  mkdirSync(releaseDir, { recursive: true });
  const latestJson = join(releaseDir, STAGING_MANIFEST_NAME);
  rmSync(latestJson, { force: true });

  const remoteUrl = await mustRun(input.runner, "git", ["remote", "get-url", "origin"], input.repoRoot, input.env);
  const repoSlug = releaseRepoSlug(remoteUrl);
  const releaseView = await input.runner.run("gh", ["release", "view", releaseTag, "--repo", repoSlug], {
    cwd: input.repoRoot,
    env: input.env
  });
  if (releaseView.exitCode !== 0) {
    throw new Error(`Staging prerelease not found: ${releaseTag}`);
  }
  if (!input.dryRun) {
    const download = await input.runner.run("gh", ["release", "download", releaseTag, "--repo", repoSlug, "--pattern", STAGING_MANIFEST_NAME, "--dir", releaseDir, "--clobber"], {
      cwd: input.repoRoot,
      env: input.env
    });
    if (download.exitCode !== 0) {
      throw new Error(`Staging manifest asset not found on ${releaseTag}: ${STAGING_MANIFEST_NAME}`);
    }
    if (!existsSync(latestJson)) throw new Error(`Staging manifest asset not found on ${releaseTag}: ${STAGING_MANIFEST_NAME}`);
    await ensureStagingGithubRelease(input, repoSlug);
    await mustRun(input.runner, "gh", ["release", "upload", STAGING_CHANNEL_TAG, latestJson, "--repo", repoSlug, "--clobber"], input.repoRoot, input.env);
  }
  return { version: normalizedVersion, dmgPaths: [], updaterPaths: [], latestJson };
}

/**
 * Leaves `release/X.Y` behind at the commit a production release was cut from.
 *
 * A series branch is a *consequence* of releasing, not a separate ceremony
 * somebody has to remember. Before this, the only code that pushed
 * `refs/heads/release/*` was `kd release cut` and its recut, so a series
 * promoted straight off a bare main RC — which is how `v0.4.0` shipped —
 * published a tag and nothing else. The released commit was then reachable
 * only by tag, with main already dozens of commits past it, and there was
 * nowhere to apply `0.4.1`: `kd release cut` cuts at `origin/main`'s tip, and
 * `--recut` moves unreleased series only.
 *
 * The branch is created *here*, at publication, rather than when the staging
 * candidate is built, because that is when a release becomes real. Most RCs
 * never promote; several RCs of one series are built from different commits;
 * and an RC-time branch would point at a commit that the release itself then
 * moves past (the production `release: vX.Y.Z` bump commit is a child of the
 * RC). Releasing is the one moment with exactly one commit that deserves the
 * name.
 *
 * Two rules keep it safe. It only ever *creates*: an existing branch is read
 * and reported, never moved, so a live series carrying backports cannot be
 * rewound by a promotion of an older candidate. And it runs *before* the tag
 * is pushed and the GitHub release is created, so a failure to write the
 * branch aborts the release before anything is published, instead of leaving
 * the published-tag-without-a-branch state this exists to prevent. Because it
 * is create-only, a retry after any later failure finds the branch already
 * there and proceeds.
 */
async function ensureReleaseSeriesBranch(
  context: ReleaseCommandContext,
  version: string,
  releasedCommit: string
): Promise<ReleaseSeriesBranchOutcome> {
  const branch = releaseSeriesBranch(releaseSeriesFromVersion(version));
  if (!/^[0-9a-f]{40}$/i.test(releasedCommit)) {
    throw new Error(
      `Cannot resolve the commit v${version} is being released from (${JSON.stringify(releasedCommit)}), ` +
        `so ${branch} cannot be created at it. Refusing to publish a release with no series branch.`
    );
  }
  const existing = await mustRun(
    context.runner,
    "git",
    ["ls-remote", "origin", `refs/heads/${branch}`],
    context.repoRoot,
    context.env
  );
  const existingSha = existing.trim().split(/\s+/)[0] ?? "";
  if (existingSha) {
    return {
      branch,
      commit: existingSha,
      created: false,
      detail:
        existingSha.toLowerCase() === releasedCommit.toLowerCase()
          ? null
          : `${branch} already exists at ${existingSha}; v${version} was released from ${releasedCommit}. ` +
            `Left where it is: a release only ever creates a missing series branch. For an ordinary patch ` +
            `that is expected — the release commit is the branch tip's child and stays reachable by its tag — ` +
            `and for a historical promotion it is the point, since the branch may already carry backports ` +
            `past this release. Keep backporting onto ${branch} as usual.`
    };
  }
  await mustRun(
    context.runner,
    "git",
    ["push", "origin", `${releasedCommit}:refs/heads/${branch}`],
    context.repoRoot,
    context.env
  );
  return { branch, commit: releasedCommit, created: true, detail: null };
}

export async function shipRelease(input: ReleaseShipInput): Promise<ReleaseShipResult> {
  const environment = releaseEnvironment(input.environment);
  if (input.rollbackTo) {
    return rollbackStagingRelease(input, input.rollbackTo);
  }
  if (input.promoteFrom && environment !== "production") {
    throw new Error("Promotion ships a production release; it cannot target staging.");
  }
  if (input.release && input.archLabels.length !== 2) {
    throw new Error("updater releases must include both arm64 and x86_64 artifacts");
  }
  // Open, validate, read, and prove the exact selected file before version files
  // or build outputs can change. Retain that material for both architectures so
  // a later pathname change cannot turn into a late or inconsistent failure.
  const updaterSigningKey = await preflightUpdaterSigningKey({
    cwd: input.repoRoot,
    env: input.env,
    runner: input.runner
  });
  await assertCleanGitWorktree(input.repoRoot, input.runner, input.env);
  let repoSlug = "";

  let version: string;
  let pushBranch = "main";
  let promotionSourceCommit: string | null = null;
  let stagingSourceBranch = "main";
  let postPromotionTrunk: PostPromotionTrunkRecord | null = null;
  let recutAuthorization: LineageRecutRecord | null = null;
  let versionFloor: MainStagingVersionFloor | null = null;
  let seriesBranch: ReleaseSeriesBranchOutcome | null = null;
  // True when the build takes its version from the committed VERSION /
  // VERSION_RC pair, so kd must not write either one to build.
  let versionFromCommittedFiles = false;
  if (input.promoteFrom) {
    const promotion = await resolvePromotion(input, input.promoteFrom);
    version = promotion.version;
    promotionSourceCommit = promotion.sourceCommit;
    // The candidate already committed this version, so promoting is dropping
    // the `-staging.N` suffix and nothing else. Building writes no file and
    // publishing adds no commit, which is what makes the commit that ships the
    // same commit that soaked instead of a child of it.
    versionFromCommittedFiles = readCurrentVersion(input.repoRoot) === version;
  } else if (environment === "staging") {
    const stagingContext = await resolveStagingContext(input);
    stagingSourceBranch = stagingContext.sourceBranch;
    versionFloor = stagingContext.versionFloor;
    const publishGate = await assertStagingPublishAllowed(input, {
      sourceBranch: stagingContext.sourceBranch,
      commit: stagingContext.commit,
      branchTip: stagingContext.branchTip
    });
    repoSlug = publishGate.repoSlug;
    postPromotionTrunk = publishGate.postPromotion;
    recutAuthorization = publishGate.recut;
    const activeBaseVersion = publishGate.active &&
        publishGate.active.sourceBranch === stagingContext.sourceBranch &&
        !(await activeProductionTagExists(input, publishGate.active.version))
      ? productionVersionForStaging(publishGate.active.version)
      : null;
    const baseVersion = !input.bumpExplicit && activeBaseVersion
      ? activeBaseVersion
      : stagingContext.baseVersion;
    version = stagingContext.committedCandidateVersion
      ?? await resolveNextStagingVersion(input, baseVersion, publishGate.active?.version ?? null);
    versionFromCommittedFiles = stagingContext.committedCandidateVersion !== undefined;
    if (recutAuthorization && input.release) {
      const reusable = await findReusableStagingCandidate(
        input,
        repoSlug,
        baseVersion,
        publishGate.active?.version ?? null,
        stagingContext.commit,
        stagingContext.sourceBranch
      );
      if (reusable) version = reusable;
    }
    assertStagingVersionAdvances(
      version,
      publishGate.active,
      stagingContext.committedCandidateVersion ? stagingContext.sourceBranch : undefined
    );
  } else {
    const sourceVersion = readCurrentVersion(input.repoRoot);
    version = bumpVersion(sourceVersion, input.bump);
  }

  const bazelArgs = [input.dryRun ? "-c" : "--config=notarize", input.dryRun ? "opt" : "-c", ...(input.dryRun ? [] : ["opt"])];
  const targets = input.archLabels.flatMap((label) => [bazelTargetForLabel(label, input.dryRun, environment), updaterBundleTargetForLabel(label, environment)]);
  if (versionFromCommittedFiles) {
    // Build what is committed. The staging bundle targets combine VERSION with
    // VERSION_RC themselves, so there is nothing to write and nothing to put
    // back: no bump, no build, no toss away.
    await mustRun(input.runner, "bazel", ["build", ...bazelArgs, ...targets], input.repoRoot, input.env);
  } else {
    const versionFileSnapshot = snapshotVersionFiles(input.repoRoot);
    try {
      // Both files, always: the staging bundle builds VERSION plus VERSION_RC,
      // so writing the suffixed string into VERSION alone would stamp it twice.
      writeReleaseVersionFiles(input.repoRoot, version);
      await mustRun(input.runner, "bazel", ["build", ...bazelArgs, ...targets], input.repoRoot, input.env);
    } catch (error) {
      restoreVersionFiles(input.repoRoot, versionFileSnapshot);
      throw error;
    }
    if (environment === "staging") {
      restoreVersionFiles(input.repoRoot, versionFileSnapshot);
    }
  }

  if (!repoSlug) {
    const remoteUrl = await mustRun(input.runner, "git", ["remote", "get-url", "origin"], input.repoRoot, input.env);
    repoSlug = releaseRepoSlug(remoteUrl);
  }
  const releaseDir = releaseOutputDir(input.repoRoot, environment);
  mkdirSync(releaseDir, { recursive: true });
  const dmgPaths: string[] = [];
  const updaterPaths: string[] = [];
  const platforms: Record<string, { signature: string; url: string }> = {};
  const downloadBase = environment === "staging"
    ? `https://github.com/${repoSlug}/releases/download/v${version}`
    : `https://github.com/${repoSlug}/releases/download/v${version}`;

  for (const label of input.archLabels) {
    const dmgSource = await resolveBazelOutput(input, bazelTargetForLabel(label, input.dryRun, environment));
    const dmgDest = join(releaseDir, releaseAssetName(version, label, environment));
    cpSync(dmgSource, dmgDest);
    await validateDmgImageResources(input, dmgDest);
    dmgPaths.push(dmgDest);

    const bundleSource = await resolveBazelOutput(input, updaterBundleTargetForLabel(label, environment));
    const bundlePath = join(releaseDir, updaterAssetName(version, label, environment));
    const sigPath = join(releaseDir, updaterSignatureName(version, label, environment));
    await createUpdaterBundleWithSigningKey(
      input,
      bundleSource,
      bundlePath,
      sigPath,
      updaterSigningKey
    );
    updaterPaths.push(bundlePath, sigPath);
    platforms[updaterPlatformKey(label)] = {
      url: `${downloadBase}/${updaterAssetName(version, label, environment)}`,
      signature: readFileSync(sigPath, "utf8").trim()
    };
  }

  const latestJson = join(releaseDir, environment === "staging" ? STAGING_MANIFEST_NAME : "latest.json");
  const notes = input.release && environment === "production"
    ? await mustRun(input.runner, "gh", ["api", `repos/${repoSlug}/releases/generate-notes`, "-X", "POST", "-f", `tag_name=v${version}`, "-f", `target_commitish=${promotionSourceCommit ?? pushBranch}`, "--jq", ".body"], input.repoRoot, input.env)
    : environment === "staging"
      ? `Staging updater manifest for v${version}\n\n${SOURCE_BRANCH_TRAILER} ${stagingSourceBranch}${recutAuthorization ? `\nLineage-Recut-Authorization: ${recutAuthorization.recutId}` : ""}`
      : `Dry-run updater manifest for v${version}`;
  const pubDate = new Date().toISOString();
  writeLatestJson(latestJson, version, notes, pubDate, platforms);

  if (input.release && environment === "staging") {
    const targetCommit = await mustRun(input.runner, "git", ["rev-parse", "HEAD"], input.repoRoot, input.env);
    await createOrReuseStagingCandidate(
      input,
      repoSlug,
      version,
      targetCommit,
      notes,
      [...dmgPaths, ...updaterPaths, latestJson]
    );
    await ensureStagingGithubRelease(input, repoSlug);
    if (postPromotionTrunk) {
      const channelBody = composePostPromotionTrunkBody(
        await readStagingChannelBody(input, repoSlug),
        postPromotionTrunk
      );
      await mustRun(
        input.runner,
        "gh",
        ["release", "edit", STAGING_CHANNEL_TAG, "--repo", repoSlug, "--notes", channelBody],
        input.repoRoot,
        input.env
      );
    }
    await mustRun(input.runner, "gh", ["release", "upload", STAGING_CHANNEL_TAG, latestJson, "--repo", repoSlug, "--clobber"], input.repoRoot, input.env);
    // The recut grant becomes durable only after the pointer release serves
    // this candidate. If the upload fails, the grant remains pending and a
    // retry can reuse the versioned RC instead of being stranded by an
    // application record that claims the move happened.
    if (recutAuthorization) {
      const appliedTag = `recut-applied/${recutAuthorization.recutId}`;
      const application: LineageRecutApplicationRecord = {
        recutId: recutAuthorization.recutId,
        version,
        commit: targetCommit.toLowerCase(),
        appliedAt: pubDate,
        tag: appliedTag
      };
      const existing = await input.runner.run(
        "git",
        ["ls-remote", "--tags", "origin", `refs/tags/${appliedTag}`, `refs/tags/${appliedTag}^{}`],
        { cwd: input.repoRoot, env: input.env }
      );
      const existingCommit = existing.exitCode === 0 ? parseRemoteTagCommit(existing.stdout, appliedTag) : null;
      if (existingCommit && existingCommit.toLowerCase() !== targetCommit.toLowerCase()) {
        throw new Error(`Recut application tag ${appliedTag} resolves to ${existingCommit}, not destination ${targetCommit}.`);
      }
      if (!existingCommit) {
        await mustRun(input.runner, "git", ["tag", "-a", appliedTag, targetCommit, "-m", `kanna-recut-application-schema: 1\nrecut-id: ${application.recutId}\nversion: ${application.version}\ncommit: ${application.commit}\napplied-at: ${application.appliedAt}`], input.repoRoot, input.env);
        await mustRun(input.runner, "git", ["push", "origin", `refs/tags/${appliedTag}`], input.repoRoot, input.env);
      }
      const appliedBody = composeStagingChannelRecutApplicationBody(await readStagingChannelBody(input, repoSlug), application);
      await mustRun(input.runner, "gh", ["release", "edit", STAGING_CHANNEL_TAG, "--repo", repoSlug, "--notes", appliedBody], input.repoRoot, input.env);
    }
    await pruneStagingChannelAssets(input, repoSlug);
  } else if (input.release) {
    // A candidate whose own commit already states this version needs no release
    // commit: the tag goes straight onto the commit that soaked. Only a build
    // whose version kd had to write still commits it, which is what a bare-main
    // production ship does.
    if (!versionFromCommittedFiles) {
      await mustRun(input.runner, "git", ["add", "-f", "VERSION", "apps/desktop/src-tauri/tauri.conf.json", "apps/desktop/src-tauri/Cargo.toml", "apps/desktop/src-tauri/Cargo.lock"], input.repoRoot, input.env);
      await mustRun(input.runner, "git", ["commit", "-m", `release: v${version}`], input.repoRoot, input.env);
    }
    await mustRun(input.runner, "git", ["tag", `v${version}`], input.repoRoot, input.env);
    // The series branch is part of releasing, so it lands before anything is
    // published: the exact released commit, created only when absent, and a
    // failure here aborts before the tag or the GitHub release exist.
    const releasedCommit = await mustRun(input.runner, "git", ["rev-parse", "HEAD"], input.repoRoot, input.env);
    seriesBranch = await ensureReleaseSeriesBranch(input, version, releasedCommit);
    // A promotion may select a historical RC after main, its source branch, and
    // desktop-staging have advanced. Publish the immutable production tag; do
    // not rewind or overwrite any moving branch/channel pointer. Ordinary
    // production ships still advance their selected branch as before.
    await mustRun(
      input.runner,
      "git",
      ["push", "origin", ...(promotionSourceCommit ? [] : [`HEAD:${pushBranch}`]), `v${version}`],
      input.repoRoot,
      input.env
    );
    await mustRun(input.runner, "gh", ["release", "create", `v${version}`, ...dmgPaths, ...updaterPaths, "--title", `Kanna v${version}`, "--notes", notes], input.repoRoot, input.env);
    await mustRun(input.runner, "gh", ["release", "upload", `v${version}`, latestJson, "--clobber"], input.repoRoot, input.env);
  }

  return {
    version,
    dmgPaths,
    updaterPaths,
    latestJson,
    ...(versionFloor ? { versionFloor } : {}),
    ...(seriesBranch ? { seriesBranch } : {})
  };
}
