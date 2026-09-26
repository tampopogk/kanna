import { describe, expect, it } from "vitest";
import * as Y from "yjs";
import { DesignSession, fromBase64, toBase64 } from "./session";
import { toThreadData, bodyText } from "./threadStore";
import {
  DesignRequestError,
  type DesignThread,
  type DesignTransport,
  type DesignView,
} from "./types";

const SCHEMA = "test-schema";

/** kanna-server in miniature: one authoritative document and a feed. */
class FakeServer {
  doc = new Y.Doc();
  revision = 0;
  feed = 0;
  threads: DesignThread[] = [];
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
      schemaVersion: SCHEMA,
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
        if (request.schemaVersion !== SCHEMA) throw new DesignRequestError("schema", 422, "schema");
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
        const thread: DesignThread = {
          id: request.threadId,
          number: this.threads.length + 1,
          kind: request.kind,
          status: "open",
          anchor: null,
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

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(check: () => boolean, label: string) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    if (check()) return;
    await wait(10);
  }
  throw new Error(`timed out waiting for ${label}`);
}

describe("DesignSession", () => {
  it("keeps two clients' concurrent edits and converges through the server", async () => {
    const server = new FakeServer();
    const a = new DesignSession(server.transport(), { schemaVersion: SCHEMA, flushDelayMs: 5, wait: async () => { await wait(5); } });
    const b = new DesignSession(server.transport(), { schemaVersion: SCHEMA, flushDelayMs: 5, wait: async () => { await wait(5); } });
    await a.start();
    await b.start();
    a.doc.getText("t").insert(0, "from a ");
    b.doc.getText("t").insert(0, "from b ");
    await a.whenSaved();
    await b.whenSaved();
    await until(
      () => a.doc.getText("t").toString() === b.doc.getText("t").toString() && a.doc.getText("t").length === 14,
      "convergence",
    );
    expect(server.doc.getText("t").toString()).toBe(a.doc.getText("t").toString());
    expect(a.status).toBe("synced");
    a.close();
    b.close();
  });

  it("keeps edits made while offline and sends them on reconnect", async () => {
    const server = new FakeServer();
    const session = new DesignSession(server.transport(), { schemaVersion: SCHEMA, flushDelayMs: 5, wait: async () => { await wait(5); } });
    await session.start();
    server.down = true;
    session.doc.getText("t").insert(0, "typed offline");
    await expect(session.whenSaved()).rejects.toThrow("offline");
    expect(session.status).toBe("offline");
    expect(session.hasUnsavedChanges).toBe(true);
    server.down = false;
    await session.whenSaved();
    expect(server.doc.getText("t").toString()).toBe("typed offline");
    expect(session.status).toBe("synced");
    session.close();
  });

  it("stops editing when the server refuses its schema", async () => {
    const server = new FakeServer();
    const session = new DesignSession(server.transport(), { schemaVersion: "older-schema", wait: async () => { await wait(5); } });
    await expect(session.start()).rejects.toThrow();
    expect(session.status).toBe("incompatible");
    session.close();
  });

  it("follows the feed and a read-only viewer never sends updates", async () => {
    const server = new FakeServer();
    const viewer = new DesignSession(server.transport(), { schemaVersion: SCHEMA, readOnly: true, wait: async () => { await wait(5); } });
    await viewer.start();
    viewer.doc.getText("t").insert(0, "local only");
    await viewer.whenSaved();
    expect(server.doc.getText("t").toString()).toBe("");
    const writer = new DesignSession(server.transport(), { schemaVersion: SCHEMA, wait: async () => { await wait(5); } });
    await writer.start();
    await writer.createThread({ threadId: "th-1", commentId: "cm-1", kind: "message", body: "hello" });
    await until(() => viewer.view?.threads.length === 1, "the viewer's feed");
    viewer.close();
    writer.close();
  });
});

describe("thread conversion", () => {
  it("maps server threads to BlockNote threads without losing number, kind or delivery", () => {
    const data = toThreadData({
      id: "th-1",
      number: 3,
      kind: "comment",
      status: "resolved",
      anchor: { blockId: "b", quotedText: "q", state: "attached" },
      comments: [
        { id: "c1", author: "operator", body: "change it", createdAt: "2026-09-26T10:00:00.000Z", delivery: { id: "d", state: "delivered" } },
        { id: "c2", author: "agent", body: "done", createdAt: "2026-09-26T10:01:00.000Z" },
      ],
      deliveryStatus: "agent_replied",
      createdAt: "2026-09-26T10:00:00.000Z",
      resolvedAt: "2026-09-26T10:02:00.000Z",
      resolvedBy: "agent",
    });
    expect(data.resolved).toBe(true);
    expect(data.metadata).toEqual({ number: 3, kind: "comment", deliveryStatus: "agent_replied" });
    expect(data.comments.map((comment) => comment.userId)).toEqual(["person", "agent"]);
    expect(bodyText(data.comments[1].body)).toBe("done");
  });

  it("reads comment bodies as plain text", () => {
    expect(bodyText([{ type: "paragraph", content: [{ type: "text", text: "one " }, { type: "text", text: "line" }] }, { type: "paragraph", content: "two" }])).toBe("one line\ntwo");
    expect(bodyText(undefined)).toBe("");
  });
});
