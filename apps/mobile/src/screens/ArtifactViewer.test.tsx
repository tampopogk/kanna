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

/** The artifact page the host document frames, decoded from its `srcdoc`. */
function framedPage(): string {
  const host = webView().props.source.html as string;
  const srcdoc = host.match(/<iframe [^>]*srcdoc="([^"]*)"><\/iframe>/)?.[1];
  expect(srcdoc).toBeDefined();
  return srcdoc!
    .replace(/&quot;/g, '"')
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
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
    expect(api.readArtifactFile.mock.calls.map((call) => call.slice(1))).toEqual([
      [V2, "index.html"],
      [V2, "css/site.css"]
    ]);
    const html = framedPage();
    expect(html).toContain("Version two");
    expect(inlinedStylesheet(html)).toContain("height: 120px");
    expect(text(byTestId("artifact-viewer-current-id")[0])).toContain(V2.slice(0, 12));
  });

  it("opens an in-tree link inside the viewer, with that page's own relative assets", async () => {
    const api = await open(V2);
    await act(async () => {
      expect(webView().props.onShouldStartLoadWithRequest({ url: "kanna-artifact:pages/about.html" })).toBe(false);
    });
    await flush();
    const html = framedPage();
    expect(html).toContain("About v2");
    expect(inlinedStylesheet(html)).toContain("height: 120px");
    expect(api.readArtifactFile).toHaveBeenCalledWith("repo-1", V2, "pages/about.html");
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
      "javascript:alert(1)", "kanna-artifact:../../escape.html"
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

  it("does not rely on the JS navigation callback: the page runs in a sandboxed frame of a script-free host", async () => {
    // On Android, react-native-webview allows a navigation the JS thread has
    // not answered within 250 ms. The engine must refuse top-level navigation
    // itself, which a frame sandboxed without allow-top-navigation does.
    await open(V2);
    const host = webView().props.source.html as string;
    const withoutFrame = host.replace(/<iframe [^>]*><\/iframe>/, "");
    // Exactly one frame, and the host has no script, link, form or refresh of
    // its own that could navigate the top-level document.
    expect(host.match(/<iframe /g)).toHaveLength(1);
    expect(withoutFrame).not.toMatch(/<script|<a |<form|http-equiv="refresh"|<base/i);
    const frame = host.match(/<iframe ([^>]*?) srcdoc=/)?.[1] ?? "";
    expect(frame).toContain('sandbox="allow-scripts"');
    for (const flag of ["allow-top-navigation", "allow-popups", "allow-same-origin", "allow-forms", "allow-modals"]) {
      expect(frame).not.toContain(flag);
    }
    // The host policy is the page's policy, except that its one frame may
    // navigate only to an in-tree link, which has no network request.
    const hostPolicy = host.match(/<meta http-equiv="Content-Security-Policy" content="([^"]+)">/)?.[1] ?? "";
    expect(hostPolicy).toContain("frame-src kanna-artifact:;");
    expect(hostPolicy).toContain("child-src kanna-artifact:;");
    expect(hostPolicy.replace(/frame-src [^;]+|child-src [^;]+/g, ""))
      .toBe(ARTIFACT_DOCUMENT_POLICY.replace(/frame-src [^;]+|child-src [^;]+/g, ""));
    expect(hostPolicy).not.toMatch(/https?:|\*/);
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

  it("maps only in-tree navigations to the host and refuses the rest", () => {
    const opened: string[] = [];
    expect(shouldStartArtifactLoad({ url: "kanna-artifact:pages/a%20b.html#x" }, (path) => opened.push(path))).toBe(false);
    expect(shouldStartArtifactLoad({ url: "https://example.com/pages/a.html" }, (path) => opened.push(path))).toBe(false);
    expect(opened).toEqual(["pages/a b.html"]);
    // The host's own document and the sandboxed frame it loads are the only
    // loads the callback lets through.
    expect(shouldStartArtifactLoad({ url: "about:srcdoc" }, (path) => opened.push(path))).toBe(true);
  });
});
