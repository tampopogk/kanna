// This phone's secure-channel identity: one X25519 static key per app
// install, generated from the platform CSPRNG and kept in the secure key
// store. Every desktop this phone pairs with registers its public half; a
// desktop that no longer has it (revoked, or a fresh install here) simply
// treats the phone as unpaired.

import { encodeKey, generateKeypair, keypairFromPrivate, type Keypair } from "@kanna/secure-channel";
import { decodeBase64Url, encodeBase64Url } from "@kanna/secure-channel";
import type { RandomBytes } from "./randomBytes";
import type { SecureKeyStore } from "./secureKeyStore";

export const CHANNEL_IDENTITY_KEY = "kanna.secure-channel.device-identity.v1";

export interface DeviceChannelIdentity {
  keypair: Keypair;
  /** Unpadded base64url public key, the form the desktop stores. */
  publicKey: string;
}

/**
 * Loads the identity, generating and storing one on first use. A stored
 * value that does not decode is *replaced*, not repaired: the phone cannot
 * prove it owned whatever that was, and re-pairing is the recovery.
 */
export async function loadOrCreateChannelIdentity(
  store: SecureKeyStore,
  randomBytes: RandomBytes,
): Promise<DeviceChannelIdentity> {
  const stored = await store.getItem(CHANNEL_IDENTITY_KEY);
  if (stored) {
    try {
      const privateKey = decodeBase64Url(stored.trim());
      if (privateKey.length === 32) {
        const keypair = keypairFromPrivate(privateKey);
        return { keypair, publicKey: encodeKey(keypair.publicKey) };
      }
    } catch {
      // fall through to regeneration
    }
  }
  const keypair = generateKeypair(randomBytes);
  await store.setItem(CHANNEL_IDENTITY_KEY, encodeBase64Url(keypair.privateKey));
  return { keypair, publicKey: encodeKey(keypair.publicKey) };
}
