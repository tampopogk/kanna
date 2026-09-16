// A WebSocket-shaped wrapper that performs the KSC handshake on open and
// then seals every outbound text frame and opens every inbound one, so the
// `StreamClient` above it is unchanged: it sees plaintext KSP frames and a
// socket that "opens" only once the desktop has proven its identity.
//
// Failure semantics are the whole point of this file:
// - The desktop answers with something that is not a KSC frame (an old
//   desktop replies to our handshake with a plaintext KSP `error` frame, or
//   an intermediary strips the handshake): `unsupported`. No plaintext KSP
//   frame is ever sent on the socket in that case.
// - The handshake fails to authenticate (wrong desktop key — the desktop
//   rotated, or someone is answering in its place): `identity_mismatch`.
// - No answer within the timeout: `timeout`.
// - A transport frame fails to open after the handshake (tamper, replay,
//   reorder): `transport`, and the socket closes; nothing after it is
//   delivered.
// Every refusal is reported through `onRefusal`, surfaced to the client as a
// synthetic KSP `error` frame (the same trick the relay tunnel socket uses)
// and then as a close with a KSC-specific close code, so a client can tell
// it from an ordinary network drop and stop retrying.

import {
  isWireFrame,
  startInitiator,
  type Channel,
  type InitiatorHello,
  type Keypair,
  type RandomBytes,
  type ResponderHello,
  SecureChannelError,
} from "./channel";
import { utf8Decode, utf8Encode } from "./bytes";

/** Minimal WebSocket surface shared with `@kanna/stream-client` and the
 * mobile transports; structurally compatible with both. */
export interface SealedWebSocketLike {
  send(data: string): void;
  close(): void;
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: ((event: unknown) => void) | null;
  onerror: ((event: unknown) => void) | null;
}

export type SealedSocketRefusal =
  | "unsupported"
  | "identity_mismatch"
  | "timeout"
  | "transport"
  | "peer_closed";

/** WebSocket close code a sealed socket reports when it, not the network,
 * ended the connection. Outside the 4000-4999 application range the relay
 * uses (4005 auth failure, 4402 entitlement, ...). */
export const SECURE_CHANNEL_CLOSE_CODE = 4910;
export const SECURE_CHANNEL_HANDSHAKE_TIMEOUT_MS = 10_000;

export interface SealedSocketOptions {
  /** The transport underneath: a LAN WebSocket or a relay tunnel socket. */
  inner: SealedWebSocketLike;
  localIdentity: Keypair;
  desktopId: string;
  desktopPublicKey: Uint8Array;
  hello: InitiatorHello;
  randomBytes: RandomBytes;
  handshakeTimeoutMs?: number;
  maxMessageLen?: number;
  onRefusal?(refusal: SealedSocketRefusal, detail: string): void;
  /** Called once the desktop's hello has been authenticated. */
  onEstablished?(hello: ResponderHello, channel: Channel): void;
}

export interface SealedSocket extends SealedWebSocketLike {
  /** The established channel's short authentication string, once open. */
  readonly sas: string | null;
}

/** What a plaintext answer to our handshake means. Only the desktop's own
 * refusal codes are distinguished; anything else is "not a secure-channel
 * desktop". The answer is untrusted text: it can only make the refusal
 * *more* specific, never turn it into an acceptance. */
function classifyPlaintextAnswer(data: string): [SealedSocketRefusal, string] {
  try {
    const frame = JSON.parse(data) as { type?: unknown; code?: unknown; message?: unknown };
    if (frame.type === "error" && frame.code === "secure_channel_refused") {
      return ["identity_mismatch", "the desktop refused the secure channel handshake (wrong desktop key, or a revoked/unknown device)"];
    }
    if (frame.type === "error" && frame.code === "secure_channel_unavailable") {
      return ["unsupported", "the desktop has no secure channel identity right now"];
    }
  } catch {
    // Not JSON: an older desktop or an intermediary.
  }
  return ["unsupported", "the desktop did not answer with a secure channel handshake"];
}

export function createSealedSocket(options: SealedSocketOptions): SealedSocket {
  const inner = options.inner;
  const timeoutMs = options.handshakeTimeoutMs ?? SECURE_CHANNEL_HANDSHAKE_TIMEOUT_MS;
  let pending: ReturnType<typeof startInitiator> | null = null;
  let channel: Channel | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let closeNotified = false;
  let closedByUs = false;

  const socket: SealedSocket = {
    sas: null,
    onopen: null,
    onmessage: null,
    onclose: null,
    onerror: null,
    send(data: string) {
      if (!channel) {
        // Nothing may leave before the desktop is authenticated. The
        // StreamClient only sends after `onopen`, so reaching here is a
        // programming error rather than a downgrade path; refuse loudly.
        throw new SecureChannelError("closed", "secure channel not established");
      }
      inner.send(channel.sender.seal(utf8Encode(data)));
    },
    close() {
      closedByUs = true;
      clearTimer();
      if (channel && !channel.sender.isClosed()) {
        try {
          inner.send(channel.sender.sealClose("client closed"));
        } catch {
          // The inner socket may already be gone; the close below still runs.
        }
      }
      inner.close();
    },
  };

  const clearTimer = () => {
    if (timer) clearTimeout(timer);
    timer = null;
  };

  const emitClose = (event: unknown) => {
    if (closeNotified) return;
    closeNotified = true;
    clearTimer();
    socket.onclose?.(event);
  };

  const refuse = (refusal: SealedSocketRefusal, detail: string) => {
    clearTimer();
    options.onRefusal?.(refusal, detail);
    socket.onmessage?.({
      data: JSON.stringify({
        type: "error",
        code: `secure_channel_${refusal}`,
        message: detail,
      }),
    });
    closedByUs = true;
    inner.close();
    emitClose({ code: SECURE_CHANNEL_CLOSE_CODE, reason: `secure_channel_${refusal}: ${detail}` });
  };

  inner.onopen = () => {
    try {
      pending = startInitiator(
        options.localIdentity,
        options.desktopPublicKey,
        options.desktopId,
        options.hello,
        { randomBytes: options.randomBytes, maxMessageLen: options.maxMessageLen },
      );
    } catch (error) {
      refuse("transport", error instanceof Error ? error.message : String(error));
      return;
    }
    timer = setTimeout(() => {
      if (!channel) refuse("timeout", "the desktop did not answer the secure channel handshake");
    }, timeoutMs);
    inner.send(pending.message1);
  };

  inner.onmessage = (event) => {
    const data = event.data;
    if (typeof data !== "string") return;
    if (channel) {
      let received;
      try {
        received = channel.receiver.open(data);
      } catch (error) {
        refuse("transport", error instanceof Error ? error.message : String(error));
        return;
      }
      for (const item of received) {
        if (item.kind === "message") {
          socket.onmessage?.({ data: utf8Decode(item.data) });
        } else {
          options.onRefusal?.("peer_closed", item.reason);
          closedByUs = true;
          inner.close();
          emitClose({ code: 1000, reason: item.reason, authenticatedClose: true });
          return;
        }
      }
      return;
    }
    if (!pending) return;
    if (!isWireFrame(data)) {
      // A desktop that predates KSC answers our handshake with a plaintext
      // KSP error; a current desktop that could not authenticate us (wrong
      // pinned key, or someone answering in its place) says so in the clear
      // too, because we have no channel to read anything else in; and an
      // active relay could try to talk plaintext at us. In every case the
      // channel cannot be verified and nothing else is sent.
      refuse(...classifyPlaintextAnswer(data));
      return;
    }
    try {
      const established = pending.finish(data);
      channel = established.channel;
      pending = null;
      clearTimer();
      (socket as { sas: string | null }).sas = channel.sas;
      if (established.hello.desktopId !== options.desktopId) {
        refuse("identity_mismatch", "the desktop identified itself as a different machine");
        return;
      }
      options.onEstablished?.(established.hello, channel);
      socket.onopen?.({});
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      const reason: SealedSocketRefusal =
        error instanceof SecureChannelError && error.reason === "noise" ? "identity_mismatch" : "transport";
      refuse(reason, detail);
    }
  };

  inner.onerror = (event) => socket.onerror?.(event);
  inner.onclose = (event) => {
    if (!channel && !closedByUs) {
      // Closed under us before the handshake finished: an old desktop that
      // dropped the socket, or a stripped handshake. Report it as a refusal
      // rather than letting the client retry a plaintext path.
      clearTimer();
      options.onRefusal?.("unsupported", "the connection closed before the secure channel was established");
    }
    emitClose(event);
  };

  return socket;
}
