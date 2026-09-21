import { chmodSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { runCli } from "../src/cli";
import type { CommandRunner } from "../src/runtime/process";
import { nodeCommandRunner } from "../src/runtime/process";

vi.mock("../src/runtime/updater-key", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../src/runtime/updater-key")>();
  return {
    ...actual,
    preflightUpdaterSigningKey: vi.fn(async (input: { env: NodeJS.ProcessEnv }) => {
      const keyPath = input.env.TAURI_PRIVATE_KEY_PATH;
      if (!keyPath) throw new Error("Missing TAURI_PRIVATE_KEY_PATH.");
      const permissions = statSync(keyPath).mode & 0o777;
      if (permissions !== 0o400 && permissions !== 0o600) {
        throw new Error("Tauri updater private key must have owner-only read permissions.");
      }
      return readFileSync(keyPath, "utf8").trim();
    })
  };
});
import {
  bazelTargetForLabel,
  compareVersions,
  createUpdaterBundle,
  cutReleaseBranch,
  deriveMainStagingBaseVersion,
  nextSeriesPatchVersion,
  parsePromotionVersions,
  parseReleaseBranchSeries,
  releaseAssetName,
  releaseSeriesBranch,
  releaseSeriesFromVersion,
  releaseStatus,
  resetStagingLineage,
  shipRelease,
  updaterAssetName,
  updaterBundleTargetForLabel,
  updaterSignatureName,
  type ReleaseArchLabel,
  type ReleaseResetStagingInput,
  type ReleaseShipInput
} from "../src/runtime/release";
import { parseLineageRecutRecord } from "../src/runtime/release-lineage";
import { stagingVersionForWorktree } from "./staging-version-genrule";

interface CommandCall {
  command: string;
  args: string[];
  options?: { cwd?: string; env?: NodeJS.ProcessEnv };
}

function createReleaseRepo(root: string): { repoRoot: string; privateKeyPath: string } {
  const repoRoot = join(root, "repo");
  const tauriDir = join(repoRoot, "apps", "desktop", "src-tauri");
  mkdirSync(tauriDir, { recursive: true });
  writeFileSync(join(repoRoot, "VERSION"), "1.2.3\n");
  writeFileSync(join(repoRoot, "VERSION_RC"), "1\n");
  writeFileSync(join(tauriDir, "tauri.conf.json"), '{\n  "version": "1.2.3"\n}\n');
  writeFileSync(join(tauriDir, "Cargo.toml"), '[package]\nname = "kanna"\nversion = "1.2.3"\n');
  writeFileSync(join(tauriDir, "Cargo.lock"), "# lock\n");
  const privateKeyPath = join(root, "updater-private.key");
  writeFileSync(privateKeyPath, "private key\n", { mode: 0o600 });
  return { repoRoot, privateKeyPath };
}

/**
 * Put a release branch's committed candidate into the fixture worktree.
 *
 * A release branch states the version it ships under and which candidate of it
 * this is, so a fixture shipping from one has to commit both — that pair is the
 * whole input, and it is what the staging bundle combines at build time.
 */
function writeBranchCandidate(repoRoot: string, version: string, candidate: number): void {
  // Mirrors the commit `kd release cut` composes: all four files agree on the
  // series version, and the counter says which candidate of it this is.
  writeFileSync(join(repoRoot, "VERSION"), `${version}\n`);
  writeFileSync(join(repoRoot, "VERSION_RC"), `${candidate}\n`);
  const tauriPath = join(repoRoot, "apps", "desktop", "src-tauri", "tauri.conf.json");
  const cargoPath = join(repoRoot, "apps", "desktop", "src-tauri", "Cargo.toml");
  writeFileSync(tauriPath, readFileSync(tauriPath, "utf8").replace(/"version": "[^"]*"/, `"version": "${version}"`));
  writeFileSync(cargoPath, readFileSync(cargoPath, "utf8").replace(/^version = "[^"]*"/m, `version = "${version}"`));
}

function releaseEnv(privateKeyPath: string): NodeJS.ProcessEnv {
  return {
    KANNA_UPDATER_PUBKEY: "pubkey",
    TAURI_PRIVATE_KEY_PATH: privateKeyPath,
    PATH: process.env.PATH
  };
}

function readVersionFiles(repoRoot: string): string[] {
  return [
    readFileSync(join(repoRoot, "VERSION"), "utf8"),
    readFileSync(join(repoRoot, "apps", "desktop", "src-tauri", "tauri.conf.json"), "utf8"),
    readFileSync(join(repoRoot, "apps", "desktop", "src-tauri", "Cargo.toml"), "utf8")
  ];
}

function writeReleaseBuildOutputs(repoRoot: string, labels: ReleaseArchLabel[]): Map<string, string> {
  const outputs = new Map<string, string>();
  mkdirSync(join(repoRoot, "bazel-out", "release"), { recursive: true });

  for (const label of labels) {
    const dmgRel = `bazel-out/release/Kanna-${label}.dmg`;
    const bundleRel = `bazel-out/release/Kanna-${label}.app.tar.gz`;
    writeFileSync(join(repoRoot, dmgRel), `${label} dmg\n`);
    writeFileSync(join(repoRoot, bundleRel), `${label} updater bundle\n`);
    outputs.set(bazelTargetForLabel(label, false), dmgRel);
    outputs.set(updaterBundleTargetForLabel(label), bundleRel);
  }

  return outputs;
}

function writeStagingReleaseBuildOutputs(repoRoot: string, labels: ReleaseArchLabel[]): Map<string, string> {
  const outputs = new Map<string, string>();
  mkdirSync(join(repoRoot, "bazel-out", "release", "staging"), { recursive: true });

  for (const label of labels) {
    const dmgRel = `bazel-out/release/staging/Kanna-Staging-${label}.dmg`;
    const bundleRel = `bazel-out/release/staging/Kanna-Staging-${label}.app.tar.gz`;
    writeFileSync(join(repoRoot, dmgRel), `${label} staging dmg\n`);
    writeFileSync(join(repoRoot, bundleRel), `${label} staging updater bundle\n`);
    outputs.set(bazelTargetForLabel(label, true, "staging"), dmgRel);
    outputs.set(bazelTargetForLabel(label, false, "staging"), dmgRel);
    outputs.set(updaterBundleTargetForLabel(label, "staging"), bundleRel);
  }

  return outputs;
}

/**
 * `readStagingChannel` asks for the pointer release's asset list before it will
 * trust (or distrust) the channel, so fixtures have to answer that query
 * explicitly. `null` models a channel that does not exist at all — a genuine
 * 404, which is the only shape that reads as "uninitialized".
 */
function stagingChannelAssetsResponse(assets: string[] | null): { exitCode: number; stdout: string; stderr: string } {
  if (assets === null) return { exitCode: 1, stdout: "", stderr: "release not found\n" };
  return { exitCode: 0, stdout: JSON.stringify({ assets: assets.map((name) => ({ name })) }), stderr: "" };
}

function isStagingChannelAssetsQuery(command: string, args: string[]): boolean {
  return (
    command === "gh" &&
    args[0] === "release" &&
    args[1] === "view" &&
    args[2] === "desktop-staging" &&
    args.includes("--json") &&
    args.includes("assets")
  );
}

/** The commit `cutReleaseBranch` composes to set a new series' version. */
const CUT_VERSION_COMMIT = "c077c077c077c077c077c077c077c077c077c077";

/**
 * Answers the git plumbing `cutReleaseBranch` uses to build that commit against
 * a temporary index. A mocked runner that let these fall through would report
 * success while pushing an empty ref, so they are answered explicitly here and
 * proven for real in release-cut-version.test.ts.
 */
function cutVersionCommitPlumbing(
  command: string,
  args: string[]
): { exitCode: number; stdout: string; stderr: string } | null {
  if (command !== "git") return null;
  const subcommand = args[0] ?? "";
  if (subcommand === "read-tree" || subcommand === "update-index") {
    return { exitCode: 0, stdout: "", stderr: "" };
  }
  if (subcommand === "hash-object") {
    return { exitCode: 0, stdout: "b10bb10bb10bb10bb10bb10bb10bb10bb10bb10b\n", stderr: "" };
  }
  if (subcommand === "write-tree") {
    return { exitCode: 0, stdout: "47ee47ee47ee47ee47ee47ee47ee47ee47ee47ee\n", stderr: "" };
  }
  if (subcommand === "commit-tree") {
    return { exitCode: 0, stdout: `${CUT_VERSION_COMMIT}\n`, stderr: "" };
  }
  // The version-bearing manifests are absent from these fixtures, so the
  // composer skips them; the real-git test covers the case where they exist.
  if (subcommand === "cat-file" && args[1] === "-e") {
    return { exitCode: 1, stdout: "", stderr: "path does not exist" };
  }
  // How a recut decides whether a branch-only commit is kd's own series version
  // commit. These fixtures' commits are ordinary work, so they answer as such
  // and the hygiene gate keeps refusing them; release-cut-version.test.ts drives
  // the real predicate against real commits.
  if (subcommand === "log" && args.includes("--format=%s")) {
    return { exitCode: 0, stdout: "work that exists only on the branch\n", stderr: "" };
  }
  if (subcommand === "show" && args.includes("--name-only")) {
    return { exitCode: 0, stdout: "crates/kanna-server/src/lib.rs\n", stderr: "" };
  }
  return null;
}

function isProductionReleaseListQuery(command: string, args: string[]): boolean {
  return (
    command === "gh" &&
    args[0] === "release" &&
    args[1] === "list" &&
    args.includes("--exclude-pre-releases")
  );
}

describe("release updater bundling", () => {
  // A regular full kd release ship -> updater install E2E would need signed release
  // artifacts, both macOS architectures, GitHub release metadata/assets, and a
  // WebDriver-driven installed app. The existing opt-in full-bundle updater E2E
  // builds its own temporary debug bundle instead of executing this release helper.
  // A feasible regular E2E would need a hermetic release backend with small signed
  // fixtures and a local updater manifest server. This test keeps the regression
  // guard at the production bundle helper boundary: command-runner env propagation,
  // copying the Bazel-created updater archive, and signer output placement.
  it("copies the Bazel updater bundle and renames the generated signature", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const repoRoot = join(root, "repo");
      const bundleSource = join(repoRoot, "bazel-out", "release", "Kanna-arm64.app.tar.gz");
      const bundlePath = join(repoRoot, ".build", "release", "Kanna_1.2.4_arm64.app.tar.gz");
      const signaturePath = join(repoRoot, ".build", "release", "custom-updater.sig");
      const privateKeyPath = join(root, "updater-private.key");

      mkdirSync(join(repoRoot, "bazel-out", "release"), { recursive: true });
      mkdirSync(join(repoRoot, ".build", "release"), { recursive: true });
      writeFileSync(bundleSource, "bazel updater archive\n");
      writeFileSync(privateKeyPath, "private key\n", { mode: 0o600 });

      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          if (command === "pnpm") {
            const signedBundlePath = args.at(-1);
            expect(signedBundlePath).toBe(bundlePath);
            writeFileSync(`${signedBundlePath}.sig`, "signed bundle\n");
            return { exitCode: 0, stdout: "", stderr: "" };
          }

          return { exitCode: 1, stdout: "", stderr: `unexpected command ${command}` };
        }
      };
      const input: ReleaseShipInput = {
        repoRoot,
        bump: "patch",
        archLabels: ["arm64"],
        release: false,
        dryRun: true,
        env: releaseEnv(privateKeyPath),
        runner
      };

      await createUpdaterBundle(input, bundleSource, bundlePath, signaturePath);

      expect(readFileSync(bundlePath, "utf8")).toBe("bazel updater archive\n");
      expect(readFileSync(signaturePath, "utf8")).toBe("signed bundle\n");
      expect(calls.some((call) => call.command === "tar")).toBe(false);
      const signerCall = calls.find((call) => call.command === "pnpm");
      expect(signerCall?.args).toEqual([
        "--dir",
        join(repoRoot, "apps", "desktop"),
        "exec",
        "tauri",
        "signer",
        "sign",
        bundlePath
      ]);
      // The key and its password travel through the signer's environment, never
      // argv, so neither is visible to other processes via ps.
      expect(signerCall?.args).not.toContain("private key");
      expect(signerCall?.args).not.toContain("password");
      expect(signerCall?.options?.env?.TAURI_SIGNING_PRIVATE_KEY).toBe("private key");
      expect(signerCall?.options?.env?.TAURI_SIGNING_PRIVATE_KEY_PASSWORD).toBe("");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("ignores an ambient key password and always passes an explicit empty one", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const repoRoot = join(root, "repo");
      const bundleSource = join(repoRoot, "bazel-out", "release", "Kanna.app.tar.gz");
      const bundlePath = join(repoRoot, ".build", "release", "Kanna.app.tar.gz");
      const signaturePath = `${bundlePath}.sig`;
      const privateKeyPath = join(root, "updater-private.key");

      mkdirSync(join(repoRoot, "bazel-out", "release"), { recursive: true });
      mkdirSync(join(repoRoot, ".build", "release"), { recursive: true });
      writeFileSync(bundleSource, "bazel updater archive\n");
      writeFileSync(privateKeyPath, "private key\n", { mode: 0o600 });

      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          if (command === "pnpm") {
            writeFileSync(`${args.at(-1)}.sig`, "signed bundle\n");
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${command}` };
        }
      };

      await createUpdaterBundle(
        {
          repoRoot,
          bump: "patch",
          archLabels: ["arm64"],
          release: false,
          dryRun: true,
          // Updater keys used by kd are unencrypted. An ambient
          // TAURI_PRIVATE_KEY_PASSWORD must not reach the signer, and an
          // absent one must not leave it prompting on a TTY -- that prompt used
          // to kill non-interactive ships after the whole build had completed.
          env: { ...releaseEnv(privateKeyPath), TAURI_PRIVATE_KEY_PASSWORD: "stale ambient value" },
          runner
        },
        bundleSource,
        bundlePath,
        signaturePath
      );

      const signerEnv = calls.find((call) => call.command === "pnpm")?.options?.env;
      expect(signerEnv?.TAURI_SIGNING_PRIVATE_KEY).toBe("private key");
      expect(signerEnv?.TAURI_SIGNING_PRIVATE_KEY_PASSWORD).toBe("");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("does not expose signer output when signing fails", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const repoRoot = join(root, "repo");
      const bundleSource = join(repoRoot, "bundle.app.tar.gz");
      const bundlePath = join(repoRoot, "signed.app.tar.gz");
      const privateKeyPath = join(root, "updater-private.key");
      mkdirSync(repoRoot, { recursive: true });
      writeFileSync(bundleSource, "bundle\n");
      writeFileSync(privateKeyPath, "secret updater key\n", { mode: 0o600 });
      const runner: CommandRunner = {
        async run() {
          return {
            exitCode: 1,
            stdout: "secret updater key",
            stderr: "TAURI_SIGNING_PRIVATE_KEY=secret updater key"
          };
        }
      };

      let message = "";
      try {
        await createUpdaterBundle(
          {
            repoRoot,
            bump: "patch",
            archLabels: ["arm64"],
            release: false,
            dryRun: true,
            env: releaseEnv(privateKeyPath),
            runner
          },
          bundleSource,
          bundlePath,
          `${bundlePath}.sig`
        );
      } catch (error) {
        message = error instanceof Error ? error.message : String(error);
      }
      expect(message).toMatch(/Tauri updater signing failed/);
      expect(message).not.toContain("secret updater key");
      expect(message).not.toContain("TAURI_SIGNING_PRIVATE_KEY");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});

describe("release shipping", () => {
  it.each([
    ["dry-run", { dryRun: true, release: false, environment: "production" as const }],
    ["staging", { dryRun: false, release: false, environment: "staging" as const }],
    ["production", { dryRun: false, release: true, environment: "production" as const }],
    ["promotion", {
      dryRun: false,
      release: true,
      environment: "production" as const,
      promoteFrom: "1.2.4-staging.3"
    }]
  ])("preflights the selected key file before mutations for %s", async (_label, mode) => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      chmodSync(privateKeyPath, 0o644);
      const originalFiles = readVersionFiles(repoRoot);
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          if (command === "git" && args.join(" ") === "status --porcelain") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          return { exitCode: 1, stdout: "", stderr: "must not run after failed preflight" };
        }
      };

      await expect(shipRelease({
        repoRoot,
        bump: "patch",
        archLabels: mode.release ? ["arm64", "x86_64"] : ["arm64"],
        ...mode,
        env: releaseEnv(privateKeyPath),
        runner
      })).rejects.toThrow(/owner-only read permissions/);
      expect(calls).toEqual([]);
      expect(readVersionFiles(repoRoot)).toEqual(originalFiles);
      expect(existsSync(join(repoRoot, ".build", "release"))).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("uses staging artifact names and Bazel targets when shipping staging", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    // What Bazel would stamp from the files kd wrote, captured during the build.
    let stampedStagingVersion: string | null = null;
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64"]);
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          if (isStagingChannelAssetsQuery(command, args)) return stagingChannelAssetsResponse(null);
          if (command === "git" && args.join(" ") === "status --porcelain") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "git" && args.join(" ") === "rev-parse --abbrev-ref HEAD") {
            return { exitCode: 0, stdout: "main\n", stderr: "" };
          }
          if (command === "git" && args.join(" ") === "rev-parse HEAD") {
            return { exitCode: 0, stdout: "1234567890abcdef\n", stderr: "" };
          }
          if (command === "git" && args.join(" ") === "ls-remote --tags origin v1.3.0-staging.*") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "bazel" && args[0] === "build") {
            expect(args).toContain("//:kanna_signed_dmg_staging_arm64");
            expect(args).toContain("//:kanna_updater_bundle_staging_arm64");
            expect(args).not.toContain("//:kanna_signed_dmg_release_arm64");
            // kd writes the base version and the candidate counter separately;
            // the staging bundle recombines them. Writing the suffixed string
            // into VERSION stamped it twice (0.5.0-staging.3-staging.1), which
            // is semver-greater than the feed, so no installed staging client
            // could ever update. Run the real genrule over what was written.
            expect(readVersionFiles(repoRoot)).toEqual([
              "1.3.0\n",
              '{\n  "version": "1.3.0"\n}\n',
              '[package]\nname = "kanna"\nversion = "1.3.0"\n'
            ]);
            stampedStagingVersion = stagingVersionForWorktree(repoRoot);
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "git" && args.join(" ") === "remote get-url origin") {
            return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
          }
          if (isProductionReleaseListQuery(command, args)) {
            return { exitCode: 0, stdout: "[]", stderr: "" };
          }
          if (command === "bazel" && args[0] === "cquery") {
            return { exitCode: 0, stdout: `${outputs.get(args[3]) ?? ""}\n`, stderr: "" };
          }
          if (command === "sh" && args[0] === "-c") {
            expect(args[1]).toContain("hdiutil attach");
            expect(args[1]).toContain("sips -g pixelWidth -g pixelHeight");
            expect(args.at(-1)).toBe(join(repoRoot, ".build", "release", "staging", "Kanna_Staging_1.3.0-staging.1_arm64.dmg"));
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "pnpm") {
            const signedBundlePath = args.at(-1);
            expect(typeof signedBundlePath).toBe("string");
            writeFileSync(`${signedBundlePath}.sig`, "staging signature\n");
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${command} ${args.join(" ")}` };
        }
      };

      const result = await shipRelease({
        repoRoot,
        bump: "patch",
        archLabels: ["arm64"],
        release: false,
        dryRun: true,
        environment: "staging",
        env: releaseEnv(privateKeyPath),
        runner
      });

      expect(releaseAssetName("1.2.4-staging.1", "arm64", "staging")).toBe("Kanna_Staging_1.2.4-staging.1_arm64.dmg");
      expect(updaterAssetName("1.2.4-staging.1", "arm64", "staging")).toBe("Kanna_Staging_1.2.4-staging.1_arm64.app.tar.gz");
      expect(updaterSignatureName("1.2.4-staging.1", "arm64", "staging")).toBe("Kanna_Staging_1.2.4-staging.1_arm64.app.tar.gz.sig");
      expect(result.dmgPaths).toEqual([join(repoRoot, ".build", "release", "staging", "Kanna_Staging_1.3.0-staging.1_arm64.dmg")]);
      expect(result.updaterPaths).toEqual([
        join(repoRoot, ".build", "release", "staging", "Kanna_Staging_1.3.0-staging.1_arm64.app.tar.gz"),
        join(repoRoot, ".build", "release", "staging", "Kanna_Staging_1.3.0-staging.1_arm64.app.tar.gz.sig")
      ]);
      expect(result.latestJson).toBe(join(repoRoot, ".build", "release", "staging", "latest-staging.json"));
      const validationCall = calls.find((call) => call.command === "sh" && call.args[0] === "-c");
      const signerCall = calls.find((call) => call.command === "pnpm");
      expect(validationCall).toBeDefined();
      expect(signerCall).toBeDefined();
      expect(calls.indexOf(validationCall!)).toBeLessThan(calls.indexOf(signerCall!));
      expect(readVersionFiles(repoRoot)).toEqual([
        "1.2.3\n",
        '{\n  "version": "1.2.3"\n}\n',
        '[package]\nname = "kanna"\nversion = "1.2.3"\n'
      ]);
      // The bundle's version and the published version are the same string.
      // They are produced by different halves -- kd writes two files, Bazel
      // recombines them -- so nothing but this comparison holds them together.
      expect(stampedStagingVersion).toBe(result.version);
      // And the ship put the checkout back exactly as it found it.
      expect(readFileSync(join(repoRoot, "VERSION_RC"), "utf8")).toBe("1\n");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("takes a release-branch candidate from the committed version and counter, writing nothing", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      // The branch states the version it ships under and which candidate this
      // is. Nothing is counted at ship time, so rebuilding this commit yields
      // this same candidate -- the RC number is a property of the commit.
      writeBranchCandidate(repoRoot, "1.3.1", 3);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64"]);
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          if (isStagingChannelAssetsQuery(command, args)) return stagingChannelAssetsResponse(null);
          const key = `${command} ${args.join(" ")}`;
          if (key === "git status --porcelain") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (key === "git rev-parse --abbrev-ref HEAD") {
            return { exitCode: 0, stdout: "release/1.3\n", stderr: "" };
          }
          if (key === "git rev-parse HEAD") {
            return { exitCode: 0, stdout: "branchsha\n", stderr: "" };
          }
          if (key === "git ls-remote origin refs/heads/release/1.3") {
            return { exitCode: 0, stdout: "branchsha\trefs/heads/release/1.3\n", stderr: "" };
          }
          if (key === "git fetch origin release/1.3") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (key === "git ls-remote --tags origin v1.3.*") {
            return { exitCode: 0, stdout: "sha1\trefs/tags/v1.3.0\nsha2\trefs/tags/v1.3.0-staging.9\n", stderr: "" };
          }
          if (key === "git remote get-url origin") {
            return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
          }
          if (command === "bazel" && args[0] === "build") {
            // VERSION on disk is the *release* version during the build. The
            // staging bundle target combines it with VERSION_RC itself, so kd
            // never writes the candidate string into the worktree.
            expect(readVersionFiles(repoRoot)[0]).toBe("1.3.1\n");
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "bazel" && args[0] === "cquery") {
            return { exitCode: 0, stdout: `${outputs.get(args[3] ?? "") ?? ""}\n`, stderr: "" };
          }
          if (command === "sh" && args[0] === "-c") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "pnpm") {
            const signedBundlePath = args.at(-1);
            expect(typeof signedBundlePath).toBe("string");
            writeFileSync(`${signedBundlePath}.sig`, "staging signature\n", "utf8");
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${key}` };
        }
      };

      const result = await shipRelease({
        repoRoot,
        bump: "patch",
        archLabels: ["arm64"],
        release: false,
        dryRun: true,
        environment: "staging",
        env: releaseEnv(privateKeyPath),
        runner
      });

      expect(result.version).toBe("1.3.1-staging.3");
      // No bump, no build, no toss away: the committed files are untouched.
      expect(readVersionFiles(repoRoot)[0]).toBe("1.3.1\n");
      // The candidate number comes from the branch, never from counting tags.
      expect(calls.some((call) => call.args.join(" ").includes("v1.3.1-staging.*"))).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("names VERSION_RC and the value to commit when a second candidate would repeat the first", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-second-candidate-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      // The branch shipped 1.3.1-staging.1. The next backport lands, the
      // operator ships again, and VERSION_RC still says 1 — nothing advances
      // it. Before this the refusal offered a bare ship or --minor/--major,
      // neither of which can move a branch candidate.
      writeBranchCandidate(repoRoot, "1.3.1", 1);
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          const key = `${command} ${args.join(" ")}`;
          if (isStagingChannelAssetsQuery(command, args)) return stagingChannelAssetsResponse(["latest-staging.json"]);
          if (command === "gh" && args[0] === "release" && args[1] === "download") {
            const dirIndex = args.indexOf("--dir");
            writeFileSync(join(args[dirIndex + 1] ?? "", "latest-staging.json"), '{"version":"1.3.1-staging.1"}\n');
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args.join(" ").startsWith("release view v1.3.1-staging.1")) {
            return {
              exitCode: 0,
              stdout: JSON.stringify({
                tagName: "v1.3.1-staging.1",
                targetCommitish: "branchsha0000000000000000000000000000000",
                body: "Staging updater manifest for v1.3.1-staging.1\n\nSource-Branch: release/1.3",
                publishedAt: "2026-09-20T00:00:00Z",
                isPrerelease: true
              }),
              stderr: ""
            };
          }
          if (key === "git status --porcelain") return { exitCode: 0, stdout: "", stderr: "" };
          if (key === "git rev-parse --abbrev-ref HEAD") return { exitCode: 0, stdout: "release/1.3\n", stderr: "" };
          if (key === "git rev-parse HEAD") return { exitCode: 0, stdout: "branchsha\n", stderr: "" };
          if (key === "git ls-remote origin refs/heads/release/1.3") {
            return { exitCode: 0, stdout: "branchsha\trefs/heads/release/1.3\n", stderr: "" };
          }
          if (key === "git fetch origin release/1.3") return { exitCode: 0, stdout: "", stderr: "" };
          if (key === "git ls-remote --tags origin v1.3.*") {
            return { exitCode: 0, stdout: "sha1\trefs/tags/v1.3.0\n", stderr: "" };
          }
          if (key === "git remote get-url origin") {
            return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
          }
          if (command === "git" && (args[0] === "fetch" || args[0] === "merge-base")) {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args[0] === "release" && args[1] === "list") {
            return { exitCode: 0, stdout: JSON.stringify([{ tagName: "v1.3.1-staging.1", createdAt: "2026-09-20T00:00:00Z" }]), stderr: "" };
          }
          if (command === "gh" && args[0] === "release" && args[1] === "view") {
            return { exitCode: 0, stdout: JSON.stringify({ body: "" }), stderr: "" };
          }
          if (command === "git" && args[0] === "ls-remote") return { exitCode: 0, stdout: "", stderr: "" };
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${key}` };
        }
      };

      const shipping = shipRelease({
        repoRoot,
        bump: "patch",
        archLabels: ["arm64"],
        release: false,
        dryRun: true,
        environment: "staging",
        env: releaseEnv(privateKeyPath),
        runner
      });

      await expect(shipping).rejects.toThrow(/VERSION_RC/);
      await expect(shipping).rejects.toThrow(/Commit VERSION_RC 2 onto release\/1\.3/);
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses a release-branch candidate whose committed version already shipped", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-stale-version-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      // v1.3.0 is released and the branch still says 1.3.0: the candidate line
      // was never started. Refused here, with the commit to make, rather than
      // at promotion where the forward-version gate would reject it much later.
      writeBranchCandidate(repoRoot, "1.3.0", 1);
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          if (isStagingChannelAssetsQuery(command, args)) return stagingChannelAssetsResponse(null);
          const key = `${command} ${args.join(" ")}`;
          if (key === "git status --porcelain") return { exitCode: 0, stdout: "", stderr: "" };
          if (key === "git rev-parse --abbrev-ref HEAD") return { exitCode: 0, stdout: "release/1.3\n", stderr: "" };
          if (key === "git rev-parse HEAD") return { exitCode: 0, stdout: "branchsha\n", stderr: "" };
          if (key === "git ls-remote origin refs/heads/release/1.3") {
            return { exitCode: 0, stdout: "branchsha\trefs/heads/release/1.3\n", stderr: "" };
          }
          if (key === "git fetch origin release/1.3") return { exitCode: 0, stdout: "", stderr: "" };
          if (key === "git ls-remote --tags origin v1.3.*") {
            return { exitCode: 0, stdout: "sha1\trefs/tags/v1.3.0\n", stderr: "" };
          }
          if (key === "git remote get-url origin") {
            return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
          }
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${key}` };
        }
      };

      await expect(
        shipRelease({
          repoRoot,
          bump: "patch",
          archLabels: ["arm64"],
          release: false,
          dryRun: true,
          environment: "staging",
          env: releaseEnv(privateKeyPath),
          runner
        })
      ).rejects.toThrow(/commit VERSION 1\.3\.1 \(and VERSION_RC 1\) onto release\/1\.3/);
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("versions and records a release-branch RC shipped from a task worktree via --branch", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      writeBranchCandidate(repoRoot, "1.3.0", 1);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64"]);
      const branchSha = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          if (isStagingChannelAssetsQuery(command, args)) return stagingChannelAssetsResponse(null);
          const key = `${command} ${args.join(" ")}`;
          if (key === "git status --porcelain") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (key === "git rev-parse HEAD") {
            return { exitCode: 0, stdout: `${branchSha}\n`, stderr: "" };
          }
          if (key === "git ls-remote origin refs/heads/release/1.3") {
            return { exitCode: 0, stdout: `${branchSha}\trefs/heads/release/1.3\n`, stderr: "" };
          }
          if (key === "git fetch origin release/1.3") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (key === "git ls-remote --tags origin v1.3.*") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (key === "git ls-remote --tags origin v1.3.0-staging.*") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (key === "git remote get-url origin") {
            return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
          }
          if (command === "bazel" && args[0] === "build") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "bazel" && args[0] === "cquery") {
            return { exitCode: 0, stdout: `${outputs.get(args[3] ?? "") ?? ""}\n`, stderr: "" };
          }
          if (command === "sh" && args[0] === "-c") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "pnpm") {
            const signedBundlePath = args.at(-1);
            expect(typeof signedBundlePath).toBe("string");
            writeFileSync(`${signedBundlePath}.sig`, "staging signature\n", "utf8");
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${key}` };
        }
      };

      const result = await shipRelease({
        repoRoot,
        bump: "patch",
        archLabels: ["arm64"],
        release: false,
        dryRun: true,
        environment: "staging",
        sourceBranch: "release/1.3",
        env: releaseEnv(privateKeyPath),
        runner
      });

      // The Kanna task worktree branch (task-*) is never consulted: no
      // rev-parse --abbrev-ref call, series versioning from release/1.3, and
      // the manifest notes record the RC's source branch.
      expect(result.version).toBe("1.3.0-staging.1");
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === "rev-parse --abbrev-ref HEAD")).toBe(false);
      const manifest = JSON.parse(readFileSync(result.latestJson, "utf8")) as { notes?: string };
      expect(manifest.notes).toContain("Source-Branch: release/1.3");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses a --branch RC when HEAD is not exactly the release branch tip", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const branchSha = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          if (isStagingChannelAssetsQuery(command, args)) return stagingChannelAssetsResponse(null);
          const key = `${command} ${args.join(" ")}`;
          if (key === "git status --porcelain") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (key === "git ls-remote origin refs/heads/release/1.3") {
            return { exitCode: 0, stdout: `${branchSha}\trefs/heads/release/1.3\n`, stderr: "" };
          }
          if (key === "git fetch origin release/1.3") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          // A task worktree that merged the branch in still descends from the
          // tip; only an exact match may claim the branch as its RC provenance.
          if (key === "git rev-parse HEAD") {
            return { exitCode: 0, stdout: "cccccccccccccccccccccccccccccccccccccccc\n", stderr: "" };
          }
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${key}` };
        }
      };

      await expect(shipRelease({
        repoRoot,
        bump: "patch",
        archLabels: ["arm64"],
        release: false,
        dryRun: true,
        environment: "staging",
        sourceBranch: "release/1.3",
        env: releaseEnv(privateKeyPath),
        runner
      })).rejects.toThrow(/release\/1\.3 tip .* is not HEAD/);
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
      expect(readVersionFiles(repoRoot)[0]).toBe("1.2.3\n");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("publishes staging as an immutable prerelease, then repoints desktop-staging", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const originalFiles = readVersionFiles(repoRoot);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const ACTIVE_VERSION = "1.2.4-staging.4";
      const ACTIVE_COMMIT = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          // The channel already serves a candidate; this publish is the normal
          // forward move from it, so the lineage gate has something to compare.
          if (command === "gh" && args[0] === "release" && args[1] === "download" && args[2] === "desktop-staging") {
            const dirIndex = args.indexOf("--dir");
            writeFileSync(join(args[dirIndex + 1] ?? "", "latest-staging.json"), `{"version":"${ACTIVE_VERSION}"}\n`);
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args[0] === "release" && args[1] === "view" && args[2] === `v${ACTIVE_VERSION}`) {
            return {
              exitCode: 0,
              stdout: JSON.stringify({
                targetCommitish: ACTIVE_COMMIT,
                publishedAt: "2026-07-06T00:00:00Z",
                body: "Staging updater manifest\n\nSource-Branch: main"
              }),
              stderr: ""
            };
          }
          if (command === "git" && args.join(" ") === "fetch --tags origin") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "git" && args[0] === "merge-base") {
            return {
              exitCode: args[2] === ACTIVE_COMMIT && args[3] === "1234567890abcdef" ? 0 : 1,
              stdout: "",
              stderr: ""
            };
          }
          if (command === "git" && args.join(" ") === "status --porcelain") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "bazel" && args[0] === "build") {
            expect(args).toContain("//:kanna_notarized_dmg_staging_arm64");
            expect(args).toContain("//:kanna_notarized_dmg_staging_x86_64");
            expect(args).toContain("//:kanna_updater_bundle_staging_arm64");
            expect(args).toContain("//:kanna_updater_bundle_staging_x86_64");
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "git" && args.join(" ") === "remote get-url origin") {
            return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
          }
          if (command === "git" && args.join(" ") === "rev-parse --abbrev-ref HEAD") {
            return { exitCode: 0, stdout: "main\n", stderr: "" };
          }
          if (command === "git" && args.join(" ") === "ls-remote --tags origin v1.2.4-staging.*") {
            return {
              exitCode: 0,
              stdout: [
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\trefs/tags/v1.2.4-staging.1",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\trefs/tags/v1.2.4-staging.4",
                "cccccccccccccccccccccccccccccccccccccccc\trefs/tags/v1.2.4-staging.4^{}"
              ].join("\n"),
              stderr: ""
            };
          }
          if (command === "git" && args.join(" ") === "rev-parse HEAD") {
            return { exitCode: 0, stdout: "1234567890abcdef\n", stderr: "" };
          }
          if (command === "bazel" && args[0] === "cquery") {
            return { exitCode: 0, stdout: `${outputs.get(args[3]) ?? ""}\n`, stderr: "" };
          }
          if (command === "sh" && args[0] === "-c") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "pnpm") {
            const signedBundlePath = args.at(-1);
            expect(typeof signedBundlePath).toBe("string");
            writeFileSync(`${signedBundlePath}.sig`, "staging signature\n");
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args.join(" ") === "release view desktop-staging --repo jemdiggity/kanna") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args[0] === "release" && args[1] === "create") {
            expect(args).toEqual([
              "release",
              "create",
              "v1.2.4-staging.5",
              "--repo",
              "jemdiggity/kanna",
              "--title",
              "Kanna Staging v1.2.4-staging.5",
              "--notes",
              "Staging updater manifest for v1.2.4-staging.5\n\nSource-Branch: main",
              "--target",
              "1234567890abcdef",
              "--prerelease",
              join(repoRoot, ".build", "release", "staging", "Kanna_Staging_1.2.4-staging.5_arm64.dmg"),
              join(repoRoot, ".build", "release", "staging", "Kanna_Staging_1.2.4-staging.5_x86_64.dmg"),
              join(repoRoot, ".build", "release", "staging", "Kanna_Staging_1.2.4-staging.5_arm64.app.tar.gz"),
              join(repoRoot, ".build", "release", "staging", "Kanna_Staging_1.2.4-staging.5_arm64.app.tar.gz.sig"),
              join(repoRoot, ".build", "release", "staging", "Kanna_Staging_1.2.4-staging.5_x86_64.app.tar.gz"),
              join(repoRoot, ".build", "release", "staging", "Kanna_Staging_1.2.4-staging.5_x86_64.app.tar.gz.sig"),
              join(repoRoot, ".build", "release", "staging", "latest-staging.json")
            ]);
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args[0] === "release" && args[1] === "upload" && args[2] === "desktop-staging") {
            expect(args[2]).toBe("desktop-staging");
            expect(args).toEqual([
              "release",
              "upload",
              "desktop-staging",
              join(repoRoot, ".build", "release", "staging", "latest-staging.json"),
              "--repo",
              "jemdiggity/kanna",
              "--clobber"
            ]);
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args[0] === "release" && args[1] === "delete-asset") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (isProductionReleaseListQuery(command, args)) {
            return { exitCode: 0, stdout: "[]", stderr: "" };
          }
          if (command === "gh" && args[0] === "release" && args[1] === "list") {
            return {
              exitCode: 0,
              stdout: [
                "Kanna Staging v1.2.4-staging.5\tLatest\tv1.2.4-staging.5\t2026-07-06T00:00:00Z",
                "Kanna Staging v1.2.4-staging.4\t\tv1.2.4-staging.4\t2026-07-05T00:00:00Z"
              ].join("\n"),
              stderr: ""
            };
          }
          if (command === "gh" && args[0] === "release" && args[1] === "view" && args[2] === "desktop-staging") {
            if (args.includes("--json")) {
              return {
                exitCode: 0,
                stdout: JSON.stringify({
                  assets: [
                    { name: "latest-staging.json" },
                    { name: "Kanna_Staging_1.2.4_arm64.dmg" }
                  ]
                }),
                stderr: ""
              };
            }
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${command} ${args.join(" ")}` };
        }
      };

      await shipRelease({
        repoRoot,
        bump: "patch",
        archLabels: ["arm64", "x86_64"],
        release: true,
        dryRun: false,
        environment: "staging",
        env: releaseEnv(privateKeyPath),
        runner
      });

      expect(readVersionFiles(repoRoot)).toEqual(originalFiles);
      const releaseCreateCall = calls.find((call) => call.command === "gh" && call.args[0] === "release" && call.args[1] === "create");
      const uploadCall = calls.find((call) => call.command === "gh" && call.args[0] === "release" && call.args[1] === "upload");
      expect(uploadCall?.args[2]).toBe("desktop-staging");
      expect(releaseCreateCall).toBeDefined();
      expect(uploadCall).toBeDefined();
      expect(calls.indexOf(releaseCreateCall!)).toBeLessThan(calls.indexOf(uploadCall!));
      expect(calls.some((call) => call.command === "git" && ["add", "commit", "tag", "push"].includes(call.args[0] ?? ""))).toBe(false);
      expect(calls.some((call) => call.command === "gh" && call.args[0] === "api")).toBe(false);
      expect(readFileSync(join(repoRoot, ".build", "release", "staging", "latest-staging.json"), "utf8")).toContain(
        "releases/download/v1.2.4-staging.5/Kanna_Staging_1.2.4-staging.5_arm64.app.tar.gz"
      );
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("retains every immutable staging prerelease as the channel advances past five", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const ACTIVE_VERSION = "1.2.4-staging.6";
      const ACTIVE_COMMIT = "ffffffffffffffffffffffffffffffffffffffff";
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          // The channel already serves a candidate; this publish is the normal
          // forward move from it, so the lineage gate has something to compare.
          if (command === "gh" && args[0] === "release" && args[1] === "download" && args[2] === "desktop-staging") {
            const dirIndex = args.indexOf("--dir");
            writeFileSync(join(args[dirIndex + 1] ?? "", "latest-staging.json"), `{"version":"${ACTIVE_VERSION}"}\n`);
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args[0] === "release" && args[1] === "view" && args[2] === `v${ACTIVE_VERSION}`) {
            return {
              exitCode: 0,
              stdout: JSON.stringify({
                targetCommitish: ACTIVE_COMMIT,
                publishedAt: "2026-07-06T00:00:00Z",
                body: "Staging updater manifest\n\nSource-Branch: main"
              }),
              stderr: ""
            };
          }
          if (command === "git" && args.join(" ") === "fetch --tags origin") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "git" && args[0] === "merge-base") {
            return {
              exitCode: args[2] === ACTIVE_COMMIT && args[3] === "1234567890abcdef" ? 0 : 1,
              stdout: "",
              stderr: ""
            };
          }
          if (command === "git" && args.join(" ") === "status --porcelain") return { exitCode: 0, stdout: "", stderr: "" };
          if (command === "git" && args.join(" ") === "remote get-url origin") return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
          if (command === "git" && args.join(" ") === "rev-parse --abbrev-ref HEAD") {
            return { exitCode: 0, stdout: "main\n", stderr: "" };
          }
          if (command === "git" && args.join(" ") === "ls-remote --tags origin v1.2.4-staging.*") {
            return {
              exitCode: 0,
              stdout: [
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\trefs/tags/v1.2.4-staging.1",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\trefs/tags/v1.2.4-staging.2",
                "cccccccccccccccccccccccccccccccccccccccc\trefs/tags/v1.2.4-staging.3",
                "dddddddddddddddddddddddddddddddddddddddd\trefs/tags/v1.2.4-staging.4",
                "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\trefs/tags/v1.2.4-staging.5",
                "ffffffffffffffffffffffffffffffffffffffff\trefs/tags/v1.2.4-staging.6"
              ].join("\n"),
              stderr: ""
            };
          }
          if (command === "git" && args.join(" ") === "rev-parse HEAD") return { exitCode: 0, stdout: "1234567890abcdef\n", stderr: "" };
          if (command === "bazel" && args[0] === "build") return { exitCode: 0, stdout: "", stderr: "" };
          if (command === "bazel" && args[0] === "cquery") return { exitCode: 0, stdout: `${outputs.get(args[3]) ?? ""}\n`, stderr: "" };
          if (command === "sh" && args[0] === "-c") return { exitCode: 0, stdout: "", stderr: "" };
          if (command === "pnpm") {
            writeFileSync(`${args.at(-1)}.sig`, "staging signature\n");
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args.join(" ") === "release view desktop-staging --repo jemdiggity/kanna") return { exitCode: 0, stdout: "", stderr: "" };
          if (command === "gh" && args[0] === "release" && args[1] === "create") return { exitCode: 0, stdout: "", stderr: "" };
          if (command === "gh" && args[0] === "release" && args[1] === "upload") return { exitCode: 0, stdout: "", stderr: "" };
          if (command === "gh" && args[0] === "release" && args[1] === "view" && args[2] === "desktop-staging" && args.includes("--json")) {
            return {
              exitCode: 0,
              stdout: JSON.stringify({ assets: [{ name: "latest-staging.json" }] }),
              stderr: ""
            };
          }
          if (isProductionReleaseListQuery(command, args)) {
            return { exitCode: 0, stdout: "[]", stderr: "" };
          }
          if (command === "gh" && args[0] === "release" && args[1] === "list") {
            return {
              exitCode: 0,
              stdout: [
                "Kanna Staging v1.2.4-staging.1\t\tv1.2.4-staging.1\t2026-07-01T00:00:00Z",
                "Kanna Staging v1.2.4-staging.2\t\tv1.2.4-staging.2\t2026-07-02T00:00:00Z",
                "Kanna Staging v1.2.4-staging.3\t\tv1.2.4-staging.3\t2026-07-03T00:00:00Z",
                "Kanna Staging v1.2.4-staging.4\t\tv1.2.4-staging.4\t2026-07-04T00:00:00Z",
                "Kanna Staging v1.2.4-staging.5\t\tv1.2.4-staging.5\t2026-07-05T00:00:00Z",
                "Kanna Staging v1.2.4-staging.6\t\tv1.2.4-staging.6\t2026-07-06T00:00:00Z"
              ].join("\n"),
              stderr: ""
            };
          }
          if (command === "gh" && args[0] === "release" && args[1] === "delete") return { exitCode: 0, stdout: "", stderr: "" };
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${command} ${args.join(" ")}` };
        }
      };

      await shipRelease({
        repoRoot,
        bump: "patch",
        archLabels: ["arm64", "x86_64"],
        release: true,
        dryRun: false,
        environment: "staging",
        env: releaseEnv(privateKeyPath),
        runner
      });

      const deleteTags = calls
        .filter((call) => call.command === "gh" && call.args[0] === "release" && call.args[1] === "delete")
        .map((call) => call.args[2]);
      expect(deleteTags).toEqual([]);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("rolls back staging by clobbering the pointer manifest from a versioned prerelease", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          if (command === "git" && args.join(" ") === "status --porcelain") return { exitCode: 0, stdout: "", stderr: "" };
          if (command === "git" && args.join(" ") === "remote get-url origin") return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
          if (command === "gh" && args.join(" ") === "release view v1.2.4-staging.3 --repo jemdiggity/kanna") return { exitCode: 0, stdout: "", stderr: "" };
          if (command === "gh" && args.join(" ") === "release view desktop-staging --repo jemdiggity/kanna") return { exitCode: 0, stdout: "", stderr: "" };
          if (command === "gh" && args[0] === "release" && args[1] === "download") {
            expect(args).toEqual([
              "release",
              "download",
              "v1.2.4-staging.3",
              "--repo",
              "jemdiggity/kanna",
              "--pattern",
              "latest-staging.json",
              "--dir",
              join(repoRoot, ".build", "release", "staging"),
              "--clobber"
            ]);
            mkdirSync(join(repoRoot, ".build", "release", "staging"), { recursive: true });
            writeFileSync(join(repoRoot, ".build", "release", "staging", "latest-staging.json"), "{}\n");
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args[0] === "release" && args[1] === "upload") return { exitCode: 0, stdout: "", stderr: "" };
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${command} ${args.join(" ")}` };
        }
      };

      const result = await shipRelease({
        repoRoot,
        bump: "patch",
        archLabels: ["arm64", "x86_64"],
        release: false,
        dryRun: false,
        environment: "staging",
        rollbackTo: "1.2.4-staging.3",
        env: releaseEnv(privateKeyPath),
        runner
      });

      expect(result.version).toBe("1.2.4-staging.3");
      expect(result.dmgPaths).toEqual([]);
      expect(result.updaterPaths).toEqual([]);
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
      expect(calls.some((call) => call.command === "git" && call.args[0] === "ls-remote")).toBe(false);
      expect(calls.find((call) => call.command === "gh" && call.args[0] === "release" && call.args[1] === "upload")?.args).toEqual([
        "release",
        "upload",
        "desktop-staging",
        join(repoRoot, ".build", "release", "staging", "latest-staging.json"),
        "--repo",
        "jemdiggity/kanna",
        "--clobber"
      ]);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("fails clearly when rollback prerelease has no staging manifest asset", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const runner: CommandRunner = {
        async run(command, args) {
          if (command === "git" && args.join(" ") === "status --porcelain") return { exitCode: 0, stdout: "", stderr: "" };
          if (command === "git" && args.join(" ") === "remote get-url origin") return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
          if (command === "gh" && args.join(" ") === "release view v1.2.4-staging.3 --repo jemdiggity/kanna") return { exitCode: 0, stdout: "", stderr: "" };
          if (command === "gh" && args[0] === "release" && args[1] === "download") return { exitCode: 1, stdout: "", stderr: "no assets match pattern" };
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${command} ${args.join(" ")}` };
        }
      };

      await expect(
        shipRelease({
          repoRoot,
          bump: "patch",
          archLabels: ["arm64", "x86_64"],
          release: false,
          dryRun: false,
          environment: "staging",
          rollbackTo: "1.2.4-staging.3",
          env: releaseEnv(privateKeyPath),
          runner
        })
      ).rejects.toThrow("Staging manifest asset not found on v1.2.4-staging.3: latest-staging.json");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses to ship from a dirty git worktree before changing version files", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const originalFiles = readVersionFiles(repoRoot);
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          if (command === "git" && args.join(" ") === "status --porcelain") {
            return { exitCode: 0, stdout: " M VERSION\n", stderr: "" };
          }
          return { exitCode: 0, stdout: "", stderr: "" };
        }
      };

      await expect(
        shipRelease({
          repoRoot,
          bump: "patch",
          archLabels: ["arm64"],
          release: false,
          dryRun: true,
          env: releaseEnv(privateKeyPath),
          runner
        })
      ).rejects.toThrow("Refusing to ship a release from a dirty git worktree");

      expect(readVersionFiles(repoRoot)).toEqual(originalFiles);
      expect(calls).toEqual([
        {
          command: "git",
          args: ["status", "--porcelain"],
          options: { cwd: repoRoot, env: releaseEnv(privateKeyPath) }
        }
      ]);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("restores version files when the Bazel build fails", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const originalFiles = readVersionFiles(repoRoot);
      const runner: CommandRunner = {
        async run(command, args) {
          if (command === "git" && args.join(" ") === "status --porcelain") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "bazel" && args[0] === "build") {
            expect(readVersionFiles(repoRoot)).toEqual([
              "1.2.4\n",
              '{\n  "version": "1.2.4"\n}\n',
              '[package]\nname = "kanna"\nversion = "1.2.4"\n'
            ]);
            return { exitCode: 1, stdout: "", stderr: "bazel failed" };
          }
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${command} ${args.join(" ")}` };
        }
      };

      await expect(
        shipRelease({
          repoRoot,
          bump: "patch",
          archLabels: ["arm64"],
          release: false,
          dryRun: true,
          env: releaseEnv(privateKeyPath),
          runner
        })
      ).rejects.toThrow("bazel failed");

      expect(readVersionFiles(repoRoot)).toEqual(originalFiles);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("stops before creating the GitHub release when git commit fails", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          if (command === "git" && args.join(" ") === "status --porcelain") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "bazel" && args[0] === "build") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "git" && args.join(" ") === "remote get-url origin") {
            return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
          }
          if (command === "bazel" && args[0] === "cquery") {
            return { exitCode: 0, stdout: `${outputs.get(args[3]) ?? ""}\n`, stderr: "" };
          }
          if (command === "sh" && args[0] === "-c") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "pnpm") {
            const signedBundlePath = args.at(-1);
            expect(typeof signedBundlePath).toBe("string");
            writeFileSync(`${signedBundlePath}.sig`, "signature\n");
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args[0] === "api") {
            return { exitCode: 0, stdout: "release notes\n", stderr: "" };
          }
          if (command === "git" && args[0] === "add") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "git" && args[0] === "commit") {
            return { exitCode: 1, stdout: "", stderr: "commit failed" };
          }
          if (command === "git" && args[0] === "tag") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args[0] === "release") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${command} ${args.join(" ")}` };
        }
      };

      await expect(
        shipRelease({
          repoRoot,
          bump: "patch",
          archLabels: ["arm64", "x86_64"],
          release: true,
          dryRun: false,
          env: releaseEnv(privateKeyPath),
          runner
        })
      ).rejects.toThrow("commit failed");

      expect(calls.some((call) => call.command === "git" && call.args[0] === "tag")).toBe(false);
      expect(calls.some((call) => call.command === "gh" && call.args[0] === "release" && call.args[1] === "create")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("stops before creating the GitHub release when git tag fails", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          if (command === "git" && args.join(" ") === "status --porcelain") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "bazel" && args[0] === "build") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "git" && args.join(" ") === "remote get-url origin") {
            return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
          }
          if (command === "bazel" && args[0] === "cquery") {
            return { exitCode: 0, stdout: `${outputs.get(args[3]) ?? ""}\n`, stderr: "" };
          }
          if (command === "sh" && args[0] === "-c") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "pnpm") {
            const signedBundlePath = args.at(-1);
            expect(typeof signedBundlePath).toBe("string");
            writeFileSync(`${signedBundlePath}.sig`, "signature\n");
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args[0] === "api") {
            return { exitCode: 0, stdout: "release notes\n", stderr: "" };
          }
          if (command === "git" && args[0] === "add") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "git" && args[0] === "commit") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "git" && args[0] === "tag") {
            return { exitCode: 1, stdout: "", stderr: "tag failed" };
          }
          if (command === "gh" && args[0] === "release") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${command} ${args.join(" ")}` };
        }
      };

      await expect(
        shipRelease({
          repoRoot,
          bump: "patch",
          archLabels: ["arm64", "x86_64"],
          release: true,
          dryRun: false,
          env: releaseEnv(privateKeyPath),
          runner
        })
      ).rejects.toThrow("tag failed");

      expect(calls.some((call) => call.command === "gh" && call.args[0] === "release" && call.args[1] === "create")).toBe(false);
      expect(existsSync(join(repoRoot, ".build", "release", "latest.json"))).toBe(true);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("pushes main and the tag before creating the GitHub release", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    const releasedCommit = "cccccccccccccccccccccccccccccccccccccccc";
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          if (command === "git" && args.join(" ") === "status --porcelain") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "git" && args.join(" ") === "rev-parse HEAD") {
            return { exitCode: 0, stdout: `${releasedCommit}\n`, stderr: "" };
          }
          if (command === "bazel" && args[0] === "build") {
            expect(readVersionFiles(repoRoot)).toEqual([
              "1.2.4\n",
              '{\n  "version": "1.2.4"\n}\n',
              '[package]\nname = "kanna"\nversion = "1.2.4"\n'
            ]);
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "git" && args.join(" ") === "remote get-url origin") {
            return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
          }
          if (command === "bazel" && args[0] === "cquery") {
            return { exitCode: 0, stdout: `${outputs.get(args[3]) ?? ""}\n`, stderr: "" };
          }
          if (command === "sh" && args[0] === "-c") {
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "pnpm") {
            const signedBundlePath = args.at(-1);
            expect(typeof signedBundlePath).toBe("string");
            writeFileSync(`${signedBundlePath}.sig`, "signature\n");
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          return { exitCode: 0, stdout: command === "gh" && args[0] === "api" ? "release notes\n" : "", stderr: "" };
        }
      };

      const result = await shipRelease({
        repoRoot,
        bump: "patch",
        archLabels: ["arm64", "x86_64"],
        release: true,
        dryRun: false,
        env: releaseEnv(privateKeyPath),
        runner
      });

      const pushIndex = calls.findIndex((call) =>
        call.command === "git" &&
        call.args.join(" ") === "push origin HEAD:main v1.2.4"
      );
      const releaseCreateIndex = calls.findIndex((call) =>
        call.command === "gh" &&
        call.args[0] === "release" &&
        call.args[1] === "create"
      );

      expect(pushIndex).toBeGreaterThan(-1);
      expect(releaseCreateIndex).toBeGreaterThan(-1);
      expect(pushIndex).toBeLessThan(releaseCreateIndex);
      // A direct production ship is a release too, so it leaves the same series
      // branch behind — before the tag is pushed and before anything published.
      const seriesPushIndex = calls.findIndex((call) =>
        call.command === "git" &&
        call.args.join(" ") === `push origin ${releasedCommit}:refs/heads/release/1.2`
      );
      expect(seriesPushIndex).toBeGreaterThan(-1);
      expect(seriesPushIndex).toBeLessThan(pushIndex);
      expect(result.seriesBranch).toEqual({
        branch: "release/1.2",
        commit: releasedCommit,
        created: true,
        detail: null
      });
      expect(readVersionFiles(repoRoot)).toEqual([
        "1.2.4\n",
        '{\n  "version": "1.2.4"\n}\n',
        '[package]\nname = "kanna"\nversion = "1.2.4"\n'
      ]);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});

describe("release promotion", () => {
  const STAGING_COMMIT = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
  const PREVIOUS_RC_COMMIT = "9999999999999999999999999999999999999999";
  const RC_PUBLISHED_AT = "2026-07-01T00:00:00Z";
  // Four days after the RC was published: past the 24h default soak window.
  const PROMOTION_NOW = Date.parse("2026-07-05T00:00:00Z");

  function promoteRunner(overrides: Partial<Record<string, { exitCode: number; stdout: string; stderr: string }>>, repoRoot: string, outputs: Map<string, string>, calls: CommandCall[]): CommandRunner {
    return {
      async run(command, args, options) {
        calls.push({ command, args, options });
        const key = `${command} ${args.join(" ")}`;
        if (isProductionReleaseListQuery(command, args)) {
          return overrides["$production-release-list"] ?? { exitCode: 0, stdout: "[]", stderr: "" };
        }
        for (const [prefix, result] of Object.entries(overrides)) {
          if (key.startsWith(prefix) && result) return result;
        }
        if (command === "git" && args.join(" ") === "status --porcelain") {
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "git" && args.join(" ") === "remote get-url origin") {
          return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
        }
        if (command === "gh" && args.join(" ").startsWith("release view v1.2.4-staging.3")) {
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              tagName: "v1.2.4-staging.3",
              targetCommitish: STAGING_COMMIT,
              body: "Staging updater manifest for v1.2.4-staging.3\n\nSource-Branch: main",
              publishedAt: RC_PUBLISHED_AT,
              isPrerelease: true
            }),
            stderr: ""
          };
        }
        if (command === "gh" && args.join(" ").startsWith("release download v1.2.4-staging.3")) {
          const dirIndex = args.indexOf("--dir");
          writeFileSync(join(args[dirIndex + 1] ?? "", "latest-staging.json"), '{"version":"1.2.4-staging.3"}\n');
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        // Lineage: the RC being promoted descends from the candidate it replaced.
        if (command === "gh" && args[0] === "release" && args[1] === "list") {
          return {
            exitCode: 0,
            stdout: JSON.stringify([
              { tagName: "v1.2.4-staging.3", createdAt: RC_PUBLISHED_AT },
              { tagName: "v1.2.4-staging.2", createdAt: "2026-06-28T00:00:00Z" }
            ]),
            stderr: ""
          };
        }
        if (command === "gh" && args.join(" ").startsWith("release view v1.2.4-staging.2")) {
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              tagName: "v1.2.4-staging.2",
              targetCommitish: PREVIOUS_RC_COMMIT,
              body: "Staging updater manifest for v1.2.4-staging.2\n\nSource-Branch: main",
              publishedAt: "2026-06-28T00:00:00Z",
              isPrerelease: true
            }),
            stderr: ""
          };
        }
        if (command === "gh" && args.join(" ").startsWith("release view desktop-staging")) {
          return { exitCode: 0, stdout: '{"body":"Pointer-only desktop staging updater channel."}', stderr: "" };
        }
        if (key === "git fetch --tags origin") {
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (key === `git merge-base --is-ancestor ${PREVIOUS_RC_COMMIT} ${STAGING_COMMIT}`) {
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "git" && args.join(" ") === "ls-remote --tags origin v1.2.4") {
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "git" && args.join(" ").startsWith("ls-remote --tags origin refs/tags/v1.2.4-staging.3")) {
          return { exitCode: 0, stdout: `${STAGING_COMMIT}\trefs/tags/v1.2.4-staging.3\n`, stderr: "" };
        }
        if (command === "git" && args.join(" ") === "fetch --no-tags origin refs/tags/v1.2.4-staging.3") {
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "git" && args.join(" ") === "rev-parse FETCH_HEAD^{commit}") {
          return { exitCode: 0, stdout: `${STAGING_COMMIT}\n`, stderr: "" };
        }
        if (command === "git" && args.join(" ") === "rev-parse HEAD") {
          return { exitCode: 0, stdout: `${STAGING_COMMIT}\n`, stderr: "" };
        }
        if (command === "git" && args[0] === "cat-file" && args[1] === "-e") {
          return { exitCode: 1, stdout: "", stderr: "path does not exist" };
        }
        if (command === "git" && args.join(" ") === `show ${STAGING_COMMIT}:release-policy.json`) {
          return { exitCode: 1, stdout: "", stderr: "path does not exist" };
        }
        if (command === "git" && args.join(" ") === "fetch origin main") {
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "git" && args.join(" ") === "rev-parse origin/main") {
          return { exitCode: 0, stdout: `${STAGING_COMMIT}\n`, stderr: "" };
        }
        if (command === "bazel" && args[0] === "build") {
          expect(args).toSatisfy((values: string[]) =>
            values.includes("//:kanna_notarized_dmg_release_arm64") ||
            values.includes("//:kanna_signed_dmg_release_arm64")
          );
          expect(args).not.toContain("//:kanna_notarized_dmg_staging_arm64");
          expect(readVersionFiles(repoRoot)).toEqual([
            "1.2.4\n",
            '{\n  "version": "1.2.4"\n}\n',
            '[package]\nname = "kanna"\nversion = "1.2.4"\n'
          ]);
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "bazel" && args[0] === "cquery") {
          return { exitCode: 0, stdout: `${outputs.get(args[3] ?? "") ?? ""}\n`, stderr: "" };
        }
        if (command === "sh" && args[0] === "-c") {
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "pnpm") {
          const signedBundlePath = args.at(-1);
          expect(typeof signedBundlePath).toBe("string");
          writeFileSync(`${signedBundlePath}.sig`, "signature\n");
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        return { exitCode: 0, stdout: command === "gh" && args[0] === "api" ? "release notes\n" : "", stderr: "" };
      }
    };
  }

  function promoteInput(repoRoot: string, privateKeyPath: string, runner: CommandRunner): ReleaseShipInput {
    return {
      repoRoot,
      bump: "patch",
      archLabels: ["arm64", "x86_64"],
      environment: "production",
      release: true,
      dryRun: false,
      promoteFrom: "1.2.4-staging.3",
      now: PROMOTION_NOW,
      env: releaseEnv(privateKeyPath),
      runner
    };
  }

  it("parses staging versions into promotion versions", () => {
    expect(parsePromotionVersions("1.2.4-staging.3")).toEqual({
      stagingVersion: "1.2.4-staging.3",
      stagingTag: "v1.2.4-staging.3",
      productionVersion: "1.2.4"
    });
    expect(parsePromotionVersions("v1.2.4-staging.10").productionVersion).toBe("1.2.4");
    expect(() => parsePromotionVersions("1.2.4")).toThrow(/Invalid staging version/);
    expect(() => parsePromotionVersions("1.2.4-rc.1")).toThrow(/Invalid staging version/);
  });

  it("promotes a staging prerelease into a production release of the same commit", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({}, repoRoot, outputs, calls);

      const result = await shipRelease(promoteInput(repoRoot, privateKeyPath, runner));

      expect(result.version).toBe("1.2.4");
      expect(result.dmgPaths).toEqual([
        join(repoRoot, ".build", "release", "Kanna_1.2.4_arm64.dmg"),
        join(repoRoot, ".build", "release", "Kanna_1.2.4_x86_64.dmg")
      ]);
      const promoteViewIndex = calls.findIndex((call) => call.command === "gh" && call.args.join(" ").startsWith("release view v1.2.4-staging.3"));
      const buildIndex = calls.findIndex((call) => call.command === "bazel" && call.args[0] === "build");
      const pushIndex = calls.findIndex((call) => call.command === "git" && call.args.join(" ") === "push origin v1.2.4");
      const releaseCreateIndex = calls.findIndex((call) => call.command === "gh" && call.args[0] === "release" && call.args[1] === "create");
      expect(promoteViewIndex).toBeGreaterThan(-1);
      expect(buildIndex).toBeGreaterThan(promoteViewIndex);
      expect(pushIndex).toBeGreaterThan(buildIndex);
      expect(releaseCreateIndex).toBeGreaterThan(pushIndex);
      expect(calls.some((call) => call.command === "git" && call.args.some((arg) => arg.startsWith("HEAD:")))).toBe(false);
      expect(calls.find((call) => call.command === "gh" && call.args[1] === "create")?.args).toContain("v1.2.4");
      expect(readVersionFiles(repoRoot)).toEqual([
        "1.2.4\n",
        '{\n  "version": "1.2.4"\n}\n',
        '[package]\nname = "kanna"\nversion = "1.2.4"\n'
      ]);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("leaves the series branch behind at the released commit", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-series-branch-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({}, repoRoot, outputs, calls);

      const result = await shipRelease(promoteInput(repoRoot, privateKeyPath, runner));

      // Releasing 1.2.4 is what makes release/1.2 exist. Before this, a series
      // promoted off a bare main RC published a tag and no branch, so there was
      // nowhere to apply 1.2.5 from: `kd release cut` cuts at origin/main's tip.
      expect(result.seriesBranch).toEqual({
        branch: "release/1.2",
        commit: STAGING_COMMIT,
        created: true,
        detail: null
      });
      const seriesPushIndex = calls.findIndex(
        (call) => call.command === "git" && call.args.join(" ") === `push origin ${STAGING_COMMIT}:refs/heads/release/1.2`
      );
      const tagPushIndex = calls.findIndex((call) => call.command === "git" && call.args.join(" ") === "push origin v1.2.4");
      const releaseCreateIndex = calls.findIndex(
        (call) => call.command === "gh" && call.args[0] === "release" && call.args[1] === "create"
      );
      expect(seriesPushIndex).toBeGreaterThan(-1);
      // Branch first: a failure to write it aborts before the tag or the
      // GitHub release exist, instead of publishing a release with no branch.
      expect(seriesPushIndex).toBeLessThan(tagPushIndex);
      expect(seriesPushIndex).toBeLessThan(releaseCreateIndex);
      // The branch is created, never forced: a promotion cannot rewind a series.
      expect(calls.some((call) => call.command === "git" && call.args.includes("--force"))).toBe(false);
      expect(calls.some((call) => call.command === "git" && call.args.some((arg) => arg.startsWith("+")))).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("promotes a committed-version candidate by tagging it, with no release commit", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-promote-no-commit-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      // The candidate's own commit already states 1.2.4, so promoting it is
      // dropping the `-staging.3` suffix and nothing else.
      writeBranchCandidate(repoRoot, "1.2.4", 3);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({}, repoRoot, outputs, calls);

      const result = await shipRelease(promoteInput(repoRoot, privateKeyPath, runner));

      expect(result.version).toBe("1.2.4");
      // No `release: v1.2.4` commit and nothing staged for one: the tag goes
      // onto the very commit that soaked, not onto a child of it.
      expect(calls.some((call) => call.command === "git" && call.args[0] === "commit")).toBe(false);
      expect(calls.some((call) => call.command === "git" && call.args[0] === "add")).toBe(false);
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === "tag v1.2.4")).toBe(true);
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === "push origin v1.2.4")).toBe(true);
      // Production ignores the candidate counter, so the build reads the
      // committed files untouched -- the same commit built the RC.
      expect(readVersionFiles(repoRoot)[0]).toBe("1.2.4\n");
      expect(result.seriesBranch).toMatchObject({ branch: "release/1.2", commit: STAGING_COMMIT, created: true });
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("still commits the version when promoting a candidate that did not carry it", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-promote-derived-"));
    try {
      // A bare-main candidate's commit says 1.2.3 while it promotes to 1.2.4,
      // so kd still has a version to write and still commits it. Removing that
      // would ship a bundle stamped with the wrong version.
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({}, repoRoot, outputs, calls);

      await shipRelease(promoteInput(repoRoot, privateKeyPath, runner));

      expect(calls.some((call) => call.command === "git" && call.args[0] === "commit")).toBe(true);
      expect(readVersionFiles(repoRoot)[0]).toBe("1.2.4\n");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("is idempotent when the series branch already holds the released commit", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-series-branch-idempotent-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "git ls-remote origin refs/heads/release/1.2": {
          exitCode: 0,
          stdout: `${STAGING_COMMIT}\trefs/heads/release/1.2\n`,
          stderr: ""
        }
      }, repoRoot, outputs, calls);

      const result = await shipRelease(promoteInput(repoRoot, privateKeyPath, runner));

      expect(result.seriesBranch).toEqual({
        branch: "release/1.2",
        commit: STAGING_COMMIT,
        created: false,
        detail: null
      });
      expect(
        calls.some((call) => call.command === "git" && call.args[0] === "push" && call.args.join(" ").includes("refs/heads/release/1.2"))
      ).toBe(false);
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === "push origin v1.2.4")).toBe(true);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("reports an existing series branch that holds other work without moving it", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-series-branch-existing-"));
    const backportTip = "dddddddddddddddddddddddddddddddddddddddd";
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "git ls-remote origin refs/heads/release/1.2": {
          exitCode: 0,
          stdout: `${backportTip}\trefs/heads/release/1.2\n`,
          stderr: ""
        }
      }, repoRoot, outputs, calls);

      const result = await shipRelease(promoteInput(repoRoot, privateKeyPath, runner));

      // The branch may legitimately carry backports past this release. It is
      // reported, not rewound; the released commit stays reachable by its tag.
      expect(result.seriesBranch).toMatchObject({
        branch: "release/1.2",
        commit: backportTip,
        created: false
      });
      expect(result.seriesBranch?.detail).toMatch(/only ever creates a missing series branch/);
      expect(
        calls.some((call) => call.command === "git" && call.args[0] === "push" && call.args.join(" ").includes("refs/heads/release/1.2"))
      ).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("publishes nothing when the series branch cannot be written", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-series-branch-failure-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        [`git push origin ${STAGING_COMMIT}:refs/heads/release/1.2`]: {
          exitCode: 1,
          stdout: "",
          stderr: "remote: refusing to create refs/heads/release/1.2\n"
        }
      }, repoRoot, outputs, calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /refusing to create refs\/heads\/release\/1\.2/
      );
      // Fail-closed: no production tag on origin and no GitHub release, so a
      // retry completes the whole publication rather than patching up a
      // half-released state.
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === "push origin v1.2.4")).toBe(false);
      expect(calls.some((call) => call.command === "gh" && call.args[0] === "release" && call.args[1] === "create")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses to promote when the staging prerelease does not exist", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "gh release view v1.2.4-staging.3": { exitCode: 1, stdout: "", stderr: "release not found" }
      }, repoRoot, new Map(), calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(/Staging prerelease not found: v1\.2\.4-staging\.3/);
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("rejects staging release metadata that does not name the selected prerelease", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "gh release view v1.2.4-staging.3": {
          exitCode: 0,
          stdout: JSON.stringify({
            tagName: "v1.2.4-staging.30",
            targetCommitish: STAGING_COMMIT,
            body: "Staging updater manifest for v1.2.4-staging.3\n\nSource-Branch: main",
            publishedAt: RC_PUBLISHED_AT,
            isPrerelease: true
          }),
          stderr: ""
        }
      }, repoRoot, new Map(), calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /metadata tag v1\.2\.4-staging\.30 does not match selected tag v1\.2\.4-staging\.3/
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("rejects a GitHub release that is not a prerelease", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "gh release view v1.2.4-staging.3": {
          exitCode: 0,
          stdout: JSON.stringify({
            tagName: "v1.2.4-staging.3",
            targetCommitish: STAGING_COMMIT,
            body: "Staging updater manifest for v1.2.4-staging.3\n\nSource-Branch: main",
            publishedAt: RC_PUBLISHED_AT,
            isPrerelease: false
          }),
          stderr: ""
        }
      }, repoRoot, new Map(), calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(/is not marked as a GitHub prerelease/);
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("rejects a candidate whose versioned staging manifest cannot be verified", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "gh release download v1.2.4-staging.3": { exitCode: 1, stdout: "", stderr: "manifest unavailable" }
      }, repoRoot, new Map(), calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /Staging manifest asset not found.*manifest unavailable/
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("rejects a staging tag whose remote commit disagrees with release metadata", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const mismatchedCommit = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
      const runner = promoteRunner({
        "git ls-remote --tags origin refs/tags/v1.2.4-staging.3": {
          exitCode: 0,
          stdout: `${mismatchedCommit}\trefs/tags/v1.2.4-staging.3\n`,
          stderr: ""
        }
      }, repoRoot, new Map(), calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        new RegExp(`tag resolves to ${mismatchedCommit}.*records ${STAGING_COMMIT}`)
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("rechecks the fetched staging tag before building", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const fetchedCommit = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
      const runner = promoteRunner({
        "git rev-parse FETCH_HEAD^{commit}": { exitCode: 0, stdout: `${fetchedCommit}\n`, stderr: "" }
      }, repoRoot, new Map(), calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        new RegExp(`Fetched v1\\.2\\.4-staging\\.3 resolves to ${fetchedCommit}.*verified immutable commit is ${STAGING_COMMIT}`)
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses to promote a version whose production tag already exists", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "git ls-remote --tags origin v1.2.4": { exitCode: 0, stdout: `${STAGING_COMMIT}\trefs/tags/v1.2.4\n`, stderr: "" }
      }, repoRoot, new Map(), calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(/Production tag v1\.2\.4 already exists/);
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses a production-regressing historical candidate before building", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "$production-release-list": {
          exitCode: 0,
          stdout: JSON.stringify([{ tagName: "v1.2.5", isPrerelease: false }]),
          stderr: ""
        }
      }, repoRoot, new Map(), calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /v1\.2\.4 does not advance.*v1\.2\.5/s
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("promotes the exact soaked A after more than five newer RCs publish while leaving staging and main advanced", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      for (const label of ["arm64", "x86_64"] as const) {
        outputs.set(bazelTargetForLabel(label, true), outputs.get(bazelTargetForLabel(label, false)) ?? "");
      }
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "gh release list --repo jemdiggity/kanna": {
          exitCode: 0,
          stdout: JSON.stringify([
            ...Array.from({ length: 6 }, (_, index) => ({
              tagName: `v1.2.4-staging.${9 - index}`,
              createdAt: `2026-07-${String(8 - index).padStart(2, "0")}T00:00:00Z`
            })),
            { tagName: "v1.2.4-staging.3", createdAt: RC_PUBLISHED_AT },
            { tagName: "v1.2.4-staging.2", createdAt: "2026-06-28T00:00:00Z" }
          ]),
          stderr: ""
        },
        "git rev-parse origin/main": { exitCode: 0, stdout: "ffffffffffffffffffffffffffffffffffffffff\n", stderr: "" }
      }, repoRoot, outputs, calls);

      await expect(shipRelease({
        ...promoteInput(repoRoot, privateKeyPath, runner),
        release: false,
        dryRun: true
      })).resolves.toMatchObject({ version: "1.2.4" });
      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).resolves.toMatchObject({ version: "1.2.4" });
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === "push origin v1.2.4")).toBe(true);
      expect(calls.some((call) => call.command === "git" && call.args.some((arg) => arg === "HEAD:main"))).toBe(false);
      expect(calls.some((call) =>
        call.command === "gh" &&
        call.args.includes("desktop-staging") &&
        ["edit", "upload"].includes(call.args[1] ?? "")
      )).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("promotes applied recut A after the release branch advances to B and newer audit records exist", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-historical-recut-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const candidateB = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
      const body = [
        "Pointer-only desktop staging updater channel.",
        "",
        "Lineage-Recut-Applied: 2026-07-02T01:00:00Z",
        "Recut-Applied-Id: 1.2-2",
        "Recut-Applied-Version: 1.2.4-staging.4",
        `Recut-Applied-Commit: ${candidateB}`,
        "Recut-Applied-Tag: recut-applied/1.2-2",
        "",
        "Lineage-Recut: 2026-07-02T00:00:00Z",
        "Recut-Id: 1.2-2",
        "Recut-Series: 1.2",
        "Recut-Branch: release/1.2",
        `Recut-Old-Tip: ${STAGING_COMMIT}`,
        `Recut-New-Tip: ${candidateB}`,
        "Recut-Archive-Tag: recut/release/1.2-2",
        `Recut-From: 1.2.4-staging.3 (${STAGING_COMMIT}) source release/1.2`,
        "Recut-Prior-Epoch: 1.2.4-staging.3",
        "Recut-Requester: later-test",
        "Recut-Reason: advance to B",
        "",
        "Lineage-Recut-Applied: 2026-07-01T01:00:00Z",
        "Recut-Applied-Id: 1.2-1",
        "Recut-Applied-Version: 1.2.4-staging.3",
        `Recut-Applied-Commit: ${STAGING_COMMIT}`,
        "Recut-Applied-Tag: recut-applied/1.2-1",
        "",
        "Lineage-Recut: 2026-07-01T00:00:00Z",
        "Recut-Id: 1.2-1",
        "Recut-Series: 1.2",
        "Recut-Branch: release/1.2",
        `Recut-Old-Tip: ${PREVIOUS_RC_COMMIT}`,
        `Recut-New-Tip: ${STAGING_COMMIT}`,
        "Recut-Archive-Tag: recut/release/1.2-1",
        `Recut-From: 1.2.4-staging.2 (${PREVIOUS_RC_COMMIT}) source release/1.2`,
        "Recut-Prior-Epoch: 1.2.4-staging.2",
        "Recut-Requester: promotion-test",
        "Recut-Reason: authorize A"
      ].join("\n");
      const runner = promoteRunner({
        "gh release view v1.2.4-staging.3": {
          exitCode: 0,
          stdout: JSON.stringify({
            tagName: "v1.2.4-staging.3",
            targetCommitish: STAGING_COMMIT,
            body: "Staging updater manifest for v1.2.4-staging.3\n\nSource-Branch: release/1.2",
            publishedAt: RC_PUBLISHED_AT,
            isPrerelease: true
          }),
          stderr: ""
        },
        "gh release list --repo jemdiggity/kanna": {
          exitCode: 0,
          stdout: JSON.stringify([
            { tagName: "v1.2.4-staging.4", createdAt: "2026-07-02T00:00:00Z" },
            { tagName: "v1.2.4-staging.3", createdAt: RC_PUBLISHED_AT },
            { tagName: "v1.2.4-staging.2", createdAt: "2026-06-28T00:00:00Z" }
          ]),
          stderr: ""
        },
        "gh release view desktop-staging": {
          exitCode: 0,
          stdout: JSON.stringify({ body }),
          stderr: ""
        },
        [`git merge-base --is-ancestor ${PREVIOUS_RC_COMMIT} ${STAGING_COMMIT}`]: { exitCode: 1, stdout: "", stderr: "" },
        [`git merge-base --is-ancestor ${STAGING_COMMIT} ${PREVIOUS_RC_COMMIT}`]: { exitCode: 1, stdout: "", stderr: "" },
        "git ls-remote origin refs/heads/release/1.2": {
          exitCode: 0,
          stdout: `${candidateB}\trefs/heads/release/1.2\n`,
          stderr: ""
        }
      }, repoRoot, outputs, calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).resolves.toMatchObject({ version: "1.2.4" });
      expect(calls.some((call) => call.command === "bazel" && call.args[0] === "build")).toBe(true);
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === "push origin v1.2.4")).toBe(true);
      // The series branch has advanced to B. Promoting historical A reads it and
      // leaves it exactly there: a release only ever creates a missing branch,
      // so an older candidate can never rewind a live one.
      expect(
        calls.some((call) => call.command === "git" && call.args[0] === "push" && call.args.join(" ").includes("refs/heads/release/1.2"))
      ).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses promotion before building when a later RC tries to reuse a consumed recut grant", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-consumed-recut-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const recutTip = "cccccccccccccccccccccccccccccccccccccccc";
      const appliedCommit = "dddddddddddddddddddddddddddddddddddddddd";
      const body = [
        "Pointer-only desktop staging updater channel.",
        "",
        "Lineage-Recut-Applied: 2026-07-01T01:00:00Z",
        "Recut-Applied-Id: 1.2-1",
        "Recut-Applied-Version: 1.2.4-staging.2",
        `Recut-Applied-Commit: ${appliedCommit}`,
        "Recut-Applied-Tag: recut-applied/1.2-1",
        "",
        "Lineage-Recut: 2026-07-01T00:00:00Z",
        "Recut-Id: 1.2-1",
        "Recut-Series: 1.2",
        "Recut-Branch: release/1.2",
        `Recut-Old-Tip: ${PREVIOUS_RC_COMMIT}`,
        `Recut-New-Tip: ${recutTip}`,
        "Recut-Archive-Tag: recut/release/1.2-1",
        `Recut-From: 1.2.4-staging.1 (${PREVIOUS_RC_COMMIT}) source release/1.2`,
        "Recut-Prior-Epoch: 1.2.4-staging.1",
        "Recut-Requester: consumed-grant-test",
        "Recut-Reason: authorize staging.2 only"
      ].join("\n");
      const runner = promoteRunner({
        "gh release view v1.2.4-staging.3": {
          exitCode: 0,
          stdout: JSON.stringify({
            tagName: "v1.2.4-staging.3",
            targetCommitish: STAGING_COMMIT,
            body: "Staging updater manifest for v1.2.4-staging.3\n\nSource-Branch: release/1.2",
            publishedAt: RC_PUBLISHED_AT,
            isPrerelease: true
          }),
          stderr: ""
        },
        "gh release list --repo jemdiggity/kanna": {
          exitCode: 0,
          stdout: JSON.stringify([
            { tagName: "v1.2.4-staging.3", createdAt: RC_PUBLISHED_AT },
            { tagName: "v1.2.4-staging.2", createdAt: "2026-06-30T00:00:00Z" },
            { tagName: "v1.2.4-staging.1", createdAt: "2026-06-29T00:00:00Z" }
          ]),
          stderr: ""
        },
        "gh release view v1.2.4-staging.2": {
          exitCode: 0,
          stdout: JSON.stringify({
            tagName: "v1.2.4-staging.2",
            targetCommitish: appliedCommit,
            body: "Staging updater manifest for v1.2.4-staging.2\n\nSource-Branch: release/1.2",
            publishedAt: "2026-06-30T00:00:00Z",
            isPrerelease: true
          }),
          stderr: ""
        },
        "gh release view desktop-staging": {
          exitCode: 0,
          stdout: JSON.stringify({ body }),
          stderr: ""
        },
        [`git merge-base --is-ancestor ${appliedCommit} ${STAGING_COMMIT}`]: { exitCode: 1, stdout: "", stderr: "" },
        [`git merge-base --is-ancestor ${STAGING_COMMIT} ${appliedCommit}`]: { exitCode: 1, stdout: "", stderr: "" },
        [`git merge-base --is-ancestor ${recutTip} ${STAGING_COMMIT}`]: { exitCode: 0, stdout: "", stderr: "" },
        "git ls-remote origin refs/heads/release/1.2": {
          exitCode: 0,
          stdout: `${STAGING_COMMIT}\trefs/heads/release/1.2\n`,
          stderr: ""
        }
      }, repoRoot, new Map(), calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /share only an older merge base/
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
      expect(calls).toContainEqual(expect.objectContaining({
        command: "git",
        args: ["merge-base", "--is-ancestor", appliedCommit, STAGING_COMMIT]
      }));
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses to promote when HEAD is not the staging build's commit", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "git rev-parse HEAD": { exitCode: 0, stdout: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\n", stderr: "" }
      }, repoRoot, new Map(), calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(/is not the commit v1\.2\.4-staging\.3 was built from/);
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("promotes a soaked branch RC while origin/main keeps moving", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "gh release view v1.2.4-staging.3": {
          exitCode: 0,
          stdout: `{"tagName":"v1.2.4-staging.3","targetCommitish":"${STAGING_COMMIT}","publishedAt":"${RC_PUBLISHED_AT}","body":"Staging updater manifest for v1.2.4-staging.3\\n\\nSource-Branch: release/1.2","isPrerelease":true}\n`,
          stderr: ""
        },
        "git ls-remote origin refs/heads/release/1.2": {
          exitCode: 0,
          stdout: `${STAGING_COMMIT}\trefs/heads/release/1.2\n`,
          stderr: ""
        },
        "git rev-parse origin/main": {
          exitCode: 0,
          stdout: "ffffffffffffffffffffffffffffffffffffffff\n",
          stderr: ""
        }
      }, repoRoot, outputs, calls);

      const result = await shipRelease(promoteInput(repoRoot, privateKeyPath, runner));

      expect(result.version).toBe("1.2.4");
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === "push origin v1.2.4")).toBe(true);
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === "fetch origin main")).toBe(false);
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === "rev-parse origin/main")).toBe(false);
      const notesCall = calls.find((call) => call.command === "gh" && call.args[0] === "api");
      expect(notesCall?.args).toContain(`target_commitish=${STAGING_COMMIT}`);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("keeps a release-branch RC promotable when the branch has advanced past it", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "gh release view v1.2.4-staging.3": {
          exitCode: 0,
          stdout: `{"tagName":"v1.2.4-staging.3","targetCommitish":"${STAGING_COMMIT}","publishedAt":"${RC_PUBLISHED_AT}","body":"Staging updater manifest for v1.2.4-staging.3\\n\\nSource-Branch: release/1.2","isPrerelease":true}\n`,
          stderr: ""
        },
        "git ls-remote origin refs/heads/release/1.2": {
          exitCode: 0,
          stdout: "ffffffffffffffffffffffffffffffffffffffff\trefs/heads/release/1.2\n",
          stderr: ""
        }
      }, repoRoot, outputs, calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).resolves.toMatchObject({ version: "1.2.4" });
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === "fetch origin main")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("promotes a main RC to main even when a dormant same-series release branch exists", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "gh release view v1.2.4-staging.3": {
          exitCode: 0,
          stdout: `{"tagName":"v1.2.4-staging.3","targetCommitish":"${STAGING_COMMIT}","publishedAt":"${RC_PUBLISHED_AT}","body":"Staging updater manifest for v1.2.4-staging.3\\n\\nSource-Branch: main","isPrerelease":true}\n`,
          stderr: ""
        },
        "git ls-remote origin refs/heads/release/1.2": {
          exitCode: 0,
          stdout: "dddddddddddddddddddddddddddddddddddddddd\trefs/heads/release/1.2\n",
          stderr: ""
        }
      }, repoRoot, outputs, calls);

      const result = await shipRelease(promoteInput(repoRoot, privateKeyPath, runner));

      expect(result.version).toBe("1.2.4");
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === "push origin v1.2.4")).toBe(true);
      const notesCall = calls.find((call) => call.command === "gh" && call.args[0] === "api");
      expect(notesCall?.args).toContain(`target_commitish=${STAGING_COMMIT}`);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("keeps a release-branch RC promotable when its source branch was deleted", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "gh release view v1.2.4-staging.3": {
          exitCode: 0,
          stdout: `{"tagName":"v1.2.4-staging.3","targetCommitish":"${STAGING_COMMIT}","publishedAt":"${RC_PUBLISHED_AT}","body":"Staging updater manifest for v1.2.4-staging.3\\n\\nSource-Branch: release/1.2","isPrerelease":true}\n`,
          stderr: ""
        }
      }, repoRoot, outputs, calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).resolves.toMatchObject({ version: "1.2.4" });
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === "push origin v1.2.4")).toBe(true);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses to promote a candidate whose lineage diverged from the one it replaced", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        [`git merge-base --is-ancestor ${PREVIOUS_RC_COMMIT} ${STAGING_COMMIT}`]: { exitCode: 1, stdout: "", stderr: "" },
        [`git merge-base --is-ancestor ${STAGING_COMMIT} ${PREVIOUS_RC_COMMIT}`]: { exitCode: 1, stdout: "", stderr: "" }
      }, repoRoot, new Map(), calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /share only an older merge base/
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses promotion before building when a full first page cannot load complete release history", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-incomplete-history-promote-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const firstPage = Array.from({ length: 100 }, (_, index) => ({
        tagName: `v1.2.4-staging.${102 - index}`,
        createdAt: new Date(PROMOTION_NOW - (index + 1) * 3_600_000).toISOString()
      }));
      const runner = promoteRunner({
        "gh release list --repo jemdiggity/kanna": {
          exitCode: 0,
          stdout: JSON.stringify(firstPage),
          stderr: ""
        },
        "gh api --paginate": {
          exitCode: 1,
          stdout: "",
          stderr: "HTTP 503: Service unavailable"
        }
      }, repoRoot, new Map(), calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /Could not read complete GitHub release history.*HTTP 503/s
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses to promote before the policy soak window has elapsed", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({}, repoRoot, new Map(), calls);

      await expect(
        shipRelease({ ...promoteInput(repoRoot, privateKeyPath, runner), now: Date.parse("2026-07-01T05:00:00Z") })
      ).rejects.toThrow(/soaked 5\.0h of the required 24h/);
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it.each([
    { checkoutHours: 1, candidateHours: 24, allowed: false },
    { checkoutHours: 24, candidateHours: 1, allowed: true }
  ])("promotion uses candidate policy $candidateHours hours when checkout policy is $checkoutHours hours", async ({ checkoutHours, candidateHours, allowed }) => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-policy-source-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      writeFileSync(join(repoRoot, "release-policy.json"), JSON.stringify({ productionSoakHours: checkoutHours }));
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      for (const label of ["arm64", "x86_64"] as const) {
        outputs.set(bazelTargetForLabel(label, true), outputs.get(bazelTargetForLabel(label, false)) ?? "");
      }
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        [`git cat-file -e ${STAGING_COMMIT}:release-policy.json`]: {
          exitCode: 0,
          stdout: "",
          stderr: ""
        },
        [`git show ${STAGING_COMMIT}:release-policy.json`]: {
          exitCode: 0,
          stdout: JSON.stringify({ productionSoakHours: candidateHours }),
          stderr: ""
        }
      }, repoRoot, outputs, calls);
      const operation = shipRelease({
        ...promoteInput(repoRoot, privateKeyPath, runner),
        release: false,
        dryRun: true,
        now: Date.parse("2026-07-01T03:00:00Z")
      });

      if (allowed) {
        await expect(operation).resolves.toMatchObject({ version: "1.2.4" });
      } else {
        await expect(operation).rejects.toThrow(/soaked 3\.0h of the required 24h/);
        expect(calls.some((call) => call.command === "bazel")).toBe(false);
      }
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("promotes inside the soak window only through the explicit human override", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({}, repoRoot, outputs, calls);

      const result = await shipRelease({
        ...promoteInput(repoRoot, privateKeyPath, runner),
        now: Date.parse("2026-07-01T05:00:00Z"),
        soakOverrideReason: "Grace requested the ship; the fix is a one-line crash guard"
      });

      expect(result.version).toBe("1.2.4");
      expect(calls.some((call) => call.command === "bazel" && call.args[0] === "build")).toBe(true);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("runs the same gates for a --dry-run rehearsal", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({}, repoRoot, new Map(), calls);

      await expect(
        shipRelease({
          ...promoteInput(repoRoot, privateKeyPath, runner),
          release: false,
          dryRun: true,
          now: Date.parse("2026-07-01T05:00:00Z")
        })
      ).rejects.toThrow(/soaked 5\.0h of the required 24h/);
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses to promote a candidate from an abandoned series", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = promoteRunner({
        "git ls-remote --tags origin refs/tags/abandoned/release/1.2": {
          exitCode: 0,
          stdout: "sha\trefs/tags/abandoned/release/1.2\n",
          stderr: ""
        },
        "git for-each-ref": {
          exitCode: 0,
          stdout: "Abandoned release/1.2 at 2026-08-13T09:00:00.000Z\n\nReason: superseded by 1.3\n",
          stderr: ""
        }
      }, repoRoot, new Map(), calls);

      await expect(shipRelease(promoteInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /release\/1\.2 was abandoned on 2026-08-13T09:00:00\.000Z: superseded by 1\.3/
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("rejects promotion aimed at the staging environment", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const runner = promoteRunner({}, repoRoot, new Map(), []);
      await expect(shipRelease({
        ...promoteInput(repoRoot, privateKeyPath, runner),
        environment: "staging"
      })).rejects.toThrow(/cannot target staging/);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});

describe("release series", () => {
  it("compares semantic versions including prerelease ordering", () => {
    expect(compareVersions("1.2.4-staging.10", "1.2.4-staging.9")).toBeGreaterThan(0);
    expect(compareVersions("1.2.4-staging.9", "1.2.4-staging.10")).toBeLessThan(0);
    expect(compareVersions("1.2.4", "1.2.4-staging.10")).toBeGreaterThan(0);
    expect(compareVersions("1.2.4-staging.10", "1.2.4-staging.10")).toBe(0);
  });

  it("derives series and branch names from versions", () => {
    expect(releaseSeriesFromVersion("1.2.4")).toEqual({ major: 1, minor: 2 });
    expect(releaseSeriesFromVersion("v1.2.4-staging.3")).toEqual({ major: 1, minor: 2 });
    expect(releaseSeriesBranch({ major: 1, minor: 2 })).toBe("release/1.2");
    expect(() => releaseSeriesFromVersion("nope")).toThrow(/Invalid version/);
  });

  it("parses release branch names", () => {
    expect(parseReleaseBranchSeries("release/1.2")).toEqual({ major: 1, minor: 2 });
    expect(parseReleaseBranchSeries("main")).toBeNull();
    expect(parseReleaseBranchSeries("release/1.2.3")).toBeNull();
    expect(parseReleaseBranchSeries("feature/release/1.2")).toBeNull();
  });

  it("computes the next patch version for a series from released tags", () => {
    expect(nextSeriesPatchVersion("", { major: 1, minor: 3 })).toBe("1.3.0");
    const tags = [
      "sha1\trefs/tags/v1.3.0",
      "sha2\trefs/tags/v1.3.0^{}",
      "sha3\trefs/tags/v1.3.2",
      "sha4\trefs/tags/v1.3.0-staging.4",
      "sha5\trefs/tags/v1.4.0"
    ].join("\n");
    expect(nextSeriesPatchVersion(tags, { major: 1, minor: 3 })).toBe("1.3.3");
  });

  it("floors a main staging version above the greatest production semantic version when VERSION is stale", () => {
    const derivation = deriveMainStagingBaseVersion("0.0.68", "0.2.0", "patch");
    expect(derivation.baseVersion).toBe("0.2.1");
    expect(compareVersions(derivation.baseVersion, "0.2.0")).toBeGreaterThan(0);
    expect(derivation.versionFloor?.detail).toMatch(
      /VERSION 0\.0\.68 lags greatest production semantic version v0\.2\.0/
    );

    expect(deriveMainStagingBaseVersion("0.3.0", "0.2.0", "patch")).toEqual({
      baseVersion: "0.3.1",
      versionFloor: null
    });
  });

});

describe("release cut", () => {
  const MAIN_SHA = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
  const RELEASE_01_SHA = "0101010101010101010101010101010101010101";
  const CUT_NOW = Date.parse("2026-08-13T09:00:00Z");

  interface CutFixture {
    trunkVersion?: string;
    /** Release branches present on origin, as branch -> tip sha. */
    releaseBranches?: Record<string, string>;
    /** Production tags present on origin, without the leading v. */
    productionTags?: string[];
    /**
     * Staging prereleases present on origin, without the leading v. Real
     * `ls-remote --tags origin 'vX.Y.*'` returns these alongside production
     * tags: it expands the pattern with a leading wildcard-and-slash, and that
     * wildcard crosses path separators.
     */
    stagingTags?: string[];
    /** Series already carrying an abandonment tag, as `X.Y` -> tag message. */
    abandonedSeries?: Record<string, string>;
    activeStagingVersion?: string | null;
    activeStagingSourceBranch?: string;
    channelBody?: string;
  }

  function cutRunner(fixture: CutFixture, calls: CommandCall[]): CommandRunner {
    const branches = fixture.releaseBranches ?? {};
    return {
      async run(command, args, options) {
        calls.push({ command, args, options });
        const key = `${command} ${args.join(" ")}`;
        if (key === "git fetch origin main") return { exitCode: 0, stdout: "", stderr: "" };
        if (key === "git rev-parse origin/main") return { exitCode: 0, stdout: `${MAIN_SHA}\n`, stderr: "" };
        if (key === "git show origin/main:VERSION") {
          return { exitCode: 0, stdout: `${fixture.trunkVersion ?? "1.2.3"}\n`, stderr: "" };
        }
        if (key === "git remote get-url origin") {
          return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
        }
        if (isProductionReleaseListQuery(command, args)) {
          return {
            exitCode: 0,
            stdout: JSON.stringify(
              (fixture.productionTags ?? []).map((version) => ({ tagName: `v${version}`, isPrerelease: false }))
            ),
            stderr: ""
          };
        }
        if (key === "git ls-remote --heads origin refs/heads/release/*") {
          return {
            exitCode: 0,
            stdout: Object.entries(branches).map(([branch, sha]) => `${sha}\trefs/heads/${branch}`).join("\n"),
            stderr: ""
          };
        }
        if (command === "git" && args[0] === "ls-remote" && args[1] === "--tags") {
          const pattern = args[3] ?? "";
          const abandonedMatch = /^refs\/tags\/abandoned\/release\/(\d+\.\d+)$/.exec(pattern);
          if (abandonedMatch) {
            const has = Boolean(fixture.abandonedSeries?.[abandonedMatch[1] ?? ""]);
            return { exitCode: 0, stdout: has ? `sha\t${pattern}\n` : "", stderr: "" };
          }
          const seriesMatch = /^v(\d+\.\d+)\.\*$/.exec(pattern);
          if (seriesMatch) {
            const series = seriesMatch[1] ?? "";
            const matched = [...(fixture.productionTags ?? []), ...(fixture.stagingTags ?? [])].filter((tag) =>
              tag.startsWith(`${series}.`)
            );
            return {
              exitCode: 0,
              stdout: matched.map((tag) => `sha\trefs/tags/v${tag}`).join("\n"),
              stderr: ""
            };
          }
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "git" && args[0] === "for-each-ref") {
          const ref = args[2] ?? "";
          const series = /abandoned\/release\/(\d+\.\d+)$/.exec(ref)?.[1] ?? "";
          return { exitCode: 0, stdout: fixture.abandonedSeries?.[series] ?? "", stderr: "" };
        }
        if (isStagingChannelAssetsQuery(command, args)) {
          return stagingChannelAssetsResponse(fixture.activeStagingVersion ? ["latest-staging.json"] : null);
        }
        if (command === "gh" && args[0] === "release" && args[1] === "download") {
          if (!fixture.activeStagingVersion) return { exitCode: 1, stdout: "", stderr: "release not found" };
          const dirIndex = args.indexOf("--dir");
          writeFileSync(
            join(args[dirIndex + 1] ?? "", "latest-staging.json"),
            `{"version":"${fixture.activeStagingVersion}"}\n`
          );
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "gh" && args[1] === "view" && args[2] === "desktop-staging") {
          return { exitCode: 0, stdout: JSON.stringify({ body: fixture.channelBody ?? "" }), stderr: "" };
        }
        if (command === "gh" && args[0] === "release" && args[1] === "view") {
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              targetCommitish: RELEASE_01_SHA,
              publishedAt: "2026-08-01T00:00:00Z",
              body: `Staging updater manifest\n\nSource-Branch: ${fixture.activeStagingSourceBranch ?? "main"}`
            }),
            stderr: ""
          };
        }
        if (command === "git" && (args[0] === "fetch" || args[0] === "push" || args[0] === "tag")) {
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        const plumbing = cutVersionCommitPlumbing(command, args);
        if (plumbing) return plumbing;
        return { exitCode: 1, stdout: "", stderr: `unexpected command ${key}` };
      }
    };
  }

  it("cuts the next series branch from origin/main", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const result = await cutReleaseBranch({ repoRoot, bump: "minor", env: {}, runner: cutRunner({}, calls) });

      // The branch tip is the version commit, not main's tip: cutting is when
      // the series version is set, and it is set by that commit.
      expect(result).toEqual({
        branch: "release/1.3",
        version: "1.3.0",
        commit: CUT_VERSION_COMMIT,
        trunkCommit: MAIN_SHA,
        trunkVersion: "1.2.3",
        abandoned: []
      });
      expect(
        calls.some((call) => call.command === "git" && call.args.join(" ") === `push origin ${CUT_VERSION_COMMIT}:refs/heads/release/1.3`)
      ).toBe(true);
      const composed = calls.find((call) => call.command === "git" && call.args[0] === "commit-tree");
      expect(composed?.args).toEqual(expect.arrayContaining(["-p", MAIN_SHA, "-m", "release: cut 1.3.0"]));
      // The version reaches the branch through a commit, never through the
      // caller's worktree: nothing is checked out and nothing is written.
      expect(calls.some((call) => call.command === "git" && ["checkout", "switch", "reset"].includes(call.args[0] ?? ""))).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("derives the series from origin/main VERSION, not the stale local worktree", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      // Local worktree VERSION is 1.2.3, but the branch is pushed at
      // origin/main whose VERSION is 1.4.7 — the series must follow the latter.
      const result = await cutReleaseBranch({
        repoRoot,
        bump: "minor",
        env: {},
        runner: cutRunner({ trunkVersion: "1.4.7" }, calls)
      });

      expect(result.branch).toBe("release/1.5");
      expect(result.version).toBe("1.5.0");
      expect(calls.some((call) => call.command === "git" && call.args.join(" ") === `push origin ${CUT_VERSION_COMMIT}:refs/heads/release/1.5`)).toBe(true);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("cuts in the same production-floored series as a bare main RC", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const result = await cutReleaseBranch({
        repoRoot,
        bump: "minor",
        env: {},
        runner: cutRunner({ trunkVersion: "0.0.68", productionTags: ["0.2.0"] }, calls)
      });

      expect(result.branch).toBe("release/0.3");
      expect(result.version).toBe("0.3.0");
      expect(
        calls.some(
          (call) =>
            call.command === "git" && call.args.join(" ") === `push origin ${CUT_VERSION_COMMIT}:refs/heads/release/0.3`
        )
      ).toBe(true);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses to cut a series branch that already exists", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = cutRunner({ releaseBranches: { "release/1.3": MAIN_SHA } }, calls);

      await expect(cutReleaseBranch({ repoRoot, bump: "minor", env: {}, runner })).rejects.toThrow(/release\/1\.3 already exists/);
      expect(calls.some((call) => call.command === "git" && call.args[0] === "push")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  // The recovery case: trunk still records 0.0.68, release/0.1 exists and its
  // RC is being abandoned rather than promoted, and the intended next series is
  // 0.2. Bump inference cannot express that — --minor would aim back at 0.1 —
  // and nothing here may promote 0.1.0, delete release/0.1, or hand-push a ref.
  const recoveryFixture: CutFixture = {
    trunkVersion: "0.0.68",
    releaseBranches: { "release/0.1": RELEASE_01_SHA },
    // The series being abandoned has shipped RCs — every real one has — and
    // those prereleases come back from the same ls-remote glob that looks for
    // production tags. Only a vX.Y.Z tag means the series actually released.
    stagingTags: ["0.1.0-staging.7", "0.1.0-staging.8"],
    activeStagingVersion: "0.1.0-staging.8",
    activeStagingSourceBranch: "release/0.1",
    channelBody: [
      "Pointer-only desktop staging updater channel.",
      "",
      "Lineage-Reset: 2026-08-13T08:00:00Z",
      `Reset-From: 0.1.0-staging.8 (${RELEASE_01_SHA}) source release/0.1`,
      "Reset-To: release/0.2",
      "Reset-Reason: abandoning the 0.1 series"
    ].join("\n")
  };

  it("cuts an explicitly named next series while abandoning the series it steps over", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const result = await cutReleaseBranch({
        repoRoot,
        bump: "minor",
        version: "0.2.0",
        abandonSeries: ["0.1"],
        reason: "0.1 diverged from main and will never ship",
        now: CUT_NOW,
        env: {},
        runner: cutRunner(recoveryFixture, calls)
      });

      expect(result).toEqual({
        branch: "release/0.2",
        version: "0.2.0",
        commit: CUT_VERSION_COMMIT,
        trunkCommit: MAIN_SHA,
        trunkVersion: "0.0.68",
        abandoned: [
          {
            series: "0.1",
            branch: "release/0.1",
            commit: RELEASE_01_SHA,
            tag: "abandoned/release/0.1",
            reason: "0.1 diverged from main and will never ship",
            abandonedAt: "2026-08-13T09:00:00.000Z",
            alreadyAbandoned: false
          }
        ]
      });

      const tagCall = calls.find((call) => call.command === "git" && call.args[0] === "tag");
      expect(tagCall?.args.slice(0, 5)).toEqual(["tag", "-f", "-a", "abandoned/release/0.1", RELEASE_01_SHA]);
      expect(tagCall?.args.at(-1)).toContain("Reason: 0.1 diverged from main and will never ship");
      const pushes = calls.filter((call) => call.command === "git" && call.args[0] === "push").map((call) => call.args.join(" "));
      // The abandonment record lands before the skip becomes real, the branch is
      // never deleted, and no v0.1.0 production tag is created to advance VERSION.
      expect(pushes).toEqual([
        "push origin refs/tags/abandoned/release/0.1",
        `push origin ${CUT_VERSION_COMMIT}:refs/heads/release/0.2`
      ]);
      expect(calls.some((call) => call.command === "git" && call.args.includes("--delete"))).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses to silently step over an unreleased series", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot } = createReleaseRepo(root);
      const calls: CommandCall[] = [];

      await expect(
        cutReleaseBranch({ repoRoot, bump: "minor", version: "0.2.0", env: {}, runner: cutRunner(recoveryFixture, calls) })
      ).rejects.toThrow(/--abandon-series 0\.1 --reason/);
      expect(calls.some((call) => call.command === "git" && call.args[0] === "push")).toBe(false);
      expect(calls.some((call) => call.command === "git" && call.args[0] === "tag")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("requires a reason before recording an abandonment", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot } = createReleaseRepo(root);
      const calls: CommandCall[] = [];

      await expect(
        cutReleaseBranch({
          repoRoot,
          bump: "minor",
          version: "0.2.0",
          abandonSeries: ["0.1"],
          env: {},
          runner: cutRunner(recoveryFixture, calls)
        })
      ).rejects.toThrow(/requires --reason/);
      expect(calls.some((call) => call.command === "git" && call.args[0] === "push")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("requires the staging channel to be released from the abandoned series first", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot } = createReleaseRepo(root);
      const calls: CommandCall[] = [];

      await expect(
        cutReleaseBranch({
          repoRoot,
          bump: "minor",
          version: "0.2.0",
          abandonSeries: ["0.1"],
          reason: "0.1 diverged",
          env: {},
          runner: cutRunner({ ...recoveryFixture, channelBody: "" }, calls)
        })
      ).rejects.toThrow(/kd release reset-staging --to release\/0\.2 .* --confirm-abandon 0\.1\.0-staging\.8/);
      expect(calls.some((call) => call.command === "git" && call.args[0] === "push")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses to abandon a series while the channel cannot be read", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          if (isStagingChannelAssetsQuery(command, args)) {
            calls.push({ command, args, options });
            return { exitCode: 1, stdout: "", stderr: "HTTP 503: Service unavailable" };
          }
          return cutRunner(recoveryFixture, calls).run(command, args, options);
        }
      };

      await expect(
        cutReleaseBranch({
          repoRoot,
          bump: "minor",
          version: "0.2.0",
          abandonSeries: ["0.1"],
          reason: "0.1 diverged",
          env: {},
          runner
        })
      ).rejects.toThrow(/Cannot tell whether desktop-staging still serves the series being abandoned/);
      expect(calls.some((call) => call.command === "git" && call.args[0] === "push")).toBe(false);
      expect(calls.some((call) => call.command === "git" && call.args[0] === "tag")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("does not require re-abandoning a series that already carries the record", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const result = await cutReleaseBranch({
        repoRoot,
        bump: "minor",
        version: "0.3.0",
        now: CUT_NOW,
        env: {},
        runner: cutRunner(
          {
            ...recoveryFixture,
            releaseBranches: { "release/0.1": RELEASE_01_SHA, "release/0.2": MAIN_SHA },
            abandonedSeries: {
              "0.1": "Abandoned release/0.1 at 2026-08-13T09:00:00.000Z\n\nReason: 0.1 diverged\n"
            },
            productionTags: ["0.2.0"],
            activeStagingVersion: null
          },
          calls
        )
      });

      expect(result.abandoned).toEqual([
        {
          series: "0.1",
          branch: "release/0.1",
          commit: RELEASE_01_SHA,
          tag: "abandoned/release/0.1",
          reason: "0.1 diverged",
          abandonedAt: "2026-08-13T09:00:00.000Z",
          alreadyAbandoned: true
        }
      ]);
      expect(calls.some((call) => call.command === "git" && call.args[0] === "tag")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("rejects an explicit target that is not a series start or is not ahead of trunk", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = cutRunner({ trunkVersion: "0.0.68" }, calls);

      await expect(cutReleaseBranch({ repoRoot, bump: "minor", version: "0.2.3", env: {}, runner })).rejects.toThrow(
        /A series cut starts at patch 0/
      );
      await expect(cutReleaseBranch({ repoRoot, bump: "minor", version: "0.0.0", env: {}, runner })).rejects.toThrow(
        /not ahead of origin\/main's VERSION/
      );
      await expect(
        cutReleaseBranch({ repoRoot, bump: "minor", version: "0.2.0", abandonSeries: ["9.9"], reason: "x", env: {}, runner })
      ).rejects.toThrow(/does not name a release branch this cut steps over/);
      expect(calls.some((call) => call.command === "git" && call.args[0] === "push")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("does not ask to abandon a series that already shipped a production release", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const result = await cutReleaseBranch({
        repoRoot,
        bump: "minor",
        version: "0.3.0",
        env: {},
        runner: cutRunner(
          {
            trunkVersion: "0.0.68",
            releaseBranches: { "release/0.1": RELEASE_01_SHA },
            productionTags: ["0.1.0"],
            activeStagingVersion: null
          },
          calls
        )
      });

      expect(result.abandoned).toEqual([]);
      expect(result.branch).toBe("release/0.3");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});

describe("release status", () => {
  const MAIN_COMMIT = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
  const PREVIOUS_RC_COMMIT = "9999999999999999999999999999999999999999";
  const NOW = Date.parse("2026-07-08T00:00:00Z");

  it("reports a recut migration as authorized provenance without hiding the git relationship", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-status-recut-"));
    try {
      const oldTip = PREVIOUS_RC_COMMIT;
      const before = await releaseStatus({
        repoRoot: root,
        env: {},
        now: NOW,
        runner: statusRunner({
          activeVersion: "0.3.0-staging.10",
          activeSourceBranch: "main",
          candidateTags: ["v0.3.0-staging.10"],
          releaseBranchSha: oldTip,
          recutTags: [],
          previousCommit: "8888888888888888888888888888888888888888",
          channelBody: "Pointer-only desktop staging updater channel."
        })
      });
      expect(before.staging).toMatchObject({ version: "0.3.0-staging.10", sourceBranch: "main" });
      expect(before.releaseBranch).toMatchObject({ commit: oldTip, recuts: [] });

      const body = [
        "Pointer-only desktop staging updater channel.",
        "",
        "Lineage-Recut: 2026-09-05T23:00:00.000Z",
        "Recut-Id: 0.3-1",
        "Recut-Series: 0.3",
        "Recut-Branch: release/0.3",
        `Recut-Old-Tip: ${oldTip}`,
        `Recut-New-Tip: ${MAIN_COMMIT}`,
        "Recut-Archive-Tag: recut/release/0.3-1",
        `Recut-From: 0.3.0-staging.10 (${oldTip}) source main`,
        "Recut-Prior-Epoch: 0.3.0-staging.10",
        "Recut-Requester: migration-test",
        "Recut-Reason: include the latest feature"
      ].join("\n");
      const result = await releaseStatus({
        repoRoot: root,
        env: {},
        now: NOW,
        runner: statusRunner({
          activeVersion: "0.3.0-staging.11",
          activeSourceBranch: "release/0.3",
          candidateTags: ["v0.3.0-staging.11", "v0.3.0-staging.10"],
          releaseBranchSha: MAIN_COMMIT,
          recutTags: ["recut/release/0.3-1"],
          previousCommit: oldTip,
          channelBody: body
        })
      });
      expect(parseLineageRecutRecord(body)?.recutId).toBe("0.3-1");
      expect(result.lineage?.recut?.recutId).toBe("0.3-1");
      expect(result.lineage).toMatchObject({ relationship: "descendant", valid: true, authorizedByRecut: true });
      expect(result.releaseBranch?.recuts).toEqual([{
        id: "0.3-1",
        archiveTag: "recut/release/0.3-1",
        status: "pending",
        oldTip,
        newTip: MAIN_COMMIT
      }]);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("reports a later release-branch tip descended from a recut as authorized", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-status-recut-descendant-"));
    try {
      const oldTip = PREVIOUS_RC_COMMIT;
      const recutTip = MAIN_COMMIT;
      const laterBranchTip = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
      const body = [
        "Pointer-only desktop staging updater channel.",
        "",
        "Lineage-Recut: 2026-09-05T23:00:00.000Z",
        "Recut-Id: 0.3-1",
        "Recut-Series: 0.3",
        "Recut-Branch: release/0.3",
        `Recut-Old-Tip: ${oldTip}`,
        `Recut-New-Tip: ${recutTip}`,
        "Recut-Archive-Tag: recut/release/0.3-1",
        `Recut-From: 0.3.0-staging.10 (${oldTip}) source main`,
        "Recut-Prior-Epoch: 0.3.0-staging.10",
        "Recut-Requester: status-test",
        "Recut-Reason: include the latest feature"
      ].join("\n");
      const result = await releaseStatus({
        repoRoot: root,
        env: {},
        now: NOW,
        runner: statusRunner({
          activeVersion: "0.3.0-staging.11",
          activeCommit: laterBranchTip,
          activeSourceBranch: "release/0.3",
          candidateTags: ["v0.3.0-staging.11", "v0.3.0-staging.10"],
          previousCommit: oldTip,
          previousIsAncestor: 1,
          activeIsAncestor: 1,
          releaseBranchSha: laterBranchTip,
          recutTags: ["recut/release/0.3-1"],
          recutNewTip: recutTip,
          recutNewTipIsAncestor: 0,
          channelBody: body
        })
      });
      expect(result.lineage).toMatchObject({
        relationship: "diverged",
        valid: true,
        authorizedByRecut: true,
        recut: { recutId: "0.3-1" }
      });
      expect(result.releaseBranch?.recuts).toEqual([{
        id: "0.3-1",
        archiveTag: "recut/release/0.3-1",
        status: "pending",
        oldTip,
        newTip: recutTip
      }]);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("keeps a recut candidate promotable after its application is recorded", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-status-recut-applied-"));
    try {
      const oldTip = PREVIOUS_RC_COMMIT;
      const recutTip = MAIN_COMMIT;
      const candidateCommit = "beef000000000000000000000000000000000000";
      const body = [
        "Pointer-only desktop staging updater channel.",
        "",
        "Lineage-Recut: 2026-09-05T23:00:00.000Z",
        "Recut-Id: 0.3-1",
        "Recut-Series: 0.3",
        "Recut-Branch: release/0.3",
        `Recut-Old-Tip: ${oldTip}`,
        `Recut-New-Tip: ${recutTip}`,
        "Recut-Archive-Tag: recut/release/0.3-1",
        `Recut-From: 0.3.0-staging.7 (${oldTip}) source release/0.3`,
        "Recut-Prior-Epoch: 0.3.0-staging.7",
        "Recut-Requester: promotion-test",
        "Recut-Reason: include the latest feature",
        "",
        "Lineage-Recut-Applied: 2026-09-06T00:00:00.000Z",
        "Recut-Applied-Id: 0.3-1",
        "Recut-Applied-Version: 0.3.0-staging.8",
        `Recut-Applied-Commit: ${candidateCommit}`,
        "Recut-Applied-Tag: recut-applied/0.3-1"
      ].join("\n");
      const result = await releaseStatus({
        repoRoot: root,
        env: {},
        now: NOW,
        runner: statusRunner({
          activeVersion: "0.3.0-staging.8",
          activeCommit: candidateCommit,
          activeSourceBranch: "release/0.3",
          candidateTags: ["v0.3.0-staging.8", "v0.3.0-staging.7"],
          previousCommit: oldTip,
          previousIsAncestor: 1,
          activeIsAncestor: 1,
          releaseBranchSha: candidateCommit,
          recutTags: ["recut/release/0.3-1"],
          recutNewTip: recutTip,
          recutNewTipIsAncestor: 0,
          productionTag: "v0.2.0",
          channelBody: body
        })
      });

      expect(result.lineage).toMatchObject({
        relationship: "diverged",
        valid: true,
        authorizedByRecut: true
      });
      expect(result.releaseBranch?.recuts).toEqual([{
        id: "0.3-1",
        archiveTag: "recut/release/0.3-1",
        status: "applied",
        oldTip,
        newTip: recutTip
      }]);
      expect(result.promotion.allowed).toBe(true);
      expect(result.promotion.blockers).toEqual([]);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("keeps applied recut A eligible after the branch advances to B and later audit blocks are recorded", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-status-historical-recut-"));
    try {
      const oldTip = PREVIOUS_RC_COMMIT;
      const candidateA = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
      const candidateB = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
      const body = [
        "Pointer-only desktop staging updater channel.",
        "",
        "Lineage-Recut-Applied: 2026-09-07T00:00:00.000Z",
        "Recut-Applied-Id: 0.3-2",
        "Recut-Applied-Version: 0.3.0-staging.9",
        `Recut-Applied-Commit: ${candidateB}`,
        "Recut-Applied-Tag: recut-applied/0.3-2",
        "",
        "Lineage-Recut: 2026-09-06T23:00:00.000Z",
        "Recut-Id: 0.3-2",
        "Recut-Series: 0.3",
        "Recut-Branch: release/0.3",
        `Recut-Old-Tip: ${candidateA}`,
        `Recut-New-Tip: ${candidateB}`,
        "Recut-Archive-Tag: recut/release/0.3-2",
        `Recut-From: 0.3.0-staging.8 (${candidateA}) source release/0.3`,
        "Recut-Prior-Epoch: 0.3.0-staging.8",
        "Recut-Requester: later-test",
        "Recut-Reason: advance the train again",
        "",
        "Lineage-Recut-Applied: 2026-09-06T00:00:00.000Z",
        "Recut-Applied-Id: 0.3-1",
        "Recut-Applied-Version: 0.3.0-staging.8",
        `Recut-Applied-Commit: ${candidateA}`,
        "Recut-Applied-Tag: recut-applied/0.3-1",
        "",
        "Lineage-Recut: 2026-09-05T23:00:00.000Z",
        "Recut-Id: 0.3-1",
        "Recut-Series: 0.3",
        "Recut-Branch: release/0.3",
        `Recut-Old-Tip: ${oldTip}`,
        `Recut-New-Tip: ${candidateA}`,
        "Recut-Archive-Tag: recut/release/0.3-1",
        `Recut-From: 0.3.0-staging.7 (${oldTip}) source release/0.3`,
        "Recut-Prior-Epoch: 0.3.0-staging.7",
        "Recut-Requester: promotion-test",
        "Recut-Reason: authorize candidate A"
      ].join("\n");
      const result = await releaseStatus({
        repoRoot: root,
        env: {},
        now: NOW,
        candidateVersion: "0.3.0-staging.8",
        runner: statusRunner({
          activeVersion: "0.3.0-staging.9",
          activeCommit: candidateB,
          activeSourceBranch: "release/0.3",
          candidateTags: ["v0.3.0-staging.9", "v0.3.0-staging.8", "v0.3.0-staging.7"],
          previousCommit: oldTip,
          previousIsAncestor: 1,
          activeIsAncestor: 1,
          releaseBranchSha: candidateB,
          productionTag: "v0.2.0",
          channelBody: body,
          historicalCandidates: {
            "0.3.0-staging.8": {
              commit: candidateA,
              sourceBranch: "release/0.3",
              publishedAt: new Date(NOW - 48 * 3_600_000).toISOString()
            }
          }
        })
      });

      expect(result.lineage).toMatchObject({
        relationship: "diverged",
        valid: true,
        authorizedByRecut: true,
        recut: { recutId: "0.3-1" }
      });
      expect(result.promotion).toMatchObject({ allowed: true, base: candidateA });
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it.each([
    { label: "rejects divergence", previousIsAncestor: 1, relationship: "diverged", allowed: false },
    { label: "allows descendant progression", previousIsAncestor: 0, relationship: "descendant", allowed: true }
  ])("checks the actual predecessor after a recut grant is consumed and $label", async ({ previousIsAncestor, relationship, allowed }) => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-status-consumed-recut-"));
    try {
      const oldTip = PREVIOUS_RC_COMMIT;
      const recutTip = "cccccccccccccccccccccccccccccccccccccccc";
      const appliedCommit = "dddddddddddddddddddddddddddddddddddddddd";
      const currentCommit = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
      const body = [
        "Pointer-only desktop staging updater channel.",
        "",
        "Lineage-Recut-Applied: 2026-09-06T00:00:00.000Z",
        "Recut-Applied-Id: 0.3-1",
        "Recut-Applied-Version: 0.3.0-staging.2",
        `Recut-Applied-Commit: ${appliedCommit}`,
        "Recut-Applied-Tag: recut-applied/0.3-1",
        "",
        "Lineage-Recut: 2026-09-05T23:00:00.000Z",
        "Recut-Id: 0.3-1",
        "Recut-Series: 0.3",
        "Recut-Branch: release/0.3",
        `Recut-Old-Tip: ${oldTip}`,
        `Recut-New-Tip: ${recutTip}`,
        "Recut-Archive-Tag: recut/release/0.3-1",
        `Recut-From: 0.3.0-staging.1 (${oldTip}) source release/0.3`,
        "Recut-Prior-Epoch: 0.3.0-staging.1",
        "Recut-Requester: consumed-grant-test",
        "Recut-Reason: authorize staging.2 only"
      ].join("\n");
      const result = await releaseStatus({
        repoRoot: root,
        env: {},
        now: NOW,
        runner: statusRunner({
          activeVersion: "0.3.0-staging.3",
          activeCommit: currentCommit,
          activeSourceBranch: "release/0.3",
          candidateTags: ["v0.3.0-staging.3", "v0.3.0-staging.2", "v0.3.0-staging.1"],
          previousCommit: appliedCommit,
          previousIsAncestor,
          activeIsAncestor: 1,
          releaseBranchSha: currentCommit,
          productionTag: "v0.2.0",
          channelBody: body
        })
      });

      expect(result.lineage).toMatchObject({
        relationship,
        previous: { version: "0.3.0-staging.2", commit: appliedCommit },
        valid: allowed,
        authorizedByRecut: false
      });
      expect(result.promotion.allowed).toBe(allowed);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  interface StatusFixture {
    activeVersion: string | null;
    activeCommit?: string | null;
    activeSourceBranch?: string | null;
    activePublishedAt?: string | null;
    activeTagName?: string;
    activeIsPrerelease?: boolean;
    activeTagCommit?: string;
    productionTag?: string | null;
    productionPublishedAt?: string;
    /** Ordered newest-first list of staging prereleases on the repo. */
    candidateTags?: string[];
    /** Complete paginated release history when the first 100-entry page fills. */
    allCandidateTags?: string[];
    paginatedHistoryError?: string;
    paginatedHistoryRaw?: string;
    previousCommit?: string | null;
    /** Result of `git merge-base --is-ancestor <previous> <active>`. */
    previousIsAncestor?: number;
    /** Result of `git merge-base --is-ancestor <active> <previous>`. */
    activeIsAncestor?: number;
    releaseBranchSha?: string | null;
    /** Production tags that exist on origin, without the leading v. */
    existingProductionTags?: string[];
    channelBody?: string;
    /** Simulates a transient GitHub failure reading the channel. */
    channelUnreadable?: boolean;
    /** Raw latest-staging.json contents, for malformed-manifest cases. */
    manifestBody?: string;
    /** Raw manifest on the immutable versioned prerelease. */
    versionedManifestBody?: string;
    abandonedSeries?: Record<string, string>;
    recutTags?: string[];
    recutNewTip?: string;
    recutNewTipIsAncestor?: number;
    cherry?: string;
    behindMain?: number;
    commitsSinceProduction?: number;
    historicalCandidates?: Record<string, {
      commit: string;
      sourceBranch: string;
      publishedAt: string;
      releasePolicy?: string | null;
    }>;
    /** Policy file contents at the active candidate commit; null means absent. */
    releasePolicy?: string | null;
  }

  function statusRunner(fixture: StatusFixture, calls: CommandCall[] = []): CommandRunner {
    const activeCommit = fixture.activeCommit === undefined ? MAIN_COMMIT : fixture.activeCommit;
    const previousCommit = fixture.previousCommit === undefined ? PREVIOUS_RC_COMMIT : fixture.previousCommit;
    const candidateTags = fixture.candidateTags ?? (fixture.activeVersion
      ? [`v${fixture.activeVersion}`, "v0.0.0-staging.1"]
      : []);
    let fetchedCandidateCommit = activeCommit;
    return {
      async run(command, args, options) {
        calls.push({ command, args, options });
        const key = `${command} ${args.join(" ")}`;
        if (key === "git remote get-url origin") {
          return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
        }
        if (key === "git fetch --tags origin main" || key === "git fetch --tags origin") {
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (key === "git rev-parse origin/main") {
          return { exitCode: 0, stdout: `${MAIN_COMMIT}\n`, stderr: "" };
        }
        if (key === "gh release view --repo jemdiggity/kanna --json tagName,publishedAt") {
          if (!fixture.productionTag) return { exitCode: 1, stdout: "", stderr: "no releases" };
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              tagName: fixture.productionTag,
              publishedAt: fixture.productionPublishedAt ?? "2026-07-01T00:00:00Z"
            }),
            stderr: ""
          };
        }
        if (isStagingChannelAssetsQuery(command, args)) {
          if (fixture.channelUnreadable) return { exitCode: 1, stdout: "", stderr: "HTTP 503: Service unavailable" };
          return stagingChannelAssetsResponse(fixture.activeVersion ? ["latest-staging.json"] : null);
        }
        if (command === "gh" && args[0] === "release" && args[1] === "download") {
          if (!fixture.activeVersion) return { exitCode: 1, stdout: "", stderr: "release not found" };
          const dirIndex = args.indexOf("--dir");
          const selectedVersion = (args[2] ?? "").replace(/^v/, "");
          const manifestBody = args[2] === "desktop-staging"
            ? fixture.manifestBody
            : fixture.versionedManifestBody ?? `{"version":"${selectedVersion}"}\n`;
          writeFileSync(
            join(args[dirIndex + 1] ?? "", "latest-staging.json"),
            manifestBody ?? `{"version":"${fixture.activeVersion}"}\n`
          );
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (fixture.activeVersion && key.startsWith(`gh release view v${fixture.activeVersion} `)) {
          if (activeCommit === null) return { exitCode: 1, stdout: "", stderr: "release not found" };
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              tagName: fixture.activeTagName ?? `v${fixture.activeVersion}`,
              targetCommitish: activeCommit,
              publishedAt: fixture.activePublishedAt === undefined ? "2026-07-06T00:00:00Z" : fixture.activePublishedAt,
              body: fixture.activeSourceBranch
                ? `Staging updater manifest for v${fixture.activeVersion}\n\nSource-Branch: ${fixture.activeSourceBranch}`
                : `Staging updater manifest for v${fixture.activeVersion}`,
              isPrerelease: fixture.activeIsPrerelease ?? true
            }),
            stderr: ""
          };
        }
        if (command === "gh" && args[0] === "release" && args[1] === "view") {
          const selectedVersion = (args[2] ?? "").replace(/^v/, "");
          const historical = fixture.historicalCandidates?.[selectedVersion];
          if (historical) {
            return {
              exitCode: 0,
              stdout: JSON.stringify({
                tagName: `v${selectedVersion}`,
                targetCommitish: historical.commit,
                publishedAt: historical.publishedAt,
                body: `Staging updater manifest for v${selectedVersion}\n\nSource-Branch: ${historical.sourceBranch}`,
                isPrerelease: true
              }),
              stderr: ""
            };
          }
        }
        if (isProductionReleaseListQuery(command, args)) {
          const versions = fixture.existingProductionTags ?? [];
          return {
            exitCode: 0,
            stdout: JSON.stringify(versions.map((version) => ({ tagName: `v${version}`, isPrerelease: false }))),
            stderr: ""
          };
        }
        if (command === "gh" && args[0] === "release" && args[1] === "list") {
          return {
            exitCode: 0,
            stdout: JSON.stringify(
              candidateTags.map((tag, index) => ({
                tagName: tag,
                createdAt: new Date(NOW - (index + 1) * 86_400_000).toISOString()
              }))
            ),
            stderr: ""
          };
        }
        if (command === "gh" && args[0] === "api" && args.includes("--paginate")) {
          if (fixture.paginatedHistoryError) {
            return { exitCode: 1, stdout: "", stderr: fixture.paginatedHistoryError };
          }
          if (fixture.paginatedHistoryRaw !== undefined) {
            return { exitCode: 0, stdout: fixture.paginatedHistoryRaw, stderr: "" };
          }
          const tags = fixture.allCandidateTags ?? candidateTags;
          return {
            exitCode: 0,
            stdout: JSON.stringify([tags.map((tag, index) => ({
              tag_name: tag,
              created_at: new Date(NOW - (index + 1) * 86_400_000).toISOString()
            }))]),
            stderr: ""
          };
        }
        if (command === "gh" && args[0] === "release" && args[1] === "view" && args[2] === "desktop-staging") {
          return { exitCode: 0, stdout: JSON.stringify({ body: fixture.channelBody ?? "" }), stderr: "" };
        }
        if (command === "gh" && args[0] === "release" && args[1] === "view") {
          if (previousCommit === null) return { exitCode: 1, stdout: "", stderr: "release not found" };
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              tagName: args[2],
              targetCommitish: previousCommit,
              body: `Staging updater manifest for ${args[2]}\n\nSource-Branch: main`,
              publishedAt: "2026-07-02T00:00:00Z",
              isPrerelease: true
            }),
            stderr: ""
          };
        }
        if (command === "git" && args[0] === "merge-base") {
          const [, , base, candidate] = args;
          const historicalCommits = Object.values(fixture.historicalCandidates ?? {}).map((entry) => entry.commit);
          if (base === fixture.recutNewTip && candidate === activeCommit) {
            return { exitCode: fixture.recutNewTipIsAncestor ?? 1, stdout: "", stderr: "" };
          }
          if (base === previousCommit && candidate === activeCommit) {
            return { exitCode: fixture.previousIsAncestor ?? 0, stdout: "", stderr: "" };
          }
          if (base === previousCommit && historicalCommits.includes(candidate ?? "")) {
            return { exitCode: fixture.previousIsAncestor ?? 0, stdout: "", stderr: "" };
          }
          if (base === activeCommit && candidate === previousCommit) {
            return { exitCode: fixture.activeIsAncestor ?? 1, stdout: "", stderr: "" };
          }
          if (historicalCommits.includes(base ?? "") && candidate === previousCommit) {
            return { exitCode: fixture.activeIsAncestor ?? 1, stdout: "", stderr: "" };
          }
          return { exitCode: 1, stdout: "", stderr: "" };
        }
        if (command === "git" && args[0] === "ls-remote" && args[1] === "origin") {
          const sha = fixture.releaseBranchSha ?? "";
          return { exitCode: 0, stdout: sha ? `${sha}\t${args[2]}\n` : "", stderr: "" };
        }
        if (command === "git" && args[0] === "ls-remote" && args[1] === "--tags") {
          const pattern = args[3] ?? "";
          if (pattern.startsWith("recut/release/")) {
            return {
              exitCode: 0,
              stdout: (fixture.recutTags ?? []).map((tag) => `sha\trefs/tags/${tag}\n`).join(""),
              stderr: ""
            };
          }
          if (fixture.activeVersion && pattern === `refs/tags/v${fixture.activeVersion}`) {
            const tagCommit = fixture.activeTagCommit ?? activeCommit ?? "";
            return {
              exitCode: 0,
              stdout: tagCommit ? `${tagCommit}\t${pattern}\n` : "",
              stderr: ""
            };
          }
          const historicalVersion = pattern.replace(/^refs\/tags\/v/, "");
          const historical = fixture.historicalCandidates?.[historicalVersion];
          if (historical) {
            return { exitCode: 0, stdout: `${historical.commit}\t${pattern}\n`, stderr: "" };
          }
          const abandoned = /^refs\/tags\/abandoned\/release\/(\d+\.\d+)$/.exec(pattern);
          if (abandoned) {
            const has = Boolean(fixture.abandonedSeries?.[abandoned[1] ?? ""]);
            return { exitCode: 0, stdout: has ? `sha\t${pattern}\n` : "", stderr: "" };
          }
          const wanted = pattern.replace(/^v/, "");
          const exists = (fixture.existingProductionTags ?? []).includes(wanted);
          return { exitCode: 0, stdout: exists ? `sha\trefs/tags/v${wanted}\n` : "", stderr: "" };
        }
        if (command === "git" && args[0] === "for-each-ref") {
          const series = /abandoned\/release\/(\d+\.\d+)$/.exec(args[2] ?? "")?.[1] ?? "";
          return { exitCode: 0, stdout: fixture.abandonedSeries?.[series] ?? "", stderr: "" };
        }
        if (command === "git" && args[0] === "fetch") {
          const selectedTag = (args.at(-1) ?? "").replace(/^refs\/tags\/v/, "");
          fetchedCandidateCommit = fixture.historicalCandidates?.[selectedTag]?.commit ?? activeCommit;
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (key === "git rev-parse FETCH_HEAD^{commit}") {
          return { exitCode: 0, stdout: `${fixture.activeTagCommit ?? fetchedCandidateCommit ?? ""}\n`, stderr: "" };
        }
        if (command === "git" && ["cat-file", "show"].includes(args[0] ?? "") && args.at(-1)?.endsWith(":release-policy.json")) {
          const revision = args.at(-1) ?? "";
          const commit = revision.slice(0, -":release-policy.json".length);
          const historical = Object.values(fixture.historicalCandidates ?? {}).find((entry) => entry.commit === commit);
          const policy = historical ? historical.releasePolicy : fixture.releasePolicy;
          if (policy === undefined || policy === null) return { exitCode: 1, stdout: "", stderr: "path does not exist" };
          return { exitCode: 0, stdout: args[0] === "show" ? policy : "", stderr: "" };
        }
        if (command === "git" && args[0] === "log") {
          expect(args).toContain("--no-merges");
          expect(args).toContain("--cherry-pick");
          expect(args).toContain("--right-only");
          return { exitCode: 0, stdout: fixture.cherry ?? "", stderr: "" };
        }
        if (command === "git" && args[0] === "rev-list") {
          const range = args[2] ?? "";
          if (range.startsWith("v")) return { exitCode: 0, stdout: `${fixture.commitsSinceProduction ?? 12}\n`, stderr: "" };
          return { exitCode: 0, stdout: `${fixture.behindMain ?? 0}\n`, stderr: "" };
        }
        return { exitCode: 1, stdout: "", stderr: `unexpected command ${key}` };
      }
    };
  }

  it("reports lineage, soak, and promotion state for a healthy main candidate", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const runner = statusRunner({
        activeVersion: "1.2.4-staging.3",
        activeSourceBranch: "main",
        productionTag: "v1.2.3"
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.production).toEqual({ version: "1.2.3", tag: "v1.2.3", publishedAt: "2026-07-01T00:00:00Z" });
      expect(result.staging).toEqual({
        version: "1.2.4-staging.3",
        tag: "v1.2.4-staging.3",
        commit: MAIN_COMMIT,
        sourceBranch: "main",
        commitsBehindMain: 0,
        publishedAt: "2026-07-06T00:00:00Z",
        ageHours: 48
      });
      expect(result.policy.productionSoakHours).toBe(24);
      expect(result.lineage?.relationship).toBe("descendant");
      expect(result.lineage?.previous).toEqual({
        version: "0.0.0-staging.1",
        tag: "v0.0.0-staging.1",
        commit: PREVIOUS_RC_COMMIT
      });
      expect(result.lineage?.valid).toBe(true);
      expect(result.freeze).toEqual({ active: false, branch: null, reason: null, waivedByReset: false });
      expect(result.promotion.mechanicallyPromotable).toBe(true);
      expect(result.promotion.base).toBe(MAIN_COMMIT);
      expect(result.promotion.soak).toMatchObject({ requiredHours: 24, elapsedHours: 48, satisfied: true, overridden: false });
      expect(result.promotion.allowed).toBe(true);
      expect(result.promotion.blockers).toEqual([]);
      expect(result.promoteCommand).toBe("kd release promote 1.2.4-staging.3");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("assesses a soaked historical RC while the live newer RC keeps its own soak", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-historical-status-"));
    try {
      const candidateA = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
      const candidateB = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
      const fixture: StatusFixture = {
        activeVersion: "1.2.4-staging.22",
        activeCommit: candidateB,
        activeSourceBranch: "main",
        activePublishedAt: new Date(NOW - 3 * 3_600_000).toISOString(),
        candidateTags: ["v1.2.4-staging.22", "v1.2.4-staging.21", "v1.2.4-staging.20"],
        previousCommit: PREVIOUS_RC_COMMIT,
        productionTag: "v1.2.3",
        existingProductionTags: ["1.2.3"],
        behindMain: 4,
        historicalCandidates: {
          "1.2.4-staging.21": {
            commit: candidateA,
            sourceBranch: "main",
            publishedAt: new Date(NOW - 48 * 3_600_000).toISOString()
          }
        }
      };

      const selected = await releaseStatus({
        repoRoot: root,
        env: {},
        runner: statusRunner(fixture),
        now: NOW,
        candidateVersion: "1.2.4-staging.21"
      });
      expect(selected.staging).toMatchObject({ version: "1.2.4-staging.22", commit: candidateB });
      expect(selected.promotion.candidate).toMatchObject({ version: "1.2.4-staging.21", commit: candidateA });
      expect(selected.promotion).toMatchObject({
        base: candidateA,
        allowed: true,
        soak: { elapsedHours: 48, satisfied: true }
      });
      expect(selected.promoteCommand).toBe("kd release promote 1.2.4-staging.21");

      const live = await releaseStatus({ repoRoot: root, env: {}, runner: statusRunner(fixture), now: NOW });
      expect(live.promotion.candidate?.version).toBe("1.2.4-staging.22");
      expect(live.promotion.soak).toMatchObject({ elapsedHours: 3, satisfied: false });
      expect(live.promotion.allowed).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("finds a retained historical candidate and its predecessor beyond the first 100 releases", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-paginated-history-"));
    try {
      const firstPage = Array.from({ length: 100 }, (_, index) => `v1.2.4-staging.${122 - index}`);
      const allCandidateTags = [...firstPage, "v1.2.4-staging.22", "v1.2.4-staging.21"];
      const candidateA = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
      const calls: CommandCall[] = [];
      const result = await releaseStatus({
        repoRoot: root,
        env: {},
        now: NOW,
        candidateVersion: "1.2.4-staging.22",
        runner: statusRunner({
          activeVersion: "1.2.4-staging.122",
          activeCommit: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
          activeSourceBranch: "main",
          candidateTags: firstPage,
          allCandidateTags,
          previousCommit: PREVIOUS_RC_COMMIT,
          productionTag: "v1.2.3",
          existingProductionTags: ["1.2.3"],
          historicalCandidates: {
            "1.2.4-staging.22": {
              commit: candidateA,
              sourceBranch: "main",
              publishedAt: new Date(NOW - 48 * 3_600_000).toISOString()
            }
          }
        }, calls)
      });

      expect(result.lineage).toMatchObject({
        relationship: "descendant",
        previous: { version: "1.2.4-staging.21", tag: "v1.2.4-staging.21" },
        valid: true
      });
      expect(result.promotion).toMatchObject({ allowed: true, base: candidateA });
      expect(calls).toContainEqual(expect.objectContaining({
        command: "gh",
        args: ["api", "--paginate", "--slurp", "repos/jemdiggity/kanna/releases?per_page=100"]
      }));
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it.each([
    { label: "the paginated request fails", paginatedHistoryError: "HTTP 503: Service unavailable" },
    { label: "the paginated response is malformed", paginatedHistoryRaw: '{"message":"not a page list"}' }
  ])("fails status closed when the first release page is full and $label", async (pagination) => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-incomplete-history-status-"));
    try {
      const firstPage = Array.from({ length: 100 }, (_, index) => `v1.2.4-staging.${101 - index}`);
      await expect(releaseStatus({
        repoRoot: root,
        env: {},
        now: NOW,
        runner: statusRunner({
          activeVersion: "1.2.4-staging.2",
          activeSourceBranch: "main",
          candidateTags: firstPage,
          productionTag: "v1.2.3",
          existingProductionTags: ["1.2.3"],
          ...pagination
        })
      })).rejects.toThrow(/complete GitHub release history|complete staging release history/i);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it.each([
    { checkoutHours: 1, candidateHours: 24, allowed: false },
    { checkoutHours: 24, candidateHours: 1, allowed: true }
  ])("uses historical candidate A's $candidateHours-hour policy when checkout policy is $checkoutHours hours", async ({ checkoutHours, candidateHours, allowed }) => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-historical-policy-"));
    try {
      writeFileSync(join(root, "release-policy.json"), JSON.stringify({ productionSoakHours: checkoutHours }));
      const candidateA = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
      const result = await releaseStatus({
        repoRoot: root,
        env: {},
        now: NOW,
        candidateVersion: "1.2.4-staging.21",
        runner: statusRunner({
          activeVersion: "1.2.4-staging.22",
          activeCommit: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
          activeSourceBranch: "main",
          activePublishedAt: new Date(NOW - 1 * 3_600_000).toISOString(),
          candidateTags: ["v1.2.4-staging.22", "v1.2.4-staging.21", "v1.2.4-staging.20"],
          previousCommit: PREVIOUS_RC_COMMIT,
          productionTag: "v1.2.3",
          existingProductionTags: ["1.2.3"],
          historicalCandidates: {
            "1.2.4-staging.21": {
              commit: candidateA,
              sourceBranch: "main",
              publishedAt: new Date(NOW - 3 * 3_600_000).toISOString(),
              releasePolicy: JSON.stringify({ productionSoakHours: candidateHours })
            }
          }
        })
      });

      expect(result.policy.productionSoakHours).toBe(candidateHours);
      expect(result.promotion.soak).toMatchObject({ requiredHours: candidateHours, elapsedHours: 3, satisfied: allowed });
      expect(result.promotion.allowed).toBe(allowed);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("reports immutable tag identity failures as promotion blockers", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const runner = statusRunner({
        activeVersion: "1.2.4-staging.3",
        activeSourceBranch: "main",
        activeTagCommit: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        productionTag: "v1.2.3"
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.promotion.mechanicallyPromotable).toBe(false);
      expect(result.promotion.base).toBeNull();
      expect(result.promotion.allowed).toBe(false);
      expect(result.promotion.blockers.join(" ")).toMatch(/failed immutable identity verification.*tag resolves to/);
      expect(result.promoteCommand).toBeNull();
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("reports a versioned manifest mismatch as a promotion blocker", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const runner = statusRunner({
        activeVersion: "1.2.4-staging.3",
        activeSourceBranch: "main",
        versionedManifestBody: '{"version":"1.2.4-staging.30"}\n',
        productionTag: "v1.2.3"
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.promotion.mechanicallyPromotable).toBe(false);
      expect(result.promotion.base).toBeNull();
      expect(result.promotion.allowed).toBe(false);
      expect(result.promotion.blockers.join(" ")).toMatch(
        /latest-staging\.json version 1\.2\.4-staging\.30 does not match selected version 1\.2\.4-staging\.3/
      );
      expect(result.promoteCommand).toBeNull();
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  // The v0.1.0-staging.7 -> .8 incident: .8 was mechanically aligned to
  // release/0.1's tip while its history had diverged from .7. Status must not
  // collapse that into one "promotable" flag.
  it("separates mechanical promotability from a diverged lineage", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const divergedCommit = "beef000000000000000000000000000000000000";
      const runner = statusRunner({
        activeVersion: "0.1.0-staging.8",
        activeCommit: divergedCommit,
        activeSourceBranch: "release/0.1",
        candidateTags: ["v0.1.0-staging.8", "v0.1.0-staging.7"],
        previousIsAncestor: 1,
        activeIsAncestor: 1,
        releaseBranchSha: divergedCommit,
        productionTag: "v0.0.9"
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.promotion.mechanicallyPromotable).toBe(true);
      expect(result.promotion.base).toBe(divergedCommit);
      expect(result.lineage?.relationship).toBe("diverged");
      expect(result.lineage?.previous?.tag).toBe("v0.1.0-staging.7");
      expect(result.lineage?.valid).toBe(false);
      expect(result.lineage?.authorizedByReset).toBe(false);
      expect(result.promotion.allowed).toBe(false);
      expect(result.promotion.blockers.join(" ")).toMatch(/share only an older merge base/);
      expect(result.promoteCommand).toBeNull();
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("treats a divergence as valid when a recorded reset authorized it", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const divergedCommit = "beef000000000000000000000000000000000000";
      const runner = statusRunner({
        activeVersion: "0.1.0-staging.8",
        activeCommit: divergedCommit,
        activeSourceBranch: "release/0.1",
        candidateTags: ["v0.1.0-staging.8", "v0.1.0-staging.7"],
        previousIsAncestor: 1,
        activeIsAncestor: 1,
        releaseBranchSha: divergedCommit,
        productionTag: "v0.0.9",
        channelBody: [
          "Pointer-only desktop staging updater channel.",
          "",
          "Lineage-Reset: 2026-07-04T00:00:00Z",
          `Reset-From: 0.1.0-staging.7 (${PREVIOUS_RC_COMMIT}) source main`,
          "Reset-To: release/0.1",
          "Reset-Reason: hotfix the 0.1 series from its stale branch"
        ].join("\n")
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.lineage?.relationship).toBe("diverged");
      expect(result.lineage?.valid).toBe(true);
      expect(result.lineage?.authorizedByReset).toBe(true);
      expect(result.lineage?.reset?.reason).toBe("hotfix the 0.1 series from its stale branch");
      expect(result.promotion.allowed).toBe(true);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("reports a recorded post-promotion trunk resumption as valid", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const runner = statusRunner({
        activeVersion: "1.4.0-staging.1",
        activeCommit: MAIN_COMMIT,
        activeSourceBranch: "main",
        candidateTags: ["v1.4.0-staging.1", "v1.3.0-staging.2"],
        previousCommit: PREVIOUS_RC_COMMIT,
        previousIsAncestor: 1,
        activeIsAncestor: 1,
        productionTag: "v1.3.0",
        channelBody: [
          "Pointer-only desktop staging updater channel.",
          "",
          "Post-Promotion-Trunk-Resumption: 2026-08-17T03:00:00.000Z",
          "Promoted-Version: 1.3.0",
          "Promoted-Tag: v1.3.0",
          `Promoted-Commit: ${PREVIOUS_RC_COMMIT}`,
          `Production-Tag-Commit: ${PREVIOUS_RC_COMMIT}`,
          `Resumed-To: ${MAIN_COMMIT} source main`
        ].join("\n")
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.lineage).toMatchObject({
        relationship: "diverged",
        valid: true,
        authorizedByReset: false,
        authorizedByPromotion: true,
        postPromotion: {
          promotedVersion: "1.3.0",
          promotedTag: "v1.3.0",
          promotedCommit: PREVIOUS_RC_COMMIT,
          productionTagCommit: PREVIOUS_RC_COMMIT,
          newCommit: MAIN_COMMIT,
          newBranch: "main"
        }
      });
      expect(result.lineage?.detail).toContain("resumed trunk");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("reports that an unpromoted release-branch candidate does not freeze the macOS train", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const rcCommit = "cccccccccccccccccccccccccccccccccccccccc";
      const runner = statusRunner({
        activeVersion: "1.3.0-staging.2",
        activeCommit: rcCommit,
        activeSourceBranch: "release/1.3",
        releaseBranchSha: rcCommit,
        productionTag: "v1.2.3",
        behindMain: 5,
        cherry: "1111111111111111111111111111111111111111 fix: only on the branch"
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.freeze).toEqual({ active: false, branch: null, reason: null, waivedByReset: false });
      expect(result.staging?.commitsBehindMain).toBe(5);
      expect(result.releaseBranch).toEqual({
        name: "release/1.3",
        commit: rcCommit,
        abandoned: null,
        recuts: [],
        unmergedCommits: [{ sha: "1111111111111111111111111111111111111111", subject: "fix: only on the branch" }],
        unmergedCommitCount: 1
      });
      expect(result.promotion.base).toBe(rcCommit);
      expect(result.promotion.allowed).toBe(true);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("does not require a recorded reset to keep the macOS train moving", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const rcCommit = "cccccccccccccccccccccccccccccccccccccccc";
      const runner = statusRunner({
        activeVersion: "1.3.0-staging.2",
        activeCommit: rcCommit,
        activeSourceBranch: "release/1.3",
        releaseBranchSha: rcCommit,
        productionTag: "v1.2.3",
        channelBody: [
          "Pointer-only desktop staging updater channel.",
          "",
          "Lineage-Reset: 2026-09-04T00:00:00Z",
          `Reset-From: 1.3.0-staging.2 (${rcCommit}) source release/1.3`,
          "Reset-To: main",
          "Reset-Reason: abandon the release candidate and resume trunk"
        ].join("\n")
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.freeze).toEqual({ active: false, branch: null, reason: null, waivedByReset: false });
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("clears the freeze once the release-branch candidate's production tag exists", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const rcCommit = "cccccccccccccccccccccccccccccccccccccccc";
      const runner = statusRunner({
        activeVersion: "1.3.0-staging.2",
        activeCommit: rcCommit,
        activeSourceBranch: "release/1.3",
        releaseBranchSha: rcCommit,
        existingProductionTags: ["1.3.0"],
        productionTag: "v1.3.0"
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.freeze).toEqual({ active: false, branch: null, reason: null, waivedByReset: false });
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("blocks promotion while the policy soak window has not elapsed", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const runner = statusRunner({
        activeVersion: "1.2.4-staging.3",
        activeSourceBranch: "main",
        activePublishedAt: new Date(NOW - 3 * 3_600_000).toISOString(),
        productionTag: "v1.2.3"
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.promotion.mechanicallyPromotable).toBe(true);
      expect(result.lineage?.valid).toBe(true);
      expect(result.promotion.soak).toMatchObject({ requiredHours: 24, elapsedHours: 3, satisfied: false });
      expect(result.promotion.allowed).toBe(false);
      expect(result.promotion.blockers.join(" ")).toMatch(/soaked 3\.0h of the required 24h/);
      expect(result.promoteCommand).toBeNull();
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("reads the soak window from the selected candidate's release policy", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const runner = statusRunner({
        activeVersion: "1.2.4-staging.3",
        activeSourceBranch: "main",
        activePublishedAt: new Date(NOW - 3 * 3_600_000).toISOString(),
        productionTag: "v1.2.3",
        releasePolicy: '{"productionSoakHours": 1}\n'
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.policy.productionSoakHours).toBe(1);
      expect(result.promotion.soak.satisfied).toBe(true);
      expect(result.promotion.allowed).toBe(true);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("keeps an immutable staging candidate promotable after main advances", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const staleCommit = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
      const runner = statusRunner({
        activeVersion: "1.2.4-staging.3",
        activeCommit: staleCommit,
        activeSourceBranch: "main",
        productionTag: "v1.2.3",
        behindMain: 7
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.staging?.commitsBehindMain).toBe(7);
      expect(result.promotion.mechanicallyPromotable).toBe(true);
      expect(result.promotion.base).toBe(staleCommit);
      expect(result.promotion.mechanicalReason).toBeNull();
      expect(result.promotion.allowed).toBe(true);
      expect(result.promoteCommand).toBe("kd release promote 1.2.4-staging.3");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("keeps a main RC promotable when a dormant same-series release branch exists", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const runner = statusRunner({
        activeVersion: "1.3.1-staging.1",
        activeSourceBranch: "main",
        releaseBranchSha: "dddddddddddddddddddddddddddddddddddddddd",
        productionTag: "v1.3.0"
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.staging?.sourceBranch).toBe("main");
      expect(result.releaseBranch?.name).toBe("release/1.3");
      expect(result.freeze.active).toBe(false);
      expect(result.promotion.mechanicallyPromotable).toBe(true);
      expect(result.promotion.allowed).toBe(true);
      expect(result.promoteCommand).toBe("kd release promote 1.3.1-staging.1");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("fails closed when the active candidate's GitHub metadata is missing", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const runner = statusRunner({
        activeVersion: "1.2.4-staging.3",
        activeCommit: null,
        productionTag: "v1.2.3"
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.staging?.commit).toBeNull();
      expect(result.promotion.mechanicallyPromotable).toBe(false);
      expect(result.promotion.allowed).toBe(false);
      expect(result.promotion.blockers.join(" ")).toMatch(/records no target commit/);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("fails closed when the active candidate is absent from the prerelease listing", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const runner = statusRunner({
        activeVersion: "1.2.4-staging.3",
        activeSourceBranch: "main",
        candidateTags: [],
        productionTag: "v1.2.3"
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.lineage?.relationship).toBe("unknown");
      expect(result.lineage?.valid).toBe(false);
      expect(result.promotion.mechanicallyPromotable).toBe(true);
      expect(result.promotion.allowed).toBe(false);
      expect(result.promoteCommand).toBeNull();
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("reports an abandoned series and refuses to promote its candidate", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const rcCommit = "cccccccccccccccccccccccccccccccccccccccc";
      const runner = statusRunner({
        activeVersion: "0.1.0-staging.8",
        activeCommit: rcCommit,
        activeSourceBranch: "release/0.1",
        releaseBranchSha: rcCommit,
        productionTag: "v0.0.68",
        abandonedSeries: {
          "0.1": "Abandoned release/0.1 at 2026-08-13T09:00:00.000Z\n\nReason: 0.1 diverged from main\n"
        }
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.releaseBranch?.abandoned).toEqual({
        abandonedAt: "2026-08-13T09:00:00.000Z",
        reason: "0.1 diverged from main"
      });
      // Mechanically it still matches its branch tip; the series decision is what stops it.
      expect(result.promotion.mechanicallyPromotable).toBe(true);
      expect(result.promotion.allowed).toBe(false);
      expect(result.promotion.blockers[0]).toMatch(/release\/0\.1 was abandoned on 2026-08-13T09:00:00\.000Z/);
      expect(result.promoteCommand).toBeNull();
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("reports an unreadable channel as an error, not as an empty one", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const runner = statusRunner({
        activeVersion: "1.2.4-staging.3",
        channelUnreadable: true,
        productionTag: "v1.2.3"
      });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.staging).toBeNull();
      expect(result.promotion.allowed).toBe(false);
      expect(result.promotion.blockers).toEqual([
        expect.stringContaining("desktop-staging channel could not be read")
      ]);
      expect(result.promotion.blockers[0]).not.toMatch(/No staging release candidate is active/);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("returns empty channels when no releases exist yet", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const runner = statusRunner({ activeVersion: null, productionTag: null });

      const result = await releaseStatus({ repoRoot: root, env: {}, runner, now: NOW });

      expect(result.production).toBeNull();
      expect(result.staging).toBeNull();
      expect(result.releaseBranch).toBeNull();
      expect(result.lineage).toBeNull();
      expect(result.freeze).toEqual({ active: false, branch: null, reason: null, waivedByReset: false });
      expect(result.promotion.allowed).toBe(false);
      expect(result.promotion.blockers).toEqual(["No staging release candidate is active on the channel."]);
      expect(result.promoteCommand).toBeNull();
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});

// The staging channel is a single pointer, so these gates run at the command
// boundary before anything is built. A real end-to-end proof would need signed
// artifacts, live GitHub prereleases, and an installed updater; see the note at
// the top of this file and docs/2026-08-13-release-lifecycle-e2e-gap.md.
describe("staging publish lineage gates", () => {
  const ACTIVE_COMMIT = "7777777777777777777777777777777777777777";
  const DESCENDANT_COMMIT = "8888888888888888888888888888888888888888";
  const DIVERGED_COMMIT = "beef000000000000000000000000000000000000";
  const BRANCH_POINT = "1111111111111111111111111111111111111111";
  const PRODUCTION_TAG_COMMIT = "4444444444444444444444444444444444444444";

  interface ShipGateFixture {
    head: string;
    branch?: string;
    sourceBranch?: string;
    activeVersion?: string | null;
    activeCommit?: string | null;
    activeSourceBranch?: string | null;
    /** `git merge-base --is-ancestor <active> <head>` exit code. */
    activeIsAncestorOfHead?: number;
    /** `git merge-base --is-ancestor <head> <active>` exit code. */
    headIsAncestorOfActive?: number;
    existingProductionTags?: string[];
    productionReleaseTargetCommit?: string;
    productionTagCommit?: string;
    productionTagParent?: string;
    originMain?: string;
    originRelease?: string;
    recutNewTip?: string;
    recutNewTipIsAncestor?: number;
    candidateSourceBranch?: string;
    mergeBase?: string;
    /** Result of proving the merge-base is contained by origin/main. */
    mainContainsMergeBase?: number;
    /** Result of proving the candidate remains in its recorded source branch. */
    releaseContainsCandidate?: number;
    /** Result of proving HEAD descends from the main/release branch point. */
    headDescendsFromMergeBase?: number;
    channelBody?: string;
    releaseBranchSha?: string;
    /** Production releases in GitHub's newest-created-first response order. */
    productionReleasesInCreationOrder?: string[];
    /** Series carrying an abandonment tag, as `X.Y` -> annotated tag message. */
    abandonedSeries?: Record<string, string>;
    /** Simulates a transient GitHub failure reading the channel itself. */
    channelUnreadable?: boolean;
    /** Simulates the manifest asset existing but failing to download. */
    manifestDownloadFails?: boolean;
    /** Raw latest-staging.json contents, for malformed-manifest cases. */
    manifestBody?: string;
    /** Existing versioned candidate and a one-shot pointer upload failure for retry coverage. */
    existingStagingTags?: string[];
    failStagingChannelUploadOnce?: boolean;
  }

  function shipGateRunner(fixture: ShipGateFixture, repoRoot: string, outputs: Map<string, string>, calls: CommandCall[]): CommandRunner {
    const activeVersion = fixture.activeVersion === undefined ? "1.2.4-staging.2" : fixture.activeVersion;
    const activeCommit = fixture.activeCommit === undefined ? ACTIVE_COMMIT : fixture.activeCommit;
    let existingStagingTags = [...(fixture.existingStagingTags ?? [])];
    let failStagingChannelUpload = fixture.failStagingChannelUploadOnce ?? false;
    return {
      async run(command, args, options) {
        calls.push({ command, args, options });
        const key = `${command} ${args.join(" ")}`;
        if (key === "git status --porcelain") return { exitCode: 0, stdout: "", stderr: "" };
        if (key === "git remote get-url origin") {
          return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
        }
        if (key === "git rev-parse --abbrev-ref HEAD") {
          return { exitCode: 0, stdout: `${fixture.branch ?? "main"}\n`, stderr: "" };
        }
        if (key === "git rev-parse HEAD") return { exitCode: 0, stdout: `${fixture.head}\n`, stderr: "" };
        if (key === "git rev-parse FETCH_HEAD^{commit}") {
          return { exitCode: 0, stdout: `${fixture.productionTagCommit ?? activeCommit ?? ""}\n`, stderr: "" };
        }
        if (key === "git show -s --format=%P FETCH_HEAD") {
          return { exitCode: 0, stdout: `${fixture.productionTagParent ?? activeCommit ?? ""}\n`, stderr: "" };
        }
        if (key === "git show -s --format=%s FETCH_HEAD") {
          const productionVersion = activeVersion?.replace(/-staging\.\d+$/, "") ?? "";
          return { exitCode: 0, stdout: `release: v${productionVersion}\n`, stderr: "" };
        }
        if (key === "git rev-parse origin/main") {
          return { exitCode: 0, stdout: `${fixture.originMain ?? fixture.head}\n`, stderr: "" };
        }
        if (fixture.activeSourceBranch && key === `git rev-parse origin/${fixture.activeSourceBranch}`) {
          return { exitCode: 0, stdout: `${fixture.originRelease ?? activeCommit ?? ""}\n`, stderr: "" };
        }
        if (command === "git" && args[0] === "ls-remote" && args[1] === "origin") {
          const sha = fixture.releaseBranchSha ?? "";
          return { exitCode: 0, stdout: sha ? `${sha}\t${args[2]}\n` : "", stderr: "" };
        }
        if (command === "git" && ["fetch", "push", "tag"].includes(args[0] ?? "")) return { exitCode: 0, stdout: "", stderr: "" };
        if (isStagingChannelAssetsQuery(command, args)) {
          if (fixture.channelUnreadable) return { exitCode: 1, stdout: "", stderr: "HTTP 503: Service unavailable" };
          return stagingChannelAssetsResponse(activeVersion ? ["latest-staging.json"] : null);
        }
        if (command === "gh" && args[0] === "release" && args[1] === "download") {
          if (!activeVersion) return { exitCode: 1, stdout: "", stderr: "release not found" };
          if (fixture.manifestDownloadFails) return { exitCode: 1, stdout: "", stderr: "HTTP 502: Bad gateway" };
          const dirIndex = args.indexOf("--dir");
          writeFileSync(
            join(args[dirIndex + 1] ?? "", "latest-staging.json"),
            fixture.manifestBody ?? `{"version":"${activeVersion}"}\n`
          );
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (activeVersion && key.startsWith(`gh release view v${activeVersion} `)) {
          if (activeCommit === null) return { exitCode: 1, stdout: "", stderr: "release not found" };
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              targetCommitish: activeCommit,
              publishedAt: "2026-07-06T00:00:00Z",
              body: `Staging updater manifest\n\nSource-Branch: ${fixture.activeSourceBranch ?? "main"}`
            }),
            stderr: ""
          };
        }
        const productionVersion = activeVersion?.replace(/-staging\.\d+$/, "");
        if (productionVersion && key.startsWith(`gh release view v${productionVersion} `)) {
          const exists = (fixture.existingProductionTags ?? []).includes(productionVersion);
          if (!exists) return { exitCode: 1, stdout: "", stderr: "release not found" };
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              tagName: `v${productionVersion}`,
              targetCommitish: fixture.productionReleaseTargetCommit ?? activeCommit,
              isPrerelease: false
            }),
            stderr: ""
          };
        }
        if (command === "gh" && args[0] === "release" && args[1] === "view" && args[2] === "desktop-staging") {
          return { exitCode: 0, stdout: JSON.stringify({ body: fixture.channelBody ?? "" }), stderr: "" };
        }
        if (command === "git" && args[0] === "merge-base") {
          if (args[1] !== "--is-ancestor") {
            return { exitCode: 0, stdout: `${fixture.mergeBase ?? BRANCH_POINT}\n`, stderr: "" };
          }
          const [, , base, candidate] = args;
          if (base === fixture.recutNewTip && candidate === fixture.head) {
            return { exitCode: fixture.recutNewTipIsAncestor ?? 1, stdout: "", stderr: "" };
          }
          if (base === activeCommit && candidate === fixture.head) {
            return { exitCode: fixture.activeIsAncestorOfHead ?? 0, stdout: "", stderr: "" };
          }
          if (base === fixture.head && candidate === activeCommit) {
            return { exitCode: fixture.headIsAncestorOfActive ?? 1, stdout: "", stderr: "" };
          }
          if (base === activeCommit && candidate === (fixture.originRelease ?? activeCommit)) {
            return { exitCode: fixture.releaseContainsCandidate ?? 0, stdout: "", stderr: "" };
          }
          if (base === (fixture.mergeBase ?? BRANCH_POINT) && candidate === (fixture.originMain ?? fixture.head)) {
            return { exitCode: fixture.mainContainsMergeBase ?? 0, stdout: "", stderr: "" };
          }
          if (base === (fixture.mergeBase ?? BRANCH_POINT) && candidate === fixture.head) {
            return { exitCode: fixture.headDescendsFromMergeBase ?? 0, stdout: "", stderr: "" };
          }
          return { exitCode: 1, stdout: "", stderr: "" };
        }
        if (command === "git" && args[0] === "ls-remote" && args[1] === "--tags") {
          const pattern = args[3] ?? "";
          if (pattern.startsWith("refs/tags/v") && !pattern.includes("staging") && !pattern.includes("abandoned")) {
            const wanted = pattern.replace(/^refs\/tags\/v/, "");
            const exists = (fixture.existingProductionTags ?? []).includes(wanted);
            const tagCommit = fixture.productionTagCommit ?? activeCommit ?? "";
            return {
              exitCode: 0,
              stdout: exists && tagCommit ? `${tagCommit}\trefs/tags/v${wanted}\n` : "",
              stderr: ""
            };
          }
          const abandoned = /^refs\/tags\/abandoned\/release\/(\d+\.\d+)$/.exec(pattern);
          if (abandoned) {
            const has = Boolean(fixture.abandonedSeries?.[abandoned[1] ?? ""]);
            return { exitCode: 0, stdout: has ? `sha\t${pattern}\n` : "", stderr: "" };
          }
          const wanted = pattern.replace(/^v/, "");
          if (wanted.includes("staging")) {
            return {
              exitCode: 0,
              stdout: existingStagingTags.map((tag) => `${fixture.head}\trefs/tags/v${tag}\n`).join(""),
              stderr: ""
            };
          }
          const exists = (fixture.existingProductionTags ?? []).includes(wanted);
          return { exitCode: 0, stdout: exists ? `sha\trefs/tags/v${wanted}\n` : "", stderr: "" };
        }
        if (command === "git" && args[0] === "for-each-ref") {
          const series = /abandoned\/release\/(\d+\.\d+)$/.exec(args[2] ?? "")?.[1] ?? "";
          return { exitCode: 0, stdout: fixture.abandonedSeries?.[series] ?? "", stderr: "" };
        }
        if (command === "bazel" && args[0] === "build") return { exitCode: 0, stdout: "", stderr: "" };
        if (command === "bazel" && args[0] === "cquery") {
          return { exitCode: 0, stdout: `${outputs.get(args[3] ?? "") ?? ""}\n`, stderr: "" };
        }
        if (command === "sh" && args[0] === "-c") return { exitCode: 0, stdout: "", stderr: "" };
        if (command === "pnpm") {
          writeFileSync(`${args.at(-1)}.sig`, "staging signature\n");
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "gh" && args[0] === "release" && args[1] === "view" && args[2] !== "desktop-staging") {
          const tag = args[2] ?? "";
          if (!existingStagingTags.includes(tag.replace(/^v/, ""))) {
            return { exitCode: 1, stdout: "", stderr: "release not found" };
          }
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              tagName: tag,
              targetCommitish: fixture.head,
              body: `Staging updater manifest for ${tag}\n\nSource-Branch: ${fixture.candidateSourceBranch ?? "release/1.3"}`,
              publishedAt: "2026-08-17T03:00:00Z"
            }),
            stderr: ""
          };
        }
        if (command === "gh" && args[0] === "release" && args[1] === "create") {
          const tag = (args[2] ?? "").replace(/^v/, "");
          if (existingStagingTags.includes(tag)) {
            return { exitCode: 1, stdout: "", stderr: "release already exists" };
          }
          existingStagingTags.push(tag);
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "gh" && args[0] === "release" && args[1] === "upload" && args[2] === "desktop-staging" && failStagingChannelUpload) {
          failStagingChannelUpload = false;
          return { exitCode: 1, stdout: "", stderr: "transient channel upload failure" };
        }
        if (command === "gh" && args[0] === "release" && ["edit", "upload"].includes(args[1] ?? "")) {
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (isProductionReleaseListQuery(command, args)) {
          const versions = fixture.productionReleasesInCreationOrder ?? fixture.existingProductionTags ?? [];
          return {
            exitCode: 0,
            stdout: JSON.stringify(versions.map((version) => ({ tagName: `v${version}`, isPrerelease: false }))),
            stderr: ""
          };
        }
        if (command === "gh" && args[0] === "release" && args[1] === "list") {
          return {
            exitCode: 0,
            stdout: JSON.stringify(existingStagingTags.map((tag, index) => ({
              tagName: `v${tag}`,
              createdAt: new Date(Date.parse("2026-08-17T03:00:00Z") + index * 1000).toISOString()
            }))),
            stderr: ""
          };
        }
        return { exitCode: 1, stdout: "", stderr: `unexpected command ${key}` };
      }
    };
  }

  function shipGateInput(
    repoRoot: string,
    privateKeyPath: string,
    runner: CommandRunner,
    sourceBranch?: string,
    release = false
  ): ReleaseShipInput {
    return {
      repoRoot,
      bump: "patch",
      archLabels: release ? ["arm64", "x86_64"] : ["arm64"],
      release,
      dryRun: !release,
      environment: "staging",
      sourceBranch,
      now: Date.parse("2026-08-17T03:00:00Z"),
      env: releaseEnv(privateKeyPath),
      runner
    };
  }

  it("automatically continues the active unpromoted staging series", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64"]);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner({ head: DESCENDANT_COMMIT }, repoRoot, outputs, calls);

      const result = await shipRelease(shipGateInput(repoRoot, privateKeyPath, runner));

      expect(result.version).toBe("1.2.4-staging.3");
      expect(calls.some((call) => call.command === "bazel" && call.args[0] === "build")).toBe(true);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("starts the next valid staging series after an earlier RC is promoted while a newer RC remains live", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-next-train-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64"]);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        {
          head: DESCENDANT_COMMIT,
          activeVersion: "1.2.4-staging.22",
          activeCommit: ACTIVE_COMMIT,
          activeSourceBranch: "main",
          existingProductionTags: ["1.2.4"],
          productionReleasesInCreationOrder: ["1.2.4"]
        },
        repoRoot,
        outputs,
        calls
      );

      const result = await shipRelease(shipGateInput(repoRoot, privateKeyPath, runner));
      expect(result.version).toBe("1.3.0-staging.1");
      expect(result.versionFloor).toMatchObject({
        versionFile: "1.2.3",
        greatestProductionVersion: "1.2.4",
        baseVersion: "1.3.0"
      });
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses a semver regression before building even when commit lineage moves forward", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        { head: DESCENDANT_COMMIT, activeVersion: "2.0.0-staging.3" },
        repoRoot,
        new Map(),
        calls
      );
      const input = shipGateInput(repoRoot, privateKeyPath, runner);
      input.bumpExplicit = true;

      await expect(shipRelease(input)).rejects.toThrow(
        /Refusing to roll the staging channel version back.*derived v1\.2\.4-staging\.1.*currently serves v2\.0\.0-staging\.3/s
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
      expect(readVersionFiles(repoRoot)[0]).toBe("1.2.3\n");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("returns a nonzero CLI exit code when staging semver would not advance", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-cli-"));
    const privateKeyPath = join(root, "updater-private.key");
    writeFileSync(privateKeyPath, "private key\n", { mode: 0o600 });
    const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
    const calls: CommandCall[] = [];
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const previousEnvironment = {
      HOME: process.env.HOME,
      KANNA_UPDATER_PUBKEY: process.env.KANNA_UPDATER_PUBKEY,
      TAURI_PRIVATE_KEY_PATH: process.env.TAURI_PRIVATE_KEY_PATH
    };
    process.env.HOME = root;
    process.env.KANNA_UPDATER_PUBKEY = "pubkey";
    process.env.TAURI_PRIVATE_KEY_PATH = privateKeyPath;

    const runner = vi.spyOn(nodeCommandRunner, "run").mockImplementation(async (command, args, options) => {
      calls.push({ command, args, options });
      const key = `${command} ${args.join(" ")}`;
      if (key === "git rev-parse --show-toplevel") {
        return { exitCode: 0, stdout: `${repoRoot}\n`, stderr: "" };
      }
      if (key === "git rev-parse --abbrev-ref HEAD") {
        return { exitCode: 0, stdout: "main\n", stderr: "" };
      }
      if (key === "git rev-parse --short HEAD") {
        return { exitCode: 0, stdout: "8888888\n", stderr: "" };
      }
      if (key === "git rev-parse HEAD") {
        return { exitCode: 0, stdout: `${DESCENDANT_COMMIT}\n`, stderr: "" };
      }
      if (key === "git status --porcelain") {
        return { exitCode: 0, stdout: "", stderr: "" };
      }
      if (key === "git remote get-url origin") {
        return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
      }
      if (isProductionReleaseListQuery(command, args)) {
        return { exitCode: 0, stdout: "[]", stderr: "" };
      }
      if (isStagingChannelAssetsQuery(command, args)) {
        return stagingChannelAssetsResponse(["latest-staging.json"]);
      }
      if (command === "gh" && args[0] === "release" && args[1] === "download") {
        const dirIndex = args.indexOf("--dir");
        writeFileSync(join(args[dirIndex + 1] ?? "", "latest-staging.json"), '{"version":"2.0.0-staging.3"}\n');
        return { exitCode: 0, stdout: "", stderr: "" };
      }
      if (key.startsWith("gh release view v2.0.0-staging.3 ")) {
        return {
          exitCode: 0,
          stdout: JSON.stringify({
            targetCommitish: ACTIVE_COMMIT,
            publishedAt: "2026-08-21T00:00:00Z",
            body: "Staging updater manifest\n\nSource-Branch: main"
          }),
          stderr: ""
        };
      }
      if (key.startsWith("gh release view v2.0.0 ")) {
        return { exitCode: 1, stdout: "", stderr: "release not found" };
      }
      if (command === "git" && args[0] === "fetch") {
        return { exitCode: 0, stdout: "", stderr: "" };
      }
      if (command === "git" && args[0] === "merge-base" && args[1] === "--is-ancestor") {
        return {
          exitCode: args[2] === ACTIVE_COMMIT && args[3] === DESCENDANT_COMMIT ? 0 : 1,
          stdout: "",
          stderr: ""
        };
      }
      if (command === "git" && args[0] === "ls-remote") {
        return { exitCode: 0, stdout: "", stderr: "" };
      }
      return { exitCode: 1, stdout: "", stderr: `unexpected command ${key}` };
    });

    try {
      await expect(runCli(["release", "ship", "--staging", "--dry-run", "--patch"])).resolves.toBe(1);
      expect(error).toHaveBeenLastCalledWith(expect.stringMatching(
        /Refusing to roll the staging channel version back.*currently serves v2\.0\.0-staging\.3/s
      ));
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      runner.mockRestore();
      error.mockRestore();
      for (const [key, value] of Object.entries(previousEnvironment)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
      }
      await rm(root, { recursive: true, force: true });
    }
  });

  it("uses an explicit bump to start a new series instead of continuing the channel", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64"]);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner({ head: DESCENDANT_COMMIT }, repoRoot, outputs, calls);
      const input = shipGateInput(repoRoot, privateKeyPath, runner);
      input.bump = "minor";
      input.bumpExplicit = true;

      const result = await shipRelease(input);

      expect(result.version).toBe("1.3.0-staging.1");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("floors main staging against the greatest production version regardless of release creation order", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64"]);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        {
          head: DESCENDANT_COMMIT,
          activeVersion: null,
          productionReleasesInCreationOrder: ["1.2.5", "1.3.0"]
        },
        repoRoot,
        outputs,
        calls
      );

      const result = await shipRelease(shipGateInput(repoRoot, privateKeyPath, runner));

      expect(result.version).toBe("1.4.0-staging.1");
      expect(result.version).not.toBe("1.2.6-staging.1");
      expect(result.versionFloor).toEqual({
        versionFile: "1.2.3",
        greatestProductionVersion: "1.3.0",
        baseVersion: "1.4.0",
        detail:
          "VERSION 1.2.3 lags greatest production semantic version v1.3.0; " +
          "derived main staging version 1.4.0 from the production floor."
      });
      const query = calls.find((call) => isProductionReleaseListQuery(call.command, call.args));
      const limitIndex = query?.args.indexOf("--limit") ?? -1;
      expect(limitIndex).toBeGreaterThanOrEqual(0);
      expect(query?.args[limitIndex + 1]).toBe("1000");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("publishes a same-branch fast-forward RC from the release branch tip", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      writeBranchCandidate(repoRoot, "1.3.0", 2);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64"]);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        {
          head: DESCENDANT_COMMIT,
          activeVersion: "1.3.0-staging.1",
          activeSourceBranch: "release/1.3",
          releaseBranchSha: DESCENDANT_COMMIT
        },
        repoRoot,
        outputs,
        calls
      );

      const result = await shipRelease(shipGateInput(repoRoot, privateKeyPath, runner, "release/1.3"));

      expect(result.version).toBe("1.3.0-staging.2");
      const manifest = JSON.parse(readFileSync(result.latestJson, "utf8")) as { notes?: string };
      expect(manifest.notes).toContain("Source-Branch: release/1.3");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("allows the main-to-release-branch freeze transition when the cut descends from the active RC", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      writeBranchCandidate(repoRoot, "1.3.0", 1);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64"]);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        {
          head: DESCENDANT_COMMIT,
          activeVersion: "1.2.4-staging.2",
          activeSourceBranch: "main",
          releaseBranchSha: DESCENDANT_COMMIT
        },
        repoRoot,
        outputs,
        calls
      );

      const result = await shipRelease(shipGateInput(repoRoot, privateKeyPath, runner, "release/1.3"));

      expect(result.version).toBe("1.3.0-staging.1");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  // v0.1.0-staging.7 -> v0.1.0-staging.8: a stale release branch whose history
  // both added and dropped commits relative to the channel.
  it("refuses a divergent-history candidate before building", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      writeBranchCandidate(repoRoot, "0.1.0", 8);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        {
          head: DIVERGED_COMMIT,
          activeVersion: "0.1.0-staging.7",
          activeSourceBranch: "main",
          activeIsAncestorOfHead: 1,
          headIsAncestorOfActive: 1,
          releaseBranchSha: DIVERGED_COMMIT
        },
        repoRoot,
        new Map(),
        calls
      );

      await expect(shipRelease(shipGateInput(repoRoot, privateKeyPath, runner, "release/0.1"))).rejects.toThrow(
        /diverged from the active channel/
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
      // The refused ship left the branch's committed candidate exactly as it was.
      expect(readVersionFiles(repoRoot)[0]).toBe("0.1.0\n");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses a candidate that would roll the channel backwards", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        { head: DESCENDANT_COMMIT, activeIsAncestorOfHead: 1, headIsAncestorOfActive: 0 },
        repoRoot,
        new Map(),
        calls
      );

      await expect(shipRelease(shipGateInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /Refusing to roll the staging channel back/
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("keeps main staging publishing while an earlier release-branch RC soaks", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64"]);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        { head: DESCENDANT_COMMIT, activeVersion: "1.3.0-staging.2", activeSourceBranch: "release/1.3" },
        repoRoot,
        outputs,
        calls
      );

      await expect(shipRelease(shipGateInput(repoRoot, privateKeyPath, runner))).resolves.toMatchObject({
        version: "1.3.0-staging.3"
      });
      expect(calls.some((call) => call.command === "bazel")).toBe(true);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("allows promoted divergent RC lineage to resume on forward main and records provenance", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        {
          head: DIVERGED_COMMIT,
          activeVersion: "1.3.0-staging.2",
          activeSourceBranch: "release/1.3",
          activeIsAncestorOfHead: 1,
          headIsAncestorOfActive: 1,
          existingProductionTags: ["1.3.0"],
          productionReleaseTargetCommit: "main",
          productionTagCommit: PRODUCTION_TAG_COMMIT,
          productionTagParent: ACTIVE_COMMIT,
          originMain: DIVERGED_COMMIT,
          originRelease: ACTIVE_COMMIT
        },
        repoRoot,
        outputs,
        calls
      );

      const result = await shipRelease(shipGateInput(repoRoot, privateKeyPath, runner, undefined, true));

      expect(result.version).toBe("1.4.0-staging.1");
      expect(result.versionFloor).toEqual({
        versionFile: "1.2.3",
        greatestProductionVersion: "1.3.0",
        baseVersion: "1.4.0",
        detail:
          "VERSION 1.2.3 lags greatest production semantic version v1.3.0; " +
          "derived main staging version 1.4.0 from the production floor."
      });
      const edit = calls.find(
        (call) => call.command === "gh" && call.args[0] === "release" && call.args[1] === "edit"
      );
      const body = edit?.args[edit.args.indexOf("--notes") + 1] ?? "";
      expect(body).toContain("Post-Promotion-Trunk-Resumption: 2026-08-17T03:00:00.000Z");
      expect(body).toContain("Promoted-Version: 1.3.0");
      expect(body).toContain("Promoted-Tag: v1.3.0");
      expect(body).toContain(`Promoted-Commit: ${ACTIVE_COMMIT}`);
      expect(body).toContain(`Production-Tag-Commit: ${PRODUCTION_TAG_COMMIT}`);
      expect(body).toContain(`Resumed-To: ${DIVERGED_COMMIT} source main`);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses promoted but stale main that does not descend from the release branch point", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        {
          head: DIVERGED_COMMIT,
          activeVersion: "1.3.0-staging.2",
          activeSourceBranch: "release/1.3",
          activeIsAncestorOfHead: 1,
          headIsAncestorOfActive: 1,
          existingProductionTags: ["1.3.0"],
          originMain: "2222222222222222222222222222222222222222",
          headDescendsFromMergeBase: 1
        },
        repoRoot,
        new Map(),
        calls
      );

      await expect(shipRelease(shipGateInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /diverged from the active channel/
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses promoted divergence when the production tag targets another commit", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        {
          head: DIVERGED_COMMIT,
          activeVersion: "1.3.0-staging.2",
          activeSourceBranch: "release/1.3",
          activeIsAncestorOfHead: 1,
          headIsAncestorOfActive: 1,
          existingProductionTags: ["1.3.0"],
          productionReleaseTargetCommit: "main",
          productionTagCommit: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          productionTagParent: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        },
        repoRoot,
        new Map(),
        calls
      );

      await expect(shipRelease(shipGateInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /diverged from the active channel/
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses to move the channel when the active candidate's metadata is unreadable", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner({ head: DESCENDANT_COMMIT, activeCommit: null }, repoRoot, new Map(), calls);

      await expect(shipRelease(shipGateInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /Cannot verify staging lineage/
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  // An uninitialized channel is the ONLY shape that may skip the lineage
  // comparison, and it has to be positive evidence: the pointer release does
  // not exist. A failed read looks identical from a single exit code, so these
  // cases are kept apart deliberately — conflating them is what would let a
  // rate limit or a 5xx wave a publish through with nothing verified.
  it("publishes onto an uninitialized channel without a lineage to compare", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64"]);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner({ head: DESCENDANT_COMMIT, activeVersion: null }, repoRoot, outputs, calls);

      const result = await shipRelease(shipGateInput(repoRoot, privateKeyPath, runner));

      expect(result.version).toBe("1.3.0-staging.1");
      // Positive evidence of emptiness: the channel release itself 404s.
      const channelRead = calls.find(
        (call) => call.command === "gh" && call.args[1] === "view" && call.args[2] === "desktop-staging"
      );
      expect(channelRead).toBeDefined();
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses when the channel exists but its manifest cannot be read", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        { head: DESCENDANT_COMMIT, manifestDownloadFails: true },
        repoRoot,
        new Map(),
        calls
      );

      await expect(shipRelease(shipGateInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /Cannot verify staging lineage.*Bad gateway/s
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses when the channel itself cannot be reached", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner({ head: DESCENDANT_COMMIT, channelUnreadable: true }, repoRoot, new Map(), calls);

      await expect(shipRelease(shipGateInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /Cannot verify staging lineage.*Service unavailable/s
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses when the channel manifest is present but unparseable", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        { head: DESCENDANT_COMMIT, manifestBody: "{ not json\n" },
        repoRoot,
        new Map(),
        calls
      );

      await expect(shipRelease(shipGateInput(repoRoot, privateKeyPath, runner))).rejects.toThrow(
        /Cannot verify staging lineage.*has no valid version/s
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("lets a recorded lineage reset authorize exactly the divergent publish it named", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      writeBranchCandidate(repoRoot, "0.1.0", 8);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64"]);
      const calls: CommandCall[] = [];
      const channelBody = [
        "Pointer-only desktop staging updater channel.",
        "",
        "Lineage-Reset: 2026-07-07T00:00:00Z",
        `Reset-From: 0.1.0-staging.7 (${ACTIVE_COMMIT}) source main`,
        "Reset-To: release/0.1",
        "Reset-Reason: hotfix the 0.1 series"
      ].join("\n");
      const fixture: ShipGateFixture = {
        head: DIVERGED_COMMIT,
        activeVersion: "0.1.0-staging.7",
        activeSourceBranch: "main",
        activeIsAncestorOfHead: 1,
        headIsAncestorOfActive: 1,
        releaseBranchSha: DIVERGED_COMMIT,
        channelBody
      };

      const result = await shipRelease(
        shipGateInput(repoRoot, privateKeyPath, shipGateRunner(fixture, repoRoot, outputs, calls), "release/0.1")
      );
      expect(result.version).toBe("0.1.0-staging.8");

      // The same record does not license a different destination.
      const otherCalls: CommandCall[] = [];
      // The reset named release/0.1; a release/0.2 publish is a different
      // branch, shipped from a worktree carrying that series' own version.
      writeBranchCandidate(repoRoot, "0.2.0", 1);
      await expect(
        shipRelease(
          shipGateInput(repoRoot, privateKeyPath, shipGateRunner(fixture, repoRoot, outputs, otherCalls), "release/0.2")
        )
      ).rejects.toThrow(/diverged from the active channel/);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("consumes a recut for a later release-branch backport and rejects another branch", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-recut-ship-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      writeBranchCandidate(repoRoot, "1.3.0", 3);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const recutNewTip = "9999999999999999999999999999999999999999";
      const laterBranchTip = DIVERGED_COMMIT;
      const channelBody = [
        "Pointer-only desktop staging updater channel.",
        "",
        "Lineage-Recut: 2026-08-16T00:00:00Z",
        "Recut-Id: 1.3-1",
        "Recut-Series: 1.3",
        "Recut-Branch: release/1.3",
        `Recut-Old-Tip: ${ACTIVE_COMMIT}`,
        `Recut-New-Tip: ${recutNewTip}`,
        "Recut-Archive-Tag: recut/release/1.3-1",
        `Recut-From: 1.3.0-staging.2 (${ACTIVE_COMMIT}) source main`,
        "Recut-Prior-Epoch: 1.3.0-staging.2",
        "Recut-Requester: test-user",
        "Recut-Reason: include the latest feature"
      ].join("\n");
      const fixture: ShipGateFixture = {
        head: laterBranchTip,
        activeVersion: "1.3.0-staging.2",
        activeSourceBranch: "main",
        activeIsAncestorOfHead: 1,
        headIsAncestorOfActive: 1,
        releaseBranchSha: laterBranchTip,
        recutNewTip,
        recutNewTipIsAncestor: 0,
        channelBody
      };

      const result = await shipRelease(
        shipGateInput(repoRoot, privateKeyPath, shipGateRunner(fixture, repoRoot, outputs, calls), "release/1.3", true)
      );
      expect(result.version).toBe("1.3.0-staging.3");
      const manifest = JSON.parse(readFileSync(result.latestJson, "utf8")) as { notes?: string };
      expect(manifest.notes).toContain("Lineage-Recut-Authorization: 1.3-1");
      expect(calls.some((call) => call.command === "git" && call.args.includes("recut-applied/1.3-1"))).toBe(true);

      writeBranchCandidate(repoRoot, "1.2.4", 3);
      await expect(
        shipRelease(
          shipGateInput(repoRoot, privateKeyPath, shipGateRunner(fixture, repoRoot, outputs, []), "release/1.2")
        )
      ).rejects.toThrow(/diverged from the active channel/);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("retries the same recut candidate when the pointer upload fails", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-recut-retry-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      writeBranchCandidate(repoRoot, "1.3.0", 3);
      const outputs = writeStagingReleaseBuildOutputs(repoRoot, ["arm64", "x86_64"]);
      const calls: CommandCall[] = [];
      const recutNewTip = "9999999999999999999999999999999999999999";
      const channelBody = [
        "Pointer-only desktop staging updater channel.",
        "",
        "Lineage-Recut: 2026-08-16T00:00:00Z",
        "Recut-Id: 1.3-1",
        "Recut-Series: 1.3",
        "Recut-Branch: release/1.3",
        `Recut-Old-Tip: ${ACTIVE_COMMIT}`,
        `Recut-New-Tip: ${recutNewTip}`,
        "Recut-Archive-Tag: recut/release/1.3-1",
        `Recut-From: 1.3.0-staging.2 (${ACTIVE_COMMIT}) source main`,
        "Recut-Prior-Epoch: 1.3.0-staging.2",
        "Recut-Requester: test-user",
        "Recut-Reason: include the latest feature"
      ].join("\n");
      const fixture: ShipGateFixture = {
        head: DIVERGED_COMMIT,
        activeVersion: "1.3.0-staging.2",
        activeSourceBranch: "main",
        activeIsAncestorOfHead: 1,
        headIsAncestorOfActive: 1,
        releaseBranchSha: DIVERGED_COMMIT,
        recutNewTip,
        recutNewTipIsAncestor: 0,
        channelBody,
        failStagingChannelUploadOnce: true
      };
      const runner = shipGateRunner(fixture, repoRoot, outputs, calls);
      const input = shipGateInput(repoRoot, privateKeyPath, runner, "release/1.3", true);

      await expect(shipRelease(input)).rejects.toThrow(/transient channel upload failure/);
      expect(calls.some((call) => call.command === "git" && call.args.includes("recut-applied/1.3-1"))).toBe(false);

      const retry = await shipRelease(input);
      expect(retry.version).toBe("1.3.0-staging.3");
      expect(calls.filter((call) => call.command === "gh" && call.args[0] === "release" && call.args[1] === "create")).toHaveLength(2);
      expect(calls.filter((call) => call.command === "git" && call.args[0] === "tag" && call.args.includes("recut-applied/1.3-1"))).toHaveLength(1);
      expect(calls.filter((call) => call.command === "git" && call.args[0] === "push" && call.args.some((arg) => arg.includes("recut-applied/1.3-1")))).toHaveLength(1);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses to ship an RC from an abandoned series", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner = shipGateRunner(
        {
          head: DESCENDANT_COMMIT,
          releaseBranchSha: DESCENDANT_COMMIT,
          abandonedSeries: {
            "0.1": "Abandoned release/0.1 at 2026-08-13T09:00:00.000Z\n\nReason: 0.1 diverged from main\n"
          }
        },
        repoRoot,
        new Map(),
        calls
      );

      await expect(shipRelease(shipGateInput(repoRoot, privateKeyPath, runner, "release/0.1"))).rejects.toThrow(
        /release\/0\.1 was abandoned on 2026-08-13T09:00:00\.000Z: 0\.1 diverged from main/
      );
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("still repoints the channel non-linearly through an explicit rollback", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const { repoRoot, privateKeyPath } = createReleaseRepo(root);
      const calls: CommandCall[] = [];
      const runner: CommandRunner = {
        async run(command, args, options) {
          calls.push({ command, args, options });
          const key = `${command} ${args.join(" ")}`;
          if (key === "git status --porcelain") return { exitCode: 0, stdout: "", stderr: "" };
          if (key === "git remote get-url origin") {
            return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
          }
          if (command === "gh" && args[1] === "view") return { exitCode: 0, stdout: "", stderr: "" };
          if (command === "gh" && args[1] === "download") {
            const dirIndex = args.indexOf("--dir");
            writeFileSync(join(args[dirIndex + 1] ?? "", "latest-staging.json"), '{"version":"1.2.4-staging.1"}\n');
            return { exitCode: 0, stdout: "", stderr: "" };
          }
          if (command === "gh" && args[1] === "upload") return { exitCode: 0, stdout: "", stderr: "" };
          return { exitCode: 1, stdout: "", stderr: `unexpected command ${key}` };
        }
      };

      const result = await shipRelease({
        repoRoot,
        bump: "patch",
        archLabels: ["arm64"],
        release: true,
        dryRun: false,
        environment: "staging",
        rollbackTo: "1.2.4-staging.1",
        env: releaseEnv(privateKeyPath),
        runner
      });

      expect(result.version).toBe("1.2.4-staging.1");
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
      expect(calls.some((call) => call.command === "git" && call.args[0] === "merge-base")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});

describe("staging lineage reset", () => {
  const ACTIVE_COMMIT = "7777777777777777777777777777777777777777";
  const RESET_NOW = Date.parse("2026-07-08T12:00:00Z");

  function resetRunner(
    calls: CommandCall[],
    options: { activeVersion?: string | null; channelBody?: string } = {}
  ): CommandRunner {
    const activeVersion = options.activeVersion === undefined ? "1.3.0-staging.2" : options.activeVersion;
    return {
      async run(command, args, runOptions) {
        calls.push({ command, args, options: runOptions });
        const key = `${command} ${args.join(" ")}`;
        if (key === "git remote get-url origin") {
          return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
        }
        if (isStagingChannelAssetsQuery(command, args)) {
          return stagingChannelAssetsResponse(activeVersion ? ["latest-staging.json"] : null);
        }
        if (command === "gh" && args[0] === "release" && args[1] === "download") {
          if (!activeVersion) return { exitCode: 1, stdout: "", stderr: "not found" };
          const dirIndex = args.indexOf("--dir");
          writeFileSync(join(args[dirIndex + 1] ?? "", "latest-staging.json"), `{"version":"${activeVersion}"}\n`);
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (activeVersion && key.startsWith(`gh release view v${activeVersion} `)) {
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              targetCommitish: ACTIVE_COMMIT,
              publishedAt: "2026-07-06T00:00:00Z",
              body: "Staging updater manifest\n\nSource-Branch: release/1.3"
            }),
            stderr: ""
          };
        }
        if (command === "gh" && args[1] === "view" && args[2] === "desktop-staging" && args.includes("--json")) {
          return { exitCode: 0, stdout: JSON.stringify({ body: options.channelBody ?? "" }), stderr: "" };
        }
        if (command === "gh" && args[1] === "view" && args[2] === "desktop-staging") {
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "gh" && args[1] === "edit") return { exitCode: 0, stdout: "", stderr: "" };
        return { exitCode: 1, stdout: "", stderr: `unexpected command ${key}` };
      }
    };
  }

  function resetInput(root: string, runner: CommandRunner, overrides: Partial<ReleaseResetStagingInput> = {}): ReleaseResetStagingInput {
    return {
      repoRoot: root,
      toBranch: "main",
      reason: "0.1 soak abandoned; shipping main again",
      confirmAbandon: "1.3.0-staging.2",
      dryRun: false,
      now: RESET_NOW,
      env: {},
      runner,
      ...overrides
    };
  }

  it("records old and new provenance on the pointer release without building or repointing", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const calls: CommandCall[] = [];
      const runner = resetRunner(calls);

      const result = await resetStagingLineage(resetInput(root, runner));

      expect(result).toEqual({
        from: {
          version: "1.3.0-staging.2",
          tag: "v1.3.0-staging.2",
          commit: ACTIVE_COMMIT,
          sourceBranch: "release/1.3"
        },
        to: { branch: "main" },
        reason: "0.1 soak abandoned; shipping main again",
        resetAt: "2026-07-08T12:00:00.000Z",
        applied: true
      });
      const edit = calls.find((call) => call.command === "gh" && call.args[1] === "edit");
      expect(edit?.args[2]).toBe("desktop-staging");
      const notes = edit?.args.at(-1) ?? "";
      expect(notes).toContain("Lineage-Reset: 2026-07-08T12:00:00.000Z");
      expect(notes).toContain(`Reset-From: 1.3.0-staging.2 (${ACTIVE_COMMIT}) source release/1.3`);
      expect(notes).toContain("Reset-To: main");
      expect(notes).toContain("Reset-Reason: 0.1 soak abandoned; shipping main again");
      expect(calls.some((call) => call.command === "bazel")).toBe(false);
      expect(calls.some((call) => call.command === "gh" && call.args[1] === "upload")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("keeps earlier reset records as an audit trail", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const calls: CommandCall[] = [];
      const runner = resetRunner(calls, {
        channelBody: [
          "Pointer-only desktop staging updater channel.",
          "",
          "Lineage-Reset: 2026-05-01T00:00:00Z",
          "Reset-From: 1.0.0-staging.4 (aaaa) source main",
          "Reset-To: release/1.0",
          "Reset-Reason: earlier abandon"
        ].join("\n")
      });

      await resetStagingLineage(resetInput(root, runner));

      const notes = calls.find((call) => call.command === "gh" && call.args[1] === "edit")?.args.at(-1) ?? "";
      expect(notes.indexOf("2026-07-08T12:00:00.000Z")).toBeLessThan(notes.indexOf("2026-05-01T00:00:00Z"));
      expect(notes).toContain("Reset-Reason: earlier abandon");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses a confirmation that does not name the active candidate", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const calls: CommandCall[] = [];
      const runner = resetRunner(calls);

      await expect(
        resetStagingLineage(resetInput(root, runner, { confirmAbandon: "1.3.0-staging.1" }))
      ).rejects.toThrow(/does not match the active staging candidate 1\.3\.0-staging\.2/);
      expect(calls.some((call) => call.command === "gh" && call.args[1] === "edit")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("requires a reason and a valid destination branch", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const calls: CommandCall[] = [];
      const runner = resetRunner(calls);

      await expect(resetStagingLineage(resetInput(root, runner, { reason: "  " }))).rejects.toThrow(/requires --reason/);
      await expect(resetStagingLineage(resetInput(root, runner, { toBranch: "hotfix/x" }))).rejects.toThrow(
        /Expected main or release\/X\.Y/
      );
      await expect(resetStagingLineage(resetInput(root, runner, { confirmAbandon: " " }))).rejects.toThrow(
        /requires --confirm-abandon/
      );
      expect(calls.some((call) => call.command === "gh" && call.args[1] === "edit")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses when the channel has no active candidate to abandon", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const calls: CommandCall[] = [];
      const runner = resetRunner(calls, { activeVersion: null });

      await expect(resetStagingLineage(resetInput(root, runner))).rejects.toThrow(/no active staging candidate/);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("rehearses without editing the pointer release under --dry-run", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-"));
    try {
      const calls: CommandCall[] = [];
      const runner = resetRunner(calls);

      const result = await resetStagingLineage(resetInput(root, runner, { dryRun: true }));

      expect(result.applied).toBe(false);
      expect(calls.some((call) => call.command === "gh" && call.args[1] === "edit")).toBe(false);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});

describe("release branch recut", () => {
  const OLD_TIP = "2222222222222222222222222222222222222222";
  const MAIN_TIP = "3333333333333333333333333333333333333333";

  function recutRunner(calls: CommandCall[], options: { branchOnly?: boolean; productionTag?: boolean; unreadableChannel?: boolean; branchChangedDuringBuild?: boolean } = {}): CommandRunner {
    let branchTip = OLD_TIP;
    return {
      async run(command, args, runOptions) {
        calls.push({ command, args, options: runOptions });
        const key = `${command} ${args.join(" ")}`;
        if (key === "git remote get-url origin") return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
        if (command === "git" && args[0] === "fetch") return { exitCode: 0, stdout: "", stderr: "" };
        if (key === "git rev-parse origin/main") return { exitCode: 0, stdout: `${MAIN_TIP}\n`, stderr: "" };
        if (key === "git show origin/main:VERSION") return { exitCode: 0, stdout: "0.3.0\n", stderr: "" };
        if (key === "git rev-parse FETCH_HEAD^{commit}") return { exitCode: 0, stdout: `${MAIN_TIP}\n`, stderr: "" };
        if (key === "git ls-remote origin refs/heads/release/0.3") return { exitCode: 0, stdout: `${branchTip}\trefs/heads/release/0.3\n`, stderr: "" };
        if (command === "git" && args[0] === "ls-remote" && args[1] === "--tags") {
          const pattern = args.at(-1) ?? "";
          if (pattern === "refs/tags/v0.3.*") {
            return options.productionTag
              ? { exitCode: 0, stdout: "deadbeef\trefs/tags/v0.3.0\n", stderr: "" }
              : { exitCode: 0, stdout: "", stderr: "" };
          }
          if (pattern.includes("v0.3.0-staging.10")) return { exitCode: 0, stdout: `${MAIN_TIP}\t${pattern}\n`, stderr: "" };
          if (pattern.startsWith("recut/release/0.3-")) return { exitCode: 0, stdout: "", stderr: "" };
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (isStagingChannelAssetsQuery(command, args)) {
          return options.unreadableChannel
            ? { exitCode: 1, stdout: "", stderr: "HTTP 503: Service unavailable" }
            : stagingChannelAssetsResponse(["latest-staging.json"]);
        }
        if (command === "gh" && args[0] === "release" && args[1] === "download") {
          const dirIndex = args.indexOf("--dir");
          writeFileSync(join(args[dirIndex + 1] ?? "", "latest-staging.json"), '{"version":"0.3.0-staging.10"}\n');
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (key.startsWith("gh release view v0.3.0-staging.10 ")) {
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              tagName: "v0.3.0-staging.10",
              targetCommitish: MAIN_TIP,
              publishedAt: "2026-09-05T00:00:00Z",
              body: "Staging updater manifest for v0.3.0-staging.10\n\nSource-Branch: main",
              isPrerelease: true
            }),
            stderr: ""
          };
        }
        if (key.startsWith("git log --no-merges")) {
          return options.branchOnly ? { exitCode: 0, stdout: `${OLD_TIP} branch-only fix\n`, stderr: "" } : { exitCode: 0, stdout: "", stderr: "" };
        }
        if (key.startsWith("git log --merges")) return { exitCode: 0, stdout: "", stderr: "" };
        if (key.startsWith("gh release view desktop-staging")) return { exitCode: 0, stdout: "", stderr: "" };
        if (key.startsWith("gh release edit desktop-staging")) return { exitCode: 0, stdout: "", stderr: "" };
        if (key.startsWith("git push origin --force-with-lease=")) {
          // The branch lands at the commit the refspec names — the composed
          // series version commit, not main's tip, which is what leaves the
          // moved branch still stating its own version.
          const refspec = args.find((arg) => arg.includes(":refs/heads/")) ?? "";
          branchTip = refspec.split(":")[0] || MAIN_TIP;
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (key.startsWith("git push origin refs/tags/recut/")) {
          if (options.branchChangedDuringBuild) branchTip = "4444444444444444444444444444444444444444";
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (key.startsWith("git tag -a") || key.startsWith("git push origin")) return { exitCode: 0, stdout: "", stderr: "" };
        const plumbing = cutVersionCommitPlumbing(command, args);
        if (plumbing) return plumbing;
        throw new Error(`unexpected command ${key}`);
      }
    };
  }

  it("archives the old tip before moving the branch and records the recut", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-recut-"));
    try {
      const calls: CommandCall[] = [];
      const result = await cutReleaseBranch({
        repoRoot: root,
        bump: "minor",
        version: "0.3.0",
        recut: true,
        reason: "include the latest feature",
        confirmRecut: "0.3.0-staging.10",
        confirmOldTip: OLD_TIP,
        env: {},
        runner: recutRunner(calls)
      });
      // The branch moves to the composed series version commit, whose parent is
      // main's tip — the same shape `cut` produces, so the moved branch still
      // states its own version instead of inheriting trunk's.
      expect(result.recut).toMatchObject({
        archiveTag: "recut/release/0.3-1",
        oldTip: OLD_TIP,
        newTip: CUT_VERSION_COMMIT,
        applied: true
      });
      expect(result.trunkCommit).toBe(MAIN_TIP);
      const composed = calls.find((call) => call.command === "git" && call.args[0] === "commit-tree");
      expect(composed?.args).toEqual(expect.arrayContaining(["-p", MAIN_TIP, "-m", "release: cut 0.3.0"]));
      const tagPush = calls.findIndex((call) => call.command === "git" && call.args.includes("refs/tags/recut/release/0.3-1"));
      const branchPush = calls.findIndex((call) => call.command === "git" && call.args.some((arg) => arg.includes("refs/heads/release/0.3")) && call.args.includes("--force-with-lease=refs/heads/release/0.3:" + OLD_TIP));
      expect(tagPush).toBeGreaterThanOrEqual(0);
      expect(branchPush).toBeGreaterThan(tagPush);
      const edit = calls.find((call) => call.command === "gh" && call.args[0] === "release" && call.args[1] === "edit");
      expect(edit?.args.at(-1)).toContain("Lineage-Recut: 2026-09");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses branch-only work, production history, and unreadable channel state", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-recut-"));
    try {
      const input = { repoRoot: root, bump: "minor" as const, version: "0.3.0", recut: true, reason: "move", confirmRecut: "0.3.0-staging.10", confirmOldTip: OLD_TIP, env: {} };
      await expect(cutReleaseBranch({ ...input, runner: recutRunner([], { branchOnly: true }) })).rejects.toThrow(/branch-only commit/);
      await expect(cutReleaseBranch({ ...input, runner: recutRunner([], { productionTag: true }) })).rejects.toThrow(/production release/);
      await expect(cutReleaseBranch({ ...input, runner: recutRunner([], { unreadableChannel: true }) })).rejects.toThrow(/channel is unreadable/);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("refuses when a pinned branch moves during the recut", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-recut-"));
    try {
      await expect(cutReleaseBranch({
        repoRoot: root,
        bump: "minor",
        version: "0.3.0",
        recut: true,
        reason: "include the latest feature",
        confirmRecut: "0.3.0-staging.10",
        confirmOldTip: OLD_TIP,
        env: {},
        runner: recutRunner([], { branchChangedDuringBuild: true })
      })).rejects.toThrow(/release refs changed before branch move/);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});
