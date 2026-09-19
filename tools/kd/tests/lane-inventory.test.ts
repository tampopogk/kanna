import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  LANE_MANIFEST_PATH,
  auditLaneInventory,
  checkTriggerPathsExist,
  discoverE2eFileSuites,
  discoverKdTestSuites,
  discoverPackageScriptSuites,
  discoverShellTestSuites,
  discoverTestSuites,
  discoverVitestExcludeSuites,
  parseLaneManifest,
  parseVitestExcludeGlobs,
  parseWorkspacePackages,
  laneTriggerPaths,
  matchTriggerPaths,
  readLaneManifest,
  reconcileLaneInventory,
  validateLaneManifest,
  type DiscoveredSuiteKind,
  type LaneManifest,
  type LaneManifestEntry,
} from "../src/runtime/lane-inventory";
import { kdTestScratchDirSync } from "./test-paths";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");

/**
 * The inventory check, asserted against repository state rather than against a
 * literal in the module it tests.
 *
 * `remote-e2e-selection.test.ts` and `test-all.test.ts` both compare a literal
 * to the same literal, so nothing the repository does can break either — which
 * is how a suite was added, excluded from the collector, gated by nothing, and
 * went red for months with every test green. `ci-workflow.test.ts` reads
 * `readdirSync` instead, so adding a file breaks it, and it has held. This file
 * generalizes that: a suite's presence on disk, in a workspace manifest, in the
 * `kd` task registry or behind a Vitest `exclude` is what makes it a member,
 * and `docs/verification/lanes.json` must account for it exactly once.
 *
 * Nothing here runs a suite. The fixture trees below are directories of text.
 */

// ---------------------------------------------------------------------------
// Fixture trees
// ---------------------------------------------------------------------------

function writeTree(files: Record<string, string>): string {
  const root = kdTestScratchDirSync("lane-inventory-");
  for (const [path, contents] of Object.entries(files)) {
    const absolute = resolve(root, path);
    mkdirSync(dirname(absolute), { recursive: true });
    writeFileSync(absolute, contents);
  }
  return root;
}

function manifestJson(manifest: LaneManifest): string {
  return JSON.stringify(manifest, null, 2);
}

const FIXTURE_LANE: LaneManifestEntry = {
  id: "fixture-unit",
  description: "The fixture package's unit suite.",
  invocation: "pnpm test",
  trigger: "per-branch",
  scope: ["branch"],
  host: "any",
  hostCapabilities: ["posix"],
  triggerPaths: ["packages/thing/"],
  owner: "merge-master",
  claims: ["script:packages/thing#test"],
};

const FIXTURE_UNOWNED_LANE: LaneManifestEntry = {
  id: "fixture-gate",
  description: "The fixture package's gate suite.",
  invocation: "pnpm --dir packages/thing test:gate",
  trigger: "unowned",
  scope: ["sweep"],
  host: "any",
  hostCapabilities: ["posix"],
  triggerPaths: ["packages/thing/"],
  owner: "unowned",
  claims: [
    "script:packages/thing#test:gate",
    "e2e-file:packages/thing/src/gate.e2e.test.ts",
  ],
  note: "Nothing reaches it and nobody is accountable for its colour.",
};

/** A tree with exactly two suites, both claimed, and nothing else. */
function baseFixtureFiles(lanes: LaneManifest["lanes"]): Record<string, string> {
  return {
    "pnpm-workspace.yaml": 'packages:\n  - "packages/*"\n',
    "package.json": JSON.stringify({ name: "fixture-root", private: true }),
    "packages/thing/package.json": JSON.stringify({
      name: "@fixture/thing",
      scripts: { test: "vitest run", "test:gate": "vitest run src/gate.e2e.test.ts" },
    }),
    "packages/thing/src/gate.e2e.test.ts": "// a suite\n",
    [LANE_MANIFEST_PATH]: manifestJson({
      description: "Fixture manifest.",
      lanes,
    }),
  };
}

// ---------------------------------------------------------------------------
// Discovery reads repository state
// ---------------------------------------------------------------------------

describe("test suite discovery", () => {
  it("finds a workspace package's test scripts, including a narrow one", () => {
    const root = writeTree(baseFixtureFiles([FIXTURE_LANE, FIXTURE_UNOWNED_LANE]));
    try {
      expect(discoverPackageScriptSuites(root).map((suite) => suite.id)).toEqual([
        "script:packages/thing#test",
        "script:packages/thing#test:gate",
      ]);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  /**
   * `kd`'s standalone install ships `dist/` with no node_modules, so this reads
   * `pnpm-workspace.yaml` directly rather than pulling a YAML parser into the
   * bundle. It must handle the forms pnpm accepts for this one key.
   */
  it("reads the workspace package globs in block and flow form", () => {
    expect(
      parseWorkspacePackages(
        [
          "# a comment",
          "packages:",
          '  - "apps/*"',
          "  - packages/* # trailing comment",
          "  - 'tools/*'",
          "",
          "hoist: false",
          '  - "not/a/package"',
        ].join("\n"),
      ),
    ).toEqual(["apps/*", "packages/*", "tools/*"]);

    expect(parseWorkspacePackages('packages: ["apps/*", "tools/*"]\nhoist: false\n'))
      .toEqual(["apps/*", "tools/*"]);
    expect(parseWorkspacePackages("hoist: false\n")).toEqual([]);
  });

  it("finds a kd test subcommand by its registry id", () => {
    const root = writeTree({
      "tools/kd/src/tasks/registry.ts": [
        'export const tasks = [',
        '  { id: "test.sample", description: "" },',
        '  { id: "dev.up", description: "" },',
        "];",
      ].join("\n"),
    });
    try {
      expect(discoverKdTestSuites(root)).toEqual([
        {
          id: "kd-test:sample",
          kind: "kd-test",
          source: "tools/kd/src/tasks/registry.ts",
          detail: "./kd test sample",
        },
      ]);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("finds each Vitest exclude glob and ignores the shared spread", () => {
    const root = writeTree({
      "pkg/vitest.config.ts": [
        'import { configDefaults, defineConfig } from "vitest/config";',
        "export default defineConfig({",
        "  test: {",
        "    exclude: [...configDefaults.exclude, \"tests/e2e/mock/**\", 'tests/e2e/real/**'],",
        "  },",
        "});",
      ].join("\n"),
    });
    try {
      expect(discoverVitestExcludeSuites(root).map((suite) => suite.id)).toEqual([
        "vitest-exclude:pkg/vitest.config.ts#tests/e2e/mock/**",
        "vitest-exclude:pkg/vitest.config.ts#tests/e2e/real/**",
      ]);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("reads an exclude array that contains a nested array", () => {
    expect(
      parseVitestExcludeGlobs(
        'exclude: [...configDefaults.exclude, ["a/**", "b/**"], "c/**"],',
      ),
    ).toEqual(["a/**", "b/**", "c/**"]);
  });

  it("finds e2e and shell suites on disk and skips dependency trees", () => {
    const root = writeTree({
      "tests/thing/src/flow.e2e.test.ts": "// suite\n",
      "tests/thing/src/unit.test.ts": "// collected by the package script\n",
      "scripts/thing.test.sh": "#!/bin/sh\n",
      "node_modules/vendor/vendor.e2e.test.ts": "// not ours\n",
      ".build/generated/stale.e2e.test.ts": "// build output\n",
    });
    try {
      expect(discoverE2eFileSuites(root).map((suite) => suite.id)).toEqual([
        "e2e-file:tests/thing/src/flow.e2e.test.ts",
      ]);
      expect(discoverShellTestSuites(root).map((suite) => suite.id)).toEqual([
        "shell-test:scripts/thing.test.sh",
      ]);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

// ---------------------------------------------------------------------------
// The check fails on each way a suite can escape accounting
// ---------------------------------------------------------------------------

function auditFixture(files: Record<string, string>) {
  const root = writeTree(files);
  try {
    return auditLaneInventory(root);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

describe("lane inventory reconciliation", () => {
  it("passes when every discovered suite is claimed exactly once", () => {
    expect(auditFixture(baseFixtureFiles([FIXTURE_LANE, FIXTURE_UNOWNED_LANE])).problems)
      .toEqual([]);
  });

  it("fails when a suite is added and claimed by nothing", () => {
    const files = baseFixtureFiles([FIXTURE_LANE, FIXTURE_UNOWNED_LANE]);
    files["packages/thing/src/later.e2e.test.ts"] = "// added by a branch, gated by nothing\n";

    const problems = auditFixture(files);

    expect(problems.problems).toEqual([
      {
        kind: "unclaimed-suite",
        subject: "e2e-file:packages/thing/src/later.e2e.test.ts",
        message:
          'packages/thing/src/later.e2e.test.ts declares "e2e-file:packages/thing/src/later.e2e.test.ts"'
          + ` and no lane in ${LANE_MANIFEST_PATH} claims it`,
      },
    ]);
  });

  it("fails when a new test script is added and claimed by nothing", () => {
    const files = baseFixtureFiles([FIXTURE_LANE, FIXTURE_UNOWNED_LANE]);
    files["packages/thing/package.json"] = JSON.stringify({
      name: "@fixture/thing",
      scripts: {
        test: "vitest run",
        "test:gate": "vitest run src/gate.e2e.test.ts",
        "test:smoke": "vitest run src/smoke",
      },
    });

    expect(auditFixture(files).problems.map((problem) => problem.subject)).toEqual([
      "script:packages/thing#test:smoke",
    ]);
  });

  it("fails when two lanes claim the same suite", () => {
    const problems = auditFixture(
      baseFixtureFiles([
        FIXTURE_LANE,
        FIXTURE_UNOWNED_LANE,
        {
          ...FIXTURE_LANE,
          id: "fixture-unit-again",
          claims: ["script:packages/thing#test"],
        },
      ]),
    ).problems;

    expect(problems).toEqual([
      {
        kind: "double-claimed-suite",
        subject: "script:packages/thing#test",
        message:
          '"script:packages/thing#test" is claimed by 2 lanes (fixture-unit, fixture-unit-again)',
      },
    ]);
  });

  it("fails when a lane claims a suite that no longer exists", () => {
    const files = baseFixtureFiles([FIXTURE_LANE, FIXTURE_UNOWNED_LANE]);
    delete files["packages/thing/src/gate.e2e.test.ts"];

    expect(auditFixture(files).problems).toEqual([
      {
        kind: "phantom-claim",
        subject: "e2e-file:packages/thing/src/gate.e2e.test.ts",
        message:
          'lane "fixture-gate" claims "e2e-file:packages/thing/src/gate.e2e.test.ts",'
          + " which no longer exists in the repository",
      },
    ]);
  });

  it("fails an unowned lane that gives no reason", () => {
    const { note, ...silent } = FIXTURE_UNOWNED_LANE;
    expect(note).toBeDefined();

    expect(auditFixture(baseFixtureFiles([FIXTURE_LANE, silent])).problems).toEqual([
      {
        kind: "missing-note",
        subject: "fixture-gate",
        message: 'lane "fixture-gate" is unowned and must state why in "note"',
      },
    ]);
  });

  it("accepts an unowned lane that states why", () => {
    expect(
      validateLaneManifest({ description: "d", lanes: [FIXTURE_UNOWNED_LANE] }),
    ).toEqual([]);
  });

  it("fails a lane that claims nothing and a lane id declared twice", () => {
    const problems = validateLaneManifest({
      description: "d",
      lanes: [
        { ...FIXTURE_LANE, claims: [] },
        { ...FIXTURE_LANE, claims: ["script:packages/thing#test"] },
      ],
    });

    expect(problems.map((problem) => problem.kind)).toEqual([
      "empty-claims",
      "duplicate-lane-id",
    ]);
  });

  it("fails a trigger path that is a glob or an absolute path", () => {
    const problems = validateLaneManifest({
      description: "d",
      lanes: [{ ...FIXTURE_LANE, triggerPaths: ["packages/**", "/packages/thing/"] }],
    });

    expect(problems.map((problem) => problem.kind)).toEqual([
      "malformed-trigger-path",
      "malformed-trigger-path",
    ]);
  });

  /**
   * A prefix that matches nothing selects nothing, which is how a path-gated
   * lane quietly detaches after a directory is renamed.
   */
  it("fails a trigger path that matches nothing in the tree", () => {
    const files = baseFixtureFiles([
      { ...FIXTURE_LANE, triggerPaths: ["packages/renamed-away/"] },
      FIXTURE_UNOWNED_LANE,
    ]);

    expect(auditFixture(files).problems).toEqual([
      {
        kind: "stale-trigger-path",
        subject: "fixture-unit",
        message:
          'lane "fixture-unit" declares trigger path "packages/renamed-away/",'
          + " which matches nothing in the repository",
      },
    ]);
  });

  it("accepts an unscoped lane that declares no trigger paths", () => {
    expect(
      validateLaneManifest({ description: "d", lanes: [{ ...FIXTURE_LANE, triggerPaths: [] }] }),
    ).toEqual([]);
  });

  it("matches changed paths by prefix without matching a sibling directory", () => {
    expect(
      matchTriggerPaths(
        ["crates/kanna-server/src/lib.rs", "crates/kanna-server-extra/src/lib.rs", "README.md"],
        ["crates/kanna-server/"],
      ),
    ).toEqual(["crates/kanna-server/src/lib.rs"]);
  });

  /**
   * An empty prefix list answers "not required" for every branch. Resolving it
   * to `[]` would turn a detached gate into a silent pass, so it throws.
   */
  it("refuses to resolve trigger paths for an unknown or unscoped lane", () => {
    const root = writeTree(
      baseFixtureFiles([{ ...FIXTURE_LANE, triggerPaths: [] }, FIXTURE_UNOWNED_LANE]),
    );
    try {
      expect(laneTriggerPaths(root, "fixture-gate")).toEqual(["packages/thing/"]);
      expect(() => laneTriggerPaths(root, "fixture-unit")).toThrow(/no trigger paths/);
      expect(() => laneTriggerPaths(root, "absent")).toThrow(/declares no lane/);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("reports every unaccounted suite rather than the first", () => {
    const problems = reconcileLaneInventory({
      discovered: [
        { id: "a", kind: "e2e-file", source: "a", detail: "a" },
        { id: "b", kind: "e2e-file", source: "b", detail: "b" },
        { id: "c", kind: "e2e-file", source: "c", detail: "c" },
      ],
      manifest: {
        description: "d",
        lanes: [{ ...FIXTURE_LANE, claims: ["b"] }],
      },
    });

    expect(problems.map((problem) => problem.subject)).toEqual(["a", "c"]);
  });
});

describe("lane manifest schema", () => {
  const valid = manifestJson({ description: "d", lanes: [FIXTURE_LANE] });

  it("accepts the documented shape", () => {
    expect(parseLaneManifest(valid).lanes[0]?.id).toBe("fixture-unit");
  });

  it("rejects an unknown trigger, a non-kebab id and an unknown field", () => {
    expect(() => parseLaneManifest(valid.replace('"per-branch"', '"whenever"'))).toThrow();
    expect(() => parseLaneManifest(valid.replace('"fixture-unit"', '"Fixture Unit"'))).toThrow();
    expect(() => parseLaneManifest(valid.replace('"host":', '"hosts":'))).toThrow();
  });

  /**
   * A machine id in `hostCapabilities` would pin a lane to hardware that comes
   * and goes, and could not express one machine covering for another.
   */
  it("rejects a host capability or scope outside the closed vocabulary", () => {
    expect(() => parseLaneManifest(valid.replace('"posix"', '"jeremys-mac-studio"'))).toThrow();
    expect(() => parseLaneManifest(valid.replace('"branch"', '"sometimes"'))).toThrow();
  });

  it("requires a capability set and a scope on every lane", () => {
    expect(() =>
      parseLaneManifest(
        manifestJson({
          description: "d",
          lanes: [{ ...FIXTURE_LANE, hostCapabilities: [] }],
        }),
      ),
    ).toThrow();
    expect(() =>
      parseLaneManifest(
        manifestJson({ description: "d", lanes: [{ ...FIXTURE_LANE, scope: [] }] }),
      ),
    ).toThrow();
  });
});

// ---------------------------------------------------------------------------
// This repository
// ---------------------------------------------------------------------------

describe("this repository's lane inventory", () => {
  const audit = auditLaneInventory(repoRoot);

  /**
   * The acceptance criterion, and the reason this file runs inside `pnpm test`:
   * adding a suite that no lane claims turns this red on the branch that adds
   * it, on every machine, with no scheduler and no operator involved.
   */
  it("accounts for every discovered suite exactly once", () => {
    expect(audit.problems.map((problem) => problem.message)).toEqual([]);
  });

  /**
   * A discovery mechanism that quietly stopped finding anything would make the
   * check above vacuous for that whole class of suite. The manifest's claims
   * catch most of it as phantom claims, but a mechanism whose suites are all
   * removed at once would not be noticed, so assert each one still bites.
   */
  it.each<DiscoveredSuiteKind>([
    "package-script",
    "kd-test",
    "vitest-exclude",
    "e2e-file",
    "shell-test",
  ])("still discovers %s suites", (kind) => {
    expect(audit.discovered.filter((suite) => suite.kind === kind).length).toBeGreaterThan(0);
  });

  it("discovers every suite through exactly one identity", () => {
    const ids = discoverTestSuites(repoRoot).map((suite) => suite.id);
    expect(ids).toEqual([...new Set(ids)]);
  });

  /**
   * `unowned` is a permitted claim and a large number of them is the honest
   * state of this repository today — but each one must say why, so the count
   * is a worklist rather than a shrug.
   */
  it("gives a written reason wherever it says nobody is accountable", () => {
    const silent = readLaneManifest(repoRoot).lanes.filter(
      (lane) => (lane.trigger === "unowned" || lane.owner === "unowned") && !lane.note,
    );
    expect(silent.map((lane) => lane.id)).toEqual([]);
  });

  /**
   * The manifest is a record, not a runner: it must not be the thing that
   * decides what executes. Kept as an assertion because the temptation to make
   * a gate read it directly is exactly how the next literal gets written.
   */
  /**
   * The trigger prefixes for the remote E2E lane used to live in
   * `remote-e2e.ts` as a constant asserted against its own literal. They are
   * here now; nothing may keep a second copy.
   */
  it("holds the trigger paths that `kd test remote-e2e --if-changed` selects on", () => {
    expect(laneTriggerPaths(repoRoot, "remote-e2e-dev")).toEqual([
      "services/relay/",
      "crates/kanna-server/",
      "services/firebase-functions/",
      "apps/mobile/src/lib/",
      "tests/remote-e2e/",
      "tools/kd/",
    ]);
  });

  it("declares only trigger paths that still match something in the tree", () => {
    expect(checkTriggerPathsExist(repoRoot, audit.manifest).map((problem) => problem.message))
      .toEqual([]);
  });

  it("names a real invocation for every lane", () => {
    for (const lane of audit.manifest.lanes) {
      expect(lane.invocation.trim(), lane.id).toBe(lane.invocation);
      expect(lane.invocation.length, lane.id).toBeGreaterThan(0);
    }
  });
});
