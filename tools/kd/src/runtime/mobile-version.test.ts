import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { parseCliArgs } from "../cli";
import {
  buildMobileVersionBumpPlan,
  executeMobileVersionBumpWithContext
} from "./mobile-version";

const roots: string[] = [];

async function fixture(version = "1.2.3"): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), "kanna-mobile-version-"));
  roots.push(root);
  await mkdir(join(root, "apps/mobile"), { recursive: true });
  await writeFile(join(root, "apps/mobile/VERSION"), `${version}\n`);
  return root;
}

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

describe("mobile release version bump", () => {
  it("parses the supported release-planning command", () => {
    expect(parseCliArgs(["mobile", "version", "bump", "--minor", "--dry-run"])).toEqual({
      taskId: "mobile.version.bump",
      input: {
        major: false,
        minor: true,
        patch: false,
        dryRun: true
      }
    });
  });

  it.each([
    ["major", "2.0.0"],
    ["minor", "1.3.0"],
    ["patch", "1.2.4"]
  ] as const)("plans a %s bump from the independent mobile ledger", async (bump, nextVersion) => {
    const repoRoot = await fixture();
    const plan = await buildMobileVersionBumpPlan(repoRoot, {
      major: bump === "major",
      minor: bump === "minor",
      patch: bump === "patch",
      dryRun: true
    });

    expect(plan).toMatchObject({ currentVersion: "1.2.3", nextVersion, bump, dryRun: true });
    expect(await readFile(join(repoRoot, "apps/mobile/VERSION"), "utf8")).toBe("1.2.3\n");
  });

  it("writes only the mobile version ledger", async () => {
    const repoRoot = await fixture();
    await writeFile(join(repoRoot, "VERSION"), "9.8.7\n");

    const result = await executeMobileVersionBumpWithContext(
      { major: false, minor: false, patch: true },
      { repoRoot }
    );

    expect(result.message).toContain("1.2.3 → 1.2.4");
    expect(await readFile(join(repoRoot, "apps/mobile/VERSION"), "utf8")).toBe("1.2.4\n");
    expect(await readFile(join(repoRoot, "VERSION"), "utf8")).toBe("9.8.7\n");
  });

  it("requires one explicit semantic bump", async () => {
    const repoRoot = await fixture();
    await expect(
      buildMobileVersionBumpPlan(repoRoot, {
        major: false,
        minor: false,
        patch: false
      })
    ).rejects.toThrow("exactly one");
    await expect(
      buildMobileVersionBumpPlan(repoRoot, {
        major: false,
        minor: true,
        patch: true
      })
    ).rejects.toThrow("exactly one");
  });
});
