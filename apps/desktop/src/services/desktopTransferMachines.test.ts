import { describe, expect, it, vi } from "vitest";

import {
  createDesktopTransferMachineSync,
  filterPairableTransferPeerPayload,
  pairedTransferPeersFromTargets,
  parseLanTransferPeers,
  mergeTransferMachines,
  type LanTransferPeer,
} from "./desktopTransferMachines";
import type { DesktopCloudTransferMachine } from "./desktopCloudTaskIndex";
import type { DesktopAuthSession } from "./desktopAuth";

const lanPeer = (overrides: Partial<LanTransferPeer> = {}): LanTransferPeer => ({
  id: "peer-b",
  name: "Mac B",
  publicKey: "key-b",
  endpoint: "192.168.1.2:4455",
  trusted: false,
  acceptingTransfers: true,
  ...overrides,
});

const cloudMachine = (
  overrides: Partial<DesktopCloudTransferMachine> = {},
): DesktopCloudTransferMachine => ({
  desktopId: "desktop-b",
  displayName: "Mac B",
  online: true,
  peerId: "peer-b",
  publicKey: "key-b",
  protocolVersion: 1,
  acceptingTransfers: true,
  ...overrides,
});

describe("mergeTransferMachines", () => {
  it("uses same-account trust for a matching LAN peer and keeps cloud fallback", () => {
    expect(mergeTransferMachines({
      currentDesktopId: "desktop-a",
      lanPeers: [lanPeer()],
      cloudMachines: [cloudMachine()],
    })).toEqual([expect.objectContaining({
      peerId: "peer-b",
      trustSource: "same-account-cloud",
      preferredTransport: "lan",
      relayDesktopId: "desktop-b",
      cloudFallback: true,
    })]);
  });

  it("excludes the current, offline, incompatible, and non-accepting cloud desktops", () => {
    expect(mergeTransferMachines({
      currentDesktopId: "desktop-a",
      lanPeers: [],
      cloudMachines: [
        cloudMachine({ desktopId: "desktop-a" }),
        cloudMachine({ desktopId: "desktop-offline", peerId: "peer-offline", online: false }),
        cloudMachine({ desktopId: "desktop-v2", peerId: "peer-v2", protocolVersion: 2 }),
        cloudMachine({
          desktopId: "desktop-disabled",
          peerId: "peer-disabled",
          acceptingTransfers: false,
        }),
      ],
    })).toEqual([]);
  });

  it("prefers cloud for an eligible cloud-only machine", () => {
    expect(mergeTransferMachines({
      currentDesktopId: "desktop-a",
      lanPeers: [],
      cloudMachines: [cloudMachine()],
    })).toEqual([{
      peerId: "peer-b",
      desktopId: "desktop-b",
      name: "Mac B",
      publicKey: "key-b",
      lanEndpoint: null,
      relayDesktopId: "desktop-b",
      trustSource: "same-account-cloud",
      preferredTransport: "cloud",
      cloudFallback: false,
    }]);
  });

  it("does not confer same-account trust when a LAN peer has a mismatched key", () => {
    expect(mergeTransferMachines({
      currentDesktopId: "desktop-a",
      lanPeers: [lanPeer({ publicKey: "different-key" })],
      cloudMachines: [cloudMachine()],
    })).toEqual([]);
  });

  it("keeps a durably paired LAN-only machine", () => {
    expect(mergeTransferMachines({
      currentDesktopId: "desktop-a",
      lanPeers: [lanPeer({ trusted: true })],
      cloudMachines: [],
    })).toEqual([expect.objectContaining({
      peerId: "peer-b",
      trustSource: "paired-lan",
      preferredTransport: "lan",
      relayDesktopId: null,
      cloudFallback: false,
    })]);
  });

  it("keeps real mDNS peers with no process id and excludes explicit cloud proxies", () => {
    const mdns = { peer_id: "peer-mdns", display_name: "LAN Mac", public_key: "key", endpoint: "192.168.1.20:4455", pid: 0, lan_discovered: true, trusted: true, accepting_transfers: true };
    const cloud = { ...mdns, peer_id: "peer-cloud", endpoint: "127.0.0.1:55443", lan_discovered: false };
    const peers = parseLanTransferPeers([mdns, cloud]);
    expect(peers.map((peer) => peer.id)).toEqual(["peer-mdns"]);
    expect(filterPairableTransferPeerPayload([mdns, cloud])).toEqual([mdns]);
    expect(mergeTransferMachines({ currentDesktopId: null, cloudMachines: [], lanPeers: peers }))
      .toEqual([expect.objectContaining({ peerId: "peer-mdns", preferredTransport: "lan", trustSource: "paired-lan" })]);
  });

  it("keeps session-scoped cloud peers out of Pair Machine", () => {
    expect(filterPairableTransferPeerPayload([
      { peer_id: "peer-lan", pid: 42 },
      { peer_id: "peer-cloud", pid: 0 },
      { peer_id: "peer-legacy" },
    ])).toEqual([
      { peer_id: "peer-lan", pid: 42 },
      { peer_id: "peer-legacy" },
    ]);
  });
});

describe("cloud credential renewal", () => {
  it("forces Firebase renewal for an explicit route refresh", async () => {
    const getIdToken = vi.fn(async (_forceRefresh?: boolean) => "fresh-id-token");
    const session: DesktopAuthSession = {
      initialize: async () => {},
      getState: () => ({
        status: "signedIn",
        user: { uid: "owner", email: null, displayName: null },
      }),
      subscribe: () => () => undefined,
      signInWithEmailPassword: async () => {},
      signOut: async () => ({ desktopCredentialError: null }),
      getIdToken,
    };
    const sync = createDesktopTransferMachineSync({
      getTransferIdentity: async () => ({
        peerId: "peer-local",
        displayName: "Local",
        publicKey: "public-local",
        protocolVersion: 1,
        acceptingTransfers: true,
      }),
      putLocalIdentity: async () => {},
      resolveRelayUrl: async () => "wss://relay.kanna.build",
      ensureProxy: async () => ({ endpoint: "127.0.0.1:4455" }),
      removeProxy: async () => {},
      clearProxies: async () => {},
      upsertExternalPeer: async () => ({}),
      removeExternalPeer: async () => ({}),
      clearExternalPeers: async () => ({}),
    });

    await sync.setSignedInSession(session, "desktop-local");
    await sync.setCloudMachines([cloudMachine()]);
    await sync.markSidecarReady();
    getIdToken.mockClear();

    await sync.refreshCloudRoute("peer-b");
    expect(getIdToken).toHaveBeenCalledExactlyOnceWith(true);
  });
});

describe("paired peers (sealed routes)", () => {
  const paired = (overrides = {}) => ({
    peerId: "peer-b",
    desktopId: "desktop-b",
    name: "Mac B",
    transferable: true,
    unavailableReason: null,
    ...overrides,
  });

  it("offers a paired sibling as end-to-end encrypted and drops the same machine's Firestore and LAN entries", () => {
    const machines = mergeTransferMachines({
      currentDesktopId: "desktop-a",
      lanPeers: [lanPeer({ trusted: true, publicKey: "key-from-mdns" })],
      cloudMachines: [cloudMachine({ publicKey: "key-substituted-in-firestore" })],
      pairedPeers: [paired()],
    });
    expect(machines).toEqual([expect.objectContaining({
      peerId: "peer-b",
      desktopId: "desktop-b",
      trustSource: "paired-peer",
      preferredTransport: "cloud",
      cloudFallback: false,
      relayDesktopId: "desktop-b",
    })]);
  });

  it("drops every legacy route once legacy desktop-to-desktop access is off", () => {
    expect(mergeTransferMachines({
      currentDesktopId: "desktop-a",
      lanPeers: [lanPeer({ id: "peer-lan", trusted: true })],
      cloudMachines: [cloudMachine({ desktopId: "desktop-legacy", peerId: "peer-legacy" })],
      pairedPeers: [paired()],
      legacyAllowed: false,
    }).map((machine) => machine.trustSource)).toEqual(["paired-peer"]);
  });

  it("skips a paired sibling whose route is not transferable, and itself", () => {
    expect(mergeTransferMachines({
      currentDesktopId: "desktop-a",
      lanPeers: [],
      cloudMachines: [],
      pairedPeers: [
        paired({ transferable: false, unavailableReason: "transfer identity not pinned" }),
        paired({ desktopId: "desktop-a", peerId: "peer-a" }),
      ],
    })).toEqual([]);
  });

  it("maps the server's resolved targets to sealed routes only", () => {
    expect(pairedTransferPeersFromTargets([
      {
        peerId: "peer-b", name: "Mac B", machineId: "desktop-b", trusted: true, acceptingTransfers: true,
        lanAvailable: false, cloudAvailable: true, preferredTransport: "cloud", cloudFallback: false,
        cloudRoute: { peerId: "peer-b", machineId: "desktop-b", status: "ready", kind: "peer-tunnel" },
        transferable: true, unavailableReason: null,
      },
      {
        peerId: "peer-legacy", name: "Legacy", machineId: "desktop-legacy", trusted: true, acceptingTransfers: true,
        lanAvailable: false, cloudAvailable: true, preferredTransport: "cloud", cloudFallback: false,
        cloudRoute: { peerId: "peer-legacy", machineId: "desktop-legacy", status: "ready", kind: "relay-proxy" },
        transferable: true, unavailableReason: null,
      },
    ])).toEqual([paired()]);
  });

  it("registers only the unpaired same-account machine from Firestore", async () => {
    const upsert = vi.fn(async () => undefined);
    const removeExternalPeer = vi.fn(async () => undefined);
    const removeProxy = vi.fn(async () => undefined);
    const sync = createDesktopTransferMachineSync({
      getTransferIdentity: async () => ({
        peerId: "peer-a", displayName: "Mac A", publicKey: "key-a", protocolVersion: 1, acceptingTransfers: true,
      }),
      putLocalIdentity: async () => undefined,
      resolveRelayUrl: async () => "ws://127.0.0.1:9080",
      ensureProxy: async ({ peerId }) => ({ endpoint: `127.0.0.1:1${peerId.length}` }),
      removeProxy,
      clearProxies: async () => undefined,
      upsertExternalPeer: upsert,
      removeExternalPeer,
      clearExternalPeers: async () => undefined,
    });
    const session = {
      getIdToken: async () => "id-token",
    } as unknown as DesktopAuthSession;
    await sync.markSidecarReady();
    await sync.setSignedInSession(session, "desktop-a");
    sync.setPairedPeers([paired()]);
    await sync.setCloudMachines([
      cloudMachine(),
      cloudMachine({ desktopId: "desktop-legacy", peerId: "peer-legacy" }),
    ]);
    // Only the unpaired legacy machine was registered from Firestore; the
    // paired one rides its sealed route.
    expect(upsert.mock.calls.map(([input]) => input.peer.peerId)).toEqual(["peer-legacy"]);
    expect(sync.getTransferMachines().map((machine) => [machine.peerId, machine.trustSource])).toEqual([
      ["peer-b", "paired-peer"],
      ["peer-legacy", "same-account-cloud"],
    ]);
    // Legacy desktop-to-desktop access stopped being a setting on
    // 2026-09-20; outbound registration is unchanged, so there is no switch
    // left to turn it off here. A sibling on 0.4.0 or later refuses the
    // route at its own end instead.
    expect(removeExternalPeer).not.toHaveBeenCalled();
    expect(removeProxy).not.toHaveBeenCalled();
  });
});
