import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  setDesktopReadinessConfirmedForTests,
  updateDesktopServerClientHandlersForTests,
} from "./desktopServerClient";
import {
  localServicesFailure,
  localServicesState,
  resetLocalServicesForTests,
  waitForLocalServices,
  waitForLocalServicesStartupGrace,
} from "./localServices";

describe("localServices", () => {
  let warnSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    // `ensureDesktopReady` caches its own confirmation for the window's life.
    setDesktopReadinessConfirmedForTests(false);
    resetLocalServicesForTests({ retryDelayMs: 1, startupGraceMs: 5 });
  });

  afterEach(() => {
    warnSpy.mockRestore();
    updateDesktopServerClientHandlersForTests({ ensureDesktopReady: undefined });
    setDesktopReadinessConfirmedForTests(false);
    resetLocalServicesForTests();
  });

  it("reports ready once the native readiness gate answers", async () => {
    updateDesktopServerClientHandlersForTests({ ensureDesktopReady: async () => {} });

    await expect(waitForLocalServicesStartupGrace()).resolves.toBe(true);
    expect(localServicesState().value).toBe("ready");
    expect(localServicesFailure().value).toBeNull();
  });

  // The whole point: a server that is not answering is a reported state, not a
  // thrown startup failure.
  it("answers the grace period without throwing while services are down", async () => {
    updateDesktopServerClientHandlersForTests({
      ensureDesktopReady: async () => {
        throw new Error("kanna-server is not running");
      },
    });

    await expect(waitForLocalServicesStartupGrace()).resolves.toBe(false);
    expect(localServicesState().value).toBe("unavailable");
    expect(localServicesFailure().value).toBe("kanna-server is not running");
  });

  it("keeps retrying past the grace period and recovers on its own", async () => {
    let attempts = 0;
    let responsive = false;
    updateDesktopServerClientHandlersForTests({
      ensureDesktopReady: async () => {
        attempts += 1;
        if (!responsive) throw new Error("kanna-server is not running");
      },
    });

    // The window stops waiting here; the retry behind it does not.
    expect(await waitForLocalServicesStartupGrace()).toBe(false);
    responsive = true;
    expect(await waitForLocalServices()).toBe(true);
    expect(localServicesState().value).toBe("ready");
    expect(attempts).toBeGreaterThan(1);
  });

  // The grace belongs to the window: `main.ts` spends it before mounting and
  // `useAppLifecycle` asks again afterwards, so charging it twice would keep a
  // degraded window behind a blank screen for double the intended wait.
  it("spends the startup grace once per window", async () => {
    updateDesktopServerClientHandlersForTests({
      ensureDesktopReady: async () => {
        throw new Error("kanna-server is not running");
      },
    });

    expect(await waitForLocalServicesStartupGrace()).toBe(false);
    const secondWaitStartedAt = Date.now();
    expect(await waitForLocalServicesStartupGrace()).toBe(false);

    expect(Date.now() - secondWaitStartedAt).toBeLessThan(5);
  });

  it("runs one shared attempt for every waiting window", async () => {
    let attempts = 0;
    updateDesktopServerClientHandlersForTests({
      ensureDesktopReady: async () => {
        attempts += 1;
      },
    });

    await Promise.all([
      waitForLocalServices(),
      waitForLocalServices(),
      waitForLocalServicesStartupGrace(),
    ]);

    expect(attempts).toBe(1);
  });
});
