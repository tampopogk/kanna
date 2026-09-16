import { webcrypto } from "node:crypto";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { utf8Decode, utf8Encode } from "./bytes";
import {
  PROTOCOL_VERSION,
  generateKeypair,
  readInitiatorHello,
  type Channel,
  type Keypair,
} from "./channel";
import {
  SECURE_CHANNEL_CLOSE_CODE,
  createSealedSocket,
  type SealedSocketRefusal,
  type SealedWebSocketLike,
} from "./sealedSocket";

const randomBytes = (length: number) => webcrypto.getRandomValues(new Uint8Array(length));

class FakeSocket implements SealedWebSocketLike {
  sent: string[] = [];
  closed = false;
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.closed = true;
  }
  open() {
    this.onopen?.({});
  }
  deliver(data: string) {
    this.onmessage?.({ data });
  }
}

/** A desktop-side responder driven by hand in tests. */
function fakeDesktop(identity: Keypair, desktopId = "DESKTOP-1") {
  return {
    answer(message1: string): { message2: string; channel: Channel } {
      const pending = readInitiatorHello(identity, desktopId, message1, { randomBytes });
      return pending.accept({ version: PROTOCOL_VERSION, desktopId });
    },
  };
}

describe("sealed socket", () => {
  let phone: Keypair;
  let desktop: Keypair;
  let inner: FakeSocket;
  let refusals: { refusal: SealedSocketRefusal; detail: string }[];
  let outbound: string[];
  let closes: unknown[];
  let opened: number;

  beforeEach(() => {
    vi.useFakeTimers();
    phone = generateKeypair(randomBytes);
    desktop = generateKeypair(randomBytes);
    inner = new FakeSocket();
    refusals = [];
    outbound = [];
    closes = [];
    opened = 0;
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function create(desktopPublicKey = desktop.publicKey, desktopId = "DESKTOP-1") {
    const socket = createSealedSocket({
      inner,
      localIdentity: phone,
      desktopId,
      desktopPublicKey,
      hello: { version: PROTOCOL_VERSION, intent: "session", deviceId: "phone-1" },
      randomBytes,
      onRefusal: (refusal, detail) => refusals.push({ refusal, detail }),
    });
    socket.onopen = () => {
      opened += 1;
    };
    socket.onmessage = (event) => outbound.push(String(event.data));
    socket.onclose = (event) => closes.push(event);
    return socket;
  }

  it("opens only after the desktop authenticates, then seals both directions", () => {
    const socket = create();
    inner.open();
    expect(inner.sent).toHaveLength(1);
    expect(inner.sent[0].startsWith("ksc1:")).toBe(true);
    expect(opened).toBe(0);
    const { message2, channel } = fakeDesktop(desktop).answer(inner.sent[0]);
    inner.deliver(message2);
    expect(opened).toBe(1);
    expect(socket.sas).toBe(channel.sas);

    socket.send('{"type":"auth","capabilities":[]}');
    expect(inner.sent[1]).not.toContain("auth");
    expect(channel.receiver.open(inner.sent[1])).toEqual([
      { kind: "message", data: utf8Encode('{"type":"auth","capabilities":[]}') },
    ]);
    inner.deliver(channel.sender.sealText('{"type":"auth_ok"}'));
    expect(outbound).toEqual(['{"type":"auth_ok"}']);

    socket.close();
    const [closeFrame] = inner.sent.slice(-1);
    expect(channel.receiver.open(closeFrame)).toEqual([{ kind: "closed", reason: "client closed" }]);
    expect(inner.closed).toBe(true);
  });

  it("refuses to send anything before the handshake completes", () => {
    const socket = create();
    inner.open();
    expect(() => socket.send('{"type":"auth"}')).toThrow(/not established/);
    expect(inner.sent).toHaveLength(1);
    expect(inner.sent[0].startsWith("ksc1:")).toBe(true);
  });

  it("treats a plaintext answer (old desktop or stripped handshake) as unsupported and sends no plaintext", () => {
    create();
    inner.open();
    inner.deliver('{"type":"error","code":"bad_frame","message":"unparseable frame"}');
    expect(refusals).toEqual([
      { refusal: "unsupported", detail: expect.stringContaining("did not answer") },
    ]);
    expect(outbound).toEqual([
      JSON.stringify({
        type: "error",
        code: "secure_channel_unsupported",
        message: "the desktop did not answer with a secure channel handshake",
      }),
    ]);
    expect(closes).toEqual([{ code: SECURE_CHANNEL_CLOSE_CODE, reason: expect.stringContaining("unsupported") }]);
    expect(inner.closed).toBe(true);
    expect(inner.sent.every((frame) => frame.startsWith("ksc1:"))).toBe(true);
  });

  it("reads the desktop's own plaintext refusal as an identity mismatch, never as acceptance", () => {
    create();
    inner.open();
    inner.deliver(JSON.stringify({ type: "error", code: "secure_channel_refused", message: "handshake refused" }));
    expect(refusals.map((entry) => entry.refusal)).toEqual(["identity_mismatch"]);
    expect(opened).toBe(0);
    expect(inner.closed).toBe(true);
    // A forged "all good" in plaintext is still just plaintext.
    const again = new FakeSocket();
    inner = again;
    refusals = [];
    create();
    again.open();
    again.deliver(JSON.stringify({ type: "auth_ok" }));
    expect(refusals.map((entry) => entry.refusal)).toEqual(["unsupported"]);
    expect(opened).toBe(0);
  });

  it("refuses a responder that does not hold the pinned desktop key", () => {
    create();
    inner.open();
    const impostor = generateKeypair(randomBytes);
    // The impostor cannot read message 1, so it answers with a handshake of
    // its own making (a message-1-shaped frame against the phone's key).
    const forged = readInitiatorHello;
    void forged;
    const bogus = fakeDesktop(impostor);
    expect(() => bogus.answer(inner.sent[0])).toThrow();
    // Deliver a frame that is KSC-shaped but not the genuine message 2.
    inner.deliver(inner.sent[0]);
    expect(refusals.map((entry) => entry.refusal)).toEqual(["identity_mismatch"]);
    expect(closes).toHaveLength(1);
    expect(opened).toBe(0);
  });

  it("refuses when the desktop identifies itself as another machine", () => {
    create(desktop.publicKey, "DESKTOP-1");
    inner.open();
    // A desktop holding the right key but answering under a different id
    // cannot even complete the handshake: the prologue differs.
    expect(() => fakeDesktop(desktop, "DESKTOP-2").answer(inner.sent[0])).toThrow();
  });

  it("times out a handshake nobody answers", () => {
    create();
    inner.open();
    vi.advanceTimersByTime(10_001);
    expect(refusals.map((entry) => entry.refusal)).toEqual(["timeout"]);
    expect(inner.closed).toBe(true);
    expect(opened).toBe(0);
  });

  it("reports a pre-handshake close as unsupported rather than an ordinary drop", () => {
    create();
    inner.open();
    inner.onclose?.({ code: 1006 });
    expect(refusals.map((entry) => entry.refusal)).toEqual(["unsupported"]);
    expect(closes).toEqual([{ code: 1006 }]);
  });

  it("ends the session on a tampered transport frame and delivers nothing after it", () => {
    const socket = create();
    inner.open();
    const { message2, channel } = fakeDesktop(desktop).answer(inner.sent[0]);
    inner.deliver(message2);
    const genuine = channel.sender.sealText('{"type":"auth_ok"}');
    const tampered = genuine.slice(0, -4) + "AAAA";
    inner.deliver(tampered);
    expect(refusals.map((entry) => entry.refusal)).toEqual(["transport"]);
    expect(outbound.filter((frame) => frame.includes("auth_ok"))).toHaveLength(0);
    expect(inner.closed).toBe(true);
    expect(() => socket.send("x")).not.toThrow();
    inner.deliver(channel.sender.sealText('{"type":"late"}'));
    expect(outbound.filter((frame) => frame.includes("late"))).toHaveLength(0);
  });

  it("surfaces the peer's authenticated close distinctly from a network drop", () => {
    create();
    inner.open();
    const { message2, channel } = fakeDesktop(desktop).answer(inner.sent[0]);
    inner.deliver(message2);
    inner.deliver(channel.sender.sealClose("device revoked"));
    expect(refusals).toEqual([{ refusal: "peer_closed", detail: "device revoked" }]);
    expect(closes).toEqual([{ code: 1000, reason: "device revoked", authenticatedClose: true }]);
    expect(utf8Decode(utf8Encode("roundtrip"))).toBe("roundtrip");
  });
});
