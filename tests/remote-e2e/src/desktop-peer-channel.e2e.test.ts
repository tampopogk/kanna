// Sealed desktop-to-desktop sessions against the real relay, two real
// servers and daemons: pairing by string, sealed invokes in both directions
// through a desktop-secret relay tunnel, a sibling terminal view spliced
// through the local peer proxy with known markers asserted absent from the
// relay's frame log and both servers' logs, a rotated peer key refused,
// unpairing closing the route, reconnection after a relay restart, and the
// legacy switch refusing a relay-attested invoke from an unpaired sibling.
//
// The harness binds every server to loopback while Bonjour advertises the
// routable interface, so a paired sibling's LAN candidate is unreachable
// and every sealed session here takes the relay route - which is the route
// the relay must be unable to read.

import { createHash, webcrypto } from "node:crypto";
import { readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { localProcessFetch } from "@kanna/local-process-fetch";
import { StreamClient, type WebSocketLike as StreamWebSocketLike } from "../../../packages/stream-client/src/index";
import { BUFFY_UID } from "./firebaseAuth";
import { startRemoteHarness, type RemoteDesktop, type RemoteHarness } from "./harness";
import { NodeRelaySocket } from "./nodeRelayClient";
import { createScriptedTask, waitForCondition } from "./terminalFlowTestUtils";

interface MachineInvokeResult {
  status: number;
  body: unknown;
  error: string | null;
  route: string;
}

interface Peer {
  desktopId: string;
  displayName: string;
  encryption: string;
  transferIdentityPinned: boolean;
  reachable: { lan: boolean; relay: boolean };
}

interface PeerList {
  desktopId: string;
  peerChannelAvailable: boolean;
  legacyAccessAllowed: boolean;
  relayPeerTunnelsAvailable: boolean;
  peers: Peer[];
}

type Desktop = Pick<RemoteHarness, "lanBaseUrl" | "desktopId">;

const randomBytes = (length: number) => webcrypto.getRandomValues(new Uint8Array(length));
const marker = (label: string) =>
  `PEER-${label}-${Array.from(randomBytes(6), (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
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

async function startSameAccountPeer(harness: RemoteHarness, label: string): Promise<RemoteDesktop> {
  const desktopId = `desktop-peer-${label}-${Date.now()}`;
  const desktopSecret = desktopSecretFor(desktopId);
  await publishDesktopCredentialAsBuffy(harness, { desktopId, desktopSecret, displayName: `Peer E2E ${label}` });
  return await harness.startAdditionalDesktop({ desktopId, desktopSecret });
}

async function json<T>(response: Response): Promise<T> {
  const text = await response.text();
  if (!response.ok) throw new Error(`HTTP ${response.status}: ${text}`);
  return JSON.parse(text) as T;
}

async function invokeMachine(from: Desktop, targetDesktopId: string, path: string, method = "GET", body: unknown = null): Promise<MachineInvokeResult> {
  return json<MachineInvokeResult>(await localProcessFetch(
    `${from.lanBaseUrl}/v1/cloud/desktops/${encodeURIComponent(targetDesktopId)}/invoke`,
    { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ method, path, body }) }
  ));
}

async function invokeMachineRefusal(from: Desktop, targetDesktopId: string, path: string): Promise<string> {
  const response = await localProcessFetch(
    `${from.lanBaseUrl}/v1/cloud/desktops/${encodeURIComponent(targetDesktopId)}/invoke`,
    { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ method: "GET", path, body: null }) }
  );
  const text = await response.text();
  expect(response.ok, `expected a refusal, got ${response.status}: ${text}`).toBe(false);
  return text;
}

async function createOffer(issuer: Desktop): Promise<{ pairingString: string; code: string }> {
  return json(await localProcessFetch(`${issuer.lanBaseUrl}/v1/peers/pairing-offers`, { method: "POST" }));
}

async function pair(claimant: Desktop, pairingString: string): Promise<{ desktopId: string; displayName: string; encryption: string; route: string }> {
  return json(await localProcessFetch(`${claimant.lanBaseUrl}/v1/peers/pair`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ pairingString })
  }));
}

async function pairFailure(claimant: Desktop, pairingString: string): Promise<string> {
  const response = await localProcessFetch(`${claimant.lanBaseUrl}/v1/peers/pair`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ pairingString })
  });
  const text = await response.text();
  expect(response.ok, `expected pairing to fail, got ${response.status}: ${text}`).toBe(false);
  return text;
}

/**
 * Dials the renderer's sibling-view proxy the way the desktop webview does
 * and answers with the first frame the proxy sends back. A handshake refused
 * at the local-client boundary never opens, so it rejects instead - which is
 * the shape of the regression this covers.
 */
async function firstPeerViewFrame(
  url: string,
  headers: Record<string, string>,
  credential: string
): Promise<{ type?: string; code?: string; message?: string }> {
  const socket = new NodeRelaySocket(url, headers);
  try {
    return await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("the peer view proxy sent no frame")), 30_000);
      const settle = (outcome: () => void) => {
        clearTimeout(timer);
        outcome();
      };
      socket.onopen = () => socket.send(JSON.stringify({ type: "auth", credential }));
      socket.onmessage = (event) =>
        settle(() => resolve(JSON.parse(String(event.data)) as { type?: string; code?: string }));
      socket.onclose = (event) =>
        settle(() => reject(new Error(`closed before any frame: ${JSON.stringify(event)}`)));
      socket.onerror = (error) =>
        settle(() => reject(error instanceof Error ? error : new Error(String(error))));
    });
  } finally {
    socket.close();
  }
}

async function listPeers(desktop: Desktop): Promise<PeerList> {
  return json(await localProcessFetch(`${desktop.lanBaseUrl}/v1/peers`));
}

async function unpair(desktop: Desktop, peerDesktopId: string): Promise<number> {
  return (await localProcessFetch(`${desktop.lanBaseUrl}/v1/peers/${encodeURIComponent(peerDesktopId)}`, { method: "DELETE" })).status;
}

async function setPeerLegacyAccess(desktop: Desktop, allowed: boolean): Promise<void> {
  const response = await localProcessFetch(`${desktop.lanBaseUrl}/v1/settings/desktop_peer_legacy_access`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ value: allowed ? "allowed" : "refused" })
  });
  expect(response.ok).toBe(true);
}

async function pairBoth(issuer: Desktop, claimant: Desktop): Promise<string> {
  const offer = await createOffer(issuer);
  expect(offer.pairingString.startsWith(`KANNA-PEER:${issuer.desktopId}:`)).toBe(true);
  const result = await pair(claimant, offer.pairingString);
  expect(result.desktopId).toBe(issuer.desktopId);
  expect(result.encryption).toBe("e2ee");
  return result.route;
}

describe("desktop peer secure channel E2E", () => {
  let harness: RemoteHarness;
  let peer: RemoteDesktop;

  /** Both desktops pinned to each other, whatever an earlier test left. */
  async function ensurePaired(): Promise<void> {
    const mine = await listPeers(harness);
    const theirs = await listPeers(peer);
    if (
      mine.peers.some((entry) => entry.desktopId === peer.desktopId)
      && theirs.peers.some((entry) => entry.desktopId === harness.desktopId)
      && (await invokeMachine(harness, peer.desktopId, "/v1/status")).route.startsWith("peer-")
      && (await invokeMachine(peer, harness.desktopId, "/v1/status")).status === 200
    ) {
      return;
    }
    await pairBoth(peer, harness);
  }

  beforeAll(async () => {
    harness = await startRemoteHarness();
    const desktopSecret = desktopSecretFor(harness.desktopId);
    await publishDesktopCredentialAsBuffy(harness, { desktopId: harness.desktopId, desktopSecret, displayName: "Peer E2E Desktop" });
    await harness.restartServerWithIdentity({ desktopId: harness.desktopId, desktopSecret });
    await harness.waitForDesktop();
    peer = await startSameAccountPeer(harness, "b");
  }, 240_000);

  afterAll(async () => {
    await peer?.stop();
    await harness?.stop();
  }, 30_000);

  it("pairs by string and carries sealed invokes in both directions over the relay", async () => {
    const route = await pairBoth(peer, harness);
    expect(route.startsWith("peer-")).toBe(true);

    const mine = await listPeers(harness);
    expect(mine.peerChannelAvailable).toBe(true);
    expect(mine.relayPeerTunnelsAvailable).toBe(true);
    expect(mine.peers.map((entry) => [entry.desktopId, entry.encryption])).toEqual([[peer.desktopId, "e2ee"]]);
    const theirs = await listPeers(peer);
    expect(theirs.peers.map((entry) => [entry.desktopId, entry.encryption])).toEqual([[harness.desktopId, "e2ee"]]);

    const forward = await invokeMachine(harness, peer.desktopId, "/v1/status");
    expect(forward.status).toBe(200);
    expect(forward.route).toBe("peer-relay");
    expect((forward.body as { desktopId?: string }).desktopId).toBe(peer.desktopId);
    const backward = await invokeMachine(peer, harness.desktopId, "/v1/status");
    expect(backward.status).toBe(200);
    expect(backward.route).toBe("peer-relay");

    // The sibling route set, not desktop-local authority: the peer's own
    // pairing controls stay out of reach through the sealed session.
    const local = await invokeMachine(harness, peer.desktopId, "/v1/peers");
    expect(local.status).toBe(401);
    expect(local.route).toBe("peer-relay");

    const machines = await json<{ machines: Array<{ id: string; encryption: string }> }>(
      await localProcessFetch(`${harness.lanBaseUrl}/v1/cloud/desktops`)
    );
    expect(machines.machines.find((machine) => machine.id === peer.desktopId)?.encryption).toBe("e2ee");

    // A wrong secret pins nothing and counts as a failed attempt.
    const offer = await createOffer(peer);
    const tampered = offer.pairingString.replace(/:[A-Z2-7]+$/, ":AAAAAAAAAAAAAAAAAAAAAAAAAA");
    expect(await pairFailure(harness, tampered)).toContain("peer pairing refused");
    // A key that is not the issuer's fails the handshake: nothing is pinned.
    const [prefix, desktopId, code, , secret] = offer.pairingString.split(":");
    const substitutedKey = "A".repeat(52);
    expect(await pairFailure(harness, `${prefix}:${desktopId}:${code}:${substitutedKey}:${secret}`)).toContain("peer_identity_mismatch");
    const still = await listPeers(harness);
    expect(still.peers).toHaveLength(1);
  }, 120_000);

  it("splices a sibling terminal view through the local proxy with nothing readable at the relay", async () => {
    const task = await createScriptedTask(peer, { displayName: "Sealed peer terminal", tracePartialInput: true });
    const terminalMarker = marker("TERM");
    const outputs: string[] = [];
    let exit: number | null = null;
    const client = new StreamClient({
      url: `ws://127.0.0.1:${harness.ports.server}/v1/peers/${encodeURIComponent(peer.desktopId)}/ksp`,
      // A loopback process needs no credential at the proxy; the desktop
      // webview would present the local control credential here.
      webSocketFactory: (url) => new NodeRelaySocket(url) as unknown as StreamWebSocketLike,
      reconnectDelaysMs: [250]
    });
    try {
      client.attachTerminal(task.taskId, {
        onSnapshot: (_cols, _rows, dataB64) => outputs.push(Buffer.from(dataB64, "base64").toString("utf8")),
        onOutput: (dataB64) => outputs.push(Buffer.from(dataB64, "base64").toString("utf8")),
        onSessionExit: (code) => { exit = code; },
        onError: (code, message) => outputs.push(`ERROR ${code} ${message}`)
      });
      await waitForCondition(
        async () => outputs.join("").includes("SCRIPT_INPUT_READY"),
        30_000,
        `no terminal output through the peer proxy: ${outputs.join("").slice(-500)}`
      );
      client.sendTermInput(task.taskId, Buffer.from(terminalMarker).toString("base64"));
      await waitForCondition(
        async () => outputs.join("").includes(`SCRIPT_PARTIAL:${terminalMarker}`),
        30_000,
        `typed marker never echoed: ${outputs.join("").slice(-500)}`
      );
      // A request over the same spliced session answers with the sibling
      // route set.
      const status = await client.request("GET", "/v1/status");
      expect(status.status).toBe(200);
      expect((status.body as { desktopId?: string }).desktopId).toBe(peer.desktopId);
    } finally {
      client.close();
    }
    expect(exit).toBeNull();

    const inputMarker = marker("INPUT");
    const input = await invokeMachine(harness, peer.desktopId, `/v1/tasks/${encodeURIComponent(task.taskId)}/input`, "POST", { input: inputMarker });
    expect(input.status).toBeGreaterThanOrEqual(200);
    expect(input.status).toBeLessThan(300);
    expect(input.route).toBe("peer-relay");

    // The relay logged every tunnel frame's size and nothing else: no
    // marker, no KSP frame type, no request path.
    const relayLogs = harness.relayLogs();
    expect(relayLogs).toContain("Tunnel client->desktop: <");
    for (const forbidden of [terminalMarker, inputMarker, "Tunnel client->desktop: request", "Tunnel client->desktop: term_input", "Tunnel desktop->client: term_output", "/v1/tasks/"]) {
      expect(relayLogs, `relay log leaked ${forbidden}`).not.toContain(forbidden);
    }
    // Neither server's log carries the markers either.
    expect(harness.serverLogs()).not.toContain(terminalMarker);
    expect(harness.serverLogs()).not.toContain(inputMarker);
    expect(peer.serverLogs()).not.toContain(terminalMarker);
    expect(peer.serverLogs()).not.toContain(inputMarker);
  }, 120_000);

  /**
   * The splice test above dials with a Node socket, which carries neither an
   * `Origin` nor a `Sec-Fetch-*` header and so keeps ordinary loopback
   * authority (`ProxyAuth::LoopbackProcess`). The desktop webview is a
   * browser: its handshake always carries those headers, takes the
   * browser-originated path through `lan_trust`, and proves the local control
   * credential in its first `auth` frame. That path is the one that was
   * broken - `/v1/peers/{desktop_id}/ksp` is parameterized, so it was missing
   * from the stream-upgrade exemption and every renderer handshake was
   * answered 403 before the proxy ever ran. A Node-socket dial can never
   * catch that, so it is asserted here explicitly.
   */
  it("admits the renderer's browser-originated sibling view and refuses it without the credential", async () => {
    await ensurePaired();
    const credential = (await readFile(join(harness.paths.daemonDir, "task-events.token"), "utf8")).trim();
    const url = `ws://127.0.0.1:${harness.ports.server}/v1/peers/${encodeURIComponent(peer.desktopId)}/ksp`;
    const browserHandshake = {
      Origin: "tauri://localhost",
      "Sec-Fetch-Mode": "websocket",
      "Sec-Fetch-Site": "same-origin"
    };

    const admitted = await firstPeerViewFrame(url, browserHandshake, credential);
    expect(admitted.type, `expected the sibling's auth_ok, got ${JSON.stringify(admitted)}`).toBe("auth_ok");

    // The credential is what admits it, not the loopback address: a page the
    // user happens to have open gets the same 4-byte answer as a stranger.
    const refused = await firstPeerViewFrame(url, browserHandshake, "not-the-local-control-token");
    expect(refused).toMatchObject({ type: "error", code: "unauthorized" });
  }, 120_000);

  it("refuses a rotated peer key and recovers only by pairing again", async () => {
    // This desktop loses its peer identity file: the next start mints a
    // new key, which is exactly what an impostor answering at this
    // desktop's address would look like to the sibling.
    await harness.stopServer();
    await rm(join(harness.paths.daemonDir, "peer-channel-identity.json"), { force: true });
    await harness.startServer();
    await harness.waitForDesktop();
    const refused = await invokeMachineRefusal(peer, harness.desktopId, "/v1/status");
    expect(refused).toContain("peer_identity_mismatch");
    // Nothing plaintext was tried instead: the sibling still lists it as a
    // pinned (now stale) peer rather than downgrading.
    expect((await listPeers(peer)).peers.map((entry) => entry.desktopId)).toEqual([harness.desktopId]);
    // Re-pairing replaces the pin on the sibling and restores the route.
    const route = await pairBoth(harness, peer);
    expect(route.startsWith("peer-")).toBe(true);
    const restored = await invokeMachine(peer, harness.desktopId, "/v1/status");
    expect(restored.status).toBe(200);
    expect(restored.route).toBe("peer-relay");
  }, 120_000);

  it("unpairing withdraws the sealed route on both sides until paired again", async () => {
    await ensurePaired();
    expect(await unpair(harness, peer.desktopId)).toBe(204);
    // Legacy routing is still on, so an unpaired sibling is reached the old
    // way - the documented migration window, reported honestly as `relay`.
    const legacy = await invokeMachine(harness, peer.desktopId, "/v1/status");
    expect(legacy.status).toBe(200);
    expect(legacy.route).toBe("relay");
    // The other side still holds its pin, but this desktop no longer
    // recognises that key: the handshake grants it pairing-only authority,
    // which the other side refuses at the handshake - no plaintext fallback.
    expect(await invokeMachineRefusal(peer, harness.desktopId, "/v1/status")).toContain("peer_pairing_required");
    expect(await unpair(harness, peer.desktopId)).toBe(404);
    // With legacy routing off, an unpaired sibling is not reachable at all.
    await setPeerLegacyAccess(harness, false);
    try {
      expect(await invokeMachineRefusal(harness, peer.desktopId, "/v1/status")).toContain("peer_pairing_required");
    } finally {
      await setPeerLegacyAccess(harness, true);
    }
    const route = await pairBoth(peer, harness);
    expect(route.startsWith("peer-")).toBe(true);
    const sealed = await invokeMachine(harness, peer.desktopId, "/v1/status");
    expect(sealed.status).toBe(200);
    expect(sealed.route).toBe("peer-relay");
  }, 120_000);

  it("reconnects with a fresh handshake after the relay restarts", async () => {
    await ensurePaired();
    await harness.stopRelay();
    await harness.startRelay();
    await harness.waitForDesktop(harness.desktopId);
    await harness.waitForDesktop(peer.desktopId);
    await waitForCondition(
      async () => {
        try {
          const response = await invokeMachine(harness, peer.desktopId, "/v1/status");
          return response.status === 200 && response.route === "peer-relay";
        } catch {
          return false;
        }
      },
      60_000,
      "sealed invoke did not resume after the relay restart"
    );
  }, 120_000);

  it("with legacy routing off, an unpaired sibling's relay-attested invoke is refused while the sealed route keeps working", async () => {
    await ensurePaired();
    const stranger = await startSameAccountPeer(harness, "c");
    try {
      const allowed = await invokeMachine(stranger, peer.desktopId, "/v1/status");
      expect(allowed.status).toBe(200);
      expect(allowed.route).toBe("relay");
      await setPeerLegacyAccess(peer, false);
      try {
        const refused = await invokeMachine(stranger, peer.desktopId, "/v1/status");
        expect(refused.status).toBe(401);
        expect(refused.error ?? "").toContain("peer_legacy_access_refused");
        const sealed = await invokeMachine(harness, peer.desktopId, "/v1/status");
        expect(sealed.status).toBe(200);
        expect(sealed.route).toBe("peer-relay");
        const machines = await json<{ machines: Array<{ id: string; encryption: string }> }>(
          await localProcessFetch(`${peer.lanBaseUrl}/v1/cloud/desktops`)
        );
        expect(machines.machines.find((machine) => machine.id === harness.desktopId)?.encryption).toBe("e2ee");
        expect(machines.machines.find((machine) => machine.id === stranger.desktopId)?.encryption).toBe("pairingRequired");
        expect(await invokeMachineRefusal(peer, stranger.desktopId, "/v1/status")).toContain("peer_pairing_required");
      } finally {
        await setPeerLegacyAccess(peer, true);
      }
    } finally {
      await stranger.stop();
    }
  }, 120_000);
});
