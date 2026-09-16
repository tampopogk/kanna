export interface MachinePairingPayload {
  desktopId: string;
  code: string;
  /** `KANNA2` payloads carry the desktop's secure-channel public key
   * (unpadded base64url) and a QR-only secret. Scanning the QR is what
   * anchors the key: it came from the desktop's screen, not the network. */
  channelPublicKey?: string;
  qrSecret?: string;
}

export type PairingPayloadFailure = "invalid" | "unsupported-version";

export class PairingPayloadError extends Error {
  constructor(
    public readonly reason: PairingPayloadFailure,
    message: string
  ) {
    super(message);
    this.name = "PairingPayloadError";
  }
}

export function normalizePairingCode(value: string): string {
  return value.replace(/[\s-]/g, "").toUpperCase();
}

export function parseMachinePairingPayload(raw: string): MachinePairingPayload {
  const trimmed = raw.trim();
  if (trimmed.startsWith("KANNA1:")) {
    return parseCompactPayload(trimmed);
  }
  if (trimmed.startsWith("KANNA2:")) {
    return parseKeyedPayload(trimmed);
  }
  if (/^KANNA\d+:/.test(trimmed)) {
    throw new PairingPayloadError(
      "unsupported-version",
      "This pairing code was made by an incompatible version of Kanna."
    );
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    throw new PairingPayloadError("invalid", "This is not a Kanna machine pairing code.");
  }

  if (!parsed || typeof parsed !== "object") {
    throw new PairingPayloadError("invalid", "This is not a Kanna machine pairing code.");
  }

  const candidate = parsed as Record<string, unknown>;
  if (candidate.type !== "kanna.machine-pairing") {
    throw new PairingPayloadError("invalid", "This is not a Kanna machine pairing code.");
  }
  if (candidate.version !== 1) {
    throw new PairingPayloadError(
      "unsupported-version",
      "This pairing code was made by an incompatible version of Kanna."
    );
  }

  const desktopId = typeof candidate.desktopId === "string"
    ? candidate.desktopId.trim()
    : "";
  const code = typeof candidate.code === "string"
    ? normalizePairingCode(candidate.code)
    : "";
  if (!desktopId || !/^[0-9A-F]{6}$/.test(code)) {
    throw new PairingPayloadError("invalid", "This Kanna machine pairing code is incomplete.");
  }

  return { desktopId, code };
}

const BASE32_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/** RFC 4648 base32 without padding, the QR-alphanumeric-safe encoding the
 * desktop uses for its key and QR secret. */
export function decodeBase32(text: string): Uint8Array {
  const out: number[] = [];
  let buffer = 0;
  let bits = 0;
  for (const character of text.toUpperCase()) {
    const value = BASE32_ALPHABET.indexOf(character);
    if (value < 0) throw new Error("base32: invalid character");
    buffer = (buffer << 5) | value;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      out.push((buffer >> bits) & 0xff);
    }
  }
  return Uint8Array.from(out);
}

function base64UrlOf(bytes: Uint8Array): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  let out = "";
  let index = 0;
  for (; index + 2 < bytes.length; index += 3) {
    const triple = (bytes[index] << 16) | (bytes[index + 1] << 8) | bytes[index + 2];
    out += alphabet[(triple >> 18) & 63] + alphabet[(triple >> 12) & 63] + alphabet[(triple >> 6) & 63] + alphabet[triple & 63];
  }
  const remaining = bytes.length - index;
  if (remaining === 1) {
    const value = bytes[index] << 16;
    out += alphabet[(value >> 18) & 63] + alphabet[(value >> 12) & 63];
  } else if (remaining === 2) {
    const value = (bytes[index] << 16) | (bytes[index + 1] << 8);
    out += alphabet[(value >> 18) & 63] + alphabet[(value >> 12) & 63] + alphabet[(value >> 6) & 63];
  }
  return out;
}

function parseKeyedPayload(raw: string): MachinePairingPayload {
  const fields = raw.slice("KANNA2:".length).split(":");
  if (fields.length !== 4) {
    throw new PairingPayloadError("invalid", "This Kanna machine pairing code is incomplete.");
  }
  const desktopId = fields[0]?.trim() ?? "";
  const code = normalizePairingCode(fields[1] ?? "");
  const encodedKey = fields[2]?.trim() ?? "";
  const qrSecret = (fields[3] ?? "").trim().toUpperCase();
  if (!desktopId || !/^[0-9A-F]{6}$/.test(code) || !/^[A-Z2-7]{52}$/.test(encodedKey) || !/^[A-Z2-7]{26}$/.test(qrSecret)) {
    throw new PairingPayloadError("invalid", "This Kanna machine pairing code is incomplete.");
  }
  let keyBytes: Uint8Array;
  try {
    keyBytes = decodeBase32(encodedKey);
  } catch {
    throw new PairingPayloadError("invalid", "This Kanna machine pairing code is incomplete.");
  }
  if (keyBytes.length !== 32) {
    throw new PairingPayloadError("invalid", "This Kanna machine pairing code is incomplete.");
  }
  return { desktopId, code, channelPublicKey: base64UrlOf(keyBytes), qrSecret };
}

function parseCompactPayload(raw: string): MachinePairingPayload {
  const fields = raw.slice("KANNA1:".length).split(":");
  if (fields.length !== 2) {
    throw new PairingPayloadError("invalid", "This Kanna machine pairing code is incomplete.");
  }

  const desktopId = fields[0]?.trim() ?? "";
  const code = normalizePairingCode(fields[1] ?? "");
  if (!desktopId || !/^[0-9A-F]{6}$/.test(code)) {
    throw new PairingPayloadError("invalid", "This Kanna machine pairing code is incomplete.");
  }
  return { desktopId, code };
}
