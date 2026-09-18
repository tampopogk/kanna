import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  symlinkSync,
  writeFileSync
} from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { cleanWorkspace } from "../src/runtime/clean";
import type { CommandRunner } from "../src/runtime/process";
import { buildConfigSchemaPages } from "../src/runtime/pages";
import {
  bazelTargetForLabel,
  bumpVersion,
  releaseAssetName,
  releaseRepoSlug,
  signedAppTargetForLabel,
  updaterAssetName,
  updaterBundleTargetForLabel,
  updaterPlatformKey,
  updaterSignatureName
} from "../src/runtime/release";

function bazelRunner(outputBase: string): CommandRunner {
  return {
    async run(command, args) {
      expect(command).toBe("bazel");
      expect(args).toEqual(["info", "output_base"]);
      return { exitCode: 0, stdout: `${outputBase}\n`, stderr: "" };
    }
  };
}

describe("clean runtime", () => {
  it("removes Bazel's configured output base and workspace artifacts without removing shared caches by default", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-clean-"));
    const home = join(root, "home");
    const repo = join(root, "repo");
    const bazelOutputBase = join(root, "custom-bazel-output-root", "output-base");
    const sharedRust = join(home, "Library", "Caches", "kanna", "rust-build");
    for (const dir of [
      join(repo, ".build"),
      join(repo, "apps", "desktop", "src-tauri", "target"),
      bazelOutputBase,
      sharedRust
    ]) {
      mkdirSync(dir, { recursive: true });
      writeFileSync(join(dir, "artifact.txt"), "x");
    }

    const result = await cleanWorkspace({
      repoRoot: repo,
      homeDir: home,
      runner: bazelRunner(bazelOutputBase),
      all: false,
      dry: false,
      sharedRustBuild: false
    });

    expect(result.bazelOutputBase).toBe(bazelOutputBase);
    expect(result.removals.every((removal) => removal.outcome !== "failed")).toBe(true);
    expect(result.removals.find((removal) => removal.path === bazelOutputBase)).toEqual({
      path: bazelOutputBase,
      outcome: "removed"
    });
    expect(existsSync(bazelOutputBase)).toBe(false);
    expect(existsSync(join(repo, ".build"))).toBe(false);
    expect(existsSync(sharedRust)).toBe(true);
    await rm(root, { recursive: true, force: true });
  });

  it("removes the exact external build target recorded by the workspace link", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-clean-external-"));
    const repo = join(root, "task-abcd1234-2");
    const externalBuild = join(root, "external", "task-abcd1234-2");
    mkdirSync(repo, { recursive: true });
    mkdirSync(externalBuild, { recursive: true });
    writeFileSync(join(externalBuild, "artifact.txt"), "x");
    symlinkSync(externalBuild, join(repo, ".build"));

    const result = await cleanWorkspace({
      repoRoot: repo,
      homeDir: join(root, "home"),
      runner: bazelRunner(join(root, "bazel-output")),
      all: false,
      dry: false,
      sharedRustBuild: false
    });

    const externalCanonical = join(realpathSync(join(root, "external")), "task-abcd1234-2");
    const recordPath = join(repo, ".kanna-external-build-target");
    expect(result.removals).toEqual([
      { path: externalCanonical, outcome: "removed" },
      { path: join(repo, ".build"), outcome: "removed" },
      { path: recordPath, outcome: "absent" },
      { path: join(repo, "apps", "desktop", "src-tauri", "target"), outcome: "absent" },
      { path: join(root, "bazel-output"), outcome: "absent" }
    ]);
    expect(existsSync(externalBuild)).toBe(false);
    expect(existsSync(join(repo, ".build"))).toBe(false);
    await rm(root, { recursive: true, force: true });
  });

  it("skips an external build target belonging to a sibling workspace but keeps cleaning the rest", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-clean-mismatch-"));
    const repo = join(root, "task-current");
    const siblingBuild = join(root, "external", "task-sibling");
    const tauriTarget = join(repo, "apps", "desktop", "src-tauri", "target");
    mkdirSync(repo, { recursive: true });
    mkdirSync(siblingBuild, { recursive: true });
    mkdirSync(tauriTarget, { recursive: true });
    writeFileSync(join(siblingBuild, "artifact.txt"), "keep");
    writeFileSync(join(tauriTarget, "artifact.txt"), "x");
    symlinkSync(siblingBuild, join(repo, ".build"));
    const bazelOutputBase = join(root, "bazel-output");
    mkdirSync(bazelOutputBase, { recursive: true });

    const result = await cleanWorkspace({
      repoRoot: repo,
      homeDir: join(root, "home"),
      runner: bazelRunner(bazelOutputBase),
      all: true,
      dry: false,
      sharedRustBuild: false
    });

    const buildFailure = result.removals.find((removal) => removal.path === join(repo, ".build"));
    expect(buildFailure?.outcome).toBe("failed");
    expect(buildFailure?.error).toMatch(/Refusing to clean external \.build target.*expected an exact workspace target/);
    expect(readFileSync(join(siblingBuild, "artifact.txt"), "utf8")).toBe("keep");
    expect(lstatSync(join(repo, ".build")).isSymbolicLink()).toBe(true);

    // Everything unrelated to the mismatched pointer still gets cleaned.
    expect(existsSync(tauriTarget)).toBe(false);
    expect(result.removals.find((removal) => removal.path === tauriTarget)?.outcome).toBe("removed");
    expect(existsSync(bazelOutputBase)).toBe(false);
    expect(result.removals.find((removal) => removal.path === bazelOutputBase)?.outcome).toBe("removed");
    await rm(root, { recursive: true, force: true });
  });

  it("reports an unavailable recorded external build, preserves its authoritative link, and keeps cleaning the rest", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-clean-dangling-"));
    const repo = join(root, "task-dangling");
    const tauriTarget = join(repo, "apps", "desktop", "src-tauri", "target");
    mkdirSync(tauriTarget, { recursive: true });
    symlinkSync(join(root, "external", "task-dangling"), join(repo, ".build"));

    const result = await cleanWorkspace({
      repoRoot: repo,
      homeDir: join(root, "home"),
      runner: bazelRunner(join(root, "bazel-output")),
      all: false,
      dry: false,
      sharedRustBuild: false
    });

    const buildFailure = result.removals.find((removal) => removal.path === join(repo, ".build"));
    expect(buildFailure?.outcome).toBe("failed");
    expect(buildFailure?.error).toMatch(/Cannot clean external \.build target.*recorded target is unavailable.*preserving/);
    expect(lstatSync(join(repo, ".build")).isSymbolicLink()).toBe(true);
    expect(existsSync(tauriTarget)).toBe(false);
    expect(result.removals.find((removal) => removal.path === tauriTarget)?.outcome).toBe("removed");
    await rm(root, { recursive: true, force: true });
  });

  it("remains idempotent when no workspace build path is recorded", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-clean-absent-"));
    const repo = join(root, "task-absent");
    mkdirSync(repo, { recursive: true });

    const bazelOutputBase = join(root, "bazel-output");
    const result = await cleanWorkspace({
      repoRoot: repo,
      homeDir: join(root, "home"),
      runner: bazelRunner(bazelOutputBase),
      all: false,
      dry: false,
      sharedRustBuild: false
    });

    expect(result.bazelOutputBase).toBe(bazelOutputBase);
    expect(result.removals.every((removal) => removal.outcome === "absent")).toBe(true);
    expect(result.removals.map((removal) => removal.path)).toEqual([
      join(repo, ".build"),
      join(repo, ".kanna-external-build-target"),
      join(repo, "apps", "desktop", "src-tauri", "target"),
      bazelOutputBase
    ]);
    await rm(root, { recursive: true, force: true });
  });

  it("still reclaims .build and the other repo-local paths when Bazel is unavailable", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-clean-no-bazel-"));
    const repo = join(root, "repo");
    mkdirSync(join(repo, ".build"), { recursive: true });
    const runner: CommandRunner = {
      async run() {
        throw new Error("spawn bazel ENOENT");
      }
    };

    const result = await cleanWorkspace({ repoRoot: repo, runner, all: false, dry: false, sharedRustBuild: false });

    expect(result.bazelOutputBase).toBeUndefined();
    const bazelFailure = result.removals.find((removal) => removal.outcome === "failed");
    expect(bazelFailure?.path).toBe("Bazel output base");
    expect(bazelFailure?.error).toMatch(/Cannot resolve Bazel output base.*could not run.*ENOENT/);
    expect(existsSync(join(repo, ".build"))).toBe(false);
    expect(result.removals.find((removal) => removal.path === join(repo, ".build"))?.outcome).toBe("removed");
    await rm(root, { recursive: true, force: true });
  });

  it("fails only the Bazel candidate when Bazel cannot report its output base", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-clean-bazel-failure-"));
    const repo = join(root, "repo");
    mkdirSync(join(repo, ".build"), { recursive: true });
    const failedRunner: CommandRunner = {
      async run() {
        return { exitCode: 37, stdout: "", stderr: "could not read bazelrc" };
      }
    };
    const emptyRunner: CommandRunner = {
      async run() {
        return { exitCode: 0, stdout: "\n", stderr: "" };
      }
    };

    const failedResult = await cleanWorkspace({
      repoRoot: repo,
      runner: failedRunner,
      all: false,
      dry: false,
      sharedRustBuild: false
    });
    expect(failedResult.removals.find((removal) => removal.outcome === "failed")?.error).toMatch(
      /Cannot resolve Bazel output base.*could not read bazelrc/
    );
    expect(existsSync(join(repo, ".build"))).toBe(false);
    mkdirSync(join(repo, ".build"), { recursive: true });

    const emptyResult = await cleanWorkspace({
      repoRoot: repo,
      runner: emptyRunner,
      all: false,
      dry: false,
      sharedRustBuild: false
    });
    expect(emptyResult.removals.find((removal) => removal.outcome === "failed")?.error).toMatch(
      /Cannot resolve Bazel output base.*returned no path/
    );
    expect(existsSync(join(repo, ".build"))).toBe(false);
    await rm(root, { recursive: true, force: true });
  });

  it("continues past a removal failure and still attempts the remaining candidates", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-clean-partial-failure-"));
    const repo = join(root, "repo");
    const tauriDir = join(repo, "apps", "desktop", "src-tauri");
    mkdirSync(join(tauriDir, "target"), { recursive: true });
    writeFileSync(join(tauriDir, "target", "artifact.txt"), "x");
    mkdirSync(join(repo, ".build"), { recursive: true });
    writeFileSync(join(repo, ".build", "artifact.txt"), "x");
    const bazelOutputBase = join(root, "bazel-output");
    mkdirSync(bazelOutputBase, { recursive: true });
    chmodSync(tauriDir, 0o500);

    try {
      const result = await cleanWorkspace({
        repoRoot: repo,
        homeDir: join(root, "home"),
        runner: bazelRunner(bazelOutputBase),
        all: false,
        dry: false,
        sharedRustBuild: false
      });

      const targetOutcome = result.removals.find((removal) => removal.path === join(tauriDir, "target"));
      expect(targetOutcome?.outcome).toBe("failed");
      expect(targetOutcome?.error).toBeTruthy();

      expect(existsSync(join(repo, ".build"))).toBe(false);
      expect(result.removals.find((removal) => removal.path === join(repo, ".build"))?.outcome).toBe("removed");
      expect(existsSync(bazelOutputBase)).toBe(false);
      expect(result.removals.find((removal) => removal.path === bazelOutputBase)?.outcome).toBe("removed");
    } finally {
      chmodSync(tauriDir, 0o700);
      await rm(root, { recursive: true, force: true });
    }
  });
});

describe("pages runtime", () => {
  it("builds the config schema Pages artifact", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-pages-"));
    mkdirSync(join(root, ".kanna"), { recursive: true });
    mkdirSync(join(root, "crates", "kanna-tool-catalog", "src"), { recursive: true });
    writeFileSync(
      join(root, ".kanna", "config.schema.json"),
      '{"type":"object","properties":{"workflow":{"type":"string"}}}\n'
    );
    writeFileSync(
      join(root, "crates", "kanna-tool-catalog", "src", "catalog.json"),
      JSON.stringify({
        guides: [{ sections: [{ body: "Catalog-owned meaning", schemaPaths: ["/properties/workflow"] }] }]
      })
    );

    const [schema, cname] = buildConfigSchemaPages({ repoRoot: root, outDir: join(root, "out") });

    expect(JSON.parse(readFileSync(schema, "utf8"))).toEqual({
      type: "object",
      properties: { workflow: { type: "string", description: "Catalog-owned meaning" } }
    });
    expect(readFileSync(cname, "utf8")).toBe("schemas.kanna.build\n");
    await rm(root, { recursive: true, force: true });
  });
});

describe("release runtime", () => {
  it("builds release names and targets without shell scripts", () => {
    expect(bumpVersion("1.2.3", "major")).toBe("2.0.0");
    expect(bumpVersion("1.2.3", "minor")).toBe("1.3.0");
    expect(bumpVersion("1.2.3", "patch")).toBe("1.2.4");
    expect(releaseAssetName("1.2.4", "arm64")).toBe("Kanna_1.2.4_arm64.dmg");
    expect(updaterAssetName("1.2.4", "x86_64")).toBe("Kanna_1.2.4_x86_64.app.tar.gz");
    expect(updaterSignatureName("1.2.4", "x86_64")).toBe("Kanna_1.2.4_x86_64.app.tar.gz.sig");
    expect(updaterPlatformKey("arm64")).toBe("darwin-aarch64");
    expect(bazelTargetForLabel("arm64", true)).toBe("//:kanna_signed_dmg_release_arm64");
    expect(bazelTargetForLabel("arm64", false)).toBe("//:kanna_notarized_dmg_release_arm64");
    expect(signedAppTargetForLabel("x86_64")).toBe("//:kanna_signed_app_release_x86_64");
    expect(updaterBundleTargetForLabel("x86_64")).toBe("//:kanna_updater_bundle_release_x86_64");
    expect(releaseRepoSlug("git@github.com:jemdiggity/kanna.git")).toBe("jemdiggity/kanna");
  });
});
