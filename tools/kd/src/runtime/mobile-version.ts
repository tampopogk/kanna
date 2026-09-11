import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { bumpVersion, type ReleaseBump } from "./release";

export interface MobileVersionBumpInput {
  major: boolean;
  minor: boolean;
  patch: boolean;
  dryRun?: boolean;
}

export interface MobileVersionBumpPlan {
  currentVersion: string;
  nextVersion: string;
  bump: ReleaseBump;
  versionPath: string;
  dryRun: boolean;
}

const MOBILE_VERSION_PATTERN = /^\d+\.\d+\.\d+$/;

export async function buildMobileVersionBumpPlan(
  repoRoot: string,
  input: MobileVersionBumpInput
): Promise<MobileVersionBumpPlan> {
  const selected = (["major", "minor", "patch"] as const).filter(
    (bump) => input[bump]
  );
  if (selected.length !== 1) {
    throw new Error(
      "mobile version bump requires exactly one of --major, --minor, or --patch."
    );
  }

  const versionPath = join(repoRoot, "apps/mobile/VERSION");
  const currentVersion = (await readFile(versionPath, "utf8")).trim();
  if (!MOBILE_VERSION_PATTERN.test(currentVersion)) {
    throw new Error(
      `Mobile VERSION file at ${versionPath} is malformed; expected X.Y.Z, got ${JSON.stringify(currentVersion)}.`
    );
  }

  const bump = selected[0];
  return {
    currentVersion,
    nextVersion: bumpVersion(currentVersion, bump),
    bump,
    versionPath,
    dryRun: input.dryRun === true
  };
}

export async function executeMobileVersionBumpWithContext(
  input: MobileVersionBumpInput,
  context: { repoRoot: string }
): Promise<{ ok: true; message: string; data: MobileVersionBumpPlan }> {
  const plan = await buildMobileVersionBumpPlan(context.repoRoot, input);
  if (!plan.dryRun) {
    await writeFile(plan.versionPath, `${plan.nextVersion}\n`);
  }
  return {
    ok: true,
    message: `${plan.dryRun ? "Dry run: would bump" : "Bumped"} mobile release version ${plan.currentVersion} → ${plan.nextVersion}.`,
    data: plan
  };
}
