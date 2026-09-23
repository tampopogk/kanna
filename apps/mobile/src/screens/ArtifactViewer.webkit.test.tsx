import React from "react";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { ArtifactDetail, ArtifactFileContent } from "../lib/api/types";

/**
 * The in-tree navigation path end to end in a real WebKit engine (T12
 * follow-up): a DOM click on a link inside the sandboxed artifact frame runs
 * the page's own link script, the host document's message check and its
 * top-level `kanna-host:open` navigation, which WebKit hands to the navigation
 * delegate. The URL the delegate receives is then given to the viewer's actual
 * `onShouldStartLoadWithRequest`, and the page the viewer renders in answer is
 * loaded into WebKit again.
 *
 * `tests/webkit/artifact-host-harness.swift` is that delegate: a WKWebView,
 * the engine react-native-webview uses on iOS. It needs macOS and a Swift
 * compiler; elsewhere this suite is skipped. It does not cover Android's
 * WebView, which is on the physical-device check list.
 */

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock("react-native", () => ({
  ActivityIndicator: "ActivityIndicator",
  Linking: { openURL: vi.fn(), canOpenURL: vi.fn() },
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

import { ArtifactViewer } from "./ArtifactViewer";
import { encodeBase64, encodeUtf8 } from "./buildArtifactDocument";

const WORKTREE_ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const HARNESS_SOURCE = fileURLToPath(new URL("../../tests/webkit/artifact-host-harness.swift", import.meta.url));

function swiftAvailable(): boolean {
  if (process.platform !== "darwin") return false;
  try {
    execFileSync("xcrun", ["--find", "swiftc"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

const BENIGN = "6".repeat(40);
const HOSTILE = "7".repeat(40);
const FORGED = "kanna-host:open?path=pages%2Fabout.html";

const FILES: Record<string, Record<string, string>> = {
  [BENIGN]: {
    "index.html": `<link rel="stylesheet" href="css/site.css"><h1>Index page</h1><p><a id="about" href="pages/about.html">About</a></p>`,
    "css/site.css": "h1 { color: rgb(12, 34, 56); }",
    "pages/about.html": `<link rel="stylesheet" href="../css/site.css"><h1>About page</h1>`
  },
  [HOSTILE]: {
    // Every way the page might reach the viewer or leave the frame on its own.
    "index.html": `<h1>Hostile page</h1><a href="pages/about.html">About</a>
<form id="f" action="https://example.com/form" method="post"><input name="x" value="1"></form>
<script>
var attempts = [
  function () { top.location.href = ${JSON.stringify(FORGED)}; },
  function () { top.location.href = "https://example.com/top"; },
  function () { location.href = ${JSON.stringify(FORGED)}; },
  function () { window.open(${JSON.stringify(FORGED)}); },
  function () { window.open("https://example.com/popup"); },
  function () { document.getElementById("f").submit(); },
  function () { var m = document.createElement("meta"); m.httpEquiv = "refresh"; m.content = "0;url=" + ${JSON.stringify(FORGED)}; document.head.appendChild(m); },
  function () { var a = document.createElement("a"); a.href = ${JSON.stringify(FORGED)}; a.target = "_top"; document.body.appendChild(a); a.click(); }
];
attempts.forEach(function (attempt) { try { attempt(); } catch (error) {} });
</script>`,
    "pages/about.html": `<h1>About page</h1>`
  }
};

function detail(artifactId: string): ArtifactDetail {
  const files = Object.entries(FILES[artifactId]).map(([path, text]) => ({ path, size: text.length }));
  return {
    repoId: "repo-1", artifactId, retained: true,
    reference: { type: "stored", repoId: "repo-1", artifactId, kind: "mockup" },
    files,
    versions: [{
      schemaVersion: 1, recordId: "v", repoId: "repo-1", artifactId, kind: "mockup", entrypoint: "index.html",
      createdAt: "2026-09-23T10:00:00Z", retention: "keep", producedBy: { taskId: "task-a" }, fileCount: files.length,
      totalBytes: 1, storage: { commit: "c".repeat(40), ref: `refs/kanna/artifacts/trees/${artifactId}` }
    }],
    comments: [], decisions: []
  };
}

async function readArtifactFile(repoId: string, artifactId: string, path: string): Promise<ArtifactFileContent> {
  const bytes = encodeUtf8(FILES[artifactId][path]);
  return {
    repoId, artifactId, path, size: bytes.length, dataBase64: encodeBase64(bytes),
    mediaType: path.endsWith(".css") ? "text/css; charset=utf-8" : "text/html; charset=utf-8"
  };
}

type HarnessEvent =
  | { kind: "navigation"; url: string; mainFrame: boolean }
  | { kind: "window-open"; url: string }
  | { kind: "document"; top: boolean; mainFrame: boolean; text: string }
  | { kind: "click"; selector: string; found: boolean; mainFrame: boolean };

describe.skipIf(!swiftAvailable())("ArtifactViewer in a real WKWebView", () => {
  let directory = "";
  let harness = "";
  let renderer: ReactTestRenderer | null = null;

  beforeAll(() => {
    const scratch = join(WORKTREE_ROOT, ".tmp");
    mkdirSync(scratch, { recursive: true });
    directory = mkdtempSync(join(scratch, "artifact-webkit-"));
    harness = join(directory, "artifact-host-harness");
    execFileSync("xcrun", ["swiftc", "-swift-version", "5", "-O", HARNESS_SOURCE, "-o", harness], { stdio: "pipe" });
  }, 180_000);

  afterAll(() => {
    act(() => renderer?.unmount());
    if (directory && existsSync(directory)) rmSync(directory, { recursive: true, force: true });
  });

  async function flush() {
    for (let index = 0; index < 10; index += 1) {
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
    }
  }

  async function open(artifactId: string) {
    act(() => renderer?.unmount());
    await act(async () => {
      renderer = create(
        <ArtifactViewer
          repoId="repo-1"
          initialArtifactId={artifactId}
          getArtifact={async (_repoId, id) => detail(id)}
          readArtifactFile={readArtifactFile}
          onClose={() => undefined}
        />
      );
    });
    await flush();
  }

  function webViewProps() {
    const [view] = renderer!.root.findAll((node) => node.type === "WebView");
    expect(view).toBeDefined();
    return view.props;
  }

  /** Load the viewer's current host document into WebKit and report what WebKit did. */
  function runInWebKit(selector: string, name: string, html = webViewProps().source.html as string): HarnessEvent[] {
    const file = join(directory, `${name}.html`);
    writeFileSync(file, html);
    const output = execFileSync(harness, [file, selector, "2"], { encoding: "utf8", timeout: 30_000 });
    return output.trim().split("\n").filter(Boolean).map((line) => JSON.parse(line) as HarnessEvent);
  }

  function frameTexts(events: HarnessEvent[]): string[] {
    return events.flatMap((event) => (event.kind === "document" && !event.top && event.text ? [event.text] : []));
  }

  function refusedRequests(events: HarnessEvent[]): HarnessEvent[] {
    return events.filter((event) =>
      event.kind === "window-open" ||
      (event.kind === "navigation" && event.url !== "about:blank" && event.url !== "about:srcdoc"));
  }

  it("follows a clicked in-tree link from the sandboxed frame to the viewer and renders that page", async () => {
    await open(BENIGN);
    const first = runInWebKit('a[href^="#kanna-artifact="]', "benign-index");
    expect(frameTexts(first).at(-1)).toContain("Index page");
    expect(first).toContainEqual(expect.objectContaining({ kind: "click", found: true, mainFrame: false }));
    // The only request WebKit handed the delegate is the host's own, for the
    // top-level document, naming the clicked file.
    expect(refusedRequests(first)).toEqual([
      { kind: "navigation", url: "kanna-host:open?path=pages%2Fabout.html", mainFrame: true }
    ]);

    // That exact URL, answered by the viewer's real callback: refused, and
    // the file rendered instead.
    let allowed = true;
    await act(async () => {
      allowed = webViewProps().onShouldStartLoadWithRequest({ url: (refusedRequests(first)[0] as { url: string }).url });
    });
    await flush();
    expect(allowed).toBe(false);
    expect((webViewProps().source.html as string)).toContain("About page");

    const second = runInWebKit("", "benign-about");
    expect(frameTexts(second).at(-1)).toContain("About page");
    expect(refusedRequests(second)).toEqual([]);
  }, 60_000);

  it("gives a hostile page no way to reach the viewer or leave its frame", async () => {
    await open(HOSTILE);
    const events = runInWebKit("", "hostile-index");
    // Top-level navigation, self-navigation, popups, forms, meta refresh and
    // _top links all die inside WebKit: the delegate is never asked, so the
    // viewer's callback is never the barrier.
    expect(refusedRequests(events)).toEqual([]);
    // The frame still shows the page it was given.
    expect(frameTexts(events).every((text) => text.includes("Hostile page"))).toBe(true);
    expect(frameTexts(events).length).toBeGreaterThan(0);

    // Positive control: the same host document with the frame's sandbox
    // removed. The attempts are live — the forged host-open and the popups
    // reach the delegate — so the result above is the sandbox's doing.
    const host = webViewProps().source.html as string;
    const unsandboxed = host.replace(' sandbox="allow-scripts"', "");
    expect(unsandboxed).not.toBe(host);
    const control = refusedRequests(runInWebKit("", "hostile-unsandboxed", unsandboxed));
    expect(control).toContainEqual({ kind: "navigation", url: FORGED, mainFrame: true });
    expect(control).toContainEqual({ kind: "window-open", url: "https://example.com/popup" });
  }, 60_000);
});
