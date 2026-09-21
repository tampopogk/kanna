import { execFileSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { nodeCommandRunner } from "../src/runtime/process";
import { cutReleaseBranch } from "../src/runtime/release-cut";
import type { CommandRunner } from "../src/runtime/process";

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

/**
 * Real git, mocked GitHub.
 *
 * A recut is git plumbing whose failure is invisible to a fully mocked runner —
 * a mock happily reports success while pushing a ref that carries the wrong
 * tree — so git runs for real against a real remote. Only the `gh` calls are
 * answered from a fixture, as an uninitialized staging channel, because the
 * test must not reach GitHub.
 */
function realGitMockedGithub(): CommandRunner {
  return {
    async run(command, args, options) {
      // The remote is a local bare repo, but the slug derived from it only ever
      // addresses the mocked `gh` calls, so name one git never has to resolve.
      if (command === "git" && args.join(" ") === "remote get-url origin") {
        return { exitCode: 0, stdout: "git@github.com:jemdiggity/kanna.git\n", stderr: "" };
      }
      if (command === "git") return nodeCommandRunner.run(command, args, options);
      if (command === "gh") {
        // `release view desktop-staging` 404s: the channel has no candidate, so
        // the recut's confirmation is the literal "empty".
        if (args[0] === "release" && args[1] === "view") {
          return { exitCode: 1, stdout: "", stderr: "release not found\n" };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      }
      throw new Error(`unexpected command in recut fixture: ${command} ${args.join(" ")}`);
    }
  };
}

describe("release cut --recut keeps the series version", () => {
  it("recuts a branch it cut, landing the version commit on the new main tip", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-recut-version-"));
    try {
      const { repoRoot } = createOriginAndClone(root);
      const runner = realGitMockedGithub();

      const cut = await cutReleaseBranch({ repoRoot, bump: "minor", version: "0.3.0", env: process.env, runner });

      // main moves on, which is the whole reason to recut.
      writeFileSync(join(repoRoot, "trunk-work.txt"), "later trunk work\n");
      git(repoRoot, "add", "-A");
      git(repoRoot, "commit", "-qm", "more trunk work");
      git(repoRoot, "push", "-q", "origin", "main");
      const newTrunkTip = git(repoRoot, "rev-parse", "HEAD");

      const recut = await cutReleaseBranch({
        repoRoot,
        bump: "minor",
        version: "0.3.0",
        recut: true,
        reason: "series moved to pick up later trunk work",
        confirmRecut: "empty",
        confirmOldTip: cut.commit,
        env: process.env,
        runner
      });

      // Before this, the freshly cut branch was un-recuttable: its own
      // `release: cut 0.3.0` commit is branch-only by construction, so the
      // hygiene gate counted it as work a recut would lose.
      expect(recut.recut?.applied).toBe(true);
      expect(recut.recut?.oldTip).toBe(cut.commit.toLowerCase());
      expect(recut.commit).toBe(recut.recut?.newTip);
      expect(recut.trunkCommit).toBe(newTrunkTip);

      git(repoRoot, "fetch", "-q", "origin", "release/0.3");
      const branchTip = git(repoRoot, "rev-parse", "FETCH_HEAD");
      expect(branchTip).toBe(recut.commit);
      // The moved branch still states its series version. Pushing main's tip
      // bare left trunk's VERSION there, and the next ship refused the branch
      // as out of series.
      expect(git(repoRoot, "show", `${branchTip}:VERSION`)).toBe("0.3.0");
      expect(git(repoRoot, "show", `${branchTip}:VERSION_RC`)).toBe("1");
      expect(git(repoRoot, "rev-parse", `${branchTip}^`)).toBe(newTrunkTip);
      expect(git(repoRoot, "log", "-1", "--format=%s", branchTip)).toBe("release: cut 0.3.0");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("still refuses a branch carrying a genuine backport", async () => {
    const root = await mkdtemp(join(tmpdir(), "kd-release-recut-backport-"));
    try {
      const { repoRoot } = createOriginAndClone(root);
      const runner = realGitMockedGithub();
      const cut = await cutReleaseBranch({ repoRoot, bump: "minor", version: "0.3.0", env: process.env, runner });

      // A real fix on the branch and nowhere else. The exemption is for kd's
      // own version commit only; this must refuse exactly as it did before.
      git(repoRoot, "fetch", "-q", "origin", "release/0.3");
      git(repoRoot, "checkout", "-q", "-B", "backport", "FETCH_HEAD");
      writeFileSync(join(repoRoot, "hotfix.txt"), "a fix that exists only here\n");
      git(repoRoot, "add", "-A");
      git(repoRoot, "commit", "-qm", "fix something on the branch only");
      git(repoRoot, "push", "-q", "origin", "HEAD:refs/heads/release/0.3");
      const branchTip = git(repoRoot, "rev-parse", "HEAD");
      git(repoRoot, "checkout", "-q", "main");

      writeFileSync(join(repoRoot, "trunk-work.txt"), "later trunk work\n");
      git(repoRoot, "add", "-A");
      git(repoRoot, "commit", "-qm", "more trunk work");
      git(repoRoot, "push", "-q", "origin", "main");

      await expect(
        cutReleaseBranch({
          repoRoot,
          bump: "minor",
          version: "0.3.0",
          recut: true,
          reason: "trying to move a branch that carries real work",
          confirmRecut: "empty",
          confirmOldTip: branchTip,
          env: process.env,
          runner
        })
      ).rejects.toThrow(/branch-only commit\(s\) not present on origin\/main[\s\S]*fix something on the branch only/);
      expect(cut.commit).not.toBe(branchTip);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});

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
