import type {
  MobileServerStatus,
  PairingClaimRequest,
  PairingClaimResponse
} from "../api/types";
import type { BonjourBrowser, BonjourService } from "../discovery/bonjour";
import type { FetchLike } from "../transports/lanTransport";
import type { TrustedDesktopRecord } from "../../state/sessionPersistence";
import {
  normalizePairingCode,
  parseMachinePairingPayload,
  type MachinePairingPayload
} from "./pairingPayload";
import {
  StreamClient,
  SECURE_CHANNEL_REFUSED_CLOSE_CODE,
  type WebSocketLike as StreamWebSocketLike
} from "@kanna/stream-client";
import type { Keypair, SealedWebSocketLike } from "@kanna/secure-channel";
import type { RandomBytes } from "../security/randomBytes";
import { sealSocket } from "../security/secureChannelPeer";

export type MachinePairingFailure =
  | "invalid-code"
  | "expired"
  | "rate-limited"
  | "not-found"
  | "multiple-matches"
  | "identity-mismatch"
  | "unreachable"
  /** The person rejected the pairing on the desktop. */
  | "confirmation-rejected"
  /** The desktop's confirmation window closed before anyone answered. */
  | "confirmation-expired"
  /** The desktop advertised end-to-end encryption but could not be verified. */
  | "not-verified"
  /** This phone cannot create a secure identity (no keystore / no CSPRNG). */
  | "secure-identity-unavailable";

export class MachinePairingError extends Error {
  constructor(
    public readonly reason: MachinePairingFailure,
    message: string
  ) {
    super(message);
    this.name = "MachinePairingError";
  }
}

export interface MobileDeviceIdentity {
  deviceId: string;
  deviceName: string;
}

/** Progress a pairing reports while it waits on the person. */
export interface MachinePairingHooks {
  /**
   * A typed-code pairing needs the person to compare this short
   * authentication string with the one the desktop shows, and confirm it
   * there. Called once, before the claim starts waiting.
   */
  onConfirmationRequired?(sas: string): void;
}

export interface MachinePairingService {
  claimPayload(rawPayload: string, hooks?: MachinePairingHooks): Promise<TrustedDesktopRecord>;
  claimCode(code: string, hooks?: MachinePairingHooks): Promise<TrustedDesktopRecord>;
}

/** What the secure-channel pairing paths need from the app. Absent, every
 * pairing is a legacy plaintext claim (the pre-E2EE behaviour). */
export interface MachinePairingSecureChannel {
  /** This phone's identity, once the secure key store produced it. `null`
   * means it could not be created; a desktop that requires the secure
   * channel then cannot be paired, and the error says why. */
  getIdentity(): Keypair | null;
  randomBytes: RandomBytes;
  createLanSocket(url: string): SealedWebSocketLike;
  /** A relay tunnel to the desktop, for pairing from a QR when the phone is
   * not on the desktop's network. `null` when no relay session is
   * available (signed out, offline). */
  createRelayTunnelSocket?(desktopId: string): SealedWebSocketLike | null;
}

export function createMachinePairingService(input: {
  bonjourBrowser: BonjourBrowser;
  fetchImpl: FetchLike;
  getDeviceIdentity(): MobileDeviceIdentity;
  secureChannel?: MachinePairingSecureChannel;
  claimTimeoutMs?: number;
  confirmationTimeoutMs?: number;
  now?: () => Date;
}): MachinePairingService {
  const now = input.now ?? (() => new Date());
  const claimTimeoutMs = input.claimTimeoutMs ?? 5_000;
  const confirmationTimeoutMs = input.confirmationTimeoutMs ?? 180_000;

  async function claimCandidates(
    payload: MachinePairingPayload,
    candidates: readonly BonjourService[],
    claimMode: "payload" | "code",
    hooks: MachinePairingHooks
  ): Promise<TrustedDesktopRecord> {
    if (candidates.length === 0) {
      throw pairingError("not-found");
    }

    const settled = await Promise.allSettled(
      candidates.map((candidate) => claimCandidate({
        candidate,
        payload,
        deviceIdentity: input.getDeviceIdentity(),
        fetchImpl: input.fetchImpl,
        secureChannel: input.secureChannel,
        timeoutMs: claimTimeoutMs,
        confirmationTimeoutMs,
        hooks,
        now
      }))
    );
    const successes = settled.flatMap((result) =>
      result.status === "fulfilled" ? [result.value] : []
    );

    if (successes.length === 1) {
      return successes[0];
    }
    if (successes.length > 1) {
      throw pairingError("multiple-matches");
    }

    const failures = settled.flatMap((result) =>
      result.status === "rejected" && result.reason instanceof MachinePairingError
        ? [result.reason.reason]
        : ["unreachable" as const]
    );
    for (const decisive of [
      "rate-limited",
      "expired",
      "identity-mismatch",
      "confirmation-rejected",
      "confirmation-expired",
      "not-verified",
      "secure-identity-unavailable"
    ] as const) {
      if (failures.includes(decisive)) throw pairingError(decisive);
    }
    if (failures.every((failure) => failure === "unreachable")) {
      throw pairingError("unreachable");
    }
    if (claimMode === "payload" && failures.includes("unreachable")) {
      throw pairingError("unreachable");
    }
    throw pairingError("not-found");
  }

  return {
    async claimPayload(rawPayload, hooks = {}) {
      const payload = parseMachinePairingPayload(rawPayload);
      let refreshFailed = false;
      try {
        // A scan usually beats discovery to the punch, so wait for the desktop
        // the QR names before deciding nothing advertised it.
        await input.bonjourBrowser.refresh?.({ desktopId: payload.desktopId });
      } catch {
        refreshFailed = true;
      }
      const candidates = input.bonjourBrowser.getServices().filter(
        (service) => desktopIdsEqual(service.txt.desktopId, payload.desktopId)
      );
      if (candidates.length === 0 && payload.channelPublicKey) {
        // Not on the desktop's network. A key-bearing QR still anchors the
        // desktop, so the relay tunnel is a fine transport for the sealed
        // claim: the relay carries only ciphertext.
        const relaySocket = input.secureChannel?.createRelayTunnelSocket?.(payload.desktopId) ?? null;
        if (relaySocket && input.secureChannel) {
          return claimSealed({
            socket: relaySocket,
            desktopId: payload.desktopId,
            desktopPublicKey: payload.channelPublicKey,
            payload,
            deviceIdentity: input.getDeviceIdentity(),
            secureChannel: input.secureChannel,
            confirmationTimeoutMs,
            hooks,
            lanEndpoints: [],
            now
          });
        }
      }
      if (candidates.length === 0 && refreshFailed) {
        throw pairingError("unreachable");
      }
      return claimCandidates(payload, candidates, "payload", hooks);
    },

    async claimCode(rawCode, hooks = {}) {
      const code = normalizePairingCode(rawCode);
      if (!/^[0-9A-F]{6}$/.test(code)) {
        throw pairingError("invalid-code");
      }
      let refreshFailed = false;
      try {
        // A typed code names no desktop, so any advertised machine will do.
        await input.bonjourBrowser.refresh?.({});
      } catch {
        refreshFailed = true;
      }
      const candidates = input.bonjourBrowser.getServices().filter(
        (service) => typeof service.txt.desktopId === "string" && service.txt.desktopId.trim()
      );
      if (candidates.length === 0 && refreshFailed) {
        throw pairingError("unreachable");
      }
      // A typed code names no desktop id; each candidate is tried under its
      // own advertised id.
      return claimCandidates({ desktopId: "", code }, candidates, "code", hooks);
    }
  };
}

async function claimCandidate(input: {
  candidate: BonjourService;
  payload: MachinePairingPayload;
  deviceIdentity: MobileDeviceIdentity;
  fetchImpl: FetchLike;
  secureChannel?: MachinePairingSecureChannel;
  timeoutMs: number;
  confirmationTimeoutMs: number;
  hooks: MachinePairingHooks;
  now(): Date;
}): Promise<TrustedDesktopRecord> {
  const desktopId = input.candidate.txt.desktopId;
  const baseUrl = `http://${input.candidate.host}:${input.candidate.port}`;
  const status = await readStatus(baseUrl, input.fetchImpl, input.timeoutMs);
  const advertisedKey = status?.channelPublicKey?.trim() || null;
  const scannedKey = input.payload.channelPublicKey ?? null;
  if (scannedKey && advertisedKey && scannedKey !== advertisedKey) {
    // The network says one key, the screen said another: somebody between
    // the two is answering for this desktop. Never proceed on either.
    throw pairingError("identity-mismatch");
  }
  const desktopPublicKey = scannedKey ?? advertisedKey;
  if (desktopPublicKey) {
    if (!input.secureChannel) {
      throw pairingError("not-verified");
    }
    const streamVersion = status?.kspStreamVersion === 2 ? 2 : 1;
    const socket = input.secureChannel.createLanSocket(buildKspWebSocketUrl(baseUrl, streamVersion));
    const lastSeenAt = input.now().toISOString();
    return claimSealed({
      socket,
      desktopId,
      desktopPublicKey,
      payload: input.payload,
      deviceIdentity: input.deviceIdentity,
      secureChannel: input.secureChannel,
      confirmationTimeoutMs: input.confirmationTimeoutMs,
      hooks: input.hooks,
      lanEndpoints: [{ baseUrl, lastSeenAt }],
      now: input.now
    });
  }
  if (scannedKey) {
    // A key-bearing QR from a desktop whose status carries no key: the
    // status was rewritten, or the QR was not this desktop's. Refuse.
    throw pairingError("identity-mismatch");
  }
  return claimLegacy({ ...input, desktopId, baseUrl });
}

async function readStatus(
  baseUrl: string,
  fetchImpl: FetchLike,
  timeoutMs: number
): Promise<MobileServerStatus | null> {
  const abortController = new AbortController();
  let timeout: ReturnType<typeof setTimeout> | null = null;
  try {
    const response = await Promise.race([
      fetchImpl(`${baseUrl}/v1/status`, { signal: abortController.signal }),
      new Promise<never>((_, reject) => {
        timeout = setTimeout(() => {
          abortController.abort();
          reject(pairingError("unreachable"));
        }, timeoutMs);
      })
    ]);
    if (!response.ok) return null;
    const body = (await response.json()) as MobileServerStatus;
    return body && typeof body === "object" ? body : null;
  } catch (error) {
    if (error instanceof MachinePairingError) throw error;
    throw pairingError("unreachable");
  } finally {
    if (timeout !== null) clearTimeout(timeout);
  }
}

/**
 * The secure-channel claim: handshake against the anchored desktop key,
 * then the claim as a sealed request. With a QR-scanned key (and its QR
 * secret) the desktop registers the phone at once; with a typed code the
 * desktop parks the pairing until the person confirms the short
 * authentication string there, and this side polls for that decision.
 */
async function claimSealed(input: {
  socket: SealedWebSocketLike;
  desktopId: string;
  desktopPublicKey: string;
  payload: MachinePairingPayload;
  deviceIdentity: MobileDeviceIdentity;
  secureChannel: MachinePairingSecureChannel;
  confirmationTimeoutMs: number;
  hooks: MachinePairingHooks;
  lanEndpoints: TrustedDesktopRecord["lanEndpoints"];
  now(): Date;
}): Promise<TrustedDesktopRecord> {
  const identity = input.secureChannel.getIdentity();
  if (!identity) {
    throw pairingError("secure-identity-unavailable");
  }
  let refusal: string | null = null;
  let sas: string | null = null;
  let sealed: SealedWebSocketLike;
  try {
    sealed = sealSocket(input.socket, {
      desktopId: input.desktopId,
      desktopPublicKey: input.desktopPublicKey,
      identity,
      intent: "pairing",
      randomBytes: input.secureChannel.randomBytes,
      onRefusal: (kind, detail) => {
        refusal = `${kind}: ${detail}`;
      },
      onEstablished: (established) => {
        sas = established;
      }
    });
  } catch {
    throw pairingError("identity-mismatch");
  }
  // Exactly one handshake per claim. A reconnect would be a *new* session
  // with a new SAS, and the desktop keys its pending confirmation to the
  // session that claimed; a silently reconnected socket must not exist.
  let handedOut = false;
  const client = new StreamClient({
    url: "sealed://pairing",
    webSocketFactory: () => {
      if (handedOut) return deadSocket();
      handedOut = true;
      return sealed as unknown as StreamWebSocketLike;
    },
    reconnectDelaysMs: [250]
  });
  try {
    const request: PairingClaimRequest = {
      code: input.payload.code,
      deviceId: input.deviceIdentity.deviceId,
      deviceName: input.deviceIdentity.deviceName,
      ...(input.payload.qrSecret ? { qrSecret: input.payload.qrSecret } : {})
    };
    let response: { status: number; body: unknown };
    try {
      response = await client.request("POST", "/v1/pairing/sessions/claim", request);
    } catch (error) {
      throw sealedFailure(error, refusal);
    }
    if (response.status === 202) {
      if (!sas) throw pairingError("not-verified");
      input.hooks.onConfirmationRequired?.(sas);
      const deadline = Date.now() + input.confirmationTimeoutMs;
      while (Date.now() < deadline) {
        let poll: { status: number; body: unknown };
        try {
          poll = await client.request("GET", "/v1/pairing/confirmation");
        } catch (error) {
          throw sealedFailure(error, refusal);
        }
        if (poll.status === 200) {
          response = poll;
          break;
        }
        if (poll.status === 202) continue;
        if (poll.status === 403) throw pairingError("confirmation-rejected");
        if (poll.status === 410) throw pairingError("confirmation-expired");
        throw pairingError("unreachable");
      }
      if (response.status !== 200) throw pairingError("confirmation-expired");
    } else if (response.status !== 200) {
      throw claimStatusError(response.status);
    }
    const claim = response.body as PairingClaimResponse;
    if (
      !claim ||
      typeof claim.desktopId !== "string" ||
      typeof claim.desktopName !== "string" ||
      !desktopIdsEqual(claim.desktopId, input.desktopId) ||
      claim.secureChannel !== true
    ) {
      throw pairingError("identity-mismatch");
    }
    const lastSeenAt = input.now().toISOString();
    return {
      desktopId: claim.desktopId,
      displayName: claim.desktopName,
      lanEndpoints: input.lanEndpoints,
      lastSeenAt,
      ...(typeof claim.deviceSecret === "string" && claim.deviceSecret
        ? { deviceSecret: claim.deviceSecret }
        : {}),
      channelPublicKey: input.desktopPublicKey,
      ...pairingPushMaterial(claim, input.deviceIdentity.deviceId)
    };
  } finally {
    client.close();
  }
}

function sealedFailure(error: unknown, refusal: string | null): MachinePairingError {
  if (error instanceof MachinePairingError) return error;
  if (refusal) return pairingError("not-verified", refusal);
  return pairingError("unreachable");
}

function claimStatusError(status: number): MachinePairingError {
  if (status === 410) return pairingError("expired");
  if (status === 429) return pairingError("rate-limited");
  if (status === 400 || status === 409) return pairingError("not-found");
  return pairingError("unreachable");
}

/** A socket that closes at once with the secure-channel refusal code, so a
 * client that tries to reconnect stops instead of handshaking again. */
function deadSocket(): StreamWebSocketLike {
  const socket: StreamWebSocketLike = {
    onopen: null,
    onmessage: null,
    onclose: null,
    onerror: null,
    send() {},
    close() {}
  };
  setTimeout(() => {
    socket.onclose?.({ code: SECURE_CHANNEL_REFUSED_CLOSE_CODE, reason: "pairing sessions are single-use" });
  }, 0);
  return socket;
}

function buildKspWebSocketUrl(baseUrl: string, streamVersion: 1 | 2): string {
  const url = new URL(baseUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.pathname = `/v${streamVersion}/stream`;
  url.search = "";
  return url.toString();
}

/** The pre-E2EE claim: a plaintext POST answered with a bearer secret. Only
 * a desktop that advertises no secure-channel key gets this. */
async function claimLegacy(input: {
  desktopId: string;
  baseUrl: string;
  payload: MachinePairingPayload;
  deviceIdentity: MobileDeviceIdentity;
  fetchImpl: FetchLike;
  timeoutMs: number;
  now(): Date;
}): Promise<TrustedDesktopRecord> {
  const body: PairingClaimRequest = {
    code: input.payload.code,
    deviceId: input.deviceIdentity.deviceId,
    deviceName: input.deviceIdentity.deviceName
  };

  const abortController = new AbortController();
  let timeout: ReturnType<typeof setTimeout> | null = null;
  let response;
  try {
    response = await Promise.race([
      input.fetchImpl(`${input.baseUrl}/v1/pairing/sessions/claim`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
        signal: abortController.signal
      }),
      new Promise<never>((_, reject) => {
        timeout = setTimeout(() => {
          abortController.abort();
          reject(pairingError("unreachable"));
        }, input.timeoutMs);
      })
    ]);
  } catch (error) {
    if (error instanceof MachinePairingError) throw error;
    throw pairingError("unreachable");
  } finally {
    if (timeout !== null) clearTimeout(timeout);
  }

  if (!response.ok) {
    throw claimStatusError(response.status);
  }

  let claim: PairingClaimResponse;
  try {
    claim = await response.json() as PairingClaimResponse;
  } catch {
    throw pairingError("unreachable");
  }
  if (
    !claim ||
    typeof claim.desktopId !== "string" ||
    typeof claim.desktopName !== "string" ||
    !desktopIdsEqual(claim.desktopId, input.desktopId)
  ) {
    throw pairingError("identity-mismatch");
  }

  const lastSeenAt = input.now().toISOString();
  const pushMaterial = pairingPushMaterial(claim, input.deviceIdentity.deviceId);
  return {
    desktopId: claim.desktopId,
    displayName: claim.desktopName,
    lanEndpoints: [{ baseUrl: input.baseUrl, lastSeenAt }],
    lastSeenAt,
    ...(typeof claim.deviceSecret === "string" && claim.deviceSecret
      ? { deviceSecret: claim.deviceSecret }
      : {}),
    ...pushMaterial
  };
}

function pairingPushMaterial(
  claim: PairingClaimResponse,
  deviceId: string
): Pick<TrustedDesktopRecord, "desktopPushIdentity" | "pushPairingCert"> {
  const identity = claim.desktopPushIdentity;
  const certificate = claim.pushPairingCert;
  if (
    !identity ||
    !certificate ||
    typeof identity.publicKey !== "string" ||
    !identity.publicKey ||
    typeof identity.relayUrl !== "string" ||
    typeof identity.environment !== "string" ||
    !identity.environment ||
    certificate.deviceId !== deviceId ||
    !Number.isSafeInteger(certificate.issuedAt) ||
    !Number.isSafeInteger(certificate.expiresAt) ||
    certificate.expiresAt <= certificate.issuedAt ||
    typeof certificate.signature !== "string" ||
    !certificate.signature
  ) {
    return {};
  }
  return {
    desktopPushIdentity: identity,
    pushPairingCert: certificate
  };
}

function desktopIdsEqual(left: string, right: string): boolean {
  return left.toUpperCase() === right.toUpperCase();
}

function pairingError(reason: MachinePairingFailure, detail?: string): MachinePairingError {
  const messages: Record<MachinePairingFailure, string> = {
    "invalid-code": "Enter the six-character pairing code shown on the desktop.",
    expired: "That pairing session expired. Start a new one on the desktop.",
    "rate-limited": "Too many attempts. Start a new pairing session on the desktop.",
    "not-found": "No machine on this network accepted that pairing code.",
    "multiple-matches": "More than one machine accepted that code. Start a new pairing session.",
    "identity-mismatch": "The machine identity did not match its network advertisement.",
    unreachable: "The machine could not be reached. Check that both apps are on the same network.",
    "confirmation-rejected": "The pairing was rejected on the desktop.",
    "confirmation-expired": "The desktop stopped waiting for confirmation. Start a new pairing session.",
    "not-verified": "The desktop's encrypted connection could not be verified. Update Kanna on the desktop and try again.",
    "secure-identity-unavailable": "This phone cannot create a secure identity. Check that the device has a passcode set."
  };
  return new MachinePairingError(reason, detail ? `${messages[reason]} (${detail})` : messages[reason]);
}
