// A desktop-side secure-channel responder for mobile unit tests: answers the
// handshake with the TypeScript Noise core, then plays a minimal KSP server
// over the sealed frames (auth -> auth_ok, request -> a routed response).
// It is deliberately in-memory (no sockets) so tests are deterministic.

import { webcrypto } from "node:crypto";
import {
  generateKeypair,
  readInitiatorHello,
  encodeKey,
  type Channel,
  type Keypair,
  type SealedWebSocketLike,
} from "@kanna/secure-channel";

export const testRandomBytes = (length: number) => webcrypto.getRandomValues(new Uint8Array(length));

export interface FakeSecureDesktopRoute {
  status: number;
  body?: unknown;
}

export interface FakeSecureDesktopOptions {
  desktopId: string;
  identity?: Keypair;
  /** Answers a sealed KSP request. Receives the parsed request frame. */
  route(request: { method: string; path: string; body?: unknown }): FakeSecureDesktopRoute | Promise<FakeSecureDesktopRoute>;
  /** When set, answers the handshake with this plaintext instead (an old
   * desktop that does not know the secure channel). */
  plaintextReply?: string;
}

export interface FakeSecureDesktop {
  identity: Keypair;
  publicKey: string;
  /** The last established channel (desktop side), for SAS comparison. */
  lastChannel: Channel | null;
  handshakes: number;
  /** Every plaintext WebSocket frame the phone sent. */
  wireFrames: string[];
  /** A fresh socket the phone can dial. */
  createSocket(): SealedWebSocketLike;
}

export function createFakeSecureDesktop(options: FakeSecureDesktopOptions): FakeSecureDesktop {
  const identity = options.identity ?? generateKeypair(testRandomBytes);
  const desktop: FakeSecureDesktop = {
    identity,
    publicKey: encodeKey(identity.publicKey),
    lastChannel: null,
    handshakes: 0,
    wireFrames: [],
    createSocket() {
      let channel: Channel | null = null;
      let closed = false;
      const socket: SealedWebSocketLike = {
        onopen: null,
        onmessage: null,
        onclose: null,
        onerror: null,
        send(data: string) {
          desktop.wireFrames.push(data);
          if (closed) return;
          if (!channel) {
            if (options.plaintextReply !== undefined) {
              const reply = options.plaintextReply;
              queueMicrotask(() => socket.onmessage?.({ data: reply }));
              return;
            }
            let pending: ReturnType<typeof readInitiatorHello>;
            try {
              pending = readInitiatorHello(identity, options.desktopId, data, { randomBytes: testRandomBytes });
            } catch (error) {
              // The real server answers a failed handshake in the clear.
              const reply = JSON.stringify({
                type: "error",
                code: "secure_channel_refused",
                message: error instanceof Error ? error.message : String(error),
              });
              queueMicrotask(() => socket.onmessage?.({ data: reply }));
              return;
            }
            const { message2, channel: established } = pending.accept({ version: 1, desktopId: options.desktopId });
            channel = established;
            desktop.lastChannel = established;
            desktop.handshakes += 1;
            queueMicrotask(() => socket.onmessage?.({ data: message2 }));
            return;
          }
          const active = channel;
          const received = active.receiver.open(data);
          for (const item of received) {
            if (item.kind === "closed") {
              closed = true;
              return;
            }
            const frame = JSON.parse(new TextDecoder().decode(item.data)) as Record<string, unknown>;
            if (frame.type === "auth") {
              const reply = JSON.stringify({ type: "auth_ok", stream_kinds: ["agent", "terminal"], capabilities: [] });
              queueMicrotask(() => socket.onmessage?.({ data: active.sender.sealText(reply) }));
            } else if (frame.type === "request") {
              void Promise.resolve(
                options.route({
                  method: String(frame.method),
                  path: String(frame.path),
                  body: frame.body,
                }),
              ).then((response) => {
                const reply = JSON.stringify({
                  type: "response",
                  id: frame.id,
                  status: response.status,
                  ...(response.body === undefined ? {} : { body: response.body }),
                });
                if (!closed) socket.onmessage?.({ data: active.sender.sealText(reply) });
              });
            }
          }
        },
        close() {
          closed = true;
          queueMicrotask(() => socket.onclose?.({ code: 1000 }));
        },
      };
      queueMicrotask(() => socket.onopen?.({}));
      return socket;
    },
  };
  return desktop;
}
