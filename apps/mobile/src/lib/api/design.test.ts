import { describe, expect, it, vi } from "vitest";
import { designRoute, designTaskPath, parseDesignOperation } from "./design";
import { createLanTransport } from "../transports/lanTransport";
import { createRemoteTransport, type RemoteDesktopInvoker } from "../transports/remoteTransport";
import { buildDesignDocument, designPageMessageScript } from "../../screens/buildDesignDocument";
import { readDesignBridgeMessage } from "./design";
import { designButtonLabel } from "../../screens/designPresentation";

type FetchLike = Parameters<typeof createLanTransport>[1];

describe("design operations the phone page may ask for", () => {
  it("accepts exactly the known shapes and maps each to one design route", () => {
    const comment = parseDesignOperation("createThread", [
      {
        threadId: "th-1",
        commentId: "cm-1",
        kind: "comment",
        body: "tighten this",
        anchor: { blockId: "b1", quotedText: "the text", stateVector: "AQID" },
        extra: "dropped",
      },
    ]);
    expect(comment).toEqual({
      op: "createThread",
      request: {
        threadId: "th-1",
        commentId: "cm-1",
        kind: "comment",
        body: "tighten this",
        anchor: { blockId: "b1", quotedText: "the text", stateVector: "AQID" },
      },
    });
    expect(designRoute(comment!)).toEqual({ method: "POST", subpath: "/threads", body: comment!.request });
    expect(designTaskPath("task/1", designRoute({ op: "view" }))).toBe("/v1/tasks/task%2F1/design");
    expect(designRoute(parseDesignOperation("changes", [{ doc: 3, feed: -1, timeoutMs: 8000 }])!).subpath).toBe(
      "/changes?doc=3&timeoutMs=8000",
    );
    expect(designRoute(parseDesignOperation("resolve", ["th-1", false])!)).toEqual({
      method: "POST",
      subpath: "/threads/th-1/resolve",
      body: { resolved: false },
    });
    expect(designRoute(parseDesignOperation("sync", [{ schemaVersion: "s", stateVector: "", update: "AA==" }])!)).toEqual({
      method: "POST",
      subpath: "/document/sync",
      body: { schemaVersion: "s", stateVector: "", update: "AA==" },
    });
  });

  it("refuses anything else before a request is made", () => {
    for (const [op, args] of [
      ["deleteTask", []],
      ["view", ["extra"]],
      ["resolve", ["../../../v1/tasks", true]],
      ["reply", ["th 1", { commentId: "cm", body: "x" }]],
      ["retryDelivery", ["dl/../x"]],
      ["createThread", [{ threadId: "th", commentId: "cm", kind: "approval", body: "x" }]],
      ["createThread", [{ threadId: "th", commentId: "cm", kind: "message", body: 5 }]],
      ["sync", [{ schemaVersion: "s", stateVector: "not base64!" }]],
      ["changes", [{ timeoutMs: 60_000 }]],
      ["view", "not-an-array"],
    ] as const) {
      expect(parseDesignOperation(op, args), `${op} ${JSON.stringify(args)}`).toBeNull();
    }
  });
});

describe("design requests over each transport", () => {
  it("LAN: posts the operation's body to the task's design route as the paired device", async () => {
    const fetchImpl = vi.fn<FetchLike>().mockResolvedValue({ ok: true, status: 200, json: async () => ({ id: "th-1" }) });
    const transport = createLanTransport("http://127.0.0.1:48120", fetchImpl, undefined, {
      deviceCredentials: { deviceId: "phone-1", deviceSecret: "lan-secret" },
    });
    await transport.requestDesign!("task-1", {
      op: "reply",
      threadId: "th-1",
      request: { commentId: "cm-2", body: "shorter" },
    });
    const [url, init] = fetchImpl.mock.calls[0];
    expect(url).toBe("http://127.0.0.1:48120/v1/tasks/task-1/design/threads/th-1/comments");
    expect(init?.method).toBe("POST");
    expect(JSON.parse(String(init?.body))).toEqual({ commentId: "cm-2", body: "shorter" });
  });

  it("LAN: refuses without a paired device", async () => {
    const fetchImpl = vi.fn<FetchLike>();
    const transport = createLanTransport("http://127.0.0.1:48120", fetchImpl);
    await expect(transport.requestDesign!("task-1", { op: "view" })).rejects.toThrow(/paired device/);
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it("relay: routes a cloud task's design request to its owner desktop and local id", async () => {
    const invokeDesktop = vi.fn<RemoteDesktopInvoker>().mockResolvedValue({ docRevision: 2, feedRevision: 5 });
    const transport = createRemoteTransport({
      listDesktopRecords: async () => [],
      getSelectedDesktopId: () => null,
      invokeDesktop,
      listCloudTasks: async () => [
        {
          id: "cloud-task-1",
          repoId: "repo-1",
          title: "Design task",
          stage: "design",
          ownerDesktopId: "desktop-owner",
          ownerLocalTaskId: "local/task-1",
          ownerOnline: true,
        },
      ],
    });
    await expect(
      transport.requestDesign!("cloud-task-1", { op: "changes", known: { doc: 1, feed: 4, timeoutMs: 8000 } }),
    ).resolves.toEqual({ docRevision: 2, feedRevision: 5 });
    expect(invokeDesktop).toHaveBeenCalledWith({
      desktopId: "desktop-owner",
      method: "GET",
      path: "/v1/tasks/local%2Ftask-1/design/changes?doc=1&feed=4&timeoutMs=8000",
      body: null,
    });
  });
});

describe("the design page", () => {
  it("can reach nothing but the app: no network, no origin, no credential", () => {
    const html = buildDesignDocument({ theme: "dark" });
    expect(html).toContain("connect-src 'none'");
    expect(html).toContain("default-src 'none'");
    expect(html).toContain('data-theme="dark"');
    expect(html).not.toMatch(/Bearer|deviceSecret|x-kanna-device/i);
    // Inlined code cannot close its own element early: the only closing tags
    // are the page's own two scripts and one stylesheet.
    expect(html.match(/<\/script/gi)).toHaveLength(2);
    expect(html.match(/<\/style/gi)).toHaveLength(1);
  });

  it("reads only well-formed bridge messages and never lets the page pick the task", () => {
    expect(readDesignBridgeMessage("not json")).toBeNull();
    expect(readDesignBridgeMessage(JSON.stringify({ kind: "other", type: "request" }))).toBeNull();
    expect(readDesignBridgeMessage(JSON.stringify({ kind: "kanna-design", type: "ready" }))).toEqual({ type: "ready" });
    const request = readDesignBridgeMessage(
      JSON.stringify({ kind: "kanna-design", type: "request", id: 7, op: "view", args: [], taskId: "someone-else" }),
    );
    expect(request).toEqual({ type: "request", id: 7, operation: { op: "view" } });
    const refused = readDesignBridgeMessage(
      JSON.stringify({ kind: "kanna-design", type: "request", id: 8, op: "advanceStage", args: [] }),
    );
    expect(refused).toEqual({ type: "request", id: 8, operation: null });
    expect(designPageMessageScript({ type: "resume" })).toBe(
      'window.__kannaDesign && window.__kannaDesign.receive({"type":"resume"}); true;',
    );
  });

  it("labels the way in with the position and what needs attention", () => {
    expect(
      designButtonLabel({
        stage: "design",
        inDesignStage: true,
        status: "designing",
        position: "prototype",
        positions: [{ name: "prototype", label: "Prototype" }],
        nextStage: "plan",
        openThreads: 2,
        waitingFeedback: 1,
        uncertainFeedback: 1,
        approvalPhase: null,
      }),
    ).toBe("Design · Prototype · 2 open · 1 not delivered ›");
  });
});
