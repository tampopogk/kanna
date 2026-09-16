// The secure channel against the real relay, server and daemon, driven by
// the real mobile relay client: pairing inside a sealed LAN session, sealed
// sessions and control over the relay tunnel with a known marker asserted
// absent from every tunnel frame and from the relay's own frame log, a
// hostile responder, the legacy switch refusing forged account-only invokes
// and plaintext tunnels, and revocation closing a live session.

import { createHash, webcrypto } from "node:crypto";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  StreamClient,
  type WebSocketLike as StreamWebSocketLike
} from "../../../packages/stream-client/src/index";
import {
  encodeKey,
  generateKeypair,
  type Keypair,
  type SealedWebSocketLike
} from "../../../packages/secure-channel/src/index";
import { sealSocket, type SecureChannelPeer } from "../../../apps/mobile/src/lib/security/secureChannelPeer";
import { parseMachinePairingPayload } from "../../../apps/mobile/src/lib/pairing/pairingPayload";
import type { SecureChannelRoute } from "../../../apps/mobile/src/lib/transports/relayClient";
import { localProcessFetch } from "@kanna/local-process-fetch";
import { BUFFY_UID } from "./firebaseAuth";
import { startRemoteHarness, type RemoteHarness } from "./harness";
import {
  createNodeRelayDesktopClient,
  NodeRelaySocket,
  tapRelaySocket,
  type RelayFrameTap
} from "./nodeRelayClient";
import {
  collectTerminalEvents,
  connectRawRelayClient,
  createScriptedTask,
  taskInputCount,
  waitForCondition,
  waitForTerminalOutput
} from "./terminalFlowTestUtils";

const randomBytes = (length: number) => webcrypto.getRandomValues(new Uint8Array(length));
const DEVICE_ID = "e2e-sealed-phone";

/**
 * A relay tunnel is only offered to a desktop signed into the account (an
 * anonymous desktop cannot open one), so the harness desktop is given a real
 * credential document and a matching `desktop_secret` first - the same
 * sequence `lan-desktop-routing.e2e.test.ts` proves.
 */
async function signInDesktopAsBuffy(harness: RemoteHarness): Promise<void> {
  const sha256Hex = (value: string) => createHash("sha256").update(value).digest("hex");
  const desktopSecret = sha256Hex(`${harness.desktopId}:secret`);
  const idToken = await harness.getIdToken();
  const response = await fetch(
    `http://127.0.0.1:${harness.ports.firestore}/v1/projects/kanna-local/databases/(default)/documents/desktopCredentials/${harness.desktopId.replace(/\//g, "_")}`,
    {
      method: "PATCH",
      headers: { Authorization: `Bearer ${idToken}`, "Content-Type": "application/json" },
      body: JSON.stringify({
        fields: {
          desktopId: { stringValue: harness.desktopId },
          displayName: { stringValue: "Secure Channel E2E Desktop" },
          desktopSecretHash: { stringValue: sha256Hex(desktopSecret) },
          revokedAt: { nullValue: null },
          uid: { stringValue: BUFFY_UID },
          updatedAt: { stringValue: new Date().toISOString() }
        }
      })
    }
  );
  if (!response.ok) {
    throw new Error(`failed to publish desktop credential: ${response.status} ${await response.text()}`);
  }
  await harness.restartServerWithIdentity({ desktopId: harness.desktopId, desktopSecret });
  await harness.waitForDesktop();
}

function marker(label: string): string {
  return `SEALED-${label}-${Array.from(randomBytes(6), (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

async function setLegacyAccess(harness: RemoteHarness, allowed: boolean): Promise<void> {
  const response = await localProcessFetch(`${harness.lanBaseUrl}/v1/settings/mobile_legacy_access`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ value: allowed ? "allowed" : "refused" })
  });
  expect(response.ok).toBe(true);
}

/** A sealed LAN StreamClient, the shape the phone's LAN transport uses. */
function sealedLanClient(harness: RemoteHarness, peer: SecureChannelPeer): StreamClient {
  return new StreamClient({
    url: `ws://127.0.0.1:${harness.ports.server}/v2/stream`,
    webSocketFactory: (url) =>
      sealSocket(new NodeRelaySocket(url) as unknown as SealedWebSocketLike, peer) as unknown as StreamWebSocketLike,
    reconnectDelaysMs: [250]
  });
}

describe("secure channel E2E", () => {
  let harness: RemoteHarness;
  let phone: Keypair;
  let desktopPublicKey: string;

  beforeAll(async () => {
    harness = await startRemoteHarness();
    await signInDesktopAsBuffy(harness);
    phone = generateKeypair(randomBytes);
  }, 240_000);

  afterAll(async () => {
    await harness?.stop();
  }, 30_000);

  const sealedPeer = (overrides: Partial<SecureChannelPeer> = {}): SecureChannelPeer => ({
    desktopId: harness.desktopId,
    desktopPublicKey,
    identity: phone,
    deviceId: DEVICE_ID,
    intent: "session",
    randomBytes,
    ...overrides
  });

  it("pairs from a KANNA2 QR inside a sealed LAN session; the code and QR secret never cross in the clear", async () => {
    const session = await harness.createDesktopPairingSession();
    const payload = parseMachinePairingPayload(session.pairingPayload);
    expect(payload.channelPublicKey).toBeDefined();
    expect(payload.qrSecret).toBeDefined();
    desktopPublicKey = payload.channelPublicKey as string;
    const status = await (await localProcessFetch(`${harness.lanBaseUrl}/v1/status`)).json() as { channelPublicKey?: string };
    expect(status.channelPublicKey).toBe(desktopPublicKey);

    const frames: string[] = [];
    const client = new StreamClient({
      url: `ws://127.0.0.1:${harness.ports.server}/v2/stream`,
      webSocketFactory: (url) => {
        const raw = new NodeRelaySocket(url);
        const originalSend = raw.send.bind(raw);
        raw.send = (data: string) => {
          frames.push(data);
          originalSend(data);
        };
        return sealSocket(raw as unknown as SealedWebSocketLike, sealedPeer({ intent: "pairing", deviceId: undefined })) as unknown as StreamWebSocketLike;
      },
      reconnectDelaysMs: [250]
    });
    try {
      const claim = await client.request("POST", "/v1/pairing/sessions/claim", {
        code: payload.code,
        deviceId: DEVICE_ID,
        deviceName: "E2E Sealed Phone",
        qrSecret: payload.qrSecret
      });
      expect(claim.status).toBe(200);
      expect(claim.body).toMatchObject({ desktopId: harness.desktopId, secureChannel: true });
    } finally {
      client.close();
    }
    expect(frames.length).toBeGreaterThan(0);
    for (const frame of frames) {
      expect(frame.startsWith("ksc1:")).toBe(true);
      expect(frame).not.toContain(payload.code);
      expect(frame).not.toContain(payload.qrSecret as string);
    }
  }, 60_000);

  it("carries sessions and control through the relay as ciphertext only, with LAN-paired authority", async () => {
    const tap: RelayFrameTap = { sent: [], received: [] };
    const client = createNodeRelayDesktopClient({
      relayUrl: harness.relayUrl,
      getIdToken: () => harness.getIdToken(),
      getSecureChannelRoute: (): SecureChannelRoute => ({ kind: "sealed", peer: sealedPeer() }),
      createSocket: (url) => tapRelaySocket(new NodeRelaySocket(url), tap)
    });
    try {
      // A sealed request frame answered with the paired route set.
      const status = await client.invokeDesktop({
        desktopId: harness.desktopId,
        method: "GET",
        path: "/v1/status",
        body: null
      }) as { desktopId?: string };
      expect(status.desktopId).toBe(harness.desktopId);
      // ...but not desktop-local authority.
      await expect(client.invokeDesktop({
        desktopId: harness.desktopId,
        method: "GET",
        path: "/v1/pairing/pending-confirmation",
        body: null
      })).rejects.toMatchObject({ status: 401 });

      const task = await createScriptedTask(harness, {
        displayName: "Sealed relay task",
        tracePartialInput: true
      });
      // A terminal stream over the same sealed tunnel, driven exactly the way
      // the shared collector drives the legacy relay path, with typed bytes
      // traced back through it by the scripted agent.
      const terminalMarker = marker("TERM");
      const events = collectTerminalEvents({ ...harness, client }, task.taskId);
      try {
        await waitForTerminalOutput(events, "SCRIPT_INPUT_READY", 20_000);
        events.sendInput(Buffer.from(terminalMarker).toString("base64"));
        await waitForTerminalOutput(events, `SCRIPT_PARTIAL:${terminalMarker}`, 20_000);
      } finally {
        events.close();
      }

      const inputMarker = marker("INPUT");
      const before = await taskInputCount(harness, task.taskId);
      await client.invokeDesktop({
        desktopId: harness.desktopId,
        method: "POST",
        path: `/v1/tasks/${encodeURIComponent(task.taskId)}/input`,
        body: { input: inputMarker }
      });
      await waitForCondition(
        async () => (await taskInputCount(harness, task.taskId)) === before + 1,
        20_000,
        "sealed task input was not recorded"
      );

      // Nothing readable crossed the relay in either direction: every frame
      // after the tunnel setup is a ksc1 frame, and the plaintext setup
      // frames carry no request, no marker and no bearer secret.
      const isSetup = (frame: string) => {
        try {
          const parsed = JSON.parse(frame) as { type?: string };
          return ["auth", "auth_ok", "tunnel_request", "tunnel_ready", "invoke", "response"].includes(parsed.type ?? "");
        } catch {
          return false;
        }
      };
      const allFrames = [...tap.sent, ...tap.received];
      expect(allFrames.length).toBeGreaterThan(2);
      for (const frame of allFrames) {
        expect(frame.startsWith("ksc1:") || isSetup(frame)).toBe(true);
        expect(frame).not.toContain(inputMarker);
        expect(frame).not.toContain(terminalMarker);
        expect(frame).not.toContain("/v1/tasks");
        expect(frame).not.toContain("term_input");
      }
      // The sealed path uses no relay `invoke` at all.
      expect(tap.sent.filter((frame) => frame.includes('"type":"invoke"'))).toHaveLength(0);
      const relayLogs = harness.relayLogs();
      expect(relayLogs).toContain("Tunnel client->desktop: <");
      expect(relayLogs).not.toContain(inputMarker);
      expect(relayLogs).not.toContain(terminalMarker);
      expect(relayLogs).not.toContain("Tunnel client->desktop: request");
      expect(relayLogs).not.toContain("Tunnel client->desktop: term_input");
    } finally {
      client.close();
    }
  }, 120_000);

  it("refuses a responder that does not hold the pinned key and sends nothing but the handshake", async () => {
    const impostor = generateKeypair(randomBytes);
    const refusals: string[] = [];
    const frames: string[] = [];
    const client = new StreamClient({
      url: `ws://127.0.0.1:${harness.ports.server}/v2/stream`,
      webSocketFactory: (url) => {
        const raw = new NodeRelaySocket(url);
        const originalSend = raw.send.bind(raw);
        raw.send = (data: string) => {
          frames.push(data);
          originalSend(data);
        };
        return sealSocket(
          raw as unknown as SealedWebSocketLike,
          sealedPeer({ desktopPublicKey: encodeKey(impostor.publicKey), onRefusal: (refusal) => refusals.push(refusal) })
        ) as unknown as StreamWebSocketLike;
      },
      reconnectDelaysMs: [250]
    });
    try {
      await expect(client.request("GET", "/v1/status")).rejects.toThrow();
    } finally {
      client.close();
    }
    expect(refusals).toEqual(["identity_mismatch"]);
    expect(frames).toHaveLength(1);
    expect(frames[0].startsWith("ksc1:")).toBe(true);
  }, 60_000);

  it("refuses forged account-only relay invokes and plaintext tunnels once legacy access is off, while sealed sessions keep working", async () => {
    const task = await createScriptedTask(harness, { displayName: "Legacy gate task" });
    await setLegacyAccess(harness, false);
    try {
      const raw = await connectRawRelayClient(harness);
      try {
        const before = await taskInputCount(harness, task.taskId);
        raw.send({
          type: "invoke",
          id: "forged-1",
          desktopId: harness.desktopId,
          method: "POST",
          path: `/v1/tasks/${encodeURIComponent(task.taskId)}/input`,
          body: { input: marker("FORGED") }
        });
        const response = await raw.waitFor((message) => message.type === "response" && message.id === "forged-1", 20_000);
        expect(response.status).toBe(401);
        expect(await taskInputCount(harness, task.taskId)).toBe(before);
      } finally {
        raw.close();
      }

      // A legacy (plaintext) relay client cannot open a tunnel session.
      const legacy = createNodeRelayDesktopClient({ relayUrl: harness.relayUrl, getIdToken: () => harness.getIdToken() });
      try {
        const errors: string[] = [];
        const subscription = legacy.observeTaskTerminal({ desktopId: harness.desktopId, taskId: task.taskId }, (event) => {
          if (event.type === "error") errors.push(event.code ?? event.message);
        });
        try {
          await waitForCondition(
            async () => errors.includes("legacy_access_refused"),
            20_000,
            "plaintext relay tunnel was not refused"
          );
        } finally {
          subscription.close();
        }
      } finally {
        legacy.close();
      }

      // The sealed session is unaffected.
      const sealed = createNodeRelayDesktopClient({
        relayUrl: harness.relayUrl,
        getIdToken: () => harness.getIdToken(),
        getSecureChannelRoute: (): SecureChannelRoute => ({ kind: "sealed", peer: sealedPeer() })
      });
      try {
        const status = await sealed.invokeDesktop({ desktopId: harness.desktopId, method: "GET", path: "/v1/status", body: null }) as { desktopId?: string };
        expect(status.desktopId).toBe(harness.desktopId);
      } finally {
        sealed.close();
      }
    } finally {
      await setLegacyAccess(harness, true);
    }
  }, 120_000);

  it("closes a live sealed session on revocation and treats the next handshake as unpaired", async () => {
    const client = sealedLanClient(harness, sealedPeer());
    try {
      const status = await client.request("GET", "/v1/status");
      expect(status.status).toBe(200);
      const removed = await localProcessFetch(`${harness.lanBaseUrl}/v1/pairing/trusted-devices/${encodeURIComponent(DEVICE_ID)}`, {
        method: "DELETE"
      });
      expect(removed.status).toBe(204);
      await expect(client.request("GET", "/v1/status")).rejects.toThrow();
    } finally {
      client.close();
    }
    const again = sealedLanClient(harness, sealedPeer({ intent: "pairing", deviceId: undefined }));
    try {
      const status = await again.request("GET", "/v1/status");
      expect(status.status).toBe(401);
    } finally {
      again.close();
    }
  }, 60_000);
});
