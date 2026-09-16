// Private key material lives in the platform keystore (iOS Keychain,
// Android Keystore-backed SharedPreferences) through `expo-secure-store`,
// never in AsyncStorage: AsyncStorage is a plain file in the app container
// that device backups and forensic extraction read as-is.
//
// `WHEN_UNLOCKED_THIS_DEVICE_ONLY` keeps the key out of iCloud/iTunes backups
// and unreadable while the device is locked. A restored backup therefore has
// no device identity and simply pairs again; it never inherits another
// phone's identity.

export interface SecureKeyStore {
  getItem(key: string): Promise<string | null>;
  setItem(key: string, value: string): Promise<void>;
  deleteItem(key: string): Promise<void>;
}

export class SecureKeyStoreUnavailableError extends Error {
  constructor(detail: string) {
    super(`the secure key store is unavailable: ${detail}`);
    this.name = "SecureKeyStoreUnavailableError";
  }
}

interface ExpoSecureStoreModule {
  isAvailableAsync(): Promise<boolean>;
  getItemAsync(key: string, options?: { keychainAccessible?: unknown }): Promise<string | null>;
  setItemAsync(key: string, value: string, options?: { keychainAccessible?: unknown }): Promise<void>;
  deleteItemAsync(key: string, options?: { keychainAccessible?: unknown }): Promise<void>;
  WHEN_UNLOCKED_THIS_DEVICE_ONLY: unknown;
}

const KEY_PATTERN = /^[A-Za-z0-9._-]+$/;

function assertKey(key: string): void {
  if (!KEY_PATTERN.test(key)) {
    throw new Error(`secure key store key must match ${KEY_PATTERN}: ${key}`);
  }
}

/** The production store. Fails closed when the native module is missing. */
export async function createExpoSecureKeyStore(): Promise<SecureKeyStore> {
  let module: ExpoSecureStoreModule;
  try {
    module = (await import("expo-secure-store")) as unknown as ExpoSecureStoreModule;
  } catch (error) {
    throw new SecureKeyStoreUnavailableError(error instanceof Error ? error.message : String(error));
  }
  if (!(await module.isAvailableAsync())) {
    throw new SecureKeyStoreUnavailableError("expo-secure-store reports no keystore on this device");
  }
  const options = { keychainAccessible: module.WHEN_UNLOCKED_THIS_DEVICE_ONLY };
  return {
    async getItem(key) {
      assertKey(key);
      return module.getItemAsync(key, options);
    },
    async setItem(key, value) {
      assertKey(key);
      if (value.length > 2048) {
        // expo-secure-store's documented per-item limit.
        throw new Error("secure key store values must be at most 2048 bytes");
      }
      await module.setItemAsync(key, value, options);
    },
    async deleteItem(key) {
      assertKey(key);
      await module.deleteItemAsync(key, options);
    },
  };
}

/** Process-local store for tests and harnesses. */
export function createMemorySecureKeyStore(initial: Record<string, string> = {}): SecureKeyStore {
  const items = new Map(Object.entries(initial));
  return {
    async getItem(key) {
      assertKey(key);
      return items.get(key) ?? null;
    },
    async setItem(key, value) {
      assertKey(key);
      items.set(key, value);
    },
    async deleteItem(key) {
      assertKey(key);
      items.delete(key);
    },
  };
}
