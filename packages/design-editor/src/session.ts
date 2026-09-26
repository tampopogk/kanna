import * as Y from "yjs";
import {
  DesignRequestError,
  type CreateThreadRequest,
  type DesignThread,
  type DesignTransport,
  type DesignView,
} from "./types";

/** Transactions applied from the server; never sent back to it. */
export const SERVER_ORIGIN = Symbol("kanna-server");

/**
 * `connecting` until the first sync; `synced` when nothing local is unsaved;
 * `saving` while local edits are on their way; `offline` while the server
 * cannot be reached (edits stay queued and are sent on reconnect);
 * `incompatible` when the server refuses this editor's schema (the editor
 * must stop editing: a narrower schema would delete content); `closed`.
 */
export type DesignSessionStatus =
  | "connecting"
  | "synced"
  | "saving"
  | "offline"
  | "incompatible"
  | "closed";

export function toBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let index = 0; index < bytes.length; index += chunk) {
    binary += String.fromCharCode(...bytes.subarray(index, index + chunk));
  }
  return btoa(binary);
}

export function fromBase64(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

export interface DesignSessionOptions {
  schemaVersion: string;
  /** A viewer that never writes the document (it may still comment). */
  readOnly?: boolean;
  /** Batch local keystrokes this long before sending them. */
  flushDelayMs?: number;
  /** Long-poll timeout the server may hold a change request for. */
  pollTimeoutMs?: number;
  wait?: (ms: number) => Promise<void>;
}

/**
 * One client's live view of a design: the Yjs document, kept in sync with
 * kanna-server by state vector over plain JSON requests, and the feedback
 * feed. Every sync catches up from the server's durable state, so a client
 * that was offline, backgrounded or restarted loses nothing and needs no
 * server-side memory of it.
 */
export class DesignSession {
  readonly doc = new Y.Doc();
  view: DesignView | null = null;
  status: DesignSessionStatus = "connecting";
  lastError: string | null = null;
  private pending: Uint8Array[] = [];
  private inFlight: Promise<void> | null = null;
  private flushTimer: ReturnType<typeof setTimeout> | null = null;
  private docRevision = -1;
  private feedRevision = -1;
  private listeners = new Set<() => void>();
  private closed = false;
  private readonly wait: (ms: number) => Promise<void>;

  constructor(
    private readonly transport: DesignTransport,
    private readonly options: DesignSessionOptions,
  ) {
    this.wait = options.wait ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
    this.doc.on("update", (update: Uint8Array, origin: unknown) => {
      if (origin === SERVER_ORIGIN || this.options.readOnly) return;
      this.pending.push(update);
      this.setStatus("saving");
      this.scheduleFlush();
    });
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private emit(): void {
    for (const listener of this.listeners) listener();
  }

  private setStatus(status: DesignSessionStatus, error?: string): void {
    if (this.status === "incompatible" || this.status === "closed") return;
    this.status = status;
    this.lastError = error ?? null;
    this.emit();
  }

  /** Load the feed and the document, then follow the server's changes. */
  async start(): Promise<void> {
    await this.refreshView();
    await this.sync();
    void this.follow();
  }

  close(): void {
    this.closed = true;
    if (this.flushTimer) clearTimeout(this.flushTimer);
    this.status = "closed";
    this.doc.destroy();
    this.emit();
  }

  get isClosed(): boolean {
    return this.closed;
  }

  get hasUnsavedChanges(): boolean {
    return this.pending.length > 0 || this.inFlight !== null;
  }

  stateVector(): string {
    return toBase64(Y.encodeStateVector(this.doc));
  }

  async refreshView(): Promise<DesignView | null> {
    try {
      const view = await this.transport.view();
      this.view = view;
      this.feedRevision = view.feedRevision;
      this.emit();
      return view;
    } catch (error) {
      this.fail(error);
      return null;
    }
  }

  private scheduleFlush(): void {
    if (this.flushTimer || this.closed) return;
    this.flushTimer = setTimeout(() => {
      this.flushTimer = null;
      void this.sync().catch(() => undefined);
    }, this.options.flushDelayMs ?? 120);
  }

  /**
   * Send what is pending (if anything) and apply whatever the server has
   * that this client lacks. One request at a time; a failed send keeps its
   * update queued for the next attempt.
   */
  async sync(): Promise<void> {
    if (this.closed) return;
    if (this.inFlight) {
      await this.inFlight;
      if (this.pending.length === 0) return;
    }
    const run = async () => {
      const outgoing = this.pending.length ? Y.mergeUpdates(this.pending.splice(0)) : null;
      try {
        const response = await this.transport.sync({
          schemaVersion: this.options.schemaVersion,
          stateVector: toBase64(Y.encodeStateVector(this.doc)),
          ...(outgoing ? { update: toBase64(outgoing) } : {}),
        });
        if (this.closed) return;
        if (response.update) Y.applyUpdate(this.doc, fromBase64(response.update), SERVER_ORIGIN);
        this.docRevision = response.revision;
        this.setStatus(this.pending.length ? "saving" : "synced");
      } catch (error) {
        if (outgoing) this.pending.unshift(outgoing);
        this.fail(error);
        throw error;
      }
    };
    this.inFlight = run().finally(() => {
      this.inFlight = null;
    });
    await this.inFlight;
    if (this.pending.length && !this.closed) await this.sync();
  }

  /** Resolves once every local edit has been acknowledged by the server. */
  async whenSaved(): Promise<void> {
    if (this.flushTimer) {
      clearTimeout(this.flushTimer);
      this.flushTimer = null;
    }
    while (!this.closed && (this.pending.length || this.inFlight)) await this.sync();
  }

  private fail(error: unknown): void {
    if (error instanceof DesignRequestError && error.status === 422) {
      this.setStatus("incompatible", error.message);
      this.status = "incompatible";
      this.emit();
      return;
    }
    this.setStatus("offline", error instanceof Error ? error.message : String(error));
  }

  /** Follow document and feed changes until closed, retrying with backoff. */
  private async follow(): Promise<void> {
    let backoff = 500;
    while (!this.closed && this.status !== "incompatible") {
      try {
        const changes = await this.transport.changes({
          doc: this.docRevision,
          feed: this.feedRevision,
          timeoutMs: this.options.pollTimeoutMs ?? 20_000,
        });
        if (this.closed) return;
        if (changes.docRevision !== this.docRevision) await this.sync();
        if (changes.feedRevision !== this.feedRevision) await this.refreshView();
        backoff = 500;
      } catch (error) {
        this.fail(error);
        await this.wait(backoff);
        backoff = Math.min(backoff * 2, 10_000);
        // Catch up on anything missed while unreachable.
        await this.sync().catch(() => undefined);
        await this.refreshView();
      }
    }
  }

  private async afterFeedback(thread: DesignThread): Promise<DesignThread> {
    await this.refreshView();
    return thread;
  }

  async createThread(request: CreateThreadRequest): Promise<DesignThread> {
    return this.afterFeedback(await this.transport.createThread(request));
  }

  async reply(threadId: string, request: { commentId: string; body: string }): Promise<DesignThread> {
    return this.afterFeedback(await this.transport.reply(threadId, request));
  }

  async resolve(threadId: string, resolved: boolean): Promise<DesignThread> {
    return this.afterFeedback(await this.transport.resolve(threadId, resolved));
  }

  async retryDelivery(deliveryId: string): Promise<void> {
    await this.transport.retryDelivery(deliveryId);
    await this.refreshView();
  }
}

/** A client-chosen id for threads and comments (the server's idempotency key). */
export function newId(prefix: string): string {
  const random = globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return `${prefix}-${random.replace(/[^a-zA-Z0-9-]/g, "").slice(0, 36)}`;
}
