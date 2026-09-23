import React from "react";
import { act, create, type ReactTestInstance, type ReactTestRenderer } from "react-test-renderer";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ArtifactDetail, ArtifactFileContent } from "../lib/api/types";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const linking = vi.hoisted(() => ({ openURL: vi.fn(), canOpenURL: vi.fn() }));

vi.mock("react-native", () => ({
  ActivityIndicator: "ActivityIndicator",
  Linking: linking,
  Modal: "Modal",
  Pressable: "Pressable",
  SafeAreaView: "SafeAreaView",
  ScrollView: "ScrollView",
  StyleSheet: { create: <T extends Record<string, unknown>>(styles: T) => styles },
  Text: "Text",
  TextInput: "TextInput",
  View: "View"
}));

vi.mock("react-native-webview", () => ({ WebView: "WebView" }));

import {
  ArtifactViewer,
  resetConfirmedArtifactRemotesForTests,
  shouldStartArtifactLoad,
  type ArtifactViewerActions
} from "./ArtifactViewer";
import {
  ARTIFACT_DOCUMENT_POLICY,
  decodeBase64,
  decodeUtf8,
  encodeBase64,
  encodeUtf8
} from "./buildArtifactDocument";

const V1 = "1".repeat(40);
const V2 = "2".repeat(40);
const MISSING = "3".repeat(40);
const EXPIRED = "4".repeat(40);

function version(artifactId: string, previous?: string) {
  return {
    schemaVersion: 1, recordId: `v-${artifactId.slice(0, 4)}`, repoId: "repo-1", artifactId,
    kind: "mockup" as const, entrypoint: "index.html", createdAt: "2026-09-23T10:00:00Z",
    ...(previous ? { previous } : {}), retention: "keep" as const, producedBy: { taskId: "task-producer" },
    fileCount: 3, totalBytes: 300, storage: { commit: "c".repeat(40), ref: `refs/kanna/artifacts/trees/${artifactId}` }
  };
}

function detail(artifactId: string, overrides: Partial<ArtifactDetail> = {}): ArtifactDetail {
  return {
    repoId: "repo-1", artifactId, retained: true,
    reference: { type: "stored", repoId: "repo-1", artifactId, kind: "mockup" },
    files: [{ path: "index.html", size: 1 }, { path: "css/site.css", size: 1 }],
    versions: [version(artifactId)], comments: [], decisions: [], ...overrides
  };
}

const DETAILS: Record<string, ArtifactDetail> = {
  [V2]: detail(V2, {
    versions: [version(V2, V1)],
    files: [
      { path: "index.html", size: 1 },
      { path: "css/site.css", size: 1 },
      { path: "pages/about.html", size: 1 }
    ],
    comments: [
      { schemaVersion: 1, recordId: "c-1", repoId: "repo-1", aboutArtifactId: V2, createdAt: "2026-09-23T11:00:00Z",
        author: "designer", body: "header is too tall", anchor: { path: "css/site.css", position: "line 2", excerpt: "height: 120px" } },
      { schemaVersion: 1, recordId: "c-stray", repoId: "repo-1", aboutArtifactId: V1, createdAt: "2026-09-23T11:00:00Z",
        author: "someone", body: "stray note about v1" }
    ],
    decisions: [{ schemaVersion: 1, recordId: "d-1", repoId: "repo-1", aboutArtifactId: V2, createdAt: "2026-09-23T12:00:00Z", who: "owner", what: "approved" }]
  }),
  [V1]: detail(V1, {
    comments: [{ schemaVersion: 1, recordId: "c-v1", repoId: "repo-1", aboutArtifactId: V1, createdAt: "2026-09-22T11:00:00Z",
      author: "designer", body: "first pass" }]
  }),
  [EXPIRED]: detail(EXPIRED, { retained: false, files: [] })
};

const FILES: Record<string, Record<string, string>> = {
  [V2]: {
    "index.html": `<link rel="stylesheet" href="css/site.css"><h1>Version two</h1><a href="pages/about.html">About</a>`,
    "css/site.css": "h1 {\n  height: 120px;\n}\n",
    "pages/about.html": `<link rel="stylesheet" href="../css/site.css"><h1>About v2</h1>`
  },
  [V1]: {
    "index.html": `<link rel="stylesheet" href="css/site.css"><h1>Version one</h1>`,
    "css/site.css": "h1 { height: 80px; }"
  }
};

function client() {
  const getArtifact = vi.fn(async (_repoId: string, artifactId: string) => {
    const found = DETAILS[artifactId];
    if (!found) throw new Error(`Remote request failed (404): {"error":"artifact_not_found","artifactId":"${artifactId}"}`);
    return found;
  });
  const readArtifactFile = vi.fn(async (repoId: string, artifactId: string, path: string): Promise<ArtifactFileContent> => {
    const text = FILES[artifactId]?.[path];
    if (text === undefined) throw new Error(`Remote request failed (404): {"error":"artifact_file_not_found"}`);
    const bytes = encodeUtf8(text);
    return {
      repoId, artifactId, path, size: bytes.length, dataBase64: encodeBase64(bytes),
      mediaType: path.endsWith(".css") ? "text/css; charset=utf-8" : "text/html; charset=utf-8"
    };
  });
  return { getArtifact, readArtifactFile };
}

let renderer: ReactTestRenderer | null = null;

afterEach(() => {
  act(() => renderer?.unmount());
  renderer = null;
  linking.openURL.mockClear();
});

async function flush() {
  for (let index = 0; index < 10; index += 1) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  }
}

async function open(artifactId: string) {
  const api = client();
  await act(async () => {
    renderer = create(
      <ArtifactViewer
        repoId="repo-1"
        initialArtifactId={artifactId}
        getArtifact={api.getArtifact}
        readArtifactFile={api.readArtifactFile}
        onClose={() => undefined}
      />
    );
  });
  await flush();
  return api;
}

function byTestId(testID: string): ReactTestInstance[] {
  return renderer!.root.findAll((node) => node.props.testID === testID && typeof node.type === "string");
}

function text(node: ReactTestInstance): string {
  return node.findAll(() => true)
    .flatMap((child) => child.children.filter((value): value is string => typeof value === "string"))
    .join("");
}

function webView(): ReactTestInstance {
  const [view] = renderer!.root.findAll((node) => node.type === "WebView");
  expect(view).toBeDefined();
  return view;
}

interface HostRun {
  frame: { contentWindow: object; srcdoc: string; attributes: Record<string, string> };
  /** Where the host navigated the top-level document, if it did. */
  navigations: string[];
  /** Deliver a message to the host, as if sent by `source` (the frame by default). */
  post(data: unknown, source?: object): void;
}

/**
 * Run the host document's own script against a stand-in window, document and
 * location: the page it shows, the frame it fills, the message listener it
 * installs and any top-level navigation it makes. Only the script the WebView
 * would execute is used.
 */
function runHost(host = webView().props.source.html as string): HostRun {
  const dataText = host.match(/<script type="application\/json" id="kanna-artifact-host">([\s\S]*?)<\/script>/)?.[1];
  const scripts = [...host.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((match) => match[1]);
  expect(dataText).toBeDefined();
  expect(scripts).toHaveLength(1);
  const frame = {
    contentWindow: {},
    srcdoc: "",
    attributes: {} as Record<string, string>,
    getAttribute(name: string) { return this.attributes[name] ?? null; },
    setAttribute(name: string, value: string) { this.attributes[name] = value; }
  };
  const navigations: string[] = [];
  const listeners: Array<(event: { data: unknown; source: object }) => void> = [];
  const hostWindow = { addEventListener: (_type: string, listener: (typeof listeners)[number]) => listeners.push(listener) };
  const hostDocument = {
    getElementById: (id: string) =>
      id === "kanna-artifact-host" ? { textContent: dataText } : id === "kanna-artifact-frame" ? frame : null
  };
  const hostLocation = {
    set href(value: string) { navigations.push(value); },
    get href() { return "about:blank"; }
  };
  new Function("window", "document", "location", scripts[0])(hostWindow, hostDocument, hostLocation);
  return {
    frame,
    navigations,
    post: (data, source = frame.contentWindow) => listeners.forEach((listener) => listener({ data, source }))
  };
}

/** The page the host shows. */
function framedPage(): string {
  return runHost().frame.srcdoc;
}

/** Let the WebView ask the viewer about a navigation, as the native side does. */
async function navigate(url: string): Promise<boolean> {
  let allowed = true;
  await act(async () => {
    allowed = webView().props.onShouldStartLoadWithRequest({ url });
  });
  await flush();
  return allowed;
}

function inlinedStylesheet(html: string): string {
  const url = html.match(/href="data:text\/css[^,]*,([^"]+)"/)?.[1] ?? "";
  return decodeUtf8(decodeBase64(url));
}

async function press(testID: string) {
  const [button] = byTestId(testID);
  await act(async () => {
    button.props.onPress();
  });
  await flush();
}

describe("ArtifactViewer (mobile)", () => {
  it("opens an artifact by repository and tree id and renders its relative assets", async () => {
    const api = await open(V2);
    expect(api.getArtifact).toHaveBeenCalledWith("repo-1", V2);
    // The entry and its own stylesheet; nothing it merely links to.
    expect(api.readArtifactFile.mock.calls.map((call) => call.slice(1))).toEqual([
      [V2, "index.html"],
      [V2, "css/site.css"]
    ]);
    const html = framedPage();
    expect(html).toContain("Version two");
    expect(inlinedStylesheet(html)).toContain("height: 120px");
    expect(text(byTestId("artifact-viewer-current-id")[0])).toContain(V2.slice(0, 12));
  });

  it("opens an in-tree link through the trusted host, reading that page only then", async () => {
    const api = await open(V2);
    expect(api.readArtifactFile).not.toHaveBeenCalledWith("repo-1", V2, "pages/about.html");
    const host = runHost();
    expect(host.frame.srcdoc).toContain(`href="#kanna-artifact=pages/about.html"`);
    // The page's own click handler asks its parent for the path.
    expect(host.frame.srcdoc).toContain(`parent.postMessage({kind:"kanna-artifact-navigate",path:p},"*")`);
    host.post({ kind: "kanna-artifact-navigate", path: "pages/about.html" });
    // The host, not the page, asks the viewer, by a top-level navigation.
    expect(host.navigations).toEqual(["kanna-host:open?path=pages%2Fabout.html"]);
    // The viewer refuses that navigation and renders the file itself.
    expect(await navigate(host.navigations[0])).toBe(false);
    expect(api.readArtifactFile).toHaveBeenCalledWith("repo-1", V2, "pages/about.html");
    const about = runHost();
    expect(about.frame.attributes["data-path"]).toBe("pages/about.html");
    expect(about.frame.srcdoc).toContain("About v2");
    expect(inlinedStylesheet(about.frame.srcdoc)).toContain("height: 120px");
    expect(linking.openURL).not.toHaveBeenCalled();
  });

  it("refuses every request on the host channel but a file of this tree from its own frame", async () => {
    await open(V2);
    const host = runHost();
    for (const forged of [
      { kind: "kanna-artifact-navigate", path: "../outside.html" },
      { kind: "kanna-artifact-navigate", path: "https://evil.example/" },
      { kind: "kanna-artifact-navigate", path: "javascript:alert(1)" },
      { kind: "kanna-artifact-navigate", path: "/pages/about.html" },
      { kind: "kanna-artifact-navigate", path: "__proto__" },
      { kind: "kanna-artifact-navigate", path: "constructor" },
      { kind: "kanna-artifact-navigate", path: "missing.html" },
      { kind: "kanna-artifact-navigate", path: "index.html" },
      { kind: "kanna-artifact-navigate", path: ["pages/about.html"] },
      { kind: "kanna-artifact-navigate" },
      { kind: "ReactNativeWebView", path: "pages/about.html" },
      "kanna-artifact-navigate:pages/about.html",
      null
    ]) {
      host.post(forged);
      expect(host.navigations, JSON.stringify(forged)).toEqual([]);
    }
    // A valid request from anything but its own frame is ignored too.
    host.post({ kind: "kanna-artifact-navigate", path: "pages/about.html" }, {});
    expect(host.navigations).toEqual([]);
    expect(host.frame.attributes["data-path"]).toBe("index.html");
    // The viewer checks the path again: a host-open for anything but a file
    // of this tree reads nothing.
    const api = client();
    const before = api.readArtifactFile.mock.calls.length;
    for (const url of [
      "kanna-host:open?path=..%2Foutside.html",
      "kanna-host:open?path=missing.html",
      "kanna-host:open?path=%E0%A4%A",
      "kanna-host:open?path=https%3A%2F%2Fevil.example%2F"
    ]) {
      expect(await navigate(url), url).toBe(false);
    }
    expect(api.readArtifactFile.mock.calls.length).toBe(before);
    expect(runHost().frame.attributes["data-path"]).toBe("index.html");
  });

  it("follows the previous link to the older tree id and comes back", async () => {
    await open(V2);
    await press("artifact-viewer-previous");
    expect(framedPage()).toContain("Version one");
    expect(byTestId("artifact-viewer-comment").map(text)).toEqual([expect.stringContaining("first pass")]);
    expect(byTestId("artifact-viewer-previous")[0].props.disabled).toBe(true);
    await press("artifact-viewer-newer");
    expect(framedPage()).toContain("Version two");
  });

  it("shows comment anchors and decisions recorded on the exact version only", async () => {
    const api = await open(V2);
    const comments = byTestId("artifact-viewer-comment");
    expect(comments).toHaveLength(1);
    expect(text(comments[0])).toContain("header is too tall");
    expect(JSON.stringify(renderer!.toJSON())).not.toContain("stray note about v1");
    expect(text(byTestId("artifact-viewer-anchor-path")[0])).toBe("css/site.css");
    expect(text(byTestId("artifact-viewer-anchor-position")[0])).toContain("line 2");
    expect(text(byTestId("artifact-viewer-anchor-excerpt")[0])).toContain("height: 120px");
    const decisions = byTestId("artifact-viewer-decision");
    expect(decisions).toHaveLength(1);
    expect(text(decisions[0])).toContain("owner");
    expect(text(decisions[0])).toContain("approved");

    // The anchor opens that file of the same tree at the anchored line.
    await press("artifact-viewer-anchor");
    const html = framedPage();
    expect(html).toContain("const targetLine = 2;");
    expect(api.readArtifactFile).toHaveBeenLastCalledWith("repo-1", V2, "css/site.css");
  });

  it("says a missing object is missing and renders no content", async () => {
    const api = await open(MISSING);
    const [state] = byTestId("artifact-viewer-unavailable");
    expect(text(state)).toContain("No artifact with this id");
    expect(text(state)).toContain(MISSING);
    expect(renderer!.root.findAll((node) => node.type === "WebView")).toHaveLength(0);
    expect(api.readArtifactFile).not.toHaveBeenCalled();
  });

  it("renders a descriptor that is no longer retained as expired and reads no content", async () => {
    const api = await open(EXPIRED);
    expect(text(byTestId("artifact-viewer-expired")[0])).toContain("Produced, no longer retained");
    expect(renderer!.root.findAll((node) => node.type === "WebView")).toHaveLength(0);
    expect(api.readArtifactFile).not.toHaveBeenCalled();
  });

  it("isolates shared HTML: no native bridge, no credential, no storage, no windows, no navigation out", async () => {
    await open(V2);
    const props = webView().props;
    // No bridge into React Native: without onMessage the WebView injects no
    // `window.ReactNativeWebView`, and nothing is injected into the page.
    for (const bridge of [
      "onMessage", "injectedJavaScript", "injectedJavaScriptBeforeContentLoaded",
      "injectedJavaScriptObject", "onOpenWindow", "onFileDownload", "applicationNameForUserAgent"
    ]) {
      expect(props[bridge], bridge).toBeUndefined();
    }
    expect(props.javaScriptCanOpenWindowsAutomatically).toBe(false);
    expect(props.setSupportMultipleWindows).toBe(false);
    expect(props.domStorageEnabled).toBe(false);
    expect(props.sharedCookiesEnabled).toBe(false);
    expect(props.thirdPartyCookiesEnabled).toBe(false);
    expect(props.incognito).toBe(true);
    expect(props.allowFileAccess).toBe(false);
    expect(props.allowFileAccessFromFileURLs).toBe(false);
    expect(props.allowUniversalAccessFromFileURLs).toBe(false);
    // Everything passes the library's whitelist so that nothing is handed to
    // Linking.openURL; this view's own callback then refuses it.
    expect(props.originWhitelist).toEqual(["*"]);
    // The page is the built document and nothing else: no URL, base URL,
    // headers or cookies of the connection that fetched its bytes.
    expect(Object.keys(props.source)).toEqual(["html"]);
    for (const url of [
      "https://evil.example/", "http://192.168.1.2:48120/v1/tasks", "tel:5551234", "sms:5551234",
      "mailto:a@b.c", "kanna://e2e-trust", "file:///etc/passwd", "data:text/html,<script>1</script>",
      "javascript:alert(1)", "kanna-artifact:pages/about.html", "about:srcdoc#x", "about:blank?x",
      "kanna-host:open?path=missing.html"
    ]) {
      expect(props.onShouldStartLoadWithRequest({ url }), url).toBe(false);
    }
    expect(props.onShouldStartLoadWithRequest({ url: "about:blank" })).toBe(true);
    expect(props.onShouldStartLoadWithRequest({ url: "about:srcdoc" })).toBe(true);
    expect(linking.openURL).not.toHaveBeenCalled();
    // The page carries a no-network policy ahead of its own markup.
    const html = framedPage();
    expect(html.indexOf(`<meta http-equiv="Content-Security-Policy" content="default-src 'none';`)).toBeLessThan(html.indexOf("<link"));
    expect(html).toContain("connect-src 'none'");
  });

  it("does not rely on the JS navigation callback: the page runs in a sandboxed frame of a trusted host", async () => {
    // On Android, react-native-webview allows a navigation the JS thread has
    // not answered within 250 ms. The engine must refuse top-level navigation
    // itself, which a frame sandboxed without allow-top-navigation does.
    await open(V2);
    const host = webView().props.source.html as string;
    // Exactly one frame, and the host has no link, form, refresh or base; its
    // one script navigates only to a host-open request for a file of the tree.
    expect(host.match(/<iframe /g)).toHaveLength(1);
    const outsidePage = host.replace(/<script type="application\/json"[\s\S]*?<\/script>/, "");
    expect(outsidePage).not.toMatch(/<a |<form|http-equiv="refresh"|<base|window\.open|srcdoc="/i);
    expect(outsidePage.match(/location\.href=[^;}]*/g)).toEqual([
      `location.href="kanna-host:open?path="+encodeURIComponent(d.path)`
    ]);
    const frame = host.match(/<iframe ([^>]*)><\/iframe>/)?.[1] ?? "";
    expect(frame).toContain('sandbox="allow-scripts"');
    for (const flag of ["allow-top-navigation", "allow-popups", "allow-same-origin", "allow-forms", "allow-modals"]) {
      expect(frame).not.toContain(flag);
    }
    // The host policy is exactly the page's: the frame cannot navigate itself.
    const hostPolicy = host.match(/<meta http-equiv="Content-Security-Policy" content="([^"]+)">/)?.[1] ?? "";
    expect(hostPolicy).toBe(ARTIFACT_DOCUMENT_POLICY);
    expect(hostPolicy).toContain("frame-src 'none'");
    // Page markup cannot end the data block early.
    expect(host.match(/<script type="application\/json"[^>]*>([\s\S]*?)<\/script>/)?.[1]).not.toContain("<");
    // The page inside the frame is intact, quotes and ampersands included.
    expect(framedPage()).toContain('<h1>Version two</h1>');
  });

  it("keeps Newer available when the previous version cannot be read", async () => {
    const api = client();
    api.getArtifact.mockImplementation(async (_repoId: string, artifactId: string) => {
      if (artifactId === V1) throw new Error(`Remote request failed (404): {"error":"artifact_not_found","artifactId":"${V1}"}`);
      return DETAILS[artifactId];
    });
    await act(async () => {
      renderer = create(
        <ArtifactViewer
          repoId="repo-1"
          initialArtifactId={V2}
          getArtifact={api.getArtifact}
          readArtifactFile={api.readArtifactFile}
          onClose={() => undefined}
        />
      );
    });
    await flush();
    await press("artifact-viewer-previous");
    expect(text(byTestId("artifact-viewer-unavailable")[0])).toContain("No artifact with this id");
    const [newer] = byTestId("artifact-viewer-newer");
    expect(newer.props.disabled).toBe(false);
    await press("artifact-viewer-newer");
    expect(text(byTestId("artifact-viewer-current-id")[0])).toContain(V2.slice(0, 12));
    expect(framedPage()).toContain("Version two");
  });

  it("lets only the host document and its frame document load, and opens only files of the tree", () => {
    const files = new Set(["index.html", "pages/a b.html"]);
    const opened: string[] = [];
    const open = (path: string) => opened.push(path);
    expect(shouldStartArtifactLoad({ url: "about:blank" }, files, open)).toBe(true);
    expect(shouldStartArtifactLoad({ url: "about:srcdoc" }, files, open)).toBe(true);
    expect(shouldStartArtifactLoad({ url: "https://example.com/pages/a.html" }, files, open)).toBe(false);
    expect(shouldStartArtifactLoad({ url: "#kanna-artifact=pages/a.html" }, files, open)).toBe(false);
    expect(shouldStartArtifactLoad({ url: "kanna-host:open?path=pages%2Fa%20b.html" }, files, open)).toBe(false);
    expect(shouldStartArtifactLoad({ url: "kanna-host:open?path=pages%2Fother.html" }, files, open)).toBe(false);
    expect(opened).toEqual(["pages/a b.html"]);
  });
});

describe("ArtifactViewer (mobile) reads", () => {
  const BIG = "b".repeat(40);
  const IMAGE_BYTES = 896 * 1024;
  const pages = Array.from({ length: 23 }, (_, index) => `p${index}.html`);

  /** The review's shape: a tiny index linking 23 pages, each with three distinct 896 KiB images. */
  function bigTree() {
    const files: Record<string, { bytes: number; text?: string }> = {
      "index.html": { bytes: 0, text: pages.map((page) => `<a href="${page}">${page}</a>`).join("") }
    };
    for (const page of pages) {
      files[page] = { bytes: 0, text: [0, 1, 2].map((image) => `<img src="img/${page}-${image}.png">`).join("") + `<h1>${page}</h1>` };
      for (const image of [0, 1, 2]) files[`img/${page}-${image}.png`] = { bytes: IMAGE_BYTES };
    }
    const reads: string[] = [];
    let base64Bytes = 0;
    const gates = new Map<string, () => void>();
    let holdPath: string | null = null;
    const readArtifactFile = vi.fn(async (repoId: string, artifactId: string, path: string): Promise<ArtifactFileContent> => {
      reads.push(path);
      const file = files[path];
      if (!file) throw new Error(`Remote request failed (404): {"error":"artifact_file_not_found"}`);
      if (path === holdPath) await new Promise<void>((resolve) => gates.set(path, resolve));
      const bytes = file.text === undefined ? new Uint8Array(file.bytes) : encodeUtf8(file.text);
      const dataBase64 = encodeBase64(bytes);
      base64Bytes += dataBase64.length;
      return {
        repoId, artifactId, path, size: bytes.length, dataBase64,
        mediaType: path.endsWith(".png") ? "image/png" : "text/html; charset=utf-8"
      };
    });
    const getArtifact = vi.fn(async () => detail(BIG, {
      files: Object.entries(files).map(([path, file]) => ({ path, size: file.text?.length ?? file.bytes }))
    }));
    return {
      readArtifactFile, getArtifact, reads,
      base64: () => base64Bytes,
      hold(path: string) { holdPath = path; },
      release(path: string) { gates.get(path)?.(); }
    };
  }

  async function openBig(tree: ReturnType<typeof bigTree>) {
    await act(async () => {
      renderer = create(
        <ArtifactViewer
          repoId="repo-1"
          initialArtifactId={BIG}
          getArtifact={tree.getArtifact}
          readArtifactFile={tree.readArtifactFile}
          onClose={() => undefined}
        />
      );
    });
    await flush();
  }

  it("shows the entry after its own reads, and reads a linked page only when it is opened", async () => {
    const tree = bigTree();
    await openBig(tree);
    // One read, of the index itself, before the entry is on screen.
    expect(tree.reads).toEqual(["index.html"]);
    expect(tree.base64()).toBeLessThan(4 * 1024);
    expect(framedPage()).toContain("p22.html");
    // The last link, far past any page budget, still opens, with its assets.
    const host = runHost();
    host.post({ kind: "kanna-artifact-navigate", path: "p22.html" });
    expect(await navigate(host.navigations[0])).toBe(false);
    expect(tree.reads.slice(1).sort()).toEqual(["img/p22.html-0.png", "img/p22.html-1.png", "img/p22.html-2.png", "p22.html"]);
    const page = runHost().frame.srcdoc;
    expect(page).toContain("<h1>p22.html</h1>");
    expect(page.match(/src="data:image\/png;base64,/g)).toHaveLength(3);
  });

  it("stops reading when the target changes or the viewer closes mid-page", async () => {
    const tree = bigTree();
    await openBig(tree);
    tree.hold("p3.html");
    const host = runHost();
    host.post({ kind: "kanna-artifact-navigate", path: "p3.html" });
    await navigate(host.navigations[0]);
    expect(tree.reads).toEqual(["index.html", "p3.html"]);
    // The reader moves on before p3.html arrives: none of its images is read.
    await act(async () => {
      renderer!.unmount();
    });
    renderer = null;
    tree.release("p3.html");
    await flush();
    expect(tree.reads).toEqual(["index.html", "p3.html"]);
  });
});

const REMOTE = "ssh://git.example.com/team/artifacts.git";
const FETCHED = "5".repeat(40);

function sharingActions(overrides: Partial<ArtifactViewerActions> = {}) {
  const actions = {
    getArtifactRemote: vi.fn(async (repoId: string) => ({
      repoId, configured: true, remote: REMOTE, source: "committed" as const, configFile: ".kanna/config.json"
    })),
    recordArtifactComment: vi.fn(async (repoId: string, artifactId: string, input: { author: string; body: string; anchor?: object }) => ({
      schemaVersion: 1, recordId: "c-new", repoId, aboutArtifactId: artifactId, createdAt: "2026-09-23T13:00:00Z", ...input
    })),
    recordArtifactDecision: vi.fn(async (repoId: string, artifactId: string, input: { who: string; what: string }) => ({
      schemaVersion: 1, recordId: "d-new", repoId, aboutArtifactId: artifactId, createdAt: "2026-09-23T13:00:00Z", ...input
    })),
    pushArtifact: vi.fn(async (_repoId: string, artifactId: string) => ({
      remote: REMOTE, artifactId, artifactIds: [artifactId, V1], createdRefs: ["refs/kanna/artifacts/shared/content/x/y"], upToDateRefs: 2
    })),
    fetchArtifact: vi.fn(async (_repoId: string, artifactId: string) => {
      DETAILS[artifactId] ??= detail(artifactId);
      return {
        remote: REMOTE, artifactId, fetched: [artifactId], contentRetained: [artifactId], recordsImported: 2,
        refused: [{ ref: `refs/kanna/artifacts/shared/records/${artifactId}/comments/bad`, reason: "record is not canonical" }],
        missing: [MISSING], detail: DETAILS[artifactId]
      };
    }),
    ...overrides
  };
  return actions;
}

async function openSharing(artifactId: string, actions = sharingActions()) {
  const api = client();
  await act(async () => {
    renderer = create(
      <ArtifactViewer
        repoId="repo-1"
        initialArtifactId={artifactId}
        getArtifact={api.getArtifact}
        readArtifactFile={api.readArtifactFile}
        actions={actions}
        onClose={() => undefined}
      />
    );
  });
  await flush();
  return { api, actions };
}

async function type(testID: string, value: string) {
  const [input] = byTestId(testID);
  await act(async () => {
    input.props.onChangeText(value);
  });
}

describe("ArtifactViewer (mobile) recording and sharing", () => {
  afterEach(() => {
    resetConfirmedArtifactRemotesForTests();
    delete DETAILS[FETCHED];
  });

  it("records a comment anchored to the exact version and the file on screen", async () => {
    const { actions } = await openSharing(V2);
    await type("artifact-viewer-comment-author", "stakeholder");
    await type("artifact-viewer-comment-body", "Contrast is too low");
    await type("artifact-viewer-anchor-position-input", "line 2");
    await type("artifact-viewer-anchor-excerpt-input", "height: 120px");
    await press("artifact-viewer-comment-submit");
    expect(actions.recordArtifactComment).toHaveBeenCalledWith("repo-1", V2, {
      author: "stakeholder",
      body: "Contrast is too low",
      anchor: { path: "index.html", position: "line 2", excerpt: "height: 120px" }
    });
    expect(byTestId("artifact-viewer-comment").map(text).join("\n")).toContain("Contrast is too low");

    // Another asset of the same tree, chosen explicitly.
    await type("artifact-viewer-comment-body", "Stylesheet note");
    await press("artifact-viewer-anchor-choice-css/site.css");
    await press("artifact-viewer-comment-submit");
    expect(actions.recordArtifactComment).toHaveBeenLastCalledWith("repo-1", V2, {
      author: "stakeholder", body: "Stylesheet note", anchor: { path: "css/site.css" }
    });
  });

  it("records a decision as data about the tree id and says it operates no gate", async () => {
    const { actions } = await openSharing(V2);
    expect(text(byTestId("artifact-viewer-decision-note")[0])).toMatch(/does not move any task or operate any gate/);
    await type("artifact-viewer-decision-who", "stakeholder");
    await type("artifact-viewer-decision-what", "approved");
    await press("artifact-viewer-decision-submit");
    expect(actions.recordArtifactDecision).toHaveBeenCalledWith("repo-1", V2, { who: "stakeholder", what: "approved" });
    const decisions = byTestId("artifact-viewer-decision").map(text);
    expect(decisions.at(-1)).toContain("approved");
    // Only artifact operations exist on this surface; nothing reaches a task.
    expect(actions.pushArtifact).not.toHaveBeenCalled();
    // A recording on one version stays off another.
    await press("artifact-viewer-previous");
    expect(byTestId("artifact-viewer-decision").map(text).join()).not.toContain("stakeholder");
    // Back on it, a server read that now returns the record shows it once.
    DETAILS[V2].decisions.push(await actions.recordArtifactDecision.mock.results[0].value);
    try {
      await press("artifact-viewer-newer");
      expect(byTestId("artifact-viewer-decision").map(text).filter((entry) => entry.includes("stakeholder"))).toHaveLength(1);
    } finally {
      DETAILS[V2].decisions.pop();
    }
  });

  it("names the remote and its config source, and confirms the first push to it", async () => {
    const { actions } = await openSharing(V2);
    expect(text(byTestId("artifact-viewer-remote-url")[0])).toBe(REMOTE);
    expect(text(byTestId("artifact-viewer-remote-source")[0])).toContain("committed repo config (.kanna/config.json)");
    await press("artifact-viewer-push");
    expect(actions.pushArtifact).not.toHaveBeenCalled();
    expect(text(byTestId("artifact-viewer-push-confirm")[0])).toContain(REMOTE);
    await press("artifact-viewer-push-accept");
    expect(actions.pushArtifact).toHaveBeenCalledWith("repo-1", V2);
    expect(text(byTestId("artifact-viewer-remote-outcome")[0])).toContain("1 refs created, 2 already up to date");
    // Once pushed there, the next push goes without asking.
    await press("artifact-viewer-push");
    expect(byTestId("artifact-viewer-push-confirm")).toHaveLength(0);
    expect(actions.pushArtifact).toHaveBeenCalledTimes(2);
  });

  it("shows a refused push with the server's reasons", async () => {
    const { actions } = await openSharing(V2, sharingActions({
      pushArtifact: vi.fn(async () => {
        throw new Error(`artifact remote ${REMOTE} already holds different objects under refs/kanna/artifacts/shared/records/x/comments/y (already exists); nothing there was overwritten`);
      })
    }));
    await press("artifact-viewer-push");
    await press("artifact-viewer-push-accept");
    expect(actions.pushArtifact).toHaveBeenCalled();
    expect(text(byTestId("artifact-viewer-remote-error")[0])).toContain("already exists");
  });

  it("fetches a hash, opens it and lists refused refs and missing versions", async () => {
    const { actions, api } = await openSharing(V2);
    await type("artifact-viewer-id-input", FETCHED);
    await press("artifact-viewer-fetch");
    expect(actions.fetchArtifact).toHaveBeenCalledWith("repo-1", FETCHED);
    expect(api.getArtifact).toHaveBeenCalledWith("repo-1", FETCHED);
    expect(text(byTestId("artifact-viewer-current-id")[0])).toContain(FETCHED.slice(0, 12));
    const outcome = text(byTestId("artifact-viewer-remote-outcome")[0]);
    expect(outcome).toContain("2 records imported");
    expect(outcome).toContain("no task moved");
    expect(text(byTestId("artifact-viewer-fetch-refused")[0])).toContain("record is not canonical");
    expect(text(byTestId("artifact-viewer-fetch-missing-versions")[0])).toContain(MISSING);
  });

  it("offers a fetch for a hash this desktop does not hold", async () => {
    const { actions } = await openSharing(MISSING);
    await press("artifact-viewer-fetch-missing");
    expect(actions.fetchArtifact).toHaveBeenCalledWith("repo-1", MISSING);
  });

  it("answers a late host-open load error with its own loading state, never the library's error page", async () => {
    const api = client();
    let releaseAbout = () => undefined as void;
    const readArtifactFile = vi.fn((repoId: string, artifactId: string, path: string) =>
      path === "pages/about.html"
        ? new Promise<ArtifactFileContent>((resolve) => {
            releaseAbout = () => void api.readArtifactFile(repoId, artifactId, path).then(resolve);
          })
        : api.readArtifactFile(repoId, artifactId, path));
    await act(async () => {
      renderer = create(
        <ArtifactViewer repoId="repo-1" initialArtifactId={V2} getArtifact={api.getArtifact} readArtifactFile={readArtifactFile} onClose={() => undefined} />
      );
    });
    await flush();
    const { onError, renderError } = webView().props;
    // Nothing drawn for a load error names the failure.
    const generic = create(renderError("undefined", -10, "net::ERR_UNKNOWN_URL_SCHEME"));
    expect(JSON.stringify(generic.toJSON())).not.toContain("ERR_UNKNOWN_URL_SCHEME");
    // Android let the host-open through after 250 ms and failed it. The commit
    // that learns of it swaps the failed WebView for the viewer's own loading
    // state for the requested file.
    await act(async () => {
      onError({ nativeEvent: { url: "kanna-host:open?path=pages%2Fabout.html", code: -10, description: "net::ERR_UNKNOWN_URL_SCHEME" } });
    });
    expect(renderer!.root.findAll((node) => node.type === "WebView")).toHaveLength(0);
    expect(text(renderer!.root)).toContain("Loading pages/about.html");
    expect(readArtifactFile).toHaveBeenCalledWith("repo-1", V2, "pages/about.html");
    // The late callback for the same navigation changes nothing further.
    releaseAbout();
    await flush();
    expect(framedPage()).toContain("About v2");
    expect(readArtifactFile.mock.calls.filter((call) => call[2] === "pages/about.html")).toHaveLength(1);
    // Any other load error is not a request.
    await act(async () => {
      webView().props.onError({ nativeEvent: { url: "kanna-host:open?path=..%2Fsecret", code: -10, description: "x" } });
    });
    await flush();
    expect(readArtifactFile).not.toHaveBeenCalledWith("repo-1", V2, "../secret");
  });

  it("says where to configure a remote when none is set, and cannot push or fetch", async () => {
    await openSharing(V2, sharingActions({
      getArtifactRemote: vi.fn(async (repoId: string) => ({ repoId, configured: false }))
    }));
    expect(text(byTestId("artifact-viewer-remote-unconfigured")[0])).toContain("artifacts.remote");
    expect(byTestId("artifact-viewer-push")[0].props.disabled).toBe(true);
    expect(byTestId("artifact-viewer-fetch")[0].props.disabled).toBe(true);
  });
});

describe("ArtifactViewer (mobile) abandoned reads", () => {
  it("starts a new page's reads only after the abandoned page's reads have settled", async () => {
    const api = client();
    const held: Array<{ path: string; release: () => void }> = [];
    const readArtifactFile = vi.fn((repoId: string, artifactId: string, path: string) => {
      if (artifactId === V2 && path === "index.html") {
        return new Promise<ArtifactFileContent>((resolve) => {
          held.push({ path, release: () => void api.readArtifactFile(repoId, artifactId, path).then(resolve) });
        });
      }
      return api.readArtifactFile(repoId, artifactId, path);
    });
    await act(async () => {
      renderer = create(
        <ArtifactViewer repoId="repo-1" initialArtifactId={V2} getArtifact={api.getArtifact} readArtifactFile={readArtifactFile} onClose={() => undefined} />
      );
    });
    await flush();
    expect(held.map((read) => read.path)).toEqual(["index.html"]);
    // The reader moves on while V2's entry is still on the wire.
    await press("artifact-viewer-previous");
    expect(readArtifactFile.mock.calls.filter((call) => call[1] === V1)).toEqual([]);
    held[0].release();
    await flush();
    // V1's reads start only now, and V2's page, cancelled, reads nothing more.
    expect(readArtifactFile.mock.calls.filter((call) => call[1] === V1).map((call) => call[2])).toEqual(["index.html", "css/site.css"]);
    expect(readArtifactFile.mock.calls.filter((call) => call[1] === V2).map((call) => call[2])).toEqual(["index.html"]);
    expect(framedPage()).toContain("Version one");
  });
});
