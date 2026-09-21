import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { CommandRunner } from "./process";
import { mustRun, releaseRepoSlug, type ReleaseCommandContext } from "./release-command";
import {
  RELEASE_CANDIDATE_FILE,
  abandonedSeriesTag,
  compareVersions,
  deriveMainStagingBaseVersion,
  formatAbandonedSeriesMessage,
  hasProductionTagForSeries,
  parseReleaseBranchSeries,
  readAbandonedSeries,
  readGreatestProductionVersion,
  releaseSeriesBranch,
  releaseSeriesFromVersion,
  type AbandonedSeriesRecord,
  type ReleaseBump,
  type ReleaseSeries
} from "./release-version";
import {
  STAGING_CHANNEL_TAG,
  ensureStagingGithubRelease,
  readStagingChannelBody,
  readVerifiedStagingCandidate,
  resolveActiveStagingCandidate,
  verifyImmutableStagingCandidate
} from "./release-channel";
import {
  composeStagingChannelRecutBody,
  normalizeStagingVersion,
  parseLineageResetRecord,
  type LineageRecutRecord,
  type LineageResetRecord,
  type StagingCandidate
} from "./release-lineage";

async function readLineageReset(context: ReleaseCommandContext, repoSlug: string): Promise<LineageResetRecord | null> {
  return parseLineageResetRecord(await readStagingChannelBody(context, repoSlug));
}

export interface ReleaseCutInput {
  repoRoot: string;
  bump: ReleaseBump;
  /**
   * Explicit target series version `X.Y.0`. Overrides bump inference, which
   * cannot express skipping beyond the next series derived from the greater of
   * trunk's recorded version and the production-version floor.
   */
  version?: string;
  /** Series (`X.Y`) this cut deliberately skips over. Never inferred. */
  abandonSeries?: string[];
  /** Why those series are being abandoned. Required whenever any is named. */
  reason?: string;
  /** Move an existing unreleased release branch to origin/main explicitly. */
  recut?: boolean;
  dryRun?: boolean;
  confirmRecut?: string;
  confirmOldTip?: string;
  now?: number;
  env: NodeJS.ProcessEnv;
  runner: CommandRunner;
}

export interface AbandonedSeries {
  series: string;
  branch: string;
  commit: string;
  tag: string;
  reason: string;
  abandonedAt: string;
  /** True when the series already carried an abandonment tag before this cut. */
  alreadyAbandoned: boolean;
}

export interface ReleaseCutResult {
  branch: string;
  version: string;
  /** The branch tip: the commit that sets the series version. */
  commit: string;
  /** The `origin/main` tip the series was cut from; the version commit's parent. */
  trunkCommit?: string;
  /** The `VERSION` recorded at `origin/main` when the branch was cut. */
  trunkVersion: string;
  abandoned: AbandonedSeries[];
  recut?: {
    id: string;
    archiveTag: string;
    oldTip: string;
    newTip: string;
    applied: boolean;
  };
}

function recutArchiveTag(branch: string, ordinal: number): string {
  return `recut/${branch}-${ordinal}`;
}

function recutArchiveMessage(record: LineageRecutRecord, repoSlug: string): string {
  return [
    "kanna-recut-schema: 1",
    `repository: ${repoSlug}`,
    `recut-id: ${record.recutId}`,
    `series: ${record.series}`,
    `branch: ${record.branch}`,
    `old-tip: ${record.oldTip}`,
    `new-tip: ${record.newTip}`,
    `main-tip: ${record.newTip}`,
    `active-version: ${record.fromVersion ?? "empty-channel"}`,
    `active-commit: ${record.fromCommit ?? "unknown-commit"}`,
    `active-source: ${record.fromSourceBranch ?? "unknown"}`,
    `prior-epoch: ${record.priorEpoch}`,
    `timestamp: ${record.recutAt}`,
    `requester-provenance: ${record.requester}`,
    `reason: ${record.reason}`
  ].join("\n");
}

function parseRecutArchiveOrdinals(output: string, branch: string): number[] {
  const escaped = branch.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const pattern = new RegExp(`^(?:refs/tags/)?recut/${escaped}-(\\d+)(?:\\^\\{\\})?$`);
  return output.split(/\r?\n/).flatMap((line) => {
    const ref = line.trim().split(/\s+/).at(-1) ?? "";
    const match = pattern.exec(ref);
    if (!match?.[1]) return [];
    const ordinal = Number.parseInt(match[1], 10);
    return Number.isNaN(ordinal) ? [] : [ordinal];
  });
}

function normalizedRemoteRefs(output: string): string {
  return output
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .sort()
    .join("\n");
}

async function revalidateRecutPlan(
  input: ReleaseCommandContext,
  repoSlug: string,
  branch: string,
  expectedMainTip: string,
  expectedBranchTip: string,
  expectedProductionTags: string,
  expectedActive: StagingCandidate | null,
  phase: string
): Promise<void> {
  await mustRun(input.runner, "git", ["fetch", "origin", "main", branch], input.repoRoot, input.env);
  const mainTip = await mustRun(input.runner, "git", ["rev-parse", "origin/main"], input.repoRoot, input.env);
  const branchRefs = await mustRun(input.runner, "git", ["ls-remote", "origin", `refs/heads/${branch}`], input.repoRoot, input.env);
  const branchTip = branchRefs.split(/\s+/)[0] ?? "";
  if (mainTip.toLowerCase() !== expectedMainTip.toLowerCase() || branchTip.toLowerCase() !== expectedBranchTip.toLowerCase()) {
    throw new Error(
      `Cannot recut ${branch}: release refs changed ${phase}; expected main ${expectedMainTip} and ${branch} ${expectedBranchTip}, ` +
        `observed main ${mainTip} and ${branch} ${branchTip || "missing"}.`
    );
  }
  const branchSeries = parseReleaseBranchSeries(branch);
  if (!branchSeries) throw new Error(`Cannot recut ${branch}: invalid release branch identity.`);
  const productionTags = await mustRun(
    input.runner,
    "git",
    ["ls-remote", "--tags", "origin", `refs/tags/v${branchSeries.major}.${branchSeries.minor}.*`],
    input.repoRoot,
    input.env
  );
  if (normalizedRemoteRefs(productionTags) !== normalizedRemoteRefs(expectedProductionTags)) {
    throw new Error(`Cannot recut ${branch}: production tags changed ${phase}; refusing to use stale release facts.`);
  }

  const active = await resolveActiveStagingCandidate(input, repoSlug);
  if (active.error) throw new Error(`Cannot recut ${branch}: the ${STAGING_CHANNEL_TAG} channel changed or became unreadable ${phase}: ${active.error}`);
  if (!expectedActive && !active.candidate) return;
  if (!expectedActive || !active.candidate || expectedActive.version !== active.candidate.version || expectedActive.commit?.toLowerCase() !== active.candidate.commit?.toLowerCase() || expectedActive.sourceBranch !== active.candidate.sourceBranch) {
    throw new Error(`Cannot recut ${branch}: the active ${STAGING_CHANNEL_TAG} candidate changed ${phase}; refusing to use stale release facts.`);
  }
  const verified = await readVerifiedStagingCandidate(input, repoSlug, active.candidate.version);
  if (verified.error || !verified.candidate) {
    throw new Error(`Cannot recut ${branch}: the active ${STAGING_CHANNEL_TAG} candidate could not be re-verified ${phase}: ${verified.error ?? "unknown error"}`);
  }
  await verifyImmutableStagingCandidate(input, repoSlug, verified.candidate);
}

async function recutReleaseBranch(input: ReleaseCutInput): Promise<ReleaseCutResult> {
  const requested = input.version?.trim().replace(/^v/, "") ?? "";
  if (!/^\d+\.\d+\.0$/.test(requested)) {
    throw new Error("release cut --recut requires --version X.Y.0.");
  }
  if (input.abandonSeries && input.abandonSeries.length > 0) {
    throw new Error("release cut --recut cannot be combined with --abandon-series.");
  }
  const reason = input.reason?.trim() ?? "";
  if (!reason) throw new Error("release cut --recut requires --reason \"<why the series is moving>\".");
  if (/[\r\n]/.test(reason)) throw new Error("release cut --recut reason must be a single line.");
  const series = releaseSeriesFromVersion(requested);
  const branch = releaseSeriesBranch(series);
  const seriesLabel = `${series.major}.${series.minor}`;
  const remoteUrl = await mustRun(input.runner, "git", ["remote", "get-url", "origin"], input.repoRoot, input.env);
  const repoSlug = releaseRepoSlug(remoteUrl);

  await mustRun(input.runner, "git", ["fetch", "origin", "main", branch], input.repoRoot, input.env);
  const mainTip = await mustRun(input.runner, "git", ["rev-parse", "origin/main"], input.repoRoot, input.env);
  const branchRefs = await mustRun(input.runner, "git", ["ls-remote", "origin", `refs/heads/${branch}`], input.repoRoot, input.env);
  const oldTip = branchRefs.split(/\s+/)[0] ?? "";
  if (!/^[0-9a-f]{40}$/i.test(oldTip)) {
    throw new Error(`${branch} does not exist on origin. Cut it first (kd release cut --version ${requested}).`);
  }
  if (input.confirmOldTip?.trim().toLowerCase() !== oldTip.toLowerCase()) {
    throw new Error(`release cut --recut requires --confirm-old-tip ${oldTip} for the observed ${branch} tip.`);
  }
  const productionTags = await mustRun(
    input.runner,
    "git",
    ["ls-remote", "--tags", "origin", `refs/tags/v${seriesLabel}.*`],
    input.repoRoot,
    input.env
  );
  if (hasProductionTagForSeries(productionTags, series)) {
    throw new Error(`Cannot recut ${branch}: production release v${seriesLabel}.* already exists. Continue with patch fixes only; cut the next series.`);
  }

  const active = await resolveActiveStagingCandidate(input, repoSlug);
  if (active.error) throw new Error(`Cannot recut ${branch} while the ${STAGING_CHANNEL_TAG} channel is unreadable: ${active.error}`);
  let activeCandidate = active.candidate;
  if (activeCandidate) {
    const verified = await readVerifiedStagingCandidate(input, repoSlug, activeCandidate.version);
    if (verified.error || !verified.candidate) {
      throw new Error(`Cannot recut ${branch}: active staging candidate could not be verified (${verified.error ?? "unknown error"}).`);
    }
    await verifyImmutableStagingCandidate(input, repoSlug, verified.candidate);
    activeCandidate = verified.candidate;
  }
  const confirmedActive = input.confirmRecut?.trim().replace(/^v/, "") ?? "";
  const observedActive = activeCandidate?.version ?? "empty";
  if (confirmedActive !== observedActive) {
    throw new Error(`release cut --recut requires --confirm-recut ${observedActive}; the observed staging channel is ${observedActive}.`);
  }
  if (oldTip.toLowerCase() === mainTip.toLowerCase()) {
    return {
      branch,
      version: requested,
      commit: mainTip,
      trunkVersion: (await mustRun(input.runner, "git", ["show", "origin/main:VERSION"], input.repoRoot, input.env)).trim(),
      abandoned: [],
      recut: { id: "no-op", archiveTag: "", oldTip, newTip: mainTip, applied: false }
    };
  }

  const unmerged = await resolveUnmergedReleaseCommitsAtTips(input, mainTip, oldTip);
  if (!unmerged.commits || unmerged.count === null) {
    throw new Error(`Cannot recut ${branch}: release-branch hygiene could not be verified against the pinned origin/main and branch tips.`);
  }
  const merges = await input.runner.run(
    "git",
    ["log", "--merges", "--right-only", "--format=%H %s", `${mainTip}...${oldTip}`],
    { cwd: input.repoRoot, env: input.env }
  );
  const mergeOnly = merges.exitCode === 0 ? parseStrictReleaseCommitLog(merges.stdout) : null;
  if (mergeOnly === null) {
    throw new Error(`Cannot recut ${branch}: merge-resolution hygiene could not be verified; refusing to risk losing branch-only work.`);
  }
  const offending: ReleaseBranchCommit[] = [];
  for (const commit of [...unmerged.commits, ...mergeOnly]) {
    // kd's own series version commit is branch-only by construction; every
    // other branch-only commit is real work a recut would discard.
    if (await isSeriesVersionCommit(input, commit.sha)) continue;
    offending.push(commit);
  }
  if (offending.length > 0) {
    const listed = offending.slice(0, UNMERGED_COMMIT_REPORT_LIMIT).map((commit) => `${commit.sha} ${commit.subject}`).join("\n");
    throw new Error(`Cannot recut ${branch}: it contains ${offending.length} branch-only commit(s) not present on origin/main by patch identity. Backport them first; recut would lose:\n${listed}`);
  }

  const archiveRefs = await mustRun(input.runner, "git", ["ls-remote", "--tags", "origin", `recut/${branch}-*`], input.repoRoot, input.env);
  const ordinal = Math.max(0, ...parseRecutArchiveOrdinals(archiveRefs, branch)) + 1;
  const archiveTag = recutArchiveTag(branch, ordinal);
  const recutAt = new Date(input.now ?? Date.now()).toISOString();
  // A recut lands the same shape a cut does: origin/main's tip plus the commit
  // that states the series version. Pushing mainTip bare left the branch
  // carrying trunk's VERSION, and the next ship refused it as out of series.
  const newTip = await composeSeriesVersionCommit(input, mainTip, requested);
  const record: LineageRecutRecord = {
    recutId: `${seriesLabel}-${ordinal}`,
    recutAt,
    series: seriesLabel,
    branch,
    oldTip: oldTip.toLowerCase(),
    newTip: newTip.toLowerCase(),
    archiveTag,
    fromVersion: activeCandidate?.version ?? null,
    fromCommit: activeCandidate?.commit ?? null,
    fromSourceBranch: activeCandidate?.sourceBranch ?? null,
    priorEpoch: activeCandidate?.version ?? "empty",
    requester: input.env.KANNA_RELEASE_REQUESTER ?? input.env.USER ?? "unknown",
    reason
  };
  const trunkVersion = (await mustRun(input.runner, "git", ["show", "origin/main:VERSION"], input.repoRoot, input.env)).trim();
  if (input.dryRun) {
    return { branch, version: requested, commit: newTip, trunkCommit: mainTip, trunkVersion, abandoned: [], recut: { id: record.recutId, archiveTag, oldTip, newTip, applied: false } };
  }

  await revalidateRecutPlan(input, repoSlug, branch, mainTip, oldTip, productionTags, activeCandidate, "before archive tag");
  await mustRun(input.runner, "git", ["tag", "-a", archiveTag, oldTip, "-m", recutArchiveMessage(record, repoSlug)], input.repoRoot, input.env);
  await mustRun(input.runner, "git", ["push", "origin", `refs/tags/${archiveTag}`], input.repoRoot, input.env);
  await revalidateRecutPlan(input, repoSlug, branch, mainTip, oldTip, productionTags, activeCandidate, "before branch move");
  await mustRun(
    input.runner,
    "git",
    ["push", "origin", `--force-with-lease=refs/heads/${branch}:${oldTip}`, `${newTip}:refs/heads/${branch}`],
    input.repoRoot,
    input.env
  );
  await revalidateRecutPlan(input, repoSlug, branch, mainTip, newTip, productionTags, activeCandidate, "before channel write");
  await ensureStagingGithubRelease(input, repoSlug);
  const body = composeStagingChannelRecutBody(await readStagingChannelBody(input, repoSlug), record);
  await mustRun(input.runner, "gh", ["release", "edit", STAGING_CHANNEL_TAG, "--repo", repoSlug, "--notes", body], input.repoRoot, input.env);
  return { branch, version: requested, commit: newTip, trunkCommit: mainTip, trunkVersion, abandoned: [], recut: { id: record.recutId, archiveTag, oldTip, newTip, applied: true } };
}

function seriesOrdinal(series: ReleaseSeries): number {
  return series.major * 1_000_000 + series.minor;
}

export interface RemoteReleaseBranch {
  branch: string;
  series: ReleaseSeries;
  commit: string;
}

export function parseRemoteReleaseBranches(lsRemoteOutput: string): RemoteReleaseBranch[] {
  const branches: RemoteReleaseBranch[] = [];
  for (const line of lsRemoteOutput.split(/\r?\n/)) {
    const [commit, ref] = line.trim().split(/\s+/);
    if (!commit || !ref) continue;
    const branch = ref.replace(/^refs\/heads\//, "");
    const series = parseReleaseBranchSeries(branch);
    if (!series) continue;
    branches.push({ branch, series, commit });
  }
  return branches;
}

/**
 * Cuts the next stabilization branch.
 *
 * Bump inference uses the same production floor as a main staging ship, so a
 * stale `origin/main:VERSION` cannot make the recommended guard-4 remedy cut an
 * older series than the RC that a bare main ship would derive. `--version X.Y.0`
 * still names a farther intended series directly, and every series it steps over
 * must be named and reasoned for — so skipping a version is always a decision
 * someone wrote down, never a side effect of a flag.
 */
/**
 * The commit that starts a candidate line: `base` with the series version
 * written into VERSION, the candidate counter reset to 1, and the two
 * version-bearing manifests brought along.
 *
 * Setting the version is what cutting *is*. Before this the branch was cut at
 * main's tip carrying main's stale VERSION, and the number was invented later —
 * at promotion, by a `release: vX.Y.Z` commit made on top of the candidate,
 * which is why the commit that shipped was never the commit that soaked.
 *
 * Built with plumbing against a temporary index instead of a checkout: `kd
 * release cut` runs from whatever worktree the operator is in — in a Kanna task,
 * one on an unrelated branch — and cutting a branch must not disturb it.
 */
/** The paths a series version commit is allowed to touch, and nothing else. */
const SERIES_VERSION_COMMIT_PATHS = new Set([
  "VERSION",
  RELEASE_CANDIDATE_FILE,
  "apps/desktop/src-tauri/tauri.conf.json",
  "apps/desktop/src-tauri/Cargo.toml"
]);

/**
 * Is this the series version commit kd composed when it cut or recut the
 * branch?
 *
 * The recut hygiene gate refuses to move a branch carrying work origin/main
 * does not have, and it is right to — that is the check standing between a
 * recut and silently discarding a backport. But cutting now writes the series
 * version onto the branch, and that commit is branch-only by construction and
 * always will be, so without this exemption every branch `cut` creates is
 * immediately un-recuttable: exactly the operation the owner asked for by name.
 *
 * Recognised by kd's own subject *and* by touching nothing outside the version
 * files, so a genuine backport that happens to borrow the subject line is still
 * counted and still refuses.
 */
async function isSeriesVersionCommit(input: ReleaseCommandContext, sha: string): Promise<boolean> {
  const subject = await input.runner.run("git", ["log", "-1", "--format=%s", sha], {
    cwd: input.repoRoot,
    env: input.env
  });
  if (subject.exitCode !== 0 || !/^release: cut \d+\.\d+\.\d+$/.test(subject.stdout.trim())) return false;
  const changed = await input.runner.run("git", ["show", "--name-only", "--format=", sha], {
    cwd: input.repoRoot,
    env: input.env
  });
  if (changed.exitCode !== 0) return false;
  const paths = changed.stdout.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  return paths.length > 0 && paths.every((path) => SERIES_VERSION_COMMIT_PATHS.has(path));
}

async function composeSeriesVersionCommit(
  input: ReleaseCutInput,
  base: string,
  version: string
): Promise<string> {
  const indexDir = mkdtempSync(join(tmpdir(), "kd-release-cut-"));
  const env = { ...input.env, GIT_INDEX_FILE: join(indexDir, "index") };
  try {
    await mustRun(input.runner, "git", ["read-tree", base], input.repoRoot, env);

    const updates: Array<{ path: string; contents: string }> = [
      { path: "VERSION", contents: `${version}\n` },
      { path: RELEASE_CANDIDATE_FILE, contents: "1\n" }
    ];
    // The bundle stamps its version from VERSION, so these two are not what the
    // build reads — but a committed manifest that disagrees with the branch it
    // sits on is a trap for anyone reading the tree, so keep them in step.
    for (const manifest of [
      { path: "apps/desktop/src-tauri/tauri.conf.json", pattern: /"version": "[^"]*"/, replacement: `"version": "${version}"` },
      { path: "apps/desktop/src-tauri/Cargo.toml", pattern: /^version = "[^"]*"/m, replacement: `version = "${version}"` }
    ]) {
      const present = await input.runner.run("git", ["cat-file", "-e", `${base}:${manifest.path}`], {
        cwd: input.repoRoot,
        env: input.env
      });
      if (present.exitCode !== 0) continue;
      const contents = await mustRun(input.runner, "git", ["show", `${base}:${manifest.path}`], input.repoRoot, input.env);
      updates.push({ path: manifest.path, contents: `${contents.replace(manifest.pattern, manifest.replacement)}\n` });
    }

    for (const update of updates) {
      const blob = await mustRun(
        input.runner,
        "git",
        ["hash-object", "-w", "--stdin"],
        input.repoRoot,
        input.env,
        update.contents
      );
      await mustRun(
        input.runner,
        "git",
        ["update-index", "--add", "--cacheinfo", `100644,${blob},${update.path}`],
        input.repoRoot,
        env
      );
    }

    const tree = await mustRun(input.runner, "git", ["write-tree"], input.repoRoot, env);
    return await mustRun(
      input.runner,
      "git",
      ["commit-tree", tree, "-p", base, "-m", `release: cut ${version}`],
      input.repoRoot,
      input.env
    );
  } finally {
    rmSync(indexDir, { recursive: true, force: true });
  }
}

export async function cutReleaseBranch(input: ReleaseCutInput): Promise<ReleaseCutResult> {
  if (input.recut) return recutReleaseBranch(input);
  await mustRun(input.runner, "git", ["fetch", "origin", "main"], input.repoRoot, input.env);
  const commit = await mustRun(input.runner, "git", ["rev-parse", "origin/main"], input.repoRoot, input.env);
  // The caller's worktree can be stale (Kanna task worktrees fork from older
  // commits), so the series must come from the same commit the branch will
  // point at — origin/main — not from the local VERSION file.
  const trunkVersion = (await mustRun(input.runner, "git", ["show", "origin/main:VERSION"], input.repoRoot, input.env)).trim();

  const requested = input.version?.trim();
  let targetVersion: string;
  if (requested) {
    const normalized = requested.replace(/^v/, "");
    if (!/^\d+\.\d+\.0$/.test(normalized)) {
      throw new Error(
        `Invalid --version ${requested}. A series cut starts at patch 0, e.g. --version 0.2.0.`
      );
    }
    if (compareVersions(normalized, trunkVersion) <= 0) {
      throw new Error(
        `--version ${normalized} is not ahead of origin/main's VERSION (${trunkVersion}). ` +
          "A release series must be cut ahead of trunk's recorded version."
      );
    }
    targetVersion = normalized;
  } else {
    targetVersion = deriveMainStagingBaseVersion(
      trunkVersion,
      await readGreatestProductionVersion(input),
      input.bump
    ).baseVersion;
  }

  const targetSeries = releaseSeriesFromVersion(targetVersion);
  const branch = releaseSeriesBranch(targetSeries);
  const remoteBranches = parseRemoteReleaseBranches(
    await mustRun(input.runner, "git", ["ls-remote", "--heads", "origin", "refs/heads/release/*"], input.repoRoot, input.env)
  );
  if (remoteBranches.some((candidate) => candidate.branch === branch)) {
    throw new Error(
      `${branch} already exists on origin. Ship RCs from it, or name the intended next series explicitly ` +
        `(kd release cut --version X.Y.0), abandoning any series it steps over.`
    );
  }

  const trunkOrdinal = seriesOrdinal(releaseSeriesFromVersion(trunkVersion));
  const targetOrdinal = seriesOrdinal(targetSeries);
  const steppedOver = remoteBranches.filter((candidate) => {
    const ordinal = seriesOrdinal(candidate.series);
    return ordinal > trunkOrdinal && ordinal < targetOrdinal;
  });

  const named = new Set((input.abandonSeries ?? []).map((series) => series.trim()).filter(Boolean));
  const reason = input.reason?.trim() ?? "";
  const abandonedAt = new Date(input.now ?? Date.now()).toISOString();

  const pending: Array<{ candidate: RemoteReleaseBranch; existing: AbandonedSeriesRecord | null }> = [];
  const unnamed: string[] = [];
  for (const candidate of steppedOver) {
    const seriesLabel = `${candidate.series.major}.${candidate.series.minor}`;
    const seriesTags = await mustRun(
      input.runner,
      "git",
      ["ls-remote", "--tags", "origin", `v${seriesLabel}.*`],
      input.repoRoot,
      input.env
    );
    // A series that already shipped is history, not something to abandon — but
    // only production tags count. `ls-remote` expands the pattern to `*/v0.1.*`
    // and its `*` crosses `/`, so this glob also returns every
    // `v0.1.0-staging.N` prerelease. Since any series worth abandoning has
    // published RCs by definition, matching the raw output here would classify
    // every abandonment candidate as "already released" and silently skip it.
    if (hasProductionTagForSeries(seriesTags, candidate.series)) continue;
    const existing = await readAbandonedSeries(input, candidate.branch);
    if (existing) {
      pending.push({ candidate, existing });
      continue;
    }
    if (!named.has(seriesLabel)) {
      unnamed.push(seriesLabel);
      continue;
    }
    pending.push({ candidate, existing: null });
  }

  if (unnamed.length > 0) {
    throw new Error(
      `Cutting ${branch} steps over unreleased release ${unnamed.length === 1 ? "series" : "series'"} ` +
        `${unnamed.join(", ")}, which ${unnamed.length === 1 ? "is" : "are"} neither released nor abandoned. ` +
        `Name ${unnamed.length === 1 ? "it" : "them"} explicitly to record the decision: ` +
        `kd release cut --version ${targetVersion} --abandon-series ${unnamed.join(",")} --reason "<why>". ` +
        "The branch is kept and never deleted; abandoning only records that no production release will come from it."
    );
  }

  const abandoning = pending.filter((entry) => !entry.existing);
  const unknownNamed = [...named].filter(
    (series) => !steppedOver.some((candidate) => `${candidate.series.major}.${candidate.series.minor}` === series)
  );
  if (unknownNamed.length > 0) {
    throw new Error(
      `--abandon-series ${unknownNamed.join(", ")} does not name a release branch this cut steps over. ` +
        `Cutting ${branch} from a trunk at ${trunkVersion} steps over ` +
        `${steppedOver.length > 0 ? steppedOver.map((candidate) => candidate.branch).join(", ") : "no release branches"}.`
    );
  }
  if (abandoning.length > 0 && !reason) {
    throw new Error("Abandoning a release series requires --reason \"<why no production release will come from it>\".");
  }

  // The staging channel is still serving the abandoned series' candidate until
  // someone says otherwise. Abandoning the series without releasing the channel
  // would leave the next publish refused by the lineage gate with no
  // explanation, so require the reset first rather than doing it implicitly.
  if (abandoning.length > 0) {
    const remoteUrl = await mustRun(input.runner, "git", ["remote", "get-url", "origin"], input.repoRoot, input.env);
    const repoSlug = releaseRepoSlug(remoteUrl);
    const active = await resolveActiveStagingCandidate(input, repoSlug);
    if (active.error) {
      throw new Error(
        `Cannot tell whether ${STAGING_CHANNEL_TAG} still serves the series being abandoned (${active.error}). ` +
          "Refusing to abandon a series against an unreadable channel; retry once GitHub is reachable."
      );
    }
    const activeBranch = active.candidate?.sourceBranch ?? null;
    if (active.candidate && abandoning.some((entry) => entry.candidate.branch === activeBranch)) {
      const reset = await readLineageReset(input, repoSlug);
      if (!reset || normalizeStagingVersion(reset.fromVersion) !== normalizeStagingVersion(active.candidate.version)) {
        throw new Error(
          `${STAGING_CHANNEL_TAG} still serves ${active.candidate.tag} from ${activeBranch}, the series being abandoned. ` +
            "Release the channel first so the next publish is not silently refused: " +
            `kd release reset-staging --to ${branch} --reason "<why>" --confirm-abandon ${active.candidate.version}.`
        );
      }
    }
  }

  // Abandonments are recorded before the skip becomes real: a cut that fails
  // after this point leaves an audited, idempotent record and no missing one.
  for (const entry of abandoning) {
    const tag = abandonedSeriesTag(entry.candidate.branch);
    await mustRun(input.runner, "git", ["fetch", "origin", entry.candidate.branch], input.repoRoot, input.env);
    await mustRun(
      input.runner,
      "git",
      [
        "tag",
        "-f",
        "-a",
        tag,
        entry.candidate.commit,
        "-m",
        formatAbandonedSeriesMessage({ branch: entry.candidate.branch, abandonedAt, reason })
      ],
      input.repoRoot,
      input.env
    );
    await mustRun(input.runner, "git", ["push", "origin", `refs/tags/${tag}`], input.repoRoot, input.env);
  }

  const branchCommit = await composeSeriesVersionCommit(input, commit, targetVersion);
  await mustRun(input.runner, "git", ["push", "origin", `${branchCommit}:refs/heads/${branch}`], input.repoRoot, input.env);
  return {
    branch,
    version: targetVersion,
    commit: branchCommit,
    trunkCommit: commit,
    trunkVersion,
    abandoned: pending.map((entry) => ({
      series: `${entry.candidate.series.major}.${entry.candidate.series.minor}`,
      branch: entry.candidate.branch,
      commit: entry.candidate.commit,
      tag: abandonedSeriesTag(entry.candidate.branch),
      reason: entry.existing?.reason ?? reason,
      abandonedAt: entry.existing?.abandonedAt ?? abandonedAt,
      alreadyAbandoned: entry.existing !== null
    }))
  };
}

export interface ReleaseBranchCommit {
  sha: string;
  subject: string;
}

export const UNMERGED_COMMIT_REPORT_LIMIT = 20;

export function parseUnmergedReleaseCommits(logOutput: string): ReleaseBranchCommit[] {
  const commits: ReleaseBranchCommit[] = [];
  for (const line of logOutput.split(/\r?\n/)) {
    const match = /^([0-9a-f]{7,40})(?:\s+(.*))?$/.exec(line.trim());
    if (!match?.[1]) continue;
    commits.push({ sha: match[1], subject: (match[2] ?? "").trim() });
  }
  return commits;
}

async function resolveUnmergedReleaseCommitsAtTips(
  input: ReleaseCommandContext,
  mainTip: string,
  branchTip: string
): Promise<{ commits: ReleaseBranchCommit[] | null; count: number | null }> {
  const log = await input.runner.run(
    "git",
    ["log", "--no-merges", "--cherry-pick", "--right-only", "--format=%H %s", `${mainTip}...${branchTip}`],
    { cwd: input.repoRoot, env: input.env }
  );
  if (log.exitCode !== 0) return { commits: null, count: null };
  const commits = parseStrictReleaseCommitLog(log.stdout);
  if (!commits) return { commits: null, count: null };
  return { commits: commits.slice(0, UNMERGED_COMMIT_REPORT_LIMIT), count: commits.length };
}

function parseStrictReleaseCommitLog(output: string): ReleaseBranchCommit[] | null {
  const lines = output.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  const commits = parseUnmergedReleaseCommits(output);
  return commits.length === lines.length ? commits : null;
}
