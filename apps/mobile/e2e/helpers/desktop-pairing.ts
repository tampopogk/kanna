import { networkInterfaces } from "node:os";
import { localProcessFetch, type LocalProcessFetch } from "@kanna/local-process-fetch";
import type { Browser } from "webdriverio";
import { MOBILE_E2E_IDS } from "../../src/e2eTestIds";
import type { E2eConnectionDiagnostics } from "../../src/e2eConnectionDiagnostics";
import { readDesktopIdentity } from "./desktop";
import { seedPairedTrustedDesktopThroughDeepLink } from "./trust-seed";

/**
 * Pairing for the ordinary desktop-server smoke.
 *
 * The identity-only `e2e-trust` seed leaves the app with a desktop id and name
 * but no LAN endpoint and no device secret. On the simulator that still works
 * by accident: the app finds the desktop over Bonjour and `kanna-server` grants
 * a loopback peer privileged access. On a physical iPhone neither holds — the
 * peer is a LAN address, `require_http_access` denies every task read without a
 * paired device secret, and the only route is whatever Bonjour happened to
 * resolve — so the task list stays empty for the whole assertion window and the
 * runner cannot say why.
 *
 * This helper makes the ordinary smoke use the same paired contract the hybrid
 * lane already uses: create a real pairing session on the exact selected server
 * (a loopback-only server route, so it runs on the desktop's own machine), seed
 * the exact app-side endpoint, and claim the payload through the app's own
 * pairing code path, which persists the device secret. Nothing here spoofs the
 * store or the server's trust rules.
 */

export interface DesktopPairingSession {
  desktopId: string;
  pairingPayload: string;
}

export interface DesktopIdentity {
  desktopId: string;
  desktopName: string;
}

/** Bounded confirmation that the app persisted the paired credential. Not a task-load wait. */
export const PAIRING_CONFIRMATION_TIMEOUT_MS = 15_000;
const POLL_INTERVAL_MS = 250;

export interface DiagnosticsElement {
  getAttribute(name: string): Promise<string | null>;
  isExisting(): Promise<boolean>;
}

export interface DiagnosticsDriver {
  $(selector: string): Promise<DiagnosticsElement> | DiagnosticsElement;
  waitUntil(
    condition: () => Promise<boolean>,
    options: { interval: number; timeout: number; timeoutMsg: string }
  ): Promise<unknown>;
}

function isLoopbackHostname(hostname: string): boolean {
  return hostname === "127.0.0.1" || hostname === "localhost" || hostname === "::1" || hostname === "[::1]";
}

export function localInterfaceAddresses(): string[] {
  return Object.values(networkInterfaces()).flatMap((entries) =>
    (entries ?? []).map((entry) => entry.address)
  );
}

/**
 * Where the runner creates the pairing session. `kanna-server` only starts a
 * session for a loopback peer, so the configured desktop URL — which `kd`
 * already writes with this Mac's LAN address for the phone's benefit — is
 * rewritten to loopback when it names one of this machine's own interfaces,
 * and refused when it names another machine.
 */
export function resolvePairingSessionUrl(
  configuredDesktopServerUrl: string,
  localAddresses: readonly string[] = localInterfaceAddresses()
): string {
  const url = new URL(configuredDesktopServerUrl);
  if (isLoopbackHostname(url.hostname)) {
    return url.toString().replace(/\/$/, "");
  }
  if (localAddresses.includes(url.hostname)) {
    url.hostname = "127.0.0.1";
    return url.toString().replace(/\/$/, "");
  }
  throw new Error(
    `Pairing sessions can only be created on the desktop's own machine: ` +
      `KANNA_E2E_DESKTOP_SERVER_URL names ${url.hostname}, which is not a loopback address or an interface of this Mac.`
  );
}

export async function createDesktopPairingSession(
  baseUrl: string,
  fetchImpl: LocalProcessFetch = localProcessFetch
): Promise<DesktopPairingSession> {
  const response = await fetchImpl(`${baseUrl}/v1/pairing/sessions`, { method: "POST" });
  const body = await response.json().catch(() => null) as
    | { desktopId?: unknown; pairingPayload?: unknown }
    | null;
  if (
    !response.ok ||
    typeof body?.desktopId !== "string" ||
    typeof body.pairingPayload !== "string" ||
    !body.pairingPayload
  ) {
    throw new Error(
      `Failed to create a mobile E2E pairing session on ${baseUrl}: HTTP ${response.status}`
    );
  }
  return { desktopId: body.desktopId, pairingPayload: body.pairingPayload };
}

/**
 * Metro env for the desktop-server smoke modes: the app's explicit development
 * server route beside the app environment. The app consumes it only in a
 * development runtime, probes `/v1/status` for the real identity, and then
 * uses the unchanged pairing claim and device-secret LAN transport.
 */
export function resolveDesktopServerExpoEnv(input: {
  appEnv: string;
  appDesktopServerUrl: string;
}): Record<string, string> {
  return {
    KANNA_APP_ENV: input.appEnv,
    EXPO_PUBLIC_KANNA_SERVER_URL: input.appDesktopServerUrl
  };
}

export async function pairExactDesktopThroughDeepLink(input: {
  bundleId: string;
  driver: Browser;
  /** The runner-side URL `KANNA_E2E_DESKTOP_SERVER_URL` names. */
  configuredDesktopServerUrl: string;
  /** The URL the app itself must use (LAN address for a physical device). */
  appDesktopServerUrl: string;
  selectedTaskId?: string;
  localAddresses?: readonly string[];
  readIdentity?: (baseUrl: string) => Promise<DesktopIdentity>;
  createPairingSession?: (baseUrl: string) => Promise<DesktopPairingSession>;
}): Promise<DesktopIdentity> {
  const pairingSessionUrl = resolvePairingSessionUrl(
    input.configuredDesktopServerUrl,
    input.localAddresses
  );
  const readIdentity = input.readIdentity ?? readDesktopIdentity;
  const createSession = input.createPairingSession ?? createDesktopPairingSession;
  const identity = await readIdentity(pairingSessionUrl);

  await seedPairedTrustedDesktopThroughDeepLink({
    bundleId: input.bundleId,
    driver: input.driver,
    desktop: {
      desktopId: identity.desktopId,
      displayName: identity.desktopName,
      lanBaseUrl: input.appDesktopServerUrl
    },
    selectedTaskId: input.selectedTaskId,
    async createPairingSession() {
      const session = await createSession(pairingSessionUrl);
      if (session.desktopId !== identity.desktopId) {
        throw new Error(
          `Pairing session belongs to desktop ${session.desktopId}, not the selected test server ${identity.desktopId}.`
        );
      }
      return session;
    }
  });
  return identity;
}

export async function readMobileConnectionDiagnostics(
  driver: DiagnosticsDriver
): Promise<E2eConnectionDiagnostics | null> {
  const marker = await driver.$(`~${MOBILE_E2E_IDS.connectionDiagnostics}`);
  if (!(await marker.isExisting())) return null;
  const label = await marker.getAttribute("label");
  if (!label) return null;
  try {
    return JSON.parse(label) as E2eConnectionDiagnostics;
  } catch {
    return null;
  }
}

export function describesExactDesktopPairing(
  diagnostics: E2eConnectionDiagnostics | null,
  expected: { desktopId: string; appDesktopServerUrl: string }
): boolean {
  if (!diagnostics) return false;
  const origin = new URL(expected.appDesktopServerUrl).origin;
  const trusted = diagnostics.trustedDesktops.find(
    (desktop) => desktop.desktopId === expected.desktopId
  );
  return (
    diagnostics.mobileDeviceIdPresent &&
    trusted !== undefined &&
    trusted.deviceSecretPresent &&
    trusted.lanEndpoints.includes(origin)
  );
}

/**
 * Prove the claim landed before any task-list deadline starts: the app holds a
 * device id, the exact desktop's device secret, and the exact endpoint.
 */
export async function waitForExactDesktopPairing(
  driver: DiagnosticsDriver,
  expected: { desktopId: string; appDesktopServerUrl: string },
  timeoutMs: number = PAIRING_CONFIRMATION_TIMEOUT_MS
): Promise<E2eConnectionDiagnostics> {
  let latest: E2eConnectionDiagnostics | null = null;
  const expectation =
    `Expected the app to hold a paired device secret and endpoint for desktop ${expected.desktopId} ` +
    `at ${new URL(expected.appDesktopServerUrl).origin}`;
  try {
    await driver.waitUntil(
      async () => {
        latest = await readMobileConnectionDiagnostics(driver);
        return describesExactDesktopPairing(latest, expected);
      },
      { interval: POLL_INTERVAL_MS, timeout: timeoutMs, timeoutMsg: expectation }
    );
  } catch (error) {
    // The message is built after the deadline so it carries the last picture
    // the app painted, not the one from before the first poll.
    const wrapped = new Error(
      `${expectation}; last connection diagnostics: ${JSON.stringify(latest)}`
    );
    Object.assign(wrapped, { cause: error });
    throw wrapped;
  }
  return latest as unknown as E2eConnectionDiagnostics;
}

/**
 * Run a smoke section and, on failure, retain the app's sanitized connection
 * picture beside the original assertion. The deadline inside `run` is
 * untouched; only the failure report grows.
 */
export async function withConnectionDiagnostics<T>(
  driver: DiagnosticsDriver,
  label: string,
  run: () => Promise<T>
): Promise<T> {
  try {
    return await run();
  } catch (error) {
    const diagnostics = await readMobileConnectionDiagnostics(driver).catch(() => null);
    const message = error instanceof Error ? error.message : String(error);
    const wrapped = new Error(
      `${message}\n[mobile connection diagnostics after ${label}] ${JSON.stringify(diagnostics)}`
    );
    Object.assign(wrapped, { cause: error });
    throw wrapped;
  }
}
