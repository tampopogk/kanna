import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
const localCredentialMock = vi.hoisted(() => vi.fn(async () => "local-control-token"));
const fetchDesktopMachinesMock = vi.hoisted(() => vi.fn());

vi.mock("../invoke", () => ({
  invoke: invokeMock,
}));

vi.mock("./desktopAuthSdk", () => ({
  getConfiguredDesktopAuthSession: vi.fn(async () => ({
    getIdToken: vi.fn(async () => "id-token-that-must-never-be-sent"),
    getState: () => ({ status: "signedIn", user: { uid: "user-1" } }),
  })),
}));

vi.mock("./localControlCredential", () => ({
  localControlCredential: localCredentialMock,
}));

vi.mock("./kannaServerBaseUrl", () => ({
  resolveCurrentKannaServerBaseUrl: vi.fn(async () => "http://127.0.0.1:48120"),
}));

vi.mock("./desktopServerClient", () => ({
  fetchDesktopMachines: fetchDesktopMachinesMock,
}));

vi.mock("../stores/kanna", () => ({ useKannaStore: () => ({ cloudAccount: null }) }));

import {
  PRODUCTION_CLOUD_TRANSPORT_URL,
  STAGING_CLOUD_TRANSPORT_URL,
  createConfiguredDesktopRelayTerminalClient,
  createDesktopRelayTerminalClient,
  listActiveDesktopIdsViaRelay,
  parseAgentTerminalAttempts,
  peerViewProxyUrl,
  resolveDesktopCloudTransportUrlFromEnv,
  type DesktopRelayTerminalEvent,
} from "./desktopRelayTerminal";
import type { DesktopRemoteCompanionEvent } from "./desktopRemoteTaskClient";
import {
  isTaskFileUnreadableError,
  type TaskFileUnreadableError,
} from "./taskFileRead";

class FakeSocket {
  readyState = 1;
  onclose: ((event?: unknown) => void) | null = null;
  onerror: ((event?: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onopen: (() => void) | null = null;
  sent: string[] = [];

  close() {
    this.readyState = 3;
    this.onclose?.();
  }

  send(data: string) {
    this.sent.push(data);
  }

  drop(code?: number) {
    this.onclose?.(code === undefined ? {} : { code });
  }
}

/** The local proxy answers the renderer's `auth` frame with the sibling's `auth_ok`. */
async function openProxy(socket: FakeSocket) {
  socket.onopen?.();
  await Promise.resolve();
  await Promise.resolve();
  socket.onmessage?.({
    data: JSON.stringify({
      type: "auth_ok",
      stream_kinds: ["agent", "terminal", "companion"],
      capabilities: ["term_input_boundary"],
    }),
  });
  await Promise.resolve();
}

describe("peer view proxy URL", () => {
  it("names the sibling on the local server, never the relay", () => {
    expect(peerViewProxyUrl("http://127.0.0.1:48120", "desktop b/1")).toBe(
      "ws://127.0.0.1:48120/v1/peers/desktop%20b%2F1/ksp",
    );
    expect(peerViewProxyUrl("https://127.0.0.1:48120/", "desktop-b")).toBe(
      "wss://127.0.0.1:48120/v1/peers/desktop-b/ksp",
    );
  });
});

describe("configured desktop peer view client", () => {
  let originalWebSocket: typeof globalThis.WebSocket | undefined;

  beforeEach(() => {
    originalWebSocket = globalThis.WebSocket;
    invokeMock.mockResolvedValue(undefined);
    localCredentialMock.mockResolvedValue("local-control-token");
  });

  afterEach(() => {
    invokeMock.mockReset();
    localCredentialMock.mockReset();
    fetchDesktopMachinesMock.mockReset();
    if (originalWebSocket) {
      globalThis.WebSocket = originalWebSocket;
    } else {
      delete (globalThis as { WebSocket?: unknown }).WebSocket;
    }
  });

  it("opens the local proxy for the sibling with the local control credential and no relay socket or Firebase token", async () => {
    const socket = new FakeSocket();
    const webSocketMock = vi.fn(function WebSocketMock() {
      return socket;
    });
    globalThis.WebSocket = webSocketMock as unknown as typeof WebSocket;

    const client = await createConfiguredDesktopRelayTerminalClient();
    expect(client).not.toBeNull();
    const sendPromise = client!.sendInput({
      desktopId: "desktop-owner",
      taskId: "task-1",
      data: "hello\n",
    });
    await openProxy(socket);

    expect(webSocketMock).toHaveBeenCalledTimes(1);
    expect(webSocketMock).toHaveBeenCalledWith("ws://127.0.0.1:48120/v1/peers/desktop-owner/ksp");
    expect(webSocketMock).not.toHaveBeenCalledWith(PRODUCTION_CLOUD_TRANSPORT_URL);
    expect(webSocketMock).not.toHaveBeenCalledWith(STAGING_CLOUD_TRANSPORT_URL);
    const sent = socket.sent.map((entry) => JSON.parse(entry) as Record<string, unknown>);
    expect(sent[0]).toEqual({
      type: "auth",
      capabilities: ["companion_event_epoch", "term_input_boundary", "terminal_geometry", "terminal_active_view"],
      credential: "local-control-token",
    });
    expect(sent.some((frame) => frame.type === "tunnel_request")).toBe(false);
    expect(socket.sent.join("\n")).not.toContain("id-token-that-must-never-be-sent");
    expect(socket.sent.join("\n")).not.toContain("id_token");
    expect(sent).toContainEqual({
      type: "term_input",
      task_id: "task-1",
      data_b64: "aGVsbG8K",
    });
    await expect(sendPromise).resolves.toBeUndefined();
    client!.close();
  });

  it("lists reachable siblings from the local server's machine list, not the relay", async () => {
    fetchDesktopMachinesMock.mockResolvedValue({
      currentMachineId: "desktop-a",
      relayAvailable: true,
      machines: [
        { id: "desktop-a", name: "A", isLocal: true, encryption: "local" },
        { id: "desktop-b", name: "B", isLocal: false, encryption: "e2ee" },
        { id: "desktop-c", name: null, isLocal: false, encryption: "legacy" },
      ],
    });
    expect(await listActiveDesktopIdsViaRelay()).toEqual(new Set(["desktop-b", "desktop-c"]));
    fetchDesktopMachinesMock.mockRejectedValue(new Error("server down"));
    expect(await listActiveDesktopIdsViaRelay()).toBeNull();
  });
});

describe("createDesktopRelayTerminalClient", () => {
  it("surfaces a proxy refusal as a terminal error rather than reconnecting forever", async () => {
    vi.useFakeTimers();
    const sockets: FakeSocket[] = [];
    const client = createDesktopRelayTerminalClient({
      createSocket: () => {
        const socket = new FakeSocket();
        sockets.push(socket);
        return socket;
      },
      serverBaseUrl: "http://127.0.0.1:48120",
      getCredential: async () => "local-control-token",
    });
    const events: DesktopRelayTerminalEvent[] = [];
    client.observeTerminal({
      desktopId: "desktop-owner",
      taskId: "task-1",
      listener: (event) => events.push(event),
    });
    const socket = sockets[0];
    socket.onopen?.();
    await Promise.resolve();
    await Promise.resolve();
    // What `peers::send_error_and_close` actually writes: no `task_id` — the
    // connection never got far enough to be about a task — and its keys in the
    // order `serde_json::json!` sorts them into, discriminant last, which is
    // what a `{"type":"error"` prefix match silently missed.
    socket.onmessage?.({
      data: JSON.stringify({
        code: "peer_pairing_required",
        message: "this desktop is not paired with that machine; pair it from Preferences → Machines",
        type: "error",
      }),
    });
    socket.drop(1000);
    await Promise.resolve();
    expect(events).toContainEqual(expect.objectContaining({
      type: "error",
      taskId: "task-1",
      message: expect.stringContaining("Preferences"),
    }));
    // No silent retry behind a "Connecting..." line.
    vi.advanceTimersByTime(60_000);
    expect(sockets).toHaveLength(1);
    client.close();
    vi.useRealTimers();
  });

  /**
   * Spelled type-first here, the other way round from the test above: the
   * refusal is recognized by what the frame says, not by its key order.
   */
  it("drops a refused peer client so a later view dials again once the machines are paired", async () => {
    const sockets: FakeSocket[] = [];
    const client = createDesktopRelayTerminalClient({
      createSocket: () => {
        const socket = new FakeSocket();
        sockets.push(socket);
        return socket;
      },
      serverBaseUrl: "http://127.0.0.1:48120",
      getCredential: async () => "local-control-token",
    });
    const first = client.observeTerminal({
      desktopId: "desktop-owner",
      taskId: "task-1",
      listener: () => {},
    });
    sockets[0].onopen?.();
    await Promise.resolve();
    await Promise.resolve();
    sockets[0].onmessage?.({
      data: JSON.stringify({
        type: "error",
        code: "peer_pairing_required",
        message: "this desktop is not paired with that machine; pair it from Preferences → Machines",
      }),
    });
    sockets[0].drop(1000);
    await Promise.resolve();
    first.close();

    client.observeTerminal({
      desktopId: "desktop-owner",
      taskId: "task-1",
      listener: () => {},
    });
    expect(sockets).toHaveLength(2);
    client.close();
  });

  it("carries companion subscriptions over the same proxied session", async () => {
    const socket = new FakeSocket();
    const client = createDesktopRelayTerminalClient({
      createSocket: () => socket,
      serverBaseUrl: "http://127.0.0.1:48120",
      getCredential: async () => "local-control-token",
    });
    const events: DesktopRemoteCompanionEvent[] = [];
    const subscription = client.observeCompanion({
      desktopId: "desktop-owner",
      taskId: "task-1",
      listener: (event) => events.push(event),
    });
    await openProxy(socket);
    const sent = socket.sent.map((entry) => JSON.parse(entry) as Record<string, unknown>);
    expect(sent).toContainEqual(expect.objectContaining({ type: "attach", task_id: "task-1", kind: "companion" }));
    subscription.close();
    client.close();
  });

  /**
   * `kanna-server` bounds a task file read at 1 MiB and decodes it to UTF-8
   * before answering, so an oversized or binary file comes back as 413/415 —
   * a fact about that file, not about the tunnel. A reader that only displays
   * files (the tree explorer's preview column) has to tell the two apart.
   */
  it("classifies an oversized or non-text remote file separately from a failed read", async () => {
    const socket = new FakeSocket();
    const client = createDesktopRelayTerminalClient({
      createSocket: () => socket,
      serverBaseUrl: "http://127.0.0.1:48120",
      getCredential: async () => "local-control-token",
    });

    const oversizedPromise = client.readTaskFile({
      desktopId: "desktop-owner",
      taskId: "task-1",
      path: "huge.log",
    });
    const nonTextPromise = client.readTaskFile({
      desktopId: "desktop-owner",
      taskId: "task-1",
      path: "objects/pack.idx",
    });
    const unavailablePromise = client.readTaskFile({
      desktopId: "desktop-owner",
      taskId: "task-1",
      path: "src/app.ts",
    });

    await openProxy(socket);

    const sent = socket.sent.map((entry) => JSON.parse(entry));
    const request = (path: string) => sent.find((entry) => entry.path === path);
    const oversized = request("/v1/tasks/task-1/files/content?path=huge.log");
    const nonText = request("/v1/tasks/task-1/files/content?path=objects%2Fpack.idx");
    const unavailable = request("/v1/tasks/task-1/files/content?path=src%2Fapp.ts");

    socket.onmessage?.({
      data: JSON.stringify({
        type: "response",
        id: oversized.id,
        status: 413,
        body: { error: "file exceeds the 1 MiB limit" },
      }),
    });
    socket.onmessage?.({
      data: JSON.stringify({
        type: "response",
        id: nonText.id,
        status: 415,
        body: { error: "file is not valid UTF-8 text" },
      }),
    });
    socket.onmessage?.({
      data: JSON.stringify({ type: "response", id: unavailable.id, status: 503, body: null }),
    });

    const oversizedError = await oversizedPromise.catch((error: unknown) => error);
    const nonTextError = await nonTextPromise.catch((error: unknown) => error);
    const unavailableError = await unavailablePromise.catch((error: unknown) => error);

    expect(isTaskFileUnreadableError(oversizedError)).toBe(true);
    expect((oversizedError as TaskFileUnreadableError).reason).toBe("too-large");
    expect(isTaskFileUnreadableError(nonTextError)).toBe(true);
    expect((nonTextError as TaskFileUnreadableError).reason).toBe("not-text");
    // A tunnel that could not reach the desktop stays an ordinary failure.
    expect(isTaskFileUnreadableError(unavailableError)).toBe(false);
    expect((unavailableError as Error).message).toBe(
      "Remote task file read failed with HTTP 503.",
    );
  });
});

describe("resolveDesktopCloudTransportUrlFromEnv", () => {
  it("keeps the relay URL resolution for the transfer proxy and cloud features", () => {
    expect(resolveDesktopCloudTransportUrlFromEnv({ KANNA_RELAY_URL: "ws://custom" }, { dev: true })).toBe("ws://custom");
    expect(resolveDesktopCloudTransportUrlFromEnv({ KANNA_RELAY_PORT: "9080" }, { dev: true })).toBe("ws://127.0.0.1:9080");
    expect(resolveDesktopCloudTransportUrlFromEnv({ KANNA_CLOUD_ENV: "staging" }, { dev: false })).toBe(STAGING_CLOUD_TRANSPORT_URL);
    expect(resolveDesktopCloudTransportUrlFromEnv({}, { dev: false })).toBe(PRODUCTION_CLOUD_TRANSPORT_URL);
    expect(resolveDesktopCloudTransportUrlFromEnv({}, { dev: true })).toBeNull();
  });
});

describe("parseAgentTerminalAttempts", () => {
  const attempt = {
    id: "run-task-a-1",
    stage: "review",
    startedAt: "2026-09-16 00:00:00",
    cwd: "/work",
    archived: true,
    recordedLaunch: true,
    observedExitCode: 0,
  };

  it("carries the stream each attempt belongs to", () => {
    const parsed = parseAgentTerminalAttempts([
      { ...attempt, kind: "main" },
      { ...attempt, id: "run-task-a-teardown", kind: "teardown" },
    ]);

    expect(parsed.map((entry) => entry.kind)).toEqual(["main", "teardown"]);
  });

  it("reads a running teardown as the live terminal", () => {
    // Which stream an attempt is and whether its terminal still exists are
    // independent: the owner's daemon binds a `td-` session's attempt by the
    // same rule it binds an agent's, so a cleanup that is still running is the
    // live session, and a finished one is history.
    expect(parseAgentTerminalAttempts([
      { ...attempt, id: "run-task-a-teardown", kind: "teardown", archived: false, live: true },
      { ...attempt, id: "run-task-a-torn-down", kind: "teardown", live: false },
    ])).toEqual([
      expect.objectContaining({ kind: "teardown", live: true }),
      expect.objectContaining({ kind: "teardown", live: false }),
    ]);
  });

  it("reads an older desktop's kindless attempt as an agent session", () => {
    // A desktop on the other end of the relay that predates workspace
    // teardown capture only ever listed agent sessions. Defaulting keeps its
    // history readable instead of rejecting the whole response.
    expect(parseAgentTerminalAttempts([attempt])[0].kind).toBe("main");
  });

  it("rejects a malformed attempt rather than inventing fields", () => {
    expect(() => parseAgentTerminalAttempts([{ ...attempt, stage: 7 }])).toThrow();
  });
});
