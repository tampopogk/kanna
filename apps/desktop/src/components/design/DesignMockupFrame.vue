<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { acquireArtifactPreview, artifactFrameUrl, releaseArtifactPreview } from "../../utils/artifactPreview";

/**
 * A design position's HTML mockup, as the agent published it to the
 * repository's artifact store (docs/specs/app-design.md §5). It renders the
 * way ArtifactViewer renders shared HTML, and for the same reasons: the
 * frame holds the store listener's script-free shell, which frames the
 * mockup sandboxed on another origin with no network, so the mockup cannot
 * reach this window or Kanna's API.
 */
const props = defineProps<{ repoId: string; artifactId: string; entrypoint: string; title: string }>();
const { t } = useI18n();

/** See ArtifactViewer: the shell keeps the listener's origin; the content gets only scripts. */
const FRAME_SANDBOX = "allow-scripts allow-same-origin";

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
    frameSrc.value = artifactFrameUrl(opened.url, entrypoint);
  } catch (cause) {
    if (request === generation) error.value = cause instanceof Error ? cause.message : String(cause);
  }
}

watch(() => [props.repoId, props.artifactId, props.entrypoint] as const, () => void load(), { immediate: true });
onBeforeUnmount(() => {
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
      :key="frameSrc"
      data-testid="design-mockup-frame"
      :src="frameSrc"
      :title="title"
      :sandbox="FRAME_SANDBOX"
      allow=""
      referrerpolicy="no-referrer"
    />
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
</style>
