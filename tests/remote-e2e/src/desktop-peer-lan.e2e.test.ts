// The LAN route of the sealed desktop-to-desktop channel: two real servers
// bound to every interface, real Bonjour advertising the general API port
// (`lanPort`) beside the legacy TLS invoke port, and a paired sibling dialled
// at `ws://<candidate>/v1/peers/channel` - reported as route `peer-lan` -
// with the relay left as the fallback it is.

import { createHash } from "node:crypto";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { localProcessFetch } from "@kanna/local-process-fetch";
import { BUFFY_UID } from "./firebaseAuth";
import { startRemoteHarness, type RemoteDesktop, type RemoteHarness } from "./harness";
import { waitForCondition } from "./terminalFlowTestUtils";

interface MachineInvokeResult {
  status: number;
  body: unknown;
  error: string | null;
  route: string;
}

type Desktop = Pick<RemoteHarness, "lanBaseUrl" | "desktopId">;

const sha256Hex = (value: string) => createHash("sha256").update(value).digest("hex");
const desktopSecretFor = (desktopId: string) => sha256Hex(`${desktopId}:secret`);

async function publishDesktopCredentialAsBuffy(
  harness: RemoteHarness,
  input: { desktopId: string; desktopSecret: string; displayName: string }
): Promise<void> {
  const idToken = await harness.getIdToken();
  const response = await fetch(
    `http://127.0.0.1:${harness.ports.firestore}/v1/projects/kanna-local/databases/(default)/documents/desktopCredentials/${input.desktopId.replace(/\//g, "_")}`,
    {
      method: "PATCH",
      headers: { Authorization: `Bearer ${idToken}`, "Content-Type": "application/json" },
      body: JSON.stringify({
        fields: {
          desktopId: { stringValue: input.desktopId },
          displayName: { stringValue: input.displayName },
          desktopSecretHash: { stringValue: sha256Hex(input.desktopSecret) },
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
}

async function json<T>(response: Response): Promise<T> {
  const text = await response.text();
  if (!response.ok) throw new Error(`HTTP ${response.status}: ${text}`);
  return JSON.parse(text) as T;
}

async function invokeMachine(from: Desktop, targetDesktopId: string, path: string): Promise<MachineInvokeResult> {
  return json<MachineInvokeResult>(await localProcessFetch(
    `${from.lanBaseUrl}/v1/cloud/desktops/${encodeURIComponent(targetDesktopId)}/invoke`,
    { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ method: "GET", path, body: null }) }
  ));
}

describe("desktop peer secure channel over the LAN E2E", () => {
  let harness: RemoteHarness;
  let peer: RemoteDesktop;

  beforeAll(async () => {
    // Every interface, as a real desktop binds (see lan-desktop-routing),
    // so the address Bonjour advertises is reachable.
    harness = await startRemoteHarness({ lanHost: "0.0.0.0" });
    const desktopSecret = desktopSecretFor(harness.desktopId);
    await publishDesktopCredentialAsBuffy(harness, { desktopId: harness.desktopId, desktopSecret, displayName: "Peer LAN E2E Desktop" });
    await harness.restartServerWithIdentity({ desktopId: harness.desktopId, desktopSecret });
    await harness.waitForDesktop();
    const peerId = `desktop-peer-lan-${Date.now()}`;
    const peerSecret = desktopSecretFor(peerId);
    await publishDesktopCredentialAsBuffy(harness, { desktopId: peerId, desktopSecret: peerSecret, displayName: "Peer LAN E2E Peer" });
    peer = await harness.startAdditionalDesktop({ desktopId: peerId, desktopSecret: peerSecret });
  }, 240_000);

  afterAll(async () => {
    await peer?.stop();
    await harness?.stop();
  }, 30_000);

  it("reaches a paired sibling over the LAN once discovery has its API port, and never over a plaintext route", async () => {
    const offer = await json<{ pairingString: string }>(
      await localProcessFetch(`${peer.lanBaseUrl}/v1/peers/pairing-offers`, { method: "POST" })
    );
    const paired = await json<{ route: string; encryption: string }>(
      await localProcessFetch(`${harness.lanBaseUrl}/v1/peers/pair`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ pairingString: offer.pairingString })
      })
    );
    expect(paired.encryption).toBe("e2ee");
    expect(paired.route.startsWith("peer-")).toBe(true);

    // Real Bonjour resolution can take a while; until it lands the sealed
    // session rides the relay, and it must never ride anything else.
    await waitForCondition(
      async () => {
        const response = await invokeMachine(harness, peer.desktopId, "/v1/status");
        expect(response.status).toBe(200);
        expect(response.route.startsWith("peer-"), `route was ${response.route}`).toBe(true);
        return response.route === "peer-lan";
      },
      90_000,
      `${harness.desktopId} never reached ${peer.desktopId} over the LAN peer channel`
    );
    await waitForCondition(
      async () => {
        const response = await invokeMachine(peer, harness.desktopId, "/v1/status");
        expect(response.status).toBe(200);
        expect(response.route.startsWith("peer-"), `route was ${response.route}`).toBe(true);
        return response.route === "peer-lan";
      },
      90_000,
      `${peer.desktopId} never reached ${harness.desktopId} over the LAN peer channel`
    );
    const peers = await json<{ peers: Array<{ desktopId: string; reachable: { lan: boolean } }> }>(
      await localProcessFetch(`${harness.lanBaseUrl}/v1/peers`)
    );
    expect(peers.peers.find((entry) => entry.desktopId === peer.desktopId)?.reachable.lan).toBe(true);
  }, 240_000);
});
