// How the mobile transports wrap a raw socket in the Kanna secure channel,
// and how the outcome is described to the person.

import {
  createSealedSocket,
  decodeKey,
  PROTOCOL_VERSION,
  type Keypair,
  type SealedSocketRefusal,
  type SealedWebSocketLike,
} from "@kanna/secure-channel";
import type { RandomBytes } from "./randomBytes";

/** Everything needed to open a sealed session to one desktop. */
export interface SecureChannelPeer {
  desktopId: string;
  /** The desktop key this phone pinned at pairing (unpadded base64url). */
  desktopPublicKey: string;
  identity: Keypair;
  /** This phone's device id for a paired session; omitted while pairing. */
  deviceId?: string;
  intent: "session" | "pairing";
  randomBytes: RandomBytes;
  onRefusal?(refusal: SealedSocketRefusal, detail: string): void;
  onEstablished?(sas: string): void;
}

/**
 * What the app shows for a desktop's connection security. `sealed` and
 * `legacy` describe how the phone talks to it; the refusals are why a sealed
 * attempt stopped, each with a distinct remedy.
 */
export type SecureChannelStatus =
  | { mode: "sealed" }
  | { mode: "legacy" }
  | { mode: "refused"; refusal: SealedSocketRefusal; detail: string };

export function secureChannelStatusLabel(status: SecureChannelStatus | null | undefined): string {
  if (!status) return "Connection security unknown";
  switch (status.mode) {
    case "sealed":
      return "End-to-end encrypted";
    case "legacy":
      return "Not end-to-end encrypted — re-pair to upgrade";
    case "refused":
      switch (status.refusal) {
        case "unsupported":
          return "Connection not verified — the desktop needs a newer Kanna";
        case "identity_mismatch":
          return "Desktop identity changed — remove and pair again";
        case "timeout":
          return "Connection not verified — the desktop did not answer";
        case "transport":
          return "Connection interrupted — the encrypted session was tampered with or lost";
        case "peer_closed":
          return "The desktop closed the encrypted session";
        default:
          return "Connection not verified";
      }
  }
}

/** Wraps `inner` so that nothing but the handshake and sealed frames ever
 * cross it. Throws synchronously on a malformed pinned key. */
export function sealSocket(inner: SealedWebSocketLike, peer: SecureChannelPeer): SealedWebSocketLike {
  const desktopPublicKey = decodeKey(peer.desktopPublicKey);
  return createSealedSocket({
    inner,
    localIdentity: peer.identity,
    desktopId: peer.desktopId,
    desktopPublicKey,
    hello: {
      version: PROTOCOL_VERSION,
      intent: peer.intent,
      ...(peer.deviceId ? { deviceId: peer.deviceId } : {}),
      capabilities: ["ksp"],
    },
    randomBytes: peer.randomBytes,
    onRefusal: peer.onRefusal,
    onEstablished: (_hello, channel) => peer.onEstablished?.(channel.sas),
  });
}
