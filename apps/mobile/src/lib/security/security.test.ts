import { afterEach, describe, expect, it, vi } from "vitest";
import { decodeBase64Url, encodeKey, keypairFromPrivate } from "@kanna/secure-channel";
import { CHANNEL_IDENTITY_KEY, loadOrCreateChannelIdentity } from "./channelIdentity";
import { describeRandomSource, nativeRandomBytes, randomHex, RandomnessUnavailableError } from "./randomBytes";
import { createMemorySecureKeyStore } from "./secureKeyStore";
import { secureChannelStatusLabel } from "./secureChannelPeer";

describe("randomBytes", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("draws from the platform CSPRNG and never from Math.random", () => {
    const bytes = nativeRandomBytes(32);
    expect(bytes).toHaveLength(32);
    expect(bytes.some((byte) => byte !== 0)).toBe(true);
    expect(randomHex(4)).toMatch(/^[0-9a-f]{8}$/);
    expect(["expo-crypto", "global-crypto"]).toContain(describeRandomSource());
  });

  it("fails closed when no CSPRNG is present", () => {
    vi.stubGlobal("crypto", undefined);
    expect(() => nativeRandomBytes(32)).toThrow(RandomnessUnavailableError);
  });

  it("fails closed when the source returns zeros or the wrong shape", () => {
    vi.stubGlobal("crypto", { getRandomValues: <T extends Uint8Array>(array: T) => array });
    expect(() => nativeRandomBytes(32)).toThrow(/zeros/);
    vi.stubGlobal("crypto", { getRandomValues: () => new Uint8Array(3) });
    expect(() => nativeRandomBytes(32)).toThrow(/wrong shape/);
  });
});

describe("channel identity", () => {
  it("creates an identity once and reloads the same keypair from the secure store", async () => {
    const store = createMemorySecureKeyStore();
    const first = await loadOrCreateChannelIdentity(store, nativeRandomBytes);
    const second = await loadOrCreateChannelIdentity(store, nativeRandomBytes);
    expect(second.publicKey).toBe(first.publicKey);
    expect(second.keypair.privateKey).toEqual(first.keypair.privateKey);
    const stored = await store.getItem(CHANNEL_IDENTITY_KEY);
    expect(stored).not.toBeNull();
    const rebuilt = keypairFromPrivate(decodeBase64Url(stored as string));
    expect(encodeKey(rebuilt.publicKey)).toBe(first.publicKey);
  });

  it("replaces a corrupt stored value instead of trusting it", async () => {
    const store = createMemorySecureKeyStore({ [CHANNEL_IDENTITY_KEY]: "not a key" });
    const identity = await loadOrCreateChannelIdentity(store, nativeRandomBytes);
    expect(identity.publicKey).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(await store.getItem(CHANNEL_IDENTITY_KEY)).not.toBe("not a key");
  });

  it("refuses key store keys outside the safe alphabet", async () => {
    const store = createMemorySecureKeyStore();
    await expect(store.setItem("bad key!", "x")).rejects.toThrow(/must match/);
  });
});

describe("secure channel status labels", () => {
  it("names each state and remedy distinctly", () => {
    expect(secureChannelStatusLabel({ mode: "sealed" })).toBe("End-to-end encrypted");
    expect(secureChannelStatusLabel({ mode: "legacy" })).toContain("re-pair");
    expect(secureChannelStatusLabel({ mode: "refused", refusal: "unsupported", detail: "" })).toContain("newer Kanna");
    expect(secureChannelStatusLabel({ mode: "refused", refusal: "identity_mismatch", detail: "" })).toContain("pair again");
    expect(secureChannelStatusLabel(null)).toBe("Connection security unknown");
  });
});
