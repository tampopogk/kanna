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
 * A design position's HTML mockup, as the agent published it to the
 * repository's artifact store (docs/specs/app-design.md §5). It renders the
 * way ArtifactViewer renders shared HTML, and for the same reasons: the
 * frame holds the store listener's script-free shell, which frames the
 * mockup sandboxed on another origin with no network, so the mockup cannot
 * reach this window or Kanna's API.
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
  selectedPin: number | null;
}>();
const emit = defineEmits<{
  pin: [pin: MockupPinDescriptor, frame: DOMRect];
  select: [number: number];
}>();
const { t } = useI18n();

/** See ArtifactViewer: the shell keeps the listener's origin; the content gets only scripts. */
const FRAME_SANDBOX = "allow-scripts allow-same-origin";

const frame = ref<HTMLIFrameElement | null>(null);
const frameSrc = ref("");
const error = ref("");
let held: { repoId: string; artifactId: string } | null = null;
let generation = 0;
/** Reveal the selected pin once, when it was chosen here rather than in the page. */
let reveal = false;

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

function sendPins() {
  const target = mockupWindow();
  if (!target) return;
  // The mockup's origin is opaque, so no narrower target is possible; what
  // is sent is only its own elements' selectors and numbers.
  target.postMessage(
    {
      kind: MOCKUP_MESSAGE_KIND,
      type: "pins",
      pins: props.pins.map((pin) => ({ number: pin.number, page: pin.page, selector: pin.selector })),
      selected: props.selectedPin,
      reveal,
    },
    "*",
  );
  reveal = false;
}

function onMessage(event: MessageEvent) {
  const source = mockupWindow();
  if (!source || event.source !== source) return;
  const message = readMockupMessage(event.data);
  if (!message) return;
  if (message.type === "ready") sendPins();
  else if (message.type === "pin" && frame.value) emit("pin", message.pin, frame.value.getBoundingClientRect());
  else if (message.type === "select") emit("select", message.number);
}

watch(() => [props.repoId, props.artifactId, props.entrypoint] as const, () => void load(), { immediate: true });
watch(
  () => props.pins,
  () => sendPins(),
  { deep: true },
);
watch(
  () => props.selectedPin,
  () => {
    reveal = props.selectedPin !== null;
    sendPins();
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
  <div class="mockup" data-testid="design-mockup">
    <p v-if="error" class="mockup-message error" role="alert">{{ error }}</p>
    <p v-else-if="!frameSrc" class="mockup-message">{{ t("design.mockupOpening") }}</p>
    <iframe
      v-else
      ref="frame"
      :key="frameSrc"
      data-testid="design-mockup-frame"
      :src="frameSrc"
      :title="title"
      :sandbox="FRAME_SANDBOX"
      allow=""
      referrerpolicy="no-referrer"
    />
    <p v-if="frameSrc && !error" class="mockup-hint">{{ t("design.mockupHint") }}</p>
  </div>
</template>

<style scoped>
.mockup {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
iframe {
  flex: 1;
  min-height: 0;
  width: 100%;
  border: 0;
  background: white;
}
.mockup-message {
  margin: 0;
  padding: 24px;
  font-size: 13px;
  color: var(--kn-text-secondary);
}
.mockup-message.error {
  color: var(--kn-danger);
}
.mockup-hint {
  margin: 0;
  padding: 3px 12px;
  font-size: 11px;
  color: var(--kn-text-muted);
  border-top: 1px solid var(--kn-border-default);
}
</style>
