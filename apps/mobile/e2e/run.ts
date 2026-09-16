import { execFile } from "node:child_process";
import { cp, mkdir, mkdtemp, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath, pathToFileURL } from "node:url";
import type { Browser } from "webdriverio";

const execFileAsync = promisify(execFile);
import { processIdentity, processInventoryPath, recordInventoryResource, removeInventoryResource, terminateInventoryProcess } from "../../../tools/kd/src/runtime/process-inventory";
import {
  createPhysicalDeviceCapabilities,
  createSimulatorCapabilities,
  type SimulatorAlertHandling
} from "./appium.config";
import {
  assertXcuitestDriverInstalled,
  listXcuitestConnectedDeviceUdids,
  startLocalAppiumServer,
  stopSimulatorWebDriverAgent,
  waitForLocalAppiumServer
} from "./helpers/appium";
import {
  assertDesktopServerReachable,
  resolveDesktopServerUrlForTarget
} from "./helpers/desktop";
import {
  pairExactDesktopThroughDeepLink,
  resolveDesktopServerExpoEnv,
  waitForExactDesktopPairing,
  withConnectionDiagnostics
} from "./helpers/desktop-pairing";
import { ensureExpoServer } from "./helpers/metro";
import {
  assertPhysicalDeviceAppInstalled,
  resolvePhysicalDevice
} from "./helpers/device";
import { resolveRequiredMobileE2eEnv } from "./helpers/env";
import { createMobileSession } from "./helpers/session";
import { selectors } from "./helpers/selectors";
import { seedPairedTrustedDesktopThroughDeepLink } from "./helpers/trust-seed";
import {
  assertSimulatorAppInstalled,
  buildSimulatorDevelopmentClientLaunchArgs,
  bootSimulator,
  configureSimulatorExpoDevMenuPreferences,
  openSimulatorDevelopmentClient,
  resolveSimulatorDevice,
  type AvailableSimulatorDevice
} from "./helpers/simulator";
import { runBillingReviewCapture } from "./specs/billing-review/billing-review.e2e";
import { runListDetailBackSmoke } from "./specs/smoke/list-detail-back.e2e";
import {
  runProfileConnectionSmoke,
  runProfileDisconnectedConnectionSmoke
} from "./specs/smoke/profile-connection.e2e";
import { runSearchFocusSmoke } from "./specs/smoke/search-focus.e2e";
import { runShellVisualSmoke } from "./specs/smoke/shell-visual.e2e";
import { runTabReselectionSmoke } from "./specs/smoke/tab-reselection.e2e";
import { runCloudTaskFlow } from "./specs/cloud/cloud-task-flow.e2e";
import { runHybridTaskFlow } from "./specs/hybrid/hybrid-task-flow.e2e";
import { runRelayTaskFlow, runRelayTerminalControlJourney } from "./specs/relay/relay-task-flow.e2e";
import { startMobileRelayHarness } from "./helpers/relay-harness";

export const smokeSpecPaths = [
  "specs/billing-review/billing-review.e2e.ts",
  "specs/cloud/cloud-task-flow.e2e.ts",
  "specs/hybrid/hybrid-task-flow.e2e.ts",
  "specs/relay/relay-task-flow.e2e.ts",
  "specs/smoke/list-detail-back.e2e.ts",
  "specs/smoke/profile-connection.e2e.ts",
  "specs/smoke/search-focus.e2e.ts",
  "specs/smoke/shell-visual.e2e.ts",
  "specs/smoke/tab-reselection.e2e.ts"
];
export const supportedSmokeTargets = ["simulator", "device"] as const;
export const supportedSmokeModes = [
  "smoke",
  "billing-review",
  "storekit",
  "search-focus",
  "tab-reselection",
  "shell-visual",
  "profile-disconnected",
  "cloud",
  "relay", "relay-terminal-control",
  "hybrid"
] as const;

interface SimulatorSetupDependencies {
  boot: typeof bootSimulator;
  assertInstalled: typeof assertSimulatorAppInstalled;
  configureExpoDevMenu: typeof configureSimulatorExpoDevMenuPreferences;
}

export async function prepareSimulatorForLaunch(
  device: AvailableSimulatorDevice,
  bundleId: string,
  dependencies: SimulatorSetupDependencies = {
    boot: bootSimulator,
    assertInstalled: assertSimulatorAppInstalled,
    configureExpoDevMenu: configureSimulatorExpoDevMenuPreferences
  }
): Promise<void> {
  await dependencies.boot(device);
  await dependencies.assertInstalled(device, bundleId);
  await dependencies.configureExpoDevMenu(device, bundleId);
}

/**
 * Modes that drive a real desktop `kanna-server` named by
 * `KANNA_E2E_DESKTOP_SERVER_URL`. They pair the app with that exact server
 * through the app's own pairing claim rather than seeding identity-only trust.
 */
export const desktopServerModes = ["smoke", "tab-reselection", "shell-visual"] as const;

export function isDesktopServerMode(mode: string): boolean {
  return (desktopServerModes as readonly string[]).includes(mode);
}

export function resolveSmokeModeAppEnv(
  mode: string,
  configuredAppEnv: string | undefined
): string | undefined {
  if (mode === "billing-review") {
    // The Apple billing card only exists under the production identity.
    return "prod";
  }
  return mode === "storekit" || mode === "hybrid" || mode === "search-focus"
    ? "dev"
    : configuredAppEnv;
}

/**
 * The App Review screenshot lands where the caller says — a task's own `.tmp`
 * — never in a runner-chosen or committed location.
 */
export function resolveBillingReviewScreenshotPath(
  env: Record<string, string | undefined>
): string {
  const path = env.KANNA_E2E_BILLING_REVIEW_SCREENSHOT_PATH?.trim();
  if (!path) {
    throw new Error(
      "KANNA_E2E_BILLING_REVIEW_SCREENSHOT_PATH is required: an absolute .png path under the calling task's .tmp directory."
    );
  }
  if (!path.startsWith("/") || !path.toLowerCase().endsWith(".png")) {
    throw new Error(
      `KANNA_E2E_BILLING_REVIEW_SCREENSHOT_PATH must be an absolute .png path, got ${JSON.stringify(path)}.`
    );
  }
  return path;
}

export function requiresExactExpoEnvironment(mode: string): boolean {
  // Desktop-server modes pair the app with the exact server named by
  // KANNA_E2E_DESKTOP_SERVER_URL through the app's existing explicit
  // development endpoint (EXPO_PUBLIC_KANNA_SERVER_URL). A Metro that was
  // started without that route would leave the claim to whatever Bonjour
  // resolves, so such a Metro is never reused: export the same
  // EXPO_PUBLIC_KANNA_SERVER_URL before `kd dev up --mobile`, or run
  // `./kd dev down` and let the smoke start its own Metro.
  return (
    isDesktopServerMode(mode) ||
    mode === "storekit" || mode === "billing-review" ||
    mode === "relay" || mode === "relay-terminal-control" ||
    mode === "hybrid" ||
    mode === "profile-disconnected" ||
    mode === "search-focus"
  );
}

export function resolveSimulatorAlertHandling(
  mode: string
): SimulatorAlertHandling {
  if (mode === "hybrid" || mode === "search-focus") {
    return "accept";
  }
  if (mode === "relay" || mode === "relay-terminal-control" || mode === "profile-disconnected") {
    return "manual";
  }
  return "dismiss";
}

async function isDisplayed(driver: Browser, selector: string): Promise<boolean> {
  const element = await driver.$(selector);
  return element.isDisplayed().catch(() => false);
}

async function handleStartupSystemAlert(driver: Browser): Promise<void> {
  const alertText = await driver.getAlertText().catch(() => null);
  if (alertText) {
    if (!isBonjourPermissionAlert(alertText)) {
      throw new Error(
        `Mobile startup is blocked by a system alert: ${JSON.stringify(alertText)}`
      );
    }
    await driver.acceptAlert();
  }
}

async function dismissExpoStartupOverlay(driver: Browser): Promise<void> {
  const continueButton = await driver.$("~Continue");
  if (await continueButton.isDisplayed().catch(() => false)) {
    await continueButton.click();
  }
  const devMenuCloseButton = await driver.$("~xmark");
  if (await devMenuCloseButton.isDisplayed().catch(() => false)) {
    await devMenuCloseButton.click();
  } else {
    const devMenuMarker = await driver.$("~Toggle performance monitor");
    if (await devMenuMarker.isExisting()) {
      const closePoint = resolveExpoDevMenuClosePoint(await driver.getWindowSize());
      await driver.execute("mobile: tap", closePoint);
    }
  }
}

export async function waitForExpoAppReady(
  driver: Browser,
  readySelector: string = selectors.appShell
): Promise<void> {
  // XCUITest answers a missing alert with a WebDriver error. Do this once at
  // launch rather than on every readiness poll: a real iOS launch gate is
  // reported immediately, while the normal no-alert case cannot flood the
  // Appium log or turn into an unbounded alert probe loop.
  await handleStartupSystemAlert(driver);
  let consecutiveReadyPolls = 0;
  await driver.waitUntil(
    async () => {
      await dismissExpoStartupOverlay(driver);
      if (await isDisplayed(driver, readySelector)) {
        consecutiveReadyPolls += 1;
      } else {
        consecutiveReadyPolls = 0;
      }
      return consecutiveReadyPolls >= 3;
    },
    {
      interval: 250,
      timeout: 90_000,
      timeoutMsg:
        "Kanna's mobile app shell did not become visible after Expo startup overlays were handled."
    }
  );
}

export function isBonjourPermissionAlert(text: string): boolean {
  return /local network|find and connect|devices on your local network/i.test(text);
}

export function resolveExpoDevMenuClosePoint(viewport: {
  width: number;
  height: number;
}): { x: number; y: number } {
  return {
    x: viewport.width - 40,
    y: Math.round(viewport.height * 0.48)
  };
}

async function main(): Promise<void> {
  const mode = process.argv[2] ?? "smoke";
  if (!supportedSmokeModes.includes(mode as (typeof supportedSmokeModes)[number])) {
    throw new Error(`Unsupported mobile E2E mode: ${mode}`);
  }
  const modeAppEnv = resolveSmokeModeAppEnv(mode, process.env.KANNA_APP_ENV);
  if (modeAppEnv) {
    process.env.KANNA_APP_ENV = modeAppEnv;
  }

  const relayHarnessOwnsDesktopEndpoint =
    mode === "relay" || mode === "relay-terminal-control" || mode === "hybrid" || mode === "profile-disconnected";
  const needsNoDesktopServer = mode === "storekit" || mode === "billing-review";
  const env = resolveRequiredMobileE2eEnv(
    process.env as Record<string, string | undefined>,
    { requireDesktopServerUrl: !relayHarnessOwnsDesktopEndpoint && !needsNoDesktopServer },
  );
  const desktopServerUrl = relayHarnessOwnsDesktopEndpoint || needsNoDesktopServer
    ? ""
    : resolveDesktopServerUrlForTarget(env.desktopServerUrl, env.target);
  const billingReviewScreenshotPath = mode === "billing-review"
    ? resolveBillingReviewScreenshotPath(process.env as Record<string, string | undefined>)
    : null;
  if ((mode === "storekit" || mode === "billing-review" || mode === "hybrid" || mode === "profile-disconnected") && env.target !== "simulator") {
    throw new Error(
      `The mobile ${mode} E2E mode is simulator-only; it must not install or launch a physical device.`
    );
  }
  if (mode === "shell-visual" && env.target !== "simulator") {
    throw new Error(
      "The mobile shell visual E2E mode is simulator-only so screenshot geometry and colors remain pinned."
    );
  }
  if (mode === "relay-terminal-control" && env.target !== "simulator") {
    throw new Error(
      "The focused relay terminal-control journey is simulator-only because it requires retained simctl screenshots."
    );
  }
  await assertXcuitestDriverInstalled(process.env as Record<string, string | undefined>);
  const appiumServer = startLocalAppiumServer(
    env.appiumPort,
    process.env as Record<string, string | undefined>
  );
  const mobileRepoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
  const mobileInventoryPath = processInventoryPath(mobileRepoRoot);
  const appiumResource = appiumServer.pid
    ? recordInventoryResource(mobileInventoryPath, { kind: "process" as const, pid: appiumServer.pid, label: "mobile-e2e-appium", identity: processIdentity(appiumServer.pid) })
    : undefined;
  let driver: Browser | null = null;
  let expoServer: Awaited<ReturnType<typeof ensureExpoServer>> | null = null;
  let relayHarness: Awaited<ReturnType<typeof startMobileRelayHarness>> | null = null;
  let simulatorDevice: AvailableSimulatorDevice | null = null;
  let shuttingDown = false;
  const stopWda = async () => {
    if (!simulatorDevice || shuttingDown) return;
    shuttingDown = true;
    await stopSimulatorWebDriverAgent(simulatorDevice.udid);
  };
  const handleTermination = () => {
    void stopWda().finally(() => process.exit(1));
  };
  process.once("SIGINT", handleTermination);
  process.once("SIGTERM", handleTermination);

  try {
    await waitForLocalAppiumServer(env.appiumPort);

    let capabilities: Record<string, unknown>;

    if (env.target === "device") {
      const appiumVisibleUdids = await listXcuitestConnectedDeviceUdids(
        process.env as Record<string, string | undefined>
      );
      const device = await resolvePhysicalDevice(
        env.deviceUdid,
        appiumVisibleUdids,
        env.physicalDeviceName
      );
      await assertPhysicalDeviceAppInstalled(device, env.bundleId, env.metroPort);
      capabilities = createPhysicalDeviceCapabilities({
        appiumPort: env.appiumPort,
        bundleId: env.bundleId,
        deviceName: device.name,
        deviceUdid: device.udid,
        platformVersion: device.platformVersion,
        xcodeOrgId: env.xcodeOrgId,
        xcodeSigningId: env.xcodeSigningId,
        updatedWdaBundleId: env.updatedWdaBundleId,
        reservedPorts: env.reservedPorts
      });
    } else {
      const device = await resolveSimulatorDevice(env.deviceName);
      simulatorDevice = device;
      await prepareSimulatorForLaunch(device, env.bundleId);
      capabilities = createSimulatorCapabilities({
        appiumPort: env.appiumPort,
        alertHandling: resolveSimulatorAlertHandling(mode),
        bundleId: env.bundleId,
        deviceName: device.name,
        reservedPorts: env.reservedPorts
      });
      // Pin the simulator already selected and checked above. A custom name
      // alone lets Appium try to create a different device on another runtime.
      capabilities["appium:udid"] = device.udid;
    }

    const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
    const resolvedDesktopServerUrl = desktopServerUrl;

    if (isDesktopServerMode(mode)) {
      await assertDesktopServerReachable(resolvedDesktopServerUrl);
    }
    if (
      mode === "relay" ||
      mode === "relay-terminal-control" ||
      mode === "hybrid" ||
      mode === "profile-disconnected" ||
      mode === "search-focus"
    ) {
      relayHarness = await startMobileRelayHarness({
        mode: (mode === "relay" || mode === "relay-terminal-control") ? "relay" : "hybrid"
      });
    }

    expoServer = await ensureExpoServer({
      env:
        mode === "storekit" ? { KANNA_APP_ENV: "dev", EXPO_PUBLIC_KANNA_STOREKIT_TEST: "1" } :
        mode === "billing-review" ? { KANNA_APP_ENV: "prod" } :
        isDesktopServerMode(mode)
          ? resolveDesktopServerExpoEnv({
              appEnv: env.appEnv,
              appDesktopServerUrl: resolvedDesktopServerUrl
            })
          :
        (mode === "hybrid" ||
          mode === "profile-disconnected" ||
          mode === "search-focus") &&
        relayHarness
          ? relayHarness.hybridEnv
          : (mode === "relay" || mode === "relay-terminal-control") && relayHarness
          ? relayHarness.env
          :
        mode === "cloud"
          ? {
              EXPO_PUBLIC_KANNA_FORCE_CLOUD: "1",
              KANNA_APP_ENV: env.appEnv
            }
          : { KANNA_APP_ENV: env.appEnv },
      metroPort: env.metroPort,
      privateKeyPath: env.otaPrivateKeyPath,
      projectRoot,
      requireExactEnvironment: requiresExactExpoEnvironment(mode)
    });

    driver = await createMobileSession({
      port: env.appiumPort,
      capabilities
    });
    if (simulatorDevice) {
      await openSimulatorDevelopmentClient({
        appScheme: env.appScheme,
        bundleId: env.bundleId,
        device: simulatorDevice,
        metroPort: env.metroPort
      });
      if (mode === "storekit") await (await driver.$("~storekit-test-result")).waitForExist({ timeout: 90_000 });
      else await waitForExpoAppReady(driver);
    }

    if (mode === "storekit") {
      const result = await driver.$("~storekit-test-result");
      await driver.waitUntil(async () => /^(passed|failed):/.test(await result.getText()), { timeout: 180_000, timeoutMsg: "Native StoreKit test did not finish" });
      const text = await result.getText();
      if (!text.startsWith("passed:")) throw new Error(text);
      process.stdout.write(`${text}\n`);
      // Relaunch without another purchase. Reinstall is a separate opt-in
      // diagnostic: local StoreKit history did not survive removal on iOS 18.4;
      // real sandbox reinstall acceptance remains required.
      const device = simulatorDevice!;
      if (env.bundleId !== "build.kanna.app.dev") throw new Error("StoreKit reinstall is dev-only");
      const reinstall = process.env.KANNA_STOREKIT_CHECK_REINSTALL === "1";
      const { stdout: container } = await execFileAsync("xcrun", ["simctl", "get_app_container", device.udid, env.bundleId, "app"]);
      const tempRoot = resolve(projectRoot, "../../.tmp");
      await mkdir(tempRoot, { recursive: true });
      const saved = await mkdtemp(join(tempRoot, "storekit-reinstall-"));
      try {
        const appPath = join(saved, "KannaDev.app");
        if (reinstall) await cp(container.trim(), appPath, { recursive: true });
        await execFileAsync("xcrun", ["simctl", "terminate", device.udid, env.bundleId]);
        if (reinstall) {
          await execFileAsync("xcrun", ["simctl", "uninstall", device.udid, env.bundleId]);
          await execFileAsync("xcrun", ["simctl", "install", device.udid, appPath]);
        }
        await execFileAsync("xcrun", [...buildSimulatorDevelopmentClientLaunchArgs({
          appScheme: env.appScheme, bundleId: env.bundleId, deviceUdid: device.udid, metroPort: env.metroPort,
        }), "--storekit-restore"]);
        await (await driver.$("~storekit-test-result")).waitForExist({ timeout: 90_000 });
        const restored = await driver.$("~storekit-test-result");
        await driver.waitUntil(async () => /^(passed|failed):/.test(await restored.getText()), { timeout: 90_000 });
        const restoredText = await restored.getText();
        if (restoredText !== "passed: relaunch restore") throw new Error(restoredText);
        process.stdout.write(`passed: ${reinstall ? "reinstall" : "process restart"} restore\n`);
      } finally { await rm(saved, { recursive: true, force: true }); }
    } else if (mode === "billing-review") {
      if (!billingReviewScreenshotPath) {
        throw new Error("The billing review screenshot path was not resolved before launch.");
      }
      const report = await runBillingReviewCapture(driver, {
        credentials: { email: env.cloudEmail, password: env.cloudPassword },
        screenshotPath: billingReviewScreenshotPath
      });
      process.stdout.write(`${JSON.stringify({ billingReview: report })}\n`);
    } else if (mode === "shell-visual") {
      const smokeDriver = driver;
      const identity = await pairExactDesktopThroughDeepLink({
        bundleId: env.bundleId,
        driver: smokeDriver,
        configuredDesktopServerUrl: env.desktopServerUrl,
        appDesktopServerUrl: resolvedDesktopServerUrl
      });
      await waitForExactDesktopPairing(smokeDriver, {
        desktopId: identity.desktopId,
        appDesktopServerUrl: resolvedDesktopServerUrl
      });
      await withConnectionDiagnostics(smokeDriver, "shell visual smoke", () =>
        runShellVisualSmoke(smokeDriver)
      );
    } else if (mode === "profile-disconnected" && relayHarness) {
      await runProfileDisconnectedConnectionSmoke(driver, {
        bundleId: env.bundleId,
        createPairingSession: relayHarness.createPairingSession,
        credentials: relayHarness.credentials,
        desktopId: relayHarness.hybridFixture.desktop.desktopId,
        expirePairingSession: relayHarness.expirePairingSession,
        hybridFixture: relayHarness.hybridFixture,
        reopenDevelopmentClient: async () => {
          if (!simulatorDevice) {
            throw new Error("Profile machine E2E requires a simulator");
          }
          await openSimulatorDevelopmentClient({
            appScheme: env.appScheme,
            bundleId: env.bundleId,
            device: simulatorDevice,
            metroPort: env.metroPort
          });
        },
        setLanHttpEnabled: relayHarness.setLanHttpEnabled,
        waitForAppReady: (readySelector) =>
          waitForExpoAppReady(driver!, readySelector)
      });
    } else if (mode === "relay-terminal-control" && relayHarness) {
      await runRelayTerminalControlJourney(driver, {
        credentials: relayHarness.credentials, fixture: relayHarness.fixture,
        observeAuthoritativeTerminalGeometry: relayHarness.observeAuthoritativeTerminalGeometry,
        restoreDesktopTerminalControl: relayHarness.restoreDesktopTerminalControl,
        async captureScreenshot(name) {
          if (!simulatorDevice) {
            throw new Error("Focused relay terminal-control screenshots require a simulator device");
          }
          const dir = join(projectRoot, "../..", "docs/task-screenshots/5c82e022-screenshots");
          await mkdir(dir, { recursive: true });
          await execFileAsync("xcrun", ["simctl", "io", simulatorDevice.udid, "screenshot", join(dir, `${name}.png`)]);
        },
      });
    } else if (mode === "relay" && relayHarness) {
      await runRelayTaskFlow(driver, {
        bundleId: env.bundleId,
        companion: relayHarness.companion,
        credentials: relayHarness.credentials,
        emitFilePreviewLinks: relayHarness.emitFilePreviewLinks,
        filePreview: relayHarness.filePreview,
        draft: relayHarness.quickReply.draft,
        customizedReply: relayHarness.quickReply.text,
        fixture: relayHarness.fixture,
        observeAuthoritativeTerminalGeometry:
          relayHarness.observeAuthoritativeTerminalGeometry,
        prepareTaskUnreadForMarkRead: relayHarness.prepareTaskUnreadForMarkRead,
        setTaskBusyRead: relayHarness.setTaskBusyRead,
        restoreTallTerminalGeometry: relayHarness.restoreTallTerminalGeometry,
        restoreDesktopTerminalControl: relayHarness.restoreDesktopTerminalControl,
        dropRelayTunnels: relayHarness.dropRelayTunnels,
        resyncTerminalConnection: relayHarness.resyncTerminalConnection,
        setTaskBusyUnread: relayHarness.setTaskBusyUnread,
        setTaskActivity: relayHarness.setTaskActivity,
        taskRow: relayHarness.taskRow,
        taskOrdering: relayHarness.taskOrdering,
        terminalKeys: relayHarness.terminalKeys,
        // Visual verification for the changed states, captured from the lane
        // that already drives them. Written outside the bundle, never committed.
        async captureScreenshot(name) {
          if (!simulatorDevice) return;
          const dir = join(
            projectRoot,
            "../..",
            "docs/task-screenshots/8f342d4f-screenshots"
          );
          await mkdir(dir, { recursive: true });
          await execFileAsync("xcrun", [
            "simctl", "io", simulatorDevice.udid, "screenshot",
            join(dir, `${name}.png`)
          ]);
        },
        waitForAppReady: (readySelector) =>
          waitForExpoAppReady(driver!, readySelector),
        waitForLocalTaskActivity: relayHarness.waitForLocalTaskActivity,
        waitForMobileTerminalGeometry:
          relayHarness.waitForMobileTerminalGeometry,
        async waitForQuickReplyInput() {
          await relayHarness!.waitForQuickReplyInput(
            relayHarness!.quickReply.expectedInput
          );
        }
      });
    } else if (mode === "hybrid" && relayHarness) {
      await seedPairedTrustedDesktopThroughDeepLink({
        bundleId: env.bundleId,
        createPairingSession: relayHarness.createPairingSession,
        driver,
        desktop: {
          desktopId: relayHarness.hybridFixture.desktop.desktopId,
          displayName: relayHarness.hybridFixture.desktop.displayName,
          lanBaseUrl: relayHarness.hybridFixture.desktop.lanBaseUrl
        },
        selectedTaskId: relayHarness.hybridFixture.unresolvedTaskId
      });
      await runHybridTaskFlow(driver, {
        bundleId: env.bundleId,
        companion: relayHarness.companion,
        credentials: relayHarness.credentials,
        fixture: relayHarness.hybridFixture,
        publishCloudRefresh: () => relayHarness!.publishHybridCloudRefresh(),
        stopRelay: () => relayHarness!.harness.stopRelay()
      });
    } else if (mode === "search-focus" && relayHarness) {
      await seedPairedTrustedDesktopThroughDeepLink({
        bundleId: env.bundleId,
        createPairingSession: relayHarness.createPairingSession,
        driver,
        desktop: relayHarness.hybridFixture.desktop
      });
      await runSearchFocusSmoke(driver, {
        screenshotPath: process.env.KANNA_E2E_SEARCH_SCREENSHOT_PATH?.trim(),
        stopAfterTaskIdSearch: true,
        taskId: relayHarness.hybridFixture.duplicate.localTaskId
      });
    } else if (mode === "cloud") {
      await runCloudTaskFlow(driver, {
        email: env.cloudEmail,
        password: env.cloudPassword
      });
    } else {
      // Genuine pairing with the exact selected server before any task-list
      // deadline starts: the app must hold this desktop's device secret and
      // endpoint, or the row wait below could only ever prove "no rows".
      const identity = await pairExactDesktopThroughDeepLink({
        bundleId: env.bundleId,
        driver,
        configuredDesktopServerUrl: env.desktopServerUrl,
        appDesktopServerUrl: resolvedDesktopServerUrl
      });
      await waitForExactDesktopPairing(driver, {
        desktopId: identity.desktopId,
        appDesktopServerUrl: resolvedDesktopServerUrl
      });
      const smokeDriver = driver;
      if (mode === "tab-reselection") {
        await withConnectionDiagnostics(smokeDriver, "tab reselection smoke", () =>
          runTabReselectionSmoke(smokeDriver)
        );
      } else {
        await withConnectionDiagnostics(smokeDriver, "list-detail-back smoke", () =>
          runListDetailBackSmoke(smokeDriver, {
            desktopServerUrl: resolvedDesktopServerUrl
          })
        );
        await withConnectionDiagnostics(smokeDriver, "search focus smoke", () =>
          runSearchFocusSmoke(smokeDriver, {
            screenshotPath: process.env.KANNA_E2E_SEARCH_SCREENSHOT_PATH?.trim(),
            taskId: process.env.KANNA_E2E_PTY_TASK_ID?.trim()
          })
        );
        await withConnectionDiagnostics(smokeDriver, "tab reselection smoke", () =>
          runTabReselectionSmoke(smokeDriver)
        );
        if (env.target === "simulator") {
          await runShellVisualSmoke(smokeDriver);
        }
        await withConnectionDiagnostics(smokeDriver, "profile connection smoke", () =>
          runProfileConnectionSmoke(smokeDriver)
        );
      }
    }
  } finally {
    if (driver) {
      await driver.deleteSession();
    }
    if (appiumResource?.kind === "process") {
      const outcome = await terminateInventoryProcess(appiumResource);
      if (outcome !== "failed") removeInventoryResource(mobileInventoryPath, appiumResource);
    }
    await expoServer?.stop();
    await relayHarness?.stop();
    await stopWda();
    process.removeListener("SIGINT", handleTermination);
    process.removeListener("SIGTERM", handleTermination);
  }
}

const isEntrypoint =
  typeof process.argv[1] === "string" &&
  pathToFileURL(process.argv[1]).href === import.meta.url;

if (isEntrypoint) {
  void main().catch((error: unknown) => {
    const message = error instanceof Error ? error.message : String(error);
    process.stderr.write(`${message}\n`);
    process.exitCode = 1;
  });
}
