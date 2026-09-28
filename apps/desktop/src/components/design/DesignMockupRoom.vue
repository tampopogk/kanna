<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { DesignMockup, DesignThread } from "@kanna/design-editor";
import DesignMockupFrame from "./DesignMockupFrame.vue";
import { pinLabel, type MockupPinDescriptor, type MockupPinMarker } from "./mockupPins";

/**
 * A mockup position (docs/specs/app-design.md §5), as the design prototype's
 * review room showed it: the mockup on the left, its pin comments on the
 * right. One comment panel at a time (Owner): a mockup brings its own pins,
 * so the document's feed makes way for them. ⌘-click (Ctrl-click where
 * there is no ⌘) on the mockup pins that element; a plain click uses the mockup, so
 * an interactive mockup stays clickable (Owner, 2026-09-27).
 */
const props = defineProps<{
  mockup: DesignMockup | null;
  positionLabel: string;
  /** The pins of this position, in creation order. */
  threads: DesignThread[];
  canWrite: boolean;
  createPin: (pin: MockupPinDescriptor, body: string) => Promise<string | null>;
  reply: (threadId: string, body: string) => Promise<unknown>;
  resolve: (threadId: string, resolved: boolean) => Promise<unknown>;
  retry: (deliveryId: string) => Promise<unknown>;
}>();
const { t } = useI18n();

const focusId = ref<string | null>(null);
const reveal = ref<string | null>(null);
const detached = ref(new Set<string>());
const picked = ref<MockupPinDescriptor | null>(null);
const composerText = ref("");
const posting = ref(false);
const list = ref<HTMLElement | null>(null);
const composerInput = ref<HTMLTextAreaElement | null>(null);

const markers = computed<MockupPinMarker[]>(() =>
  props.threads
    .filter((thread) => thread.status === "open" && thread.anchor?.element)
    .map((thread) => ({
      id: thread.id,
      n: thread.number,
      page: thread.anchor!.element!.page,
      selector: thread.anchor!.element!.selector,
      excerpt: thread.anchor!.element!.excerpt,
    })),
);

function focus(id: string, scroll = true) {
  focusId.value = id;
  if (!scroll) return;
  void nextTick(() =>
    list.value?.querySelector(`[data-thread-id="${id}"]`)?.scrollIntoView({ block: "nearest", behavior: "smooth" }),
  );
}

function showInMockup(id: string) {
  focus(id, false);
  // A new value each time, so the same pin can be revealed twice.
  reveal.value = null;
  void nextTick(() => (reveal.value = id));
}

function onPick(pin: MockupPinDescriptor) {
  if (!props.canWrite) return;
  picked.value = pin;
  composerText.value = "";
  void nextTick(() => composerInput.value?.focus());
}

function cancel() {
  picked.value = null;
  composerText.value = "";
}

async function post() {
  const pin = picked.value;
  const body = composerText.value.trim();
  if (!pin || !body || posting.value) return;
  posting.value = true;
  try {
    const id = await props.createPin(pin, body);
    cancel();
    if (id) focus(id);
  } finally {
    posting.value = false;
  }
}

function composerKey(event: KeyboardEvent) {
  if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    void post();
  } else if (event.key === "Escape") {
    event.preventDefault();
    cancel();
  }
}

function sendReply(thread: DesignThread, event: KeyboardEvent) {
  const input = event.target as HTMLInputElement;
  const body = input.value.trim();
  if (event.key !== "Enter" || event.isComposing || !body) return;
  input.value = "";
  void props.reply(thread.id, body);
}

/** The person's feedback that did not provably reach the agent, if any. */
function uncertain(thread: DesignThread) {
  return [...thread.comments].reverse().find((comment) => comment.author === "operator" && comment.delivery?.state === "uncertain")
    ?.delivery;
}

// The thread with the newest comment: the panel snaps to it when one lands,
// unless you are typing in a thread or the composer (as in the prototype).
const newest = computed(() => {
  let best: { id: string; at: number } | null = null;
  for (const thread of props.threads) {
    for (const comment of thread.comments) {
      const at = Date.parse(comment.createdAt);
      if (!best || at > best.at) best = { id: thread.id, at };
    }
  }
  return best;
});
let seen = 0;
watch(
  newest,
  (latest, previous) => {
    if (!latest || latest.at <= seen) return;
    seen = latest.at;
    const typing = document.activeElement?.closest?.(".room-thread, .room-composer");
    if (previous !== undefined && typing) return;
    focus(latest.id);
  },
  { immediate: true },
);
watch(() => props.mockup?.artifactId, () => (detached.value = new Set()));
</script>

<template>
  <div class="room" data-testid="design-mockup-room">
    <div class="room-stage">
      <DesignMockupFrame
        v-if="mockup"
        :repo-id="mockup.repoId"
        :artifact-id="mockup.artifactId"
        :entrypoint="mockup.entrypoint"
        :title="t('design.mockupTitle', { position: positionLabel })"
        :pins="markers"
        :reveal="reveal"
        @pick="onPick"
        @focus="(id) => focus(id)"
        @detached="(ids) => (detached = new Set(ids))"
      />
      <p v-else class="waiting" data-testid="design-mockup-waiting">{{ t("design.room.waiting") }}</p>
    </div>
    <aside class="room-comments" :aria-label="t('design.room.title')">
      <h2>{{ t("design.room.title") }}</h2>
      <p class="hint">{{ t("design.room.hint") }}</p>
      <div ref="list" class="room-threads">
        <div v-if="!threads.length" class="empty">{{ t("design.room.empty") }}</div>
        <div
          v-for="thread in threads"
          :key="thread.id"
          class="room-thread"
          :class="{ focus: thread.id === focusId, resolved: thread.status === 'resolved' }"
          :data-thread-id="thread.id"
          :data-testid="`design-pin-thread-${thread.number}`"
          @click="showInMockup(thread.id)"
        >
          <div class="anchor">
            <span class="num">{{ thread.number }}</span>
            <span :class="{ detached: detached.has(thread.id) }">
              <template v-if="detached.has(thread.id)">{{ t("design.room.elementGone") }} · </template>
              {{ thread.anchor?.element ? pinLabel(thread.anchor.element) : thread.anchor?.quotedText ?? "" }}
            </span>
          </div>
          <div v-for="comment in thread.comments" :key="comment.id" class="c" :class="{ agent: comment.author === 'agent' }">
            <span class="who">{{ comment.author === "agent" ? t("design.feedback.agent") : t("design.editor.person") }}</span>{{ comment.body }}
          </div>
          <p v-if="uncertain(thread)" class="undelivered">
            {{ t("design.delivery.uncertain") }}
            <button v-if="canWrite" type="button" class="kd-linkish" @click.stop="retry(uncertain(thread)!.id)">{{ t("design.feedback.resend") }}</button>
          </p>
          <div class="reply" @click.stop>
            <input
              v-if="canWrite"
              :placeholder="t('design.feedback.replyPlaceholder')"
              :aria-label="t('design.feedback.replyLabel', { number: thread.number })"
              @keydown="sendReply(thread, $event)"
            />
            <button v-if="canWrite" type="button" @click="resolve(thread.id, thread.status !== 'resolved')">
              {{ thread.status === "resolved" ? t("design.feedback.reopen") : t("design.feedback.resolve") }}
            </button>
          </div>
        </div>
      </div>
      <div v-if="picked" class="room-composer" data-testid="design-pin-composer">
        <div class="anchor">&lt;{{ picked.tag }}&gt; “{{ picked.excerpt.length > 60 ? `${picked.excerpt.slice(0, 60)}…` : picked.excerpt }}”</div>
        <textarea
          ref="composerInput"
          v-model="composerText"
          :placeholder="t('design.room.composerPlaceholder')"
          :aria-label="t('design.room.composerPlaceholder')"
          data-testid="design-pin-body"
          @keydown="composerKey"
        />
        <div class="row">
          <button type="button" @click="cancel">{{ t("design.pin.cancel") }}</button>
          <button type="button" class="post" :disabled="!composerText.trim() || posting" data-testid="design-pin-submit" @click="post">
            {{ t("design.pin.submit") }} <kbd class="kd-kbd">⌘↵</kbd>
          </button>
        </div>
      </div>
    </aside>
  </div>
</template>

<style scoped>
.room {
  flex: 1;
  display: flex;
  min-height: 0;
  background: var(--kd-bg);
  color: var(--kd-ink);
}
.room-stage {
  flex: 1;
  padding: 16px;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
.waiting {
  margin: 0;
  padding: 40px;
  font-family: sans-serif;
  color: #888;
  border: 1px solid var(--kd-line);
  border-radius: 10px;
  background: #fff;
  flex: 1;
}
.room-comments {
  width: 340px;
  flex-shrink: 0;
  border-left: 1px solid var(--kd-line);
  background: var(--kd-panel);
  display: flex;
  flex-direction: column;
  min-height: 0;
}
h2 {
  font-size: 12px;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  color: var(--kd-muted);
  margin: 14px 16px 6px;
}
.hint {
  font-size: 12px;
  color: var(--kd-muted);
  margin: 0 16px 8px;
}
.room-threads {
  overflow: auto;
  flex: 1;
  padding: 0 12px 16px;
}
.empty {
  color: var(--kd-muted);
  font-size: 13px;
  padding: 8px 4px;
}
.room-thread {
  border: 1px solid var(--kd-line);
  border-radius: 10px;
  padding: 10px 12px;
  margin: 8px 0;
  cursor: pointer;
}
.room-thread.focus {
  border-color: var(--kd-accent);
  box-shadow: 0 0 0 3px var(--kd-accent-soft);
}
.room-thread.resolved {
  opacity: 0.55;
}
.anchor {
  font-size: 11.5px;
  color: var(--kd-muted);
  margin-bottom: 6px;
  display: flex;
  gap: 6px;
  align-items: baseline;
  overflow-wrap: anywhere;
}
.num {
  flex-shrink: 0;
  white-space: nowrap;
  background: var(--kd-accent);
  color: #fff;
  border-radius: 10px;
  font-size: 11px;
  font-weight: 700;
  padding: 1px 7px;
}
.detached {
  color: var(--kd-bad);
}
.c {
  font-size: 13px;
  margin: 5px 0;
  line-height: 1.4;
  white-space: pre-wrap;
}
.c .who {
  font-weight: 600;
  margin-right: 4px;
}
.c.agent .who {
  color: var(--kd-reply);
}
.undelivered {
  margin: 4px 0 0;
  font-size: 11.5px;
  color: var(--kd-bad);
}
.reply {
  display: flex;
  gap: 6px;
  margin-top: 8px;
}
.reply input,
.room-composer textarea {
  flex: 1;
  font: inherit;
  font-size: 13px;
  border: 1px solid var(--kd-line);
  border-radius: 7px;
  padding: 6px 8px;
  background: var(--kd-panel);
  color: var(--kd-ink);
}
.reply button,
.room-composer button {
  font: inherit;
  border: 1px solid var(--kd-line);
  background: var(--kd-panel);
  color: var(--kd-ink);
  border-radius: 7px;
  padding: 6px 11px;
  cursor: pointer;
}
.room-composer {
  border-top: 1px solid var(--kd-line);
  padding: 12px 16px;
}
.room-composer textarea {
  width: 100%;
  min-height: 64px;
  resize: vertical;
  box-sizing: border-box;
}
.room-composer .row {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
  margin-top: 8px;
}
.room-composer .post {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  border-radius: 999px;
  background: linear-gradient(135deg, #8b5cf6, #6d4be0);
  border: 0;
  color: #fff;
  font-weight: 600;
  padding: 6px 12px;
}
.room-composer .post:disabled {
  opacity: 0.6;
  cursor: default;
}
</style>
