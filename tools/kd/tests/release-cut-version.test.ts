import { execFileSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { nodeCommandRunner } from "../src/runtime/process";
import { cutReleaseBranch } from "../src/runtime/release";

/**
 * Cutting a series is the moment its version is set, and the commit that does
 * it is built with git plumbing against a temporary index rather than by
 * checking the branch out — `kd release cut` runs from whatever worktree the
 * operator is in, which in a Kanna task is one on an unrelated branch.
 *
 * Plumbing is exactly the kind of code a mocked runner will happily accept
 * while producing nothing, so these cases drive the real thing against a real
 * repository with a real remote.
 */
function git(cwd: string, ...args: string[]): string {
  return execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    env: { ...process.env, GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@t", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@t" }
  }).trim();
}

function createOriginAndClone(root: string): { repoRoot: string; origin: string } {
  const origin = join(root, "origin.git");
  const repoRoot = join(root, "repo");
  git(root, "init", "-q", "--bare", origin);
  git(root, "clone", "-q", origin, repoRoot);

  mkdirSync(join(repoRoot, "apps", "desktop", "src-tauri"), { recursive: true });
  writeFileSync(join(repoRoot, "VERSION"), "0.2.0\n");
  writeFileSync(join(repoRoot, "apps", "desktop", "src-tauri", "tauri.conf.json"), '{\n  "version": "0.2.0"\n}\n');
  writeFileSync(join(repoRoot, "apps", "desktop", "src-tauri", "Cargo.toml"), '[package]\nname = "kanna"\nversion = "0.2.0"\n');
  git(repoRoot, "add", "-A");
  git(repoRoot, "commit", "-qm", "base");
  git(repoRoot, "branch", "-M", "main");
  git(repoRoot, "push", "-q", "-u", "origin", "main");
  return { repoRoot, origin };
}

describe("release cut sets the series version", () => {
  it("pushes a branch whose tip commits VERSION and the candidate counter", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-cut-version-"));
    try {
      const { repoRoot } = createOriginAndClone(root);
      const trunkTip = git(repoRoot, "rev-parse", "HEAD");

      const result = await cutReleaseBranch({
        repoRoot,
        bump: "minor",
        version: "0.3.0",
        env: process.env,
        runner: nodeCommandRunner
      });

      expect(result.branch).toBe("release/0.3");
      expect(result.version).toBe("0.3.0");
      expect(result.trunkCommit).toBe(trunkTip);
      expect(result.commit).not.toBe(trunkTip);

      // The branch exists on the remote at exactly the commit kd reported.
      git(repoRoot, "fetch", "-q", "origin", "release/0.3");
      expect(git(repoRoot, "rev-parse", "FETCH_HEAD")).toBe(result.commit);

      // The branch states the version it will ship under, and its candidate
      // counter starts at 1 — that pair is what a staging build reads.
      expect(git(repoRoot, "show", `${result.commit}:VERSION`)).toBe("0.3.0");
      expect(git(repoRoot, "show", `${result.commit}:VERSION_RC`)).toBe("1");
      expect(git(repoRoot, "show", `${result.commit}:apps/desktop/src-tauri/tauri.conf.json`)).toContain('"version": "0.3.0"');
      expect(git(repoRoot, "show", `${result.commit}:apps/desktop/src-tauri/Cargo.toml`)).toContain('version = "0.3.0"');

      // It is a child of trunk's tip, carrying everything else unchanged.
      expect(git(repoRoot, "rev-parse", `${result.commit}^`)).toBe(trunkTip);
      expect(git(repoRoot, "log", "-1", "--format=%s", result.commit)).toBe("release: cut 0.3.0");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("leaves the caller's worktree completely untouched", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-cut-worktree-"));
    try {
      const { repoRoot } = createOriginAndClone(root);
      const headBefore = git(repoRoot, "rev-parse", "HEAD");
      const branchBefore = git(repoRoot, "rev-parse", "--abbrev-ref", "HEAD");

      await cutReleaseBranch({
        repoRoot,
        bump: "minor",
        version: "0.3.0",
        env: process.env,
        runner: nodeCommandRunner
      });

      // No checkout, no staged index, no edited files: cutting a branch the
      // operator is not on must not disturb the one they are on.
      expect(git(repoRoot, "status", "--porcelain")).toBe("");
      expect(git(repoRoot, "rev-parse", "HEAD")).toBe(headBefore);
      expect(git(repoRoot, "rev-parse", "--abbrev-ref", "HEAD")).toBe(branchBefore);
      expect(git(repoRoot, "show", "HEAD:VERSION")).toBe("0.2.0");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});
