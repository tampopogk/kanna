import { webcrypto } from "node:crypto";
import { describe, expect, it } from "vitest";
import { decodeBase64, encodeBase64, utf8Encode } from "./bytes";
import {
  MAX_CHUNK_PAYLOAD_LEN,
  PROTOCOL_VERSION,
  SecureChannelError,
  WIRE_PREFIX,
  decodeKey,
  encodeKey,
  generateKeypair,
  isWireFrame,
  readInitiatorHello,
  startInitiator,
  type Channel,
  type InitiatorHello,
} from "./channel";

const randomBytes = (length: number) => webcrypto.getRandomValues(new Uint8Array(length));
const options = { randomBytes };

const hello: InitiatorHello = {
  version: PROTOCOL_VERSION,
  intent: "session",
  deviceId: "phone-1",
  capabilities: ["ksp"],
};

function establish(maxMessageLen?: number): { phone: Channel; desktop: Channel } {
  const phone = generateKeypair(randomBytes);
  const desktop = generateKeypair(randomBytes);
  const pending = startInitiator(phone, desktop.publicKey, "DESKTOP-1", hello, { ...options, maxMessageLen });
  const responder = readInitiatorHello(desktop, "DESKTOP-1", pending.message1, { ...options, maxMessageLen });
  expect(responder.hello).toEqual(hello);
  expect(responder.remoteStatic).toEqual(phone.publicKey);
  const { message2, channel: desktopChannel } = responder.accept({
    version: PROTOCOL_VERSION,
    desktopId: "DESKTOP-1",
  });
  const { channel: phoneChannel, hello: responderHello } = pending.finish(message2);
  expect(responderHello.desktopId).toBe("DESKTOP-1");
  expect(phoneChannel.handshakeHash).toEqual(desktopChannel.handshakeHash);
  expect(phoneChannel.sas).toBe(desktopChannel.sas);
  expect(phoneChannel.sas).toMatch(/^\d{6}$/);
  return { phone: phoneChannel, desktop: desktopChannel };
}

describe("secure channel (TypeScript peers)", () => {
  it("round-trips empty, small, unicode and chunked messages both ways", () => {
    const { phone, desktop } = establish();
    const big = new Uint8Array(MAX_CHUNK_PAYLOAD_LEN * 3 + 17).fill(0xab);
    for (const payload of [utf8Encode('{"type":"auth"}'), new Uint8Array(0), utf8Encode("✓ unicode"), big]) {
      const wire = phone.sender.seal(payload);
      expect(isWireFrame(wire)).toBe(true);
      expect(wire).not.toContain("auth");
      expect(desktop.receiver.open(wire)).toEqual([{ kind: "message", data: payload }]);
    }
    const reply = desktop.sender.sealText('{"type":"auth_ok"}');
    expect(phone.receiver.open(reply)).toEqual([{ kind: "message", data: utf8Encode('{"type":"auth_ok"}') }]);
  });

  it("refuses a replayed frame and stays dead afterwards", () => {
    const { phone, desktop } = establish();
    const wire = phone.sender.sealText("first");
    desktop.receiver.open(wire);
    expect(() => desktop.receiver.open(wire)).toThrow(SecureChannelError);
    expect(desktop.receiver.isClosed()).toBe(true);
    expect(() => desktop.receiver.open(phone.sender.sealText("second"))).toThrow(/closed/);
  });

  it("refuses a reordered (dropped-then-continued) frame", () => {
    const { phone, desktop } = establish();
    phone.sender.sealText("first");
    const second = phone.sender.sealText("second");
    expect(() => desktop.receiver.open(second)).toThrow(SecureChannelError);
  });

  it("refuses tampered and truncated frames", () => {
    const { phone, desktop } = establish();
    const wire = phone.sender.sealText("payload");
    const bytes = decodeBase64(wire.slice(WIRE_PREFIX.length));
    bytes[bytes.length - 1] ^= 0x01;
    expect(() => desktop.receiver.open(WIRE_PREFIX + encodeBase64(bytes))).toThrow(SecureChannelError);

    const fresh = establish();
    const wire2 = fresh.phone.sender.sealText("payload");
    expect(() => fresh.desktop.receiver.open(wire2.slice(0, -8))).toThrow(SecureChannelError);
    expect(fresh.desktop.receiver.isClosed()).toBe(true);
  });

  it("refuses plaintext KSP where a sealed frame is expected", () => {
    const { phone } = establish();
    expect(() => phone.receiver.open('{"type":"auth_ok"}')).toThrow(/ksc1/);
  });

  it("fails the handshake against the wrong desktop key, wrong prologue, or an impostor", () => {
    const phone = generateKeypair(randomBytes);
    const desktop = generateKeypair(randomBytes);
    const impostor = generateKeypair(randomBytes);
    const pending = startInitiator(phone, desktop.publicKey, "DESKTOP-1", hello, options);
    expect(() => readInitiatorHello(impostor, "DESKTOP-1", pending.message1, options)).toThrow(SecureChannelError);
    expect(() => readInitiatorHello(desktop, "DESKTOP-2", pending.message1, options)).toThrow(SecureChannelError);
    // An impostor's own message 1 against the phone's key is not a message 2.
    const forged = startInitiator(impostor, phone.publicKey, "DESKTOP-1", hello, options);
    expect(() => pending.finish(forged.message1)).toThrow(SecureChannelError);
  });

  it("refuses an unsupported hello version after authentication", () => {
    const phone = generateKeypair(randomBytes);
    const desktop = generateKeypair(randomBytes);
    const pending = startInitiator(phone, desktop.publicKey, "D", { ...hello, version: 2 }, options);
    expect(() => readInitiatorHello(desktop, "D", pending.message1, options)).toThrow(/version/);
  });

  it("delivers an authenticated close exactly once and refuses later sends", () => {
    const { phone, desktop } = establish();
    const wire = phone.sender.sealClose("phone backgrounded");
    expect(desktop.receiver.open(wire)).toEqual([{ kind: "closed", reason: "phone backgrounded" }]);
    expect(desktop.receiver.isClosed()).toBe(true);
    expect(() => phone.sender.sealText("more")).toThrow(/closed/);
  });

  it("bounds reassembly", () => {
    const { phone, desktop } = establish(1024);
    const wire = phone.sender.seal(new Uint8Array(2048).fill(1));
    expect(() => desktop.receiver.open(wire)).toThrow(/exceeds 1024/);
  });

  it("encodes and decodes keys as unpadded base64url", () => {
    const key = generateKeypair(randomBytes).publicKey;
    const encoded = encodeKey(key);
    expect(encoded).not.toContain("=");
    expect(decodeKey(encoded)).toEqual(key);
    expect(() => decodeKey("not a key!")).toThrow(SecureChannelError);
    expect(() => decodeKey(encodeKey(key).slice(1))).toThrow(SecureChannelError);
  });
});
