import { describe, expect, it, vi } from "vitest";
import { encodeKey, generateKeypair, sasCode } from "@kanna/secure-channel";
import { createStaticBonjourBrowser } from "../discovery/bonjour";
import type { FetchLike, FetchResponseLike } from "../transports/lanTransport";
import { createMachinePairingService, MachinePairingError } from "./machinePairing";
import { createFakeSecureDesktop, testRandomBytes } from "../../test/fakeSecureDesktop";

function response(status: number, body: unknown): FetchResponseLike {
  return { ok: status >= 200 && status < 300, status, json: async () => body };
}

const BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
function base32(bytes: Uint8Array): string {
  let out = "";
  let buffer = 0;
  let bits = 0;
  for (const byte of bytes) {
    buffer = (buffer << 8) | byte;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      out += BASE32[(buffer >> bits) & 31];
    }
  }
  if (bits > 0) out += BASE32[(buffer << (5 - bits)) & 31];
  return out;
}

const QR_SECRET_BYTES = Uint8Array.from({ length: 16 }, (_, index) => index * 7 + 1);
const QR_SECRET = base32(QR_SECRET_BYTES);

function service(host: string, desktopId: string) {
  return { name: host, type: "_kanna-mobile._tcp", host, port: 48120, txt: { desktopId } };
}

const claimBody = (desktopId: string) => ({
  desktopId,
  desktopName: "Sealed Mac",
  deviceSecret: "legacy-compat-secret",
  secureChannel: true,
});

describe("machine pairing over the secure channel", () => {
  it("claims a KANNA2 QR through a sealed session anchored to the scanned key, never in plaintext", async () => {
    const claims: unknown[] = [];
    const desktop = createFakeSecureDesktop({
      desktopId: "DESKTOP-1",
      route: (request) => {
        claims.push(request);
        return request.path === "/v1/pairing/sessions/claim" ? { status: 200, body: claimBody("DESKTOP-1") } : { status: 404 };
      },
    });
    const fetchImpl = vi.fn<FetchLike>(async (url) =>
      url.endsWith("/v1/status")
        ? response(200, { desktopId: "DESKTOP-1", channelPublicKey: desktop.publicKey, kspStreamVersion: 2 })
        : response(500, { error: "plaintext claim must not be used" }),
    );
    const identity = generateKeypair(testRandomBytes);
    const openedUrls: string[] = [];
    const pairing = createMachinePairingService({
      bonjourBrowser: createStaticBonjourBrowser([service("10.0.0.5", "DESKTOP-1")]),
      fetchImpl,
      getDeviceIdentity: () => ({ deviceId: "phone-1", deviceName: "Kanna Mobile" }),
      secureChannel: {
        getIdentity: () => identity,
        randomBytes: testRandomBytes,
        createLanSocket: (url) => {
          openedUrls.push(url);
          return desktop.createSocket();
        },
      },
      now: () => new Date("2026-09-16T00:00:00.000Z"),
    });
    const payload = `KANNA2:DESKTOP-1:ABC123:${base32(desktop.identity.publicKey)}:${QR_SECRET}`;

    const record = await pairing.claimPayload(payload);

    expect(record).toEqual({
      desktopId: "DESKTOP-1",
      displayName: "Sealed Mac",
      lanEndpoints: [{ baseUrl: "http://10.0.0.5:48120", lastSeenAt: "2026-09-16T00:00:00.000Z" }],
      lastSeenAt: "2026-09-16T00:00:00.000Z",
      deviceSecret: "legacy-compat-secret",
      channelPublicKey: desktop.publicKey,
    });
    expect(openedUrls).toEqual(["ws://10.0.0.5:48120/v2/stream"]);
    // The only plaintext HTTP call is the status probe.
    expect(fetchImpl.mock.calls.map(([url]) => url)).toEqual(["http://10.0.0.5:48120/v1/status"]);
    // The desktop registered the handshake key, and saw the QR secret only
    // inside the sealed request.
    expect(desktop.lastChannel?.remoteStatic).toEqual(identity.publicKey);
    expect(claims).toEqual([
      {
        method: "POST",
        path: "/v1/pairing/sessions/claim",
        body: { code: "ABC123", deviceId: "phone-1", deviceName: "Kanna Mobile", qrSecret: QR_SECRET },
      },
    ]);
    for (const frame of desktop.wireFrames) {
      expect(frame.startsWith("ksc1:")).toBe(true);
      expect(frame).not.toContain("ABC123");
      expect(frame).not.toContain(QR_SECRET);
    }
  });

  it("refuses when the QR key and the advertised key disagree, without opening a socket", async () => {
    const desktop = createFakeSecureDesktop({ desktopId: "DESKTOP-1", route: () => ({ status: 200 }) });
    const impostor = generateKeypair(testRandomBytes);
    const fetchImpl = vi.fn<FetchLike>(async () =>
      response(200, { desktopId: "DESKTOP-1", channelPublicKey: encodeKey(impostor.publicKey) }),
    );
    const createLanSocket = vi.fn(() => desktop.createSocket());
    const pairing = createMachinePairingService({
      bonjourBrowser: createStaticBonjourBrowser([service("10.0.0.5", "DESKTOP-1")]),
      fetchImpl,
      getDeviceIdentity: () => ({ deviceId: "phone-1", deviceName: "Kanna Mobile" }),
      secureChannel: { getIdentity: () => generateKeypair(testRandomBytes), randomBytes: testRandomBytes, createLanSocket },
    });
    const payload = `KANNA2:DESKTOP-1:ABC123:${base32(desktop.identity.publicKey)}:${QR_SECRET}`;
    await expect(pairing.claimPayload(payload)).rejects.toMatchObject({ reason: "identity-mismatch" });
    expect(createLanSocket).not.toHaveBeenCalled();
  });

  it("waits for the desktop-side SAS confirmation on a typed code and shows the matching SAS", async () => {
    let polls = 0;
    const desktop = createFakeSecureDesktop({
      desktopId: "DESKTOP-1",
      route: (request) => {
        if (request.path === "/v1/pairing/sessions/claim") {
          expect(request.body).toEqual({ code: "ABC123", deviceId: "phone-1", deviceName: "Kanna Mobile" });
          return { status: 202, body: { status: "confirmation_required" } };
        }
        if (request.path === "/v1/pairing/confirmation") {
          polls += 1;
          return polls < 3 ? { status: 202, body: { status: "pending" } } : { status: 200, body: claimBody("DESKTOP-1") };
        }
        return { status: 404 };
      },
    });
    const fetchImpl = vi.fn<FetchLike>(async (url) =>
      url.endsWith("/v1/status")
        ? response(200, { desktopId: "DESKTOP-1", channelPublicKey: desktop.publicKey })
        : response(500, {}),
    );
    const shown: string[] = [];
    const pairing = createMachinePairingService({
      bonjourBrowser: createStaticBonjourBrowser([service("10.0.0.5", "DESKTOP-1")]),
      fetchImpl,
      getDeviceIdentity: () => ({ deviceId: "phone-1", deviceName: "Kanna Mobile" }),
      secureChannel: {
        getIdentity: () => generateKeypair(testRandomBytes),
        randomBytes: testRandomBytes,
        createLanSocket: () => desktop.createSocket(),
      },
    });

    const record = await pairing.claimCode("abc123", { onConfirmationRequired: (sas) => shown.push(sas) });

    expect(record.channelPublicKey).toBe(desktop.publicKey);
    expect(shown).toHaveLength(1);
    expect(shown[0]).toMatch(/^\d{6}$/);
    // Both screens derive the SAS from the same transcript.
    expect(shown[0]).toBe(sasCode(desktop.lastChannel!.handshakeHash));
    expect(polls).toBe(3);
    // One handshake for the whole ceremony: the SAS shown is the SAS confirmed.
    expect(desktop.handshakes).toBe(1);
  });

  it("refuses a typed-code claim the desktop accepts without SAS confirmation", async () => {
    // A LAN impostor that answers status with its own key and 200s the
    // claim would otherwise be pinned as the desktop.
    const desktop = createFakeSecureDesktop({
      desktopId: "DESKTOP-1",
      route: (request) =>
        request.path === "/v1/pairing/sessions/claim"
          ? { status: 200, body: claimBody("DESKTOP-1") }
          : { status: 404 },
    });
    const fetchImpl = vi.fn<FetchLike>(async () =>
      response(200, { desktopId: "DESKTOP-1", channelPublicKey: desktop.publicKey }),
    );
    const shown: string[] = [];
    const pairing = createMachinePairingService({
      bonjourBrowser: createStaticBonjourBrowser([service("10.0.0.5", "DESKTOP-1")]),
      fetchImpl,
      getDeviceIdentity: () => ({ deviceId: "phone-1", deviceName: "Kanna Mobile" }),
      secureChannel: {
        getIdentity: () => generateKeypair(testRandomBytes),
        randomBytes: testRandomBytes,
        createLanSocket: () => desktop.createSocket(),
      },
    });
    const outcome = await pairing
      .claimCode("abc123", { onConfirmationRequired: (sas) => shown.push(sas) })
      .then(
        (record) => ({ record }),
        (error: unknown) => ({ error }),
      );
    expect("record" in outcome).toBe(false);
    expect((outcome as { error: MachinePairingError }).error).toBeInstanceOf(MachinePairingError);
    expect((outcome as { error: MachinePairingError }).error.reason).toBe("not-verified");
    expect(shown).toEqual([]);
  });

  it("reports a rejection on the desktop and persists nothing", async () => {
    const desktop = createFakeSecureDesktop({
      desktopId: "DESKTOP-1",
      route: (request) =>
        request.path === "/v1/pairing/sessions/claim"
          ? { status: 202, body: { status: "confirmation_required" } }
          : { status: 403, body: { error: "rejected" } },
    });
    const fetchImpl = vi.fn<FetchLike>(async () =>
      response(200, { desktopId: "DESKTOP-1", channelPublicKey: desktop.publicKey }),
    );
    const pairing = createMachinePairingService({
      bonjourBrowser: createStaticBonjourBrowser([service("10.0.0.5", "DESKTOP-1")]),
      fetchImpl,
      getDeviceIdentity: () => ({ deviceId: "phone-1", deviceName: "Kanna Mobile" }),
      secureChannel: {
        getIdentity: () => generateKeypair(testRandomBytes),
        randomBytes: testRandomBytes,
        createLanSocket: () => desktop.createSocket(),
      },
    });
    await expect(pairing.claimCode("abc123")).rejects.toMatchObject({ reason: "confirmation-rejected" });
  });

  it("treats a desktop that answers the handshake in plaintext as unverified", async () => {
    const desktop = createFakeSecureDesktop({
      desktopId: "DESKTOP-1",
      route: () => ({ status: 200 }),
      plaintextReply: JSON.stringify({ type: "error", code: "bad_frame", message: "unparseable frame" }),
    });
    const fetchImpl = vi.fn<FetchLike>(async () =>
      response(200, { desktopId: "DESKTOP-1", channelPublicKey: desktop.publicKey }),
    );
    const pairing = createMachinePairingService({
      bonjourBrowser: createStaticBonjourBrowser([service("10.0.0.5", "DESKTOP-1")]),
      fetchImpl,
      getDeviceIdentity: () => ({ deviceId: "phone-1", deviceName: "Kanna Mobile" }),
      secureChannel: {
        getIdentity: () => generateKeypair(testRandomBytes),
        randomBytes: testRandomBytes,
        createLanSocket: () => desktop.createSocket(),
      },
    });
    const error = await pairing.claimCode("abc123").catch((failure: unknown) => failure);
    expect(error).toBeInstanceOf(MachinePairingError);
    expect((error as MachinePairingError).reason).toBe("not-verified");
    // Nothing but the handshake ever left the phone.
    expect(desktop.wireFrames).toHaveLength(1);
    expect(desktop.wireFrames[0].startsWith("ksc1:")).toBe(true);
  });

  it("pairs a scanned KANNA2 QR through the relay tunnel when the desktop is not on the LAN", async () => {
    const desktop = createFakeSecureDesktop({
      desktopId: "DESKTOP-1",
      route: (request) =>
        request.path === "/v1/pairing/sessions/claim" ? { status: 200, body: claimBody("DESKTOP-1") } : { status: 404 },
    });
    const fetchImpl = vi.fn<FetchLike>();
    const tunnels: string[] = [];
    const pairing = createMachinePairingService({
      bonjourBrowser: createStaticBonjourBrowser([]),
      fetchImpl,
      getDeviceIdentity: () => ({ deviceId: "phone-1", deviceName: "Kanna Mobile" }),
      secureChannel: {
        getIdentity: () => generateKeypair(testRandomBytes),
        randomBytes: testRandomBytes,
        createLanSocket: () => {
          throw new Error("no LAN socket expected");
        },
        createRelayTunnelSocket: (desktopId) => {
          tunnels.push(desktopId);
          return desktop.createSocket();
        },
      },
    });
    const payload = `KANNA2:DESKTOP-1:ABC123:${base32(desktop.identity.publicKey)}:${QR_SECRET}`;
    const record = await pairing.claimPayload(payload);
    expect(record.channelPublicKey).toBe(desktop.publicKey);
    expect(record.lanEndpoints).toEqual([]);
    expect(tunnels).toEqual(["DESKTOP-1"]);
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it("refuses a keyed desktop when this phone has no secure identity", async () => {
    const desktop = createFakeSecureDesktop({ desktopId: "DESKTOP-1", route: () => ({ status: 200 }) });
    const fetchImpl = vi.fn<FetchLike>(async () =>
      response(200, { desktopId: "DESKTOP-1", channelPublicKey: desktop.publicKey }),
    );
    const pairing = createMachinePairingService({
      bonjourBrowser: createStaticBonjourBrowser([service("10.0.0.5", "DESKTOP-1")]),
      fetchImpl,
      getDeviceIdentity: () => ({ deviceId: "phone-1", deviceName: "Kanna Mobile" }),
      secureChannel: { getIdentity: () => null, randomBytes: testRandomBytes, createLanSocket: () => desktop.createSocket() },
    });
    await expect(pairing.claimCode("abc123")).rejects.toMatchObject({ reason: "secure-identity-unavailable" });
    expect(desktop.wireFrames).toHaveLength(0);
  });
});
