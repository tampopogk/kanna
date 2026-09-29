import React, { useCallback, useEffect, useMemo, useRef } from "react";
import {
  AppState,
  Modal,
  Pressable,
  SafeAreaView,
  StyleSheet,
  Text,
  View
} from "react-native";
import {
  WebView as NativeWebView,
  type WebViewMessageEvent,
  type WebViewProps
} from "react-native-webview";
import type { ShouldStartLoadRequest } from "react-native-webview/lib/WebViewTypes";
import { readDesignBridgeMessage, type DesignOperation } from "../lib/api/design";
import { buildDesignDocument, designPageMessageScript } from "./buildDesignDocument";

interface DesignWebViewHandle {
  injectJavaScript(script: string): void;
}

const WebView = NativeWebView as unknown as React.ForwardRefExoticComponent<
  WebViewProps & React.RefAttributes<DesignWebViewHandle>
>;

/**
 * An App Design task's live document on the phone (docs/specs/app-design.md;
 * the owner chose viewing and commenting for slice 1). The page is Kanna's
 * own editor bundle; everything it asks for goes through `onRequest`, which
 * the app performs over its LAN or relay connection for this task only.
 *
 * The bridge is narrow on purpose: the page posts an operation name and its
 * arguments, `parseDesignOperation` accepts exactly the known shapes, and
 * anything else is refused without a request being made. The page cannot
 * navigate anywhere and holds no credential, address or task id.
 */
export interface DesignDocumentViewProps {
  visible: boolean;
  title: string;
  theme: "light" | "dark";
  onClose(): void;
  onRequest(operation: DesignOperation): Promise<unknown>;
}

interface BridgeError {
  message: string;
  status: number;
  reason: string | null;
}

function bridgeError(error: unknown): BridgeError {
  const candidate = error as { message?: unknown; status?: unknown; reason?: unknown; serverMessage?: unknown };
  return {
    message: typeof candidate?.message === "string" ? candidate.message : String(error),
    status: typeof candidate?.status === "number" ? candidate.status : 0,
    reason: typeof candidate?.reason === "string" ? candidate.reason : null
  };
}

export function DesignDocumentView(props: DesignDocumentViewProps) {
  const webView = useRef<DesignWebViewHandle>(null);
  const onRequest = useRef(props.onRequest);
  onRequest.current = props.onRequest;
  // The document is built once per open; a theme change is a message, so
  // the page keeps its state.
  const html = useMemo(() => buildDesignDocument({ theme: props.theme }), [props.visible]);

  const post = useCallback((message: unknown) => {
    webView.current?.injectJavaScript(designPageMessageScript(message));
  }, []);

  useEffect(() => {
    post({ type: "theme", theme: props.theme });
  }, [props.theme, post]);

  // Back from the background: the page catches up on what it missed.
  useEffect(() => {
    const subscription = AppState.addEventListener("change", (state) => {
      if (state === "active") post({ type: "resume" });
    });
    return () => subscription.remove();
  }, [post]);

  const onMessage = useCallback(
    (event: WebViewMessageEvent) => {
      const message = readDesignBridgeMessage(event.nativeEvent.data);
      if (!message || message.type === "ready") return;
      const { id, operation } = message;
      if (!operation) {
        post({
          type: "response",
          id,
          ok: false,
          error: { message: "The app refused a malformed design request.", status: 400, reason: "refused" }
        });
        return;
      }
      onRequest.current(operation).then(
        (value) => post({ type: "response", id, ok: true, value: value ?? null }),
        (error: unknown) => post({ type: "response", id, ok: false, error: bridgeError(error) })
      );
    },
    [post]
  );

  const onShouldStartLoadWithRequest = useCallback(
    (request: ShouldStartLoadRequest) => request.url === "about:blank" || request.url.startsWith("data:"),
    []
  );

  return (
    <Modal visible={props.visible} animationType="slide" onRequestClose={props.onClose} presentationStyle="pageSheet">
      <SafeAreaView style={[styles.container, props.theme === "dark" ? styles.dark : styles.light]}>
        <View style={styles.header}>
          <Text style={[styles.title, props.theme === "dark" ? styles.darkText : null]} numberOfLines={1}>
            {props.title}
          </Text>
          <Pressable accessibilityRole="button" accessibilityLabel="Close design" onPress={props.onClose} hitSlop={12}>
            <Text style={styles.close}>Done</Text>
          </Pressable>
        </View>
        {props.visible ? (
          <WebView
            ref={webView}
            source={{ html, baseUrl: "about:blank" }}
            originWhitelist={["about:blank", "data:*"]}
            onShouldStartLoadWithRequest={onShouldStartLoadWithRequest}
            onMessage={onMessage}
            javaScriptEnabled
            incognito
            cacheEnabled={false}
            allowFileAccess={false}
            allowsInlineMediaPlayback={false}
            setSupportMultipleWindows={false}
            keyboardDisplayRequiresUserAction={false}
            hideKeyboardAccessoryView={false}
            style={styles.webView}
            testID="design-document-webview"
          />
        ) : null}
      </SafeAreaView>
    </Modal>
  );
}

const styles = StyleSheet.create({
  container: { flex: 1 },
  light: { backgroundColor: "#ffffff" },
  dark: { backgroundColor: "#18181b" },
  header: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    paddingHorizontal: 16,
    paddingVertical: 10
  },
  title: { fontSize: 16, fontWeight: "600", flex: 1, marginRight: 12, color: "#1d1d1f" },
  darkText: { color: "#ececf1" },
  close: { fontSize: 16, color: "#7b4ff0", fontWeight: "600" },
  webView: { flex: 1, backgroundColor: "transparent" }
});
