import { describe, expect, it, vi } from "vitest";
import type { E2eConnectionDiagnostics } from "../../src/e2eConnectionDiagnostics";
import {
  createDesktopPairingSession,
  describesExactDesktopPairing,
  pairExactDesktopThroughDeepLink,
  readMobileConnectionDiagnostics,
  resolveDesktopServerExpoEnv,
  resolvePairingSessionUrl,
  waitForExactDesktopPairing,
  withConnectionDiagnostics,
  type DiagnosticsDriver
} from "./desktop-pairing";

function diagnostics(
  overrides: Partial<E2eConnectionDiagnostics> = {}
): E2eConnectionDiagnostics {
  return {
    connectionMode: "lan",
    connectionState: "connected",
    refreshStatus: "updated",
    taskCollectionStatus: "ready",
    serverStatus: "running",
    errorMessage: null,
    desktopId: "desktop-e2e",
    selectedDesktopId: "desktop-e2e",
    selectedRepoId: null,
    authStatus: "signedOut",
    mobileDeviceIdPresent: true,
    trustedDesktops: [
      {
        desktopId: "desktop-e2e",
        lanEndpoints: ["http://192.168.1.10:48121"],
        deviceSecretPresent: true,
        pushPairingCertPresent: true
      }
    ],
    liveLanDesktopIds: ["desktop-e2e"],
    accountDesktopIds: [],
    taskCounts: { repo: 0, recent: 3 },
    ...overrides
  };
}

function markerDriver(labels: Array<string | null>): DiagnosticsDriver & { polls: number } {
  const driver = {
    polls: 0,
    $: async () => ({
      isExisting: async () => labels[Math.min(driver.polls, labels.length - 1)] !== null,
      getAttribute: async () => labels[Math.min(driver.polls, labels.length - 1)]
    }),
    waitUntil: async (
      condition: () => Promise<boolean>,
      options: { timeoutMsg: string }
    ) => {
      while (driver.polls < labels.length) {
        const ready = await condition();
        driver.polls += 1;
        if (ready) return true;
      }
      throw new Error(options.timeoutMsg);
    }
  };
  return driver;
}

describe("exact desktop pairing for the ordinary smoke", () => {
  it("creates the pairing session on loopback, seeds the app route, then claims the payload", async () => {
    const order: string[] = [];
    const execute = vi.fn(async (_command: string, input: { url: string }) => {
      order.push(input.url.startsWith("kanna://e2e-trust") ? `trust:${input.url}` : `claim:${input.url}`);
    });
    const readIdentity = vi.fn(async (baseUrl: string) => {
      order.push(`identity:${baseUrl}`);
      return { desktopId: "desktop-e2e", desktopName: "E2E Mac" };
    });
    const createPairingSession = vi.fn(async (baseUrl: string) => {
      order.push(`session:${baseUrl}`);
      return { desktopId: "desktop-e2e", pairingPayload: "KANNA1:DESKTOP-E2E:ABC123" };
    });

    const identity = await pairExactDesktopThroughDeepLink({
      bundleId: "build.kanna.app",
      driver: { execute } as never,
      configuredDesktopServerUrl: "http://192.168.1.10:48121",
      appDesktopServerUrl: "http://192.168.1.10:48121",
      localAddresses: ["192.168.1.10"],
      readIdentity,
      createPairingSession
    });

    expect(identity).toEqual({ desktopId: "desktop-e2e", desktopName: "E2E Mac" });
    expect(order).toEqual([
      "identity:http://127.0.0.1:48121",
      "trust:kanna://e2e-trust?desktopId=desktop-e2e&displayName=E2E%20Mac" +
        "&lanBaseUrl=http%3A%2F%2F192.168.1.10%3A48121",
      "session:http://127.0.0.1:48121",
      "claim:kanna://e2e-pair?payload=KANNA1%3ADESKTOP-E2E%3AABC123"
    ]);
  });

  it("refuses a pairing session whose desktop is not the selected test server", async () => {
    const execute = vi.fn(async () => undefined);

    await expect(pairExactDesktopThroughDeepLink({
      bundleId: "build.kanna.app",
      driver: { execute } as never,
      configuredDesktopServerUrl: "http://127.0.0.1:48121",
      appDesktopServerUrl: "http://127.0.0.1:48121",
      readIdentity: async () => ({ desktopId: "desktop-selected", desktopName: "Selected" }),
      createPairingSession: async () => ({
        desktopId: "desktop-other",
        pairingPayload: "KANNA1:DESKTOP-OTHER:ABC123"
      })
    })).rejects.toThrow("not the selected test server desktop-selected");
    // The trust seed went out; the claim never did.
    expect(execute).toHaveBeenCalledTimes(1);
  });

  it("only creates pairing sessions on this machine", () => {
    expect(resolvePairingSessionUrl("http://127.0.0.1:48121", [])).toBe("http://127.0.0.1:48121");
    expect(resolvePairingSessionUrl("http://localhost:48120/", [])).toBe("http://localhost:48120");
    expect(resolvePairingSessionUrl("http://192.168.1.10:48121", ["127.0.0.1", "192.168.1.10"]))
      .toBe("http://127.0.0.1:48121");
    expect(() => resolvePairingSessionUrl("http://192.168.1.99:48121", ["192.168.1.10"]))
      .toThrow("Pairing sessions can only be created on the desktop's own machine");
  });

  it("hands the app the exact desktop route through the Metro environment", () => {
    expect(resolveDesktopServerExpoEnv({
      appEnv: "prod",
      appDesktopServerUrl: "http://192.168.1.10:48121"
    })).toEqual({
      KANNA_APP_ENV: "prod",
      EXPO_PUBLIC_KANNA_SERVER_URL: "http://192.168.1.10:48121"
    });
  });

  it("fails a pairing session that the server refused instead of inventing one", async () => {
    const fetchImpl = vi.fn(async () => ({
      ok: false,
      status: 403,
      json: async () => ({ error: "pairing sessions can only be started from the desktop app" })
    }));
    await expect(createDesktopPairingSession("http://127.0.0.1:48121", fetchImpl as never))
      .rejects.toThrow("Failed to create a mobile E2E pairing session on http://127.0.0.1:48121: HTTP 403");
  });

  it("recognizes a genuine pairing from the sanitized diagnostics and rejects identity-only trust", () => {
    const expected = { desktopId: "desktop-e2e", appDesktopServerUrl: "http://192.168.1.10:48121/" };
    expect(describesExactDesktopPairing(diagnostics(), expected)).toBe(true);
    expect(describesExactDesktopPairing(null, expected)).toBe(false);
    expect(describesExactDesktopPairing(diagnostics({ mobileDeviceIdPresent: false }), expected)).toBe(false);
    expect(describesExactDesktopPairing(diagnostics({
      trustedDesktops: [
        { desktopId: "desktop-e2e", lanEndpoints: [], deviceSecretPresent: false, pushPairingCertPresent: false }
      ]
    }), expected)).toBe(false);
    expect(describesExactDesktopPairing(diagnostics({
      trustedDesktops: [
        { desktopId: "desktop-e2e", lanEndpoints: ["http://10.0.0.5:48121"], deviceSecretPresent: true, pushPairingCertPresent: true }
      ]
    }), expected)).toBe(false);
  });

  it("waits for the persisted credential and reports the last picture when it never lands", async () => {
    const paired = JSON.stringify(diagnostics());
    const unpaired = JSON.stringify(diagnostics({
      mobileDeviceIdPresent: false,
      trustedDesktops: [
        { desktopId: "desktop-e2e", lanEndpoints: [], deviceSecretPresent: false, pushPairingCertPresent: false }
      ]
    }));
    const expected = { desktopId: "desktop-e2e", appDesktopServerUrl: "http://192.168.1.10:48121" };

    const eventual = markerDriver([null, unpaired, paired]);
    await expect(waitForExactDesktopPairing(eventual, expected)).resolves.toMatchObject({
      trustedDesktops: [expect.objectContaining({ deviceSecretPresent: true })]
    });
    expect(eventual.polls).toBe(3);

    const never = markerDriver([unpaired, unpaired]);
    await expect(waitForExactDesktopPairing(never, expected)).rejects.toThrow(
      /paired device secret and endpoint for desktop desktop-e2e at http:\/\/192\.168\.1\.10:48121; last connection diagnostics: .*"deviceSecretPresent":false/
    );
  });

  it("retains sanitized diagnostics beside a failed assertion without changing its deadline", async () => {
    const driver = markerDriver([JSON.stringify(diagnostics({ taskCounts: { repo: 0, recent: 0 }, taskCollectionStatus: "loading" }))]);
    const failure = new Error("Expected at least one task row in the mobile task list");

    const error = await withConnectionDiagnostics(driver, "task rows", async () => {
      throw failure;
    }).catch((caught: unknown) => caught as Error & { cause?: unknown });

    expect(error.message).toContain("Expected at least one task row in the mobile task list");
    expect(error.message).toContain('[mobile connection diagnostics after task rows] {"connectionMode":"lan"');
    expect(error.message).toContain('"taskCollectionStatus":"loading"');
    expect(error.cause).toBe(failure);
    await expect(withConnectionDiagnostics(driver, "ok", async () => "value")).resolves.toBe("value");
  });

  it("reads a missing or malformed marker as no diagnostics", async () => {
    await expect(readMobileConnectionDiagnostics(markerDriver([null]))).resolves.toBeNull();
    await expect(readMobileConnectionDiagnostics(markerDriver(["{not json"]))).resolves.toBeNull();
  });
});
