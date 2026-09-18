// The introducer half of automatic same-account E2EE peer trust: a desktop's
// control socket announces its peer channel public key on the auth frame, and
// `list_active_desktops` hands it to the account's other desktops.
//
// The property under test is who may publish a key, not who may read one:
// only a socket that proved *that* desktop's own secret on this connection is
// listed with a key, so a device-token socket or a tunnel client introduces
// nothing. `desktopIds` stays byte-identical for an older desktop.

import { EventEmitter } from "node:events";
import { describe, expect, it } from "vitest";
import type { WebSocket } from "ws";
import {
  isPeerChannelPublicKey,
  routeMessage,
  setServerConnection,
} from "./router.js";

class FakeSocket extends EventEmitter {
  readyState = 1;
  readonly sent: string[] = [];
  bufferedAmount = 0;

  send(data: unknown, options?: unknown, callback?: (error?: Error) => void): void {
    this.sent.push(typeof data === "string" ? data : String(data));
    if (typeof options === "function") options();
    else callback?.();
  }

  close(): void {
    this.readyState = 3;
  }

  pause(): void {}
  resume(): void {}

  frames(): Array<Record<string, unknown>> {
    return this.sent.map((entry) => JSON.parse(entry) as Record<string, unknown>);
  }
}

const ws = (socket: FakeSocket) => socket as unknown as WebSocket;
let nextUser = 0;
const user = () => `peer-key-user-${++nextUser}`;

const KEY_A = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const KEY_B = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

/** The `data` payload of the single response `list_active_desktops` produced. */
function listing(userId: string, source: FakeSocket, sourceDesktopId: string) {
  routeMessage(
    userId,
    "server",
    JSON.stringify({ type: "invoke", id: "list-1", command: "list_active_desktops" }),
    ws(source),
    sourceDesktopId,
    { kind: "desktop", desktopId: sourceDesktopId, desktopSecret: "secret" },
  );
  const frame = source.frames().at(-1) as {
    data: {
      desktopIds: string[];
      desktops: Array<{ desktopId: string; peerChannelPublicKey: string | null }>;
    };
  };
  return frame.data;
}

describe("peer channel key announcement", () => {
  it("lists the key a verified desktop-secret socket announced", () => {
    const userId = user();
    const a = new FakeSocket();
    setServerConnection(userId, "desktop-a", ws(a), {
      kind: "desktop",
      desktopId: "desktop-a",
      desktopSecret: "secret",
    }, KEY_A);
    const b = new FakeSocket();
    setServerConnection(userId, "desktop-b", ws(b), {
      kind: "desktop",
      desktopId: "desktop-b",
      desktopSecret: "secret",
    }, KEY_B);

    const data = listing(userId, a, "desktop-a");
    expect(data.desktopIds).toEqual(["desktop-a", "desktop-b"]);
    expect(data.desktops).toEqual([
      { desktopId: "desktop-a", peerChannelPublicKey: KEY_A },
      { desktopId: "desktop-b", peerChannelPublicKey: KEY_B },
    ]);
  });

  it("lists a desktop that announced nothing, and a device-token socket, without a key", () => {
    const userId = user();
    const a = new FakeSocket();
    setServerConnection(userId, "desktop-a", ws(a), {
      kind: "desktop",
      desktopId: "desktop-a",
      desktopSecret: "secret",
    }, KEY_A);
    // An older Kanna: verified, but announces nothing.
    const quiet = new FakeSocket();
    setServerConnection(userId, "desktop-quiet", ws(quiet), {
      kind: "desktop",
      desktopId: "desktop-quiet",
      desktopSecret: "secret",
    });
    // A device token proves account membership, never which desktop this is,
    // so a key presented on it is not published.
    const legacy = new FakeSocket();
    setServerConnection(userId, "desktop-legacy", ws(legacy), {
      kind: "device",
      desktopId: "desktop-legacy",
      deviceToken: "token",
    }, KEY_B);

    const data = listing(userId, a, "desktop-a");
    expect(data.desktopIds).toEqual(["desktop-a", "desktop-quiet", "desktop-legacy"]);
    expect(
      Object.fromEntries(data.desktops.map((entry) => [entry.desktopId, entry.peerChannelPublicKey])),
    ).toEqual({
      "desktop-a": KEY_A,
      "desktop-quiet": null,
      "desktop-legacy": null,
    });
  });

  it("refuses a malformed announcement at the shape check", () => {
    expect(isPeerChannelPublicKey(KEY_A)).toBe(true);
    for (const value of [
      "",
      "not-a-key",
      `${KEY_A}=`,
      KEY_A.slice(0, 42),
      `${KEY_A}A`,
      "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA+",
      42,
      null,
      undefined,
      { key: KEY_A },
    ]) {
      expect(isPeerChannelPublicKey(value)).toBe(false);
    }
  });
});
