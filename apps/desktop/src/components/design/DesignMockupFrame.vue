<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { acquireArtifactPreview, artifactPinFrameUrl, releaseArtifactPreview } from "../../utils/artifactPreview";
import {
  MOCKUP_MESSAGE_KIND,
  readMockupMessage,
  type MockupPinDescriptor,
  type MockupPinMarker,
} from "./mockupPins";

/**
 * A design position's HTML mockup (docs/specs/app-design.md §5), as the agent
 * published it to the repository's artifact store. It renders the way
 * ArtifactViewer renders shared HTML, and for the same reasons: the frame
 * holds the store listener's script-free shell, which frames the mockup
 * sandboxed on another origin with no network, so the mockup cannot reach
 * this window or Kanna's API.
 *
 * It is framed under the listener's pin path, whose pin script makes every
 * element pinnable (a click pins it; Alt/Option-click uses the mockup). The
 * script can only post messages: one is accepted only from the mockup frame
 * inside this frame's shell, read as a bounded description, and answered
 * with the pins to draw.
 */
const props = defineProps<{
  repoId: string;
  artifactId: string;
  entrypoint: string;
  title: string;
  pins: MockupPinMarker[];
  /** A thread to scroll the page to, once. */
  reveal: string | null;
}>();
const emit = defineEmits<{
  pick: [pin: MockupPinDescriptor];
  focus: [threadId: string];
  detached: [threadIds: string[]];
}>();
const { t } = useI18n();

/** See ArtifactViewer: the shell keeps the listener's origin; the content gets only scripts. */
const FRAME_SANDBOX = "allow-scripts allow-same-origin";

const frame = ref<HTMLIFrameElement | null>(null);
const frameSrc = ref("");
const error = ref("");
let held: { repoId: string; artifactId: string } | null = null;
let generation = 0;

function release() {
  if (!held) return;
  const { repoId, artifactId } = held;
  held = null;
  void releaseArtifactPreview(repoId, artifactId);
}

async function load() {
  const request = ++generation;
  const { repoId, artifactId, entrypoint } = props;
  release();
  frameSrc.value = "";
  error.value = "";
  try {
    const opened = await acquireArtifactPreview(repoId, artifactId);
    if (request !== generation) {
      void releaseArtifactPreview(repoId, artifactId);
      return;
    }
    held = { repoId, artifactId };
    frameSrc.value = artifactPinFrameUrl(opened.url, entrypoint);
  } catch (cause) {
    if (request === generation) error.value = cause instanceof Error ? cause.message : String(cause);
  }
}

/** The mockup's window: the one frame inside this frame's shell. */
function mockupWindow(): Window | null {
  const shell = frame.value?.contentWindow;
  if (!shell) return null;
  try {
    return shell.frames.length > 0 ? shell.frames[0] : null;
  } catch {
    return null;
  }
}

function sendPins(reveal: string | null = null) {
  const target = mockupWindow();
  if (!target) return;
  // The mockup's origin is opaque, so no narrower target is possible; what
  // is sent is only its own elements' selectors, text and numbers.
  target.postMessage(
    {
      kind: MOCKUP_MESSAGE_KIND,
      type: "pins",
      pins: props.pins.map(({ id, n, page, selector, excerpt }) => ({ id, n, page, selector, excerpt })),
      reveal,
    },
    "*",
  );
}

function onMessage(event: MessageEvent) {
  const source = mockupWindow();
  if (!source || event.source !== source) return;
  const message = readMockupMessage(event.data);
  if (!message) return;
  if (message.type === "ready") sendPins();
  else if (message.type === "pick") emit("pick", message.pin);
  else if (message.type === "focus") emit("focus", message.id);
  else if (message.type === "detached") emit("detached", message.ids);
}

watch(() => [props.repoId, props.artifactId, props.entrypoint] as const, () => void load(), { immediate: true });
watch(() => props.pins, () => sendPins(), { deep: true });
watch(
  () => props.reveal,
  (reveal) => {
    if (reveal) sendPins(reveal);
  },
);
onMounted(() => window.addEventListener("message", onMessage));
onBeforeUnmount(() => {
  window.removeEventListener("message", onMessage);
  generation += 1;
  release();
});
</script>

<template>
  <p v-if="error" class="mockup-message error" role="alert">{{ error }}</p>
  <p v-else-if="!frameSrc" class="mockup-message">{{ t("design.mockupOpening") }}</p>
  <iframe
    v-else
    ref="frame"
    :key="frameSrc"
    class="mockup-frame"
    data-testid="design-mockup-frame"
    :src="frameSrc"
    :title="title"
    :sandbox="FRAME_SANDBOX"
    allow=""
    referrerpolicy="no-referrer"
  />
</template>

<style scoped>
.mockup-frame {
  width: 100%;
  height: 100%;
  border: 1px solid var(--kd-line);
  border-radius: 10px;
  background: #fff;
  display: block;
}
.mockup-message {
  margin: 0;
  padding: 40px;
  font-family: sans-serif;
  color: #888;
}
.mockup-message.error {
  color: var(--kd-bad);
}
</style>
