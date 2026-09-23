<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import type { ArtifactComment, ArtifactDetail, ArtifactVersion } from "@kanna/core";
import {
  ArtifactUnavailableError,
  fetchArtifact,
  readArtifactFile,
  recordArtifactComment,
  recordArtifactDecision,
  type ArtifactUnavailableReason,
} from "../services/artifactClient";
import { acquireArtifactPreview, artifactFrameUrl, releaseArtifactPreview } from "../utils/artifactPreview";

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
 */
const props = defineProps<{ repoId: string; artifactId?: string; visible: boolean }>();

const FRAME_SANDBOX = "allow-scripts";

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

const latestVersion = computed<ArtifactVersion | null>(() => detail.value?.versions.at(-1) ?? null);
const previousId = computed(() => latestVersion.value?.previous ?? null);
/** A descriptor without the flag predates retention and is treated as retained. */
const retained = computed(() => detail.value?.retained !== false);
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

async function load() {
  const request = ++generation;
  const repoId = props.repoId;
  const artifactId = currentId.value;
  releaseHeld();
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
    if (loaded.retained === false || !loaded.versions.at(-1)?.entrypoint) return;
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

function openInput() {
  const id = input.value.trim().toLowerCase();
  if (!id || id === currentId.value) return;
  newer.value = [];
  currentId.value = id;
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
  if (FRAMEABLE.test(path)) {
    source.value = null;
    framePath.value = path;
    return;
  }
  // A browser shows a bare stylesheet or script as nothing useful; read it
  // as text from the same tree and mark the anchored line.
  const request = generation;
  const artifactId = currentId.value;
  const line = anchoredLine(comment.anchor?.position);
  try {
    const file = await readArtifactFile(props.repoId, artifactId, path);
    if (request !== generation) return;
    const bytes = Uint8Array.from(atob(file.dataBase64), character => character.charCodeAt(0));
    source.value = { path: file.path, lines: new TextDecoder().decode(bytes).split(/\r?\n/), line };
  } catch (cause) {
    if (request === generation) source.value = { path, lines: [], line, error: messageOf(cause) };
  }
}

function showPreview() {
  source.value = null;
  framePath.value = null;
}

watch(currentId, id => { input.value = id; });
watch(() => [props.repoId, currentId.value] as const, () => {
  if (props.visible) void load();
  else { ++generation; releaseHeld(); detail.value = null; }
}, { immediate: true });
watch(() => props.visible, visible => {
  if (visible && !detail.value && !unavailable.value && !loading.value) void load();
});
onBeforeUnmount(() => { ++generation; releaseHeld(); });

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

const UNAVAILABLE_TITLES: Record<ArtifactUnavailableReason | "error", string> = {
  "not-found": "No artifact with this id in this repository",
  "invalid-id": "Not an artifact id",
  "content-missing": "Produced, no longer retained",
  "no-entrypoint": "Nothing to open",
  "repo-not-found": "Repository not found on this machine",
  error: "Couldn’t read the artifact",
};
</script>
<template>
  <section class="artifact-viewer" data-testid="artifact-viewer" tabindex="-1">
    <form class="artifact-toolbar" @submit.prevent="openInput">
      <input v-model="input" data-testid="artifact-id-input" aria-label="Artifact tree id" placeholder="Artifact tree id (40 hex)" spellcheck="false" autocomplete="off" />
      <button type="submit">Open</button>
      <button type="button" data-testid="artifact-newer" :disabled="!newer.length" @click="goNewer">Newer</button>
      <button type="button" data-testid="artifact-previous" :disabled="!previousId" @click="goPrevious">Previous version</button>
      <button type="button" :disabled="!currentId" @click="load">Reload</button>
    </form>
    <p v-if="!currentId">Enter the tree id of an artifact in this repository.</p>
    <p v-else-if="loading">Reading artifact {{ shortId(currentId) }}…</p>
    <div v-else-if="unavailable" class="artifact-state" role="alert" data-testid="artifact-unavailable" :data-reason="unavailable.reason">
      <strong>{{ UNAVAILABLE_TITLES[unavailable.reason] }}</strong>
      <code>{{ currentId }}</code>
      <span>{{ unavailable.message }}</span>
    </div>
    <div v-else-if="detail" class="artifact-body">
      <div class="artifact-content">
        <div v-if="!retained" class="artifact-state" data-testid="artifact-expired">
          <strong>Produced, no longer retained</strong>
          <span>The records below still name this version; its content has been discarded by retention policy.</span>
        </div>
        <p v-else-if="!latestVersion?.entrypoint">This artifact has no entrypoint to open; its files are listed alongside.</p>
        <p v-else-if="previewError" role="alert">{{ previewError }}</p>
        <div v-else-if="source" class="artifact-source" data-testid="artifact-source">
          <div class="artifact-source-bar"><code>{{ source.path }}</code><button type="button" @click="showPreview">Back to preview</button></div>
          <p v-if="source.error" role="alert">{{ source.error }}</p>
          <pre v-else><span v-for="(text, index) in source.lines" :key="index" :class="{ anchored: source.line === index + 1 }" :data-line="index + 1">{{ text }}
</span></pre>
        </div>
        <p v-else-if="!frameSrc">Opening preview…</p>
        <iframe
          v-else
          data-testid="artifact-frame"
          :src="frameSrc"
          :title="`Artifact ${shortId(currentId)}`"
          :sandbox="FRAME_SANDBOX"
          allow=""
          referrerpolicy="no-referrer"
        />
      </div>
      <aside class="artifact-side">
        <section>
          <h3>Version <code data-testid="artifact-current-id">{{ shortId(currentId) }}</code></h3>
          <dl v-if="latestVersion">
            <dt>Kind</dt><dd>{{ latestVersion.kind }}</dd>
            <dt>Published</dt><dd>{{ latestVersion.createdAt }}</dd>
            <dt>By task</dt><dd><code>{{ latestVersion.producedBy.taskId }}</code></dd>
            <dt>Retention</dt><dd>{{ latestVersion.retention }}</dd>
            <dt>Previous</dt><dd><code v-if="previousId">{{ shortId(previousId) }}</code><span v-else>none</span></dd>
          </dl>
        </section>
        <section v-if="detail.files.length">
          <h3>Files</h3>
          <ul class="artifact-files">
            <li v-for="file in detail.files" :key="file.path"><code>{{ file.path }}</code> <span>{{ file.size }} B</span></li>
          </ul>
        </section>
        <section data-testid="artifact-comments">
          <h3>Comments on this version</h3>
          <p v-if="!comments.length" class="muted">No comments on {{ shortId(currentId) }}.</p>
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
            <input v-model="commentForm.author" aria-label="Comment author" placeholder="Your name" />
            <textarea v-model="commentForm.body" aria-label="Comment" placeholder="Comment on this version" rows="2" />
            <select v-model="commentForm.path" aria-label="Anchor file">
              <option value="">No file anchor</option>
              <option v-for="file in detail.files" :key="file.path" :value="file.path">{{ file.path }}</option>
            </select>
            <input v-model="commentForm.position" aria-label="Anchor position" placeholder="Position (e.g. line 4, #header)" />
            <input v-model="commentForm.excerpt" aria-label="Anchor excerpt" placeholder="Excerpt" />
            <button type="submit" :disabled="recording">Add comment</button>
          </form>
        </section>
        <section data-testid="artifact-decisions">
          <h3>Decisions on this version</h3>
          <p class="muted">A decision is a record about this version. It does not move any task.</p>
          <article v-for="decision in decisions" :key="decision.recordId" class="artifact-decision" data-testid="artifact-decision">
            <strong>{{ decision.who }}</strong>: {{ decision.what }} <time>{{ decision.createdAt }}</time>
          </article>
          <form class="artifact-form" data-testid="artifact-decision-form" @submit.prevent="submitDecision">
            <input v-model="decisionForm.who" aria-label="Decision by" placeholder="Who" />
            <input v-model="decisionForm.what" aria-label="Decision" placeholder="Decision (e.g. approved)" />
            <button type="submit" :disabled="recording">Record decision</button>
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
.artifact-body { display: flex; flex: 1; min-height: 0; }
.artifact-content { display: flex; flex: 1; flex-direction: column; min-width: 0; }
iframe { flex: 1; min-height: 0; width: 100%; border: 0; background: white; }
.artifact-side { width: 320px; flex-shrink: 0; overflow-y: auto; border-left: 1px solid var(--kn-border-default); padding: 0 12px 12px; color: var(--kn-text-secondary); }
.artifact-side h3 { font-size: 12px; margin: 12px 0 6px; color: var(--kn-text-primary); }
.artifact-side p { margin: 4px 0; }
dl { display: grid; grid-template-columns: auto 1fr; gap: 2px 8px; margin: 0; }
dt { color: var(--kn-text-muted); }
dd { margin: 0; overflow-wrap: anywhere; }
.artifact-files { list-style: none; margin: 0; padding: 0; }
.artifact-files span { color: var(--kn-text-muted); }
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
