import React, { useEffect, useMemo, useRef, useState } from "react";
import {
  ActivityIndicator,
  Modal,
  Pressable,
  SafeAreaView,
  StyleSheet,
  Text,
  View
} from "react-native";
import {
  WebView as NativeWebView,
  type WebViewNavigation,
  type WebViewProps
} from "react-native-webview";
import type { TaskFileContent } from "../lib/api/types";
import { MOBILE_E2E_IDS } from "../e2eTestIds";
import { shareTaskFile } from "../lib/files/taskFileDownload";
import {
  buildTaskFilePreviewDocument,
  prepareTaskFileMarkdown,
  type TaskFilePreviewMode
} from "./buildTaskFilePreviewDocument";

export interface TaskFilePreviewProps {
  path: string;
  initialLine?: number;
  readFile(): Promise<TaskFileContent>;
  onClose(): void;
}

type LoadState =
  | { requestPath: string; status: "loading" }
  | { file: TaskFileContent; requestPath: string; status: "content" }
  | {
      error: string;
      requestPath: string;
      retryable: boolean;
      status: "error";
    };

interface ModeState {
  key: string;
  mode: TaskFilePreviewMode;
}

type DownloadState =
  | { status: "idle" }
  | { status: "sharing" }
  | { error: string; status: "error" };

const WebView = NativeWebView as unknown as React.ComponentType<WebViewProps>;
const ENABLE_E2E_WEBVIEW_INSPECTION =
  process.env.EXPO_PUBLIC_KANNA_ENABLE_E2E_TRUST_SEED === "1";

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isShareCancellation(error: unknown): boolean {
  return /cancel(?:led|ed)?/i.test(errorMessage(error));
}

export function isTaskFilePreviewErrorRetryable(error: unknown): boolean {
  if (typeof error === "object" && error !== null && "status" in error) {
    const status = (error as { status?: unknown }).status;
    if (status === 400 || status === 413 || status === 415) return false;
  }

  const message = errorMessage(error);
  const statusMatch = message.match(/\((\d{3})\)|\bstatus\s+(\d{3})\b/i);
  const status = Number(statusMatch?.[1] ?? statusMatch?.[2]);
  if (status === 400 || status === 413 || status === 415) return false;

  return !(
    /file path must|file exceeds (?:the )?.*limit|file is not valid utf-?8/i.test(
      message
    ) || /payload too large|unsupported media type|invalid or disallowed path/i.test(message)
  );
}

function hasPositiveLine(line: number | undefined): line is number {
  return typeof line === "number" && Number.isInteger(line) && line > 0;
}

function isMarkdownPath(path: string): boolean {
  return path.toLowerCase().endsWith(".md");
}

function defaultMode(path: string, initialLine?: number): TaskFilePreviewMode {
  return isMarkdownPath(path) && !hasPositiveLine(initialLine) ? "rendered" : "raw";
}

function modeKey(path: string, initialLine?: number): string {
  return `${path}\u0000${hasPositiveLine(initialLine) ? initialLine : ""}`;
}

export function TaskFilePreview({
  path,
  initialLine,
  readFile,
  onClose
}: TaskFilePreviewProps) {
  const readFileRef = useRef(readFile);
  readFileRef.current = readFile;
  const downloadBusyRef = useRef(false);
  const downloadOperationRef = useRef(0);

  const [retryGeneration, setRetryGeneration] = useState(0);
  const [loadState, setLoadState] = useState<LoadState>({
    requestPath: path,
    status: "loading"
  });
  const [modeState, setModeState] = useState<ModeState>(() => ({
    key: modeKey(path, initialLine),
    mode: defaultMode(path, initialLine)
  }));
  const [downloadState, setDownloadState] = useState<DownloadState>({
    status: "idle"
  });

  const visibleState: LoadState =
    loadState.requestPath === path
      ? loadState
      : { requestPath: path, status: "loading" };
  const displayPath =
    visibleState.status === "content" ? visibleState.file.path : path;
  const currentModeKey = modeKey(displayPath, initialLine);
  const requestedMode =
    modeState.key === currentModeKey
      ? modeState.mode
      : defaultMode(displayPath, initialLine);
  const markdownContent =
    visibleState.status === "content" &&
    isMarkdownPath(visibleState.file.path) &&
    typeof visibleState.file.content === "string"
      ? visibleState.file.content
      : null;
  const preparedMarkdown = useMemo(
    () =>
      markdownContent === null
        ? null
        : prepareTaskFileMarkdown(markdownContent),
    [markdownContent]
  );
  const renderedMarkdownAvailable =
    markdownContent !== null && preparedMarkdown !== null;
  const mode =
    requestedMode === "rendered" &&
    visibleState.status === "content" &&
    isMarkdownPath(visibleState.file.path) &&
    !renderedMarkdownAvailable
      ? "raw"
      : requestedMode;
  const previewDocument = useMemo(
    () =>
      visibleState.status === "content"
        && typeof visibleState.file.content === "string"
        ? buildTaskFilePreviewDocument({
            path: visibleState.file.path,
            content: visibleState.file.content,
            mode,
            initialLine: hasPositiveLine(initialLine) ? initialLine : undefined,
            preparedMarkdown
          })
        : "",
    [initialLine, mode, preparedMarkdown, visibleState]
  );

  useEffect(() => {
    let active = true;
    const requestPath = path;
    downloadOperationRef.current += 1;
    downloadBusyRef.current = false;
    setDownloadState({ status: "idle" });
    setLoadState({ requestPath, status: "loading" });

    let request: Promise<TaskFileContent>;
    try {
      request = readFileRef.current();
    } catch (error) {
      request = Promise.reject(error);
    }

    void request.then(
      (file) => {
        if (!active) return;
        setLoadState({ file, requestPath, status: "content" });
      },
      (error: unknown) => {
        if (!active) return;
        setLoadState({
          error: errorMessage(error),
          requestPath,
          retryable: isTaskFilePreviewErrorRetryable(error),
          status: "error"
        });
      }
    );

    return () => {
      active = false;
      downloadOperationRef.current += 1;
      downloadBusyRef.current = false;
    };
  }, [path, retryGeneration]);

  const retry = () => {
    setLoadState({ requestPath: path, status: "loading" });
    setRetryGeneration((generation) => generation + 1);
  };

  const toggleMode = () => {
    setModeState({
      key: currentModeKey,
      mode: mode === "rendered" ? "raw" : "rendered"
    });
  };

  const download = async () => {
    if (downloadBusyRef.current || visibleState.status !== "content") return;
    downloadBusyRef.current = true;
    const operation = ++downloadOperationRef.current;
    const requestPath = path;
    const expectedPath = visibleState.file.path;
    const readFileForOperation = readFileRef.current;
    setDownloadState({ status: "sharing" });
    let failed = false;

    try {
      // Read again at the action boundary. The callback ref follows the active
      // task/account route, so cached preview bytes can never cross a route
      // change and a moved workspace is re-resolved by the desktop.
      const file = await readFileForOperation();
      if (
        operation !== downloadOperationRef.current ||
        requestPath !== path ||
        readFileRef.current !== readFileForOperation
      ) {
        return;
      }
      if (file.path !== expectedPath) {
        throw new Error("The task workspace changed. Reopen this file and try again.");
      }
      if (
        file.dataBase64 === undefined ||
        !file.fileName ||
        !file.mediaType
      ) {
        throw new Error(
          "This desktop does not support original file downloads. Update the desktop and try again."
        );
      }
      await shareTaskFile({
        dataBase64: file.dataBase64,
        fileName: file.fileName,
        mediaType: file.mediaType
      });
    } catch (error) {
      failed = true;
      if (operation === downloadOperationRef.current) {
        setDownloadState(
          isShareCancellation(error)
            ? { status: "idle" }
            : { error: errorMessage(error), status: "error" }
        );
      }
    } finally {
      if (operation === downloadOperationRef.current) {
        downloadBusyRef.current = false;
        if (!failed) setDownloadState({ status: "idle" });
      }
    }
  };

  return (
    <Modal
      animationType="slide"
      onRequestClose={onClose}
      presentationStyle="fullScreen"
      visible
    >
      <SafeAreaView style={styles.safeArea}>
        <View style={styles.header}>
          <View style={styles.headerCopy}>
            <Text style={styles.title}>File Preview</Text>
            <Text
              numberOfLines={2}
              style={styles.path}
              testID={MOBILE_E2E_IDS.taskFilePreviewPath}
            >
              {displayPath}
            </Text>
          </View>
          <View style={styles.headerActions}>
            {visibleState.status === "content" ? (
              <Pressable
                accessibilityRole="button"
                accessibilityState={{
                  busy: downloadState.status === "sharing",
                  disabled: downloadState.status === "sharing"
                }}
                disabled={downloadState.status === "sharing"}
                onPress={() => void download()}
                style={styles.downloadButton}
                testID={MOBILE_E2E_IDS.taskFilePreviewDownload}
              >
                {downloadState.status === "sharing" ? (
                  <ActivityIndicator color="#FFFFFF" size="small" />
                ) : null}
                <Text style={styles.downloadText}>
                  {downloadState.status === "sharing" ? "Preparing…" : "Download"}
                </Text>
              </Pressable>
            ) : null}
            <Pressable
              accessibilityRole="button"
              hitSlop={10}
              onPress={onClose}
              style={styles.closeButton}
              testID={MOBILE_E2E_IDS.taskFilePreviewClose}
            >
              <Text style={styles.closeText}>Close</Text>
            </Pressable>
          </View>
        </View>

        {downloadState.status === "error" ? (
          <Text
            selectable
            style={styles.downloadError}
            testID={MOBILE_E2E_IDS.taskFilePreviewDownloadError}
          >
            {downloadState.error}
          </Text>
        ) : null}

        {visibleState.status === "loading" ? (
          <View style={styles.centeredState}>
            <ActivityIndicator color="#73b7ff" size="large" />
            <Text style={styles.stateText}>Loading file…</Text>
          </View>
        ) : visibleState.status === "error" ? (
          <View style={styles.centeredState}>
            <Text
              style={styles.errorTitle}
              testID={MOBILE_E2E_IDS.taskFilePreviewError}
            >
              Couldn’t open file
            </Text>
            <Text
              selectable
              style={styles.errorText}
              testID={MOBILE_E2E_IDS.taskFilePreviewErrorMessage}
            >
              {visibleState.error}
            </Text>
            {visibleState.retryable ? (
              <Pressable
                accessibilityRole="button"
                onPress={retry}
                style={styles.retryButton}
              >
                <Text style={styles.retryText}>Retry</Text>
              </Pressable>
            ) : null}
          </View>
        ) : (
          <View style={styles.content}>
            {typeof visibleState.file.content !== "string" ? (
              <View style={styles.centeredState}>
                <Text style={styles.errorTitle}>Preview unavailable</Text>
                <Text style={styles.errorText}>
                  Download the original file to open it in another app.
                </Text>
              </View>
            ) : isMarkdownPath(visibleState.file.path) ? (
              <View style={styles.modeBar}>
                <Text
                  style={styles.modeLabel}
                  testID={MOBILE_E2E_IDS.taskFilePreviewMode}
                >
                  {!renderedMarkdownAvailable
                    ? "Raw source · Rendered preview unavailable for large Markdown"
                    : mode === "rendered"
                      ? "Rendered Markdown"
                      : "Raw source"}
                </Text>
                {renderedMarkdownAvailable ? (
                  <Pressable
                    accessibilityRole="button"
                    onPress={toggleMode}
                    style={styles.modeButton}
                  >
                    <Text style={styles.modeButtonText}>
                      {mode === "rendered" ? "Raw" : "Rendered"}
                    </Text>
                  </Pressable>
                ) : null}
              </View>
            ) : null}
            {typeof visibleState.file.content === "string" ? (
              <WebView
                allowFileAccess={false}
                allowFileAccessFromFileURLs={false}
                allowUniversalAccessFromFileURLs={false}
                allowsLinkPreview={false}
                domStorageEnabled={false}
                javaScriptCanOpenWindowsAutomatically={false}
                javaScriptEnabled={mode === "raw" && hasPositiveLine(initialLine)}
                mixedContentMode="never"
                onShouldStartLoadWithRequest={(request: WebViewNavigation) =>
                  request.url === "about:blank"
                }
                originWhitelist={["about:blank"]}
                setSupportMultipleWindows={false}
                sharedCookiesEnabled={false}
                source={{ html: previewDocument }}
                style={styles.webView}
                thirdPartyCookiesEnabled={false}
                webviewDebuggingEnabled={ENABLE_E2E_WEBVIEW_INSPECTION}
              />
            ) : null}
            {ENABLE_E2E_WEBVIEW_INSPECTION ? (
              <Text
                accessibilityValue={{
                  text: JSON.stringify({
                    content: visibleState.file.content ?? null,
                    initialLine: initialLine ?? null,
                    mode,
                    path: visibleState.file.path
                  })
                }}
                pointerEvents="none"
                style={styles.e2eInspection}
                testID={MOBILE_E2E_IDS.taskFilePreviewInspection}
              >
                {JSON.stringify({
                  content: visibleState.file.content ?? null,
                  initialLine: initialLine ?? null,
                  mode,
                  path: visibleState.file.path
                })}
              </Text>
            ) : null}
          </View>
        )}
      </SafeAreaView>
    </Modal>
  );
}

const styles = StyleSheet.create({
  e2eInspection: {
    height: 1,
    opacity: 0.01,
    position: "absolute",
    width: 1
  },
  safeArea: {
    backgroundColor: "#050B14",
    flex: 1
  },
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
  headerCopy: {
    flex: 1
  },
  headerActions: {
    alignItems: "center",
    flexDirection: "row",
    gap: 8
  },
  title: {
    color: "#F4F8FF",
    fontSize: 17,
    fontWeight: "700"
  },
  path: {
    color: "#8292A9",
    fontFamily: "Menlo",
    fontSize: 11,
    lineHeight: 16,
    marginTop: 3
  },
  closeButton: {
    borderColor: "#31415B",
    borderRadius: 9,
    borderWidth: 1,
    paddingHorizontal: 12,
    paddingVertical: 8
  },
  closeText: {
    color: "#D7E2F0",
    fontSize: 14,
    fontWeight: "600"
  },
  downloadButton: {
    alignItems: "center",
    backgroundColor: "#2D6EB8",
    borderRadius: 9,
    flexDirection: "row",
    gap: 6,
    minHeight: 38,
    paddingHorizontal: 12,
    paddingVertical: 8
  },
  downloadText: {
    color: "#FFFFFF",
    fontSize: 14,
    fontWeight: "700"
  },
  downloadError: {
    backgroundColor: "#391B22",
    borderBottomColor: "#6D2B3C",
    borderBottomWidth: 1,
    color: "#FFD4DC",
    fontSize: 13,
    lineHeight: 18,
    paddingHorizontal: 18,
    paddingVertical: 10
  },
  centeredState: {
    alignItems: "center",
    flex: 1,
    justifyContent: "center",
    paddingHorizontal: 28
  },
  stateText: {
    color: "#AEBBD0",
    fontSize: 14,
    marginTop: 12
  },
  errorTitle: {
    color: "#F4F8FF",
    fontSize: 18,
    fontWeight: "700"
  },
  errorText: {
    color: "#C4CEDD",
    fontSize: 14,
    lineHeight: 20,
    marginTop: 8,
    textAlign: "center"
  },
  retryButton: {
    backgroundColor: "#2D6EB8",
    borderRadius: 9,
    marginTop: 18,
    paddingHorizontal: 18,
    paddingVertical: 10
  },
  retryText: {
    color: "#FFFFFF",
    fontSize: 14,
    fontWeight: "700"
  },
  content: {
    flex: 1
  },
  modeBar: {
    alignItems: "center",
    borderBottomColor: "#15243C",
    borderBottomWidth: 1,
    flexDirection: "row",
    justifyContent: "space-between",
    minHeight: 44,
    paddingHorizontal: 18
  },
  modeLabel: {
    color: "#8292A9",
    flex: 1,
    fontSize: 12
  },
  modeButton: {
    borderColor: "#31415B",
    borderRadius: 7,
    borderWidth: 1,
    paddingHorizontal: 10,
    paddingVertical: 6
  },
  modeButtonText: {
    color: "#73B7FF",
    fontSize: 12,
    fontWeight: "700"
  },
  webView: {
    backgroundColor: "#050B14",
    flex: 1
  }
});
