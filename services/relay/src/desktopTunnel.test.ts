// The relay's half of sealed desktop-to-desktop sessions: a second
// desktop-secret socket (`tunnel_client`) may ask for a `peer` tunnel to a
// sibling of the same account, and only that. Exercised at the router
// boundary with fake sockets; the entitlement gate (4402) and the
// `desktopTunnel` advertisement live in the socket handler and are proven by
// the remote E2E against the real relay.

import { EventEmitter } from "node:events";
import { describe, expect, it } from "vitest";
import type { WebSocket } from "ws";
import {
  forwardTunnelData,
  routeDesktopTunnelRequest,
  routeMessage,
  setPhoneConnection,
  setServerConnection,
} from "./router.js";

class FakeSocket extends EventEmitter {
  readyState = 1;
  readonly sent: string[] = [];
  closed: { code: number; reason: string } | null = null;
  bufferedAmount = 0;

  send(data: unknown, options?: unknown, callback?: (error?: Error) => void): void {
    this.sent.push(typeof data === "string" ? data : String(data));
    if (typeof options === "function") options();
    else callback?.();
  }

  close(code = 1000, reason = ""): void {
    this.closed = { code, reason };
    this.readyState = 3;
    this.emit("close", code, Buffer.from(reason));
  }

  pause(): void {}
  resume(): void {}

  frames(): Array<Record<string, unknown>> {
    return this.sent.map((entry) => JSON.parse(entry) as Record<string, unknown>);
  }
}

const ws = (socket: FakeSocket) => socket as unknown as WebSocket;
let nextUser = 0;
const user = () => `user-${++nextUser}`;

function desktop(userId: string, desktopId: string, kind: "desktop" | "device" = "desktop"): FakeSocket {
  const socket = new FakeSocket();
  setServerConnection(
    userId,
    desktopId,
    ws(socket),
    kind === "desktop"
      ? { kind: "desktop", desktopId, desktopSecret: "secret" }
      : { kind: "device", desktopId, deviceToken: "token" },
  );
  return socket;
}

describe("desktop peer tunnels", () => {
  it("lets a desktop-secret tunnel client open a peer tunnel to a verified sibling", () => {
    const userId = user();
    const target = desktop(userId, "desktop-b");
    const client = new FakeSocket();
    routeDesktopTunnelRequest(
      userId,
      ws(client),
      { type: "tunnel_request", id: "t1", desktopId: "desktop-b", service: "peer" },
      "desktop-a",
      { kind: "desktop", desktopId: "desktop-a", desktopSecret: "secret" },
    );
    expect(client.sent).toEqual([]);
    const [establish] = target.frames();
    expect(establish).toMatchObject({
      type: "tunnel_establish",
      id: "t1",
      desktopId: "desktop-b",
      service: "peer",
    });
    expect(typeof establish.tunnelId).toBe("string");

    // The requesting socket is now a tunnel socket: frames it sends are
    // forwarded verbatim to whoever attaches, never parsed - a sealed
    // frame in particular is opaque to the relay.
    const opaque = "ksc1:AAECAwQFBgcICQ==";
    forwardTunnelData(ws(client), Buffer.from(opaque), false);
    // No peer attached yet: nothing forwarded, nothing echoed, nothing closed.
    expect(client.sent).toEqual([]);
    expect(client.closed).toBeNull();
  });

  it("refuses a device-token client, an unverified target, itself, and a phone service", () => {
    const userId = user();
    desktop(userId, "desktop-b");
    desktop(userId, "desktop-legacy", "device");

    const deviceClient = new FakeSocket();
    routeDesktopTunnelRequest(
      userId,
      ws(deviceClient),
      { type: "tunnel_request", id: 1, desktopId: "desktop-b", service: "peer" },
      "desktop-a",
      { kind: "device", desktopId: "desktop-a", deviceToken: "token" },
    );
    expect(deviceClient.frames()).toEqual([
      { type: "response", id: 1, error: "desktop-secret authentication is required" },
    ]);

    const client = new FakeSocket();
    const proof = { kind: "desktop" as const, desktopId: "desktop-a", desktopSecret: "secret" };
    routeDesktopTunnelRequest(
      userId,
      ws(client),
      { type: "tunnel_request", id: 2, desktopId: "desktop-legacy", service: "peer" },
      "desktop-a",
      proof,
    );
    routeDesktopTunnelRequest(
      userId,
      ws(client),
      { type: "tunnel_request", id: 3, desktopId: "desktop-a", service: "peer" },
      "desktop-a",
      proof,
    );
    routeDesktopTunnelRequest(
      userId,
      ws(client),
      { type: "tunnel_request", id: 4, desktopId: "desktop-b", service: "ksp" },
      "desktop-a",
      proof,
    );
    routeDesktopTunnelRequest(
      userId,
      ws(client),
      { type: "invoke", id: 5, desktopId: "desktop-b" },
      "desktop-a",
      proof,
    );
    routeDesktopTunnelRequest(
      userId,
      ws(client),
      { type: "tunnel_request", id: 6, desktopId: "desktop-offline", service: "peer" },
      "desktop-a",
      proof,
    );
    expect(client.frames().map((frame) => [frame.id, frame.error])).toEqual([
      [2, "target desktop-secret authentication is required"],
      [3, "desktopId must name another desktop of this account"],
      [4, "Unsupported tunnel service"],
      [5, "desktop tunnel clients may only request tunnels"],
      [6, "Desktop offline"],
    ]);
  });

  it("never lets a phone open a peer tunnel", () => {
    const userId = user();
    const target = desktop(userId, "desktop-b");
    const phone = new FakeSocket();
    setPhoneConnection(userId, ws(phone));
    routeMessage(
      userId,
      "phone",
      JSON.stringify({ type: "tunnel_request", id: "p1", desktopId: "desktop-b", service: "peer" }),
      ws(phone),
    );
    expect(phone.frames()).toEqual([
      { type: "response", id: "p1", error: "Unsupported tunnel service" },
    ]);
    expect(target.sent).toEqual([]);
  });

  it("a mismatched source desktop id is refused even with a desktop proof", () => {
    const userId = user();
    desktop(userId, "desktop-b");
    const client = new FakeSocket();
    routeDesktopTunnelRequest(
      userId,
      ws(client),
      { type: "tunnel_request", id: 7, desktopId: "desktop-b", service: "peer" },
      "desktop-a",
      { kind: "desktop", desktopId: "desktop-z", desktopSecret: "secret" },
    );
    expect(client.frames()).toEqual([
      { type: "response", id: 7, error: "desktop-secret authentication is required" },
    ]);
  });
});
