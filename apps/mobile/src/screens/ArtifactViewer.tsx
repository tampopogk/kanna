import React, { useEffect, useMemo, useRef, useState } from "react";
import {
  ActivityIndicator,
  Modal,
  Pressable,
  SafeAreaView,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View
} from "react-native";
import {
  WebView as NativeWebView,
  type WebViewNavigation,
  type WebViewProps
} from "react-native-webview";
import type {
  ArtifactComment,
  ArtifactDetail,
  ArtifactFileContent
} from "../lib/api/types";
import {
  buildArtifactSite,
  isolateArtifactSite,
  type ArtifactSite
} from "./buildArtifactDocument";

/**
 * One repository's artifact at an exact tree id (spec §8), on the phone.
 *
 * The rendering and navigation pattern follows TaskFilePreview — a modal with
 * a self-contained document in a locked-down WebView — but the access model
 * does not: files are read from the artifact store by repository and tree id,
 * never from a task workspace, so the artifact outlives the task that made it.
 *
 * Shared HTML is untrusted, and its scripts run. The WebView therefore gets no
 * native bridge (no `onMessage`, no injected script), no storage, no cookies,
 * no file access and no new windows, and its callback refuses every
 * navigation. That callback is not the barrier on its own: on Android the
 * library allows a navigation the JS thread has not answered within 250 ms. So
 * the page runs in a frame sandboxed with `allow-scripts` alone inside a
 * trusted host document (`isolateArtifactSite`): the engine refuses the page's
 * top-level navigation and new windows, the policy refuses any navigation of
 * the frame, and an in-tree link is a request to the host document, which
 * shows that file of the same tree from its own map. No in-tree navigation
 * passes through the native callback. The whitelist is `*` on purpose —
 * react-native-webview hands any URL
 * that fails the whitelist to `Linking.openURL`, which would let a page open
 * Safari or another app.
 */

export interface ArtifactViewerProps {
  repoId: string;
  initialArtifactId?: string;
  getArtifact(repoId: string, artifactId: string): Promise<ArtifactDetail>;
  readArtifactFile(repoId: string, artifactId: string, path: string): Promise<ArtifactFileContent>;
  onClose(): void;
}

type Unavailable = "missing" | "invalid" | "expired" | "error";

type DetailState =
  | { status: "idle" }
  | { status: "loading"; artifactId: string }
  | { status: "content"; artifactId: string; detail: ArtifactDetail }
  | { status: "unavailable"; artifactId: string; reason: Unavailable; message: string };

type DocumentState =
  | { status: "idle" }
  | { status: "loading"; key: string }
  | { status: "ready"; key: string; document: ArtifactSite }
  | { status: "error"; key: string; message: string };

interface FileTarget {
  path: string;
  initialLine?: number;
}

const WebView = NativeWebView as unknown as React.ComponentType<WebViewProps>;
const ARTIFACT_ID = /^[0-9a-f]{40}$/;

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function classify(error: unknown): Unavailable {
  const message = errorMessage(error);
  if (/artifact_content_missing|no longer retained/i.test(message)) return "expired";
  if (/artifact_not_found|\(404\)|\b404\b|not found/i.test(message)) return "missing";
  if (/invalid_artifact_id/i.test(message)) return "invalid";
  return "error";
}

function lineOf(position: string | undefined): number | undefined {
  const match = position?.match(/\bline\s*(\d+)/i) ?? position?.match(/^L?(\d+)$/i);
  const line = Number(match?.[1]);
  return Number.isInteger(line) && line > 0 ? line : undefined;
}

/** The host document itself and the frame document it sets; nothing else loads. */
export function shouldStartArtifactLoad(request: Pick<WebViewNavigation, "url">): boolean {
  return request.url === "about:blank" || request.url === "about:srcdoc";
}

const UNAVAILABLE_TITLES: Record<Unavailable, string> = {
  missing: "No artifact with this id in this repository",
  invalid: "Not an artifact id",
  expired: "Produced, no longer retained",
  error: "Couldn’t read the artifact"
};

export function ArtifactViewer({
  repoId,
  initialArtifactId,
  getArtifact,
  readArtifactFile,
  onClose
}: ArtifactViewerProps) {
  const getArtifactRef = useRef(getArtifact);
  getArtifactRef.current = getArtifact;
  const readFileRef = useRef(readArtifactFile);
  readFileRef.current = readArtifactFile;

  const [input, setInput] = useState(initialArtifactId ?? "");
  const [currentId, setCurrentId] = useState(initialArtifactId ?? "");
  const [newer, setNewer] = useState<string[]>([]);
  const [target, setTarget] = useState<FileTarget | null>(null);
  const [detailState, setDetailState] = useState<DetailState>({ status: "idle" });
  const [documentState, setDocumentState] = useState<DocumentState>({ status: "idle" });

  const detail =
    detailState.status === "content" && detailState.artifactId === currentId
      ? detailState.detail
      : null;
  const latest = detail?.versions[detail.versions.length - 1] ?? null;
  const previousId = latest?.previous ?? null;
  /** A descriptor without the flag predates retention and is treated as retained. */
  const retained = detail?.retained !== false;
  const filePath = target?.path ?? latest?.entrypoint ?? null;
  const documentKey = detail && retained && filePath
    ? `${repoId}\u0000${currentId}\u0000${filePath}\u0000${target?.initialLine ?? ""}`
    : null;
  /** Only records about this exact tree id; another version's notes are not this one's. */
  const comments = useMemo(
    () => (detail?.comments ?? []).filter((comment) => comment.aboutArtifactId === currentId),
    [currentId, detail]
  );
  const decisions = useMemo(
    () => (detail?.decisions ?? []).filter((decision) => decision.aboutArtifactId === currentId),
    [currentId, detail]
  );

  useEffect(() => {
    if (!currentId) {
      setDetailState({ status: "idle" });
      return;
    }
    let active = true;
    const artifactId = currentId;
    setDetailState({ status: "loading", artifactId });
    setTarget(null);
    void getArtifactRef.current(repoId, artifactId).then(
      (loaded) => {
        if (active) setDetailState({ status: "content", artifactId, detail: loaded });
      },
      (error: unknown) => {
        if (active) {
          setDetailState({
            status: "unavailable",
            artifactId,
            reason: classify(error),
            message: errorMessage(error)
          });
        }
      }
    );
    return () => {
      active = false;
    };
  }, [currentId, repoId]);

  useEffect(() => {
    if (!documentKey || !filePath) {
      setDocumentState({ status: "idle" });
      return;
    }
    let active = true;
    const artifactId = currentId;
    setDocumentState({ status: "loading", key: documentKey });
    void buildArtifactSite({
      path: filePath,
      initialLine: target?.initialLine,
      readFile: (path) => readFileRef.current(repoId, artifactId, path)
    }).then(
      (document) => {
        if (active) setDocumentState({ status: "ready", key: documentKey, document });
      },
      (error: unknown) => {
        if (active) setDocumentState({ status: "error", key: documentKey, message: errorMessage(error) });
      }
    );
    return () => {
      active = false;
    };
    // `documentKey` names the repository, the tree, the file and the line.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [documentKey]);

  const openInput = () => {
    const id = input.trim().toLowerCase();
    if (!id || id === currentId) return;
    setNewer([]);
    setCurrentId(id);
  };

  const goPrevious = () => {
    if (!previousId) return;
    setNewer([...newer, currentId]);
    setCurrentId(previousId);
    setInput(previousId);
  };

  const goNewer = () => {
    const next = newer[newer.length - 1];
    if (!next) return;
    setNewer(newer.slice(0, -1));
    setCurrentId(next);
    setInput(next);
  };

  const showAnchor = (comment: ArtifactComment) => {
    if (!comment.anchor?.path) return;
    setTarget({ path: comment.anchor.path, initialLine: lineOf(comment.anchor.position) });
  };

  const visibleDocument =
    documentState.status !== "idle" && documentState.key === documentKey ? documentState : null;
  const hostDocument = useMemo(
    () => (documentState.status === "ready" ? isolateArtifactSite(documentState.document) : ""),
    [documentState]
  );

  return (
    <Modal animationType="slide" onRequestClose={onClose} presentationStyle="fullScreen" visible>
      <SafeAreaView style={styles.safeArea}>
        <View style={styles.header}>
          <View style={styles.headerCopy}>
            <Text style={styles.title}>Artifact</Text>
            <Text numberOfLines={1} style={styles.path} testID="artifact-viewer-current-id">
              {currentId ? `${currentId.slice(0, 12)}${filePath ? ` · ${filePath}` : ""}` : "No artifact open"}
            </Text>
          </View>
          <Pressable accessibilityRole="button" hitSlop={10} onPress={onClose} style={styles.button}>
            <Text style={styles.buttonText}>Close</Text>
          </Pressable>
        </View>

        <View style={styles.idBar}>
          <TextInput
            accessibilityLabel="Artifact tree id"
            autoCapitalize="none"
            autoCorrect={false}
            onChangeText={setInput}
            onSubmitEditing={openInput}
            placeholder="Artifact tree id (40 hex)"
            placeholderTextColor="#5B6B82"
            style={styles.idInput}
            testID="artifact-viewer-id-input"
            value={input}
          />
          <Pressable
            accessibilityRole="button"
            disabled={!ARTIFACT_ID.test(input.trim().toLowerCase())}
            onPress={openInput}
            style={styles.button}
            testID="artifact-viewer-open"
          >
            <Text style={styles.buttonText}>Open</Text>
          </Pressable>
        </View>

        {detail || newer.length > 0 ? (
          <View style={styles.navBar}>
            <Pressable
              accessibilityRole="button"
              accessibilityState={{ disabled: !previousId }}
              disabled={!previousId}
              onPress={goPrevious}
              style={[styles.button, !previousId && styles.disabled]}
              testID="artifact-viewer-previous"
            >
              <Text style={styles.buttonText}>‹ Previous version</Text>
            </Pressable>
            <Pressable
              accessibilityRole="button"
              accessibilityState={{ disabled: newer.length === 0 }}
              disabled={newer.length === 0}
              onPress={goNewer}
              style={[styles.button, newer.length === 0 && styles.disabled]}
              testID="artifact-viewer-newer"
            >
              <Text style={styles.buttonText}>Newer ›</Text>
            </Pressable>
            {target ? (
              <Pressable accessibilityRole="button" onPress={() => setTarget(null)} style={styles.button}>
                <Text style={styles.buttonText}>Entrypoint</Text>
              </Pressable>
            ) : null}
          </View>
        ) : null}

        <View style={styles.content}>
          {!currentId ? (
            <View style={styles.centeredState}>
              <Text style={styles.stateText}>Enter the tree id of an artifact in this repository.</Text>
            </View>
          ) : detailState.status === "loading" || (detailState.status !== "unavailable" && !detail) ? (
            <View style={styles.centeredState}>
              <ActivityIndicator color="#73b7ff" size="large" />
              <Text style={styles.stateText}>Reading artifact…</Text>
            </View>
          ) : detailState.status === "unavailable" ? (
            <View style={styles.centeredState} testID="artifact-viewer-unavailable">
              <Text style={styles.errorTitle}>{UNAVAILABLE_TITLES[detailState.reason]}</Text>
              <Text selectable style={styles.errorText}>{detailState.artifactId}</Text>
              <Text selectable style={styles.errorText}>{detailState.message}</Text>
            </View>
          ) : !retained ? (
            <View style={styles.centeredState} testID="artifact-viewer-expired">
              <Text style={styles.errorTitle}>Produced, no longer retained</Text>
              <Text style={styles.errorText}>
                The records below still name this version; its content has been discarded by retention policy.
              </Text>
            </View>
          ) : !filePath ? (
            <View style={styles.centeredState}>
              <Text style={styles.stateText}>This artifact has no entrypoint to open.</Text>
            </View>
          ) : !visibleDocument || visibleDocument.status === "loading" ? (
            <View style={styles.centeredState}>
              <ActivityIndicator color="#73b7ff" size="large" />
              <Text style={styles.stateText}>Loading {filePath}…</Text>
            </View>
          ) : visibleDocument.status === "error" ? (
            <View style={styles.centeredState} testID="artifact-viewer-file-error">
              <Text style={styles.errorTitle}>Couldn’t open {filePath}</Text>
              <Text selectable style={styles.errorText}>{visibleDocument.message}</Text>
            </View>
          ) : (
            <>
              {visibleDocument.document.missing.length > 0 || visibleDocument.document.skipped.length > 0 ? (
                <Text style={styles.warning} testID="artifact-viewer-asset-warning">
                  {[
                    visibleDocument.document.missing.length
                      ? `Not in this tree: ${visibleDocument.document.missing.join(", ")}`
                      : "",
                    visibleDocument.document.skipped.length
                      ? `Not loaded: ${visibleDocument.document.skipped.join(", ")}`
                      : ""
                  ].filter(Boolean).join(" · ")}
                </Text>
              ) : null}
              <WebView
                allowFileAccess={false}
                allowFileAccessFromFileURLs={false}
                allowUniversalAccessFromFileURLs={false}
                allowsLinkPreview={false}
                cacheEnabled={false}
                dataDetectorTypes="none"
                domStorageEnabled={false}
                incognito
                javaScriptCanOpenWindowsAutomatically={false}
                javaScriptEnabled
                mixedContentMode="never"
                onShouldStartLoadWithRequest={(request: WebViewNavigation) =>
                  shouldStartArtifactLoad(request)
                }
                originWhitelist={["*"]}
                setSupportMultipleWindows={false}
                sharedCookiesEnabled={false}
                source={{ html: hostDocument }}
                style={styles.webView}
                testID="artifact-viewer-webview"
                thirdPartyCookiesEnabled={false}
              />
            </>
          )}
        </View>

        {detail ? (
          <ScrollView style={styles.records} testID="artifact-viewer-records">
            {latest ? (
              <Text style={styles.meta}>
                {latest.kind} · published {latest.createdAt} · by task {latest.producedBy.taskId} · {latest.retention}
              </Text>
            ) : null}
            <Text style={styles.sectionTitle}>Comments on this version ({comments.length})</Text>
            {comments.map((comment) => (
              <View key={comment.recordId} style={styles.record} testID="artifact-viewer-comment">
                <Text style={styles.recordAuthor}>{comment.author} · {comment.createdAt}</Text>
                <Text style={styles.recordBody}>{comment.body}</Text>
                {comment.anchor ? (
                  <Pressable
                    accessibilityRole="button"
                    disabled={!comment.anchor.path || !retained}
                    onPress={() => showAnchor(comment)}
                    style={styles.anchor}
                    testID="artifact-viewer-anchor"
                  >
                    {comment.anchor.path ? (
                      <Text style={styles.anchorPath} testID="artifact-viewer-anchor-path">{comment.anchor.path}</Text>
                    ) : null}
                    {comment.anchor.position ? (
                      <Text style={styles.anchorText} testID="artifact-viewer-anchor-position">@ {comment.anchor.position}</Text>
                    ) : null}
                    {comment.anchor.excerpt ? (
                      <Text style={styles.anchorExcerpt} testID="artifact-viewer-anchor-excerpt">“{comment.anchor.excerpt}”</Text>
                    ) : null}
                  </Pressable>
                ) : null}
              </View>
            ))}
            <Text style={styles.sectionTitle}>Decisions on this version ({decisions.length})</Text>
            <Text style={styles.meta}>A decision is a record about this version. It does not move any task.</Text>
            {decisions.map((decision) => (
              <View key={decision.recordId} style={styles.record} testID="artifact-viewer-decision">
                <Text style={styles.recordBody}>
                  <Text style={styles.recordAuthor}>{decision.who}</Text>: {decision.what}
                </Text>
                <Text style={styles.meta}>{decision.createdAt}</Text>
              </View>
            ))}
          </ScrollView>
        ) : null}
      </SafeAreaView>
    </Modal>
  );
}

const styles = StyleSheet.create({
  safeArea: { backgroundColor: "#050B14", flex: 1 },
  header: {
    alignItems: "flex-start",
    borderBottomColor: "#1D2C43",
    borderBottomWidth: 1,
    flexDirection: "row",
    gap: 12,
    justifyContent: "space-between",
    paddingHorizontal: 18,
    paddingVertical: 13
  },
  headerCopy: { flex: 1 },
  title: { color: "#F4F8FF", fontSize: 17, fontWeight: "700" },
  path: { color: "#8292A9", fontFamily: "Menlo", fontSize: 11, lineHeight: 16, marginTop: 3 },
  idBar: { flexDirection: "row", gap: 8, paddingHorizontal: 18, paddingVertical: 8 },
  idInput: {
    borderColor: "#31415B",
    borderRadius: 8,
    borderWidth: 1,
    color: "#D7E2F0",
    flex: 1,
    fontFamily: "Menlo",
    fontSize: 12,
    paddingHorizontal: 10,
    paddingVertical: 6
  },
  navBar: { flexDirection: "row", gap: 8, paddingBottom: 8, paddingHorizontal: 18 },
  button: {
    borderColor: "#31415B",
    borderRadius: 9,
    borderWidth: 1,
    paddingHorizontal: 12,
    paddingVertical: 7
  },
  buttonText: { color: "#D7E2F0", fontSize: 13, fontWeight: "600" },
  disabled: { opacity: 0.4 },
  content: { flex: 1 },
  centeredState: { alignItems: "center", flex: 1, justifyContent: "center", paddingHorizontal: 28 },
  stateText: { color: "#AEBBD0", fontSize: 14, marginTop: 12, textAlign: "center" },
  errorTitle: { color: "#F4F8FF", fontSize: 18, fontWeight: "700", textAlign: "center" },
  errorText: { color: "#C4CEDD", fontSize: 13, lineHeight: 19, marginTop: 8, textAlign: "center" },
  warning: {
    backgroundColor: "#2F2A12",
    color: "#F3E3A0",
    fontSize: 12,
    paddingHorizontal: 18,
    paddingVertical: 6
  },
  webView: { backgroundColor: "#FFFFFF", flex: 1 },
  records: {
    borderTopColor: "#1D2C43",
    borderTopWidth: 1,
    maxHeight: 260,
    paddingHorizontal: 18,
    paddingVertical: 8
  },
  meta: { color: "#8292A9", fontSize: 11, marginBottom: 4 },
  sectionTitle: { color: "#F4F8FF", fontSize: 13, fontWeight: "700", marginBottom: 4, marginTop: 8 },
  record: { borderTopColor: "#15243C", borderTopWidth: 1, paddingVertical: 6 },
  recordAuthor: { color: "#D7E2F0", fontSize: 12, fontWeight: "700" },
  recordBody: { color: "#C4CEDD", fontSize: 13, lineHeight: 18 },
  anchor: {
    backgroundColor: "#0B1422",
    borderRadius: 6,
    flexDirection: "row",
    flexWrap: "wrap",
    gap: 6,
    marginTop: 4,
    paddingHorizontal: 8,
    paddingVertical: 4
  },
  anchorPath: { color: "#73B7FF", fontFamily: "Menlo", fontSize: 11 },
  anchorText: { color: "#AEBBD0", fontSize: 11 },
  anchorExcerpt: { color: "#AEBBD0", fontSize: 11, fontStyle: "italic" }
});
