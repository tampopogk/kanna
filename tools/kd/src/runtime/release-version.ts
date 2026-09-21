import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { mustRun, releaseRepoSlug, type ReleaseCommandContext } from "./release-command";

export type ReleaseBump = "major" | "minor" | "patch";

export function bumpVersion(sourceVersion: string, bump: ReleaseBump): string {
  const [majorRaw, minorRaw, patchRaw] = sourceVersion.split(".");
  let major = Number.parseInt(majorRaw ?? "0", 10);
  let minor = Number.parseInt(minorRaw ?? "0", 10);
  let patch = Number.parseInt(patchRaw ?? "0", 10);
  if ([major, minor, patch].some(Number.isNaN)) {
    throw new Error(`Invalid VERSION: ${sourceVersion}`);
  }
  if (bump === "major") {
    major += 1;
    minor = 0;
    patch = 0;
  } else if (bump === "minor") {
    minor += 1;
    patch = 0;
  } else {
    patch += 1;
  }
  return `${major}.${minor}.${patch}`;
}

export function readCurrentVersion(repoRoot: string): string {
  return readFileSync(join(repoRoot, "VERSION"), "utf8").trim();
}

export const RELEASE_CANDIDATE_FILE = "VERSION_RC";

/**
 * The candidate counter for the version in `VERSION`, committed beside it.
 *
 * A release branch carries the version it will ship under, so a candidate of
 * that version is fully described by the two committed files and nothing has to
 * be counted at ship time. That is what makes the RC number a property of the
 * commit: rebuild the commit, get the same candidate. It also means a
 * production build of that same commit simply ignores this file, which is why
 * promotion has no version to write and needs no commit of its own.
 */
export function readReleaseCandidateNumber(repoRoot: string): number {
  const path = join(repoRoot, RELEASE_CANDIDATE_FILE);
  if (!existsSync(path)) {
    throw new Error(
      `${RELEASE_CANDIDATE_FILE} is missing. A release branch carries its version in VERSION and its ` +
        `candidate number in ${RELEASE_CANDIDATE_FILE}; commit one (starting at 1) before shipping a candidate.`
    );
  }
  const raw = readFileSync(path, "utf8").trim();
  if (!/^\d+$/.test(raw)) {
    throw new Error(`${RELEASE_CANDIDATE_FILE} must hold a non-negative integer candidate number, not ${JSON.stringify(raw)}.`);
  }
  return Number.parseInt(raw, 10);
}

/**
 * Splits a published version into the two values the build reads.
 *
 * The staging bundle composes its version as `VERSION`-staging-`VERSION_RC`, so
 * writing a fully-suffixed string into VERSION stamps the suffix twice: an RC
 * published as `0.5.0-staging.3` built as `0.5.0-staging.3-staging.1`, which
 * compares *greater* than the feed it is supposed to update from, so no
 * installed staging client would ever move. Every path that writes a version
 * goes through here, so what kd publishes and what Bazel stamps cannot drift.
 */
export function splitPublishedVersion(version: string): { base: string; candidate: number } {
  const match = /^(\d+\.\d+\.\d+)-staging\.(\d+)$/.exec(version.trim().replace(/^v/, ""));
  if (!match) return { base: version, candidate: 0 };
  return { base: match[1] ?? version, candidate: Number.parseInt(match[2] ?? "0", 10) };
}

/**
 * Writes the version files so that a build of this worktree produces exactly
 * `version` — the base in VERSION and the candidate counter in VERSION_RC.
 */
export function writeReleaseVersionFiles(repoRoot: string, version: string): void {
  const { base, candidate } = splitPublishedVersion(version);
  syncVersionFiles(repoRoot, base);
  // A production build reads VERSION alone and ignores the counter, so the
  // counter is written only when the version being built names a candidate.
  if (candidate > 0) writeFileSync(join(repoRoot, RELEASE_CANDIDATE_FILE), `${candidate}\n`);
}

function syncVersionFiles(repoRoot: string, version: string): void {
  writeFileSync(join(repoRoot, "VERSION"), `${version}\n`);
  const tauriPath = join(repoRoot, "apps", "desktop", "src-tauri", "tauri.conf.json");
  const cargoPath = join(repoRoot, "apps", "desktop", "src-tauri", "Cargo.toml");
  writeFileSync(tauriPath, readFileSync(tauriPath, "utf8").replace(/"version": "[^"]*"/, `"version": "${version}"`));
  writeFileSync(cargoPath, readFileSync(cargoPath, "utf8").replace(/^version = "[^"]*"/m, `version = "${version}"`));
}

function versionFilePaths(repoRoot: string): string[] {
  return [
    join(repoRoot, "VERSION"),
    join(repoRoot, RELEASE_CANDIDATE_FILE),
    join(repoRoot, "apps", "desktop", "src-tauri", "tauri.conf.json"),
    join(repoRoot, "apps", "desktop", "src-tauri", "Cargo.toml")
  ];
}

export function snapshotVersionFiles(repoRoot: string): Array<{ path: string; contents: string }> {
  // A file the checkout does not carry is restored by removing it again, so a
  // ship never leaves one behind in a tree that did not have it.
  return versionFilePaths(repoRoot)
    .filter((path) => existsSync(path))
    .map((path) => ({ path, contents: readFileSync(path, "utf8") }));
}

export function restoreVersionFiles(repoRoot: string, snapshot: Array<{ path: string; contents: string }>): void {
  const restored = new Set(snapshot.map((file) => file.path));
  for (const file of snapshot) {
    writeFileSync(file.path, file.contents);
  }
  // A file the build created but the checkout never carried is removed, not
  // left behind for the next ship's clean-worktree check to trip over.
  for (const path of versionFilePaths(repoRoot)) {
    if (!restored.has(path) && existsSync(path)) rmSync(path);
  }
}

export interface ReleaseSeries {
  major: number;
  minor: number;
}

export function releaseSeriesFromVersion(version: string): ReleaseSeries {
  const match = /^(\d+)\.(\d+)\.\d+/.exec(version.replace(/^v/, ""));
  const major = Number.parseInt(match?.[1] ?? "", 10);
  const minor = Number.parseInt(match?.[2] ?? "", 10);
  if (Number.isNaN(major) || Number.isNaN(minor)) {
    throw new Error(`Invalid version: ${version}`);
  }
  return { major, minor };
}

export function releaseSeriesBranch(series: ReleaseSeries): string {
  return `release/${series.major}.${series.minor}`;
}

export function parseReleaseBranchSeries(branchName: string): ReleaseSeries | null {
  const match = /^release\/(\d+)\.(\d+)$/.exec(branchName.trim());
  if (!match) return null;
  const major = Number.parseInt(match[1] ?? "", 10);
  const minor = Number.parseInt(match[2] ?? "", 10);
  if (Number.isNaN(major) || Number.isNaN(minor)) return null;
  return { major, minor };
}

/**
 * Whether `git ls-remote --tags origin 'vX.Y.*'` output contains a real
 * production tag for the series. The glob matches prereleases too, so the
 * decision has to come from the ref names, not from the output being non-empty.
 */
export function hasProductionTagForSeries(tagsOutput: string, series: ReleaseSeries): boolean {
  const pattern = new RegExp(`^(?:refs/tags/)?v${series.major}\\.${series.minor}\\.(\\d+)(?:\\^\\{\\})?$`);
  return tagsOutput.split(/\r?\n/).some((line) => {
    const ref = line.trim().split(/\s+/).at(-1) ?? "";
    return pattern.test(ref);
  });
}

export function nextSeriesPatchVersion(tagsOutput: string, series: ReleaseSeries): string {
  const pattern = new RegExp(`^(?:refs/tags/)?v${series.major}\\.${series.minor}\\.(\\d+)(?:\\^\\{\\})?$`);
  const patches: number[] = [];
  for (const line of tagsOutput.split(/\r?\n/)) {
    const ref = line.trim().split(/\s+/).at(-1) ?? "";
    const match = pattern.exec(ref);
    if (!match) continue;
    const value = Number.parseInt(match[1] ?? "", 10);
    if (!Number.isNaN(value)) patches.push(value);
  }
  if (patches.length === 0) return `${series.major}.${series.minor}.0`;
  return `${series.major}.${series.minor}.${Math.max(...patches) + 1}`;
}

export interface MainStagingVersionFloor {
  versionFile: string;
  greatestProductionVersion: string;
  baseVersion: string;
  detail: string;
}

export function deriveMainStagingBaseVersion(
  versionFile: string,
  greatestProductionVersion: string | null,
  bump: ReleaseBump
): { baseVersion: string; versionFloor: MainStagingVersionFloor | null } {
  const floorApplied =
    greatestProductionVersion !== null && compareVersions(versionFile, greatestProductionVersion) < 0;
  const sourceVersion = floorApplied ? greatestProductionVersion : versionFile;
  const baseVersion = bumpVersion(sourceVersion, bump);
  return {
    baseVersion,
    versionFloor: floorApplied && greatestProductionVersion
      ? {
          versionFile,
          greatestProductionVersion,
          baseVersion,
          detail:
            `VERSION ${versionFile} lags greatest production semantic version v${greatestProductionVersion}; ` +
            `derived main staging version ${baseVersion} from the production floor.`
        }
      : null
  };
}

function parseGreatestProductionVersion(raw: string): string | null {
  const parsed = JSON.parse(raw) as unknown;
  if (!Array.isArray(parsed)) throw new Error("Could not parse production releases from gh output.");
  if (parsed.length === 0) return null;

  let greatest: string | null = null;
  for (const item of parsed) {
    if (typeof item !== "object" || item === null) {
      throw new Error("Could not parse production releases from gh output.");
    }
    const record = item as { tagName?: unknown; isPrerelease?: unknown };
    if (
      record.isPrerelease !== false ||
      typeof record.tagName !== "string" ||
      !/^v\d+\.\d+\.\d+$/.test(record.tagName)
    ) {
      throw new Error(`Production release metadata is invalid: ${raw}`);
    }
    const version = record.tagName.slice(1);
    if (greatest === null || compareVersions(version, greatest) > 0) greatest = version;
  }
  return greatest;
}

export async function readGreatestProductionVersion(input: ReleaseCommandContext): Promise<string | null> {
  const remoteUrl = await mustRun(input.runner, "git", ["remote", "get-url", "origin"], input.repoRoot, input.env);
  const repoSlug = releaseRepoSlug(remoteUrl);
  const raw = await mustRun(
    input.runner,
    "gh",
    [
      "release",
      "list",
      "--repo",
      repoSlug,
      "--limit",
      "1000",
      "--exclude-drafts",
      "--exclude-pre-releases",
      "--json",
      "tagName,isPrerelease"
    ],
    input.repoRoot,
    input.env
  );
  return parseGreatestProductionVersion(raw);
}

export const SOURCE_BRANCH_TRAILER = "Source-Branch:";

export function parseSourceBranch(rawView: string): string | null {
  try {
    const parsed = JSON.parse(rawView) as unknown;
    if (typeof parsed !== "object" || parsed === null) return null;
    const body = (parsed as { body?: unknown }).body;
    if (typeof body !== "string") return null;
    const match = /^Source-Branch:[ \t]*(\S+)[ \t]*$/m.exec(body);
    return match?.[1] ?? null;
  } catch {
    return null;
  }
}

export interface PromotionVersions {
  stagingVersion: string;
  stagingTag: string;
  productionVersion: string;
}

export function parsePromotionVersions(promoteFrom: string): PromotionVersions {
  const stagingVersion = promoteFrom.trim().replace(/^v/, "");
  const match = /^(\d+\.\d+\.\d+)-staging\.\d+$/.exec(stagingVersion);
  const productionVersion = match?.[1];
  if (!productionVersion) {
    throw new Error(`Invalid staging version to promote: ${promoteFrom}. Expected X.Y.Z-staging.N (a staging prerelease version).`);
  }
  return { stagingVersion, stagingTag: `v${stagingVersion}`, productionVersion };
}

export function abandonedSeriesTag(branch: string): string {
  return `abandoned/${branch}`;
}

export function compareVersions(left: string, right: string): number {
  interface SemanticVersion {
    core: number[];
    prerelease: string[];
  }
  const parse = (value: string): SemanticVersion => {
    const match = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/.exec(
      value.trim().replace(/^v/, "")
    );
    if (!match) throw new Error(`Cannot compare versions ${left} and ${right}.`);
    return {
      core: [match[1], match[2], match[3]].map((part) => Number.parseInt(part ?? "", 10)),
      prerelease: match[4]?.split(".") ?? []
    };
  };
  const a = parse(left);
  const b = parse(right);
  for (let index = 0; index < 3; index += 1) {
    const leftPart = a.core[index] ?? 0;
    const rightPart = b.core[index] ?? 0;
    if (leftPart !== rightPart) return leftPart < rightPart ? -1 : 1;
  }
  if (a.prerelease.length === 0 || b.prerelease.length === 0) {
    if (a.prerelease.length === b.prerelease.length) return 0;
    return a.prerelease.length === 0 ? 1 : -1;
  }
  const identifiers = Math.max(a.prerelease.length, b.prerelease.length);
  for (let index = 0; index < identifiers; index += 1) {
    const leftIdentifier = a.prerelease[index];
    const rightIdentifier = b.prerelease[index];
    if (leftIdentifier === undefined || rightIdentifier === undefined) {
      return leftIdentifier === undefined ? -1 : 1;
    }
    if (leftIdentifier === rightIdentifier) continue;
    const leftNumeric = /^\d+$/.test(leftIdentifier);
    const rightNumeric = /^\d+$/.test(rightIdentifier);
    if (leftNumeric && rightNumeric) {
      return Number.parseInt(leftIdentifier, 10) < Number.parseInt(rightIdentifier, 10) ? -1 : 1;
    }
    if (leftNumeric !== rightNumeric) return leftNumeric ? -1 : 1;
    return leftIdentifier < rightIdentifier ? -1 : 1;
  }
  return 0;
}

export interface AbandonedSeriesRecord {
  abandonedAt: string | null;
  reason: string | null;
}

export function formatAbandonedSeriesMessage(args: { branch: string; abandonedAt: string; reason: string }): string {
  return `Abandoned ${args.branch} at ${args.abandonedAt}\n\nReason: ${args.reason}\n`;
}

export function parseAbandonedSeriesMessage(message: string): AbandonedSeriesRecord {
  const at = /^Abandoned\s+\S+\s+at\s+(\S+)\s*$/m.exec(message);
  const reason = /^Reason:[ \t]*(.+?)[ \t]*$/m.exec(message);
  return { abandonedAt: at?.[1] ?? null, reason: reason?.[1] ?? null };
}

async function remoteTagExists(context: ReleaseCommandContext, tag: string): Promise<boolean> {
  const result = await context.runner.run("git", ["ls-remote", "--tags", "origin", `refs/tags/${tag}`], {
    cwd: context.repoRoot,
    env: context.env
  });
  return result.exitCode === 0 && result.stdout.trim().length > 0;
}

/**
 * Whether a release series has been deliberately abandoned. Recorded as an
 * annotated `abandoned/release/X.Y` tag rather than by deleting the branch: the
 * branch and its history stay readable, the record carries who/when/why, and
 * both `ship` and `promote` can refuse the series without special-casing.
 */
export async function readAbandonedSeries(
  context: ReleaseCommandContext,
  branch: string
): Promise<AbandonedSeriesRecord | null> {
  const tag = abandonedSeriesTag(branch);
  if (!(await remoteTagExists(context, tag))) return null;
  await context.runner.run("git", ["fetch", "origin", `+refs/tags/${tag}:refs/tags/${tag}`], {
    cwd: context.repoRoot,
    env: context.env
  });
  const contents = await context.runner.run("git", ["for-each-ref", "--format=%(contents)", `refs/tags/${tag}`], {
    cwd: context.repoRoot,
    env: context.env
  });
  if (contents.exitCode !== 0) return { abandonedAt: null, reason: null };
  return parseAbandonedSeriesMessage(contents.stdout);
}
