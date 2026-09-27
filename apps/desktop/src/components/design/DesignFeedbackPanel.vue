<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { DesignThread } from "@kanna/design-editor";
import { elementLabel } from "./mockupPins";

/**
 * Feedback → agent (docs/specs/app-design.md §4): one panel of threads in
 * the order they were created, numbered once, newest at the bottom and
 * scrolled to. Resolved threads wait behind a toggle at the top; each can be
 * resolved and reopened. Delivery ("queued", "delivered", "agent replied",
 * "not delivered") is shown apart from resolution.
 */
const props = defineProps<{
  threads: DesignThread[];
  /** False once the design is handed off: feedback is read-only. */
  canWrite: boolean;
  selectedThreadId?: string | null;
}>();
const emit = defineEmits<{
  select: [threadId: string];
  resolve: [threadId: string, resolved: boolean];
  reply: [threadId: string, body: string];
  retry: [deliveryId: string];
}>();
const { t } = useI18n();

const ordered = computed(() => [...props.threads].sort((a, b) => a.number - b.number));
const open = computed(() => ordered.value.filter((thread) => thread.status === "open"));
const resolved = computed(() => ordered.value.filter((thread) => thread.status === "resolved"));
const showResolved = ref(false);
const replyingTo = ref<string | null>(null);
const replyText = ref("");
const list = ref<HTMLElement | null>(null);

// Keep the newest thread in view as feedback arrives.
watch(
  () => open.value.length,
  async () => {
    await nextTick();
    if (list.value) list.value.scrollTop = list.value.scrollHeight;
  },
  { immediate: true },
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

function statusDetail(thread: DesignThread): string | null {
  return lastPersonComment(thread)?.delivery?.detail ?? null;
}

function anchorLabel(thread: DesignThread): string {
  if (thread.kind === "message") return t("design.feedback.message");
  const element = thread.anchor?.element;
  if (element) {
    const shown = element.text.length > 40 ? `${element.text.slice(0, 40)}…` : element.text;
    return shown ? `${elementLabel(element)} “${shown}”` : elementLabel(element);
  }
  const quote = thread.anchor?.quotedText ?? "";
  return `“${quote.length > 60 ? `${quote.slice(0, 60)}…` : quote}”`;
}

function sendReply(thread: DesignThread) {
  const body = replyText.value.trim();
  if (!body) return;
  emit("reply", thread.id, body);
  replyText.value = "";
  replyingTo.value = null;
}
</script>

<template>
  <aside class="design-feedback" :aria-label="t('design.feedback.title')" data-testid="design-feedback">
    <header class="feedback-header">
      <h3>{{ t("design.feedback.title") }}</h3>
      <span v-if="open.length" class="count" data-testid="design-feedback-open-count">
        {{ t("design.feedback.openCount", { count: open.length }) }}
      </span>
    </header>
    <div ref="list" class="feedback-list">
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
      <template v-for="thread in [...(showResolved ? resolved : []), ...open]" :key="thread.id">
        <article
          class="thread"
          :class="{ resolved: thread.status === 'resolved', selected: thread.id === selectedThreadId }"
          :data-testid="`design-thread-${thread.number}`"
          tabindex="0"
          @click="emit('select', thread.id)"
          @keydown.enter.self="emit('select', thread.id)"
        >
          <div class="thread-anchor">
            <span class="number" :aria-label="t('design.feedback.number', { number: thread.number })">{{ thread.number }}</span>
            <span class="anchor" :class="{ detached: thread.anchor?.state === 'detached', pin: !!thread.anchor?.element }">{{ anchorLabel(thread) }}</span>
            <span v-if="thread.anchor?.state === 'detached'" class="detached-note">{{ t("design.feedback.detached") }}</span>
            <span v-else-if="thread.anchor?.state === 'outdated'" class="detached-note">{{ t("design.feedback.outdated") }}</span>
          </div>
          <p class="body">{{ lastPersonComment(thread)?.body }}</p>
          <p class="status" :class="`status-${thread.deliveryStatus}`" :title="statusDetail(thread) ?? undefined">
            {{ statusLabel(thread) }}
          </p>
          <p v-if="latestAgentReply(thread)" class="reply">
            <strong>{{ t("design.feedback.agent") }}:</strong> {{ latestAgentReply(thread)?.body }}
          </p>
          <div class="actions" @click.stop>
            <span v-if="thread.status === 'resolved'" class="resolved-tag">{{ t("design.feedback.resolvedTag") }}</span>
            <button
              v-if="uncertainDelivery(thread) && canWrite"
              type="button"
              class="linkish"
              :data-testid="`design-thread-${thread.number}-retry`"
              @click="emit('retry', uncertainDelivery(thread)!)"
            >
              {{ t("design.feedback.resend") }}
            </button>
            <button
              v-if="canWrite && thread.status === 'open'"
              type="button"
              class="linkish"
              @click="replyingTo = replyingTo === thread.id ? null : thread.id"
            >
              {{ t("design.feedback.reply") }}
            </button>
            <button
              type="button"
              class="linkish"
              :data-testid="`design-thread-${thread.number}-resolve`"
              @click="emit('resolve', thread.id, thread.status !== 'resolved')"
            >
              {{ thread.status === "resolved" ? t("design.feedback.reopen") : t("design.feedback.resolve") }}
            </button>
          </div>
          <form v-if="replyingTo === thread.id" class="reply-form" @click.stop @submit.prevent="sendReply(thread)">
            <input
              v-model="replyText"
              :aria-label="t('design.feedback.replyLabel', { number: thread.number })"
              :placeholder="t('design.feedback.replyPlaceholder')"
              @keydown.esc="replyingTo = null"
            />
            <button type="submit" :disabled="!replyText.trim()">{{ t("design.feedback.send") }}</button>
          </form>
        </article>
      </template>
      <p v-if="!ordered.length" class="empty">{{ t("design.feedback.empty") }}</p>
    </div>
  </aside>
</template>

<style scoped>
.design-feedback {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  border-left: 1px solid var(--kn-border-default);
  background: var(--kn-bg-panel);
  color: var(--kn-text-primary);
}
.feedback-header {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 10px 12px;
  border-bottom: 1px solid var(--kn-border-default);
}
.feedback-header h3 {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
}
.count {
  font-size: 12px;
  color: var(--kn-text-muted);
}
.feedback-list {
  flex: 1;
  overflow-y: auto;
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.resolved-toggle {
  align-self: flex-start;
  border: 0;
  background: none;
  color: var(--kn-text-secondary);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
  padding: 2px 4px;
}
.thread {
  border: 1px solid var(--kn-border-default);
  border-radius: 8px;
  padding: 8px 10px;
  background: var(--kn-bg-panel-raised);
  font-size: 13px;
  cursor: pointer;
}
.thread:focus-visible,
.thread.selected {
  outline: 2px solid var(--kn-accent);
  outline-offset: 1px;
}
.thread.resolved {
  opacity: 0.7;
}
.thread-anchor {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--kn-text-secondary);
}
.number {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 18px;
  height: 18px;
  border-radius: 9px;
  background: var(--kn-accent);
  color: var(--kn-text-inverse);
  font-size: 11px;
  font-weight: 600;
}
.anchor.detached {
  text-decoration: line-through;
}
.detached-note {
  font-size: 11px;
  color: var(--kn-text-muted);
}
.body {
  margin: 6px 0 4px;
  white-space: pre-wrap;
}
.status {
  margin: 0;
  font-size: 11px;
  color: var(--kn-text-muted);
}
.status-uncertain,
.status-cancelled {
  color: var(--kn-danger);
}
.status-agent_replied,
.status-delivered {
  color: var(--kn-success);
}
.reply {
  margin: 6px 0 0;
  padding: 6px 8px;
  border-left: 2px solid var(--kn-accent);
  background: var(--kn-bg-accent-subtle);
  white-space: pre-wrap;
}
.actions {
  display: flex;
  gap: 10px;
  justify-content: flex-end;
  margin-top: 6px;
  align-items: center;
}
.linkish {
  border: 0;
  background: none;
  color: var(--kn-accent);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
  padding: 0;
}
.resolved-tag {
  font-size: 11px;
  color: var(--kn-success);
  margin-right: auto;
}
.reply-form {
  display: flex;
  gap: 6px;
  margin-top: 6px;
}
.reply-form input {
  flex: 1;
  min-width: 0;
  background: var(--kn-bg-input);
  color: var(--kn-text-primary);
  border: 1px solid var(--kn-border-strong);
  border-radius: 6px;
  padding: 4px 6px;
  font: inherit;
}
.empty {
  color: var(--kn-text-muted);
  font-size: 13px;
  padding: 8px;
}
</style>
