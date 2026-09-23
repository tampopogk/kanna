<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type {
  ArtifactComment,
  ArtifactDetail,
  ArtifactFetchOutcome,
  ArtifactPushOutcome,
  ArtifactRemoteInfo,
  ArtifactVersion,
} from "@kanna/core";
import {
  ArtifactRemoteError,
  ArtifactUnavailableError,
  fetchArtifact,
  fetchArtifactFromRemote,
  fetchArtifactRemoteInfo,
  pushArtifact,
  readArtifactFile,
  recordArtifactComment,
  recordArtifactDecision,
  type ArtifactUnavailableReason,
} from "../services/artifactClient";
import { acquireArtifactPreview, artifactFrameUrl, releaseArtifactPreview } from "../utils/artifactPreview";
import { isArtifactRemoteConfirmed, rememberArtifactRemoteConfirmed } from "../utils/artifactRemoteConfirmation";

/**
 * One repository's artifact at an exact tree id (spec §8).
 *
 * The frame lifecycle follows TaskPreviewView — resolve on show, a generation
 * counter so a stale answer never lands, release on unmount — but the access
 * model does not: the address comes from the artifact store's own preview
 * listener, minted per open, never from a task or a dev-server port.
 *
 * Shared HTML is untrusted. The frame is sandboxed with `allow-scripts` alone:
 * no same-origin (the page gets an opaque origin and cannot read this window),
 * no top navigation, no popups, no forms. The listener adds its own CSP
 * sandbox and `connect-src 'none'`, and its URL carries only a capability for
 * this one tree — never this window's control credential.
 *
 * Sharing goes through the repository's configured artifact remote (§8, §14):
 * push sends this version, its earlier versions and every record about them;
 * fetch receives a version by hash. The remote is configuration, never typed
 * here, and the first push to a remote shows which config file chose it.
 */
const props = defineProps<{ repoId: string; artifactId?: string; visible: boolean }>();
/** The tree id on screen, so the tab reopens where the reader left it. */
const emit = defineEmits<{ navigate: [artifactId: string] }>();
const { t } = useI18n();

const FRAME_SANDBOX = "allow-scripts";
const ARTIFACT_ID = /^[0-9a-f]{40}$/;

const input = ref(props.artifactId ?? "");
const currentId = ref(props.artifactId ?? "");
/** Newer ids left by following `previous`, so the reader can come back. */
const newer = ref<string[]>([]);
const detail = ref<ArtifactDetail | null>(null);
const unavailable = ref<{ reason: ArtifactUnavailableReason | "error"; message: string } | null>(null);
const loading = ref(false);
const previewUrl = ref("");
const previewError = ref("");
const framePath = ref<string | null>(null);
/** A non-page anchor (a stylesheet, a script) read as text, with its anchored line. */
const source = ref<{ path: string; lines: string[]; line: number | null; error?: string } | null>(null);
let heldPreview: { repoId: string; artifactId: string } | null = null;
let generation = 0;
/** The one file read in flight, withdrawn when the reader moves on. */
let fileRead: AbortController | null = null;

const latestVersion = computed<ArtifactVersion | null>(() => detail.value?.versions.at(-1) ?? null);
const previousId = computed(() => latestVersion.value?.previous ?? null);
/** A server that predates retention (T6b) sends neither flag: treated as retained and unbound. */
const retained = computed(() => detail.value?.retained !== false && detail.value?.expired !== true);
const bindings = computed(() => detail.value?.bindings ?? []);
/** Only records about this exact tree id; another version's notes are not this one's. */
const comments = computed<ArtifactComment[]>(() =>
  (detail.value?.comments ?? []).filter(comment => comment.aboutArtifactId === currentId.value));
const decisions = computed(() =>
  (detail.value?.decisions ?? []).filter(decision => decision.aboutArtifactId === currentId.value));
const frameSrc = computed(() => previewUrl.value ? artifactFrameUrl(previewUrl.value, framePath.value) : "");

function releaseHeld() {
  if (!heldPreview) return;
  const { repoId, artifactId } = heldPreview;
  heldPreview = null;
  void releaseArtifactPreview(repoId, artifactId);
}

function abortFileRead() {
  fileRead?.abort();
  fileRead = null;
}

async function load() {
  const request = ++generation;
  const repoId = props.repoId;
  const artifactId = currentId.value;
  releaseHeld();
  abortFileRead();
  detail.value = null;
  unavailable.value = null;
  previewUrl.value = "";
  previewError.value = "";
  framePath.value = null;
  source.value = null;
  if (!artifactId) return;
  loading.value = true;
  try {
    const loaded = await fetchArtifact(repoId, artifactId);
    if (request !== generation) return;
    detail.value = loaded;
    if (!retained.value || !loaded.versions.at(-1)?.entrypoint) return;
    try {
      const opened = await acquireArtifactPreview(repoId, artifactId);
      if (request !== generation) {
        void releaseArtifactPreview(repoId, artifactId);
        return;
      }
      heldPreview = { repoId, artifactId };
      previewUrl.value = opened.url;
    } catch (cause) {
      if (request === generation) previewError.value = messageOf(cause);
    }
  } catch (cause) {
    if (request !== generation) return;
    unavailable.value = cause instanceof ArtifactUnavailableError
      ? { reason: cause.reason, message: cause.message }
      : { reason: "error", message: messageOf(cause) };
  } finally {
    if (request === generation) loading.value = false;
  }
}

function messageOf(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function show(id: string) {
  if (id === currentId.value) return;
  newer.value = [];
  currentId.value = id;
}

function openInput() {
  const id = input.value.trim().toLowerCase();
  if (id) show(id);
}

function goPrevious() {
  if (!previousId.value) return;
  newer.value = [...newer.value, currentId.value];
  currentId.value = previousId.value;
}

function goNewer() {
  const next = newer.value.at(-1);
  if (!next) return;
  newer.value = newer.value.slice(0, -1);
  currentId.value = next;
}

const FRAMEABLE = /\.(html?|svg|png|jpe?g|gif|webp|avif|ico|mp4|webm|pdf)$/i;

function anchoredLine(position: string | undefined): number | null {
  const line = Number(position?.match(/\bline\s*(\d+)/i)?.[1] ?? position?.match(/^L?(\d+)$/i)?.[1]);
  return Number.isInteger(line) && line > 0 ? line : null;
}

async function showAnchor(comment: ArtifactComment) {
  const path = comment.anchor?.path;
  if (!path) return;
  abortFileRead();
  if (FRAMEABLE.test(path)) {
    source.value = null;
    framePath.value = path;
    return;
  }
  // A browser shows a bare stylesheet or script as nothing useful; read it
  // as text from the same tree and mark the anchored line. Choosing another
  // anchor or version withdraws this read rather than letting it finish.
  const request = generation;
  const artifactId = currentId.value;
  const line = anchoredLine(comment.anchor?.position);
  const controller = new AbortController();
  fileRead = controller;
  try {
    const file = await readArtifactFile(props.repoId, artifactId, path, controller.signal);
    if (request !== generation || controller.signal.aborted) return;
    const bytes = Uint8Array.from(atob(file.dataBase64), character => character.charCodeAt(0));
    source.value = { path: file.path, lines: new TextDecoder().decode(bytes).split(/\r?\n/), line };
  } catch (cause) {
    if (request === generation && !controller.signal.aborted) source.value = { path, lines: [], line, error: messageOf(cause) };
  } finally {
    if (fileRead === controller) fileRead = null;
  }
}

function showPreview() {
  abortFileRead();
  source.value = null;
  framePath.value = null;
}

watch(currentId, id => {
  input.value = id;
  resetRemoteOutcome();
  if (id) emit("navigate", id);
});
// A tab re-aimed from outside (an agent opening another id in this tab) moves
// the viewer; the viewer's own navigation comes back as the same id and is a
// no-op here.
watch(() => props.artifactId, id => { if (id) show(id); });
watch(() => [props.repoId, currentId.value] as const, () => {
  if (props.visible) void load();
  else { ++generation; releaseHeld(); abortFileRead(); detail.value = null; }
}, { immediate: true });
watch(() => props.visible, visible => {
  if (visible && !detail.value && !unavailable.value && !loading.value) void load();
});
onBeforeUnmount(() => { ++generation; releaseHeld(); abortFileRead(); });

// ---------------------------------------------------------------------------
// Artifact remote
// ---------------------------------------------------------------------------

const remoteInfo = ref<ArtifactRemoteInfo | null>(null);
const remoteInfoError = ref("");
const remoteBusy = ref<"push" | "fetch" | null>(null);
/** The push waiting on the reader to accept a remote it has not pushed to from here. */
const confirmingPush = ref(false);
type RemoteOutcome =
  | { kind: "push"; outcome: ArtifactPushOutcome }
  | { kind: "fetch"; outcome: ArtifactFetchOutcome }
  | { kind: "error"; action: "push" | "fetch"; message: string; refs: string[] };
const remoteOutcome = ref<RemoteOutcome | null>(null);

function resetRemoteOutcome() {
  confirmingPush.value = false;
  // A fetch that brought this id in stays readable beside it; anything else
  // was about another version.
  const outcome = remoteOutcome.value;
  if (!(outcome?.kind === "fetch" && outcome.outcome.artifactId === currentId.value)) remoteOutcome.value = null;
}

async function loadRemoteInfo() {
  const repoId = props.repoId;
  remoteInfoError.value = "";
  try {
    const info = await fetchArtifactRemoteInfo(repoId);
    if (repoId === props.repoId) remoteInfo.value = info;
  } catch (cause) {
    if (repoId === props.repoId) remoteInfoError.value = messageOf(cause);
  }
}

watch(() => [props.repoId, props.visible] as const, ([, visible]) => {
  if (visible && remoteInfo.value?.repoId !== props.repoId) void loadRemoteInfo();
}, { immediate: true });

const remoteReady = computed(() => Boolean(remoteInfo.value?.configured && remoteInfo.value.remote && !remoteInfo.value.error));
const remoteSourceLabel = computed(() => {
  const info = remoteInfo.value;
  if (!info?.source) return "";
  return info.source === "committed"
    ? t("artifactViewer.remote.sourceCommitted", { file: info.configFile ?? ".kanna/config.json" })
    : t("artifactViewer.remote.sourceLocal", { file: info.configFile ?? ".kanna/config.local.json" });
});

/** The remote shown changed since the reader last saw it; they must look again. */
const remoteChanged = ref(false);
/** The reader withdrew a push before it answered. */
const pushCancelled = ref(false);
/** One push in flight, from acceptance to its answer: Cancel aborts it. */
let pushController: AbortController | null = null;

function remoteUsable(info: ArtifactRemoteInfo | null): boolean {
  return Boolean(info?.configured && info.remote && info.source && info.fingerprint && !info.error);
}

function requestPush() {
  const info = remoteInfo.value;
  if (!remoteUsable(info) || !currentId.value || remoteBusy.value) return;
  pushCancelled.value = false;
  if (isArtifactRemoteConfirmed(props.repoId, info!.remote!, info!.source!)) void startPush(info!);
  else confirmingPush.value = true;
}

/** The reader accepted the remote in the confirmation. */
function acceptPush() {
  const info = remoteInfo.value;
  if (!remoteUsable(info) || !currentId.value || remoteBusy.value) return;
  void startPush(info!);
}

function cancelPush() {
  if (pushController) {
    pushController.abort();
    pushCancelled.value = true;
  }
  confirmingPush.value = false;
}

/**
 * Push bound to the remote shown. The server resolves the configuration at
 * push time and refuses (`artifact_remote_changed`) if it no longer names the
 * remote this fingerprint identifies, before contacting any remote; the panel
 * then shows the remote now in force and asks again. Cancel aborts whichever
 * request is in flight. A push the desktop server already started may still
 * complete; nothing here reports it as done.
 */
async function startPush(shown: ArtifactRemoteInfo) {
  const repoId = props.repoId;
  const artifactId = currentId.value;
  const controller = new AbortController();
  pushController = controller;
  remoteBusy.value = "push";
  remoteOutcome.value = null;
  remoteChanged.value = false;
  try {
    const outcome = await pushArtifact(repoId, artifactId, { remoteFingerprint: shown.fingerprint! }, controller.signal);
    if (controller.signal.aborted) return;
    confirmingPush.value = false;
    rememberArtifactRemoteConfirmed(repoId, shown.remote!, shown.source!);
    if (artifactId === currentId.value) remoteOutcome.value = { kind: "push", outcome };
  } catch (cause) {
    if (controller.signal.aborted) return;
    if (cause instanceof ArtifactRemoteError && cause.code === "artifact_remote_changed") {
      try {
        const fresh = await fetchArtifactRemoteInfo(repoId, controller.signal);
        if (controller.signal.aborted || repoId !== props.repoId) return;
        remoteInfo.value = fresh;
        remoteChanged.value = true;
        confirmingPush.value = remoteUsable(fresh);
      } catch (refresh) {
        if (!controller.signal.aborted) remoteInfoError.value = messageOf(refresh);
      }
      return;
    }
    confirmingPush.value = false;
    if (artifactId === currentId.value) remoteOutcome.value = remoteFailure("push", cause);
  } finally {
    if (pushController === controller) {
      pushController = null;
      remoteBusy.value = null;
    }
  }
}

/** Fetch the hash in the id field (or the one on screen) and show it. */
async function runFetch() {
  const typed = input.value.trim().toLowerCase();
  const artifactId = ARTIFACT_ID.test(typed) ? typed : currentId.value;
  if (!ARTIFACT_ID.test(artifactId) || remoteBusy.value) return;
  confirmingPush.value = false;
  remoteBusy.value = "fetch";
  remoteOutcome.value = null;
  try {
    const outcome = await fetchArtifactFromRemote(props.repoId, artifactId);
    remoteOutcome.value = { kind: "fetch", outcome };
    if (artifactId === currentId.value) void load();
    else show(artifactId);
  } catch (cause) {
    remoteOutcome.value = remoteFailure("fetch", cause);
  } finally {
    remoteBusy.value = null;
  }
}

function remoteFailure(action: "push" | "fetch", cause: unknown): RemoteOutcome {
  return {
    kind: "error",
    action,
    message: messageOf(cause),
    refs: cause instanceof ArtifactRemoteError ? cause.refs : [],
  };
}

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------

const commentForm = ref({ author: "", body: "", path: "", position: "", excerpt: "" });
const decisionForm = ref({ who: "", what: "" });
const recordError = ref("");
const recording = ref(false);

async function submitComment() {
  const form = commentForm.value;
  if (!form.author.trim() || !form.body.trim() || recording.value) return;
  const anchor = {
    ...(form.path ? { path: form.path } : {}),
    ...(form.position.trim() ? { position: form.position.trim() } : {}),
    ...(form.excerpt.trim() ? { excerpt: form.excerpt.trim() } : {}),
  };
  await record(async (repoId, artifactId) => {
    const comment = await recordArtifactComment(repoId, artifactId, {
      author: form.author.trim(),
      body: form.body.trim(),
      ...(Object.keys(anchor).length ? { anchor } : {}),
    });
    detail.value?.comments.push(comment);
    commentForm.value = { ...commentForm.value, body: "", position: "", excerpt: "" };
  });
}

async function submitDecision() {
  const form = decisionForm.value;
  if (!form.who.trim() || !form.what.trim() || recording.value) return;
  await record(async (repoId, artifactId) => {
    // A record about this tree id. It moves no task and operates no gate.
    const decision = await recordArtifactDecision(repoId, artifactId, { who: form.who.trim(), what: form.what.trim() });
    detail.value?.decisions.push(decision);
    decisionForm.value = { ...decisionForm.value, what: "" };
  });
}

async function record(write: (repoId: string, artifactId: string) => Promise<void>) {
  const artifactId = currentId.value;
  recording.value = true;
  recordError.value = "";
  try {
    await write(props.repoId, artifactId);
  } catch (cause) {
    recordError.value = messageOf(cause);
  } finally {
    recording.value = false;
  }
}

function shortId(id: string): string {
  return id.slice(0, 12);
}

const UNAVAILABLE_KEYS: Record<ArtifactUnavailableReason | "error", string> = {
  "not-found": "artifactViewer.unavailable.notFound",
  "invalid-id": "artifactViewer.unavailable.invalidId",
  "content-missing": "artifactViewer.unavailable.contentMissing",
  "no-entrypoint": "artifactViewer.unavailable.noEntrypoint",
  "repo-not-found": "artifactViewer.unavailable.repoNotFound",
  error: "artifactViewer.unavailable.error",
};
</script>
<template>
  <section class="artifact-viewer" data-testid="artifact-viewer" tabindex="-1">
    <form class="artifact-toolbar" @submit.prevent="openInput">
      <input v-model="input" data-testid="artifact-id-input" :aria-label="t('artifactViewer.idLabel')" :placeholder="t('artifactViewer.idPlaceholder')" spellcheck="false" autocomplete="off" />
      <button type="submit">{{ t("artifactViewer.open") }}</button>
      <button type="button" data-testid="artifact-fetch" :disabled="!remoteReady || remoteBusy !== null" :title="t('artifactViewer.remote.fetchTitle')" @click="runFetch">
        {{ remoteBusy === "fetch" ? t("artifactViewer.remote.fetching") : t("artifactViewer.remote.fetch") }}
      </button>
      <button type="button" data-testid="artifact-newer" :disabled="!newer.length" @click="goNewer">{{ t("artifactViewer.newer") }}</button>
      <button type="button" data-testid="artifact-previous" :disabled="!previousId" @click="goPrevious">{{ t("artifactViewer.previous") }}</button>
      <button type="button" :disabled="!currentId" @click="load">{{ t("artifactViewer.reload") }}</button>
    </form>
    <p v-if="!currentId">{{ t("artifactViewer.enterId") }}</p>
    <p v-else-if="loading">{{ t("artifactViewer.reading", { id: shortId(currentId) }) }}</p>
    <div v-else-if="unavailable" class="artifact-state" role="alert" data-testid="artifact-unavailable" :data-reason="unavailable.reason">
      <strong>{{ t(UNAVAILABLE_KEYS[unavailable.reason]) }}</strong>
      <code>{{ currentId }}</code>
      <span>{{ unavailable.message }}</span>
      <button v-if="unavailable.reason === 'not-found' && remoteReady" type="button" data-testid="artifact-fetch-missing" :disabled="remoteBusy !== null" @click="runFetch">
        {{ t("artifactViewer.remote.fetchMissing") }}
      </button>
      <p v-if="remoteBusy === 'fetch'">{{ t("artifactViewer.remote.fetching") }}</p>
      <p v-if="remoteOutcome?.kind === 'error'" role="alert" data-testid="artifact-remote-outcome" data-kind="error">
        {{ remoteOutcome.action === "push" ? t("artifactViewer.remote.pushFailed") : t("artifactViewer.remote.fetchFailed") }} {{ remoteOutcome.message }}
      </p>
    </div>
    <div v-else-if="detail" class="artifact-body">
      <div class="artifact-content">
        <div v-if="!retained" class="artifact-state" data-testid="artifact-expired">
          <strong>{{ t("artifactViewer.expiredTitle") }}</strong>
          <span>{{ t("artifactViewer.expiredBody") }}</span>
        </div>
        <p v-else-if="!latestVersion?.entrypoint">{{ t("artifactViewer.noEntrypoint") }}</p>
        <p v-else-if="previewError" role="alert">{{ previewError }}</p>
        <div v-else-if="source" class="artifact-source" data-testid="artifact-source">
          <div class="artifact-source-bar"><code>{{ source.path }}</code><button type="button" @click="showPreview">{{ t("artifactViewer.backToPreview") }}</button></div>
          <p v-if="source.error" role="alert">{{ source.error }}</p>
          <pre v-else><span v-for="(text, index) in source.lines" :key="index" :class="{ anchored: source.line === index + 1 }" :data-line="index + 1">{{ text }}
</span></pre>
        </div>
        <p v-else-if="!frameSrc">{{ t("artifactViewer.openingPreview") }}</p>
        <iframe
          v-else
          data-testid="artifact-frame"
          :src="frameSrc"
          :title="t('artifactViewer.frameTitle', { id: shortId(currentId) })"
          :sandbox="FRAME_SANDBOX"
          allow=""
          referrerpolicy="no-referrer"
        />
      </div>
      <aside class="artifact-side">
        <section>
          <h3>{{ t("artifactViewer.version") }} <code data-testid="artifact-current-id">{{ shortId(currentId) }}</code></h3>
          <dl v-if="latestVersion">
            <dt>{{ t("artifactViewer.kind") }}</dt><dd>{{ latestVersion.kind }}</dd>
            <dt>{{ t("artifactViewer.published") }}</dt><dd>{{ latestVersion.createdAt }}</dd>
            <dt>{{ t("artifactViewer.byTask") }}</dt><dd><code>{{ latestVersion.producedBy.taskId }}</code></dd>
            <dt>{{ t("artifactViewer.retention") }}</dt><dd>{{ latestVersion.retention }}</dd>
            <dt>{{ t("artifactViewer.previousLabel") }}</dt><dd><code v-if="previousId">{{ shortId(previousId) }}</code><span v-else>{{ t("artifactViewer.none") }}</span></dd>
          </dl>
          <ul v-if="bindings.length" class="artifact-files" data-testid="artifact-bindings">
            <li v-for="binding in bindings" :key="`${binding.taskId}:${binding.name}`">{{ t("artifactViewer.boundBy", { name: binding.name, task: binding.taskId }) }}</li>
          </ul>
        </section>
        <section class="artifact-remote" data-testid="artifact-remote">
          <h3>{{ t("artifactViewer.remote.title") }}</h3>
          <p v-if="remoteInfoError" role="alert">{{ remoteInfoError }}</p>
          <p v-else-if="!remoteInfo">{{ t("artifactViewer.remote.resolving") }}</p>
          <p v-else-if="!remoteInfo.configured" data-testid="artifact-remote-unconfigured">{{ t("artifactViewer.remote.unconfigured") }}</p>
          <template v-else>
            <p v-if="remoteInfo.remote"><code data-testid="artifact-remote-url">{{ remoteInfo.remote }}</code></p>
            <p class="muted" data-testid="artifact-remote-source" :data-source="remoteInfo.source">{{ remoteSourceLabel }}</p>
            <p v-if="remoteInfo.error" role="alert" data-testid="artifact-remote-invalid">{{ remoteInfo.error.message }}</p>
          </template>
          <p v-if="remoteChanged" role="alert" data-testid="artifact-remote-changed">{{ t("artifactViewer.remote.changed") }}</p>
          <div v-if="(confirmingPush || remoteBusy === 'push') && remoteInfo?.remote" class="artifact-confirm" role="alertdialog" :data-testid="confirmingPush ? 'artifact-push-confirm' : 'artifact-push-progress'">
            <p>{{ t("artifactViewer.remote.confirmPush", { id: shortId(currentId), remote: remoteInfo.remote }) }}</p>
            <p><strong>{{ remoteSourceLabel }}</strong></p>
            <div class="artifact-confirm-actions">
              <button type="button" data-testid="artifact-push-confirm-accept" :disabled="remoteBusy !== null" @click="acceptPush">
                {{ remoteBusy === "push" ? t("artifactViewer.remote.pushing") : t("artifactViewer.remote.confirmAccept") }}
              </button>
              <button type="button" data-testid="artifact-push-cancel" @click="cancelPush">{{ t("artifactViewer.remote.confirmCancel") }}</button>
            </div>
          </div>
          <button v-else type="button" data-testid="artifact-push" :disabled="!remoteReady || !retained || remoteBusy !== null" @click="requestPush">
            {{ t("artifactViewer.remote.push") }}
          </button>
          <p v-if="pushCancelled" class="muted" data-testid="artifact-push-cancelled">{{ t("artifactViewer.remote.pushCancelled") }}</p>
          <div v-if="remoteOutcome" class="artifact-outcome" data-testid="artifact-remote-outcome" :data-kind="remoteOutcome.kind">
            <template v-if="remoteOutcome.kind === 'push'">
              <p>{{ t("artifactViewer.remote.pushed", { remote: remoteOutcome.outcome.remote, created: remoteOutcome.outcome.createdRefs.length, upToDate: remoteOutcome.outcome.upToDateRefs }) }}</p>
              <p class="muted">{{ t("artifactViewer.remote.versions", { ids: remoteOutcome.outcome.artifactIds.map(shortId).join(", ") }) }}</p>
              <details v-if="remoteOutcome.outcome.createdRefs.length">
                <summary>{{ t("artifactViewer.remote.createdRefs") }}</summary>
                <ul class="artifact-refs"><li v-for="name in remoteOutcome.outcome.createdRefs" :key="name"><code>{{ name }}</code></li></ul>
              </details>
            </template>
            <template v-else-if="remoteOutcome.kind === 'fetch'">
              <p>{{ t("artifactViewer.remote.fetched", { remote: remoteOutcome.outcome.remote, ids: remoteOutcome.outcome.fetched.map(shortId).join(", "), records: remoteOutcome.outcome.recordsImported }) }}</p>
              <p class="muted">{{ t("artifactViewer.remote.decisionsAreData") }}</p>
              <div v-if="remoteOutcome.outcome.refused.length" data-testid="artifact-fetch-refused">
                <p>{{ t("artifactViewer.remote.refused") }}</p>
                <ul class="artifact-refs"><li v-for="refused in remoteOutcome.outcome.refused" :key="refused.ref"><code>{{ refused.ref }}</code> — {{ refused.reason }}</li></ul>
              </div>
              <div v-if="remoteOutcome.outcome.missing.length" data-testid="artifact-fetch-missing-versions">
                <p>{{ t("artifactViewer.remote.missing") }}</p>
                <ul class="artifact-refs"><li v-for="id in remoteOutcome.outcome.missing" :key="id"><code>{{ id }}</code></li></ul>
              </div>
            </template>
            <template v-else>
              <p role="alert">{{ remoteOutcome.action === "push" ? t("artifactViewer.remote.pushFailed") : t("artifactViewer.remote.fetchFailed") }} {{ remoteOutcome.message }}</p>
              <ul v-if="remoteOutcome.refs.length" class="artifact-refs" data-testid="artifact-push-refused">
                <li v-for="name in remoteOutcome.refs" :key="name"><code>{{ name }}</code></li>
              </ul>
            </template>
          </div>
        </section>
        <section v-if="detail.files.length">
          <h3>{{ t("artifactViewer.files") }}</h3>
          <ul class="artifact-files">
            <li v-for="file in detail.files" :key="file.path"><code>{{ file.path }}</code> <span>{{ t("artifactViewer.bytes", { size: file.size }) }}</span></li>
          </ul>
        </section>
        <section data-testid="artifact-comments">
          <h3>{{ t("artifactViewer.commentsTitle") }}</h3>
          <p v-if="!comments.length" class="muted">{{ t("artifactViewer.noComments", { id: shortId(currentId) }) }}</p>
          <article v-for="comment in comments" :key="comment.recordId" class="artifact-comment" data-testid="artifact-comment">
            <header><strong>{{ comment.author }}</strong> <time>{{ comment.createdAt }}</time></header>
            <p>{{ comment.body }}</p>
            <button v-if="comment.anchor" type="button" class="artifact-anchor" data-testid="artifact-anchor" :disabled="!comment.anchor.path || !frameSrc" @click="showAnchor(comment)">
              <code v-if="comment.anchor.path" data-testid="artifact-anchor-path">{{ comment.anchor.path }}</code>
              <span v-if="comment.anchor.position" data-testid="artifact-anchor-position">@ {{ comment.anchor.position }}</span>
              <q v-if="comment.anchor.excerpt" data-testid="artifact-anchor-excerpt">{{ comment.anchor.excerpt }}</q>
            </button>
          </article>
          <form class="artifact-form" data-testid="artifact-comment-form" @submit.prevent="submitComment">
            <input v-model="commentForm.author" :aria-label="t('artifactViewer.commentAuthor')" :placeholder="t('artifactViewer.yourName')" />
            <textarea v-model="commentForm.body" :aria-label="t('artifactViewer.comment')" :placeholder="t('artifactViewer.commentPlaceholder')" rows="2" />
            <select v-model="commentForm.path" :aria-label="t('artifactViewer.anchorFile')">
              <option value="">{{ t("artifactViewer.noFileAnchor") }}</option>
              <option v-for="file in detail.files" :key="file.path" :value="file.path">{{ file.path }}</option>
            </select>
            <input v-model="commentForm.position" :aria-label="t('artifactViewer.anchorPosition')" :placeholder="t('artifactViewer.anchorPositionPlaceholder')" />
            <input v-model="commentForm.excerpt" :aria-label="t('artifactViewer.anchorExcerpt')" :placeholder="t('artifactViewer.anchorExcerptPlaceholder')" />
            <button type="submit" :disabled="recording">{{ t("artifactViewer.addComment") }}</button>
          </form>
        </section>
        <section data-testid="artifact-decisions">
          <h3>{{ t("artifactViewer.decisionsTitle") }}</h3>
          <p class="muted" data-testid="artifact-decision-note">{{ t("artifactViewer.decisionNote") }}</p>
          <article v-for="decision in decisions" :key="decision.recordId" class="artifact-decision" data-testid="artifact-decision">
            <strong>{{ decision.who }}</strong>: {{ decision.what }} <time>{{ decision.createdAt }}</time>
          </article>
          <form class="artifact-form" data-testid="artifact-decision-form" @submit.prevent="submitDecision">
            <input v-model="decisionForm.who" :aria-label="t('artifactViewer.decisionBy')" :placeholder="t('artifactViewer.who')" />
            <input v-model="decisionForm.what" :aria-label="t('artifactViewer.decision')" :placeholder="t('artifactViewer.decisionPlaceholder')" />
            <button type="submit" :disabled="recording">{{ t("artifactViewer.recordDecision") }}</button>
          </form>
        </section>
        <p v-if="recordError" role="alert">{{ recordError }}</p>
      </aside>
    </div>
  </section>
</template>
<style scoped>
.artifact-viewer { display: flex; flex: 1; flex-direction: column; min-height: 0; min-width: 0; font-size: 12px; }
.artifact-toolbar { display: flex; gap: 6px; align-items: center; padding: 8px 12px; border-bottom: 1px solid var(--kn-border-default); }
.artifact-toolbar input { flex: 1; min-width: 0; font-family: var(--kn-font-mono, ui-monospace, monospace); }
input, textarea, select { background: var(--kn-bg-panel); color: var(--kn-text-primary); border: 1px solid var(--kn-border-default); border-radius: 4px; padding: 3px 6px; font: inherit; }
button { background: var(--kn-bg-panel-raised); color: var(--kn-text-secondary); border: 1px solid var(--kn-border-default); border-radius: 4px; padding: 3px 6px; cursor: pointer; }
button:disabled { cursor: default; opacity: 0.5; }
p { margin: 8px 12px; color: var(--kn-text-muted); }
.artifact-state { display: flex; flex-direction: column; gap: 4px; margin: 12px; padding: 10px; border: 1px solid var(--kn-border-default); border-radius: 6px; color: var(--kn-text-secondary); }
.artifact-state button { align-self: flex-start; }
.artifact-body { display: flex; flex: 1; min-height: 0; }
.artifact-content { display: flex; flex: 1; flex-direction: column; min-width: 0; }
iframe { flex: 1; min-height: 0; width: 100%; border: 0; background: white; }
.artifact-side { width: 320px; flex-shrink: 0; overflow-y: auto; border-left: 1px solid var(--kn-border-default); padding: 0 12px 12px; color: var(--kn-text-secondary); }
.artifact-side h3 { font-size: 12px; margin: 12px 0 6px; color: var(--kn-text-primary); }
.artifact-side p { margin: 4px 0; }
dl { display: grid; grid-template-columns: auto 1fr; gap: 2px 8px; margin: 0; }
dt { color: var(--kn-text-muted); }
dd { margin: 0; overflow-wrap: anywhere; }
.artifact-files, .artifact-refs { list-style: none; margin: 0; padding: 0; }
.artifact-files span { color: var(--kn-text-muted); }
.artifact-refs code, .artifact-remote code { overflow-wrap: anywhere; }
.artifact-confirm, .artifact-outcome { border: 1px solid var(--kn-border-default); border-radius: 6px; padding: 6px 8px; margin-top: 6px; }
.artifact-confirm-actions { display: flex; gap: 6px; }
.artifact-comment, .artifact-decision { border-top: 1px solid var(--kn-border-default); padding: 6px 0; }
.artifact-comment header time, .artifact-decision time { color: var(--kn-text-muted); font-size: 11px; }
.artifact-anchor { display: flex; flex-wrap: wrap; gap: 4px; text-align: left; width: 100%; }
.artifact-anchor q { font-style: italic; }
.artifact-form { display: flex; flex-direction: column; gap: 4px; margin-top: 6px; }
.muted { color: var(--kn-text-muted); }
.artifact-source { display: flex; flex: 1; flex-direction: column; min-height: 0; }
.artifact-source-bar { display: flex; gap: 8px; align-items: center; justify-content: space-between; padding: 6px 12px; border-bottom: 1px solid var(--kn-border-default); }
.artifact-source pre { flex: 1; margin: 0; overflow: auto; padding: 8px 12px; font-family: var(--kn-font-mono, ui-monospace, monospace); color: var(--kn-text-primary); }
.artifact-source .anchored { background: var(--kn-bg-selection, rgba(90, 155, 255, 0.28)); }
</style>
