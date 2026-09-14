import { createHash, generateKeyPairSync } from "node:crypto";
import { readFileSync } from "node:fs";
import { mkdir, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { parseCliArgs } from "../cli.js";
import { resolveKdEnvironment } from "./environment.js";
import * as mobileOtaRuntime from "./mobile-ota.js";
import {
  buildMobileOtaPublishPlan,
  computeExpoUpdateId,
  executeMobileOtaDoctorWithContext,
  executeMobileOtaPublishWithContext,
  executeMobileOtaProvisionSecretWithContext,
  resolveMobileRuntimeVersion,
} from "./mobile-ota.js";
import type { CommandRunner } from "./process.js";

const tempDirs: string[] = [];
const repositoryCertificatePath = fileURLToPath(
  new URL("../../../../apps/mobile/certs/ota-codesign.pem", import.meta.url)
);
const acceptOtaCertificate = async () => ({
  keyId: "kanna-mobile-ota-v1" as const,
  codeSigning: true as const,
  validFrom: "Jul 19 19:34:32 2026 GMT",
  validTo: "Jul 16 19:34:32 2036 GMT",
});

type MobileOtaProvisionExecutor = (
  input: { staging: boolean; production: boolean },
  context: {
    repoRoot: string;
    env: NodeJS.ProcessEnv;
    runner: CommandRunner;
    request?: (input: {
      url: string;
      method: "POST";
      headers: Record<string, string>;
      body: unknown;
    }) => Promise<{ ok: boolean; status: number; body: string }>;
  }
) => Promise<{ ok: boolean; message: string; data?: unknown }>;

function getMobileOtaProvisionExecutor(): MobileOtaProvisionExecutor {
  const executor = Reflect.get(mobileOtaRuntime, "executeMobileOtaProvisionWithContext") as unknown;
  expect(executor).toBeTypeOf("function");
  return executor as MobileOtaProvisionExecutor;
}

afterEach(async () => {
  await Promise.all(tempDirs.map((dir) => rm(dir, { recursive: true, force: true })));
  tempDirs.length = 0;
});

async function makeRepoFixture(
  options: { certificatePem?: string } = {}
): Promise<string> {
  const repoRoot = await mkdtemp(join(tmpdir(), "kanna-kd-ota-"));
  tempDirs.push(repoRoot);
  await mkdir(join(repoRoot, "apps/mobile/src"), { recursive: true });
  await mkdir(join(repoRoot, "apps/mobile/certs"), { recursive: true });
  await mkdir(join(repoRoot, "apps/mobile/dist"), { recursive: true });
  await writeFile(join(repoRoot, "apps/mobile/VERSION"), "1.0.0\n");
  await writeFile(
    join(repoRoot, "apps/mobile/certs/ota-codesign.pem"),
    options.certificatePem ?? (await readFile(repositoryCertificatePath, "utf8"))
  );
  await writeFile(
    join(repoRoot, "apps/mobile/src/mobileEnvironments.json"),
    JSON.stringify({
      dev: { runtimeVersion: "1.0.0" },
      staging: { runtimeVersion: "1.0.0" },
      prod: { runtimeVersion: "1.0.0" },
    })
  );
  await mkdir(join(repoRoot, "apps/mobile/dist/_expo/static/js/ios"), { recursive: true });
  await mkdir(join(repoRoot, "apps/mobile/dist/assets"), { recursive: true });
  await writeFile(
    join(repoRoot, "apps/mobile/dist/metadata.json"),
    JSON.stringify({
      fileMetadata: {
        ios: {
          bundle: "_expo/static/js/ios/main.hbc",
          assets: [{ path: "assets/icon.png", ext: "png" }],
        },
      },
    })
  );
  await writeFile(join(repoRoot, "apps/mobile/dist/_expo/static/js/ios/main.hbc"), "bundle bytes");
  await writeFile(join(repoRoot, "apps/mobile/dist/assets/icon.png"), "png bytes");
  return repoRoot;
}

const HEAD_COMMIT = "9".repeat(40);
const SHORT_HEAD_COMMIT = HEAD_COMMIT.slice(0, 12);

/**
 * `resolveSourceRef` runs `git status --porcelain` then `git rev-parse` per ref,
 * so the publish tests have to answer both rather than a blanket exit 0.
 */
function gitResult(
  args: string[],
  options: { status?: string; commits?: Record<string, string> } = {}
): { exitCode: number; stdout: string; stderr: string } {
  if (args[0] === "branch") return { exitCode: 0, stdout: "main", stderr: "" };
  if (args[0] === "remote") return { exitCode: 0, stdout: "https://github.com/example/kanna.git", stderr: "" };
  if (args[0] === "ls-remote") return { exitCode: 0, stdout: HEAD_COMMIT + "\t" + args.at(-1), stderr: "" };
  if (args[0] === "status") {
    return { exitCode: 0, stdout: options.status ?? "", stderr: "" };
  }
  if (args[0] === "rev-parse") {
    const ref = args[args.length - 1].replace("^{commit}", "");
    const commit = (options.commits ?? { HEAD: HEAD_COMMIT })[ref];
    return commit
      ? { exitCode: 0, stdout: `${commit}\n`, stderr: "" }
      : { exitCode: 1, stdout: "", stderr: "" };
  }
  return { exitCode: 0, stdout: "", stderr: "" };
}

/** A runner that answers git, the Expo export, and the Expo public config. */
function publishRunner(
  repoRoot: string,
  options: {
    commits?: Record<string, string>;
    onGcloud?: (args: string[]) => { exitCode: number; stdout: string; stderr: string };
  } = {}
): CommandRunner {
  return {
    async run(command, args) {
      if (command === "git") return gitResult(args, { commits: options.commits });
      if (command === "gh") return { exitCode: 0, stdout: JSON.stringify({ assets: [] }), stderr: "" };
      if (command === "gcloud" && args.at(-1)?.endsWith("kanna-source.json")) {
        return { exitCode: 0, stdout: JSON.stringify({ updateId: args.at(-1)!.split("/").at(-2), ref: "main", commit: HEAD_COMMIT, shortCommit: SHORT_HEAD_COMMIT, releaseVersion: "1.0.0" }), stderr: "" };
      }
      if (command === "pnpm" && args.includes("export")) {
        await writeMinimalSdk57Export(repoRoot, args[args.indexOf("--platform") + 1] as "ios" | "android");
        return { exitCode: 0, stdout: "", stderr: "" };
      }
      if (command === "pnpm" && args.includes("config")) {
        return {
          exitCode: 0,
          stdout: JSON.stringify({ name: "Kanna", runtimeVersion: "1.0.0" }),
          stderr: "",
        };
      }
      if (command === "gcloud" && options.onGcloud) return options.onGcloud(args);
      if (command === "gcloud" && args[1] === "cat") {
        return { exitCode: 1, stdout: "", stderr: "not found: 404" };
      }
      return { exitCode: 0, stdout: "", stderr: "" };
    },
  };
}

async function writeMinimalSdk57Export(repoRoot: string, platform: "ios" | "android" = "ios"): Promise<void> {
  await mkdir(join(repoRoot, "apps/mobile/dist/bundles"), { recursive: true });
  await writeFile(
    join(repoRoot, "apps/mobile/dist/metadata.json"),
    JSON.stringify({
      fileMetadata: { [platform]: { bundle: "bundles/main.hbc", assets: [] } },
    })
  );
  await writeFile(join(repoRoot, "apps/mobile/dist/bundles/main.hbc"), "bundle");
}

describe("kd mobile OTA", () => {
  it("parses publish, status, doctor, and preflight commands under mobile ota", () => {
    expect(parseCliArgs(["mobile", "ota", "publish", "--staging", "--dry-run"])).toEqual({
      taskId: "mobile.ota.publish",
      input: { staging: true, production: false, dryRun: true, rollbackTo: undefined, ref: undefined },
    });
    expect(parseCliArgs(["mobile", "ota", "publish", "--production", "--ref", "release/0.2"])).toEqual({
      taskId: "mobile.ota.publish",
      input: {
        staging: false,
        production: true,
        dryRun: false,
        rollbackTo: undefined,
        ref: "release/0.2",
      },
    });
    expect(() => parseCliArgs(["mobile", "ota", "publish", "--production", "--ref"])).toThrow(
      "--ref requires a value"
    );
    expect(parseCliArgs(["mobile", "ota", "status", "--production"])).toEqual({
      taskId: "mobile.ota.status",
      input: { staging: false, production: true },
    });
    expect(parseCliArgs(["mobile", "ota", "provision", "--staging"])).toEqual({
      taskId: "mobile.ota.provision",
      input: { staging: true, production: false },
    });
    expect(parseCliArgs(["mobile", "ota", "provision-secret", "--staging", "--key-path", "/tmp/key.pem"])).toEqual({
      taskId: "mobile.ota.provision-secret",
      input: { staging: true, production: false, keyPath: "/tmp/key.pem" },
    });
    expect(parseCliArgs(["mobile", "ota", "doctor", "--staging"])).toEqual({
      taskId: "mobile.ota.doctor",
      input: { staging: true, production: false },
    });
    expect(parseCliArgs(["mobile", "ota", "preflight", "--production"])).toEqual({
      taskId: "mobile.ota.doctor",
      input: { staging: false, production: true },
    });
  });

  it("extends environment identity with OTA bucket and channel", () => {
    expect(resolveKdEnvironment("staging")).toMatchObject({
      otaBucket: "kanna-staging.firebasestorage.app",
      otaChannel: "staging",
    });
    expect(resolveKdEnvironment("prod")).toMatchObject({
      otaBucket: "kanna-build.firebasestorage.app",
      otaChannel: "production",
    });
  });

  it("resolves the mobile runtime version from mobileEnvironments.json", async () => {
    const repoRoot = await makeRepoFixture();
    await expect(resolveMobileRuntimeVersion(repoRoot, "staging")).resolves.toBe("1.0.0");
    await expect(resolveMobileRuntimeVersion(repoRoot, "prod")).resolves.toBe("1.0.0");
  });

  it("computes a deterministic Expo update ID from metadata.json bytes", () => {
    expect(computeExpoUpdateId(Buffer.from("{\"hello\":\"world\"}"))).toBe(
      "93a23971-a914-e5ea-cbf0-a8d25154cda3"
    );
  });

  it("builds a dry-run publish plan without uploading to GCS", async () => {
    const repoRoot = await makeRepoFixture();
    const bundleKey = createHash("sha256").update("bundle bytes").digest("base64url");
    const assetKey = createHash("sha256").update("png bytes").digest("base64url");
    const expectedUpdateId = computeExpoUpdateId(Buffer.from(JSON.stringify({
      fileMetadata: {
        ios: {
          bundle: `bundles/${bundleKey}.hbc`,
          assets: [{ path: `assets/${assetKey}`, ext: "png" }],
        },
      },
      kanna: { releaseVersion: "1.0.0" },
    })));
    const plan = await buildMobileOtaPublishPlan({
      repoRoot,
      environment: "staging",
      distDir: join(repoRoot, "apps/mobile/dist"),
      dryRun: true,
    });

    expect(plan).toMatchObject({
      bucket: "kanna-staging.firebasestorage.app",
      channel: "staging",
      runtimeVersion: "1.0.0",
      releaseVersion: "1.0.0",
      updateId: expectedUpdateId,
      pointerObject: "ota/ios/1.0.0/channels/staging.json",
    });
    expect(plan.commands.map((command) => command.command)).toEqual([
      "pnpm",
      "pnpm",
      "gcloud",
      "gcloud",
    ]);
    expect(plan.commands[0]?.args).toContain("export");
    expect(plan.commands[1]?.args).toEqual([
      "exec",
      "expo",
      "config",
      "--type",
      "public",
      "--json",
    ]);
    expect(plan.commands[2]?.args).toContain("--recursive");
    expect(plan.commands[3]?.args).toContain("gs://kanna-staging.firebasestorage.app/ota/ios/1.0.0/channels/staging.json");
  });

  it("checks git cleanliness and runs export before publishing in dry-run mode", async () => {
    const repoRoot = await makeRepoFixture();
    const expoPublicConfig = JSON.stringify({
      name: "Kanna Staging",
      runtimeVersion: "1.0.0",
      extra: { kanna: { appEnv: "staging" } },
    });
    const calls: Array<{
      command: string;
      args: string[];
      cwd?: string;
      env?: NodeJS.ProcessEnv;
    }> = [];
    const runner: CommandRunner = {
      async run(command, args, options) {
        calls.push({ command, args, cwd: options?.cwd, env: options?.env });
        if (command === "git") return gitResult(args);
        if (command === "gh") return { exitCode: 0, stdout: JSON.stringify({ assets: [] }), stderr: "" };
        if (command === "pnpm" && args.includes("export")) {
          await mkdir(join(repoRoot, "apps/mobile/dist/bundles"), { recursive: true });
          await writeFile(
            join(repoRoot, "apps/mobile/dist/metadata.json"),
            JSON.stringify({
              fileMetadata: {
                ios: {
                  bundle: "bundles/main.hbc",
                  assets: [],
                },
              },
            })
          );
          await writeFile(join(repoRoot, "apps/mobile/dist/bundles/main.hbc"), "bundle");
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "pnpm" && args.includes("config")) {
          return { exitCode: 0, stdout: expoPublicConfig, stderr: "" };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      },
    };

    const result = await executeMobileOtaPublishWithContext(
      {
        staging: true,
        production: false,
        dryRun: true,
      },
      { repoRoot, env: {}, runner }
    );

    expect(result.ok).toBe(true);
    expect(calls[0]).toMatchObject({ command: "git", args: ["status", "--porcelain"], cwd: repoRoot });
    expect(calls[1]).toMatchObject({
      command: "git",
      args: ["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
      cwd: repoRoot,
    });
    expect(calls.find(call => call.args.includes("export"))).toMatchObject({ command: "pnpm", cwd: join(repoRoot, "apps/mobile") });
    expect(calls.find(call => call.args.includes("config"))).toMatchObject({
      command: "pnpm",
      args: ["exec", "expo", "config", "--type", "public", "--json"],
      cwd: join(repoRoot, "apps/mobile"),
      env: { KANNA_APP_ENV: "staging", KANNA_APP_VERSION: "1.0.0" },
    });
    expect(calls.some((call) => call.command === "gcloud")).toBe(false);
    expect(result.message).toContain("Dry run: mobile OTA update");
    expect(result.message).toContain(`Source: HEAD (${SHORT_HEAD_COMMIT})`);
    expect(result.message).toContain("curl -H 'expo-protocol-version: 1'");
  });

  it("rejects publish before export when the committed certificate is invalid", async () => {
    const repoRoot = await makeRepoFixture({ certificatePem: "not a certificate" });
    const calls: Array<{ command: string; args: string[] }> = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        return command === "git" ? gitResult(args) : { exitCode: 0, stdout: "", stderr: "" };
      },
    };

    await expect(executeMobileOtaPublishWithContext(
      { staging: true, production: false, dryRun: true },
      { repoRoot, env: {}, runner }
    )).rejects.toThrow("not valid X.509");
    expect(calls).toEqual([
      { command: "git", args: ["status", "--porcelain"] },
      { command: "git", args: ["rev-parse", "--verify", "--quiet", "HEAD^{commit}"] },
    ]);
  });

  it("surfaces Expo public config command failures before cloud access", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: Array<{ command: string; args: string[] }> = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        if (command === "git") return gitResult(args);
        if (command === "gh") return { exitCode: 0, stdout: JSON.stringify({ assets: [] }), stderr: "" };
        if (command === "pnpm" && args.includes("export")) {
          await writeMinimalSdk57Export(repoRoot);
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "pnpm" && args.includes("config")) {
          return { exitCode: 1, stdout: "", stderr: "config failed" };
        }
        return { exitCode: 1, stdout: "", stderr: "unexpected cloud access" };
      },
    };

    await expect(
      executeMobileOtaPublishWithContext(
        { staging: true, production: false, dryRun: true },
        { repoRoot, env: {}, runner }
      )
    ).rejects.toThrow("config failed");
    expect(calls.some((call) => call.command === "gcloud")).toBe(false);
  });

  it("rejects malformed Expo public config before cloud access", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: Array<{ command: string; args: string[] }> = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        if (command === "git") return gitResult(args);
        if (command === "gh") return { exitCode: 0, stdout: JSON.stringify({ assets: [] }), stderr: "" };
        if (command === "pnpm" && args.includes("export")) {
          await writeMinimalSdk57Export(repoRoot);
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "pnpm" && args.includes("config")) {
          return { exitCode: 0, stdout: "not-json", stderr: "" };
        }
        return { exitCode: 1, stdout: "", stderr: "unexpected cloud access" };
      },
    };

    await expect(
      executeMobileOtaPublishWithContext(
        { staging: true, production: false, dryRun: true },
        { repoRoot, env: {}, runner }
      )
    ).rejects.toThrow("Expo public config command did not return valid JSON.");
    expect(calls.some((call) => call.command === "gcloud")).toBe(false);
  });

  it("publishes the update ID derived from the staged metadata uploaded to GCS", async () => {
    const repoRoot = await makeRepoFixture();
    const bundleBytes = Buffer.from("bundle bytes");
    const assetBytes = Buffer.from("asset bytes");
    const bundleKey = createHash("sha256").update(bundleBytes).digest("base64url");
    const assetKey = createHash("sha256").update(assetBytes).digest("base64url");
    const expectedStagedMetadata = JSON.stringify({
      fileMetadata: {
        ios: {
          bundle: `bundles/${bundleKey}.hbc`,
          assets: [
            {
              path: `assets/${assetKey}`,
              ext: "png",
              contentType: "image/png",
            },
          ],
        },
      },
      kanna: { releaseVersion: "1.0.0" },
    });
    const expectedUpdateId = computeExpoUpdateId(Buffer.from(expectedStagedMetadata));
    const runner: CommandRunner = {
      async run(command, args) {
        if (command === "git") return gitResult(args);
        if (command === "gh") return { exitCode: 0, stdout: JSON.stringify({ assets: [] }), stderr: "" };
        if (command === "pnpm" && args.includes("export")) {
          await mkdir(join(repoRoot, "apps/mobile/dist/_expo/static/js/ios"), { recursive: true });
          await mkdir(join(repoRoot, "apps/mobile/dist/assets"), { recursive: true });
          await writeFile(
            join(repoRoot, "apps/mobile/dist/metadata.json"),
            JSON.stringify({
              fileMetadata: {
                ios: {
                  bundle: "_expo/static/js/ios/main.hbc",
                  assets: [
                    {
                      path: "assets/icon.png",
                      ext: "png",
                      contentType: "image/png",
                    },
                  ],
                },
              },
            })
          );
          await writeFile(join(repoRoot, "apps/mobile/dist/_expo/static/js/ios/main.hbc"), bundleBytes);
          await writeFile(join(repoRoot, "apps/mobile/dist/assets/icon.png"), assetBytes);
          return { exitCode: 0, stdout: "", stderr: "" };
        }
        if (command === "pnpm" && args.includes("config")) {
          return {
            exitCode: 0,
            stdout: JSON.stringify({ name: "Kanna Staging", runtimeVersion: "1.0.0" }),
            stderr: "",
          };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      },
    };

    const result = await executeMobileOtaPublishWithContext(
      { staging: true, production: false, dryRun: true },
      { repoRoot, env: {}, runner }
    );

    expect(result.data).toMatchObject({
      updateId: expectedUpdateId,
      runtimeVersion: "1.0.0",
      releaseVersion: "1.0.0",
      channel: "staging",
      source: { ref: "HEAD", commit: HEAD_COMMIT, shortCommit: SHORT_HEAD_COMMIT },
    });
    expect(result.message).toContain(`Dry run: mobile OTA update ${expectedUpdateId}`);
  });

  it("requires an explicit --ref to publish to the production channel", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: Array<{ command: string; args: string[] }> = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        return command === "git" ? gitResult(args) : { exitCode: 0, stdout: "", stderr: "" };
      },
    };

    await expect(
      executeMobileOtaPublishWithContext(
        { staging: false, production: true, dryRun: true },
        { repoRoot, env: {}, runner }
      )
    ).rejects.toThrow("mobile ota publish --production requires --ref <branch|tag|sha>");
    expect(calls).toEqual([]);
  });

  it("publishes to staging without a --ref and reports the resolved HEAD", async () => {
    const repoRoot = await makeRepoFixture();
    const runner = publishRunner(repoRoot);

    const result = await executeMobileOtaPublishWithContext(
      { staging: true, production: false, dryRun: true },
      { repoRoot, env: {}, runner }
    );

    expect(result.message).toContain(`Source: HEAD (${SHORT_HEAD_COMMIT})`);
    expect(result.data).toMatchObject({
      source: { ref: "HEAD", commit: HEAD_COMMIT, shortCommit: SHORT_HEAD_COMMIT },
    });
  });

  it("rolls a production channel back without a --ref", async () => {
    const repoRoot = await makeRepoFixture();
    const runner = publishRunner(repoRoot);

    const result = await executeMobileOtaPublishWithContext(
      {
        staging: false,
        production: true,
        dryRun: true,
        rollbackTo: "11111111-2222-3333-4444-555555555555",
      },
      { repoRoot, env: {}, runner }
    );

    expect(result.ok).toBe(true);
    expect(result.message).toContain("Dry run: mobile OTA rollback");
  });

  describe("leaves no staging root behind", () => {
    /**
     * Every publish and rollback used to `mkdtemp` under the OS temp
     * directory and abandon the root; the Mac Studio held 3,036 of them.
     * The root is scoped to a fixture here because the real temp directory
     * is shared with every other gate on the machine.
     */
    async function scratchFixture(): Promise<string> {
      const scratchDir = await mkdtemp(join(tmpdir(), "kanna-kd-ota-scratch-"));
      tempDirs.push(scratchDir);
      return scratchDir;
    }

    const failingUpload = (args: string[]) => {
      if (args[1] === "cat") return { exitCode: 1, stdout: "", stderr: "not found: 404" };
      return args[1] === "cp"
        ? { exitCode: 1, stdout: "", stderr: "AccessDeniedException: 403" }
        : { exitCode: 0, stdout: "", stderr: "" };
    };

    it("after a publish", async () => {
      const repoRoot = await makeRepoFixture();
      const scratchDir = await scratchFixture();

      const result = await executeMobileOtaPublishWithContext(
        { staging: true, production: false, dryRun: true },
        { repoRoot, env: {}, runner: publishRunner(repoRoot), scratchDir }
      );

      expect(result.ok).toBe(true);
      expect(await readdir(scratchDir)).toEqual([]);
    });

    it("after a publish whose upload fails", async () => {
      const repoRoot = await makeRepoFixture();
      const scratchDir = await scratchFixture();

      await expect(
        executeMobileOtaPublishWithContext(
          { staging: true, production: false, dryRun: false },
          { repoRoot, env: {}, runner: publishRunner(repoRoot, { onGcloud: failingUpload }), scratchDir }
        )
      ).rejects.toThrow("AccessDeniedException");
      expect(await readdir(scratchDir)).toEqual([]);
    });

    it("after a rollback", async () => {
      const repoRoot = await makeRepoFixture();
      const scratchDir = await scratchFixture();

      const result = await executeMobileOtaPublishWithContext(
        { staging: true, production: false, dryRun: true, rollbackTo: "11111111-2222-3333-4444-555555555555" },
        { repoRoot, env: {}, runner: publishRunner(repoRoot), scratchDir }
      );

      expect(result.ok).toBe(true);
      expect(await readdir(scratchDir)).toEqual([]);
    });

    it("after a rollback whose pointer upload fails", async () => {
      const repoRoot = await makeRepoFixture();
      const scratchDir = await scratchFixture();

      await expect(
        executeMobileOtaPublishWithContext(
          { staging: true, production: false, dryRun: false, rollbackTo: "11111111-2222-3333-4444-555555555555" },
          { repoRoot, env: {}, runner: publishRunner(repoRoot, { onGcloud: failingUpload }), scratchDir }
        )
      ).rejects.toThrow("AccessDeniedException");
      expect(await readdir(scratchDir)).toEqual([]);
    });
  });

  it("refuses to publish from a dirty git worktree", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: Array<{ command: string; args: string[] }> = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        return command === "git"
          ? gitResult(args, { status: " M apps/mobile/src/App.tsx\n" })
          : { exitCode: 0, stdout: "", stderr: "" };
      },
    };

    await expect(
      executeMobileOtaPublishWithContext(
        { staging: true, production: false, dryRun: true },
        { repoRoot, env: {}, runner }
      )
    ).rejects.toThrow("Refusing to run mobile ota publish from a dirty git worktree");
    expect(calls).toEqual([{ command: "git", args: ["status", "--porcelain"] }]);
  });

  it("refuses a --ref that is not the checked-out commit", async () => {
    const repoRoot = await makeRepoFixture();
    const runner: CommandRunner = {
      async run(command, args) {
        return command === "git"
          ? gitResult(args, { commits: { HEAD: HEAD_COMMIT, "release/0.2": "1".repeat(40) } })
          : { exitCode: 0, stdout: "", stderr: "" };
      },
    };

    await expect(
      executeMobileOtaPublishWithContext(
        { staging: false, production: true, dryRun: true, ref: "release/0.2" },
        { repoRoot, env: {}, runner }
      )
    ).rejects.toThrow("--ref release/0.2");
  });

  it("records the resolved source commit in the channel pointer and the update itself", async () => {
    const repoRoot = await makeRepoFixture();
    const uploads: Array<{ args: string[] }> = [];
    // What gcloud would have sent, read while the staged files exist: the
    // publish removes its staging root before it returns.
    const uploaded = new Map<"source" | "pointer", string>();
    const runner = publishRunner(repoRoot, {
      commits: { HEAD: HEAD_COMMIT, "release/0.2": HEAD_COMMIT },
      onGcloud: (args) => {
        uploads.push({ args });
        if (args[1] === "cat") return { exitCode: 1, stdout: "", stderr: "not found: 404" };
        // A missing metadata.json is what makes the publish upload the update.
        if (args[1] === "ls") return { exitCode: 1, stdout: "", stderr: "not found" };
        if (args[1] === "rsync") uploaded.set("source", readFileSync(join(args[args.indexOf("--checksums-only") + 1], "kanna-source.json"), "utf8"));
        if (args[1] === "cp") uploaded.set("pointer", readFileSync(args[2], "utf8"));
        return { exitCode: 0, stdout: "", stderr: "" };
      },
    });

    const result = await executeMobileOtaPublishWithContext(
      { staging: false, production: true, ref: "release/0.2" },
      { repoRoot, env: {}, runner }
    );

    expect(result.message).toContain(`Source: release/0.2 (${SHORT_HEAD_COMMIT})`);
    expect(result.data).toMatchObject({
      source: { ref: "release/0.2", commit: HEAD_COMMIT, shortCommit: SHORT_HEAD_COMMIT },
    });

    expect(uploads.some((call) => call.args[1] === "rsync")).toBe(true);
    const sourceRecord = JSON.parse(uploaded.get("source") ?? "null") as Record<string, unknown>;
    expect(sourceRecord).toEqual({
      updateId: (result.data as { updateId: string }).updateId,
      ref: "release/0.2",
      commit: HEAD_COMMIT,
      shortCommit: SHORT_HEAD_COMMIT,
      releaseVersion: "1.0.0",
    });

    expect(uploads.some((call) => call.args[1] === "cp")).toBe(true);
    const pointer = JSON.parse(uploaded.get("pointer") ?? "null") as Record<string, unknown>;
    expect(pointer).toMatchObject({
      currentUpdateId: (result.data as { updateId: string }).updateId,
      runtimeVersion: "1.0.0",
      releaseVersion: "1.0.0",
      sourceRef: "release/0.2",
      sourceCommit: HEAD_COMMIT,
    });
  });

  it("refuses to republish a non-advancing mobile release to a version-aware channel", async () => {
    const repoRoot = await makeRepoFixture();
    const runner = publishRunner(repoRoot, {
      onGcloud: (args) => {
        if (args[1] === "cat") {
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              currentUpdateId: "old-update",
              runtimeVersion: "1.0.0",
              releaseVersion: "1.0.0"
            }),
            stderr: ""
          };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      }
    });

    await expect(
      executeMobileOtaPublishWithContext(
        { staging: true, production: false },
        { repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate }
      )
    ).rejects.toThrow("kd mobile version bump --patch");
  });

  it.each([
    ["network", "ServiceUnavailable: try again"],
    ["permission", "PERMISSION_DENIED: storage.objects.get denied"],
    ["unknown", "gcloud storage cat failed"],
  ])(
    "refuses to publish when the current channel pointer read has a %s failure",
    async (_kind, failure) => {
      const repoRoot = await makeRepoFixture();
      const calls: string[][] = [];
      const pointerPath = `gs://${resolveKdEnvironment("staging").otaBucket}/ota/ios/1.0.0/channels/staging.json`;
      const runner = publishRunner(repoRoot, {
        onGcloud: (args) => {
          calls.push(args);
          if (args[1] === "cat" && args[2] === pointerPath) {
            return { exitCode: 1, stdout: "", stderr: failure };
          }
          return { exitCode: 0, stdout: "", stderr: "" };
        }
      });

      await expect(
        executeMobileOtaPublishWithContext(
          { staging: true, production: false },
          { repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate }
        )
      ).rejects.toThrow(`could not read the current channel pointer ${pointerPath}`);
      expect(calls.some((args) => args[1] === "rsync" || args[1] === "cp")).toBe(false);
    }
  );

  it("refuses to publish when the current channel pointer is malformed", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: string[][] = [];
    const pointerPath = `gs://${resolveKdEnvironment("staging").otaBucket}/ota/ios/1.0.0/channels/staging.json`;
    const runner = publishRunner(repoRoot, {
      onGcloud: (args) => {
        calls.push(args);
        if (args[1] === "cat" && args[2] === pointerPath) {
          return { exitCode: 0, stdout: "{not-json", stderr: "" };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      }
    });

    await expect(
      executeMobileOtaPublishWithContext(
        { staging: true, production: false },
        { repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate }
      )
    ).rejects.toThrow(`current channel pointer ${pointerPath} is malformed`);
    expect(calls.some((args) => args[1] === "rsync" || args[1] === "cp")).toBe(false);
  });

  it("refuses to publish when fallback legacy update metadata cannot be read", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: string[][] = [];
    const pointerPath = `gs://${resolveKdEnvironment("staging").otaBucket}/ota/ios/1.0.0/channels/staging.json`;
    const metadataPath = `gs://${resolveKdEnvironment("staging").otaBucket}/ota/ios/1.0.0/updates/legacy-update/metadata.json`;
    const runner = publishRunner(repoRoot, {
      onGcloud: (args) => {
        calls.push(args);
        if (args[1] === "cat" && args[2] === pointerPath) return {
          exitCode: 0,
          stdout: JSON.stringify({ currentUpdateId: "legacy-update", runtimeVersion: "1.0.0" }),
          stderr: ""
        };
        if (args[1] === "cat" && args[2] === metadataPath) {
          return { exitCode: 1, stdout: "", stderr: "ServiceUnavailable: try again" };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      }
    });

    await expect(
      executeMobileOtaPublishWithContext(
        { staging: true, production: false },
        { repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate }
      )
    ).rejects.toThrow(`could not establish the release currently served by staging: ${metadataPath} is not readable`);
    expect(calls.some((args) => args[1] === "rsync" || args[1] === "cp")).toBe(false);
  });

  it("refuses to publish when fallback legacy update metadata is malformed", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: string[][] = [];
    const pointerPath = `gs://${resolveKdEnvironment("staging").otaBucket}/ota/ios/1.0.0/channels/staging.json`;
    const metadataPath = `gs://${resolveKdEnvironment("staging").otaBucket}/ota/ios/1.0.0/updates/legacy-update/metadata.json`;
    const runner = publishRunner(repoRoot, {
      onGcloud: (args) => {
        calls.push(args);
        if (args[1] === "cat" && args[2] === pointerPath) return {
          exitCode: 0,
          stdout: JSON.stringify({ currentUpdateId: "legacy-update", runtimeVersion: "1.0.0" }),
          stderr: ""
        };
        if (args[1] === "cat" && args[2] === metadataPath) {
          return { exitCode: 0, stdout: "{not-json", stderr: "" };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      }
    });

    await expect(
      executeMobileOtaPublishWithContext(
        { staging: true, production: false },
        { repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate }
      )
    ).rejects.toThrow(`could not establish the release currently served by staging: ${metadataPath} is malformed`);
    expect(calls.some((args) => args[1] === "rsync" || args[1] === "cp")).toBe(false);
  });

  it("accepts a confirmed missing channel as its first publication", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: string[][] = [];
    const pointerPath = `gs://${resolveKdEnvironment("staging").otaBucket}/ota/ios/1.0.0/channels/staging.json`;
    const runner = publishRunner(repoRoot, {
      onGcloud: (args) => {
        calls.push(args);
        if (args[1] === "cat" && args[2] === pointerPath) return {
          exitCode: 1,
          stdout: "",
          stderr:
            "ERROR: (gcloud.storage.cat) The following URLs matched no objects or files: " +
            pointerPath,
        };
        if (args[1] === "ls") return { exitCode: 1, stdout: "", stderr: "not found" };
        return { exitCode: 0, stdout: "", stderr: "" };
      }
    });

    await expect(
      executeMobileOtaPublishWithContext(
        { staging: true, production: false },
        { repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate }
      )
    ).resolves.toMatchObject({ ok: true });
    expect(calls.some((args) => args[1] === "rsync")).toBe(true);
    expect(calls.some((args) => args[1] === "cp")).toBe(true);
  });

  it("accepts successfully read legacy update metadata without a release version", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: string[][] = [];
    const pointerPath = `gs://${resolveKdEnvironment("staging").otaBucket}/ota/ios/1.0.0/channels/staging.json`;
    const metadataPath = `gs://${resolveKdEnvironment("staging").otaBucket}/ota/ios/1.0.0/updates/legacy-update/metadata.json`;
    const runner = publishRunner(repoRoot, {
      onGcloud: (args) => {
        calls.push(args);
        if (args[1] === "cat" && args[2] === pointerPath) return {
          exitCode: 0,
          stdout: JSON.stringify({ currentUpdateId: "legacy-update", runtimeVersion: "1.0.0" }),
          stderr: ""
        };
        if (args[1] === "cat" && args[2] === metadataPath) return {
          exitCode: 0,
          stdout: JSON.stringify({ fileMetadata: { ios: { bundle: "bundles/main.hbc", assets: [] } } }),
          stderr: ""
        };
        if (args[1] === "ls") return { exitCode: 1, stdout: "", stderr: "not found" };
        return { exitCode: 0, stdout: "", stderr: "" };
      }
    });

    await expect(
      executeMobileOtaPublishWithContext(
        { staging: true, production: false },
        { repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate }
      )
    ).resolves.toMatchObject({ ok: true });
    expect(calls.some((args) => args[1] === "rsync")).toBe(true);
    expect(calls.some((args) => args[1] === "cp")).toBe(true);
  });

  it("publishes an advancing release version", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: string[][] = [];
    const runner = publishRunner(repoRoot, {
      onGcloud: (args) => {
        calls.push(args);
        if (args[1] === "cat") return {
          exitCode: 0,
          stdout: JSON.stringify({
            currentUpdateId: "old-update",
            runtimeVersion: "1.0.0",
            releaseVersion: "0.9.9"
          }),
          stderr: ""
        };
        if (args[1] === "ls") return { exitCode: 1, stdout: "", stderr: "not found" };
        return { exitCode: 0, stdout: "", stderr: "" };
      }
    });

    await expect(
      executeMobileOtaPublishWithContext(
        { staging: true, production: false },
        { repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate }
      )
    ).resolves.toMatchObject({ ok: true });
    expect(calls.some((args) => args[1] === "rsync")).toBe(true);
    expect(calls.some((args) => args[1] === "cp")).toBe(true);
  });

  it("publishes successfully but prominently reports a paired iPhone stranded on an older runtime", async () => {
    const repoRoot = await makeRepoFixture();
    const base = publishRunner(repoRoot);
    const runner: CommandRunner = {
      async run(command, args, options) {
        if (command === "curl" && args.at(-1) === "http://127.0.0.1:48121/v1/status") return {
          exitCode: 0, stderr: "", stdout: JSON.stringify({ environment: "staging" })
        };
        if (command === "curl" && args.at(-1) === "http://127.0.0.1:48121/v1/mobile/builds") return {
          exitCode: 0, stderr: "", stdout: JSON.stringify({ desktopId: "owner-mac", devices: [{
            deviceId: "iphone", deviceName: "Owner iPhone", build: {
              environment: "staging", channel: "staging", runtimeVersion: "0.9.0",
              nativeVersion: "0.9.0", nativeBuild: "42", updateId: "old-update", source: "ota", reportedAtUnixMs: Date.now()
            }
          }] })
        };
        return base.run(command, args, options);
      }
    };
    const result = await executeMobileOtaPublishWithContext(
      { staging: true, production: false },
      { repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate }
    );
    expect(result.ok).toBe(true);
    expect(result.message).toContain("Published mobile OTA update");
    expect(result.message).toContain("WARNING OTA DRIFT");
    expect(result.message).toContain("cannot receive runtime 1.0.0");
    expect(result.message).toContain("Reported runtimes: 0.9.0");
  });

  it("status places historical channel pointers beside the paired-device picture", async () => {
    const repoRoot = await makeRepoFixture();
    const prefix = `gs://${resolveKdEnvironment("staging").otaBucket}/ota/ios/`;
    const runner: CommandRunner = {
      async run(command, args) {
        if (command === "curl" && args.at(-1) === "http://127.0.0.1:48121/v1/status") return {
          exitCode: 0, stderr: "", stdout: JSON.stringify({ environment: "staging" })
        };
        if (command === "curl") return { exitCode: 0, stderr: "", stdout: JSON.stringify({
          desktopId: "owner-mac", devices: [{ deviceId: "iphone", deviceName: "Owner iPhone", build: {
            environment: "staging", channel: "staging", runtimeVersion: "0.9.0",
            nativeVersion: "0.9.0", nativeBuild: "42", updateId: "old", source: "ota", reportedAtUnixMs: Date.now()
          } }]
        }) };
        const path = args.at(-1) ?? "";
        if (path.includes("*/channels/")) return { exitCode: 0, stderr: "", stdout: `${prefix}0.9.0/channels/staging.json\n${prefix}1.0.0/channels/staging.json` };
        if (args.includes("cat")) return { exitCode: 0, stderr: "", stdout: JSON.stringify({
          currentUpdateId: path.includes("0.9.0") ? "old" : "new",
          createdAt: path.includes("0.9.0") ? "2026-09-02" : "2026-09-08",
          ...(path.includes("1.0.0") ? { releaseVersion: "1.0.1" } : {})
        }) };
        return { exitCode: 0, stderr: "", stdout: "recent update listing" };
      }
    };
    const result = await mobileOtaRuntime.executeMobileOtaStatusWithContext(
      { staging: true, production: false }, { repoRoot, env: {}, runner }
    );
    expect(result.message).toContain("runtime 0.9.0: old; release unknown (legacy); pointer published 2026-09-02 [STALE");
    expect(result.message).toContain("runtime 1.0.0: new; release 1.0.1");
    expect(result.message).toContain("releaseVersion: 1.0.1");
    expect(result.message).toContain("WARNING OTA DRIFT");
    expect(result.data).toMatchObject({
      releaseVersion: "1.0.1",
      devices: { status: "WARN" },
      pointers: { status: "PASS" }
    });
  });

  it("preserves a target update's release version when rolling the channel back", async () => {
    const repoRoot = await makeRepoFixture();
    let uploadedPointer = "";
    const runner = publishRunner(repoRoot, {
      onGcloud: (args) => {
        if (args[1] === "cat") {
          return {
            exitCode: 0,
            stdout: JSON.stringify({ kanna: { releaseVersion: "1.0.1" } }),
            stderr: ""
          };
        }
        if (args[1] === "cp") {
          uploadedPointer = readFileSync(args[2], "utf8");
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      }
    });

    const result = await executeMobileOtaPublishWithContext(
      {
        staging: true,
        production: false,
        rollbackTo: "11111111-2222-3333-4444-555555555555"
      },
      { repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate }
    );

    expect(result.data).toMatchObject({ releaseVersion: "1.0.1" });
    expect(JSON.parse(uploadedPointer)).toMatchObject({
      currentUpdateId: "11111111-2222-3333-4444-555555555555",
      runtimeVersion: "1.0.0",
      releaseVersion: "1.0.1"
    });
  });

  it("provisions a missing staging OTA bucket and relay storage access", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: Array<{ command: string; args: string[] }> = [];
    const requests: Array<{
      url: string;
      method: "POST";
      headers: Record<string, string>;
      body: unknown;
    }> = [];
    const serviceAccount = "kanna-relay-staging@kanna-staging.iam.gserviceaccount.com";
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        const joined = args.join(" ");
        if (joined.includes("storage buckets describe")) {
          return { exitCode: 1, stdout: "", stderr: "not found: 404" };
        }
        if (joined.includes("auth print-access-token")) {
          return { exitCode: 0, stdout: "test-access-token\n", stderr: "" };
        }
        if (joined.includes("compute instances describe")) {
          return { exitCode: 0, stdout: `${serviceAccount}\n`, stderr: "" };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      },
    };

    const result = await getMobileOtaProvisionExecutor()(
      { staging: true, production: false },
      {
        repoRoot,
        env: {},
        runner,
        async request(input) {
          requests.push(input);
          return { ok: true, status: 200, body: "{}" };
        },
      }
    );

    expect(result.ok).toBe(true);
    expect(calls.map(({ args }) => args)).toContainEqual([
      "services", "enable", "storage.googleapis.com", "firebasestorage.googleapis.com",
      "--project", "kanna-staging",
    ]);
    expect(calls.map(({ args }) => args)).toContainEqual([
      "auth", "print-access-token",
    ]);
    expect(calls.some(({ args }) => args.includes("create"))).toBe(false);
    expect(requests).toEqual([{
      url: "https://firebasestorage.googleapis.com/v1alpha/projects/kanna-staging/defaultBucket",
      method: "POST",
      headers: {
        authorization: "Bearer test-access-token",
        "content-type": "application/json",
      },
      body: { location: "US-CENTRAL1" },
    }]);
    expect(calls.map(({ args }) => args)).toContainEqual([
      "storage", "buckets", "add-iam-policy-binding",
      "gs://kanna-staging.firebasestorage.app", "--project", "kanna-staging",
      "--member", `serviceAccount:${serviceAccount}`,
      "--role", "roles/storage.objectViewer",
    ]);
    expect(result.message).toContain("kanna-staging.firebasestorage.app");
    expect(result.message).toContain(serviceAccount);
  });

  it("reuses an existing staging OTA bucket", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: Array<{ command: string; args: string[] }> = [];
    let requested = false;
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        const joined = args.join(" ");
        if (joined.includes("storage buckets describe")) {
          return { exitCode: 0, stdout: "{}", stderr: "" };
        }
        if (joined.includes("compute instances describe")) {
          return {
            exitCode: 0,
            stdout: "kanna-relay-staging@kanna-staging.iam.gserviceaccount.com\n",
            stderr: "",
          };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      },
    };

    await getMobileOtaProvisionExecutor()(
      { staging: true, production: false },
      {
        repoRoot,
        env: {},
        runner,
        async request() {
          requested = true;
          return { ok: true, status: 200, body: "{}" };
        },
      }
    );

    expect(calls.some(({ args }) => args.includes("create"))).toBe(false);
    expect(calls.some(({ args }) => args.includes("print-access-token"))).toBe(false);
    expect(requested).toBe(false);
  });

  it("does not create an OTA bucket when bucket inspection is forbidden", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: Array<{ command: string; args: string[] }> = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        if (args.join(" ").includes("storage buckets describe")) {
          return { exitCode: 1, stdout: "", stderr: "PERMISSION_DENIED" };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      },
    };

    await expect(getMobileOtaProvisionExecutor()(
      { staging: true, production: false },
      { repoRoot, env: {}, runner }
    )).rejects.toThrow("PERMISSION_DENIED");
    expect(calls.some(({ args }) => args.includes("create"))).toBe(false);
    expect(calls.some(({ args }) => args.includes("print-access-token"))).toBe(false);
  });

  it("stops before IAM and hides the access token when Firebase bucket provisioning fails", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: Array<{ command: string; args: string[] }> = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        const joined = args.join(" ");
        if (joined.includes("storage buckets describe")) {
          return { exitCode: 1, stdout: "", stderr: "not found: 404" };
        }
        if (joined.includes("auth print-access-token")) {
          return { exitCode: 0, stdout: "test-access-token\n", stderr: "" };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      },
    };

    const result = getMobileOtaProvisionExecutor()(
      { staging: true, production: false },
      {
        repoRoot,
        env: {},
        runner,
        async request() {
          return {
            ok: false,
            status: 400,
            body: "Blaze plan required; token=test-access-token",
          };
        },
      }
    );

    await expect(result).rejects.toThrow(
      "Firebase default-bucket provisioning failed (HTTP 400): Blaze plan required; token=[redacted]"
    );
    await expect(result).rejects.not.toThrow("test-access-token");
    expect(calls.some(({ args }) => args.includes("add-iam-policy-binding"))).toBe(false);
  });

  it("requires exactly one environment for OTA infrastructure provisioning", async () => {
    const repoRoot = await makeRepoFixture();
    const runner: CommandRunner = {
      async run() {
        throw new Error("cloud command must not run");
      },
    };
    const executeProvision = getMobileOtaProvisionExecutor();

    await expect(executeProvision(
      { staging: false, production: false },
      { repoRoot, env: {}, runner }
    )).rejects.toThrow("mobile ota provision requires --staging or --production");
    await expect(executeProvision(
      { staging: true, production: true },
      { repoRoot, env: {}, runner }
    )).rejects.toThrow("mobile ota provision accepts only one of --staging or --production");
  });

  it("provisions the private key through kd-managed gcloud commands", async () => {
    const repoRoot = await makeRepoFixture();
    const keyPath = join(repoRoot, "ota-private-key.pem");
    await writeFile(keyPath, "private key");
    const calls: Array<{ command: string; args: string[] }> = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        if (args.includes("describe") && args.includes("kanna-mobile-ota-private-key-pem")) {
          return { exitCode: 1, stdout: "", stderr: "not found: 404" };
        }
        if (args.includes("instances") && args.includes("describe")) {
          return { exitCode: 0, stdout: "relay-sa@kanna-staging.iam.gserviceaccount.com\n", stderr: "" };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      },
    };

    const result = await executeMobileOtaProvisionSecretWithContext(
      { staging: true, production: false, keyPath },
      { repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate }
    );

    expect(result.ok).toBe(true);
    expect(calls.map((call) => call.args)).toContainEqual([
      "services", "enable", "secretmanager.googleapis.com", "--project", "kanna-staging",
    ]);
    expect(calls.map((call) => call.args.slice(0, 3).join(" "))).toContain("secrets create kanna-mobile-ota-private-key-pem");
    expect(calls.map((call) => call.args.slice(0, 4).join(" "))).toContain("secrets versions add kanna-mobile-ota-private-key-pem");
    expect(calls.at(-1)?.args).toContain("roles/secretmanager.secretAccessor");
  });

  it("rejects provision-secret before cloud commands when the key mismatches", async () => {
    const repoRoot = await makeRepoFixture();
    const keyPath = join(repoRoot, "mismatched-private-key.pem");
    const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
    await writeFile(
      keyPath,
      privateKey.export({ type: "pkcs8", format: "pem" }).toString()
    );
    const calls: Array<{ command: string; args: string[] }> = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        return { exitCode: 0, stdout: "", stderr: "" };
      },
    };

    await expect(executeMobileOtaProvisionSecretWithContext(
      { staging: true, production: false, keyPath },
      { repoRoot, env: {}, runner }
    )).rejects.toThrow("does not match the committed mobile OTA certificate");
    expect(calls).toEqual([]);
  });

  it("does not create or version an OTA secret when secret inspection is forbidden", async () => {
    const repoRoot = await makeRepoFixture();
    const keyPath = join(repoRoot, "ota-private-key.pem");
    await writeFile(keyPath, "private key");
    const calls: Array<{ command: string; args: string[] }> = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        if (args.includes("describe") && args.includes("kanna-mobile-ota-private-key-pem")) {
          return { exitCode: 1, stdout: "", stderr: "PERMISSION_DENIED" };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      },
    };

    await expect(executeMobileOtaProvisionSecretWithContext(
      { staging: true, production: false, keyPath },
      { repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate }
    )).rejects.toThrow("PERMISSION_DENIED");
    expect(calls.some(({ args }) => args.includes("create"))).toBe(false);
    expect(calls.some(({ args }) => args.includes("versions"))).toBe(false);
  });

  it("runs a read-only staging OTA doctor against GCS, relay, and Secret Manager wiring", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: Array<{ command: string; args: string[] }> = [];
    const pointer = {
      currentUpdateId: "11111111-2222-3333-4444-555555555555",
      createdAt: "2026-06-30T00:00:00.000Z",
      runtimeVersion: "1.0.0",
    };
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        const joined = args.join(" ");
        if (command === "gcloud" && joined.includes("storage cat gs://kanna-staging.firebasestorage.app/ota/ios/1.0.0/channels/staging.json")) {
          return { exitCode: 0, stdout: JSON.stringify(pointer), stderr: "" };
        }
        if (command === "gcloud" && joined.includes("updates/11111111-2222-3333-4444-555555555555/metadata.json")) {
          return { exitCode: 0, stdout: "{\"fileMetadata\":{\"ios\":{\"bundle\":\"bundles/main.hbc\"}}}", stderr: "" };
        }
        if (command === "gcloud" && joined.includes("updates/11111111-2222-3333-4444-555555555555/expoConfig.json")) {
          return { exitCode: 0, stdout: "{\"name\":\"Kanna\"}", stderr: "" };
        }
        if (command === "gcloud" && joined.includes("secrets describe kanna-mobile-ota-private-key-pem")) {
          return { exitCode: 0, stdout: "name: projects/kanna-staging/secrets/kanna-mobile-ota-private-key-pem\n", stderr: "" };
        }
        if (command === "gcloud" && joined.includes("compute instances describe kanna-relay-staging")) {
          return { exitCode: 0, stdout: "relay-sa@kanna-staging.iam.gserviceaccount.com\n", stderr: "" };
        }
        if (command === "gcloud" && joined.includes("secrets get-iam-policy kanna-mobile-ota-private-key-pem")) {
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              bindings: [{
                role: "roles/secretmanager.secretAccessor",
                members: ["serviceAccount:relay-sa@kanna-staging.iam.gserviceaccount.com"],
              }],
            }),
            stderr: "",
          };
        }
        if (command === "gcloud" && joined.includes("storage buckets get-iam-policy gs://kanna-staging.firebasestorage.app")) {
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              bindings: [{
                role: "roles/storage.objectViewer",
                members: ["serviceAccount:relay-sa@kanna-staging.iam.gserviceaccount.com"],
              }],
            }),
            stderr: "",
          };
        }
        if (command === "curl" && args.at(-1) === "https://relay-staging.kanna.build/health") {
          return { exitCode: 0, stdout: "{\"ok\":true}", stderr: "" };
        }
        if (command === "curl" && args.at(-1) === "https://relay-staging.kanna.build/ota/manifest") {
          return { exitCode: 0, stdout: "multipart manifest", stderr: "" };
        }
        return { exitCode: 1, stdout: "", stderr: `unexpected command: ${command} ${joined}` };
      },
    };

    const result = await executeMobileOtaDoctorWithContext(
      { staging: true, production: false },
      { repoRoot, env: {}, runner }
    );

    expect(result.ok).toBe(false);
    expect(result.message).toContain("Mobile OTA staging preflight");
    expect(result.message).toContain("WARN device compatibility:");
    expect(result.message).toContain("reachability UNKNOWN");
    expect(calls.some((call) => call.command === "gcloud" && call.args.includes("cp"))).toBe(false);
    expect(calls.some((call) => call.command === "gcloud" && call.args.includes("rsync"))).toBe(false);
    expect(calls.some((call) => call.command === "gcloud" && call.args.includes("create"))).toBe(false);
    expect(calls.some((call) => call.command === "gcloud" && call.args.includes("add-iam-policy-binding"))).toBe(false);
    expect(calls.some((call) => call.command === "gcloud" && call.args.includes("access"))).toBe(false);
    const iamCalls = calls.filter((call) => call.args.includes("get-iam-policy"));
    expect(iamCalls).toHaveLength(2);
    for (const call of iamCalls) {
      expect(call.args).toContain("--format=json");
      expect(call.args.some((arg) => arg.startsWith("--filter"))).toBe(false);
      expect(call.args.some((arg) => arg.startsWith("--flatten"))).toBe(false);
    }
  });

  it("reports an invalid committed certificate as a read-only doctor failure", async () => {
    const repoRoot = await makeRepoFixture({ certificatePem: "not a certificate" });
    const calls: Array<{ command: string; args: string[] }> = [];
    const pointer = {
      currentUpdateId: "11111111-2222-3333-4444-555555555555",
      createdAt: "2026-06-30T00:00:00.000Z",
      runtimeVersion: "1.0.0",
    };
    const member = "serviceAccount:relay-sa@kanna-staging.iam.gserviceaccount.com";
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        const joined = args.join(" ");
        if (command === "gcloud" && joined.includes("channels/staging.json")) {
          return { exitCode: 0, stdout: JSON.stringify(pointer), stderr: "" };
        }
        if (command === "gcloud" && joined.includes("updates/11111111-2222-3333-4444-555555555555/")) {
          return { exitCode: 0, stdout: "{}", stderr: "" };
        }
        if (command === "gcloud" && joined.includes("secrets describe")) {
          return { exitCode: 0, stdout: "{}", stderr: "" };
        }
        if (command === "gcloud" && joined.includes("compute instances describe")) {
          return { exitCode: 0, stdout: "relay-sa@kanna-staging.iam.gserviceaccount.com\n", stderr: "" };
        }
        if (command === "gcloud" && joined.includes("get-iam-policy")) {
          const role = joined.includes("secrets get-iam-policy")
            ? "roles/secretmanager.secretAccessor"
            : "roles/storage.objectViewer";
          return {
            exitCode: 0,
            stdout: JSON.stringify({ bindings: [{ role, members: [member] }] }),
            stderr: "",
          };
        }
        if (command === "curl") {
          return { exitCode: 0, stdout: "ok", stderr: "" };
        }
        return { exitCode: 1, stdout: "", stderr: `unexpected command: ${command} ${joined}` };
      },
    };

    const result = await executeMobileOtaDoctorWithContext(
      { staging: true, production: false },
      { repoRoot, env: {}, runner }
    );

    expect(result.ok).toBe(false);
    expect(result.message).toContain("FAIL certificate");
    expect(result.message).toContain("not valid X.509");
    expect(calls.some(({ args }) => args.includes("cp"))).toBe(false);
    expect(calls.some(({ args }) => args.includes("rsync"))).toBe(false);
    expect(calls.some(({ args }) => args.includes("create"))).toBe(false);
    expect(calls.some(({ args }) => args.includes("add-iam-policy-binding"))).toBe(false);
  });

  it("reports a missing OTA pointer without probing update objects", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: Array<{ command: string; args: string[] }> = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push({ command, args });
        const joined = args.join(" ");
        if (command === "gcloud" && joined.includes("storage cat gs://kanna-staging.firebasestorage.app/ota/ios/1.0.0/channels/staging.json")) {
          return { exitCode: 1, stdout: "", stderr: "No URLs matched" };
        }
        if (command === "gcloud" && joined.includes("secrets describe kanna-mobile-ota-private-key-pem")) {
          return { exitCode: 0, stdout: "name: projects/kanna-staging/secrets/kanna-mobile-ota-private-key-pem\n", stderr: "" };
        }
        if (command === "gcloud" && joined.includes("compute instances describe kanna-relay-staging")) {
          return { exitCode: 0, stdout: "relay-sa@kanna-staging.iam.gserviceaccount.com\n", stderr: "" };
        }
        if (command === "gcloud" && joined.includes("secrets get-iam-policy kanna-mobile-ota-private-key-pem")) {
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              bindings: [{
                role: "roles/secretmanager.secretAccessor",
                members: ["serviceAccount:relay-sa@kanna-staging.iam.gserviceaccount.com"],
              }],
            }),
            stderr: "",
          };
        }
        if (command === "gcloud" && joined.includes("storage buckets get-iam-policy gs://kanna-staging.firebasestorage.app")) {
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              bindings: [{
                role: "roles/storage.objectViewer",
                members: ["serviceAccount:relay-sa@kanna-staging.iam.gserviceaccount.com"],
              }],
            }),
            stderr: "",
          };
        }
        if (command === "curl" && args.at(-1) === "https://relay-staging.kanna.build/health") {
          return { exitCode: 0, stdout: "{\"ok\":true}", stderr: "" };
        }
        if (command === "curl" && args.at(-1) === "https://relay-staging.kanna.build/ota/manifest") {
          return { exitCode: 22, stdout: "", stderr: "HTTP 404" };
        }
        return { exitCode: 1, stdout: "", stderr: `unexpected command: ${command} ${joined}` };
      },
    };

    const result = await executeMobileOtaDoctorWithContext(
      { staging: true, production: false },
      { repoRoot, env: {}, runner }
    );

    expect(result.ok).toBe(false);
    expect(result.message).toContain("FAIL pointer");
    expect(result.message).toContain("manifest: relay reports no update for the channel");
    expect(calls.some((call) => call.args.join(" ").includes("/updates/"))).toBe(false);
  });

  it("rejects a relay storage member bound to the wrong IAM role", async () => {
    const repoRoot = await makeRepoFixture();
    const member = "serviceAccount:relay-sa@kanna-staging.iam.gserviceaccount.com";
    const pointer = {
      currentUpdateId: "11111111-2222-3333-4444-555555555555",
      runtimeVersion: "1.0.0",
    };
    const runner: CommandRunner = {
      async run(command, args) {
        const joined = args.join(" ");
        if (command === "gcloud" && joined.includes("channels/staging.json")) {
          return { exitCode: 0, stdout: JSON.stringify(pointer), stderr: "" };
        }
        if (command === "gcloud" && joined.includes("updates/11111111-2222-3333-4444-555555555555/")) {
          return { exitCode: 0, stdout: "{}", stderr: "" };
        }
        if (command === "gcloud" && joined.includes("secrets describe")) {
          return { exitCode: 0, stdout: "{}", stderr: "" };
        }
        if (command === "gcloud" && joined.includes("compute instances describe")) {
          return { exitCode: 0, stdout: "relay-sa@kanna-staging.iam.gserviceaccount.com\n", stderr: "" };
        }
        if (command === "gcloud" && joined.includes("secrets get-iam-policy")) {
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              bindings: [{ role: "roles/secretmanager.secretAccessor", members: [member] }],
            }),
            stderr: "",
          };
        }
        if (command === "gcloud" && joined.includes("storage buckets get-iam-policy")) {
          return {
            exitCode: 0,
            stdout: JSON.stringify({
              bindings: [{ role: "roles/storage.objectCreator", members: [member] }],
            }),
            stderr: "",
          };
        }
        if (command === "curl") {
          return { exitCode: 0, stdout: "ok", stderr: "" };
        }
        return { exitCode: 1, stdout: "", stderr: `unexpected command: ${command} ${joined}` };
      },
    };

    const result = await executeMobileOtaDoctorWithContext(
      { staging: true, production: false },
      { repoRoot, env: {}, runner }
    );

    expect(result.ok).toBe(false);
    expect(result.message).toContain("FAIL GCS IAM");
  });
});

describe("platform-specific OTA publication", () => {
  it.each(["publish", "status", "doctor", "preflight"])("propagates --platform for %s and rejects unknown platforms at the task boundary", async (command) => {
    const { getTaskDefinition } = await import("../tasks/registry");
    const parsed = parseCliArgs(["mobile", "ota", command, "--staging", "--platform", "android"]);
    const schema = getTaskDefinition(parsed.taskId).inputSchema;
    expect(schema.parse(parsed.input)).toMatchObject({ platform: "android" });
    expect(schema.parse({ staging: true })).toMatchObject({ platform: "ios" });
    expect(() => schema.parse({ ...parsed.input, platform: "web" })).toThrow();
    expect(() => parseCliArgs(["mobile", "ota", command, "--platform"])).toThrow("requires a value");
  });

  it.each(["ios", "android"] as const)("exports, hashes, reconciles partial objects, and commits only the %s pointer", async (platform) => {
    const repoRoot = await makeRepoFixture();
    const calls: string[][] = [];
    let stagedMetadata: any;
    const base = publishRunner(repoRoot, { onGcloud: (args) => {
      calls.push(args);
      if (args[1] === "cat") return { exitCode: 1, stdout: "", stderr: "404 not found" };
      // Simulate metadata already existing: this must not skip the full sync.
      if (args[1] === "rsync") {
        stagedMetadata = JSON.parse(readFileSync(join(args[4], "metadata.json"), "utf8"));
        expect(readFileSync(join(args[4], stagedMetadata.fileMetadata[platform].bundle), "utf8")).toBe("bundle");
      }
      return { exitCode: 0, stdout: "metadata already exists", stderr: "" };
    } });
    const result = await executeMobileOtaPublishWithContext({ staging: true, production: false, platform }, {
      repoRoot, env: {}, runner: base, validateOtaCertificate: acceptOtaCertificate,
    });
    expect(result.data).toMatchObject({ platform });
    expect(result.message).toContain(`expo-platform: ${platform}`);
    expect(Object.keys(stagedMetadata.fileMetadata)).toEqual([platform]);
    const writes = calls.filter(args => ["rsync", "cp"].includes(args[1]));
    expect(writes.map(args => args[1])).toEqual(["rsync", "cp"]);
    expect(writes.every(args => args.at(-1)?.includes(`/ota/${platform}/1.0.0/`))).toBe(true);
    expect(writes[0]).toContain("--checksums-only");
  });

  it.each(["export", "config-runtime", "rsync", "cp"])("reports %s failure without committing a pointer prematurely", async (failure) => {
    const repoRoot = await makeRepoFixture();
    const writes: string[] = [];
    const base = publishRunner(repoRoot);
    const runner: CommandRunner = { async run(command, args, options) {
      if (command === "gcloud" && ["rsync", "cp"].includes(args[1])) writes.push(args[1]);
      if (args.includes(failure)) return { exitCode: 1, stdout: "", stderr: "injected failure" };
      if (failure === "config-runtime" && command === "pnpm" && args.includes("config")) {
        return { exitCode: 0, stdout: JSON.stringify({ runtimeVersion: "wrong" }), stderr: "" };
      }
      return base.run(command, args, options);
    } };
    await expect(executeMobileOtaPublishWithContext({ staging: true, production: false, platform: "android" }, {
      repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate,
    })).rejects.toThrow(failure === "config-runtime" ? "does not match publication runtime" : "injected failure");
    expect(writes).toEqual(failure === "cp" ? ["rsync", "cp"] : failure === "rsync" ? ["rsync"] : []);
  });

  it("checks only Android pointers and emits Android manifest headers in status/doctor", async () => {
    const repoRoot = await makeRepoFixture();
    const calls: Array<{ command: string; args: string[] }> = [];
    const runner: CommandRunner = { async run(command, args) {
      calls.push({ command, args });
      return { exitCode: 1, stdout: "", stderr: "missing" };
    } };
    for (const execute of [mobileOtaRuntime.executeMobileOtaStatusWithContext, executeMobileOtaDoctorWithContext]) {
      const result = await execute({ staging: true, production: false, platform: "android" }, {
        repoRoot, env: {}, runner, validateOtaCertificate: acceptOtaCertificate,
      });
      expect(result.ok).toBe(false);
      expect(result.data).toMatchObject({ platform: "android" });
    }
    expect(calls.some(call => call.args.includes("expo-platform: android"))).toBe(true);
    expect(calls.some(call => call.args.some(arg => arg.includes("ota/ios/")))).toBe(false);
    expect(calls.some(call => call.args.some(arg => arg.includes("ota/android/")))).toBe(true);
  });
});

describe("OTA staging lineage caller", () => {
  const activeCommit = "a".repeat(40);
  const targetCommit = "b".repeat(40);
  const remoteTip = "c".repeat(40);
  const rollbackId = "11111111-2222-3333-4444-555555555555";
  const success = (stdout = "") => ({ exitCode: 0, stdout, stderr: "" });

  async function fixture(options: {
    relationship?: "same" | "descendant" | "behind" | "diverged";
    activeBranch?: string; currentBranch?: string; unreadable?: boolean;
    rollbackSource?: "valid" | "missing" | "unidentified";
  } = {}) {
    const repoRoot = await makeRepoFixture();
    const calls: Array<{ command: string; args: string[] }> = [];
    const base = publishRunner(repoRoot);
    const proposed = options.rollbackSource ? targetCommit : HEAD_COMMIT;
    const active = options.relationship === "same" ? proposed : activeCommit;
    const runner: CommandRunner = { async run(command, args, runOptions) {
      calls.push({ command, args });
      if (command === "git") {
        if (args[0] === "branch") return success(options.currentBranch ?? "main");
        if (args[0] === "ls-remote") return success(args.includes("--tags") ? "" : `${remoteTip}\t${args.at(-1)}`);
        if (args[0] === "merge-base") {
          // Membership in the verified remote branch is separate from the
          // proposal's relationship to the active desktop candidate.
          if (args.at(-1) === remoteTip) return success();
          const forward = args[2] === active;
          const ancestor = forward ? options.relationship === "descendant" : options.relationship === "behind";
          return { exitCode: ancestor ? 0 : 1, stdout: "", stderr: "" };
        }
      }
      if (command === "gh") {
        if (options.unreadable) return { exitCode: 1, stdout: "", stderr: "HTTP 503" };
        if (args.includes("download")) {
          await writeFile(join(args[args.indexOf("--dir") + 1], "latest-staging.json"), JSON.stringify({ version: "1.2.0-staging.1" }));
          return success();
        }
        if (args.at(-1) === "assets") return success(JSON.stringify({ assets: [{ name: "latest-staging.json" }] }));
        if (args.at(-1) === "body") return success(JSON.stringify({ body: "" }));
        return success(JSON.stringify({ targetCommitish: active, body: `Source-Branch: ${options.activeBranch ?? "main"}`, publishedAt: "2026-09-10T00:00:00Z" }));
      }
      if (command === "gcloud" && args.at(-1)?.endsWith("kanna-source.json")) {
        if (options.rollbackSource === "missing") return { exitCode: 1, stdout: "", stderr: "404" };
        return success(JSON.stringify({ updateId: rollbackId, ref: options.rollbackSource === "unidentified" ? "HEAD" : "main", commit: targetCommit, shortCommit: targetCommit.slice(0, 12), releaseVersion: "1.0.0" }));
      }
      return base.run(command, args, runOptions);
    } };
    return { repoRoot, calls, runner, env: {}, validateOtaCertificate: acceptOtaCertificate };
  }

  it.each(["same", "descendant"] as const)("permits %s staging source through the real shared gate", async (relationship) => {
    const context = await fixture({ relationship });
    await expect(executeMobileOtaPublishWithContext({ staging: true, production: false, platform: "android", dryRun: true }, context)).resolves.toMatchObject({ ok: true });
    const gateIndex = context.calls.findIndex(call => call.command === "gh");
    const exportIndex = context.calls.findIndex(call => call.args.includes("export"));
    expect(gateIndex).toBeGreaterThan(0);
    expect(exportIndex).toBeGreaterThan(gateIndex);
  });

  it.each([
    { relationship: "behind" as const, error: "roll the staging channel back" },
    { relationship: "diverged" as const, error: "diverged" },
    { unreadable: true, error: "Cannot verify staging lineage" },
    { relationship: "descendant" as const, activeBranch: "release/1.2", error: "frozen to that branch" },
    { currentBranch: "task-example", error: "Cannot establish staging OTA source lineage" },
  ])("refuses unsafe staging source before exporting, including dry-run: %j", async ({ error, ...options }) => {
    const context = await fixture(options);
    await expect(executeMobileOtaPublishWithContext({ staging: true, production: false, platform: "android", dryRun: true }, context)).rejects.toThrow(error);
    expect(context.calls.some(call => call.command === "pnpm" || call.command === "gcloud")).toBe(false);
  });

  it.each(["valid", "missing", "unidentified"] as const)("checks rollback target provenance (%s), never substitutes checkout HEAD", async (rollbackSource) => {
    const context = await fixture({ relationship: "behind", rollbackSource });
    await expect(executeMobileOtaPublishWithContext({ staging: true, production: false, platform: "android", rollbackTo: rollbackId, dryRun: true }, context)).rejects.toThrow(
      rollbackSource === "valid" ? "roll the staging channel back" : rollbackSource === "missing" ? "missing or unverifiable" : "Cannot establish staging OTA source lineage"
    );
    expect(context.calls.some(call => call.args.includes("export") || call.args[1] === "cp")).toBe(false);
    if (rollbackSource === "valid") {
      expect(context.calls).toContainEqual({ command: "git", args: ["merge-base", "--is-ancestor", activeCommit, targetCommit] });
      expect(context.calls.some(call => call.args[0] === "merge-base" && call.args.includes(HEAD_COMMIT))).toBe(false);
    }
  });

  it("rolls back only the selected Android pointer when the durable target passes lineage", async () => {
    const context = await fixture({ relationship: "same", rollbackSource: "valid" });
    const result = await executeMobileOtaPublishWithContext({ staging: true, production: false, platform: "android", rollbackTo: rollbackId }, context);
    expect(result.data).toMatchObject({ platform: "android" });
    expect(context.calls.filter(call => call.args[1] === "cp").map(call => call.args.at(-1))).toEqual([
      "gs://kanna-staging.firebasestorage.app/ota/android/1.0.0/channels/staging.json",
    ]);
  });
});

it("does not stage iOS metadata as an Android update", async () => {
  const repoRoot = await makeRepoFixture();
  await expect(buildMobileOtaPublishPlan({ repoRoot, environment: "staging", platform: "android" })).rejects.toThrow("fileMetadata.android.bundle");
});
