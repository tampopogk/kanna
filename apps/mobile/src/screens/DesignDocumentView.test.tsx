import React from "react";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import { afterEach, describe, expect, it, vi } from "vitest";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const appState = vi.hoisted(() => ({ listeners: [] as Array<(state: string) => void> }));

vi.mock("react-native", () => ({
  AppState: {
    addEventListener: (_: string, listener: (state: string) => void) => {
      appState.listeners.push(listener);
      return { remove: () => undefined };
    },
  },
  Modal: "Modal",
  Pressable: "Pressable",
  SafeAreaView: "SafeAreaView",
  StyleSheet: { create: <T extends Record<string, unknown>>(styles: T) => styles },
  Text: "Text",
  View: "View",
}));
vi.mock("react-native-webview", () => ({ WebView: "WebView" }));

import { DesignDocumentView } from "./DesignDocumentView";

let renderer: ReactTestRenderer | null = null;
afterEach(() => {
  act(() => renderer?.unmount());
  renderer = null;
  appState.listeners.length = 0;
});

function mountView(onRequest: (operation: unknown) => Promise<unknown>) {
  const injected: string[] = [];
  act(() => {
    renderer = create(
      <DesignDocumentView visible title="Artifact viewer" theme="light" onClose={() => undefined} onRequest={onRequest as never} />,
      { createNodeMock: () => ({ injectJavaScript: (script: string) => injected.push(script) }) },
    );
  });
  const webView = renderer!.root.findByType("WebView" as never);
  const post = async (message: unknown) => {
    await act(async () => {
      webView.props.onMessage({ nativeEvent: { data: JSON.stringify(message) } });
      await Promise.resolve();
      await Promise.resolve();
    });
  };
  return { webView, injected, post };
}

describe("DesignDocumentView", () => {
  it("performs a well-formed request for the page and answers it by id", async () => {
    const onRequest = vi.fn(async () => ({ position: "static", threads: [] }));
    const { injected, post } = mountView(onRequest);
    await post({ kind: "kanna-design", type: "request", id: 3, op: "view", args: [] });
    expect(onRequest).toHaveBeenCalledWith({ op: "view" });
    const answer = injected.find((script) => script.includes('"id":3'));
    expect(answer).toContain('"ok":true');
    expect(answer).toContain('"position":"static"');
  });

  it("refuses a malformed request without performing anything", async () => {
    const onRequest = vi.fn(async () => null);
    const { injected, post } = mountView(onRequest);
    await post({ kind: "kanna-design", type: "request", id: 4, op: "advanceStage", args: ["task-1"] });
    expect(onRequest).not.toHaveBeenCalled();
    expect(injected.find((script) => script.includes('"id":4'))).toContain('"reason":"refused"');
  });

  it("hands the page a failure's status and reason, so a schema refusal stops the editor", async () => {
    const onRequest = vi.fn(async () => {
      throw Object.assign(new Error("schema mismatch"), { status: 422, reason: "schema" });
    });
    const { injected, post } = mountView(onRequest);
    await post({ kind: "kanna-design", type: "request", id: 5, op: "sync", args: [{ schemaVersion: "x", stateVector: "" }] });
    const answer = injected.find((script) => script.includes('"id":5'))!;
    expect(answer).toContain('"status":422');
    expect(answer).toContain('"reason":"schema"');
  });

  it("never navigates away and catches up when the app returns to the foreground", () => {
    const { webView, injected } = mountView(async () => null);
    expect(webView.props.onShouldStartLoadWithRequest({ url: "about:blank" })).toBe(true);
    expect(webView.props.onShouldStartLoadWithRequest({ url: "https://example.com" })).toBe(false);
    expect(webView.props.source.html).toContain("connect-src 'none'");
    act(() => appState.listeners.forEach((listener) => listener("active")));
    expect(injected.some((script) => script.includes('"type":"resume"'))).toBe(true);
  });
});
