// The one place the mobile app draws cryptographic randomness.
//
// Hermes ships no `crypto.getRandomValues`; `expo-crypto` provides one backed
// by the platform CSPRNG (SecRandomCopyBytes / SecureRandom). Nothing here
// ever falls back to `Math.random`: an identity or a handshake ephemeral made
// from a predictable source is worse than no channel at all, so a runtime
// without a CSPRNG fails closed and says so.

export type RandomBytes = (length: number) => Uint8Array;

export class RandomnessUnavailableError extends Error {
  constructor(detail: string) {
    super(`secure randomness is unavailable on this runtime: ${detail}`);
    this.name = "RandomnessUnavailableError";
  }
}

type GetRandomValues = <T extends Uint8Array>(array: T) => T;

function globalGetRandomValues(): GetRandomValues | null {
  const cryptoObject = (globalThis as { crypto?: { getRandomValues?: unknown } }).crypto;
  const candidate = cryptoObject?.getRandomValues;
  if (typeof candidate !== "function") return null;
  return candidate.bind(cryptoObject) as GetRandomValues;
}

function expoGetRandomValues(): GetRandomValues | null {
  try {
    // Loaded lazily so the module can be imported by tests and tooling that
    // run in plain Node, where the native Expo module does not exist.
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const expoCrypto = require("expo-crypto") as { getRandomValues?: unknown };
    const candidate = expoCrypto.getRandomValues;
    if (typeof candidate !== "function") return null;
    return candidate as GetRandomValues;
  } catch {
    return null;
  }
}

let expoSource: GetRandomValues | null | undefined;

function resolveSource(): GetRandomValues {
  // The native module is resolved once; the global is looked up per call so
  // a runtime that installs or removes it (and tests that stub it) is seen.
  if (expoSource === undefined) expoSource = expoGetRandomValues();
  const source = expoSource ?? globalGetRandomValues();
  if (!source) {
    throw new RandomnessUnavailableError("neither expo-crypto nor crypto.getRandomValues is present");
  }
  return source;
}

/** Platform CSPRNG bytes, or a thrown `RandomnessUnavailableError`. */
export const nativeRandomBytes: RandomBytes = (length) => {
  if (!Number.isInteger(length) || length <= 0 || length > 65_536) {
    throw new RandomnessUnavailableError(`invalid random length ${length}`);
  }
  const source = resolveSource();
  const out = source(new Uint8Array(length));
  if (!(out instanceof Uint8Array) || out.length !== length) {
    throw new RandomnessUnavailableError("the random source returned the wrong shape");
  }
  // A CSPRNG returning all zeros for 16+ bytes is astronomically unlikely;
  // a broken shim returning an untouched buffer is not.
  if (length >= 16 && out.every((byte) => byte === 0)) {
    throw new RandomnessUnavailableError("the random source returned zeros");
  }
  return out;
};

/** Lower-case hex of `byteLength` random bytes. */
export function randomHex(byteLength: number, randomBytes: RandomBytes = nativeRandomBytes): string {
  return Array.from(randomBytes(byteLength), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** Which source `nativeRandomBytes` resolved to; for diagnostics only. */
export function describeRandomSource(): "expo-crypto" | "global-crypto" | "none" {
  if (expoGetRandomValues()) return "expo-crypto";
  if (globalGetRandomValues()) return "global-crypto";
  return "none";
}
