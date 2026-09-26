/**
 * App Design on the phone (docs/specs/app-design.md): the operations the
 * design page may ask the app to perform, and the one place each becomes a
 * kanna-server route.
 *
 * The page is Kanna's own bundled code, but it runs in a WebView, so the app
 * treats what it posts as input: an operation must be one of these, with
 * arguments of exactly these shapes, and it always addresses the task the
 * app opened the page for. The page never names a URL, a task or a
 * credential; the transport (LAN or relay) is the app's.
 */

export type DesignOperation =
  | { op: "view" }
  | { op: "changes"; known: { doc?: number; feed?: number; timeoutMs?: number } }
  | { op: "sync"; request: { schemaVersion: string; stateVector: string; update?: string } }
  | {
      op: "createThread";
      request: {
        threadId: string;
        commentId: string;
        kind: "comment" | "message";
        body: string;
        anchor?: { blockId: string; quotedText: string; stateVector?: string };
      };
    }
  | { op: "reply"; threadId: string; request: { commentId: string; body: string } }
  | { op: "resolve"; threadId: string; resolved: boolean }
  | { op: "retryDelivery"; deliveryId: string };

export interface DesignRoute {
  method: "GET" | "POST";
  /** The path under `/v1/tasks/{task}/design`. */
  subpath: string;
  body: unknown | null;
}

const ID = /^[A-Za-z0-9_-]{1,80}$/;
const BASE64 = /^[A-Za-z0-9+/]*={0,2}$/;
/** Generous for one sync from a phone that only comments. */
const MAX_UPDATE_CHARS = 1_500_000;
const MAX_BODY_CHARS = 20_000;

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const isId = (value: unknown): value is string => typeof value === "string" && ID.test(value);
const isText = (value: unknown, max = MAX_BODY_CHARS): value is string =>
  typeof value === "string" && value.length <= max;
const isBase64 = (value: unknown, max = MAX_UPDATE_CHARS): value is string =>
  typeof value === "string" && value.length <= max && BASE64.test(value);
const isRevision = (value: unknown) =>
  value === undefined || (typeof value === "number" && Number.isInteger(value) && value >= -1);

/** Parse what the page posted; anything else is `null` and nothing is sent. */
export function parseDesignOperation(op: unknown, args: unknown): DesignOperation | null {
  if (!Array.isArray(args)) return null;
  switch (op) {
    case "view":
      return args.length === 0 ? { op } : null;
    case "changes": {
      const known = args[0];
      if (!isObject(known) || !isRevision(known.doc) || !isRevision(known.feed)) return null;
      const timeoutMs = known.timeoutMs;
      if (timeoutMs !== undefined && (typeof timeoutMs !== "number" || timeoutMs < 0 || timeoutMs > 25_000)) return null;
      return {
        op,
        known: {
          ...(known.doc !== undefined ? { doc: known.doc as number } : {}),
          ...(known.feed !== undefined ? { feed: known.feed as number } : {}),
          ...(timeoutMs !== undefined ? { timeoutMs } : {}),
        },
      };
    }
    case "sync": {
      const request = args[0];
      if (!isObject(request) || !isText(request.schemaVersion, 200) || !isBase64(request.stateVector, 100_000)) return null;
      if (request.update !== undefined && !isBase64(request.update)) return null;
      return {
        op,
        request: {
          schemaVersion: request.schemaVersion,
          stateVector: request.stateVector,
          ...(request.update !== undefined ? { update: request.update as string } : {}),
        },
      };
    }
    case "createThread": {
      const request = args[0];
      if (!isObject(request) || !isId(request.threadId) || !isId(request.commentId) || !isText(request.body)) return null;
      if (request.kind !== "comment" && request.kind !== "message") return null;
      let anchor: { blockId: string; quotedText: string; stateVector?: string } | undefined;
      if (request.anchor !== undefined) {
        const raw = request.anchor;
        if (!isObject(raw) || !isText(raw.blockId, 100) || !isText(raw.quotedText, 5_000)) return null;
        if (raw.stateVector !== undefined && !isBase64(raw.stateVector, 100_000)) return null;
        anchor = {
          blockId: raw.blockId,
          quotedText: raw.quotedText,
          ...(raw.stateVector !== undefined ? { stateVector: raw.stateVector as string } : {}),
        };
      }
      return {
        op,
        request: {
          threadId: request.threadId,
          commentId: request.commentId,
          kind: request.kind,
          body: request.body,
          ...(anchor ? { anchor } : {}),
        },
      };
    }
    case "reply": {
      const [threadId, request] = args;
      if (!isId(threadId) || !isObject(request) || !isId(request.commentId) || !isText(request.body)) return null;
      return { op, threadId, request: { commentId: request.commentId, body: request.body } };
    }
    case "resolve": {
      const [threadId, resolved] = args;
      return isId(threadId) && typeof resolved === "boolean" ? { op, threadId, resolved } : null;
    }
    case "retryDelivery": {
      const [deliveryId] = args;
      return isId(deliveryId) ? { op, deliveryId } : null;
    }
    default:
      return null;
  }
}

export function designRoute(operation: DesignOperation): DesignRoute {
  switch (operation.op) {
    case "view":
      return { method: "GET", subpath: "", body: null };
    case "changes": {
      const query = new URLSearchParams();
      if (operation.known.doc !== undefined && operation.known.doc >= 0) query.set("doc", String(operation.known.doc));
      if (operation.known.feed !== undefined && operation.known.feed >= 0) query.set("feed", String(operation.known.feed));
      if (operation.known.timeoutMs !== undefined) query.set("timeoutMs", String(operation.known.timeoutMs));
      return { method: "GET", subpath: `/changes?${query.toString()}`, body: null };
    }
    case "sync":
      return { method: "POST", subpath: "/document/sync", body: operation.request };
    case "createThread":
      return { method: "POST", subpath: "/threads", body: operation.request };
    case "reply":
      return {
        method: "POST",
        subpath: `/threads/${encodeURIComponent(operation.threadId)}/comments`,
        body: operation.request,
      };
    case "resolve":
      return {
        method: "POST",
        subpath: `/threads/${encodeURIComponent(operation.threadId)}/resolve`,
        body: { resolved: operation.resolved },
      };
    case "retryDelivery":
      return {
        method: "POST",
        subpath: `/deliveries/${encodeURIComponent(operation.deliveryId)}/retry`,
        body: null,
      };
  }
}

export function designTaskPath(taskId: string, route: DesignRoute): string {
  return `/v1/tasks/${encodeURIComponent(taskId)}/design${route.subpath}`;
}

/** What the page asked for, if it is a well-formed request. */
export function readDesignBridgeMessage(
  data: string
): { type: "request"; id: number; operation: DesignOperation | null } | { type: "ready" } | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(data);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const message = parsed as { kind?: unknown; type?: unknown; id?: unknown; op?: unknown; args?: unknown };
  if (message.kind !== "kanna-design") return null;
  if (message.type === "ready") return { type: "ready" };
  if (message.type !== "request" || typeof message.id !== "number" || !Number.isInteger(message.id)) return null;
  return { type: "request", id: message.id, operation: parseDesignOperation(message.op, message.args) };
}
