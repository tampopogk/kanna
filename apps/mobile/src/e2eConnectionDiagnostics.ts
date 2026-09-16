import type { SessionState } from "./state/sessionStore";
import type { TrustedDesktopRecord } from "./state/sessionPersistence";

/**
 * The sanitized connection picture the E2E runner reads when a task-list or
 * pairing assertion fails.
 *
 * A failed smoke used to retain only the count of rendered task rows, which
 * proves neither a slow load nor a missing credential. This marker names the
 * route, status, and credential *presence* of the running app so a zero-row
 * result can be classified. It deliberately carries booleans and origins only:
 * never a device secret, push certificate, id token, email, task title, or
 * terminal content.
 */
export interface E2eConnectionDiagnostics {
  connectionMode: string | null;
  connectionState: SessionState["connectionState"];
  refreshStatus: SessionState["refreshStatus"];
  taskCollectionStatus: SessionState["taskCollectionStatus"];
  serverStatus: string | null;
  errorMessage: string | null;
  desktopId: string | null;
  selectedDesktopId: string | null;
  selectedRepoId: string | null;
  authStatus: string;
  mobileDeviceIdPresent: boolean;
  trustedDesktops: E2eTrustedDesktopDiagnostics[];
  liveLanDesktopIds: string[];
  accountDesktopIds: string[];
  taskCounts: { repo: number; recent: number };
}

export interface E2eTrustedDesktopDiagnostics {
  desktopId: string;
  /** Origins only (`http://host:port`); a malformed persisted hint reads as `<invalid>`. */
  lanEndpoints: string[];
  deviceSecretPresent: boolean;
  pushPairingCertPresent: boolean;
}

export function buildE2eConnectionDiagnostics(
  state: Pick<
    SessionState,
    | "connectionMode"
    | "connectionState"
    | "refreshStatus"
    | "taskCollectionStatus"
    | "serverStatus"
    | "errorMessage"
    | "desktopId"
    | "selectedDesktopId"
    | "selectedRepoId"
    | "auth"
    | "mobileDeviceId"
    | "trustedDesktops"
    | "liveLanDesktops"
    | "accountDesktops"
    | "repoTasks"
    | "recentTasks"
  >
): E2eConnectionDiagnostics {
  return {
    connectionMode: state.connectionMode,
    connectionState: state.connectionState,
    refreshStatus: state.refreshStatus,
    taskCollectionStatus: state.taskCollectionStatus,
    serverStatus: state.serverStatus,
    errorMessage: state.errorMessage,
    desktopId: state.desktopId,
    selectedDesktopId: state.selectedDesktopId,
    selectedRepoId: state.selectedRepoId,
    authStatus: state.auth.status,
    mobileDeviceIdPresent: typeof state.mobileDeviceId === "string" && state.mobileDeviceId.length > 0,
    trustedDesktops: state.trustedDesktops.map(describeTrustedDesktop),
    liveLanDesktopIds: state.liveLanDesktops.map((desktop) => desktop.id),
    accountDesktopIds: state.accountDesktops.map((desktop) => desktop.id),
    taskCounts: { repo: state.repoTasks.length, recent: state.recentTasks.length }
  };
}

export function serializeE2eConnectionDiagnostics(
  diagnostics: E2eConnectionDiagnostics
): string {
  return JSON.stringify(diagnostics);
}

function describeTrustedDesktop(
  desktop: TrustedDesktopRecord
): E2eTrustedDesktopDiagnostics {
  return {
    desktopId: desktop.desktopId,
    lanEndpoints: desktop.lanEndpoints.map((endpoint) => endpointOrigin(endpoint.baseUrl)),
    deviceSecretPresent:
      typeof desktop.deviceSecret === "string" && desktop.deviceSecret.length > 0,
    pushPairingCertPresent: desktop.pushPairingCert !== undefined
  };
}

function endpointOrigin(baseUrl: string): string {
  try {
    return new URL(baseUrl).origin;
  } catch {
    return "<invalid>";
  }
}
