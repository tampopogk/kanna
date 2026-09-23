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

import { ArtifactViewer, shouldStartArtifactLoad } from "./ArtifactViewer";
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
  /** Deliver a message to the host, as if sent by `source` (the frame by default). */
  post(data: unknown, source?: object): void;
}

/**
 * Run the host document's own script against a stand-in window and document:
 * the pages it carries, the frame it fills, and the message listener it
 * installs. Only the script the WebView would execute is used.
 */
function runHost(host = webView().props.source.html as string): HostRun {
  const pagesText = host.match(/<script type="application\/json" id="kanna-artifact-pages">([\s\S]*?)<\/script>/)?.[1];
  const scripts = [...host.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((match) => match[1]);
  const entry = host.match(/<iframe [^>]*data-entry="([^"]*)"/)?.[1];
  expect(pagesText).toBeDefined();
  expect(scripts).toHaveLength(1);
  expect(entry).toBeDefined();
  const frame = {
    contentWindow: {},
    srcdoc: "",
    attributes: { "data-entry": entry! } as Record<string, string>,
    getAttribute(name: string) { return this.attributes[name] ?? null; },
    setAttribute(name: string, value: string) { this.attributes[name] = value; }
  };
  const listeners: Array<(event: { data: unknown; source: object }) => void> = [];
  const hostWindow = { addEventListener: (_type: string, listener: (typeof listeners)[number]) => listeners.push(listener) };
  const hostDocument = {
    getElementById: (id: string) =>
      id === "kanna-artifact-pages" ? { textContent: pagesText } : id === "kanna-artifact-frame" ? frame : null
  };
  new Function("window", "document", scripts[0])(hostWindow, hostDocument);
  return {
    frame,
    post: (data, source = frame.contentWindow) => listeners.forEach((listener) => listener({ data, source }))
  };
}

/** The page the host shows first. */
function framedPage(): string {
  return runHost().frame.srcdoc;
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
    // The entry, its stylesheet, and the one page it links to; each file once.
    expect(api.readArtifactFile.mock.calls.map((call) => call.slice(1))).toEqual([
      [V2, "index.html"],
      [V2, "css/site.css"],
      [V2, "pages/about.html"]
    ]);
    const html = framedPage();
    expect(html).toContain("Version two");
    expect(inlinedStylesheet(html)).toContain("height: 120px");
    expect(text(byTestId("artifact-viewer-current-id")[0])).toContain(V2.slice(0, 12));
  });

  it("opens an in-tree link in the host document, with that page's own relative assets", async () => {
    const api = await open(V2);
    // The linked page was rendered with the entry, so the host has it.
    expect(api.readArtifactFile).toHaveBeenCalledWith("repo-1", V2, "pages/about.html");
    const host = runHost();
    const index = host.frame.srcdoc;
    expect(index).toContain(`href="#kanna-artifact=pages/about.html"`);
    // The page's own click handler asks its parent for the path.
    expect(index).toContain(`parent.postMessage({kind:"kanna-artifact-navigate",path:p},"*")`);
    host.post({ kind: "kanna-artifact-navigate", path: "pages/about.html" });
    expect(host.frame.attributes["data-path"]).toBe("pages/about.html");
    expect(host.frame.srcdoc).toContain("About v2");
    expect(inlinedStylesheet(host.frame.srcdoc)).toContain("height: 120px");
    // No navigation went through the native callback for it.
    expect(linking.openURL).not.toHaveBeenCalled();
  });

  it("refuses every request on the host channel but an in-tree path from its own frame", async () => {
    await open(V2);
    const host = runHost();
    const index = host.frame.srcdoc;
    for (const forged of [
      { kind: "kanna-artifact-navigate", path: "../outside.html" },
      { kind: "kanna-artifact-navigate", path: "https://evil.example/" },
      { kind: "kanna-artifact-navigate", path: "javascript:alert(1)" },
      { kind: "kanna-artifact-navigate", path: "/pages/about.html" },
      { kind: "kanna-artifact-navigate", path: "__proto__" },
      { kind: "kanna-artifact-navigate", path: "constructor" },
      { kind: "kanna-artifact-navigate", path: "css/site.css" },
      { kind: "kanna-artifact-navigate", path: ["pages/about.html"] },
      { kind: "kanna-artifact-navigate" },
      { kind: "ReactNativeWebView", path: "pages/about.html" },
      "kanna-artifact-navigate:pages/about.html",
      null
    ]) {
      host.post(forged);
      expect(host.frame.srcdoc, JSON.stringify(forged)).toBe(index);
    }
    // A valid request from anything but its own frame is ignored too.
    host.post({ kind: "kanna-artifact-navigate", path: "pages/about.html" }, {});
    expect(host.frame.srcdoc).toBe(index);
    expect(host.frame.attributes["data-path"]).toBe("index.html");
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
      "javascript:alert(1)", "kanna-artifact:pages/about.html", "about:srcdoc#x", "about:blank?x"
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
    // Exactly one frame, and the host has no link, form, refresh or base that
    // could navigate the top-level document; its one script only fills the frame.
    expect(host.match(/<iframe /g)).toHaveLength(1);
    const outsidePages = host.replace(/<script type="application\/json"[\s\S]*?<\/script>/, "");
    expect(outsidePages).not.toMatch(/<a |<form|http-equiv="refresh"|<base|location|window\.open|srcdoc="/i);
    const frame = host.match(/<iframe ([^>]*)><\/iframe>/)?.[1] ?? "";
    expect(frame).toContain('sandbox="allow-scripts"');
    for (const flag of ["allow-top-navigation", "allow-popups", "allow-same-origin", "allow-forms", "allow-modals"]) {
      expect(frame).not.toContain(flag);
    }
    // The host policy is exactly the page's: the frame cannot navigate itself.
    const hostPolicy = host.match(/<meta http-equiv="Content-Security-Policy" content="([^"]+)">/)?.[1] ?? "";
    expect(hostPolicy).toBe(ARTIFACT_DOCUMENT_POLICY);
    expect(hostPolicy).toContain("frame-src 'none'");
    // Page markup cannot end the pages block early.
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

  it("lets only the host document and its frame document load", () => {
    expect(shouldStartArtifactLoad({ url: "about:blank" })).toBe(true);
    expect(shouldStartArtifactLoad({ url: "about:srcdoc" })).toBe(true);
    expect(shouldStartArtifactLoad({ url: "https://example.com/pages/a.html" })).toBe(false);
    expect(shouldStartArtifactLoad({ url: "#kanna-artifact=pages/a.html" })).toBe(false);
  });
});
