// Sealed desktop-to-desktop sessions against the real relay, two real
// servers and daemons: automatic same-account enrollment with no ceremony at
// all, the pairing string upgrading that pin to verified, sealed invokes in
// both directions through a desktop-secret relay tunnel, a sibling terminal
// view spliced through the local peer proxy with known markers asserted
// absent from the relay's frame log and both servers' logs, a rotated peer
// key refused and never re-enrolled, unpairing, and reconnection after a
// relay restart.
//
// The first case is the reported failure itself: two desktops signed into one
// account, freshly started, no pins anywhere, opening a session and getting
// "this desktop is not paired with that machine". It must now simply work.
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
  provenance: "verified" | "account";
  identityChanged: boolean;
  transferIdentityPinned: boolean;
  reachable: { lan: boolean; relay: boolean };
}

interface PeerList {
  desktopId: string;
  peerChannelAvailable: boolean;
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

interface Machine {
  id: string;
  encryption: string;
  provenance?: "verified" | "account";
  identityChanged?: boolean;
}

async function listMachines(desktop: Desktop): Promise<Machine[]> {
  const list = await json<{ machines: Machine[] }>(
    await localProcessFetch(`${desktop.lanBaseUrl}/v1/cloud/desktops`)
  );
  return list.machines;
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

  // The reported failure, exactly: two same-account desktops, no ceremony
  // anywhere, one opens a session to the other. This must establish trust by
  // itself and take the sealed route - never `peer_pairing_required`, and
  // never the legacy plaintext path either.
  it("pairs two same-account desktops automatically on first contact, with no ceremony", async () => {
    expect(await listPeers(harness).then((list) => list.peers), "no pin exists yet").toEqual([]);
    expect(await listPeers(peer).then((list) => list.peers)).toEqual([]);

    const first = await invokeMachine(harness, peer.desktopId, "/v1/status");
    expect(first.status).toBe(200);
    expect(first.route, "first contact must be sealed, not the legacy relay path").toBe("peer-relay");
    expect((first.body as { desktopId?: string }).desktopId).toBe(peer.desktopId);

    // One exchange pins both directions, and both say what they rest on.
    const mine = (await listPeers(harness)).peers;
    expect(mine.map((entry) => [entry.desktopId, entry.encryption, entry.provenance]))
      .toEqual([[peer.desktopId, "e2ee", "account"]]);
    expect(mine[0].identityChanged).toBe(false);
    const theirs = (await listPeers(peer)).peers;
    expect(theirs.map((entry) => [entry.desktopId, entry.encryption, entry.provenance]))
      .toEqual([[harness.desktopId, "e2ee", "account"]]);

    // The reverse direction rides that same pin without a second exchange.
    const backward = await invokeMachine(peer, harness.desktopId, "/v1/status");
    expect(backward.status).toBe(200);
    expect(backward.route).toBe("peer-relay");

    const machine = (await listMachines(harness)).find((entry) => entry.id === peer.desktopId);
    expect(machine?.encryption).toBe("e2ee");
    expect(machine?.provenance, "an automatic pin must never claim to be verified").toBe("account");
    expect(machine?.identityChanged).toBe(false);
  }, 120_000);

  it("pairs by string and carries sealed invokes in both directions over the relay", async () => {
    const route = await pairBoth(peer, harness);
    expect(route.startsWith("peer-")).toBe(true);

    const mine = await listPeers(harness);
    expect(mine.peerChannelAvailable).toBe(true);
    expect(mine.relayPeerTunnelsAvailable).toBe(true);
    // The ceremony is the upgrade path: the record the previous case
    // established automatically is now a verified one.
    expect(mine.peers.map((entry) => [entry.desktopId, entry.encryption, entry.provenance]))
      .toEqual([[peer.desktopId, "e2ee", "verified"]]);
    const theirs = await listPeers(peer);
    expect(theirs.peers.map((entry) => [entry.desktopId, entry.encryption, entry.provenance]))
      .toEqual([[harness.desktopId, "e2ee", "verified"]]);

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

    expect((await listMachines(harness)).find((machine) => machine.id === peer.desktopId))
      .toMatchObject({ encryption: "e2ee", provenance: "verified" });

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
    // And automatic enrollment must NOT rescue this. The relay is publishing
    // the new key right now and both desktops are on one account, so the only
    // thing standing between the sibling and the impostor's key is the pin -
    // which is exactly the property trust-on-first-use depends on. Repeated
    // attempts must keep failing, and the change must be reported.
    for (let attempt = 0; attempt < 2; attempt += 1) {
      expect(await invokeMachineRefusal(peer, harness.desktopId, "/v1/status"))
        .toContain("peer_identity_mismatch");
    }
    const stale = (await listPeers(peer)).peers[0];
    expect(stale.identityChanged, "a changed key must be reported, not retried away").toBe(true);
    expect((await listMachines(peer)).find((machine) => machine.id === harness.desktopId))
      .toMatchObject({ encryption: "e2ee", identityChanged: true });

    // Two documented recoveries, both requiring a person. First: unpairing
    // clears the stale pin, after which automatic enrollment may run again.
    expect(await unpair(peer, harness.desktopId)).toBe(204);
    const reEnrolled = await invokeMachine(peer, harness.desktopId, "/v1/status");
    expect(reEnrolled.status).toBe(200);
    expect(reEnrolled.route).toBe("peer-relay");
    expect((await listPeers(peer)).peers[0]).toMatchObject({ provenance: "account", identityChanged: false });
    // Second: the ceremony, which also upgrades the record to verified.
    const route = await pairBoth(harness, peer);
    expect(route.startsWith("peer-")).toBe(true);
    const restored = await invokeMachine(peer, harness.desktopId, "/v1/status");
    expect(restored.status).toBe(200);
    expect(restored.route).toBe("peer-relay");
    expect((await listPeers(peer)).peers[0].provenance).toBe("verified");
  }, 120_000);

  // Unpairing still withdraws the route; what changed is what happens next.
  // For a same-account pair the answer is no longer the legacy plaintext
  // path, and is never `peer_pairing_required`: the machines simply pair
  // themselves again, which is the whole point of this work.
  it("unpairing withdraws the sealed route, and same-account machines re-establish it themselves", async () => {
    await ensurePaired();
    expect(await unpair(harness, peer.desktopId)).toBe(204);
    expect(await unpair(harness, peer.desktopId)).toBe(404);

    // Legacy routing is on, but it is not what serves this: the sibling is
    // enrolled and the invoke takes the sealed route, not `relay`.
    const reEnrolled = await invokeMachine(harness, peer.desktopId, "/v1/status");
    expect(reEnrolled.status).toBe(200);
    expect(reEnrolled.route, "an unpaired same-account sibling must re-pair, not downgrade").toBe("peer-relay");
    expect((await listPeers(harness)).peers[0]).toMatchObject({ desktopId: peer.desktopId, provenance: "account" });

    // And with legacy routing off - the configuration this work is meant to
    // make shippable - it still works, because nothing plaintext is needed.
    expect(await unpair(harness, peer.desktopId)).toBe(204);
    await setPeerLegacyAccess(harness, false);
    try {
      const strict = await invokeMachine(harness, peer.desktopId, "/v1/status");
      expect(strict.status).toBe(200);
      expect(strict.route).toBe("peer-relay");
    } finally {
      await setPeerLegacyAccess(harness, true);
    }

    // The ceremony still upgrades that automatic pin to a verified one.
    const route = await pairBoth(peer, harness);
    expect(route.startsWith("peer-")).toBe(true);
    const sealed = await invokeMachine(harness, peer.desktopId, "/v1/status");
    expect(sealed.status).toBe(200);
    expect(sealed.route).toBe("peer-relay");
    expect((await listPeers(harness)).peers[0].provenance).toBe("verified");
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

  // A third machine joining the account is the ordinary case this work is
  // for: it has paired with nobody, and it reaches its siblings sealed from
  // its very first call - with legacy routing off, so nothing plaintext can
  // be what served it. The plaintext paths' own refusals are covered where
  // they can be provoked deterministically, in
  // `peer_tests::the_legacy_gate_refuses_every_plaintext_sibling_path_when_off`.
  it("a newly added third machine reaches its siblings sealed, with legacy routing off", async () => {
    await ensurePaired();
    const stranger = await startSameAccountPeer(harness, "c");
    try {
      await setPeerLegacyAccess(peer, false);
      await setPeerLegacyAccess(stranger, false);
      try {
        const sealed = await invokeMachine(stranger, peer.desktopId, "/v1/status");
        expect(sealed.status).toBe(200);
        expect(sealed.route, "a brand-new same-account machine must not need a ceremony").toBe("peer-relay");
        expect((await listPeers(stranger)).peers.map((entry) => [entry.desktopId, entry.provenance]))
          .toEqual([[peer.desktopId, "account"]]);

        // The existing verified pair is untouched by any of it.
        const existing = await invokeMachine(harness, peer.desktopId, "/v1/status");
        expect(existing.status).toBe(200);
        expect(existing.route).toBe("peer-relay");
        const machines = await listMachines(peer);
        expect(machines.find((machine) => machine.id === harness.desktopId))
          .toMatchObject({ encryption: "e2ee", provenance: "verified" });
        expect(machines.find((machine) => machine.id === stranger.desktopId))
          .toMatchObject({ encryption: "e2ee", provenance: "account" });
      } finally {
        await setPeerLegacyAccess(peer, true);
        await setPeerLegacyAccess(stranger, true);
      }
    } finally {
      await stranger.stop();
    }
  }, 120_000);
});
