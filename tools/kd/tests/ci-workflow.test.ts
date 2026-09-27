import { readdirSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
const workflowsDir = resolve(repoRoot, ".github/workflows");

// Hosted CI was removed: `ci.yml` and `remote-e2e.yml` are gone, and verification
// is local (`pnpm test`, `./kd test rust`) plus the Kanna review stage. The
// config-schema Pages workflow stays because it is continuous deployment of the
// public https://schemas.kanna.build/config.schema.json contract, not a check.
//
// `linux-release-check.yml` followed them on 2026-09-26, per the owner's
// directive ("i hate remote ci. open a task to remove linux ci on gh."). It
// had been the native x86-64/arm64 build and installed-check lane, the
// apt/GnuPG interop lane, and the workflow_dispatch A/B prepared-pair upgrade
// lane; none of those have a hosted replacement. They run only by hand now,
// on the Linux dev VM: `./kd build linux-package`, and
// `./kd test linux-installed --old-artifact <deb> --new-artifact <deb>`.
const CONFIG_SCHEMA_DEPLOYMENT = "config-schema-pages.yml";
const REMOVED_CI_WORKFLOWS = ["ci.yml", "remote-e2e.yml", "linux-release-check.yml"];

function workflowFiles(): string[] {
  return readdirSync(workflowsDir, { withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => entry.name)
    .sort();
}

describe("GitHub Actions workflow set", () => {
  it("contains exactly the intended set", () => {
    expect(workflowFiles()).toEqual([CONFIG_SCHEMA_DEPLOYMENT]);
  });

  it("keeps the config-schema Pages deployment", () => {
    expect(workflowFiles()).toContain(CONFIG_SCHEMA_DEPLOYMENT);
  });

  it("does not reintroduce the removed CI workflows", () => {
    const workflows = workflowFiles();
    for (const removed of REMOVED_CI_WORKFLOWS) {
      expect(workflows, `${removed} must stay removed`).not.toContain(removed);
    }
  });
});
