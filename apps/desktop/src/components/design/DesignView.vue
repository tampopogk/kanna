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
  type DesignThread,
  type DesignView,
  type MountedDesignEditor,
} from "@kanna/design-editor";
import "@kanna/design-editor/styles";
import {
  confirmDesignApproval,
  desktopDesignTransport,
  setDesignPosition,
  prepareDesignCandidate,
  reopenDesign,
  retryDesignHandoff,
} from "../../services/designClient";
import { useThemeRuntime } from "../../theme/runtime";
import DesignFeed from "./DesignFeed.vue";
import DesignLadder from "./DesignLadder.vue";
import DesignMockupRoom from "./DesignMockupRoom.vue";
import DesignSignoffBar from "./DesignSignoffBar.vue";
import { designToast, sayDesign } from "./designToast";
import type { MockupPinDescriptor } from "./mockupPins";
import "./design-surface.css";

/**
 * An App Design task's design surface (docs/specs/app-design.md §4), laid out
 * as the approved design prototype: beside the task's real agent terminal
 * (Owner: the terminal itself, not an imitation), the current position's
 * artifact with the sign-off bar under it, and one comment panel. A mockup
 * position shows its mockup and its own pin comments; the document position
 * shows the live document and the "Feedback → agent" feed. The position
 * ladder is in the task header.
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
const selectedThreadId = ref<string | null>(null);
const editorHost = ref<HTMLElement | null>(null);
let editor: MountedDesignEditor | null = null;
let unsubscribe: (() => void) | null = null;

const theme = computed<"light" | "dark">(() => (effectiveAppTheme.value === "light" ? "light" : "dark"));
const editable = computed(
  () => !!view.value?.inDesignStage && view.value.status === "designing" && status.value !== "incompatible",
);
const position = computed(() => view.value?.positions.find((candidate) => candidate.name === view.value?.position) ?? null);
/** What the position shows: its mockup room, or the live document. */
const showsMockup = computed(() => position.value?.artifact === "mockup");
const pins = computed<DesignThread[]>(() =>
  (view.value?.threads ?? []).filter((thread) => thread.anchor?.element?.position === view.value?.position),
);
const documentThreads = computed<DesignThread[]>(() => (view.value?.threads ?? []).filter((thread) => !thread.anchor?.element));
const syncProblem = computed(() => (status.value === "offline" ? t("design.status.offline") : null));

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

function mountEditor() {
  if (!editorHost.value || !session.value || editor) return;
  editor = mountDesignEditor(editorHost.value, {
    session: session.value,
    theme: theme.value,
    editable: editable.value,
    labels: labels.value,
    onNotice: (text, kind) => sayDesign(text, kind),
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
onBeforeUnmount(() => close());
watch(() => props.taskId, (taskId) => open(taskId));
watch([theme, editable, labels, selectedThreadId], () =>
  editor?.update({
    theme: theme.value,
    editable: editable.value,
    labels: labels.value,
    selectedThreadId: selectedThreadId.value,
  }),
);

async function act<T>(action: () => Promise<T>): Promise<T | null> {
  try {
    return await action();
  } catch (error) {
    sayDesign(error instanceof Error ? error.message : String(error), "error");
    return null;
  }
}

async function createPin(pin: MockupPinDescriptor, body: string): Promise<string | null> {
  const mockup = position.value?.mockup;
  if (!session.value || !view.value || !mockup) return null;
  const { rect: _rect, ...element } = pin;
  const threadId = newId("th");
  const created = await act(() =>
    session.value!.createThread({
      threadId,
      commentId: newId("cm"),
      kind: "comment",
      body,
      anchor: { element: { ...element, position: view.value!.position, artifactId: mockup.artifactId } },
    }),
  );
  if (!created) return null;
  sayDesign(t("design.room.pinned"));
  return threadId;
}

/** A position picked in the ladder: the design moves there, inside the one stage. */
async function pickPosition(name: string) {
  const current = view.value;
  if (!current) return;
  const label = current.positions.find((candidate) => candidate.name === name)?.label ?? name;
  if (name === current.position) {
    sayDesign(t("design.youAreHere"));
    return;
  }
  const moved = await act(() => setDesignPosition(props.taskId, name));
  if (moved === null) return;
  await session.value?.refreshView();
  sayDesign(t("design.nowIn", { position: label }));
}

const threadActions = {
  reply: (threadId: string, body: string) => act(() => session.value!.reply(threadId, { commentId: newId("cm"), body })),
  resolve: (threadId: string, resolved: boolean) => act(() => session.value!.resolve(threadId, resolved)),
  retry: (deliveryId: string) => act(() => session.value!.retryDelivery(deliveryId)),
};

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
  <section class="kd design-surface" :class="`theme-${theme}`" data-testid="design-view">
    <DesignLadder v-if="view" :design="view" :set-app-theme="setAppTheme" @pick="pickPosition" />
    <p v-if="status === 'incompatible'" class="banner" role="alert">{{ t("design.incompatible") }}</p>
    <div class="workspace">
      <section class="surface-col">
        <DesignMockupRoom
          v-if="showsMockup"
          :mockup="position?.mockup ?? null"
          :position-label="position?.label ?? ''"
          :threads="pins"
          :can-write="editable"
          :create-pin="createPin"
          :reply="threadActions.reply"
          :resolve="threadActions.resolve"
          :retry="threadActions.retry"
        />
        <!-- Kept mounted on a mockup position: the live document keeps syncing. -->
        <div v-show="!showsMockup" class="surface">
          <div class="page">
            <div ref="editorHost" class="editor-host" data-testid="design-editor-host" />
          </div>
        </div>
        <DesignSignoffBar
          v-if="view"
          :approval="view.approval"
          :status="view.status"
          :in-design-stage="view.inDesignStage"
          :next-stage="view.nextStage"
          :scratch-repository="view.scratchRepository"
          :sync-problem="syncProblem"
          :prepare="approvalActions.prepare"
          :confirm="approvalActions.confirm"
          :reopen="approvalActions.reopen"
          :retry="approvalActions.retry"
        />
      </section>
      <DesignFeed
        v-if="!showsMockup"
        :threads="documentThreads"
        :can-write="editable"
        :selected-thread-id="selectedThreadId"
        @open="(id) => (selectedThreadId = id)"
        @resolve="threadActions.resolve"
        @retry="threadActions.retry"
      />
    </div>
    <div class="toast" :class="{ show: !!designToast, error: designToast?.kind === 'error' }" role="status">
      {{ designToast?.text ?? "" }}
    </div>
  </section>
</template>

<style scoped>
.design-surface {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--kd-bg);
  color: var(--kd-ink);
  font-family: Inter, system-ui, sans-serif;
}
.banner {
  margin: 0;
  padding: 6px 12px;
  font-size: 12px;
  background: var(--kn-danger-bg);
  color: var(--kn-danger);
}
.workspace {
  flex: 1;
  display: flex;
  min-height: 0;
}
.surface-col {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  background: var(--kd-panel-2);
}
.surface {
  flex: 1;
  overflow: auto;
  padding: 18px;
  position: relative;
}
.page {
  max-width: 720px;
  margin: 0 auto;
  background: var(--kd-page);
  border-radius: 10px;
  box-shadow: var(--kd-shadow);
  border: 1px solid var(--kd-line-2);
  padding: 28px 0;
  min-height: 70vh;
}
.editor-host {
  min-height: 100%;
}
.toast {
  position: absolute;
  bottom: 60px;
  left: 50%;
  transform: translateX(-50%);
  background: var(--kd-ink);
  color: var(--kd-bg);
  padding: 8px 14px;
  border-radius: 8px;
  font-size: 12.5px;
  opacity: 0;
  transition: opacity 0.2s;
  pointer-events: none;
  z-index: 30;
  max-width: calc(100% - 32px);
}
.toast.show {
  opacity: 1;
}
.toast.error {
  background: var(--kd-bad);
  color: #fff;
}
@media (max-width: 720px) {
  .workspace {
    flex-direction: column;
  }
}
</style>
