import { DesignRequestError, type DesignTransport } from "../types";

/**
 * The phone page's side of the WebView bridge. The page never holds a
 * credential or an address: it names an operation and its arguments, and the
 * app — which alone knows the task, the connection (LAN or relay) and the
 * device's pairing — validates the request and performs it.
 *
 * Page → app: `{ kind: "kanna-design", type: "request", id, op, args }`.
 * App → page: `window.__kannaDesign.receive({ type: "response", id, ok, value | error })`,
 * plus `{ type: "resume" }` when the app returns to the foreground and
 * `{ type: "theme", theme }`.
 */
export type DesignBridgeOp =
  | "view"
  | "changes"
  | "sync"
  | "createThread"
  | "reply"
  | "resolve"
  | "retryDelivery";

export interface DesignBridgeRequest {
  kind: "kanna-design";
  type: "request";
  id: number;
  op: DesignBridgeOp;
  args: unknown[];
}

export type DesignBridgeResponse =
  | { type: "response"; id: number; ok: true; value: unknown }
  | { type: "response"; id: number; ok: false; error: { message: string; status: number; reason: string | null } };

type Post = (message: string) => void;

export class PageBridge {
  private next = 1;
  private waiting = new Map<number, { resolve(value: unknown): void; reject(error: unknown): void }>();

  constructor(private readonly post: Post) {}

  call<T>(op: DesignBridgeOp, ...args: unknown[]): Promise<T> {
    const id = this.next++;
    const request: DesignBridgeRequest = { kind: "kanna-design", type: "request", id, op, args };
    return new Promise<T>((resolve, reject) => {
      this.waiting.set(id, { resolve: resolve as (value: unknown) => void, reject });
      this.post(JSON.stringify(request));
    });
  }

  receive(message: DesignBridgeResponse): void {
    const waiter = this.waiting.get(message.id);
    if (!waiter) return;
    this.waiting.delete(message.id);
    if (message.ok) waiter.resolve(message.value);
    else waiter.reject(new DesignRequestError(message.error.message, message.error.status, message.error.reason));
  }

  /** Fail every request still waiting (the app reloaded the page's host). */
  failAll(reason: string): void {
    for (const waiter of this.waiting.values()) waiter.reject(new DesignRequestError(reason, 0, "bridge"));
    this.waiting.clear();
  }

  transport(): DesignTransport {
    return {
      view: () => this.call("view"),
      changes: (known) => this.call("changes", known),
      sync: (request) => this.call("sync", request),
      createThread: (request) => this.call("createThread", request),
      reply: (threadId, request) => this.call("reply", threadId, request),
      resolve: (threadId, resolved) => this.call("resolve", threadId, resolved),
      retryDelivery: (deliveryId) => this.call("retryDelivery", deliveryId),
    };
  }
}
