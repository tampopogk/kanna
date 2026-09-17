import { getConfiguredDesktopAuthSession } from "./desktopAuthSdk";
import { invoke } from "../invoke";
import { type CloudAccessSnapshot, StreamClient } from "@kanna/stream-client";
import { createDesktopStreamFrameDecoder } from "./desktopStreamFrameDecoder";
import { TaskFileUnreadableError, taskFileUnreadableReasonForStatus } from "./taskFileRead";
import { localControlCredential } from "./localControlCredential";
import { resolveCurrentKannaServerBaseUrl } from "./kannaServerBaseUrl";
import { fetchDesktopMachines } from "./desktopServerClient";
import type {
  AgentTerminalArchive,
  AgentTerminalAttempt,
  DesktopRemoteTaskClient,
  DesktopRemoteTaskViewClient,
  RemoteTaskDiffContent,
  RemoteTaskGraphContent,
  RemoteTaskDirectoryListing,
} from "./desktopRemoteTaskClient";

export type {
  AdvanceRemoteTaskStageOptions,
  DesktopRemoteTerminalClient as DesktopRelayTerminalClient,
  DesktopRemoteTerminalEvent as DesktopRelayTerminalEvent,
  DesktopRemoteTerminalSubscription as DesktopRelayTerminalSubscription,
  MarkRemoteTaskReadOptions,
  ObserveDesktopRemoteTerminalOptions as ObserveDesktopRelayTerminalOptions,
  ReadRemoteTaskFileOptions,
  RemoteTaskActionOptions as RemoteTerminalActionOptions,
  RemoteTaskFileContent,
  ResizeRemoteTerminalOptions,
  SendRemoteTerminalInputOptions,
} from "./desktopRemoteTaskClient";

export const PRODUCTION_CLOUD_TRANSPORT_URL = "wss://relay.kanna.build";
export const STAGING_CLOUD_TRANSPORT_URL = "wss://relay-staging.kanna.build";

interface RelaySocketLike {
  readyState: number;
  close(): void;
  send(data: string): void;
  onclose: ((event?: unknown) => void) | null;
  onerror: ((event?: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onopen: (() => void) | null;
}

/**
 * How this window reaches a sibling desktop: never directly. Every sibling
 * view goes to the local `kanna-server`'s loopback proxy
 * (`GET /v1/peers/{desktop_id}/ksp`), which splices the KSP frames into a
 * sealed peer session to the sibling over LAN or the relay. The window
 * proves it is the app with the local control credential in its first
 * `auth` frame; it holds no relay socket, no Firebase token and no peer key
 * on this path.
 */
export interface DesktopRelayTerminalClientOptions {
  createSocket?: (url: string) => RelaySocketLike;
  /** The local `kanna-server` base URL (`http://127.0.0.1:<port>`). */
  serverBaseUrl: string;
  /** This desktop's local control credential. */
  getCredential(forceRefresh?: boolean): Promise<string | null>;
  observeAccess?(listener: (access: CloudAccessSnapshot) => void): () => void;
}

/** The loopback proxy URL for a sibling's sealed session. */
export function peerViewProxyUrl(serverBaseUrl: string, desktopId: string): string {
  const url = new URL(serverBaseUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.pathname = `/v1/peers/${encodeURIComponent(desktopId)}/ksp`;
  url.search = "";
  url.hash = "";
  return url.toString();
}

function assertSuccessfulTaskAction(
  response: { status: number; body: unknown },
  action: string,
): void {
  if (response.status >= 200 && response.status < 300) return;
  const body = response.body;
  let message: string | null = null;
  if (typeof body === "string" && body.trim()) {
    message = body.trim();
  } else if (body && typeof body === "object") {
    const candidate = body as { error?: unknown; message?: unknown };
    if (typeof candidate.error === "string" && candidate.error.trim()) {
      message = candidate.error.trim();
    } else if (typeof candidate.message === "string" && candidate.message.trim()) {
      message = candidate.message.trim();
    }
  }
  throw new Error(message ?? `Remote ${action} failed with HTTP ${response.status}`);
}

async function configuredClientOptions(): Promise<DesktopRelayTerminalClientOptions> {
  await invoke("ensure_mobile_server");
  const serverBaseUrl = await resolveCurrentKannaServerBaseUrl("creating peer view client");
  const authSession = await getConfiguredDesktopAuthSession();
  return {
    serverBaseUrl,
    getCredential: (forceRefresh?: boolean) => localControlCredential(forceRefresh),
    observeAccess: await configuredAccessObserver(authSession),
  };
}

export async function createConfiguredDesktopRelayTerminalClient(): Promise<DesktopRemoteTaskClient | null> {
  return createDesktopRelayTerminalClient(await configuredClientOptions());
}

export async function createConfiguredDesktopRemoteTaskViewClient(): Promise<DesktopRemoteTaskViewClient | null> {
  return createDesktopRelayTerminalClient(await configuredClientOptions());
}

async function configuredAccessObserver(authSession: Awaited<ReturnType<typeof getConfiguredDesktopAuthSession>>) {
  const [{ watch }, { useKannaStore }] = await Promise.all([import("vue"), import("../stores/kanna")]);
  const store = useKannaStore();
  return (listener: (access: CloudAccessSnapshot) => void) => watch(() => store.cloudAccount, (account) => {
    const auth = authSession.getState();
    if (auth.status === "signedIn" && auth.user.uid === account?.userId) {
      listener(account.entitlement ?? { active: true, status: "unknown", currentPeriodEndsAt: null, graceEndsAt: null });
    }
  }, { immediate: true });
}

/**
 * The sibling desktops this desktop can currently reach, as the local server
 * reports them (`GET /v1/cloud/desktops`: relay-listed and LAN-discovered
 * siblings, paired or legacy). The window never asks the relay itself.
 */
export async function listActiveDesktopIdsViaRelay(): Promise<Set<string> | null> {
  try {
    const list = await fetchDesktopMachines();
    return new Set(
      list.machines
        .filter((machine) => !machine.isLocal && machine.id.length > 0)
        .map((machine) => machine.id),
    );
  } catch (error) {
    console.debug("[peer-view] machine list unavailable:", error);
    return null;
  }
}

export function createDesktopRelayTerminalClient({
  createSocket = (url) => new WebSocket(url) as unknown as RelaySocketLike,
  getCredential,
  serverBaseUrl,
  observeAccess,
}: DesktopRelayTerminalClientOptions): DesktopRemoteTaskViewClient {
  const clients = new Map<string, StreamClient>();
  let latestAccess: CloudAccessSnapshot | null = null;
  const stopAccess = observeAccess?.((access) => {
    latestAccess = access;
    for (const client of clients.values()) client.refreshAccess(access.active);
  });

  const clientForDesktop = (desktopId: string): StreamClient => {
    const existing = clients.get(desktopId);
    if (existing) return existing;
    const proxyUrl = peerViewProxyUrl(serverBaseUrl, desktopId);
    const client = new StreamClient({
      url: proxyUrl,
      credentialProvider: (forceRefresh) => getCredential(forceRefresh),
      webSocketFactory: (url) => createSocket(url),
      reconnectDelaysMs: [250, 500, 1000, 2000],
      onAccessRequired: observeAccess ? () => undefined : undefined,
      terminalViewerRole: "remote",
      frameDecoder: createDesktopStreamFrameDecoder(),
    });
    clients.set(desktopId, client);
    if (latestAccess) client.refreshAccess(latestAccess.active);
    return client;
  };

  return {
    close() {
      stopAccess?.();
      for (const client of clients.values()) {
        client.close();
      }
      clients.clear();
    },
    observeTerminal(options) {
      const client = clientForDesktop(options.desktopId);
      client.attachTerminal(options.taskId, {
        onSnapshot(cols, rows, dataB64) {
          options.listener({
            type: "snapshot",
            taskId: options.taskId,
            cols,
            rows,
            data: decodeBase64(dataB64),
          });
        },
        onOutput(dataB64) {
          options.listener({
            type: "output",
            taskId: options.taskId,
            data: decodeBase64(dataB64),
          });
        },
        onSessionExit(code) {
          options.listener({ type: "exit", taskId: options.taskId, code });
        },
        onError(_code, message) {
          options.listener({ type: "error", taskId: options.taskId, message });
        },
      }, { passiveInitialAttach: true });
      return {
        close() {
          client.detach(options.taskId, "terminal");
        },
        registerViewer(cols: number, rows: number) {
          client.registerTerminalViewer(options.taskId, cols, rows);
        },
        setViewerVisible(visible: boolean) {
          client.setTerminalViewerVisibility(options.taskId, visible);
        },
        activate() {
          client.setTerminalViewerVisibility(options.taskId, true);
          client.activateTerminalViewer(options.taskId);
        },
      };
    },
    observeCompanion(options) {
      const client = clientForDesktop(options.desktopId);
      client.attachCompanion(options.taskId, {
        onSnapshot(snapshot) {
          options.listener({
            type: "snapshot",
            taskId: options.taskId,
            snapshot,
          });
        },
        onUnavailable() {
          options.listener({ type: "unavailable", taskId: options.taskId });
        },
        onEventResult(result) {
          options.listener({
            type: "event_result",
            taskId: options.taskId,
            result,
          });
        },
        onConnectionChange(connected) {
          options.listener({
            type: "connection",
            taskId: options.taskId,
            connected,
          });
        },
        onError(code, message) {
          options.listener({
            type: "error",
            taskId: options.taskId,
            code,
            message,
          });
        },
      });
      let closed = false;
      return {
        close() {
          if (closed) return;
          closed = true;
          client.detach(options.taskId, "companion");
        },
        sendEvent(sessionId, revision, event) {
          if (closed) return false;
          return client.sendCompanionEvent(
            options.taskId,
            sessionId,
            revision,
            event,
          );
        },
      };
    },
    async sendInput(options) {
      const client = clientForDesktop(options.desktopId);
      const dataB64 = encodeBase64(options.data);
      if (options.controlInput) {
        client.sendTermInput(options.taskId, dataB64, false, true);
      } else if (options.submissionBoundary) {
        client.sendTermInput(options.taskId, dataB64, true);
      } else {
        client.sendTermInput(options.taskId, dataB64);
      }
    },
    async resize(options) {
      clientForDesktop(options.desktopId).registerTerminalViewer(
        options.taskId,
        options.cols,
        options.rows,
      );
    },
    async closeTask(options) {
      const response = await clientForDesktop(options.desktopId).request(
        "POST",
        `/v1/tasks/${encodeURIComponent(options.taskId)}/actions/close`,
        null,
      );
      assertSuccessfulTaskAction(response, "task close");
    },
    async advanceStage(options) {
      const body = {
        source: "operator",
        ...(options.expectedTransitionRevision
          ? { expectedTransitionRevision: options.expectedTransitionRevision }
          : {}),
      };
      const response = await clientForDesktop(options.desktopId).request(
        "POST",
        `/v1/tasks/${encodeURIComponent(options.taskId)}/actions/advance-stage`,
        body,
      );
      assertSuccessfulTaskAction(response, "stage advance");
    },
    async readTaskFile(options) {
      const response = await clientForDesktop(options.desktopId).request(
        "GET",
        `/v1/tasks/${encodeURIComponent(options.taskId)}/files/content?path=${encodeURIComponent(options.path)}`,
        null,
      );
      if (response.status < 200 || response.status >= 300) {
        const message = `Remote task file read failed with HTTP ${response.status}.`;
        // 413/415 is the server saying this file has no text to hand over, not
        // that the tunnel or the task is unreachable. Same sentence either way;
        // only the type tells a caller which one it got.
        const reason = taskFileUnreadableReasonForStatus(response.status);
        throw reason ? new TaskFileUnreadableError(reason, message) : new Error(message);
      }
      const body = response.body;
      if (!isRecord(body) || typeof body.path !== "string" || typeof body.content !== "string") {
        throw new Error("Remote task file response was malformed.");
      }
      return { path: body.path, content: body.content };
    },
    async listTaskDirectory(options) {
      const client = clientForDesktop(options.desktopId);
      const entries: RemoteTaskDirectoryListing["entries"] = [];
      let offset = 0;
      let responsePath = options.path;
      let totalEntries = 0;
      while (true) {
        const response = await client.request(
          "GET",
          `/v1/tasks/${encodeURIComponent(options.taskId)}/browse?path=${encodeURIComponent(options.path)}&showAllFiles=${options.showAllFiles === true}&offset=${offset}&limit=100`,
          null,
        );
        assertSuccessfulTaskAction(response, "task directory read");
        const page = parseTaskDirectoryListing(response.body);
        responsePath = page.path;
        totalEntries = page.totalEntries;
        entries.push(...page.entries);
        if (page.nextOffset === null) break;
        offset = page.nextOffset;
      }
      return {
        path: responsePath,
        entries,
        offset: 0,
        nextOffset: null,
        totalEntries,
      };
    },
    async readTaskDiff(options) {
      const query = new URLSearchParams({
        scope: options.request.scope,
        mode: options.request.mode,
      });
      const response = await clientForDesktop(options.desktopId).request(
        "GET",
        `/v1/tasks/${encodeURIComponent(options.taskId)}/diff?${query.toString()}`,
        null,
      );
      assertSuccessfulTaskAction(response, "task diff read");
      return parseTaskDiffContent(response.body);
    },
    async readTaskGraph(options) {
      const query = options.request.fromRef
        ? `?fromRef=${encodeURIComponent(options.request.fromRef)}`
        : "";
      const response = await clientForDesktop(options.desktopId).request(
        "GET", `/v1/tasks/${encodeURIComponent(options.taskId)}/graph${query}`, null,
      );
      assertSuccessfulTaskAction(response, "task graph read");
      return parseTaskGraphContent(response.body);
    },
    async listAgentTerminalAttempts(options) {
      const response = await clientForDesktop(options.desktopId).request(
        "GET",
        `/v1/tasks/${encodeURIComponent(options.taskId)}/terminal-attempts`,
        null,
      );
      assertSuccessfulTaskAction(response, "agent history list");
      return parseAgentTerminalAttempts(response.body);
    },
    async readAgentTerminalArchive(options) {
      const response = await clientForDesktop(options.desktopId).request(
        "GET",
        `/v1/tasks/${encodeURIComponent(options.taskId)}/terminal-attempts/${encodeURIComponent(options.runId)}`,
        null,
      );
      assertSuccessfulTaskAction(response, "agent history read");
      return parseAgentTerminalArchive(response.body);
    },
    async markTaskRead(options) {
      const response = await clientForDesktop(options.desktopId).request(
        "POST",
        `/v1/tasks/${encodeURIComponent(options.taskId)}/actions/mark-read`,
        { expectedActivityRevision: options.expectedActivityRevision },
      );
      assertSuccessfulTaskAction(response, "mark read");
    },
  };
}

export function parseTaskDirectoryListing(value: unknown): RemoteTaskDirectoryListing {
  if (!isRecord(value) || !Array.isArray(value.entries)) {
    throw new Error("Remote task directory response was malformed.");
  }
  const entries = value.entries.map((entry) => {
    if (
      !isRecord(entry)
      || typeof entry.name !== "string"
      || typeof entry.path !== "string"
      || typeof entry.isDir !== "boolean"
    ) {
      throw new Error("Remote task directory response was malformed.");
    }
    const size = typeof entry.size === "number" || entry.size === null
      ? entry.size
      : undefined;
    return { name: entry.name, path: entry.path, isDir: entry.isDir, size };
  });
  if (
    typeof value.path !== "string"
    || typeof value.offset !== "number"
    || !(typeof value.nextOffset === "number" || value.nextOffset === null)
    || typeof value.totalEntries !== "number"
  ) {
    throw new Error("Remote task directory response was malformed.");
  }
  return {
    path: value.path,
    entries,
    offset: value.offset,
    nextOffset: value.nextOffset,
    totalEntries: value.totalEntries,
  };
}

export function parseAgentTerminalAttempts(value: unknown): AgentTerminalAttempt[] {
  if (!Array.isArray(value)) {
    throw new Error("Remote agent history list response was malformed.");
  }
  return value.map((attempt) => {
    if (
      !isRecord(attempt)
      || typeof attempt.id !== "string"
      || typeof attempt.stage !== "string"
      || typeof attempt.startedAt !== "string"
      || !(typeof attempt.cwd === "string" || attempt.cwd === null)
      || typeof attempt.archived !== "boolean"
      || !(typeof attempt.live === "boolean" || attempt.live === undefined)
      || typeof attempt.recordedLaunch !== "boolean"
      || !(typeof attempt.observedExitCode === "number" || attempt.observedExitCode === null)
    ) {
      throw new Error("Remote agent history list response was malformed.");
    }
    return {
      id: attempt.id,
      stage: attempt.stage,
      startedAt: attempt.startedAt,
      cwd: attempt.cwd,
      // An owner desktop whose server or daemon cannot report a live session
      // reports none, which lists every attempt as history rather than failing
      // the whole response.
      live: attempt.live === true,
      archived: attempt.archived,
      recordedLaunch: attempt.recordedLaunch,
      observedExitCode: attempt.observedExitCode,
    };
  });
}

export function parseAgentTerminalArchive(value: unknown): AgentTerminalArchive | null {
  if (value === null) return null;
  if (!isRecord(value) || !isRecord(value.binding)) {
    throw new Error("Remote agent history response was malformed.");
  }
  const snapshot = value.snapshot;
  if (
    typeof value.binding.task_id !== "string"
    || typeof value.binding.spawned_run_id !== "string"
    || typeof value.session_id !== "string"
    || typeof value.cwd !== "string"
    || !(typeof value.unavailable_reason === "string" || value.unavailable_reason === null)
    || !(typeof value.observed_exit_code === "number" || value.observed_exit_code === null)
    || !(snapshot === null || (
      isRecord(snapshot)
      && typeof snapshot.vt === "string"
      && typeof snapshot.cols === "number"
      && typeof snapshot.rows === "number"
    ))
  ) {
    throw new Error("Remote agent history response was malformed.");
  }
  return {
    binding: {
      task_id: value.binding.task_id,
      spawned_run_id: value.binding.spawned_run_id,
    },
    session_id: value.session_id,
    cwd: value.cwd,
    snapshot: snapshot === null
      ? null
      : { vt: snapshot.vt as string, cols: snapshot.cols as number, rows: snapshot.rows as number },
    unavailable_reason: value.unavailable_reason,
    observed_exit_code: value.observed_exit_code,
  };
}

export function parseTaskDiffContent(value: unknown): RemoteTaskDiffContent {
  if (
    !isRecord(value)
    || typeof value.taskId !== "string"
    || !(typeof value.baseRef === "string" || value.baseRef === null)
    || !(typeof value.mergeBase === "string" || value.mergeBase === null)
    || typeof value.patch !== "string"
    || typeof value.truncated !== "boolean"
  ) {
    throw new Error("Remote task diff response was malformed.");
  }
  return {
    taskId: value.taskId,
    baseRef: value.baseRef,
    mergeBase: value.mergeBase,
    patch: value.patch,
    truncated: value.truncated,
  };
}

export function parseTaskGraphContent(value: unknown): RemoteTaskGraphContent {
  if (!isRecord(value) || typeof value.taskId !== "string" || !Array.isArray(value.commits)
    || !(typeof value.headCommit === "string" || value.headCommit === null)) {
    throw new Error("Remote task graph response was malformed.");
  }
  for (const commit of value.commits) {
    if (!isRecord(commit) || typeof commit.hash !== "string" || typeof commit.shortHash !== "string"
      || typeof commit.message !== "string" || typeof commit.author !== "string"
      || typeof commit.timestamp !== "number" || !Array.isArray(commit.parents) || !Array.isArray(commit.refs)
      || !commit.parents.every((value) => typeof value === "string") || !commit.refs.every((value) => typeof value === "string")) {
      throw new Error("Remote task graph response was malformed.");
    }
  }
  return value as unknown as RemoteTaskGraphContent;
}

export async function resolveDesktopRelayUrl(): Promise<string | null> {
  const configured = await invoke<string>("read_env_var", { name: "KANNA_RELAY_URL" }).catch(() => "");
  const port = await invoke<string>("read_env_var", { name: "KANNA_RELAY_PORT" }).catch(() => "");
  const cloudEnv = await invoke<string>("read_env_var", { name: "KANNA_CLOUD_ENV" }).catch(() => "");
  return resolveDesktopCloudTransportUrlFromEnv({
    KANNA_RELAY_URL: configured,
    KANNA_RELAY_PORT: port,
    KANNA_CLOUD_ENV: cloudEnv,
  }, { dev: import.meta.env.DEV });
}

export function resolveDesktopCloudTransportUrlFromEnv(
  env: { KANNA_RELAY_URL?: string | null; KANNA_RELAY_PORT?: string | null; KANNA_CLOUD_ENV?: string | null },
  options: { dev: boolean },
): string | null {
  const configured = env.KANNA_RELAY_URL?.trim();
  if (configured) return configured;

  const port = env.KANNA_RELAY_PORT?.trim();
  if (port) return `ws://127.0.0.1:${port}`;

  const cloudEnv = env.KANNA_CLOUD_ENV?.trim().toLowerCase();
  if (cloudEnv === "staging") return STAGING_CLOUD_TRANSPORT_URL;
  if (!options.dev) return PRODUCTION_CLOUD_TRANSPORT_URL;

  return null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function decodeBase64(value: string): Uint8Array {
  if (!value) return new Uint8Array();
  const binary = globalThis.atob(value);
  return Uint8Array.from(binary, (char) => char.charCodeAt(0));
}

function encodeBase64(value: string): string {
  const bytes = new TextEncoder().encode(value);
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return globalThis.btoa(binary);
}
