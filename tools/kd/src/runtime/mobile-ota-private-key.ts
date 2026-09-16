import {
  closeSync,
  constants,
  fstatSync,
  openSync
} from "node:fs";
import { isAbsolute } from "node:path";

export const MOBILE_OTA_PRIVATE_KEY_ENV = "KANNA_OTA_PRIVATE_KEY_PATH";

export function mobileAppEnvironmentRequiresOtaSigning(
  appEnvironment: string | undefined
): boolean {
  const normalized = appEnvironment?.trim();
  return normalized === "staging" || normalized === "prod" || normalized === "production";
}

/**
 * Resolve and preflight the local key file Expo needs to sign a development
 * manifest. This deliberately opens no key material: Expo remains the only
 * process that reads the selected secret.
 */
export function resolveMobileOtaPrivateKeyPath(
  env: Record<string, string | undefined>,
  options: { required: boolean }
): string | undefined {
  // Development config has no OTA certificate and must stay unsigned even if
  // the parent shell happens to carry a signing-key selector.
  if (!options.required) return undefined;

  const keyPath = env[MOBILE_OTA_PRIVATE_KEY_ENV]?.trim();
  if (!keyPath) {
    throw new Error(
      `Signed mobile Metro startup requires ${MOBILE_OTA_PRIVATE_KEY_ENV}. ` +
        "For the production QA gate, select the existing local key with --key-path <absolute-path>."
    );
  }
  if (keyPath.includes("\n") || keyPath.includes("\r") || keyPath.includes("\0")) {
    throw new Error(
      `Invalid ${MOBILE_OTA_PRIVATE_KEY_ENV}: line breaks and NUL bytes are not allowed.`
    );
  }
  if (!isAbsolute(keyPath)) {
    throw new Error(`Invalid ${MOBILE_OTA_PRIVATE_KEY_ENV}: use an absolute path.`);
  }

  let descriptor: number;
  try {
    descriptor = openSync(
      keyPath,
      constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK
    );
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === "ENOENT") {
      throw new Error(`The selected mobile OTA private key file does not exist.`);
    }
    if (code === "ELOOP") {
      throw new Error(`The selected mobile OTA private key must not be a symbolic link.`);
    }
    if (code === "EACCES" || code === "EPERM") {
      throw new Error(`The selected mobile OTA private key file is not readable.`);
    }
    throw new Error(
      `Unable to open the selected mobile OTA private key file (${code ?? "unknown error"}).`
    );
  }

  try {
    if (!fstatSync(descriptor).isFile()) {
      throw new Error(`The selected mobile OTA private key must be a regular file.`);
    }
  } finally {
    closeSync(descriptor);
  }

  return keyPath;
}
