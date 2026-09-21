import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { CommandRunner } from "./process";
import { mustRun, releaseRepoSlug, stagingTag, type ReleaseCommandContext } from "./release-command";
import {
  parsePromotionVersions,
  parseReleaseBranchSeries,
  parseSourceBranch,
  releaseSeriesBranch,
  releaseSeriesFromVersion
} from "./release-version";
import {
  composeStagingChannelBody,
  type LineageResetRecord,
  type StagingCandidate
} from "./release-lineage";

export const STAGING_CHANNEL_TAG = "desktop-staging";
export const STAGING_MANIFEST_NAME = "latest-staging.json";

export async function ensureStagingGithubRelease(input: ReleaseCommandContext, repoSlug: string): Promise<void> {
  const view = await input.runner.run("gh", ["release", "view", STAGING_CHANNEL_TAG, "--repo", repoSlug], {
    cwd: input.repoRoot,
    env: input.env
  });
  if (view.exitCode === 0) return;

  await mustRun(input.runner, "gh", [
    "release",
    "create",
    STAGING_CHANNEL_TAG,
    "--repo",
    repoSlug,
    "--title",
    "Kanna Desktop Staging",
    "--notes",
    "Pointer-only desktop staging updater channel.",
    "--prerelease"
  ], input.repoRoot, input.env);
}

interface StagingReleaseMetadata {
  tagName: string;
  targetCommitish: string;
  body: string;
  publishedAt: string | null;
  isPrerelease: boolean;
}

function parseStagingReleaseMetadata(raw: string): StagingReleaseMetadata {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== "object" || parsed === null) throw new Error("not an object");
    const record = parsed as {
      tagName?: unknown;
      targetCommitish?: unknown;
      body?: unknown;
      publishedAt?: unknown;
      isPrerelease?: unknown;
    };
    if (
      typeof record.tagName !== "string" ||
      typeof record.targetCommitish !== "string" ||
      typeof record.body !== "string" ||
      typeof record.isPrerelease !== "boolean"
    ) {
      throw new Error("missing required fields");
    }
    return {
      tagName: record.tagName,
      targetCommitish: record.targetCommitish,
      body: record.body,
      publishedAt: typeof record.publishedAt === "string" ? record.publishedAt : null,
      isPrerelease: record.isPrerelease
    };
  } catch {
    throw new Error(`Could not read immutable staging release metadata from gh output: ${raw}`);
  }
}

function validateStagingReleaseMetadata(raw: string, stagingVersion: string): StagingCandidate {
  const tag = stagingTag(stagingVersion);
  const productionVersion = parsePromotionVersions(stagingVersion).productionVersion;
  const metadata = parseStagingReleaseMetadata(raw);
  if (metadata.tagName !== tag) {
    throw new Error(`Staging release metadata tag ${metadata.tagName} does not match selected tag ${tag}.`);
  }
  if (!metadata.isPrerelease) {
    throw new Error(`${tag} is not marked as a GitHub prerelease.`);
  }
  if (!/^[0-9a-f]{40}$/i.test(metadata.targetCommitish)) {
    throw new Error(`${tag} targetCommitish is not an immutable full commit SHA: ${metadata.targetCommitish}`);
  }
  const expectedNotesPrefix = `Staging updater manifest for v${stagingVersion}`;
  if (
    metadata.body !== expectedNotesPrefix &&
    !metadata.body.startsWith(`${expectedNotesPrefix}\n`) &&
    !metadata.body.startsWith(`${expectedNotesPrefix}\r\n`)
  ) {
    throw new Error(`${tag} release notes do not identify the expected staging version ${stagingVersion}.`);
  }
  const sourceBranch = parseSourceBranch(raw);
  const expectedReleaseBranch = releaseSeriesBranch(releaseSeriesFromVersion(productionVersion));
  if (sourceBranch !== "main" && sourceBranch !== expectedReleaseBranch) {
    throw new Error(
      `${tag} has invalid or missing Source-Branch metadata; expected main or ${expectedReleaseBranch}.`
    );
  }
  return {
    version: stagingVersion,
    tag,
    commit: metadata.targetCommitish.toLowerCase(),
    sourceBranch,
    publishedAt: metadata.publishedAt
  };
}

export function parseRemoteTagCommit(raw: string, tag: string): string | null {
  const directRef = `refs/tags/${tag}`;
  const peeledRef = `${directRef}^{}`;
  let direct: string | null = null;
  let peeled: string | null = null;
  for (const line of raw.split(/\r?\n/)) {
    const [sha, ref] = line.trim().split(/\s+/, 2);
    if (!sha || !/^[0-9a-f]{40}$/i.test(sha)) continue;
    if (ref === directRef) direct = sha;
    if (ref === peeledRef) peeled = sha;
  }
  return peeled ?? direct;
}

function parseTargetCommitish(raw: string): string {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed === "object" && parsed !== null && "targetCommitish" in parsed) {
      const commit = (parsed as { targetCommitish?: unknown }).targetCommitish;
      if (typeof commit === "string" && commit.trim().length > 0) return commit.trim();
    }
  } catch {
    // Fall through to the shared error below for unparseable gh output.
  }
  throw new Error(`Could not read targetCommitish from gh release view output: ${raw}`);
}

function parsePublishedAt(raw: string): string | null {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== "object" || parsed === null) return null;
    const record = parsed as { publishedAt?: unknown; createdAt?: unknown };
    if (typeof record.publishedAt === "string" && record.publishedAt.trim().length > 0) return record.publishedAt.trim();
    if (typeof record.createdAt === "string" && record.createdAt.trim().length > 0) return record.createdAt.trim();
    return null;
  } catch {
    return null;
  }
}

export async function readStagingChannelBody(context: ReleaseCommandContext, repoSlug: string): Promise<string> {
  const view = await context.runner.run(
    "gh",
    ["release", "view", STAGING_CHANNEL_TAG, "--repo", repoSlug, "--json", "body"],
    { cwd: context.repoRoot, env: context.env }
  );
  if (view.exitCode !== 0) return "";
  try {
    const parsed = JSON.parse(view.stdout) as unknown;
    const body = typeof parsed === "object" && parsed !== null ? (parsed as { body?: unknown }).body : undefined;
    return typeof body === "string" ? body : "";
  } catch {
    return "";
  }
}

interface CandidateLookup {
  candidate: StagingCandidate | null;
  error: string | null;
}

export async function readStagingCandidate(
  context: ReleaseCommandContext,
  repoSlug: string,
  version: string
): Promise<CandidateLookup> {
  const tag = stagingTag(version);
  const view = await context.runner.run(
    "gh",
    ["release", "view", tag, "--repo", repoSlug, "--json", "targetCommitish,body,publishedAt"],
    { cwd: context.repoRoot, env: context.env }
  );
  if (view.exitCode !== 0) {
    return {
      candidate: { version, tag, commit: null, sourceBranch: null, publishedAt: null },
      error: `prerelease ${tag} could not be read from GitHub`
    };
  }
  let commit: string | null = null;
  try {
    commit = parseTargetCommitish(view.stdout);
  } catch {
    commit = null;
  }
  return {
    candidate: {
      version,
      tag,
      commit,
      sourceBranch: parseSourceBranch(view.stdout),
      publishedAt: parsePublishedAt(view.stdout)
    },
    error: commit ? null : `prerelease ${tag} records no target commit`
  };
}

export async function readVerifiedStagingCandidate(
  context: ReleaseCommandContext,
  repoSlug: string,
  version: string
): Promise<CandidateLookup> {
  const tag = stagingTag(version);
  const view = await context.runner.run(
    "gh",
    ["release", "view", tag, "--repo", repoSlug, "--json", "tagName,targetCommitish,body,publishedAt,isPrerelease"],
    { cwd: context.repoRoot, env: context.env }
  );
  if (view.exitCode !== 0) {
    return {
      candidate: { version, tag, commit: null, sourceBranch: null, publishedAt: null },
      error: `Staging prerelease not found: ${tag}`
    };
  }
  try {
    return { candidate: validateStagingReleaseMetadata(view.stdout, version), error: null };
  } catch (error) {
    let commit: string | null = null;
    try {
      commit = parseTargetCommitish(view.stdout);
    } catch {
      commit = null;
    }
    return {
      candidate: {
        version,
        tag,
        commit,
        sourceBranch: parseSourceBranch(view.stdout),
        publishedAt: parsePublishedAt(view.stdout)
      },
      error: error instanceof Error ? error.message : String(error)
    };
  }
}

async function readVersionedStagingManifestVersion(
  context: ReleaseCommandContext,
  repoSlug: string,
  tag: string
): Promise<string> {
  const manifestDir = mkdtempSync(join(tmpdir(), "kanna-release-candidate-"));
  try {
    const download = await context.runner.run(
      "gh",
      ["release", "download", tag, "--repo", repoSlug, "--pattern", STAGING_MANIFEST_NAME, "--dir", manifestDir, "--clobber"],
      { cwd: context.repoRoot, env: context.env }
    );
    if (download.exitCode !== 0) {
      throw new Error(
        `Staging manifest asset not found on ${tag}: ${download.stderr.trim() || download.stdout.trim() || STAGING_MANIFEST_NAME}`
      );
    }
    const manifestPath = join(manifestDir, STAGING_MANIFEST_NAME);
    if (!existsSync(manifestPath)) {
      throw new Error(`Staging manifest asset not found on ${tag}: ${STAGING_MANIFEST_NAME}`);
    }
    const version = parseManifestVersion(readFileSync(manifestPath, "utf8"));
    if (!version) throw new Error(`${tag}/${STAGING_MANIFEST_NAME} has no valid version.`);
    return version;
  } finally {
    rmSync(manifestDir, { recursive: true, force: true });
  }
}

export async function verifyImmutableStagingCandidate(
  context: ReleaseCommandContext,
  repoSlug: string,
  candidate: StagingCandidate
): Promise<void> {
  if (!candidate.commit) {
    throw new Error(`${candidate.tag} records no target commit, so its immutable identity cannot be verified.`);
  }
  const manifestVersion = await readVersionedStagingManifestVersion(context, repoSlug, candidate.tag);
  if (manifestVersion !== candidate.version) {
    throw new Error(
      `${candidate.tag}/${STAGING_MANIFEST_NAME} version ${manifestVersion} does not match selected version ${candidate.version}.`
    );
  }
  const tagRefs = await mustRun(
    context.runner,
    "git",
    ["ls-remote", "--tags", "origin", `refs/tags/${candidate.tag}`, `refs/tags/${candidate.tag}^{}`],
    context.repoRoot,
    context.env
  );
  const remoteCommit = parseRemoteTagCommit(tagRefs, candidate.tag);
  if (!remoteCommit) throw new Error(`Immutable staging tag not found on origin: ${candidate.tag}`);
  if (remoteCommit.toLowerCase() !== candidate.commit.toLowerCase()) {
    throw new Error(
      `${candidate.tag} tag resolves to ${remoteCommit}, but its GitHub release metadata records ${candidate.commit}.`
    );
  }
  await mustRun(context.runner, "git", ["fetch", "--no-tags", "origin", `refs/tags/${candidate.tag}`], context.repoRoot, context.env);
  const fetchedCommit = await mustRun(context.runner, "git", ["rev-parse", "FETCH_HEAD^{commit}"], context.repoRoot, context.env);
  if (fetchedCommit.toLowerCase() !== candidate.commit.toLowerCase()) {
    throw new Error(
      `Fetched ${candidate.tag} resolves to ${fetchedCommit}, but its verified immutable commit is ${candidate.commit}.`
    );
  }
}

/**
 * The candidate `desktop-staging` currently serves. An uninitialized channel is
 * not an error: the first publish creates it, and the lookup reports no
 * candidate and no error. A channel we merely failed to read, or one whose
 * candidate's metadata cannot be resolved, reports an error — moving the
 * pointer would then be unverifiable, and every caller refuses on it.
 */
export async function resolveActiveStagingCandidate(
  context: ReleaseCommandContext,
  repoSlug: string
): Promise<CandidateLookup> {
  const channel = await readStagingChannel(context, repoSlug);
  if (channel.state === "absent") return { candidate: null, error: null };
  if (channel.state === "unreadable") return { candidate: null, error: channel.error };
  return readStagingCandidate(context, repoSlug, channel.version);
}

async function productionTagExists(context: ReleaseCommandContext, productionVersion: string): Promise<boolean> {
  const result = await context.runner.run("git", ["ls-remote", "--tags", "origin", `v${productionVersion}`], {
    cwd: context.repoRoot,
    env: context.env
  });
  if (result.exitCode !== 0) return false;
  return result.stdout.trim().length > 0;
}

/**
 * Whether the production release a staging candidate is a candidate *for*
 * already exists. A malformed channel version has no production line to check.
 */
export async function activeProductionTagExists(context: ReleaseCommandContext, stagingVersion: string): Promise<boolean> {
  const productionVersion = productionVersionForStaging(stagingVersion);
  if (!productionVersion) return false;
  return productionTagExists(context, productionVersion);
}

export function productionVersionForStaging(stagingVersion: string): string | null {
  return /^(\d+\.\d+\.\d+)-staging\.\d+$/.exec(stagingVersion.trim().replace(/^v/, ""))?.[1] ?? null;
}

export async function listStagingCandidateTags(context: ReleaseCommandContext, repoSlug: string): Promise<string[]> {
  const list = await context.runner.run(
    "gh",
    ["release", "list", "--repo", repoSlug, "--limit", "100", "--json", "tagName,createdAt"],
    { cwd: context.repoRoot, env: context.env }
  );
  if (list.exitCode !== 0) return [];
  let raw = list.stdout;
  // `gh release list` caps the first query at 100. Once that page is full,
  // read the complete retained release history so an older immutable RC is not
  // mistaken for an initial candidate merely because the train kept moving.
  if (parseReleaseListEntryCount(raw) === 100) {
    const firstPage = parseStagingReleaseList(raw);
    const all = await context.runner.run(
      "gh",
      ["api", "--paginate", "--slurp", `repos/${repoSlug}/releases?per_page=100`],
      { cwd: context.repoRoot, env: context.env }
    );
    if (all.exitCode !== 0) {
      throw new Error(
        "Could not read complete GitHub release history after the first 100 entries: " +
          (all.stderr.trim() || all.stdout.trim() || "gh api --paginate failed")
      );
    }
    const complete = parsePaginatedStagingReleaseList(all.stdout);
    const completeTags = new Set(complete.map((release) => release.tag));
    if (firstPage.some((release) => !completeTags.has(release.tag))) {
      throw new Error("Could not verify complete GitHub release history: paginated output omitted entries from the first page.");
    }
    return complete.sort(compareStagingReleasesDesc).map((release) => release.tag);
  }
  return parseStagingReleaseList(raw).sort(compareStagingReleasesDesc).map((release) => release.tag);
}

interface StagingRelease {
  tag: string;
  createdAt: string;
}

function parseReleaseListEntryCount(raw: string): number | null {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return null;
    return parsed.length;
  } catch {
    return null;
  }
}

function parseStagingReleaseList(raw: string): StagingRelease[] {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (Array.isArray(parsed)) {
      const entries = parsed.flatMap((item) => Array.isArray(item) ? item : [item]);
      return entries.flatMap((item) => {
        if (typeof item !== "object" || item === null) return [];
        const record = item as { tagName?: unknown; tag_name?: unknown; createdAt?: unknown; created_at?: unknown };
        const tag = typeof record.tagName === "string"
          ? record.tagName
          : typeof record.tag_name === "string"
            ? record.tag_name
            : "";
        const createdAt = typeof record.createdAt === "string"
          ? record.createdAt
          : typeof record.created_at === "string"
            ? record.created_at
            : "";
        return /^v\d+\.\d+\.\d+-staging\.\d+$/.test(tag) ? [{ tag, createdAt }] : [];
      });
    }
  } catch {
    // Fall through to parsing gh's tabular output, which is easier to mock in tests.
  }

  return raw.split(/\r?\n/).flatMap((line) => {
    const columns = line.trim().split("\t");
    const tag = columns.find((column) => /^v\d+\.\d+\.\d+-staging\.\d+$/.test(column.trim()))?.trim() ?? "";
    if (!tag) return [];
    return [{ tag, createdAt: columns.at(-1)?.trim() ?? "" }];
  });
}

function parsePaginatedStagingReleaseList(raw: string): StagingRelease[] {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed) || parsed.length === 0 || !parsed.every((page) => Array.isArray(page))) {
      throw new Error("expected an array of release pages");
    }
    const entries = parsed.flat();
    if (entries.length < 100) throw new Error("paginated history omitted entries from the full first page");
    return entries.flatMap((item) => {
      if (typeof item !== "object" || item === null) throw new Error("release entry is not an object");
      const record = item as { tag_name?: unknown; created_at?: unknown };
      if (typeof record.tag_name !== "string" || typeof record.created_at !== "string") {
        throw new Error("release entry is missing tag_name or created_at");
      }
      return /^v\d+\.\d+\.\d+-staging\.\d+$/.test(record.tag_name)
        ? [{ tag: record.tag_name, createdAt: record.created_at }]
        : [];
    });
  } catch (error) {
    throw new Error(
      "Could not parse complete GitHub release history from gh api --paginate: " +
        (error instanceof Error ? error.message : String(error))
    );
  }
}

function compareStagingReleasesDesc(left: StagingRelease, right: StagingRelease): number {
  const leftTime = Date.parse(left.createdAt);
  const rightTime = Date.parse(right.createdAt);
  if (!Number.isNaN(leftTime) || !Number.isNaN(rightTime)) {
    return (Number.isNaN(rightTime) ? 0 : rightTime) - (Number.isNaN(leftTime) ? 0 : leftTime);
  }
  return right.tag.localeCompare(left.tag, undefined, { numeric: true });
}

export interface ReleaseResetStagingInput {
  repoRoot: string;
  /** Branch the channel is being handed to: `main` or `release/X.Y`. */
  toBranch: string;
  reason: string;
  /** Must name the exact staging version being abandoned. */
  confirmAbandon: string;
  dryRun: boolean;
  now?: number;
  env: NodeJS.ProcessEnv;
  runner: CommandRunner;
}

export interface ReleaseResetStagingResult {
  from: { version: string; tag: string; commit: string | null; sourceBranch: string | null };
  to: { branch: string };
  reason: string;
  resetAt: string;
  applied: boolean;
}

/**
 * The one deliberate, non-linear staging transition.
 *
 * Ordinary ships only ever move the channel forward. Some transitions are
 * legitimately non-linear — abandoning a stale release soak, handing the
 * channel to an older series for a hotfix — and the answer to those is not a
 * weaker ship guard but a separate, loudly named operation that records what
 * was abandoned and why. It builds nothing, publishes nothing, and does not
 * repoint the manifest: staging users keep running the candidate they have
 * until the next publish. It authorizes exactly the next publish that leaves
 * this candidate for the named branch, so it cannot silently license a second
 * divergence later.
 */
export async function resetStagingLineage(input: ReleaseResetStagingInput): Promise<ReleaseResetStagingResult> {
  const toBranch = input.toBranch.trim();
  if (toBranch !== "main" && !parseReleaseBranchSeries(toBranch)) {
    throw new Error(`Invalid --to ${input.toBranch || "(empty)"}. Expected main or release/X.Y.`);
  }
  const reason = input.reason.trim();
  if (!reason) {
    throw new Error("release reset-staging requires --reason \"<why this lineage is being abandoned>\".");
  }
  const confirm = input.confirmAbandon.trim().replace(/^v/, "");
  if (!confirm) {
    throw new Error(
      "release reset-staging requires --confirm-abandon <active-staging-version>. Run kd release status to read it."
    );
  }

  const remoteUrl = await mustRun(input.runner, "git", ["remote", "get-url", "origin"], input.repoRoot, input.env);
  const repoSlug = releaseRepoSlug(remoteUrl);
  const active = await resolveActiveStagingCandidate(input, repoSlug);
  if (!active.candidate) {
    throw new Error(
      `${STAGING_CHANNEL_TAG} has no active staging candidate, so there is no lineage to abandon. Ship a staging RC instead.`
    );
  }
  if (confirm !== active.candidate.version) {
    throw new Error(
      `--confirm-abandon ${confirm} does not match the active staging candidate ${active.candidate.version}. ` +
        "Run kd release status and pass the exact active version to confirm what is being abandoned."
    );
  }

  const record: LineageResetRecord = {
    resetAt: new Date(input.now ?? Date.now()).toISOString(),
    fromVersion: active.candidate.version,
    fromCommit: active.candidate.commit,
    fromSourceBranch: active.candidate.sourceBranch,
    toBranch,
    reason
  };

  if (!input.dryRun) {
    await ensureStagingGithubRelease(input, repoSlug);
    const body = composeStagingChannelBody(await readStagingChannelBody(input, repoSlug), record);
    await mustRun(
      input.runner,
      "gh",
      ["release", "edit", STAGING_CHANNEL_TAG, "--repo", repoSlug, "--notes", body],
      input.repoRoot,
      input.env
    );
  }

  return {
    from: {
      version: active.candidate.version,
      tag: active.candidate.tag,
      commit: active.candidate.commit,
      sourceBranch: active.candidate.sourceBranch
    },
    to: { branch: toBranch },
    reason,
    resetAt: record.resetAt,
    applied: !input.dryRun
  };
}

function parseManifestVersion(raw: string): string | null {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== "object" || parsed === null) return null;
    const version = (parsed as { version?: unknown }).version;
    return typeof version === "string" && version.length > 0 ? version : null;
  } catch {
    return null;
  }
}

/**
 * What the `desktop-staging` pointer says, as three genuinely different
 * outcomes rather than "version or not".
 *
 * `absent` and `unreadable` look identical from a single failed command, and
 * conflating them is what makes a release tool fail open: a rate limit, an
 * expired token, or a GitHub 5xx would otherwise read as "no channel yet" and
 * skip every gate on the way to repointing a live channel. Only positive
 * evidence that the channel has no candidate — the release does not exist, or
 * it exists and carries no manifest asset — counts as `absent`. Everything else
 * that stops us reading the pointer is `unreadable`, and refuses.
 */
export type StagingChannelRead =
  | { state: "absent"; detail: string }
  | { state: "unreadable"; error: string }
  | { state: "active"; version: string };

// gh's 404 wording. Anything else that fails is treated as a real error, so an
// unrecognized failure fails closed rather than reading as an empty channel.
const CHANNEL_NOT_FOUND_PATTERN = /release not found|404|could not find release/i;

function parseChannelAssetNames(raw: string): string[] | null {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== "object" || parsed === null) return null;
    const assets = (parsed as { assets?: unknown }).assets;
    if (!Array.isArray(assets)) return null;
    return assets.flatMap((asset) =>
      typeof asset === "object" && asset !== null && typeof (asset as { name?: unknown }).name === "string"
        ? [(asset as { name: string }).name]
        : []
    );
  } catch {
    return null;
  }
}

export async function readStagingChannel(input: ReleaseCommandContext, repoSlug: string): Promise<StagingChannelRead> {
  // Asset presence is data, not an error string: ask for the asset list first
  // so "the channel has no manifest" is a positive answer rather than a guess
  // about why a download failed.
  const view = await input.runner.run(
    "gh",
    ["release", "view", STAGING_CHANNEL_TAG, "--repo", repoSlug, "--json", "assets"],
    { cwd: input.repoRoot, env: input.env }
  );
  if (view.exitCode !== 0) {
    const message = view.stderr.trim() || view.stdout.trim() || "gh release view failed.";
    if (CHANNEL_NOT_FOUND_PATTERN.test(message)) {
      return { state: "absent", detail: `${STAGING_CHANNEL_TAG} does not exist yet.` };
    }
    return { state: "unreadable", error: `could not read ${STAGING_CHANNEL_TAG}: ${message}` };
  }
  const assetNames = parseChannelAssetNames(view.stdout);
  if (!assetNames) {
    return { state: "unreadable", error: `could not parse the ${STAGING_CHANNEL_TAG} asset list from gh.` };
  }
  if (!assetNames.includes(STAGING_MANIFEST_NAME)) {
    return { state: "absent", detail: `${STAGING_CHANNEL_TAG} carries no ${STAGING_MANIFEST_NAME} yet.` };
  }

  const manifestDir = mkdtempSync(join(tmpdir(), "kanna-release-status-"));
  try {
    const download = await input.runner.run(
      "gh",
      [
        "release",
        "download",
        STAGING_CHANNEL_TAG,
        "--repo",
        repoSlug,
        "--pattern",
        STAGING_MANIFEST_NAME,
        "--dir",
        manifestDir,
        "--clobber"
      ],
      { cwd: input.repoRoot, env: input.env }
    );
    if (download.exitCode !== 0) {
      return {
        state: "unreadable",
        error: `could not download ${STAGING_MANIFEST_NAME}: ${download.stderr.trim() || download.stdout.trim() || "GitHub release download failed."}`
      };
    }
    const manifestPath = join(manifestDir, STAGING_MANIFEST_NAME);
    if (!existsSync(manifestPath)) {
      return { state: "unreadable", error: `${STAGING_MANIFEST_NAME} was not downloaded.` };
    }
    const version = parseManifestVersion(readFileSync(manifestPath, "utf8"));
    return version
      ? { state: "active", version }
      : { state: "unreadable", error: `${STAGING_MANIFEST_NAME} has no valid version.` };
  } finally {
    rmSync(manifestDir, { recursive: true, force: true });
  }
}
