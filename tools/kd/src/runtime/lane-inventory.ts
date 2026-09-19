import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { z } from "zod";

/**
 * The lane manifest and the discovery that keeps it honest.
 *
 * Two test lanes rotted in this repository for one reason, and the reason is
 * precise. `remote-e2e-selection.test.ts` asserts `REMOTE_E2E_TRIGGER_PATHS`
 * equals a literal declared in the module it tests;  `test-all.test.ts` asserts
 * `buildTestAllCommands()` equals a literal declared in the module it tests.
 * Both compare a literal to itself, so nothing in the repository's actual state
 * can break either — a suite could be added, excluded from the collector and
 * gated by nothing without a single test going red. That is how the desktop
 * mock lane "rotted until a third of the suite was red" (`test-all.ts`), and it
 * is where `tests/remote-e2e` sits today.
 *
 * The control that did *not* rot is `ci-workflow.test.ts`, which asserts
 * against `readdirSync(".github/workflows")`: adding a file breaks it. This
 * module generalizes exactly that difference. Suites are **discovered from
 * repository state** through the mechanisms by which a suite has actually
 * hidden, and `docs/verification/lanes.json` must claim each discovered suite
 * exactly once. An honest `unowned` claim with a written reason is permitted;
 * silence is not.
 *
 * Discovery is deliberately not a runner: nothing here executes a suite.
 */

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

/** How a lane is reached today — reality, not intent. */
export const LANE_TRIGGERS = [
  "per-branch",
  "post-merge",
  "periodic",
  "on-demand",
  "operator-only",
  "unowned",
] as const;

export type LaneTrigger = (typeof LANE_TRIGGERS)[number];

/**
 * Which selector includes a lane, once selectors exist. `trigger` says how a
 * lane is reached *today*; `scope` says where it belongs. Nothing reads this
 * field yet — the runner, the sweep and the ship gate are later increments —
 * so declaring a scope wires nothing into any gate.
 */
export const LANE_SCOPES = ["branch", "sweep", "release", "operator"] as const;

export type LaneScope = (typeof LANE_SCOPES)[number];

/**
 * What a host must provide for a lane to run, as a closed set of capabilities
 * and never a machine id. Repo ids are machine-local and laptops come and go,
 * so a manifest pinned to a machine cannot express "the Studio covered for the
 * MBP today" — a capability set can.
 */
export const HOST_CAPABILITIES = [
  "posix",
  "macos",
  "macos-gui",
  "linux",
  "linux-gui",
  "linux-x86_64",
  "linux-root",
  "bazel",
  "real-pty",
  "ios-simulator",
  "physical-device",
  "appium",
  "desktop-server",
  "firebase-emulators",
  "lan-peer-hosts",
  "live-provider-credentials",
  "cloud-staging-credentials",
  "cloud-production-credentials",
] as const;

export type HostCapability = (typeof HOST_CAPABILITIES)[number];

/** The `owner` value that declares, in the open, that nobody is accountable. */
export const UNOWNED = "unowned";

const laneEntrySchema = z
  .object({
    id: z
      .string()
      .regex(/^[a-z0-9]+(-[a-z0-9]+)*$/, "lane id must be kebab-case"),
    description: z.string().min(1),
    invocation: z.string().min(1),
    trigger: z.enum(LANE_TRIGGERS),
    /** Prose host requirement for a reader; `hostCapabilities` is authoritative. */
    host: z.string().min(1),
    /** The closed capability set a selector matches a host against. */
    hostCapabilities: z.array(z.enum(HOST_CAPABILITIES)).min(1),
    /** Which selectors should include this lane. Declarative; nothing reads it yet. */
    scope: z.array(z.enum(LANE_SCOPES)).min(1),
    /**
     * Path prefixes that make this lane worth running for a branch, the
     * per-lane generalization of the deleted remote-e2e.yml `paths:` filter.
     * Matched with `startsWith` against repo-relative paths. An empty list
     * means the lane is unscoped: any change may matter to it.
     */
    triggerPaths: z.array(z.string().min(1)),
    owner: z.string().min(1),
    /**
     * The discovered suite ids this lane accounts for, written out in full.
     * Exact ids rather than globs on purpose: a glob would absorb a new suite
     * silently, which is the failure this manifest exists to make impossible.
     */
    claims: z.array(z.string().min(1)),
    note: z.string().min(1).optional(),
  })
  .strict();

const laneManifestSchema = z
  .object({
    $schema: z.string().optional(),
    description: z.string().min(1),
    lanes: z.array(laneEntrySchema),
  })
  .strict();

export type LaneManifestEntry = z.infer<typeof laneEntrySchema>;
export type LaneManifest = z.infer<typeof laneManifestSchema>;

export const LANE_MANIFEST_PATH = "docs/verification/lanes.json";

export function parseLaneManifest(source: string): LaneManifest {
  return laneManifestSchema.parse(JSON.parse(source));
}

export function readLaneManifest(repoRoot: string): LaneManifest {
  return parseLaneManifest(readFileSync(resolve(repoRoot, LANE_MANIFEST_PATH), "utf8"));
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

export type DiscoveredSuiteKind =
  | "package-script"
  | "kd-test"
  | "vitest-exclude"
  | "e2e-file"
  | "shell-test";

export interface DiscoveredSuite {
  /** Stable identity, and exactly what a manifest entry writes in `claims`. */
  id: string;
  kind: DiscoveredSuiteKind;
  /** Repo-relative file this suite was discovered in. */
  source: string;
  /** What was found there: a script body, an exclude glob, a path. */
  detail: string;
}

/** Directories no discovery walk descends into. */
const SKIPPED_DIRECTORIES = new Set([
  "node_modules",
  ".git",
  ".build",
  ".turbo",
  ".tmp",
  ".kanna-worktrees",
  "dist",
  "target",
]);

function walkFiles(root: string, directory: string, results: string[]): void {
  let entries;
  try {
    entries = readdirSync(directory, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    if (entry.isSymbolicLink()) continue;
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      if (SKIPPED_DIRECTORIES.has(entry.name)) continue;
      walkFiles(root, path, results);
      continue;
    }
    if (entry.isFile()) results.push(relative(root, path));
  }
}

/** Every non-ignored file in the tree, repo-relative and sorted. */
function repositoryFiles(repoRoot: string): string[] {
  const files: string[] = [];
  walkFiles(repoRoot, repoRoot, files);
  return files.sort();
}

interface PackageManifest {
  scripts?: Record<string, string>;
}

/**
 * The `packages:` entries of a `pnpm-workspace.yaml`.
 *
 * Deliberately not a YAML parser. `kd`'s standalone install ships `dist/` with
 * no `node_modules` beside it, so a dependency reached at runtime has to be
 * bundled — and `yaml` resolves to CJS, which esbuild can only inline behind a
 * `require` shim that then throws in the ESM bundle. pnpm's own schema for this
 * key is a sequence of strings, in block or flow form, which is small enough to
 * read directly. Anything else in the file is ignored, as it is by this module.
 */
export function parseWorkspacePackages(source: string): string[] {
  const lines = source.split("\n");
  const start = lines.findIndex((line) => /^packages\s*:/.test(line));
  if (start === -1) return [];

  const patterns: string[] = [];
  const quoted = /["']([^"']+)["']/g;
  const collect = (text: string, bare?: string) => {
    const matches = [...text.matchAll(quoted)].map((match) => match[1]);
    if (matches.length > 0) patterns.push(...matches);
    else if (bare) patterns.push(bare);
  };

  // Flow form on the key's own line: `packages: ["apps/*", "tools/*"]`.
  collect(lines[start].slice(lines[start].indexOf(":") + 1));
  if (patterns.length > 0) return patterns;

  for (const line of lines.slice(start + 1)) {
    if (/^\s*(#.*)?$/.test(line)) continue;
    const item = /^\s+-\s*(.*?)\s*(?:#.*)?$/.exec(line);
    // A line that is not indented, or not a sequence item, ends the block.
    if (!item) break;
    collect(item[1], item[1]);
  }
  return patterns;
}

/**
 * Workspace package directories, from `pnpm-workspace.yaml`, plus the root —
 * whose own `test*` scripts (`test:remote-e2e`, `test:tui-fidelity`,
 * `test:agent-cli-compat`) are lanes nothing else names.
 */
export function workspacePackageDirs(repoRoot: string): string[] {
  const workspacePath = resolve(repoRoot, "pnpm-workspace.yaml");
  const dirs = new Set<string>(["."]);
  if (existsSync(workspacePath)) {
    for (const pattern of parseWorkspacePackages(readFileSync(workspacePath, "utf8"))) {
      if (pattern.endsWith("/*")) {
        const group = pattern.slice(0, -2);
        const groupPath = resolve(repoRoot, group);
        if (!existsSync(groupPath)) continue;
        for (const entry of readdirSync(groupPath, { withFileTypes: true })) {
          if (entry.isDirectory()) dirs.add(`${group}/${entry.name}`);
        }
        continue;
      }
      dirs.add(pattern);
    }
  }
  return [...dirs]
    .filter((dir) => existsSync(resolve(repoRoot, dir, "package.json")))
    .sort();
}

/**
 * Mechanism 1: a `test*` script in a workspace manifest. This is the ordinary
 * way a suite exists, and the way `tests/remote-e2e`'s own `test` script hides
 * every `*.e2e.test.ts` by enumerating the six files it is willing to run.
 */
export function discoverPackageScriptSuites(repoRoot: string): DiscoveredSuite[] {
  const suites: DiscoveredSuite[] = [];
  for (const dir of workspacePackageDirs(repoRoot)) {
    const source = dir === "." ? "package.json" : `${dir}/package.json`;
    const manifest = JSON.parse(
      readFileSync(resolve(repoRoot, source), "utf8"),
    ) as PackageManifest;
    for (const [name, body] of Object.entries(manifest.scripts ?? {})) {
      if (!/^test(:|$)/.test(name)) continue;
      suites.push({
        id: `script:${dir}#${name}`,
        kind: "package-script",
        source,
        detail: body,
      });
    }
  }
  return suites;
}

const KD_TEST_TASK_PATH = "tools/kd/src/tasks/registry.ts";

/**
 * Mechanism 2: a `kd test` subcommand. `./kd test all` runs four lanes; the
 * registry declares far more, and the remainder are reachable only by a human
 * or an agent typing the verb.
 */
export function discoverKdTestSuites(repoRoot: string): DiscoveredSuite[] {
  const path = resolve(repoRoot, KD_TEST_TASK_PATH);
  if (!existsSync(path)) return [];
  const source = readFileSync(path, "utf8");
  const suites: DiscoveredSuite[] = [];
  for (const match of source.matchAll(/id:\s*"test\.([a-z0-9-]+)"/g)) {
    const name = match[1];
    suites.push({
      id: `kd-test:${name}`,
      kind: "kd-test",
      source: KD_TEST_TASK_PATH,
      detail: `./kd test ${name}`,
    });
  }
  return suites;
}

/**
 * Pull the string literals out of an `exclude: [ ... ]` array in a Vitest
 * config, dropping spreads such as `...configDefaults.exclude`. Read off the
 * source text rather than by importing the config: a config that cannot be
 * loaded (a missing plugin, a platform guard) must still be inspectable.
 */
export function parseVitestExcludeGlobs(source: string): string[] {
  const globs: string[] = [];
  for (const start of [...source.matchAll(/\bexclude\s*:\s*\[/g)]) {
    let depth = 1;
    let index = start.index + start[0].length;
    while (index < source.length && depth > 0) {
      const character = source[index];
      if (character === "[") depth += 1;
      else if (character === "]") depth -= 1;
      index += 1;
    }
    const body = source.slice(start.index + start[0].length, index - 1);
    for (const literal of body.matchAll(/["'`]([^"'`]+)["'`]/g)) globs.push(literal[1]);
  }
  return globs;
}

/**
 * Mechanism 3: a Vitest config's `exclude` globs. This is how
 * `apps/desktop/vitest.config.ts` hid `tests/e2e/mock/**` from `pnpm test`
 * for long enough that a third of the suite went red unnoticed. An excluded
 * path is a suite the default collector will never run, so it owes a claim of
 * its own.
 */
export function discoverVitestExcludeSuites(repoRoot: string): DiscoveredSuite[] {
  const suites: DiscoveredSuite[] = [];
  for (const file of repositoryFiles(repoRoot)) {
    if (!/(^|\/)vitest[^/]*\.config\.(ts|mts|js|mjs)$/.test(file)) continue;
    const source = readFileSync(resolve(repoRoot, file), "utf8");
    for (const glob of parseVitestExcludeGlobs(source)) {
      suites.push({
        id: `vitest-exclude:${file}#${glob}`,
        kind: "vitest-exclude",
        source: file,
        detail: glob,
      });
    }
  }
  return suites;
}

/**
 * Mechanism 4: an `*.e2e.test.ts` file on disk. The file's existence is what
 * makes it a member — which is the whole point, and why an unclaimed one is a
 * red test rather than a quiet addition.
 */
export function discoverE2eFileSuites(repoRoot: string): DiscoveredSuite[] {
  return repositoryFiles(repoRoot)
    .filter((file) => file.endsWith(".e2e.test.ts"))
    .map((file) => ({
      id: `e2e-file:${file}`,
      kind: "e2e-file" as const,
      source: file,
      detail: file,
    }));
}

/**
 * Mechanism 5: a `*.test.sh` shell fixture. `docs/dev/testing.md` lists these
 * under "run directly", which is another way of saying nothing runs them.
 */
export function discoverShellTestSuites(repoRoot: string): DiscoveredSuite[] {
  return repositoryFiles(repoRoot)
    .filter((file) => file.endsWith(".test.sh"))
    .map((file) => ({
      id: `shell-test:${file}`,
      kind: "shell-test" as const,
      source: file,
      detail: file,
    }));
}

/** Every suite this repository state contains, by every mechanism, sorted. */
export function discoverTestSuites(repoRoot: string): DiscoveredSuite[] {
  return [
    ...discoverPackageScriptSuites(repoRoot),
    ...discoverKdTestSuites(repoRoot),
    ...discoverVitestExcludeSuites(repoRoot),
    ...discoverE2eFileSuites(repoRoot),
    ...discoverShellTestSuites(repoRoot),
  ].sort((left, right) => left.id.localeCompare(right.id));
}

// ---------------------------------------------------------------------------
// Reconciliation
// ---------------------------------------------------------------------------

export type LaneProblemKind =
  | "unclaimed-suite"
  | "double-claimed-suite"
  | "phantom-claim"
  | "empty-claims"
  | "duplicate-lane-id"
  | "missing-note"
  | "malformed-trigger-path"
  | "stale-trigger-path";

export interface LaneInventoryProblem {
  kind: LaneProblemKind;
  /** The discovered suite id or the lane id the problem is about. */
  subject: string;
  message: string;
}

/**
 * Structural rules the manifest owes on its own, before any discovery: unique
 * ids, no entry that claims nothing, and a written reason wherever the
 * manifest says nobody is accountable. `unowned` is a permitted claim; an
 * `unowned` entry with no reason is the silence this file exists to forbid.
 */
export function validateLaneManifest(manifest: LaneManifest): LaneInventoryProblem[] {
  const problems: LaneInventoryProblem[] = [];
  const seen = new Set<string>();
  for (const lane of manifest.lanes) {
    if (seen.has(lane.id)) {
      problems.push({
        kind: "duplicate-lane-id",
        subject: lane.id,
        message: `lane id "${lane.id}" is declared more than once`,
      });
    }
    seen.add(lane.id);

    if (lane.claims.length === 0) {
      problems.push({
        kind: "empty-claims",
        subject: lane.id,
        message: `lane "${lane.id}" claims no discovered suite, so nothing in the repository holds it to anything`,
      });
    }

    if ((lane.trigger === "unowned" || lane.owner === UNOWNED) && !lane.note) {
      problems.push({
        kind: "missing-note",
        subject: lane.id,
        message: `lane "${lane.id}" is unowned and must state why in "note"`,
      });
    }

    for (const path of lane.triggerPaths) {
      // Prefixes, matched with `startsWith`, exactly as the constant this
      // replaced was matched. A glob here would silently match nothing.
      if (path.startsWith("/") || path.includes("*") || path.trim() !== path) {
        problems.push({
          kind: "malformed-trigger-path",
          subject: lane.id,
          message:
            `lane "${lane.id}" declares trigger path "${path}": trigger paths are`
            + " repo-relative prefixes, not globs or absolute paths",
        });
      }
    }
  }
  return problems;
}

/**
 * A trigger path prefix that matches nothing in the tree is a filter that has
 * stopped selecting — the quiet way a path-gated lane detaches after a
 * directory is renamed. Checked against the repository rather than reviewed.
 */
export function checkTriggerPathsExist(
  repoRoot: string,
  manifest: LaneManifest,
): LaneInventoryProblem[] {
  const files = repositoryFiles(repoRoot);
  const problems: LaneInventoryProblem[] = [];
  for (const lane of manifest.lanes) {
    for (const path of lane.triggerPaths) {
      if (files.some((file) => file.startsWith(path))) continue;
      problems.push({
        kind: "stale-trigger-path",
        subject: lane.id,
        message: `lane "${lane.id}" declares trigger path "${path}", which matches nothing in the repository`,
      });
    }
  }
  return problems;
}

/** Repo-relative changed paths that fall under any of `triggerPaths`. */
export function matchTriggerPaths(
  changedPaths: string[],
  triggerPaths: readonly string[],
): string[] {
  return changedPaths.filter((path) => triggerPaths.some((trigger) => path.startsWith(trigger)));
}

/**
 * The trigger paths one lane declares, read from the manifest.
 *
 * Throws on an unknown lane or an empty list rather than returning `[]`: an
 * empty prefix set makes every `--if-changed` selection answer "not required",
 * which is a gate that has silently detached — the precise failure this
 * manifest exists to prevent, and one that must be loud.
 */
export function laneTriggerPaths(repoRoot: string, laneId: string): string[] {
  const lane = readLaneManifest(repoRoot).lanes.find((candidate) => candidate.id === laneId);
  if (!lane) {
    throw new Error(`${LANE_MANIFEST_PATH} declares no lane "${laneId}"`);
  }
  if (lane.triggerPaths.length === 0) {
    throw new Error(
      `lane "${laneId}" in ${LANE_MANIFEST_PATH} declares no trigger paths, so nothing would ever select it`,
    );
  }
  return [...lane.triggerPaths];
}

/**
 * Cross-check discovered suites against the manifest in both directions. A
 * suite claimed by nothing is the historical failure; a suite claimed twice
 * makes accountability ambiguous, which is the same failure wearing a hat; and
 * a claim on a suite that no longer exists is a manifest that only grows,
 * which is how a record stops describing anything.
 */
export function reconcileLaneInventory(input: {
  discovered: DiscoveredSuite[];
  manifest: LaneManifest;
}): LaneInventoryProblem[] {
  const problems: LaneInventoryProblem[] = [];
  const discoveredIds = new Set(input.discovered.map((suite) => suite.id));
  const claimants = new Map<string, string[]>();

  for (const lane of input.manifest.lanes) {
    for (const claim of lane.claims) {
      claimants.set(claim, [...(claimants.get(claim) ?? []), lane.id]);
      if (!discoveredIds.has(claim)) {
        problems.push({
          kind: "phantom-claim",
          subject: claim,
          message: `lane "${lane.id}" claims "${claim}", which no longer exists in the repository`,
        });
      }
    }
  }

  for (const suite of input.discovered) {
    const owners = claimants.get(suite.id) ?? [];
    if (owners.length === 0) {
      problems.push({
        kind: "unclaimed-suite",
        subject: suite.id,
        message: `${suite.source} declares "${suite.id}" and no lane in ${LANE_MANIFEST_PATH} claims it`,
      });
      continue;
    }
    if (owners.length > 1) {
      problems.push({
        kind: "double-claimed-suite",
        subject: suite.id,
        message: `"${suite.id}" is claimed by ${owners.length} lanes (${owners.join(", ")})`,
      });
    }
  }

  return problems.sort((left, right) => left.subject.localeCompare(right.subject));
}

/** Everything the inventory check asserts, for one repository tree. */
export function auditLaneInventory(repoRoot: string): {
  discovered: DiscoveredSuite[];
  manifest: LaneManifest;
  problems: LaneInventoryProblem[];
} {
  const discovered = discoverTestSuites(repoRoot);
  const manifest = readLaneManifest(repoRoot);
  return {
    discovered,
    manifest,
    problems: [
      ...validateLaneManifest(manifest),
      ...checkTriggerPathsExist(repoRoot, manifest),
      ...reconcileLaneInventory({ discovered, manifest }),
    ],
  };
}
