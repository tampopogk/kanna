// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import ArtifactViewer from "../ArtifactViewer.vue";
import { setDesktopServerClientHandlersForTests } from "../../services/desktopServerClient";
import { artifactFrameUrl, resetArtifactPreviewHoldersForTests } from "../../utils/artifactPreview";

/** What the Tauri mock hands this webview as its local control credential. */
const CREDENTIAL = "mock-local-control-credential";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(async (command: string) => {
    if (command === "ensure_mobile_server") return undefined;
    if (command === "mobile_server_status") return { state: "running", lanPort: 48121 };
    if (command === "local_control_credential") return "mock-local-control-credential";
    throw new Error(`unexpected invoke: ${command}`);
  }),
}));
vi.mock("../../invoke", () => ({ invoke: mocks.invoke }));

const V1 = "1".repeat(40);
const V2 = "2".repeat(40);
const MISSING = "3".repeat(40);
const EXPIRED = "4".repeat(40);
const CAP1 = "a".repeat(32);
const CAP2 = "b".repeat(32);

function version(artifactId: string, previous?: string) {
  return {
    schemaVersion: 1, recordId: `v-${artifactId.slice(0, 4)}`, repoId: "repo-1", artifactId, kind: "mockup",
    entrypoint: "index.html", createdAt: "2026-09-23T10:00:00Z", ...(previous ? { previous } : {}),
    retention: "keep", producedBy: { taskId: "task-producer" }, fileCount: 3, totalBytes: 300,
    storage: { commit: "c".repeat(40), ref: `refs/kanna/artifacts/trees/${artifactId}` },
  };
}

function detail(artifactId: string, overrides: Record<string, unknown> = {}) {
  return {
    repoId: "repo-1", artifactId, retained: true,
    reference: { type: "stored", repoId: "repo-1", artifactId, kind: "mockup" },
    files: [{ path: "index.html", size: 120 }, { path: "css/site.css", size: 80 }, { path: "img/logo.png", size: 100 }],
    versions: [version(artifactId)], comments: [], decisions: [], ...overrides,
  };
}

const DETAILS: Record<string, unknown> = {
  [V2]: detail(V2, {
    versions: [version(V2, V1)],
    comments: [
      { schemaVersion: 1, recordId: "c-1", repoId: "repo-1", aboutArtifactId: V2, createdAt: "2026-09-23T11:00:00Z",
        author: "designer", body: "header is too tall", anchor: { path: "css/site.css", position: "line 3", excerpt: "height: 120px" } },
      // A record about another version must never be shown as this one's.
      { schemaVersion: 1, recordId: "c-stray", repoId: "repo-1", aboutArtifactId: V1, createdAt: "2026-09-23T11:00:00Z",
        author: "someone", body: "stray note about v1" },
    ],
    decisions: [{ schemaVersion: 1, recordId: "d-1", repoId: "repo-1", aboutArtifactId: V2, createdAt: "2026-09-23T12:00:00Z", who: "owner", what: "approved" }],
  }),
  [V1]: detail(V1, {
    comments: [{ schemaVersion: 1, recordId: "c-v1", repoId: "repo-1", aboutArtifactId: V1, createdAt: "2026-09-22T11:00:00Z",
      author: "designer", body: "first pass", anchor: { path: "index.html", position: "#hero", excerpt: "Welcome" } }],
  }),
  [EXPIRED]: detail(EXPIRED, { retained: false, files: [] }),
};
const PREVIEWS: Record<string, string> = {
  [V1]: `http://127.0.0.1:50101/a/${CAP1}/index.html`,
  [V2]: `http://127.0.0.1:50102/a/${CAP2}/index.html`,
};

let fetchMock: ReturnType<typeof vi.fn>;

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

function calls(): { method: string; path: string; headers: Record<string, string> }[] {
  return fetchMock.mock.calls.map(([url, init]) => ({
    method: (init as RequestInit).method ?? "GET",
    path: new URL(url as string).pathname,
    headers: (init as RequestInit).headers as Record<string, string>,
  }));
}

beforeEach(() => {
  resetArtifactPreviewHoldersForTests();
  setDesktopServerClientHandlersForTests({ ensureMobileServer: async () => {} });
  fetchMock = vi.fn(async (url: string, init: RequestInit) => {
    const path = new URL(url).pathname;
    const match = path.match(/^\/v1\/repos\/repo-1\/artifacts\/([0-9a-f]{40})(\/.*)?$/);
    if (!match) return json({ error: "unexpected" }, 500);
    const [, id, rest = ""] = match;
    if (rest === "" && DETAILS[id]) return json(DETAILS[id]);
    if (rest === "") return json({ error: "artifact_not_found", message: `artifact ${id} not found`, repoId: "repo-1", artifactId: id }, 404);
    if (rest === "/preview") return json({ repoId: "repo-1", artifactId: id, entrypoint: "index.html", url: PREVIEWS[id], expiresAt: 0, idleTimeoutSecs: 900 });
    if (rest === "/preview/close") return json({ closed: true });
    if (rest === "/files") {
      const text = "h1 {\n  color: blue;\n  height: 120px;\n}\n";
      return json({ repoId: "repo-1", artifactId: id, path: new URL(url).searchParams.get("path"), mediaType: "text/css; charset=utf-8", size: text.length, dataBase64: btoa(text) });
    }
    if (rest === "/decisions") {
      const body = JSON.parse(String(init.body));
      return json({ schemaVersion: 1, recordId: "d-new", repoId: "repo-1", aboutArtifactId: id, createdAt: "2026-09-23T13:00:00Z", ...body }, 201);
    }
    if (rest === "/comments") {
      const body = JSON.parse(String(init.body));
      return json({ schemaVersion: 1, recordId: "c-new", repoId: "repo-1", aboutArtifactId: id, createdAt: "2026-09-23T13:00:00Z", ...body }, 201);
    }
    return json({ error: "unexpected" }, 500);
  });
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  setDesktopServerClientHandlersForTests(null);
  vi.unstubAllGlobals();
});

/** The client resolves the server address through a dynamic import, so wait on the view, not a tick. */
async function settle(wrapper: ReturnType<typeof mount>) {
  await vi.waitFor(() => {
    expect(wrapper.text()).not.toMatch(/Reading artifact|Opening preview/);
  });
  await flushPromises();
}

async function mountAt(artifactId: string) {
  const wrapper = mount(ArtifactViewer, { props: { repoId: "repo-1", artifactId, visible: true } });
  await settle(wrapper);
  return wrapper;
}

async function expectRequested(request: string) {
  await vi.waitFor(() => {
    expect(calls().map(call => `${call.method} ${call.path}`)).toContain(request);
  });
}

describe("ArtifactViewer", () => {
  it("opens an artifact by repository and tree id in the store's preview listener", async () => {
    const wrapper = await mountAt(V2);
    expect(calls().slice(0, 2).map(call => `${call.method} ${call.path}`)).toEqual([
      `GET /v1/repos/repo-1/artifacts/${V2}`,
      `POST /v1/repos/repo-1/artifacts/${V2}/preview`,
    ]);
    // No task route and no dev-server port is involved in opening it.
    expect(calls().some(call => call.path.startsWith("/v1/tasks/"))).toBe(false);
    const frame = wrapper.get('[data-testid="artifact-frame"]');
    expect(frame.attributes("src")).toBe(PREVIEWS[V2]);
    expect(wrapper.get('[data-testid="artifact-current-id"]').text()).toBe(V2.slice(0, 12));
  });

  it("frames the entrypoint under the capability so the page's relative assets resolve inside the tree", async () => {
    const wrapper = await mountAt(V2);
    const src = wrapper.get('[data-testid="artifact-frame"]').attributes("src")!;
    const base = `http://127.0.0.1:50102/a/${CAP2}/`;
    expect(new URL("css/site.css", src).toString()).toBe(`${base}css/site.css`);
    expect(new URL("img/logo.png", src).toString()).toBe(`${base}img/logo.png`);
    expect(new URL("../css/site.css", new URL("pages/about.html", src)).toString()).toBe(`${base}css/site.css`);
    // Selecting a comment's anchor shows that exact file of the same tree: a
    // stylesheet as its text, with the anchored line marked.
    await wrapper.get('[data-testid="artifact-anchor"]').trigger("click");
    await vi.waitFor(() => expect(wrapper.find('[data-testid="artifact-source"]').exists()).toBe(true));
    expect(calls().map(call => `${call.method} ${call.path}`)).toContain(`GET /v1/repos/repo-1/artifacts/${V2}/files`);
    expect(wrapper.get('[data-testid="artifact-source"] .anchored').text()).toBe("height: 120px;");
    expect(wrapper.get('[data-testid="artifact-source"] .anchored').attributes("data-line")).toBe("3");
  });

  it("follows the previous link to the older tree id and comes back", async () => {
    const wrapper = await mountAt(V2);
    await wrapper.get('[data-testid="artifact-previous"]').trigger("click");
    await settle(wrapper);
    expect(wrapper.get('[data-testid="artifact-current-id"]').text()).toBe(V1.slice(0, 12));
    expect(wrapper.get('[data-testid="artifact-frame"]').attributes("src")).toBe(PREVIEWS[V1]);
    await expectRequested(`POST /v1/repos/repo-1/artifacts/${V2}/preview/close`);
    expect(wrapper.get('[data-testid="artifact-previous"]').attributes("disabled")).toBeDefined();
    // The older version shows its own comments, not the newer one's.
    expect(wrapper.findAll('[data-testid="artifact-comment"]').map(comment => comment.text())).toEqual([
      expect.stringContaining("first pass"),
    ]);
    await wrapper.get('[data-testid="artifact-newer"]').trigger("click");
    await settle(wrapper);
    expect(wrapper.get('[data-testid="artifact-current-id"]').text()).toBe(V2.slice(0, 12));
  });

  it("shows comment anchors and decisions recorded on the exact version only", async () => {
    const wrapper = await mountAt(V2);
    const comments = wrapper.findAll('[data-testid="artifact-comment"]');
    expect(comments).toHaveLength(1);
    expect(comments[0].text()).toContain("header is too tall");
    expect(wrapper.text()).not.toContain("stray note about v1");
    expect(wrapper.get('[data-testid="artifact-anchor-path"]').text()).toBe("css/site.css");
    expect(wrapper.get('[data-testid="artifact-anchor-position"]').text()).toContain("line 3");
    expect(wrapper.get('[data-testid="artifact-anchor-excerpt"]').text()).toBe("height: 120px");
    const decisions = wrapper.findAll('[data-testid="artifact-decision"]');
    expect(decisions).toHaveLength(1);
    expect(decisions[0].text()).toContain("owner");
    expect(decisions[0].text()).toContain("approved");
  });

  it("records a decision as data about the tree id and calls no task or gate operation", async () => {
    const wrapper = await mountAt(V2);
    const form = wrapper.get('[data-testid="artifact-decision-form"]');
    const [who, what] = form.findAll("input");
    await who.setValue("stakeholder");
    await what.setValue("rejected: needs contrast");
    await form.trigger("submit");
    await vi.waitFor(() => expect(wrapper.findAll('[data-testid="artifact-decision"]')).toHaveLength(2));
    const writes = calls().filter(call => call.method === "POST" && !call.path.endsWith("/preview"));
    expect(writes.map(call => call.path)).toEqual([`/v1/repos/repo-1/artifacts/${V2}/decisions`]);
    expect(fetchMock.mock.calls.find(([url]) => String(url).endsWith("/decisions"))?.[1].body)
      .toBe(JSON.stringify({ who: "stakeholder", what: "rejected: needs contrast" }));
    expect(wrapper.findAll('[data-testid="artifact-decision"]').at(-1)?.text()).toContain("rejected: needs contrast");
  });

  it("says a missing object is missing and opens nothing", async () => {
    const wrapper = await mountAt(MISSING);
    const state = wrapper.get('[data-testid="artifact-unavailable"]');
    expect(state.attributes("data-reason")).toBe("not-found");
    expect(state.text()).toContain("No artifact with this id");
    expect(state.text()).toContain(MISSING);
    expect(wrapper.find('[data-testid="artifact-frame"]').exists()).toBe(false);
    expect(calls().some(call => call.path.endsWith("/preview"))).toBe(false);
  });

  it("renders a descriptor that is no longer retained as expired without opening a preview", async () => {
    const wrapper = await mountAt(EXPIRED);
    expect(wrapper.get('[data-testid="artifact-expired"]').text()).toContain("Produced, no longer retained");
    expect(wrapper.find('[data-testid="artifact-frame"]').exists()).toBe(false);
    expect(calls().some(call => call.path.endsWith("/preview"))).toBe(false);
  });

  it("isolates shared HTML: no credential, no same-origin, no top navigation, no popups, no host bridge", async () => {
    const wrapper = await mountAt(V2);
    // The host really holds the credential and uses it for control requests…
    expect(calls()[0].headers.Authorization).toBe(`Bearer ${CREDENTIAL}`);
    const frame = wrapper.get('[data-testid="artifact-frame"]');
    const element = frame.element as HTMLIFrameElement;
    // …and none of it reaches the frame's address or attributes.
    for (const attribute of Array.from(element.attributes)) {
      expect(attribute.value).not.toContain(CREDENTIAL);
    }
    expect(frame.attributes("sandbox")).toBe("allow-scripts");
    for (const forbidden of ["allow-same-origin", "allow-top-navigation", "allow-popups", "allow-forms", "allow-modals"]) {
      expect(frame.attributes("sandbox")).not.toContain(forbidden);
    }
    expect(frame.attributes("allow")).toBe("");
    expect(frame.attributes("referrerpolicy")).toBe("no-referrer");
    // A separate loopback origin, never the control API's.
    const src = new URL(frame.attributes("src")!);
    expect(src.origin).not.toBe("http://127.0.0.1:48121");
    expect(src.origin).not.toBe(window.location.origin);
    expect(src.search).toBe("");
  });

  it("refuses to frame anything but the artifact listener, whatever the server answers", () => {
    for (const hostile of [
      "http://127.0.0.1:48121/v1/tasks",
      `http://localhost:50102/a/${CAP2}/index.html`,
      `https://127.0.0.1:50102/a/${CAP2}/index.html`,
      `http://127.0.0.1:50102/a/${CAP2}/index.html?token=${CREDENTIAL}`,
      "javascript:alert(1)",
      `http://evil.example/a/${CAP2}/index.html`,
    ]) {
      expect(() => artifactFrameUrl(hostile, null), hostile).toThrow();
    }
  });

  it("releases the preview listener when the viewer goes away", async () => {
    const wrapper = await mountAt(V2);
    wrapper.unmount();
    await expectRequested(`POST /v1/repos/repo-1/artifacts/${V2}/preview/close`);
  });
});
