import { describe, expect, it } from "vitest";
import {
  normalizePairingCode,
  parseMachinePairingPayload
} from "./pairingPayload";

describe("parseMachinePairingPayload", () => {
  it("accepts the compact version-one desktop identity and code", () => {
    expect(parseMachinePairingPayload(
      "  KANNA1:DESKTOP-21B320E8-A5AD-4FAE-9D87-1DB14090F0A9:ABC123\n"
    )).toEqual({
      desktopId: "DESKTOP-21B320E8-A5AD-4FAE-9D87-1DB14090F0A9",
      code: "ABC123"
    });
  });

  it("keeps accepting legacy JSON payloads from older desktops", () => {
    expect(parseMachinePairingPayload(JSON.stringify({
      type: "kanna.machine-pairing",
      version: 1,
      desktopId: "desktop-1",
      code: "abc123"
    }))).toEqual({ desktopId: "desktop-1", code: "ABC123" });
  });

  it.each([
    ["not-json", "invalid"],
    ["KANNA1:DESKTOP-1", "invalid"],
    ["KANNA1:DESKTOP-1:ABC123:EXTRA", "invalid"],
    ["KANNA2:DESKTOP-1:ABC123", "invalid"],
    ["KANNA2:DESKTOP-1:ABC123:NOTAKEY:NOTASECRET", "invalid"],
    ["KANNA3:DESKTOP-1:ABC123", "unsupported-version"],
    [JSON.stringify({ type: "other", version: 1 }), "invalid"],
    [JSON.stringify({ type: "kanna.machine-pairing", version: 2 }), "unsupported-version"]
  ])("rejects %s", (raw, reason) => {
    expect(() => parseMachinePairingPayload(raw)).toThrowError(
      expect.objectContaining({ reason })
    );
  });

  it("rejects missing identities and malformed codes", () => {
    expect(() => parseMachinePairingPayload(JSON.stringify({
      type: "kanna.machine-pairing",
      version: 1,
      desktopId: " ",
      code: "ABC123"
    }))).toThrowError(expect.objectContaining({ reason: "invalid" }));
    expect(() => parseMachinePairingPayload(JSON.stringify({
      type: "kanna.machine-pairing",
      version: 1,
      desktopId: "desktop-1",
      code: "too-long"
    }))).toThrowError(expect.objectContaining({ reason: "invalid" }));
  });
});

describe("normalizePairingCode", () => {
  it("removes spaces and hyphens and uppercases", () => {
    expect(normalizePairingCode("ab-c 123")).toBe("ABC123");
  });

  it("parses a KANNA2 payload into the pinned desktop key and the QR-only secret", () => {
    // 32 key bytes and 16 secret bytes in RFC 4648 base32 (no padding), as
    // the desktop emits them.
    const keyBytes = Uint8Array.from({ length: 32 }, (_, index) => index * 5 + 3);
    const secretBytes = Uint8Array.from({ length: 16 }, (_, index) => 255 - index);
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    const base32 = (bytes: Uint8Array) => {
      let out = "";
      let buffer = 0;
      let bits = 0;
      for (const byte of bytes) {
        buffer = (buffer << 8) | byte;
        bits += 8;
        while (bits >= 5) {
          bits -= 5;
          out += alphabet[(buffer >> bits) & 31];
        }
      }
      if (bits > 0) out += alphabet[(buffer << (5 - bits)) & 31];
      return out;
    };
    const base64url = Buffer.from(keyBytes).toString("base64url");
    const payload = parseMachinePairingPayload(
      `KANNA2:DESKTOP-1:abc123:${base32(keyBytes)}:${base32(secretBytes)}`
    );
    expect(payload).toEqual({
      desktopId: "DESKTOP-1",
      code: "ABC123",
      channelPublicKey: base64url,
      qrSecret: base32(secretBytes)
    });
    expect(base32(keyBytes)).toHaveLength(52);
    expect(base32(secretBytes)).toHaveLength(26);
  });
});
