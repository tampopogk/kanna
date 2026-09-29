import * as Y from "yjs";
import { fromBase64, toBase64 } from "../session";
import {
  DesignRequestError,
  type DesignThread,
  type DesignTransport,
  type DesignView,
} from "../types";

/** A schema id tests share; the memory server accepts only this one. */
export let MEMORY_SCHEMA = "test-schema";
export function setMemorySchema(schema: string) {
  MEMORY_SCHEMA = schema;
}

/** kanna-server in miniature: one authoritative document and a feed. */
export class MemoryDesignServer {
  doc = new Y.Doc();
  revision = 0;
  feed = 0;
  threads: DesignThread[] = [];
  /** Every thread creation request, in order, as the server received it. */
  created: Array<import("../types").CreateThreadRequest> = [];
  down = false;
  waiters: Array<() => void> = [];

  view(): DesignView {
    return {
      taskId: "t",
      stage: "design",
      nextStage: "plan",
      currentStage: "design",
      inDesignStage: true,
      stageChain: ["design", "plan"],
      epoch: 1,
      status: "designing",
      position: "static",
      positions: [],
      schemaVersion: MEMORY_SCHEMA,
      docRevision: this.revision,
      feedRevision: this.feed,
      threads: this.threads,
      approval: null,
      agentRuntime: "idle",
      scratchRepository: null,
    };
  }

  wake() {
    for (const waiter of this.waiters.splice(0)) waiter();
  }

  transport(): DesignTransport {
    const check = () => {
      if (this.down) throw new Error("offline");
    };
    return {
      view: async () => {
        check();
        return this.view();
      },
      changes: async (known) => {
        check();
        if (known.doc === this.revision && known.feed === this.feed) {
          await new Promise<void>((resolve) => {
            this.waiters.push(resolve);
            setTimeout(resolve, 50);
          });
        }
        check();
        return { docRevision: this.revision, feedRevision: this.feed };
      },
      sync: async (request) => {
        check();
        if (request.schemaVersion !== MEMORY_SCHEMA) throw new DesignRequestError("schema", 422, "schema");
        if (request.update) {
          const before = Y.encodeStateVector(this.doc);
          Y.applyUpdate(this.doc, fromBase64(request.update));
          if (toBase64(Y.encodeStateVector(this.doc)) !== toBase64(before)) {
            this.revision += 1;
            this.wake();
          }
        }
        return {
          update: toBase64(Y.encodeStateAsUpdate(this.doc, fromBase64(request.stateVector))),
          stateVector: toBase64(Y.encodeStateVector(this.doc)),
          revision: this.revision,
        };
      },
      createThread: async (request) => {
        check();
        this.created.push(request);
        const thread: DesignThread = {
          id: request.threadId,
          number: this.threads.length + 1,
          kind: request.kind,
          status: "open",
          anchor: !request.anchor
            ? null
            : "element" in request.anchor
              ? { blockId: null, quotedText: request.anchor.element.excerpt, state: "attached", element: request.anchor.element }
              : { blockId: request.anchor.blockId, quotedText: request.anchor.quotedText, state: "attached" },
          comments: [
            { id: request.commentId, author: "operator", body: request.body, createdAt: new Date(0).toISOString(), delivery: { id: "d", state: "queued" } },
          ],
          deliveryStatus: "queued",
          createdAt: new Date(0).toISOString(),
          resolvedAt: null,
          resolvedBy: null,
        };
        if (!this.threads.some((existing) => existing.id === thread.id)) this.threads.push(thread);
        this.feed += 1;
        this.wake();
        return thread;
      },
      reply: async () => {
        throw new Error("unused");
      },
      resolve: async (threadId, resolved) => {
        const thread = this.threads.find((candidate) => candidate.id === threadId)!;
        thread.status = resolved ? "resolved" : "open";
        this.feed += 1;
        this.wake();
        return thread;
      },
      retryDelivery: async () => undefined,
    };
  }
}

