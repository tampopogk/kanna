<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { DesignThread } from "@kanna/design-editor";

/**
 * Feedback → agent (docs/specs/app-design.md §4), as in the design
 * prototype: the document's threads and /agent messages in the order they
 * were created, numbered once, newest at the bottom and scrolled to (Owner).
 * Resolved threads wait behind a toggle at the top; each can be resolved and
 * reopened. A card opens its thread in the document, where replies go.
 */
const props = defineProps<{
  threads: DesignThread[];
  /** False once the design is handed off: feedback is read-only. */
  canWrite: boolean;
  selectedThreadId?: string | null;
}>();
const emit = defineEmits<{
  open: [threadId: string];
  resolve: [threadId: string, resolved: boolean];
  retry: [deliveryId: string];
}>();
const { t } = useI18n();

const ordered = computed(() => [...props.threads].sort((a, b) => a.number - b.number));
const open = computed(() => ordered.value.filter((thread) => thread.status === "open"));
const resolved = computed(() => ordered.value.filter((thread) => thread.status === "resolved"));
const showResolved = ref(false);
const list = ref<HTMLElement | null>(null);
/** Resolved threads (when shown), an "open" divider, then the open ones. */
const entries = computed(() => [
  ...(showResolved.value ? resolved.value.map((thread) => ({ key: thread.id, thread })) : []),
  ...(showResolved.value && resolved.value.length && open.value.length ? [{ key: "divider", thread: null }] : []),
  ...open.value.map((thread) => ({ key: thread.id, thread })),
]);

// Keep the newest thread in view as feedback arrives.
watch(
  () => [open.value.length, props.threads] as const,
  async () => {
    await nextTick();
    if (list.value) list.value.scrollTop = list.value.scrollHeight;
  },
  { immediate: true, deep: true },
);

function lastPersonComment(thread: DesignThread) {
  const person = thread.comments.filter((comment) => comment.author === "operator");
  return person[person.length - 1] ?? thread.comments[0];
}

function latestAgentReply(thread: DesignThread) {
  const last = lastPersonComment(thread);
  const lastIndex = thread.comments.indexOf(last);
  const replies = thread.comments.slice(lastIndex + 1).filter((comment) => comment.author === "agent");
  return replies[replies.length - 1] ?? null;
}

function uncertainDelivery(thread: DesignThread): string | null {
  const delivery = lastPersonComment(thread)?.delivery;
  return delivery?.state === "uncertain" ? delivery.id : null;
}

function statusLabel(thread: DesignThread): string {
  return t(`design.delivery.${thread.deliveryStatus}`);
}

function failed(thread: DesignThread): boolean {
  return thread.deliveryStatus === "uncertain" || thread.deliveryStatus === "cancelled";
}

function anchorLabel(thread: DesignThread): string {
  if (thread.kind === "message") return t("design.feedback.message");
  const text = thread.anchor?.currentText ?? thread.anchor?.quotedText ?? "…";
  return thread.anchor?.state === "detached" ? `${text} · ${t("design.feedback.detached")}` : text;
}

function replyText(thread: DesignThread): string {
  const body = latestAgentReply(thread)?.body ?? "";
  return body.length > 160 ? `${body.slice(0, 160)}…` : body;
}
</script>

<template>
  <aside class="feed" :aria-label="t('design.feedback.title')" data-testid="design-feedback">
    <h4>
      {{ t("design.feedback.title") }}
      <span v-if="open.length" class="count" data-testid="design-feedback-open-count">
        {{ t("design.feedback.openCount", { count: open.length }) }}
      </span>
    </h4>
    <div ref="list" class="feed-list">
      <button
        v-if="resolved.length"
        type="button"
        class="resolved-toggle"
        :aria-expanded="showResolved"
        data-testid="design-feedback-resolved-toggle"
        @click="showResolved = !showResolved"
      >
        {{ showResolved ? "▾" : "▸" }} {{ t("design.feedback.resolvedCount", { count: resolved.length }) }}
      </button>
      <template v-for="entry in entries" :key="entry.key">
        <div v-if="entry.thread === null" class="feed-divider">{{ t("design.feedback.openDivider") }}</div>
        <template v-for="thread in entry.thread ? [entry.thread] : []" :key="thread.id">
        <div
          role="button"
          tabindex="0"
          class="fb"
          :class="{ resolved: thread.status === 'resolved', selected: thread.id === selectedThreadId }"
          :data-testid="`design-thread-${thread.number}`"
          @click="emit('open', thread.id)"
          @keydown.enter.self="emit('open', thread.id)"
        >
          <span class="on-el">
            <span class="kd-num" :aria-label="t('design.feedback.number', { number: thread.number })">{{ thread.number }}</span>{{ anchorLabel(thread) }}
          </span>
          {{ lastPersonComment(thread)?.body }}
          <small>
            {{ t("design.editor.person") }} ·
            <span :class="{ 'kd-bad': failed(thread) }" :title="lastPersonComment(thread)?.delivery?.detail ?? undefined">{{ statusLabel(thread) }}</span>
          </small>
          <div v-if="latestAgentReply(thread)" class="reply">{{ t("design.feedback.agent") }}: {{ replyText(thread) }}</div>
          <div class="fb-actions" @click.stop>
            <span v-if="thread.status === 'resolved'" class="resolved-tag">{{ t("design.feedback.resolvedTag") }}</span>
            <button
              v-if="uncertainDelivery(thread) && canWrite"
              type="button"
              class="kd-linkish"
              :data-testid="`design-thread-${thread.number}-resend`"
              @click="emit('retry', uncertainDelivery(thread)!)"
            >
              {{ t("design.feedback.resend") }}
            </button>
            <button
              v-if="canWrite"
              type="button"
              class="kd-linkish"
              :data-testid="`design-thread-${thread.number}-resolve`"
              @click="emit('resolve', thread.id, thread.status !== 'resolved')"
            >
              {{ thread.status === "resolved" ? t("design.feedback.reopen") : t("design.feedback.resolve") }}
            </button>
          </div>
        </div>
        </template>
      </template>
      <div v-if="!threads.length" class="empty">{{ t("design.feedback.empty") }}</div>
    </div>
  </aside>
</template>

<style scoped>
.feed {
  width: 240px;
  flex-shrink: 0;
  background: var(--kd-panel);
  border-left: 1px solid var(--kd-line);
  display: flex;
  flex-direction: column;
  min-height: 0;
  color: var(--kd-ink);
}
h4 {
  margin: 0;
  padding: 12px 12px 8px;
  font-size: 11px;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--kd-muted);
}
.count {
  text-transform: none;
  letter-spacing: 0;
  font-size: 10.5px;
  color: var(--kd-warn-ink);
  background: var(--kd-warn-soft);
  border-radius: 10px;
  padding: 1px 8px;
  margin-left: 4px;
}
.feed-list {
  flex: 1;
  overflow: auto;
  padding: 0 12px 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.fb {
  text-align: left;
  background: var(--kd-panel);
  border: 1px solid var(--kd-line-2);
  border-radius: 9px;
  padding: 8px 10px;
  line-height: 1.4;
  cursor: pointer;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.fb:hover,
.fb.selected {
  border-color: var(--kd-accent-line);
}
.fb.resolved {
  opacity: 0.5;
}
.fb small {
  display: block;
  color: var(--kd-muted);
  margin-top: 4px;
  white-space: normal;
}
.fb .reply {
  margin-top: 6px;
  padding-top: 6px;
  border-top: 1px dashed var(--kd-line-2);
  color: var(--kd-reply);
}
.on-el {
  display: block;
  color: var(--kd-muted);
  font-size: 11px;
  margin-bottom: 4px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.fb-actions {
  display: flex;
  justify-content: flex-end;
  align-items: center;
  gap: 8px;
  margin-top: 6px;
  white-space: normal;
}
.resolved-tag {
  font-size: 11px;
  color: var(--kd-ok);
  margin-right: auto;
}
.resolved-toggle {
  align-self: flex-start;
  border: 0;
  background: none;
  padding: 2px 0;
  color: var(--kd-muted);
  font: inherit;
  font-size: 11.5px;
  font-weight: 600;
  cursor: pointer;
}
.resolved-toggle:hover {
  color: var(--kd-ink);
}
.feed-divider {
  font-size: 10.5px;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--kd-muted);
  border-top: 1px solid var(--kd-line-2);
  padding-top: 6px;
}
.empty {
  color: var(--kd-muted);
  padding: 6px 2px;
  line-height: 1.45;
}
</style>
