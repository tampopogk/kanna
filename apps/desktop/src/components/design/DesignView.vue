<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import {
  DESIGN_SCHEMA_VERSION,
  DesignSession,
  mountDesignEditor,
  newId,
  type DesignCandidate,
  type DesignEditorLabels,
  type DesignSessionStatus,
  type DesignView,
  type MountedDesignEditor,
} from "@kanna/design-editor";
import "@kanna/design-editor/styles";
import {
  confirmDesignApproval,
  desktopDesignTransport,
  prepareDesignCandidate,
  reopenDesign,
  retryDesignHandoff,
  setDesignPosition,
} from "../../services/designClient";
import { useThemeRuntime } from "../../theme/runtime";
import DesignFeedbackPanel from "./DesignFeedbackPanel.vue";
import DesignApprovalBar from "./DesignApprovalBar.vue";
import DesignMockupFrame from "./DesignMockupFrame.vue";

/**
 * An App Design task's design surface (docs/specs/app-design.md §4), opened
 * as a view beside the task's agent terminal — the real session, which this
 * view never replaces. It shows the current position's artifact — its HTML
 * mockup once the agent has published one, else the live document, which
 * stays one click away as the position's notes — the one feedback panel,
 * and the hand-off bar.
 */
const props = defineProps<{
  taskId: string;
  visible: boolean;
  /** Persist the app-wide theme choice (the same setting Preferences uses). */
  setAppTheme?: (theme: "light" | "dark") => void;
}>();
const { t } = useI18n();
const { effectiveAppTheme } = useThemeRuntime();

const session = shallowRef<DesignSession | null>(null);
const view = shallowRef<DesignView | null>(null);
const status = ref<DesignSessionStatus>("connecting");
const statusError = ref<string | null>(null);
const selectedThreadId = ref<string | null>(null);
const notice = ref<{ text: string; kind: "info" | "error" } | null>(null);
const editorHost = ref<HTMLElement | null>(null);
let editor: MountedDesignEditor | null = null;
let unsubscribe: (() => void) | null = null;
let noticeTimer: ReturnType<typeof setTimeout> | null = null;

const theme = computed<"light" | "dark">(() => (effectiveAppTheme.value === "light" ? "light" : "dark"));
const editable = computed(
  () => !!view.value?.inDesignStage && view.value.status === "designing" && status.value !== "incompatible",
);
const currentPosition = computed(() => view.value?.positions.find((position) => position.name === view.value?.position));
const mockup = computed(() => currentPosition.value?.mockup ?? null);
/** The person chose the notes over this position's mockup. */
const showingNotes = ref(false);
const showMockup = computed(() => !!mockup.value && !showingNotes.value);
// A newly published page, or another position, shows its mockup again.
watch(
  () => `${view.value?.position ?? ""}\u0000${mockup.value?.artifactId ?? ""}`,
  () => (showingNotes.value = false),
);

const factoryStages = computed(() => {
  const chain = view.value?.stageChain ?? [];
  const index = chain.indexOf(view.value?.stage ?? "");
  return index >= 0 ? chain.slice(index + 1) : [];
});

const labels = computed<DesignEditorLabels>(() => ({
  comment: t("design.editor.comment"),
  commentShortcut: "⌘↵",
  agentItemTitle: t("design.editor.agentItemTitle"),
  agentItemSubtext: t("design.editor.agentItemSubtext"),
  agentGroup: t("design.editor.agentGroup"),
  person: t("design.editor.person"),
  agent: t("design.feedback.agent"),
  messageSent: t("design.editor.messageSent"),
  messageFailed: t("design.editor.messageFailed"),
  commentFailed: t("design.editor.commentFailed"),
}));

function showNotice(text: string, kind: "info" | "error" = "info") {
  notice.value = { text, kind };
  if (noticeTimer) clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => (notice.value = null), 2600);
}

function mountEditor() {
  if (!editorHost.value || !session.value || editor) return;
  editor = mountDesignEditor(editorHost.value, {
    session: session.value,
    theme: theme.value,
    editable: editable.value,
    labels: labels.value,
    onNotice: showNotice,
    selectedThreadId: selectedThreadId.value,
  });
}

function open(taskId: string) {
  close();
  const next = new DesignSession(desktopDesignTransport(taskId), { schemaVersion: DESIGN_SCHEMA_VERSION });
  session.value = next;
  unsubscribe = next.subscribe(() => {
    view.value = next.view;
    status.value = next.status;
    statusError.value = next.lastError;
  });
  void next
    .start()
    .catch(() => undefined)
    .finally(() => mountEditor());
}

function close() {
  editor?.destroy();
  editor = null;
  unsubscribe?.();
  unsubscribe = null;
  session.value?.close();
  session.value = null;
  view.value = null;
  status.value = "connecting";
}

onMounted(() => open(props.taskId));
onBeforeUnmount(() => {
  close();
  if (noticeTimer) clearTimeout(noticeTimer);
});
watch(() => props.taskId, (taskId) => open(taskId));
watch([theme, editable, labels, selectedThreadId], () =>
  editor?.update({
    theme: theme.value,
    editable: editable.value,
    labels: labels.value,
    selectedThreadId: selectedThreadId.value,
  }),
);

async function choosePosition(position: string) {
  if (!view.value || position === view.value.position || !editable.value) return;
  try {
    await setDesignPosition(props.taskId, position);
    await session.value?.refreshView();
  } catch (error) {
    showNotice(error instanceof Error ? error.message : String(error), "error");
  }
}

function toggleTheme() {
  props.setAppTheme?.(theme.value === "dark" ? "light" : "dark");
}

async function feedbackAction(action: () => Promise<unknown>) {
  try {
    await action();
  } catch (error) {
    showNotice(error instanceof Error ? error.message : String(error), "error");
  }
}

const statusText = computed(() => {
  switch (status.value) {
    case "connecting":
      return t("design.status.connecting");
    case "saving":
      return t("design.status.saving");
    case "offline":
      return t("design.status.offline");
    case "incompatible":
      return t("design.status.incompatible");
    case "closed":
      return "";
    default:
      return t("design.status.saved");
  }
});

const approvalActions = {
  prepare: () => prepareDesignCandidate(props.taskId).then(async (candidate) => {
    await session.value?.refreshView();
    return candidate;
  }),
  confirm: (candidate: DesignCandidate) =>
    confirmDesignApproval(props.taskId, candidate.approval.id, candidate.confirmationToken).finally(() =>
      session.value?.refreshView(),
    ),
  reopen: () => reopenDesign(props.taskId).finally(() => session.value?.refreshView()),
  retry: () => retryDesignHandoff(props.taskId).finally(() => session.value?.refreshView()),
};
</script>

<template>
  <section class="design-view" :class="`theme-${theme}`" data-testid="design-view">
    <header class="design-bar">
      <span class="workflow">{{ t("design.workflowName") }}</span>
      <nav class="positions" :aria-label="t('design.positionsLabel')">
        <template v-for="(position, index) in view?.positions ?? []" :key="position.name">
          <span v-if="index > 0" class="arrow" :title="t('design.freeMove')">⇄</span>
          <button
            type="button"
            class="position"
            :class="{ now: position.name === view?.position, done: !view?.inDesignStage }"
            :aria-current="position.name === view?.position ? 'step' : undefined"
            :disabled="!editable"
            :data-testid="`design-position-${position.name}`"
            @click="choosePosition(position.name)"
          >
            {{ position.label }}
          </button>
        </template>
        <template v-if="factoryStages.length">
          <span class="arrow" :title="t('design.handoffArrow')">⇢</span>
          <template v-for="(stage, index) in factoryStages" :key="stage">
            <span v-if="index > 0" class="arrow">→</span>
            <span class="factory-stage" :class="{ now: stage === view?.currentStage }">{{ stage }}</span>
          </template>
        </template>
      </nav>
      <span class="spacer" />
      <span class="sync-status" :class="`status-${status}`" :title="statusError ?? undefined" data-testid="design-sync-status">
        {{ statusText }}
      </span>
      <button
        v-if="setAppTheme"
        type="button"
        class="theme-toggle"
        :aria-label="theme === 'dark' ? t('design.lightTheme') : t('design.darkTheme')"
        :title="theme === 'dark' ? t('design.lightTheme') : t('design.darkTheme')"
        @click="toggleTheme"
      >
        {{ theme === "dark" ? "☀" : "☾" }}
      </button>
    </header>
    <div class="design-body">
      <div class="artifact-column">
        <p v-if="status === 'incompatible'" class="banner error" role="alert">{{ t("design.incompatible") }}</p>
        <p v-else-if="view && !view.inDesignStage" class="banner">{{ t("design.handedOffBanner") }}</p>
        <div v-if="mockup" class="artifact-bar">
          <span class="artifact-title">{{ t("design.mockupTitle", { position: currentPosition?.label ?? "" }) }}</span>
          <span class="spacer" />
          <button
            type="button"
            class="artifact-toggle"
            data-testid="design-artifact-toggle"
            :title="showMockup ? t('design.showNotesTitle') : t('design.showMockupTitle')"
            :aria-pressed="!showMockup"
            @click="showingNotes = !showingNotes"
          >
            {{ showMockup ? t("design.showNotes") : t("design.showMockup") }}
          </button>
        </div>
        <DesignMockupFrame
          v-if="showMockup && mockup"
          :repo-id="mockup.repoId"
          :artifact-id="mockup.artifactId"
          :entrypoint="mockup.entrypoint"
          :title="t('design.mockupTitle', { position: currentPosition?.label ?? '' })"
        />
        <!-- Kept mounted under the mockup: the live document keeps syncing. -->
        <div v-show="!showMockup" ref="editorHost" class="editor-host" data-testid="design-editor-host" />
        <DesignApprovalBar
          v-if="view"
          :approval="view.approval"
          :status="view.status"
          :in-design-stage="view.inDesignStage"
          :next-stage="view.nextStage"
          :prepare="approvalActions.prepare"
          :confirm="approvalActions.confirm"
          :reopen="approvalActions.reopen"
          :retry="approvalActions.retry"
        />
      </div>
      <DesignFeedbackPanel
        class="feedback-column"
        :threads="view?.threads ?? []"
        :can-write="editable"
        :selected-thread-id="selectedThreadId"
        @select="(id) => (selectedThreadId = id)"
        @resolve="(id, resolved) => feedbackAction(() => session!.resolve(id, resolved))"
        @reply="(id, body) => feedbackAction(() => session!.reply(id, { commentId: newId('cm'), body }))"
        @retry="(id) => feedbackAction(() => session!.retryDelivery(id))"
      />
    </div>
    <div v-if="notice" class="notice" :class="notice.kind" role="status">{{ notice.text }}</div>
  </section>
</template>

<style scoped>
.design-view {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--kn-bg-app);
  color: var(--kn-text-primary);
}
.design-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 12px;
  border-bottom: 1px solid var(--kn-border-default);
  font-size: 12px;
}
.workflow {
  font-weight: 600;
  color: var(--kn-text-secondary);
}
.positions {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}
.position {
  border: 1px solid var(--kn-border-strong);
  background: none;
  color: var(--kn-text-secondary);
  border-radius: 12px;
  padding: 2px 10px;
  font: inherit;
  cursor: pointer;
}
.position.now {
  border-color: var(--kn-accent);
  color: var(--kn-text-primary);
  background: var(--kn-bg-accent-subtle);
}
.position:disabled {
  cursor: default;
}
.position.done {
  opacity: 0.6;
}
.arrow {
  color: var(--kn-text-muted);
}
.factory-stage {
  color: var(--kn-text-muted);
}
.factory-stage.now {
  color: var(--kn-text-primary);
  font-weight: 600;
}
.spacer {
  flex: 1;
}
.sync-status {
  color: var(--kn-text-muted);
}
.sync-status.status-offline,
.sync-status.status-incompatible {
  color: var(--kn-danger);
}
.theme-toggle {
  border: 0;
  background: none;
  color: var(--kn-text-secondary);
  cursor: pointer;
  font-size: 14px;
}
.design-body {
  flex: 1;
  min-height: 0;
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(240px, 320px);
}
.artifact-column {
  display: flex;
  flex-direction: column;
  min-height: 0;
  min-width: 0;
}
.editor-host {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 24px 12px;
}
.artifact-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 12px;
  border-bottom: 1px solid var(--kn-border-default);
  font-size: 12px;
  color: var(--kn-text-secondary);
}
.artifact-toggle {
  border: 1px solid var(--kn-border-strong);
  background: none;
  color: var(--kn-text-secondary);
  border-radius: 10px;
  padding: 1px 10px;
  font: inherit;
  cursor: pointer;
}
.banner {
  margin: 0;
  padding: 6px 12px;
  font-size: 12px;
  background: var(--kn-bg-accent-subtle);
  color: var(--kn-text-secondary);
}
.banner.error {
  background: var(--kn-danger-bg);
  color: var(--kn-danger);
}
.feedback-column {
  min-height: 0;
}
.notice {
  position: absolute;
  bottom: 56px;
  left: 50%;
  transform: translateX(-50%);
  padding: 6px 12px;
  border-radius: 6px;
  background: var(--kn-bg-panel-raised);
  border: 1px solid var(--kn-border-strong);
  font-size: 12px;
}
.notice.error {
  color: var(--kn-danger);
}
@media (max-width: 720px) {
  .design-body {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: minmax(0, 1fr) minmax(160px, 40%);
  }
}
</style>
