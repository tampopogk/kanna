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
import type { WebViewErrorEvent } from "react-native-webview/lib/WebViewTypes";
import type {
  ArtifactComment,
  ArtifactCommentInput,
  ArtifactDecision,
  ArtifactDecisionInput,
  ArtifactDetail,
  ArtifactFetchOutcome,
  ArtifactFileContent,
  ArtifactPushOutcome,
  ArtifactRemoteInfo
} from "../lib/api/types";
import {
  ArtifactBuildCancelled,
  artifactHostOpenPath,
  buildArtifactDocument,
  isolateArtifactDocument,
  type ArtifactDocument
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
 * trusted host document (`isolateArtifactDocument`): the engine refuses the
 * page's top-level navigation and new windows, and the policy refuses any
 * navigation of the frame. An in-tree link is a request to the host document,
 * which checks it against this tree's files and only then asks the viewer, by
 * a top-level `kanna-host:open` navigation the frame itself cannot make. The
 * viewer checks the path again, refuses the navigation and renders that one
 * file; only the page on screen is ever read. The whitelist is `*` on purpose —
 * react-native-webview hands any URL
 * that fails the whitelist to `Linking.openURL`, which would let a page open
 * Safari or another app.
 *
 * When Android's 250 ms answer window lapses, the engine goes ahead with the
 * host-open navigation, fails it (an unknown scheme) and reports a load error.
 * That error is the same request arriving late: the viewer opens the path from
 * it and covers the failed document with its own loading state, so the reader
 * never sees the library's error page, and the URL is not logged.
 *
 * Comments and decisions recorded here are records about the exact tree id on
 * screen. A decision moves no task: whoever owns the task operates its gate on
 * their own machine. Push and fetch use the repository's configured artifact
 * remote; the first push to a remote shows which config file chose it.
 */

/** What recording and sharing need; absent where the connection cannot do them. */
export interface ArtifactViewerActions {
  getArtifactRemote(repoId: string): Promise<ArtifactRemoteInfo>;
  recordArtifactComment(repoId: string, artifactId: string, input: ArtifactCommentInput): Promise<ArtifactComment>;
  recordArtifactDecision(repoId: string, artifactId: string, input: ArtifactDecisionInput): Promise<ArtifactDecision>;
  pushArtifact(repoId: string, artifactId: string): Promise<ArtifactPushOutcome>;
  fetchArtifact(repoId: string, artifactId: string): Promise<ArtifactFetchOutcome>;
}

export interface ArtifactViewerProps {
  repoId: string;
  initialArtifactId?: string;
  getArtifact(repoId: string, artifactId: string): Promise<ArtifactDetail>;
  readArtifactFile(repoId: string, artifactId: string, path: string): Promise<ArtifactFileContent>;
  actions?: ArtifactViewerActions;
  onClose(): void;
}

/**
 * Remotes pushed to from this app since it started, per repository and config
 * source. Held in memory only: after a restart the first push asks again.
 */
const confirmedRemotes = new Set<string>();

export function resetConfirmedArtifactRemotesForTests(): void {
  confirmedRemotes.clear();
}

function sameRemote(a: ArtifactRemoteInfo, b: ArtifactRemoteInfo): boolean {
  return a.remote === b.remote && a.source === b.source && a.configFile === b.configFile &&
    a.configured === b.configured && !a.error === !b.error;
}

function remoteKey(repoId: string, info: ArtifactRemoteInfo): string {
  return JSON.stringify([repoId, info.remote ?? "", info.source ?? ""]);
}

type RemoteOutcome =
  | { kind: "push"; outcome: ArtifactPushOutcome }
  | { kind: "fetch"; outcome: ArtifactFetchOutcome }
  | { kind: "error"; action: "push" | "fetch"; message: string };

type Unavailable = "missing" | "invalid" | "expired" | "error";

type DetailState =
  | { status: "idle" }
  | { status: "loading"; artifactId: string }
  | { status: "content"; artifactId: string; detail: ArtifactDetail }
  | { status: "unavailable"; artifactId: string; reason: Unavailable; message: string };

type DocumentState =
  | { status: "idle" }
  | { status: "loading"; key: string }
  | { status: "ready"; key: string; path: string; document: ArtifactDocument }
  | { status: "error"; key: string; message: string };

interface FileTarget {
  path: string;
  initialLine?: number;
}

const WebView = NativeWebView as unknown as React.ComponentType<WebViewProps>;
/**
 * How long a new page waits for an abandoned page's reads before reading
 * anyway. A sealed LAN or relay request that never answers must not hold every
 * later page; after this the reads overlap rather than wait forever.
 */
export const ABANDONED_READ_WAIT_MS = 3000;
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

/** Records read from the server, then any written here that a later read has not returned yet. */
function withRecorded<T extends { recordId: string }>(read: readonly T[] | undefined, written: readonly T[]): T[] {
  const known = new Set((read ?? []).map((record) => record.recordId));
  return [...(read ?? []), ...written.filter((record) => !known.has(record.recordId))];
}

function lineOf(position: string | undefined): number | undefined {
  const match = position?.match(/\bline\s*(\d+)/i) ?? position?.match(/^L?(\d+)$/i);
  const line = Number(match?.[1]);
  return Number.isInteger(line) && line > 0 ? line : undefined;
}

/**
 * A load error for a host-open navigation Android let through after its answer
 * window lapsed: the path it asked for, or null for any other failure.
 */
export function lateArtifactHostOpenPath(
  event: Pick<WebViewErrorEvent["nativeEvent"], "url">,
  files: ReadonlySet<string>
): string | null {
  return artifactHostOpenPath(event.url ?? "", files);
}

/**
 * The host document itself and the frame document it sets load; a host-open
 * request for a file of this tree is refused and handed to `open`; nothing
 * else loads.
 */
export function shouldStartArtifactLoad(
  request: Pick<WebViewNavigation, "url">,
  files: ReadonlySet<string>,
  open: (path: string) => void
): boolean {
  if (request.url === "about:blank" || request.url === "about:srcdoc") return true;
  const path = artifactHostOpenPath(request.url, files);
  if (path) open(path);
  return false;
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
  actions,
  onClose
}: ArtifactViewerProps) {
  const getArtifactRef = useRef(getArtifact);
  getArtifactRef.current = getArtifact;
  const readFileRef = useRef(readArtifactFile);
  readFileRef.current = readArtifactFile;
  const actionsRef = useRef(actions);
  actionsRef.current = actions;
  /** The tree id on screen now, for answers that arrive after the reader moved on. */
  const currentIdRef = useRef("");
  const canShare = Boolean(actions);

  const [input, setInput] = useState(initialArtifactId ?? "");
  const [currentId, setCurrentId] = useState(initialArtifactId ?? "");
  currentIdRef.current = currentId;
  const [newer, setNewer] = useState<string[]>([]);
  const [target, setTarget] = useState<FileTarget | null>(null);
  const [detailState, setDetailState] = useState<DetailState>({ status: "idle" });
  const [documentState, setDocumentState] = useState<DocumentState>({ status: "idle" });
  /** The last WebView load error was a late host-open, already being answered. */
  const [hostOpenFailed, setHostOpenFailed] = useState(false);
  /**
   * File reads still on the wire. A read over the sealed LAN channel or the
   * relay cannot be withdrawn once sent, so a new page waits for an abandoned
   * page's reads to settle instead of piling more on top of them.
   */
  const inflightReads = useRef(new Set<Promise<unknown>>());

  const [remoteInfo, setRemoteInfo] = useState<ArtifactRemoteInfo | null>(null);
  const [remoteInfoError, setRemoteInfoError] = useState("");
  const [remoteBusy, setRemoteBusy] = useState<"push" | "fetch" | null>(null);
  const [confirmingPush, setConfirmingPush] = useState(false);
  const [remoteOutcome, setRemoteOutcome] = useState<RemoteOutcome | null>(null);
  const [author, setAuthor] = useState("");
  const [commentBody, setCommentBody] = useState("");
  /** undefined: the file on screen; null: no file anchor. */
  const [anchorPath, setAnchorPath] = useState<string | null | undefined>(undefined);
  const [anchorPosition, setAnchorPosition] = useState("");
  const [anchorExcerpt, setAnchorExcerpt] = useState("");
  const [decisionWho, setDecisionWho] = useState("");
  const [decisionWhat, setDecisionWhat] = useState("");
  const [recording, setRecording] = useState(false);
  const [recordError, setRecordError] = useState("");
  /** Records written here since this version was read, shown with the ones read. */
  const [recorded, setRecorded] = useState<{
    artifactId: string;
    comments: ArtifactComment[];
    decisions: ArtifactDecision[];
  }>({ artifactId: "", comments: [], decisions: [] });

  const detail =
    detailState.status === "content" && detailState.artifactId === currentId
      ? detailState.detail
      : null;
  const latest = detail?.versions[detail.versions.length - 1] ?? null;
  const previousId = latest?.previous ?? null;
  /** A descriptor without the flags predates retention (T6b) and is treated as retained. */
  const retained =
    detail?.retained !== false && detail?.expired !== true;
  const filePath = target?.path ?? latest?.entrypoint ?? null;
  const documentKey = detail && retained && filePath
    ? `${repoId}\u0000${currentId}\u0000${filePath}\u0000${target?.initialLine ?? ""}`
    : null;
  /** Only records about this exact tree id; another version's notes are not this one's. */
  const comments = useMemo(
    () => withRecorded(detail?.comments, recorded.artifactId === currentId ? recorded.comments : [])
      .filter((comment) => comment.aboutArtifactId === currentId),
    [currentId, detail, recorded]
  );
  const decisions = useMemo(
    () => withRecorded(detail?.decisions, recorded.artifactId === currentId ? recorded.decisions : [])
      .filter((decision) => decision.aboutArtifactId === currentId),
    [currentId, detail, recorded]
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
    setConfirmingPush(false);
    setRecordError("");
    setAnchorPath(undefined);
    setRemoteOutcome((outcome) =>
      outcome?.kind === "fetch" && outcome.outcome.artifactId === artifactId ? outcome : null
    );
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
    setHostOpenFailed(false);
    const path = filePath;
    const inflight = inflightReads.current;
    let deadline: ReturnType<typeof setTimeout> | undefined;
    const abandoned = inflight.size
      ? Promise.race([
          Promise.allSettled([...inflight]),
          new Promise<void>((resolve) => {
            deadline = setTimeout(resolve, ABANDONED_READ_WAIT_MS);
          })
        ])
      : Promise.resolve();
    void buildArtifactDocument({
      path,
      initialLine: target?.initialLine,
      readFile: async (file) => {
        await abandoned;
        if (!active) throw new ArtifactBuildCancelled();
        const read = readFileRef.current(repoId, artifactId, file);
        inflight.add(read);
        const settle = () => inflight.delete(read);
        read.then(settle, settle);
        return read;
      },
      // A new target, a new version or closing the viewer stops further reads.
      isCancelled: () => !active
    }).then(
      (document) => {
        if (active) setDocumentState({ status: "ready", key: documentKey, path, document });
      },
      (error: unknown) => {
        if (active && !(error instanceof ArtifactBuildCancelled)) {
          setDocumentState({ status: "error", key: documentKey, message: errorMessage(error) });
        }
      }
    );
    return () => {
      active = false;
      if (deadline !== undefined) clearTimeout(deadline);
    };
    // `documentKey` names the repository, the tree, the file and the line.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [documentKey]);

  useEffect(() => {
    const current = actionsRef.current;
    if (!current) return;
    let active = true;
    setRemoteInfo(null);
    setRemoteInfoError("");
    current.getArtifactRemote(repoId).then(
      (info) => {
        if (active) setRemoteInfo(info);
      },
      (error: unknown) => {
        if (active) setRemoteInfoError(errorMessage(error));
      }
    );
    return () => {
      active = false;
    };
  }, [canShare, repoId]);

  const remoteReady = Boolean(remoteInfo?.configured && remoteInfo.remote && !remoteInfo.error);
  const remoteSource = remoteInfo?.source === "committed"
    ? `Chosen by the committed repo config (${remoteInfo.configFile ?? ".kanna/config.json"}); anyone who can commit to this repository decides where pushes go.`
    : remoteInfo?.source === "machine-local"
      ? `Chosen by the desktop’s machine-local config (${remoteInfo.configFile ?? ".kanna/config.local.json"}).`
      : "";

  /** The remote shown changed since the reader last saw it; they must look again. */
  const [remoteChanged, setRemoteChanged] = useState(false);

  /**
   * Resolve the remote again right before a push. The desktop resolves the
   * repository's configuration at push time, and a pull or an edit can change
   * it after this panel loaded; a push only goes ahead when the remote now in
   * force is the one the reader saw (and confirmed). Otherwise the panel shows
   * the new one and asks again.
   */
  const guardedPush = async (shown: ArtifactRemoteInfo, accepted: boolean) => {
    const artifactId = currentId;
    if (!actions || !shown.remote || !artifactId || remoteBusy) return;
    setRemoteBusy("push");
    setRemoteOutcome(null);
    try {
      const fresh = await actions.getArtifactRemote(repoId);
      if (!sameRemote(fresh, shown)) {
        setRemoteInfo(fresh);
        setRemoteChanged(true);
        setConfirmingPush(Boolean(fresh.configured && fresh.remote && fresh.source && !fresh.error));
        return;
      }
      setRemoteChanged(false);
      if (!accepted) {
        setConfirmingPush(true);
        return;
      }
      setConfirmingPush(false);
      const outcome = await actions.pushArtifact(repoId, artifactId);
      confirmedRemotes.add(remoteKey(repoId, shown));
      setRemoteOutcome({ kind: "push", outcome });
    } catch (error) {
      setRemoteOutcome({ kind: "error", action: "push", message: errorMessage(error) });
    } finally {
      setRemoteBusy(null);
    }
  };

  const requestPush = () => {
    if (!remoteInfo?.remote || !currentId || remoteBusy) return;
    void guardedPush(remoteInfo, confirmedRemotes.has(remoteKey(repoId, remoteInfo)));
  };

  const acceptPush = () => {
    if (remoteInfo) void guardedPush(remoteInfo, true);
  };

  /** Fetch the hash in the id field (or the one on screen) and show it. */
  const runFetch = async () => {
    const typed = input.trim().toLowerCase();
    const startedOn = currentId;
    const artifactId = ARTIFACT_ID.test(typed) ? typed : currentId;
    if (!actions || !ARTIFACT_ID.test(artifactId) || remoteBusy) return;
    setConfirmingPush(false);
    setRemoteBusy("fetch");
    setRemoteOutcome(null);
    try {
      const outcome = await actions.fetchArtifact(repoId, artifactId);
      // The reader moved to another version while this was on the wire: the
      // answer is about a screen that is gone, so it changes nothing.
      if (currentIdRef.current !== startedOn) return;
      setRemoteOutcome({ kind: "fetch", outcome });
      if (artifactId === startedOn) {
        setDetailState({ status: "content", artifactId, detail: outcome.detail });
      } else {
        setNewer([]);
        setCurrentId(artifactId);
        setInput(artifactId);
      }
    } catch (error) {
      if (currentIdRef.current === startedOn) {
        setRemoteOutcome({ kind: "error", action: "fetch", message: errorMessage(error) });
      }
    } finally {
      setRemoteBusy(null);
    }
  };

  /** The file a new comment is anchored to: the one on screen unless changed. */
  const commentAnchorPath = anchorPath === undefined ? filePath : anchorPath;

  const record = async (write: (artifactId: string) => Promise<void>) => {
    if (!currentId || recording) return;
    setRecording(true);
    setRecordError("");
    try {
      await write(currentId);
    } catch (error) {
      setRecordError(errorMessage(error));
    } finally {
      setRecording(false);
    }
  };

  const submitComment = () =>
    record(async (artifactId) => {
      if (!actions || !author.trim() || !commentBody.trim()) return;
      const anchor = {
        ...(commentAnchorPath ? { path: commentAnchorPath } : {}),
        ...(anchorPosition.trim() ? { position: anchorPosition.trim() } : {}),
        ...(anchorExcerpt.trim() ? { excerpt: anchorExcerpt.trim() } : {})
      };
      const comment = await actions.recordArtifactComment(repoId, artifactId, {
        author: author.trim(),
        body: commentBody.trim(),
        ...(Object.keys(anchor).length ? { anchor } : {})
      });
      setRecorded((previous) => ({
        artifactId,
        comments: [...(previous.artifactId === artifactId ? previous.comments : []), comment],
        decisions: previous.artifactId === artifactId ? previous.decisions : []
      }));
      setCommentBody("");
      setAnchorPosition("");
      setAnchorExcerpt("");
    });

  const submitDecision = () =>
    record(async (artifactId) => {
      if (!actions || !decisionWho.trim() || !decisionWhat.trim()) return;
      // A record about this tree id. It moves no task and operates no gate.
      const decision = await actions.recordArtifactDecision(repoId, artifactId, {
        who: decisionWho.trim(),
        what: decisionWhat.trim()
      });
      setRecorded((previous) => ({
        artifactId,
        comments: previous.artifactId === artifactId ? previous.comments : [],
        decisions: [...(previous.artifactId === artifactId ? previous.decisions : []), decision]
      }));
      setDecisionWhat("");
    });

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
  const files = useMemo(() => new Set((detail?.files ?? []).map((file) => file.path)), [detail]);
  const hostDocument = useMemo(
    () =>
      documentState.status === "ready"
        ? isolateArtifactDocument({
            page: documentState.document.html,
            current: documentState.path,
            files: [...files]
          })
        : "",
    [documentState, files]
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
          {actions ? (
            <Pressable
              accessibilityRole="button"
              accessibilityState={{ disabled: !remoteReady || remoteBusy !== null }}
              disabled={!remoteReady || remoteBusy !== null}
              onPress={() => void runFetch()}
              style={[styles.button, (!remoteReady || remoteBusy !== null) && styles.disabled]}
              testID="artifact-viewer-fetch"
            >
              <Text style={styles.buttonText}>{remoteBusy === "fetch" ? "Fetching…" : "Fetch"}</Text>
            </Pressable>
          ) : null}
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
              {detailState.reason === "missing" && remoteReady ? (
                <Pressable
                  accessibilityRole="button"
                  disabled={remoteBusy !== null}
                  onPress={() => void runFetch()}
                  style={[styles.button, styles.stateButton]}
                  testID="artifact-viewer-fetch-missing"
                >
                  <Text style={styles.buttonText}>
                    {remoteBusy === "fetch" ? "Fetching…" : "Fetch it from the artifact remote"}
                  </Text>
                </Pressable>
              ) : null}
              {remoteOutcome?.kind === "error" ? (
                <Text selectable style={styles.errorText} testID="artifact-viewer-remote-error">
                  {remoteOutcome.action === "push" ? "Push failed." : "Fetch failed."} {remoteOutcome.message}
                </Text>
              ) : null}
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
                  shouldStartArtifactLoad(request, files, (path) => setTarget({ path }))
                }
                onError={(event: WebViewErrorEvent) => {
                  const path = lateArtifactHostOpenPath(event.nativeEvent, files);
                  setHostOpenFailed(Boolean(path));
                  if (path) setTarget({ path });
                }}
                renderError={() => (
                  <View style={styles.webViewCover} testID="artifact-viewer-webview-error">
                    {hostOpenFailed ? (
                      <>
                        <ActivityIndicator color="#73b7ff" size="large" />
                        <Text style={styles.stateText}>Loading…</Text>
                      </>
                    ) : (
                      <Text style={styles.errorTitle}>Couldn’t show this page</Text>
                    )}
                  </View>
                )}
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
            {actions ? (
              <View testID="artifact-viewer-remote">
                <Text style={styles.sectionTitle}>Artifact remote</Text>
                {remoteInfoError ? (
                  <Text style={styles.recordError}>{remoteInfoError}</Text>
                ) : !remoteInfo ? (
                  <Text style={styles.meta}>Resolving the artifact remote…</Text>
                ) : !remoteInfo.configured ? (
                  <Text style={styles.meta} testID="artifact-viewer-remote-unconfigured">
                    No artifact remote is configured on the desktop. Set artifacts.remote in .kanna/config.local.json or .kanna/config.json.
                  </Text>
                ) : (
                  <>
                    {remoteInfo.remote ? (
                      <Text selectable style={styles.anchorPath} testID="artifact-viewer-remote-url">{remoteInfo.remote}</Text>
                    ) : null}
                    <Text style={styles.meta} testID="artifact-viewer-remote-source">{remoteSource}</Text>
                    {remoteInfo.error ? <Text style={styles.recordError}>{remoteInfo.error.message}</Text> : null}
                  </>
                )}
                {remoteChanged ? (
                  <Text style={styles.recordError} testID="artifact-viewer-remote-changed">
                    The artifact remote changed since this panel loaded. Nothing was pushed; check where it goes now.
                  </Text>
                ) : null}
                {confirmingPush && remoteInfo?.remote ? (
                  <View style={styles.confirm} testID="artifact-viewer-push-confirm">
                    <Text style={styles.recordBody}>
                      Push {currentId.slice(0, 12)}, its earlier versions and all their comments and decisions to {remoteInfo.remote}?
                    </Text>
                    <Text style={styles.recordAuthor}>{remoteSource}</Text>
                    <View style={styles.formRow}>
                      <Pressable accessibilityRole="button" disabled={remoteBusy !== null} onPress={acceptPush} style={styles.button} testID="artifact-viewer-push-accept">
                        <Text style={styles.buttonText}>Push</Text>
                      </Pressable>
                      <Pressable accessibilityRole="button" onPress={() => setConfirmingPush(false)} style={styles.button}>
                        <Text style={styles.buttonText}>Cancel</Text>
                      </Pressable>
                    </View>
                  </View>
                ) : (
                  <Pressable
                    accessibilityRole="button"
                    accessibilityState={{ disabled: !remoteReady || !retained || remoteBusy !== null }}
                    disabled={!remoteReady || !retained || remoteBusy !== null}
                    onPress={requestPush}
                    style={[styles.button, styles.formButton, (!remoteReady || !retained || remoteBusy !== null) && styles.disabled]}
                    testID="artifact-viewer-push"
                  >
                    <Text style={styles.buttonText}>{remoteBusy === "push" ? "Pushing…" : "Push this version"}</Text>
                  </Pressable>
                )}
                {remoteOutcome ? (
                  <View style={styles.confirm} testID="artifact-viewer-remote-outcome">
                    {remoteOutcome.kind === "push" ? (
                      <>
                        <Text style={styles.recordBody}>
                          Pushed to {remoteOutcome.outcome.remote}: {remoteOutcome.outcome.createdRefs.length} refs created, {remoteOutcome.outcome.upToDateRefs} already up to date.
                        </Text>
                        <Text style={styles.meta}>
                          Versions sent: {remoteOutcome.outcome.artifactIds.map((id) => id.slice(0, 12)).join(", ")}
                        </Text>
                      </>
                    ) : remoteOutcome.kind === "fetch" ? (
                      <>
                        <Text style={styles.recordBody}>
                          Fetched {remoteOutcome.outcome.fetched.map((id) => id.slice(0, 12)).join(", ")} from {remoteOutcome.outcome.remote}; {remoteOutcome.outcome.recordsImported} records imported.
                        </Text>
                        <Text style={styles.meta}>Received decisions are records; no task moved.</Text>
                        {remoteOutcome.outcome.refused.map((refused) => (
                          <Text key={refused.ref} style={styles.recordError} testID="artifact-viewer-fetch-refused">
                            Refused {refused.ref}: {refused.reason}
                          </Text>
                        ))}
                        {remoteOutcome.outcome.missing.length ? (
                          <Text style={styles.recordError} testID="artifact-viewer-fetch-missing-versions">
                            Earlier versions neither the remote nor the desktop holds: {remoteOutcome.outcome.missing.join(", ")}
                          </Text>
                        ) : null}
                      </>
                    ) : (
                      <Text style={styles.recordError} testID="artifact-viewer-remote-error">
                        {remoteOutcome.action === "push" ? "Push failed." : "Fetch failed."} {remoteOutcome.message}
                      </Text>
                    )}
                  </View>
                ) : null}
              </View>
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
            {actions ? (
              <View style={styles.form} testID="artifact-viewer-comment-form">
                <TextInput
                  accessibilityLabel="Comment author"
                  onChangeText={setAuthor}
                  placeholder="Your name"
                  placeholderTextColor="#5B6B82"
                  style={styles.formInput}
                  testID="artifact-viewer-comment-author"
                  value={author}
                />
                <TextInput
                  accessibilityLabel="Comment"
                  multiline
                  onChangeText={setCommentBody}
                  placeholder={`Comment on ${currentId.slice(0, 12)}`}
                  placeholderTextColor="#5B6B82"
                  style={[styles.formInput, styles.formMultiline]}
                  testID="artifact-viewer-comment-body"
                  value={commentBody}
                />
                <Text style={styles.meta}>Anchored to</Text>
                <ScrollView horizontal showsHorizontalScrollIndicator={false}>
                  <View style={styles.formRow}>
                    {[null, ...(detail?.files ?? []).map((file) => file.path)].map((path) => (
                      <Pressable
                        accessibilityRole="button"
                        accessibilityState={{ selected: commentAnchorPath === path }}
                        key={path ?? ""}
                        onPress={() => setAnchorPath(path)}
                        style={[styles.chip, commentAnchorPath === path && styles.chipSelected]}
                        testID={`artifact-viewer-anchor-choice-${path ?? "none"}`}
                      >
                        <Text style={styles.chipText}>{path ?? "No file"}</Text>
                      </Pressable>
                    ))}
                  </View>
                </ScrollView>
                <TextInput
                  accessibilityLabel="Anchor position"
                  onChangeText={setAnchorPosition}
                  placeholder="Position (e.g. line 4, #header)"
                  placeholderTextColor="#5B6B82"
                  style={styles.formInput}
                  testID="artifact-viewer-anchor-position-input"
                  value={anchorPosition}
                />
                <TextInput
                  accessibilityLabel="Anchor excerpt"
                  onChangeText={setAnchorExcerpt}
                  placeholder="Excerpt"
                  placeholderTextColor="#5B6B82"
                  style={styles.formInput}
                  testID="artifact-viewer-anchor-excerpt-input"
                  value={anchorExcerpt}
                />
                <Pressable
                  accessibilityRole="button"
                  disabled={recording || !author.trim() || !commentBody.trim()}
                  onPress={() => void submitComment()}
                  style={[styles.button, styles.formButton, (recording || !author.trim() || !commentBody.trim()) && styles.disabled]}
                  testID="artifact-viewer-comment-submit"
                >
                  <Text style={styles.buttonText}>Add comment</Text>
                </Pressable>
              </View>
            ) : null}
            <Text style={styles.sectionTitle}>Decisions on this version ({decisions.length})</Text>
            <Text style={styles.meta} testID="artifact-viewer-decision-note">
              A decision is a record about this version. It does not move any task or operate any gate; the task’s owner does that on their own machine.
            </Text>
            {decisions.map((decision) => (
              <View key={decision.recordId} style={styles.record} testID="artifact-viewer-decision">
                <Text style={styles.recordBody}>
                  <Text style={styles.recordAuthor}>{decision.who}</Text>: {decision.what}
                </Text>
                <Text style={styles.meta}>{decision.createdAt}</Text>
              </View>
            ))}
            {actions ? (
              <View style={styles.form} testID="artifact-viewer-decision-form">
                <TextInput
                  accessibilityLabel="Decision by"
                  onChangeText={setDecisionWho}
                  placeholder="Who"
                  placeholderTextColor="#5B6B82"
                  style={styles.formInput}
                  testID="artifact-viewer-decision-who"
                  value={decisionWho}
                />
                <TextInput
                  accessibilityLabel="Decision"
                  onChangeText={setDecisionWhat}
                  placeholder="Decision (e.g. approved)"
                  placeholderTextColor="#5B6B82"
                  style={styles.formInput}
                  testID="artifact-viewer-decision-what"
                  value={decisionWhat}
                />
                <Pressable
                  accessibilityRole="button"
                  disabled={recording || !decisionWho.trim() || !decisionWhat.trim()}
                  onPress={() => void submitDecision()}
                  style={[styles.button, styles.formButton, (recording || !decisionWho.trim() || !decisionWhat.trim()) && styles.disabled]}
                  testID="artifact-viewer-decision-submit"
                >
                  <Text style={styles.buttonText}>Record decision</Text>
                </Pressable>
              </View>
            ) : null}
            {recordError ? (
              <Text style={styles.recordError} testID="artifact-viewer-record-error">{recordError}</Text>
            ) : null}
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
    maxHeight: 320,
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
  anchorExcerpt: { color: "#AEBBD0", fontSize: 11, fontStyle: "italic" },
  stateButton: { marginTop: 14 },
  webViewCover: {
    alignItems: "center",
    backgroundColor: "#050B14",
    bottom: 0,
    justifyContent: "center",
    left: 0,
    position: "absolute",
    right: 0,
    top: 0
  },
  form: { gap: 6, paddingVertical: 6 },
  formRow: { flexDirection: "row", gap: 6 },
  formInput: {
    borderColor: "#31415B",
    borderRadius: 8,
    borderWidth: 1,
    color: "#D7E2F0",
    fontSize: 13,
    paddingHorizontal: 10,
    paddingVertical: 6
  },
  formMultiline: { minHeight: 56, textAlignVertical: "top" },
  formButton: { alignSelf: "flex-start", marginTop: 4 },
  chip: {
    borderColor: "#31415B",
    borderRadius: 12,
    borderWidth: 1,
    paddingHorizontal: 10,
    paddingVertical: 4
  },
  chipSelected: { backgroundColor: "#16304F", borderColor: "#73B7FF" },
  chipText: { color: "#D7E2F0", fontFamily: "Menlo", fontSize: 11 },
  confirm: {
    backgroundColor: "#0B1422",
    borderRadius: 8,
    gap: 4,
    marginTop: 6,
    paddingHorizontal: 10,
    paddingVertical: 8
  },
  recordError: { color: "#F3A6A0", fontSize: 12, marginTop: 4 }
});
