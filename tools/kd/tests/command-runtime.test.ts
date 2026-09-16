import { spawnSync } from "node:child_process";
import { generateKeyPairSync } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { checkRequiredCommands } from "../src/runtime/doctor";
import { buildMobileDeviceSmokeCommand, buildMobileTestCommand } from "../src/runtime/mobile-commands";
import {
  buildProductionMobileQaCommands,
  executeProductionBillingReview,
  executeProductionMobileQa,
  formatProductionBillingReviewResult,
  isProductionBillingReviewOk,
  parseBillingReviewReport,
  readAppStoreReviewerAccount,
  resolveReviewerCredentials,
  validateProductionMobileConfig
} from "../src/runtime/mobile-qa";
import { getPortStatuses } from "../src/runtime/port-status";
import {
  respawnTmuxWindow,
  startTmuxSession,
  stopTmuxWindow,
  waitForTmuxWindowReady
} from "../src/runtime/tmux";
import { nodeCommandRunner, type CommandRunner } from "../src/runtime/process";
import { kdTestScratchDir } from "./test-paths";

const tmuxAvailable = spawnSync("tmux", ["-V"], { stdio: "ignore" }).status === 0;

async function waitForFile(path: string): Promise<string> {
  const deadline = Date.now() + 5_000;
  let lastError: unknown;
  while (Date.now() < deadline) {
    try {
      return await readFile(path, "utf8");
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  }
  throw lastError;
}

async function unusedLoopbackPort(): Promise<number> {
  const server = createServer();
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  if (!port) throw new Error("could not reserve restart test port");
  return port;
}

describe("command runtime helpers", () => {
  it("checks whether Firebase emulator ports are listening", async () => {
    const calls: string[] = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push(`${command} ${args.join(" ")}`);
        const port = args.find((arg) => arg.startsWith("-iTCP:"))?.split(":").at(-1);
        return port === "9099"
          ? { exitCode: 0, stdout: "123\n", stderr: "" }
          : { exitCode: 1, stdout: "", stderr: "" };
      }
    };

    const statuses = await getPortStatuses(runner, {
      auth: 9099,
      firestore: 8080
    });

    expect(statuses).toEqual([
      { name: "auth", port: 9099, listening: true, pids: ["123"] },
      { name: "firestore", port: 8080, listening: false, pids: [] }
    ]);
    expect(calls).toEqual(["lsof -nP -iTCP:9099 -sTCP:LISTEN -t", "lsof -nP -iTCP:8080 -sTCP:LISTEN -t"]);
  });

  it("builds mobile test commands from the repo root", () => {
    expect(buildMobileTestCommand("/repo")).toEqual({
      command: "pnpm",
      args: ["--dir", "/repo/apps/mobile", "test"]
    });
    expect(buildMobileDeviceSmokeCommand("/repo")).toEqual({
      command: "pnpm",
      args: ["--dir", "/repo/apps/mobile", "run", "test:e2e:device:smoke"]
    });
  });

  it("builds the automated production mobile QA command sequence", () => {
    expect(buildProductionMobileQaCommands("/repo")).toEqual([
      {
        name: "typecheck",
        command: "pnpm",
        args: ["--dir", "/repo/apps/mobile", "run", "typecheck"]
      },
      {
        name: "unit",
        command: "pnpm",
        args: ["--dir", "/repo/apps/mobile", "run", "test"]
      },
      {
        name: "simulator-preflight",
        command: "pnpm",
        args: ["--dir", "/repo/apps/mobile", "run", "test:e2e:preflight"]
      },
      {
        name: "simulator-smoke",
        command: "pnpm",
        args: ["--dir", "/repo/apps/mobile", "run", "test:e2e:smoke"]
      }
    ]);
  });

  it("validates production mobile config against kd production identity", () => {
    const checks = validateProductionMobileConfig({
      prod: {
        runtimeVersion: "1.0.0",
        name: "prod",
        displayName: "Kanna",
        scheme: "kanna",
        iosBundleId: "build.kanna.app",
        iosGoogleServicesFile: "./firebase/GoogleService-Info.production.plist",
        firebase: {
          apiKey: "real-key",
          authDomain: "kanna-build.firebaseapp.com",
          projectId: "kanna-build",
          storageBucket: "kanna-build.firebasestorage.app",
          messagingSenderId: "402613185450",
          appId: "1:402613185450:ios:adcedeadcd241285d859d3"
        },
        relayUrl: "wss://relay.kanna.build",
        otaChannel: "production"
      }
    });

    expect(checks.every((check) => check.ok)).toBe(true);
  });

  it("reports production mobile config drift", () => {
    const checks = validateProductionMobileConfig({
      prod: {
        runtimeVersion: "",
        name: "prod",
        displayName: "Kanna Dev",
        scheme: "kanna-dev",
        iosBundleId: "build.kanna.app.dev",
        iosGoogleServicesFile: "./firebase/GoogleService-Info.staging.plist",
        firebase: {
          apiKey: "kanna-local",
          projectId: "kanna-local",
          appId: ""
        },
        relayUrl: "ws://127.0.0.1:9080",
        otaChannel: "staging"
      }
    });

    expect(checks.filter((check) => !check.ok).map((check) => check.name)).toEqual([
      "displayName",
      "scheme",
      "iosBundleId",
      "iosGoogleServicesFile",
      "runtimeVersion",
      "firebase.projectId",
      "firebase.storageBucket",
      "firebase.apiKey",
      "firebase.appId",
      "relayUrl",
      "otaChannel"
    ]);
  });

  it("runs production mobile QA commands with production E2E environment defaults", async () => {
    const repoRoot = await mkdtemp(join(tmpdir(), "mobile-qa-"));
    await mkdir(join(repoRoot, "apps/mobile/src"), { recursive: true });
    await writeFile(
      join(repoRoot, "apps/mobile/src/mobileEnvironments.json"),
      JSON.stringify({
        prod: {
          runtimeVersion: "1.0.0",
          name: "prod",
          displayName: "Kanna",
          scheme: "kanna",
          iosBundleId: "build.kanna.app",
          iosGoogleServicesFile: "./firebase/GoogleService-Info.production.plist",
          firebase: {
            apiKey: "real-key",
            projectId: "kanna-build",
            storageBucket: "kanna-build.firebasestorage.app",
            appId: "1:402613185450:ios:adcedeadcd241285d859d3"
          },
          relayUrl: "wss://relay.kanna.build",
          otaChannel: "production"
        }
      })
    );
    const envs: Array<NodeJS.ProcessEnv | undefined> = [];
    const keyPath = join(repoRoot, "fake key with spaces.pem");
    await writeFile(keyPath, "not an actual private key", { mode: 0o600 });
    const runner: CommandRunner = {
      async run(_command, _args, options) {
        envs.push(options?.env);
        return { exitCode: 0, stdout: "ok", stderr: "" };
      }
    };

    try {
      const result = await executeProductionMobileQa({
        repoRoot,
        env: { KANNA_APPIUM_PORT: "4723", KANNA_MOBILE_PORT: "8081" },
        keyPath,
        runner
      });

      expect(result.commands).toHaveLength(4);
      expect(envs.every((env) => env?.KANNA_APP_ENV === "prod")).toBe(true);
      expect(envs.every((env) => env?.KANNA_E2E_DESKTOP_SERVER_URL === "http://127.0.0.1:48120")).toBe(true);
      expect(envs.every((env) => env?.KANNA_OTA_PRIVATE_KEY_PATH === keyPath)).toBe(true);
    } finally {
      await rm(repoRoot, { recursive: true, force: true });
    }
  });

  it("runs the billing review capture with the reviewer credential only in the subprocess environment", async () => {
    const repoRoot = await mkdtemp(join(tmpdir(), "mobile-billing-review-"));
    await mkdir(join(repoRoot, "apps/mobile/src"), { recursive: true });
    await writeFile(
      join(repoRoot, "apps/mobile/src/mobileEnvironments.json"),
      JSON.stringify({
        prod: {
          runtimeVersion: "1.0.0",
          name: "prod",
          displayName: "Kanna",
          scheme: "kanna",
          iosBundleId: "build.kanna.app",
          iosGoogleServicesFile: "./firebase/GoogleService-Info.production.plist",
          firebase: {
            apiKey: "real-key",
            projectId: "kanna-build",
            storageBucket: "kanna-build.firebasestorage.app",
            appId: "1:402613185450:ios:adcedeadcd241285d859d3"
          },
          relayUrl: "wss://relay.kanna.build",
          otaChannel: "production"
        }
      })
    );
    const keyPath = join(repoRoot, "fake-ota-key.pem");
    await writeFile(keyPath, "not an actual private key", { mode: 0o600 });
    const screenshotPath = join(repoRoot, ".tmp", "app-review", "billing.png");
    const report = {
      signedIn: true,
      emailVerified: true,
      accountState: "apple-billing",
      cardRendered: true,
      billingConfirmed: false,
      billingSources: [],
      price: null,
      priceAvailable: false,
      subscribeEnabled: null,
      restoreEnabled: false,
      eulaPresent: true,
      privacyPresent: true,
      message: null,
      screenshotPath,
      ready: false,
      blockers: ["billing-read-unconfirmed", "restore-disabled"]
    };
    const runs: Array<{ args: string[]; env: NodeJS.ProcessEnv | undefined }> = [];
    const runner: CommandRunner = {
      async run(_command, args, options) {
        runs.push({ args, env: options?.env });
        return { exitCode: 0, stdout: `noise\n${JSON.stringify({ billingReview: report })}\n`, stderr: "" };
      }
    };

    try {
      const fromAsc = async () => {
        throw new Error("App Store Connect must not be consulted when the selectors are exported");
      };
      const result = await executeProductionBillingReview({
        repoRoot,
        env: {
          KANNA_APPIUM_PORT: "4723",
          KANNA_E2E_CLOUD_EMAIL: "review@example.com",
          KANNA_E2E_CLOUD_PASSWORD: "review-secret"
        },
        keyPath,
        screenshotPath,
        runner,
        resolveAppStoreReviewerAccount: fromAsc
      });

      expect(runs).toHaveLength(1);
      expect(runs[0]?.args).toEqual(["--dir", join(repoRoot, "apps", "mobile"), "run", "test:e2e:billing-review"]);
      expect(runs[0]?.env).toMatchObject({
        KANNA_APP_ENV: "prod",
        KANNA_E2E_CLOUD_EMAIL: "review@example.com",
        KANNA_E2E_CLOUD_PASSWORD: "review-secret",
        KANNA_E2E_BILLING_REVIEW_SCREENSHOT_PATH: screenshotPath,
        KANNA_OTA_PRIVATE_KEY_PATH: keyPath
      });
      expect(result.credentialSource).toBe("environment");
      expect(result.report).toEqual(report);
      expect(isProductionBillingReviewOk(result)).toBe(false);
      const formatted = formatProductionBillingReviewResult(result);
      expect(formatted).toContain("PASS billing-review:");
      expect(formatted).toContain("ready: no (blockers: billing-read-unconfirmed, restore-disabled)");
      expect(formatted).toContain(`screenshot: ${screenshotPath}`);
      expect(formatted).toContain("waives nothing");
      expect(formatted).not.toContain("review-secret");
      expect(formatted).not.toContain("review@example.com");

      const ready = await executeProductionBillingReview({
        repoRoot,
        env: {},
        keyPath,
        screenshotPath,
        runner: {
          async run() {
            return { exitCode: 0, stdout: JSON.stringify({ billingReview: { ...report, ready: true, blockers: [] } }), stderr: "" };
          }
        },
        resolveAppStoreReviewerAccount: async () => ({ email: "asc-review@example.com", password: "asc-secret" })
      });
      expect(ready.credentialSource).toBe("app-store-connect");
      expect(isProductionBillingReviewOk(ready)).toBe(true);

      await expect(executeProductionBillingReview({
        repoRoot,
        env: {},
        keyPath,
        screenshotPath: ".tmp/relative.png",
        runner,
        resolveAppStoreReviewerAccount: fromAsc
      })).rejects.toThrow("--screenshot-path must be an absolute .png path");
    } finally {
      await rm(repoRoot, { recursive: true, force: true });
    }
  });

  it("selects the reviewer account from exported selectors or the App Store Connect review detail, never a guess", async () => {
    await expect(resolveReviewerCredentials({
      env: { KANNA_E2E_CLOUD_EMAIL: "review@example.com", KANNA_E2E_CLOUD_PASSWORD: "pw" },
      fromAppStoreConnect: async () => null
    })).resolves.toEqual({ email: "review@example.com", password: "pw", source: "environment" });
    await expect(resolveReviewerCredentials({
      env: { KANNA_E2E_CLOUD_EMAIL: "review@example.com" },
      fromAppStoreConnect: async () => null
    })).rejects.toThrow("must be exported together");
    await expect(resolveReviewerCredentials({
      env: {},
      fromAppStoreConnect: async () => ({ email: "asc@example.com", password: "asc-pw" })
    })).resolves.toEqual({ email: "asc@example.com", password: "asc-pw", source: "app-store-connect" });
    await expect(resolveReviewerCredentials({
      env: {},
      fromAppStoreConnect: async () => ({ email: "asc@example.com" })
    })).rejects.toThrow("no review demo account recorded");
    expect(parseBillingReviewReport("no report here")).toBeNull();
    expect(parseBillingReviewReport('{"billingReview":{"ready":true,"blockers":[]}}')).toEqual({ ready: true, blockers: [] });
  });

  it("reads the App Store Connect review demo account for the current mobile version", async () => {
    const repoRoot = await mkdtemp(join(tmpdir(), "mobile-billing-asc-"));
    const home = join(repoRoot, "home");
    await mkdir(join(repoRoot, "apps/mobile"), { recursive: true });
    await writeFile(join(repoRoot, "apps/mobile/VERSION"), "1.0.4\n");
    await mkdir(join(home, ".appstoreconnect/private_keys"), { recursive: true });
    const { privateKey } = generateKeyPairSync("ec", {
      namedCurve: "P-256",
      privateKeyEncoding: { type: "pkcs8", format: "pem" },
      publicKeyEncoding: { type: "spki", format: "pem" }
    });
    await writeFile(join(home, ".appstoreconnect/private_keys/AuthKey_KEY1.p8"), privateKey);
    const responses: Record<string, unknown> = {
      "/v1/apps": { data: [{ id: "app-1", attributes: { bundleId: "build.kanna.app" } }] },
      "/v1/apps/app-1/appStoreVersions": { data: [{ id: "v-104", attributes: { versionString: "1.0.4" } }] },
      "/v1/appStoreVersions/v-104/appStoreReviewDetail": {
        data: { id: "rd-1", attributes: { demoAccountName: "asc-review@example.com", demoAccountPassword: "asc-secret" } }
      }
    };
    const requested: string[] = [];
    try {
      const account = await readAppStoreReviewerAccount({
        env: { APP_STORE_CONNECT_API_KEY_ID: "KEY1", APP_STORE_CONNECT_API_ISSUER_ID: "issuer-1" },
        repoRoot,
        home,
        http: {
          async request(input) {
            const { pathname } = new URL(input.url);
            requested.push(pathname);
            return { status: 200, body: JSON.stringify(responses[pathname] ?? { data: [] }) };
          }
        }
      });
      expect(account).toEqual({ email: "asc-review@example.com", password: "asc-secret" });
      expect(requested).toEqual([
        "/v1/apps",
        "/v1/apps/app-1/appStoreVersions",
        "/v1/appStoreVersions/v-104/appStoreReviewDetail"
      ]);

      await expect(readAppStoreReviewerAccount({
        env: {},
        repoRoot,
        home,
        http: { async request() { return { status: 200, body: "{}" }; } }
      })).rejects.toThrow("mobile billing-review requires APP_STORE_CONNECT_API_KEY_ID and APP_STORE_CONNECT_API_ISSUER_ID");
    } finally {
      await rm(repoRoot, { recursive: true, force: true });
    }
  });

  it("fails production mobile QA before commands when its signing key is unavailable", async () => {
    const repoRoot = await mkdtemp(join(tmpdir(), "mobile-qa-missing-key-"));
    await mkdir(join(repoRoot, "apps/mobile/src"), { recursive: true });
    await writeFile(
      join(repoRoot, "apps/mobile/src/mobileEnvironments.json"),
      JSON.stringify({ prod: {} })
    );
    let commands = 0;
    const runner: CommandRunner = {
      async run() {
        commands += 1;
        return { exitCode: 0, stdout: "", stderr: "" };
      }
    };

    try {
      await expect(executeProductionMobileQa({
        repoRoot,
        env: {},
        keyPath: "/missing/fake-mobile-ota-key.pem",
        runner
      })).rejects.toThrow(/does not exist/);
      expect(commands).toBe(0);
    } finally {
      await rm(repoRoot, { recursive: true, force: true });
    }
  });

  it("reports required command availability for doctor", async () => {
    const runner: CommandRunner = {
      async run(_command, args) {
        return args.at(-1) === "tmux"
          ? { exitCode: 1, stdout: "", stderr: "" }
          : { exitCode: 0, stdout: `/usr/bin/${args.at(-1)}\n`, stderr: "" };
      }
    };

    const result = await checkRequiredCommands(runner, ["git", "pnpm", "tmux"]);

    expect(result.ok).toBe(false);
    expect(result.commands).toEqual([
      { name: "git", found: true, path: "/usr/bin/git" },
      { name: "pnpm", found: true, path: "/usr/bin/pnpm" },
      { name: "tmux", found: false }
    ]);
  });

  it("checks required commands with the real Node runner", async () => {
    const executableDir = await mkdtemp(join(tmpdir(), "kd-doctor-path-"));
    await symlink(process.execPath, join(executableDir, "node"));
    const isolatedPathRunner: CommandRunner = {
      run(command, args, options) {
        return nodeCommandRunner.run(command, args, {
          ...options,
          env: { ...process.env, ...options?.env, PATH: executableDir }
        });
      }
    };

    try {
      const result = await checkRequiredCommands(isolatedPathRunner, ["node"]);

      expect(result).toEqual({
        ok: true,
        commands: [
          {
            name: "node",
            found: true,
            path: join(executableDir, "node")
          }
        ]
      });
    } finally {
      await rm(executableDir, { recursive: true, force: true });
    }
  });

  it("stops a single tmux window without killing the dev session", async () => {
    const calls: string[] = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push(`${command} ${args.join(" ")}`);
        if (args.includes("list-windows")) {
          return { exitCode: 0, stdout: "desktop\nemulators\nmobile\n", stderr: "" };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      }
    };

    await expect(stopTmuxWindow(runner, { server: "kanna-task", session: "kanna-task" }, "emulators")).resolves.toBe(true);

    expect(calls).toEqual([
      "tmux -L kanna-task list-windows -t kanna-task -F #{window_name}",
      "tmux -L kanna-task send-keys -t kanna-task:emulators C-c",
      "tmux -L kanna-task kill-window -t kanna-task:emulators"
    ]);
  });

  it("respawns a single tmux window with the resolved window env", async () => {
    const calls: Array<{ command: string; args: string[]; env?: NodeJS.ProcessEnv; stdin?: string }> = [];
    const runner: CommandRunner = {
      async run(command, args, options) {
        calls.push({ command, args, env: options?.env, stdin: options?.stdin });
        if (args.includes("list-windows")) {
          return { exitCode: 0, stdout: "desktop\nmobile\n", stderr: "" };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      }
    };

    await expect(
      respawnTmuxWindow(
        runner,
        { server: "kanna-task", session: "kanna-task" },
        {
          name: "desktop",
          cwd: "/repo/apps/desktop",
          command: "pnpm exec tauri dev",
          env: {
            KANNA_CLOUD_ENV: "staging",
            KANNA_DESKTOP_AUTO_SIGN_IN_EMAIL: "dev@example.com",
          }
        }
      )
    ).resolves.toEqual({
      windowFound: true,
      state: { exists: true, dead: false, exitCode: undefined }
    });

    expect(calls).toEqual([
      {
        command: "tmux",
        args: ["-L", "kanna-task", "list-windows", "-t", "kanna-task", "-F", "#{window_name}"],
        env: undefined
      },
      {
        command: "tmux",
        args: ["-L", "kanna-task", "set-option", "-t", "kanna-task", "remain-on-exit", "on"],
        env: undefined
      },
      {
        command: "tmux",
        args: [
          "-L", "kanna-task", "display-message", "-p", "-t", "kanna-task:desktop", "#{pane_pid} #{pane_dead}"
        ],
        env: undefined
      },
      {
        command: "tmux",
        args: [
          "-L",
          "kanna-task",
          "source-file",
          "-"
        ],
        env: {
          KANNA_CLOUD_ENV: "staging",
          KANNA_DESKTOP_AUTO_SIGN_IN_EMAIL: "dev@example.com",
        },
        stdin:
          "respawn-window '-k' '-t' 'kanna-task:desktop' '-c' '/repo/apps/desktop' '-e' 'KANNA_DESKTOP_AUTO_SIGN_IN_EMAIL=dev@example.com' 'pnpm exec tauri dev'\n"
      },
      {
        command: "tmux",
        args: [
          "-L", "kanna-task", "display-message", "-p", "-t", "kanna-task:desktop", "#{pane_dead} #{pane_dead_status}"
        ],
        env: undefined
      }
    ]);
    expect(calls.map((call) => call.args.join(" ")).join("\n")).not.toContain("dev@example.com");
  });

  it.skipIf(!tmuxAvailable)("reaps a retained pane child listener and reports replacement readiness", async () => {
    const root = await kdTestScratchDir("kanna-kd-restart-listener-");
    const oldPidPath = join(root, "old.pid");
    const replacementPidPath = join(root, "replacement.pid");
    const listenerPath = join(root, "listener.cjs");
    const wrapperPath = join(root, "wrapper.sh");
    const port = await unusedLoopbackPort();
    const tmuxName = `kanna-restart-${process.pid}-${Date.now()}`;
    const target = { server: tmuxName, session: tmuxName };
    await writeFile(
      listenerPath,
      [
        'const fs = require("node:fs");',
        'const net = require("node:net");',
        'const server = net.createServer();',
        'server.on("error", () => process.exit(17));',
        'server.listen(Number(process.argv[2]), "127.0.0.1", () => fs.writeFileSync(process.argv[3], String(process.pid)));'
      ].join("\n")
    );
    await writeFile(wrapperPath, '#!/bin/sh\nnode "$1" "$2" "$3" &\nwait\n', { mode: 0o755 });
    const oldCommand = ["sh", wrapperPath, listenerPath, String(port), oldPidPath]
      .map((part) => JSON.stringify(part)).join(" ");
    const replacementCommand = ["node", listenerPath, String(port), replacementPidPath]
      .map((part) => JSON.stringify(part)).join(" ");

    try {
      await startTmuxSession(nodeCommandRunner, target, [
        { name: "desktop", cwd: root, command: oldCommand, env: { PATH: process.env.PATH } }
      ]);
      const oldPid = Number(await waitForFile(oldPidPath));

      const respawned = await respawnTmuxWindow(nodeCommandRunner, target, {
        name: "desktop",
        cwd: root,
        command: replacementCommand,
        env: { PATH: process.env.PATH }
      });
      const startup = await waitForTmuxWindowReady(
        nodeCommandRunner,
        target,
        "desktop",
        async () => {
          try {
            await waitForFile(replacementPidPath);
            return true;
          } catch {
            return false;
          }
        },
        { attempts: 20, delayMs: 25 }
      );

      expect(respawned).toEqual({
        windowFound: true,
        state: { exists: true, dead: false, exitCode: undefined }
      });
      expect(startup).toEqual({ ready: true });
      expect(() => process.kill(oldPid, 0)).toThrow();
    } finally {
      await nodeCommandRunner.run("tmux", ["-L", target.server, "kill-server"])
        .catch(() => ({ exitCode: 1, stdout: "", stderr: "" }));
      await rm(root, { recursive: true, force: true });
    }
  });

  it("adds missing windows when the tmux dev session is already running", async () => {
    const calls: string[] = [];
    const runner: CommandRunner = {
      async run(command, args) {
        calls.push(`${command} ${args.join(" ")}`);
        if (args.includes("new-session")) {
          return { exitCode: 1, stdout: "", stderr: "duplicate session: kanna-task" };
        }
        if (args.includes("list-windows")) {
          return { exitCode: 0, stdout: "desktop\n", stderr: "" };
        }
        return { exitCode: 0, stdout: "", stderr: "" };
      }
    };

    await startTmuxSession(
      runner,
      { server: "kanna-task", session: "kanna-task" },
      [
        { name: "desktop", cwd: "/repo/apps/desktop", command: "desktop", env: {} },
        { name: "mobile", cwd: "/repo/apps/mobile", command: "mobile", env: {} },
        { name: "emulators", cwd: "/repo", command: "emulators", env: {} }
      ]
    );

    expect(calls).toEqual([
      "tmux -L kanna-task new-session -d -s kanna-task -n desktop -c /repo/apps/desktop desktop",
      "tmux -L kanna-task list-windows -t kanna-task -F #{window_name}",
      "tmux -L kanna-task set-option -t kanna-task remain-on-exit on",
      "tmux -L kanna-task new-window -t kanna-task -n mobile -c /repo/apps/mobile mobile",
      "tmux -L kanna-task new-window -t kanna-task -n emulators -c /repo emulators"
    ]);
  });

  it.skipIf(!tmuxAvailable)("injects desktop credentials into a real tmux window without exposing them in command text", async () => {
    const root = await mkdtemp(join(tmpdir(), "kanna-kd-real-tmux-"));
    const outputPath = join(root, "desktop-env.json");
    const tmuxName = `kanna-test-${process.pid}-${Date.now()}`;
    const target = {
      server: tmuxName,
      session: tmuxName
    };
    const email = "dev@example.com";
    const password = "do-not-print";
    const command = [
      "node",
      "-e",
      JSON.stringify(
        `require("node:fs").writeFileSync(${JSON.stringify(outputPath)}, JSON.stringify({ email: process.env.KANNA_DESKTOP_AUTO_SIGN_IN_EMAIL, password: process.env.KANNA_DESKTOP_AUTO_SIGN_IN_PASSWORD })); setTimeout(() => {}, 10000);`
      )
    ].join(" ");

    try {
      await startTmuxSession(nodeCommandRunner, target, [
        {
          name: "desktop",
          cwd: root,
          command,
          env: {
            PATH: process.env.PATH,
            KANNA_DESKTOP_AUTO_SIGN_IN_EMAIL: email,
            KANNA_DESKTOP_AUTO_SIGN_IN_PASSWORD: password
          }
        }
      ]);

      await expect(waitForFile(outputPath)).resolves.toBe(
        JSON.stringify({ email, password })
      );

      const windows = await nodeCommandRunner.run("tmux", [
        "-L",
        target.server,
        "list-windows",
        "-t",
        target.session,
        "-F",
        "#{window_name} #{pane_current_command}"
      ]);
      const pane = await nodeCommandRunner.run("tmux", [
        "-L",
        target.server,
        "capture-pane",
        "-t",
        `${target.session}:desktop`,
        "-p",
        "-S",
        "-50"
      ]);
      const visibleTmuxText = `${windows.stdout}\n${windows.stderr}\n${pane.stdout}\n${pane.stderr}`;

      expect(visibleTmuxText).not.toContain(email);
      expect(visibleTmuxText).not.toContain(password);
    } finally {
      await nodeCommandRunner.run("tmux", ["-L", target.server, "kill-session", "-t", target.session])
        .catch(() => ({ exitCode: 1, stdout: "", stderr: "" }));
      await rm(root, { recursive: true, force: true });
    }
  });

  it.skipIf(!tmuxAvailable)("replaces real tmux processes on profile changes and preserves unchanged-profile idempotency", async () => {
    const root = await mkdtemp(join(tmpdir(), "kanna-kd-profile-reconcile-"));
    const outputPath = join(root, "profile.txt");
    const tmuxName = `kanna-profile-${process.pid}-${Date.now()}`;
    const target = { server: tmuxName, session: tmuxName };
    const command = [
      "node",
      "-e",
      JSON.stringify(
        `require("node:fs").writeFileSync(${JSON.stringify(outputPath)}, process.env.KANNA_CLOUD_ENV + ":" + process.pid); setTimeout(() => {}, 30000);`
      )
    ].join(" ");
    const window = (profile: string) => ({
      name: "desktop",
      cwd: root,
      command,
      env: { PATH: process.env.PATH, KANNA_CLOUD_ENV: profile }
    });

    try {
      await startTmuxSession(nodeCommandRunner, target, [window("emulators")], {
        reconcileKey: "dev:cloud=emulators"
      });
      const first = await waitForFile(outputPath);
      expect(first).toMatch(/^emulators:\d+$/);

      await startTmuxSession(nodeCommandRunner, target, [window("emulators")], {
        reconcileKey: "dev:cloud=emulators"
      });
      await new Promise((resolve) => setTimeout(resolve, 100));
      expect(await readFile(outputPath, "utf8")).toBe(first);

      await startTmuxSession(nodeCommandRunner, target, [window("staging")], {
        reconcileKey: "dev:cloud=staging"
      });
      const stagingPid = first.replace("emulators:", "staging:");
      for (let attempt = 0; attempt < 100; attempt += 1) {
        const current = await readFile(outputPath, "utf8");
        if (current.startsWith("staging:") && current !== stagingPid) break;
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      const changed = await readFile(outputPath, "utf8");
      expect(changed).toMatch(/^staging:\d+$/);
      expect(changed.split(":")[1]).not.toBe(first.split(":")[1]);

      await startTmuxSession(nodeCommandRunner, target, [window("staging")], {
        reconcileKey: "dev:cloud=staging"
      });
      await new Promise((resolve) => setTimeout(resolve, 100));
      expect(await readFile(outputPath, "utf8")).toBe(changed);
    } finally {
      await nodeCommandRunner.run("tmux", ["-L", target.server, "kill-session", "-t", target.session])
        .catch(() => ({ exitCode: 1, stdout: "", stderr: "" }));
      await rm(root, { recursive: true, force: true });
    }
  });
});
