import { describe, expect, it } from "vitest";
import type { Browser } from "webdriverio";
import {
  desktopServerModes,
  isDesktopServerMode,
  requiresExactExpoEnvironment,
  prepareSimulatorForLaunch,
  resolveBillingReviewScreenshotPath,
  resolveSimulatorAlertHandling,
  resolveSmokeModeAppEnv,
  smokeSpecPaths,
  supportedSmokeModes,
  supportedSmokeTargets,
  waitForExpoAppReady
} from "./run";
import type { AvailableSimulatorDevice } from "./helpers/simulator";
import { shouldReuseExpoServer } from "./helpers/metro";

describe("mobile smoke runner", () => {
  it("configures the selected simulator and bundle after install validation", async () => {
    const calls: string[] = [];
    const device: AvailableSimulatorDevice = {
      name: "iPhone 17 Pro",
      runtime: "com.apple.CoreSimulator.SimRuntime.iOS-26-2",
      state: "Booted",
      udid: "selected-simulator-udid"
    };

    await prepareSimulatorForLaunch(device, "build.kanna.app.dev", {
      boot: async (received) => { calls.push(`boot:${received.udid}`); },
      assertInstalled: async (received, bundleId) => {
        calls.push(`installed:${received.udid}:${bundleId}`);
      },
      configureExpoDevMenu: async (received, bundleId) => {
        calls.push(`preferences:${received.udid}:${bundleId}`);
      }
    });

    expect(calls).toEqual([
      "boot:selected-simulator-udid",
      "installed:selected-simulator-udid:build.kanna.app.dev",
      "preferences:selected-simulator-udid:build.kanna.app.dev"
    ]);
  });

  it("isolates the StoreKit configuration lane to dev and exact Metro environment", () => {
    expect(supportedSmokeModes).toContain("storekit");
    expect(resolveSmokeModeAppEnv("storekit", "prod")).toBe("dev");
    expect(requiresExactExpoEnvironment("storekit")).toBe(true);
  });
  it("leaves relay and profile alerts manual while preserving other lane policies", () => {
    expect(resolveSimulatorAlertHandling("relay")).toBe("manual");
    expect(resolveSimulatorAlertHandling("profile-disconnected")).toBe("manual");
    expect(resolveSimulatorAlertHandling("hybrid")).toBe("accept");
    expect(resolveSimulatorAlertHandling("smoke")).toBe("dismiss");
  });

  it("pairs every desktop-server mode with the exact selected server, the default smoke included", () => {
    expect(desktopServerModes).toEqual(["smoke", "tab-reselection", "shell-visual"]);
    expect(isDesktopServerMode("smoke")).toBe(true);
    expect(isDesktopServerMode("billing-review")).toBe(false);
    expect(isDesktopServerMode("hybrid")).toBe(false);
    // The claim must target the exact route through the app's explicit
    // development endpoint, so a Metro started without it is not reused.
    for (const mode of desktopServerModes) {
      expect(requiresExactExpoEnvironment(mode)).toBe(true);
    }
    expect(shouldReuseExpoServer(
      {
        cwd: "/repo/apps/mobile",
        commandLine: "KANNA_APP_ENV=dev expo start --dev-client"
      },
      {
        projectRoot: "/repo/apps/mobile",
        requireExactEnvironment: requiresExactExpoEnvironment("smoke"),
        env: { KANNA_APP_ENV: "dev", EXPO_PUBLIC_KANNA_SERVER_URL: "http://127.0.0.1:48121" }
      }
    )).toBe(false);
    expect(shouldReuseExpoServer(
      {
        cwd: "/repo/apps/mobile",
        commandLine: "KANNA_APP_ENV=dev EXPO_PUBLIC_KANNA_SERVER_URL=http://127.0.0.1:48121 expo start --dev-client"
      },
      {
        projectRoot: "/repo/apps/mobile",
        requireExactEnvironment: requiresExactExpoEnvironment("smoke"),
        env: { KANNA_APP_ENV: "dev", EXPO_PUBLIC_KANNA_SERVER_URL: "http://127.0.0.1:48121" }
      }
    )).toBe(true);
  });

  it("registers a production-identity billing review capture mode", () => {
    expect(supportedSmokeModes).toContain("billing-review");
    expect(smokeSpecPaths).toContain("specs/billing-review/billing-review.e2e.ts");
    expect(resolveSmokeModeAppEnv("billing-review", "dev")).toBe("prod");
    expect(resolveSmokeModeAppEnv("billing-review", undefined)).toBe("prod");
    expect(requiresExactExpoEnvironment("billing-review")).toBe(true);
    expect(resolveSimulatorAlertHandling("billing-review")).toBe("dismiss");
  });

  it("requires a caller-controlled absolute .png path for the App Review screenshot", () => {
    expect(resolveBillingReviewScreenshotPath({
      KANNA_E2E_BILLING_REVIEW_SCREENSHOT_PATH: "/repo/.tmp/app-review/billing.PNG"
    })).toBe("/repo/.tmp/app-review/billing.PNG");
    expect(() => resolveBillingReviewScreenshotPath({})).toThrow(
      "KANNA_E2E_BILLING_REVIEW_SCREENSHOT_PATH is required"
    );
    expect(() => resolveBillingReviewScreenshotPath({
      KANNA_E2E_BILLING_REVIEW_SCREENSHOT_PATH: ".tmp/billing.png"
    })).toThrow("must be an absolute .png path");
    expect(() => resolveBillingReviewScreenshotPath({
      KANNA_E2E_BILLING_REVIEW_SCREENSHOT_PATH: "/repo/.tmp/billing.jpg"
    })).toThrow("must be an absolute .png path");
  });

  it("registers the list-detail-back smoke spec", () => {
    expect(smokeSpecPaths).toContain("specs/smoke/list-detail-back.e2e.ts");
  });

  it("registers the profile and Machines smoke spec", () => {
    expect(smokeSpecPaths).toContain("specs/smoke/profile-connection.e2e.ts");
  });

  it("registers the Search focus smoke spec", () => {
    expect(smokeSpecPaths).toContain("specs/smoke/search-focus.e2e.ts");
    expect(supportedSmokeModes).toContain("search-focus");
  });

  it("registers a targeted active-tab reselection smoke mode", () => {
    expect(smokeSpecPaths).toContain("specs/smoke/tab-reselection.e2e.ts");
    expect(supportedSmokeModes).toContain("tab-reselection");
  });

  it("supports both simulator and physical-device targets", () => {
    expect(supportedSmokeTargets).toEqual(["simulator", "device"]);
  });

  it("supports a disconnected profile smoke mode", () => {
    expect(supportedSmokeModes).toContain("profile-disconnected");
    expect(requiresExactExpoEnvironment("profile-disconnected")).toBe(true);
  });

  it("supports a simulator shell visual mode without the PTY fixture", () => {
    expect(supportedSmokeModes).toContain("shell-visual");
    expect(smokeSpecPaths).toContain("specs/smoke/shell-visual.e2e.ts");
  });

  it("dismisses a dev menu that appears after the initial startup poll", async () => {
    let poll = 0;
    let devMenuDismissed = false;
    let devMenuCloseClicks = 0;

    const driver = {
      $: async (selector: string) => ({
        click: async () => {
          if (selector === "~xmark") {
            devMenuCloseClicks += 1;
            devMenuDismissed = true;
          }
        },
        isDisplayed: async () => {
          if (selector === "~xmark") {
            return poll >= 2 && !devMenuDismissed;
          }
          if (selector === "~mobile.app-shell") {
            return devMenuDismissed;
          }
          return false;
        },
        isExisting: async () => false
      }),
      acceptAlert: async () => undefined,
      execute: async () => undefined,
      getAlertText: async () => {
        throw new Error("no alert open");
      },
      getWindowSize: async () => ({ width: 393, height: 852 }),
      waitUntil: async (condition: () => Promise<boolean>) => {
        while (poll < 4) {
          poll += 1;
          if (await condition()) return true;
        }
        throw new Error("condition did not become ready");
      }
    } as unknown as Browser;

    await waitForExpoAppReady(driver);

    expect(poll).toBe(4);
    expect(devMenuCloseClicks).toBe(1);
    expect(devMenuDismissed).toBe(true);
  });

  it("does not return before a dev menu that appears over an already-visible app shell", async () => {
    let poll = 0;
    let devMenuDismissed = false;

    const driver = {
      $: async (selector: string) => ({
        click: async () => {
          if (selector === "~xmark") devMenuDismissed = true;
        },
        isDisplayed: async () => {
          if (selector === "~xmark") {
            return poll === 2 && !devMenuDismissed;
          }
          if (selector === "~mobile.app-shell") return true;
          return false;
        },
        isExisting: async () => false
      }),
      acceptAlert: async () => undefined,
      execute: async () => undefined,
      getAlertText: async () => {
        throw new Error("no alert open");
      },
      getWindowSize: async () => ({ width: 393, height: 852 }),
      waitUntil: async (condition: () => Promise<boolean>) => {
        while (poll < 6) {
          poll += 1;
          if (await condition()) return true;
        }
        throw new Error("condition did not become ready");
      }
    } as unknown as Browser;

    await waitForExpoAppReady(driver);

    expect(poll).toBeGreaterThan(2);
    expect(devMenuDismissed).toBe(true);
  });

  it("fails immediately with the unexpected startup alert text", async () => {
    const driver = {
      $: async () => ({
        click: async () => undefined,
        isDisplayed: async () => false,
        isExisting: async () => false
      }),
      getAlertText: async () => "Open in Kanna?",
      waitUntil: async (condition: () => Promise<boolean>) => {
        await condition();
        throw new Error("condition did not become ready");
      }
    } as unknown as Browser;

    await expect(waitForExpoAppReady(driver)).rejects.toThrow(
      'Mobile startup is blocked by a system alert: "Open in Kanna?"'
    );
  });

  it("accepts a dynamic relaunch selector while handling Expo overlays", async () => {
    let poll = 0;
    const readySelector: string = "~mobile.toolbar.tab.recent";
    const driver = {
      $: async (selector: string) => ({
        click: async () => undefined,
        isDisplayed: async () => {
          if (selector === "~mobile.app-shell") return true;
          if (selector === "~mobile.toolbar.tab.recent") return poll >= 4;
          return false;
        },
        isExisting: async () => false
      }),
      acceptAlert: async () => undefined,
      execute: async () => undefined,
      getAlertText: async () => {
        throw new Error("no alert open");
      },
      getWindowSize: async () => ({ width: 393, height: 852 }),
      waitUntil: async (condition: () => Promise<boolean>) => {
        while (poll < 8) {
          poll += 1;
          if (await condition()) return true;
        }
        throw new Error("condition did not become ready");
      }
    } as unknown as Browser;

    await waitForExpoAppReady(driver, readySelector);

    expect(poll).toBeGreaterThanOrEqual(4);
  });

  it("supports a force-cloud smoke mode", () => {
    expect(supportedSmokeModes).toContain("cloud");
    expect(smokeSpecPaths).toContain("specs/cloud/cloud-task-flow.e2e.ts");
  });

  it("supports a relay-backed Appium mode", () => {
    expect(supportedSmokeModes).toContain("relay");
    expect(smokeSpecPaths).toContain("specs/relay/relay-task-flow.e2e.ts");
    expect(requiresExactExpoEnvironment("relay")).toBe(true);
    expect(requiresExactExpoEnvironment("relay-terminal-control")).toBe(true);
  });

  it("does not reuse a Metro server configured for another relay environment", () => {
    expect(shouldReuseExpoServer(
      {
        cwd: "/repo/apps/mobile",
        commandLine: "KANNA_APP_ENV=dev EXPO_PUBLIC_KANNA_RELAY_URL=https://other.example expo start --dev-client",
      },
      {
        projectRoot: "/repo/apps/mobile",
        requireExactEnvironment: true,
        env: { KANNA_APP_ENV: "dev", EXPO_PUBLIC_KANNA_RELAY_URL: "https://relay.example" },
      },
    )).toBe(false);
  });

  it("supports a signed-in cloud plus trusted-LAN hybrid Appium mode", () => {
    expect(supportedSmokeModes).toContain("hybrid");
    expect(smokeSpecPaths).toContain("specs/hybrid/hybrid-task-flow.e2e.ts");
    expect(resolveSmokeModeAppEnv("hybrid", undefined)).toBe("dev");
    expect(resolveSmokeModeAppEnv("relay", "staging")).toBe("staging");
    expect(requiresExactExpoEnvironment("hybrid")).toBe(true);
    expect(requiresExactExpoEnvironment("cloud")).toBe(false);
  });
});
