import { describe, expect, it, vi } from "vitest";
import { encodeKey, generateKeypair, type SealedWebSocketLike } from "@kanna/secure-channel";
import { createLanTransport, type FetchLike, type WebSocketLike } from "./lanTransport";
import { ServerRefusalError } from "./serverRefusal";
import type { SecureChannelPeer } from "../security/secureChannelPeer";
import { createFakeSecureDesktop, testRandomBytes } from "../../test/fakeSecureDesktop";

describe("LAN transport over the secure channel", () => {
  function setup(route: Parameters<typeof createFakeSecureDesktop>[0]["route"]) {
    const desktop = createFakeSecureDesktop({ desktopId: "DESKTOP-1", route });
    const identity = generateKeypair(testRandomBytes);
    const refusals: string[] = [];
    let established = 0;
    const peer: SecureChannelPeer = {
      desktopId: "DESKTOP-1",
      desktopPublicKey: desktop.publicKey,
      identity,
      deviceId: "phone-1",
      intent: "session",
      randomBytes: testRandomBytes,
      onRefusal: (refusal, detail) => refusals.push(`${refusal}: ${detail}`),
      onEstablished: () => {
        established += 1;
      },
    };
    const fetchImpl = vi.fn<FetchLike>(async (url) => ({
      ok: true,
      status: 200,
      json: async () => (url.endsWith("/v1/status") ? { desktopId: "DESKTOP-1", kspStreamVersion: 2 } : {}),
    }));
    const socketHeaders: Array<Record<string, string> | undefined> = [];
    const transport = createLanTransport(
      "http://10.0.0.5:48120",
      fetchImpl,
      (url, headers) => {
        socketHeaders.push(headers);
        return desktop.createSocket() as unknown as WebSocketLike;
      },
      { deviceCredentials: { deviceId: "phone-1", deviceSecret: "must-not-be-sent" }, secureChannel: peer },
    );
    return { desktop, transport, fetchImpl, refusals, socketHeaders, established: () => established };
  }

  it("carries REST calls as sealed KSP request frames and sends no bearer secret anywhere", async () => {
    const seen: Array<{ method: string; path: string; body?: unknown }> = [];
    const { desktop, transport, fetchImpl, socketHeaders, established } = setup((request) => {
      seen.push(request);
      if (request.path === "/v1/tasks/recent?includeNeedsAttention=true") {
        return { status: 200, body: [{ id: "t1", title: "Task" }] };
      }
      if (request.path === "/v1/tasks/t1/input") return { status: 204 };
      return { status: 404, body: { error: "no route" } };
    });

    const recent = await transport.listRecentTasks();
    expect(recent).toEqual([{ id: "t1", title: "Task" }]);
    await expect(transport.sendTaskInput("t1", "hello")).resolves.toEqual({ status: "delivered" });
    expect(seen).toEqual([
      { method: "GET", path: "/v1/tasks/recent?includeNeedsAttention=true", body: undefined },
      { method: "POST", path: "/v1/tasks/t1/input", body: { input: "hello" } },
    ]);
    expect(established()).toBe(1);
    // No HTTP call carried the request, and the socket was opened without
    // the legacy credential headers.
    expect(fetchImpl).not.toHaveBeenCalled();
    expect(socketHeaders).toEqual([undefined]);
    for (const frame of desktop.wireFrames) {
      expect(frame.startsWith("ksc1:")).toBe(true);
      expect(frame).not.toContain("must-not-be-sent");
      expect(frame).not.toContain("hello");
    }
  });

  it("surfaces the desktop's refusal as a ServerRefusalError with its status", async () => {
    const { transport } = setup(() => ({ status: 409, body: { error: "task is busy" } }));
    await expect(transport.closeTask("t1")).rejects.toMatchObject({
      status: 409,
      message: expect.stringContaining("task is busy"),
    });
    await expect(transport.closeTask("t1")).rejects.toBeInstanceOf(ServerRefusalError);
  });

  it("keeps the plaintext status read for the capability probe only", async () => {
    const { transport, fetchImpl } = setup(() => ({ status: 200 }));
    await expect(transport.getStatus()).resolves.toMatchObject({ desktopId: "DESKTOP-1" });
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    expect(fetchImpl.mock.calls[0][0]).toBe("http://10.0.0.5:48120/v1/status");
  });

  it("fails requests and reports the refusal when the desktop cannot be verified, without retrying in plaintext", async () => {
    const identity = generateKeypair(testRandomBytes);
    const impostor = generateKeypair(testRandomBytes);
    // The phone pinned the real key; the socket is answered by someone else.
    const desktop = createFakeSecureDesktop({ desktopId: "DESKTOP-1", identity: impostor, route: () => ({ status: 200 }) });
    const refusals: string[] = [];
    const peer: SecureChannelPeer = {
      desktopId: "DESKTOP-1",
      // A different, valid key: the phone pinned a desktop that is not the
      // one answering.
      desktopPublicKey: encodeKey(generateKeypair(testRandomBytes).publicKey),
      identity,
      deviceId: "phone-1",
      intent: "session",
      randomBytes: testRandomBytes,
      onRefusal: (refusal) => refusals.push(refusal),
    };
    const sockets: SealedWebSocketLike[] = [];
    const transport = createLanTransport(
      "http://10.0.0.5:48120",
      vi.fn<FetchLike>(),
      () => {
        const socket = desktop.createSocket();
        sockets.push(socket);
        return socket as unknown as WebSocketLike;
      },
      { secureChannel: peer },
    );
    await expect(transport.listRecentTasks()).rejects.toBeInstanceOf(Error);
    expect(refusals.length).toBeGreaterThan(0);
    expect(refusals.every((refusal) => refusal === "identity_mismatch" || refusal === "transport")).toBe(true);
    // Exactly one handshake attempt; the client stopped instead of retrying.
    expect(sockets).toHaveLength(1);
  });
});
