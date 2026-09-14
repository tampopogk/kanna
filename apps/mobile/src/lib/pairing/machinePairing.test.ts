import { describe, expect, it, vi } from "vitest";
import {
  createNativeBonjourBrowser,
  createStaticBonjourBrowser,
  createUnavailableBonjourBrowser
} from "../discovery/bonjour";
import { fakeNativeBonjourModule } from "../discovery/fakeNativeBonjourModule";
import type { FetchLike, FetchResponseLike } from "../transports/lanTransport";
import { createMachinePairingService } from "./machinePairing";

const validPayload = JSON.stringify({
  type: "kanna.machine-pairing",
  version: 1,
  desktopId: "desktop-2",
  code: "ABC123"
});

function response(status: number, body: unknown): FetchResponseLike {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body
  };
}

function services() {
  return [
    {
      name: "one",
      type: "_kanna-mobile._tcp",
      host: "10.0.0.2",
      port: 48120,
      txt: { desktopId: "desktop-1" }
    },
    {
      name: "two",
      type: "_kanna-mobile._tcp",
      host: "10.0.0.3",
      port: 48120,
      txt: { desktopId: "desktop-2" }
    }
  ];
}

function pairingService(fetchImpl: FetchLike, claimTimeoutMs?: number) {
  return createMachinePairingService({
    bonjourBrowser: createStaticBonjourBrowser(services()),
    fetchImpl,
    getDeviceIdentity: () => ({
      deviceId: "phone-1",
      deviceName: "Kanna Mobile"
    }),
    claimTimeoutMs,
    now: () => new Date("2026-07-17T00:00:00.000Z")
  });
}

describe("machine pairing", () => {
  it("refreshes an explicit discovery candidate before claiming a code", async () => {
    const browser = createStaticBonjourBrowser([]);
    let refreshed = false;
    const refreshableBrowser = {
      ...browser,
      async refresh() {
        refreshed = true;
      },
      getServices: () => refreshed ? services() : []
    };
    const fetchImpl = vi.fn<FetchLike>(async (url) => response(
      url.includes("10.0.0.2") ? 200 : 400,
      url.includes("10.0.0.2")
        ? { desktopId: "desktop-1", desktopName: "Desk One" }
        : { error: "invalid code" }
    ));
    const service = createMachinePairingService({
      bonjourBrowser: refreshableBrowser,
      fetchImpl,
      getDeviceIdentity: () => ({ deviceId: "phone-1", deviceName: "Kanna Mobile" })
    });

    await expect(service.claimCode("ABC123")).resolves.toMatchObject({
      desktopId: "desktop-1"
    });
  });

  it("reports an unreachable explicit endpoint instead of no discovered machine", async () => {
    const browser = {
      ...createStaticBonjourBrowser([]),
      refresh: async () => { throw new Error("offline"); }
    };
    const service = createMachinePairingService({
      bonjourBrowser: browser,
      fetchImpl: vi.fn(),
      getDeviceIdentity: () => ({ deviceId: "phone-1", deviceName: "Kanna Mobile" })
    });

    await expect(service.claimCode("ABC123")).rejects.toMatchObject({
      reason: "unreachable"
    });
  });

  it("claims a QR payload only against its matching desktop", async () => {
    const fetchImpl = vi.fn<FetchLike>(async () => response(200, {
      desktopId: "desktop-2",
      desktopName: "Studio Mac"
    }));

    await expect(pairingService(fetchImpl).claimPayload(validPayload)).resolves.toEqual({
      desktopId: "desktop-2",
      displayName: "Studio Mac",
      lanEndpoints: [{
        baseUrl: "http://10.0.0.3:48120",
        lastSeenAt: "2026-07-17T00:00:00.000Z"
      }],
      lastSeenAt: "2026-07-17T00:00:00.000Z"
    });
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    expect(fetchImpl).toHaveBeenCalledWith(
      "http://10.0.0.3:48120/v1/pairing/sessions/claim",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({
          code: "ABC123",
          deviceId: "phone-1",
          deviceName: "Kanna Mobile"
        })
      })
    );
  });

  it("matches the uppercased compact QR identity case-insensitively", async () => {
    const fetchImpl = vi.fn<FetchLike>(async () => response(200, {
      desktopId: "DeSkToP-2",
      desktopName: "Studio Mac"
    }));

    await expect(
      pairingService(fetchImpl).claimPayload("KANNA1:DESKTOP-2:ABC123")
    ).resolves.toMatchObject({ desktopId: "DeSkToP-2" });
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    expect(fetchImpl).toHaveBeenCalledWith(
      "http://10.0.0.3:48120/v1/pairing/sessions/claim",
      expect.anything()
    );
  });

  it("stores the issued device secret from the claim response", async () => {
    const fetchImpl = vi.fn<FetchLike>(async () => response(200, {
      desktopId: "desktop-2",
      desktopName: "Studio Mac",
      deviceSecret: "issued-lan-secret"
    }));

    await expect(
      pairingService(fetchImpl).claimPayload(validPayload)
    ).resolves.toMatchObject({
      desktopId: "desktop-2",
      deviceSecret: "issued-lan-secret"
    });
  });

  it("stores the anonymous desktop identity and pairing certificate", async () => {
    const desktopPushIdentity = {
      publicKey: "desktop-ed25519-public-key",
      relayUrl: "wss://relay.example",
      environment: "development"
    };
    const pushPairingCert = {
      deviceId: "phone-1",
      issuedAt: 1_784_246_400_000,
      expiresAt: 1_847_318_400_000,
      signature: "desktop-signature"
    };
    const fetchImpl = vi.fn<FetchLike>(async () => response(200, {
      desktopId: "desktop-2",
      desktopName: "Studio Mac",
      desktopPushIdentity,
      pushPairingCert
    }));

    await expect(
      pairingService(fetchImpl).claimPayload(validPayload)
    ).resolves.toMatchObject({ desktopPushIdentity, pushPairingCert });
  });

  it("tolerates an older claim response without anonymous push fields", async () => {
    const fetchImpl = vi.fn<FetchLike>(async () => response(200, {
      desktopId: "desktop-2",
      desktopName: "Studio Mac",
      deviceSecret: "issued-lan-secret"
    }));

    const record = await pairingService(fetchImpl).claimPayload(validPayload);
    expect(record.deviceSecret).toBe("issued-lan-secret");
    expect("desktopPushIdentity" in record).toBe(false);
    expect("pushPairingCert" in record).toBe(false);
  });

  it("does not persist a certificate issued for another device", async () => {
    const fetchImpl = vi.fn<FetchLike>(async () => response(200, {
      desktopId: "desktop-2",
      desktopName: "Studio Mac",
      desktopPushIdentity: {
        publicKey: "desktop-ed25519-public-key",
        relayUrl: "wss://relay.example",
        environment: "development"
      },
      pushPairingCert: {
        deviceId: "another-phone",
        issuedAt: 1_784_246_400_000,
        expiresAt: 1_847_318_400_000,
        signature: "desktop-signature"
      }
    }));

    const record = await pairingService(fetchImpl).claimPayload(validPayload);
    expect("desktopPushIdentity" in record).toBe(false);
    expect("pushPairingCert" in record).toBe(false);
  });

  it("pairs against desktops that predate device secrets without storing one", async () => {
    const fetchImpl = vi.fn<FetchLike>(async () => response(200, {
      desktopId: "desktop-2",
      desktopName: "Studio Mac"
    }));

    const record = await pairingService(fetchImpl).claimPayload(validPayload);
    expect("deviceSecret" in record).toBe(false);
  });

  it("claims a manual code while signed out", async () => {
    const fetchImpl = vi.fn<FetchLike>(async (url) => {
      if (url.includes("10.0.0.2")) {
        return response(200, {
          desktopId: "desktop-1",
          desktopName: "Desk One"
        });
      }
      return response(400, { error: "invalid code" });
    });

    await expect(pairingService(fetchImpl).claimCode("abc-123")).resolves.toMatchObject({
      desktopId: "desktop-1",
      displayName: "Desk One"
    });
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it.each([
    [410, "expired"],
    [429, "rate-limited"]
  ])("maps HTTP %s to %s", async (status, reason) => {
    const fetchImpl = vi.fn<FetchLike>(async () => response(status, { error: reason }));

    await expect(pairingService(fetchImpl).claimCode("ABC123")).rejects.toMatchObject({ reason });
  });

  it("rejects malformed codes before discovery", async () => {
    const fetchImpl = vi.fn<FetchLike>();

    await expect(pairingService(fetchImpl).claimCode("bad")).rejects.toMatchObject({
      reason: "invalid-code"
    });
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it("rejects a successful claim whose identity does not match Bonjour", async () => {
    const fetchImpl = vi.fn<FetchLike>(async () => response(200, {
      desktopId: "desktop-imposter",
      desktopName: "Imposter"
    }));

    await expect(pairingService(fetchImpl).claimPayload(validPayload)).rejects.toMatchObject({
      reason: "identity-mismatch"
    });
  });

  it("reports multiple successful code claims instead of choosing a machine", async () => {
    const fetchImpl = vi.fn<FetchLike>(async (url) => response(200, {
      desktopId: url.includes("10.0.0.2") ? "desktop-1" : "desktop-2",
      desktopName: "Studio Mac"
    }));

    await expect(pairingService(fetchImpl).claimCode("ABC123")).rejects.toMatchObject({
      reason: "multiple-matches"
    });
  });

  it("does not let an unreachable candidate block a successful code claim", async () => {
    const fetchImpl = vi.fn<FetchLike>(async (url) => {
      if (url.includes("10.0.0.2")) {
        return response(200, {
          desktopId: "desktop-1",
          desktopName: "Desk One"
        });
      }
      return new Promise<FetchResponseLike>(() => undefined);
    });

    await expect(Promise.race([
      pairingService(fetchImpl, 10).claimCode("ABC123"),
      new Promise((_, reject) => setTimeout(
        () => reject(new Error("pairing did not honor its candidate timeout")),
        100
      ))
    ])).resolves.toMatchObject({ desktopId: "desktop-1" });
  });
});

// The Android defect this covers: the app scanned a valid QR, found no
// discovered machine, and told the owner to check the network. These run the
// real browser and the real pairing service against native discovery events.
describe("pairing against native discovery events", () => {
  const studio = {
    name: "Jeremy's Mac Studio",
    type: "_kanna-mobile._tcp.",
    host: "Jeremys-Mac-Studio.local",
    port: 48121,
    txt: { desktopId: "desktop-2" }
  };
  const other = {
    name: "Laptop",
    type: "_kanna-mobile._tcp.",
    host: "laptop.local",
    port: 48120,
    txt: { desktopId: "desktop-1" }
  };

  function nativePairingService(
    native: ReturnType<typeof fakeNativeBonjourModule>,
    fetchImpl: FetchLike
  ) {
    return createMachinePairingService({
      bonjourBrowser: createNativeBonjourBrowser(native.module, native.Emitter),
      fetchImpl,
      getDeviceIdentity: () => ({
        deviceId: "phone-1",
        deviceName: "Galaxy A15"
      }),
      now: () => new Date("2026-09-11T00:00:00.000Z")
    });
  }

  it("claims the scanned desktop once discovery resolves it", async () => {
    const native = fakeNativeBonjourModule();
    const fetchImpl = vi.fn<FetchLike>(async () => response(200, {
      desktopId: "desktop-2",
      desktopName: "Jeremy's Mac Studio",
      deviceSecret: "device-secret"
    }));
    const claimed = nativePairingService(native, fetchImpl).claimPayload(validPayload);

    // The scan beats discovery, as it does on a real phone: the claim is
    // already waiting when the resolved services arrive.
    setTimeout(() => {
      native.emit(other);
      native.emit(studio);
    }, 5);

    await expect(claimed).resolves.toEqual({
      desktopId: "desktop-2",
      displayName: "Jeremy's Mac Studio",
      deviceSecret: "device-secret",
      lanEndpoints: [{
        baseUrl: "http://Jeremys-Mac-Studio.local:48121",
        lastSeenAt: "2026-09-11T00:00:00.000Z"
      }],
      lastSeenAt: "2026-09-11T00:00:00.000Z"
    });
    // Only the desktop the QR named was claimed.
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    expect(fetchImpl).toHaveBeenCalledWith(
      "http://Jeremys-Mac-Studio.local:48121/v1/pairing/sessions/claim",
      expect.anything()
    );
  });

  it("tries every advertised desktop for a typed code", async () => {
    const native = fakeNativeBonjourModule();
    const fetchImpl = vi.fn<FetchLike>(async (url) => (
      url.includes("Jeremys-Mac-Studio")
        ? response(200, { desktopId: "desktop-2", desktopName: "Jeremy's Mac Studio" })
        : response(400, { error: "invalid code" })
    ));
    const claimed = nativePairingService(native, fetchImpl).claimCode("abc123");

    setTimeout(() => {
      native.emit(other);
      native.emit(studio);
    }, 5);

    await expect(claimed).resolves.toMatchObject({ desktopId: "desktop-2" });
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it("reports an unreachable machine when the device cannot discover at all", async () => {
    const service = createMachinePairingService({
      bonjourBrowser: createUnavailableBonjourBrowser(
        "This build cannot search the local network for Kanna desktops."
      ),
      fetchImpl: vi.fn(),
      getDeviceIdentity: () => ({ deviceId: "phone-1", deviceName: "Galaxy A15" })
    });

    await expect(service.claimPayload(validPayload)).rejects.toMatchObject({
      reason: "unreachable"
    });
  });

  it("never names the pairing code or the issued secret in a failure", async () => {
    const native = fakeNativeBonjourModule();
    const fetchImpl = vi.fn<FetchLike>(async () => response(410, {}));
    const service = createMachinePairingService({
      bonjourBrowser: createNativeBonjourBrowser(native.module, native.Emitter),
      fetchImpl,
      getDeviceIdentity: () => ({ deviceId: "phone-1", deviceName: "Galaxy A15" }),
      claimTimeoutMs: 50
    });
    const claimed = service.claimPayload(validPayload);
    native.emit(studio);

    const error = await claimed.catch((reason: Error) => reason);

    expect((error as Error).message).not.toContain("ABC123");
    expect((error as Error).message).not.toContain("device-secret");
  });
});
