import WebSocket, { type RawData } from "ws";
import * as relayClientModule from "../../../apps/mobile/src/lib/transports/relayClient";
import type {
  RelayDesktopClient,
  RelaySocketFactory,
  RelaySocketLike,
  SecureChannelRoute
} from "../../../apps/mobile/src/lib/transports/relayClient";

type RelayClientModule = typeof relayClientModule;
type RelayClientModuleWithInterop = RelayClientModule & {
  default?: RelayClientModule;
  "module.exports"?: RelayClientModule;
};

const relayClientExports = relayClientModule as RelayClientModuleWithInterop;
const createRelayDesktopClient =
  relayClientExports.createRelayDesktopClient ??
  relayClientExports.default?.createRelayDesktopClient ??
  relayClientExports["module.exports"]?.createRelayDesktopClient;

if (!createRelayDesktopClient) {
  throw new Error("Could not load createRelayDesktopClient from the mobile relay transport module.");
}

export class NodeRelaySocket implements RelaySocketLike {
  private readonly socket: WebSocket;
  onclose: ((event?: unknown) => void) | null = null;
  onerror: ((event?: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onopen: (() => void) | null = null;

  constructor(url: string, headers?: Record<string, string>) {
    this.socket = new WebSocket(url, headers ? { headers } : undefined);
    this.socket.on("open", () => this.onopen?.());
    this.socket.on("message", (data: RawData) => {
      this.onmessage?.({ data: data.toString() });
    });
    this.socket.on("error", (error) => this.onerror?.(error));
    this.socket.on("close", (code, reason) => {
      this.onclose?.({ code, reason: reason.toString() });
    });
  }

  get readyState(): number {
    return this.socket.readyState;
  }

  close(): void {
    this.socket.close();
  }

  send(data: string): void {
    this.socket.send(data);
  }
}

export function createNodeRelayDesktopClient(input: {
  getIdToken(forceRefresh?: boolean): Promise<string | null>;
  relayUrl: string;
  /** Per-desktop secure-channel route, exactly as the phone supplies it. */
  getSecureChannelRoute?(desktopId: string): SecureChannelRoute;
  /** Socket factory override, e.g. to tap the raw relay frames. */
  createSocket?: RelaySocketFactory;
}): RelayDesktopClient {
  return createRelayDesktopClient({
    createSocket: input.createSocket ?? ((url) => new NodeRelaySocket(url)),
    getIdToken: input.getIdToken,
    relayUrl: input.relayUrl,
    ...(input.getSecureChannelRoute ? { getSecureChannelRoute: input.getSecureChannelRoute } : {})
  });
}

/** Every raw relay frame a socket carried, in both directions. */
export interface RelayFrameTap {
  sent: string[];
  received: string[];
}

/** Wraps a relay socket so a test can read exactly what crossed the wire. */
export function tapRelaySocket(socket: RelaySocketLike, tap: RelayFrameTap): RelaySocketLike {
  const tapped: RelaySocketLike = {
    get readyState() {
      return socket.readyState;
    },
    close: () => socket.close(),
    send: (data) => {
      tap.sent.push(data);
      socket.send(data);
    },
    onclose: null,
    onerror: null,
    onmessage: null,
    onopen: null
  };
  socket.onopen = () => tapped.onopen?.();
  socket.onclose = (event) => tapped.onclose?.(event);
  socket.onerror = (event) => tapped.onerror?.(event);
  socket.onmessage = (event) => {
    if (typeof event.data === "string") tap.received.push(event.data);
    tapped.onmessage?.(event);
  };
  return tapped;
}
