// Byte helpers with no platform dependency: Hermes (React Native), Node and
// browsers all get the same code paths. `btoa`/`Buffer` are deliberately not
// used — their availability and binary-string semantics differ per runtime.

const STD_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

function buildLookup(alphabet: string): Int16Array {
  const lookup = new Int16Array(256).fill(-1);
  for (let index = 0; index < alphabet.length; index += 1) {
    lookup[alphabet.charCodeAt(index)] = index;
  }
  return lookup;
}

const STD_LOOKUP = buildLookup(STD_ALPHABET);
const URL_LOOKUP = buildLookup(URL_ALPHABET);

function encodeBase64With(bytes: Uint8Array, alphabet: string, pad: boolean): string {
  let out = "";
  let index = 0;
  for (; index + 2 < bytes.length; index += 3) {
    const triple = (bytes[index] << 16) | (bytes[index + 1] << 8) | bytes[index + 2];
    out +=
      alphabet[(triple >> 18) & 63] +
      alphabet[(triple >> 12) & 63] +
      alphabet[(triple >> 6) & 63] +
      alphabet[triple & 63];
  }
  const remaining = bytes.length - index;
  if (remaining === 1) {
    const value = bytes[index] << 16;
    out += alphabet[(value >> 18) & 63] + alphabet[(value >> 12) & 63];
    if (pad) out += "==";
  } else if (remaining === 2) {
    const value = (bytes[index] << 16) | (bytes[index + 1] << 8);
    out += alphabet[(value >> 18) & 63] + alphabet[(value >> 12) & 63] + alphabet[(value >> 6) & 63];
    if (pad) out += "=";
  }
  return out;
}

function decodeBase64With(text: string, lookup: Int16Array, requirePadding: boolean): Uint8Array {
  let end = text.length;
  while (end > 0 && text[end - 1] === "=") end -= 1;
  const padding = text.length - end;
  if (padding > 2) throw new Error("base64: invalid padding");
  if (requirePadding && (text.length % 4 !== 0)) throw new Error("base64: invalid length");
  if (!requirePadding && padding !== 0) throw new Error("base64url: unexpected padding");
  const remainder = end % 4;
  if (remainder === 1) throw new Error("base64: invalid length");
  const outLength = Math.floor((end * 3) / 4);
  const out = new Uint8Array(outLength);
  let outIndex = 0;
  let buffer = 0;
  let bits = 0;
  for (let index = 0; index < end; index += 1) {
    const value = lookup[text.charCodeAt(index)];
    if (value < 0) throw new Error("base64: invalid character");
    buffer = (buffer << 6) | value;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out[outIndex++] = (buffer >> bits) & 0xff;
    }
  }
  // Non-canonical trailing bits would let two encodings name one value.
  if ((buffer & ((1 << bits) - 1)) !== 0) throw new Error("base64: non-canonical encoding");
  return out;
}

export function encodeBase64(bytes: Uint8Array): string {
  return encodeBase64With(bytes, STD_ALPHABET, true);
}

export function decodeBase64(text: string): Uint8Array {
  return decodeBase64With(text, STD_LOOKUP, true);
}

export function encodeBase64Url(bytes: Uint8Array): string {
  return encodeBase64With(bytes, URL_ALPHABET, false);
}

export function decodeBase64Url(text: string): Uint8Array {
  return decodeBase64With(text, URL_LOOKUP, false);
}

export function hexToBytes(hex: string): Uint8Array {
  if (hex.length % 2 !== 0 || /[^0-9a-fA-F]/.test(hex)) throw new Error("hex: invalid");
  const out = new Uint8Array(hex.length / 2);
  for (let index = 0; index < out.length; index += 1) {
    out[index] = parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  }
  return out;
}

export function bytesToHex(bytes: Uint8Array): string {
  let out = "";
  for (const byte of bytes) out += byte.toString(16).padStart(2, "0");
  return out;
}

export function utf8Encode(text: string): Uint8Array {
  return new TextEncoder().encode(text);
}

export function utf8Decode(bytes: Uint8Array): string {
  return new TextDecoder().decode(bytes);
}

export function concatBytes(...parts: readonly Uint8Array[]): Uint8Array {
  let length = 0;
  for (const part of parts) length += part.length;
  const out = new Uint8Array(length);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

export function bytesEqual(left: Uint8Array, right: Uint8Array): boolean {
  if (left.length !== right.length) return false;
  let diff = 0;
  for (let index = 0; index < left.length; index += 1) diff |= left[index] ^ right[index];
  return diff === 0;
}

export function writeUint16BE(value: number): Uint8Array {
  return new Uint8Array([(value >> 8) & 0xff, value & 0xff]);
}
