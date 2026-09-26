import { describe, expect, it } from "vitest";
import * as Y from "yjs";
import { DesignSession } from "./session";
import { MemoryDesignServer as FakeServer, MEMORY_SCHEMA as SCHEMA } from "./testing/memoryServer";
import { toThreadData, bodyText } from "./threadStore";


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
