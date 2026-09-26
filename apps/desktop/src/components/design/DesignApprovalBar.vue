<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { DesignApproval, DesignCandidate } from "@kanna/design-editor";

/**
 * The hand-off bar under the design (docs/specs/app-design.md §6).
 *
 * "Approve for build →" prepares a candidate — the disposable repository is
 * committed and an immutable snapshot published — and shows what approving
 * will keep and what happens next. Only the person's click in that dialog
 * confirms it, and the confirmation is bound to exactly that candidate. The
 * UI never shows a version number; the approval records the exact commit
 * behind the scenes.
 */
const props = defineProps<{
  approval: DesignApproval | null;
  /** designing | handing_off | handed_off */
  status: string;
  inDesignStage: boolean;
  nextStage: string | null;
  prepare: () => Promise<DesignCandidate>;
  confirm: (candidate: DesignCandidate) => Promise<unknown>;
  reopen: () => Promise<unknown>;
  retry: () => Promise<unknown>;
}>();
const { t } = useI18n();

const candidate = ref<DesignCandidate | null>(null);
const working = ref<"preparing" | "confirming" | "reopening" | "retrying" | null>(null);
const error = ref<string | null>(null);

const handingOff = computed(() =>
  ["approved", "exported", "committing", "committed"].includes(props.approval?.phase ?? ""),
);
const handedOff = computed(() => props.approval?.phase === "entered" || props.status === "handed_off");
const failed = computed(() => props.approval?.phase === "failed");

const progress = computed(() => {
  switch (props.approval?.phase) {
    case "approved":
      return t("design.approval.progress.approved");
    case "exported":
      return props.approval.error ?? t("design.approval.progress.exported");
    case "committing":
      return t("design.approval.progress.committing");
    case "committed":
      return t("design.approval.progress.committed");
    case "entered":
      return t("design.approval.progress.entered", { stage: props.nextStage ?? "" });
    default:
      return null;
  }
});

async function run<T>(kind: NonNullable<typeof working.value>, action: () => Promise<T>): Promise<T | null> {
  working.value = kind;
  error.value = null;
  try {
    return await action();
  } catch (reason) {
    error.value = reason instanceof Error ? reason.message : String(reason);
    return null;
  } finally {
    working.value = null;
  }
}

async function askToApprove() {
  candidate.value = await run("preparing", props.prepare);
}

async function approve() {
  const shown = candidate.value;
  if (!shown) return;
  const confirmed = await run("confirming", () => props.confirm(shown));
  if (confirmed !== null) candidate.value = null;
}

function cancel() {
  candidate.value = null;
  error.value = null;
}
</script>

<template>
  <div class="design-approval-bar" data-testid="design-approval-bar">
    <span class="state" :class="{ failed }">
      <template v-if="failed">{{ t("design.approval.failed", { error: approval?.error ?? "" }) }}</template>
      <template v-else-if="progress">{{ progress }}</template>
      <template v-else>{{ t("design.approval.hint") }}</template>
    </span>
    <span v-if="error && !candidate" class="error" role="alert">{{ error }}</span>
    <span class="spacer" />
    <button
      v-if="failed"
      type="button"
      class="btn"
      :disabled="working !== null"
      data-testid="design-approval-retry"
      @click="run('retrying', retry)"
    >
      {{ t("design.approval.retry") }}
    </button>
    <button
      v-if="(handingOff && approval?.phase !== 'committing' && approval?.phase !== 'committed') || failed"
      type="button"
      class="btn"
      :disabled="working !== null"
      :title="t('design.approval.reopenHint')"
      data-testid="design-approval-reopen"
      @click="run('reopening', reopen)"
    >
      {{ t("design.approval.reopen") }}
    </button>
    <button
      v-if="!handingOff && !handedOff && !failed"
      type="button"
      class="btn approve"
      :disabled="!inDesignStage || working !== null"
      data-testid="design-approve"
      @click="askToApprove"
    >
      {{ working === "preparing" ? t("design.approval.preparing") : t("design.approval.approve") }}
    </button>
  </div>
  <div v-if="candidate" class="modal-scrim" @click.self="cancel">
    <section
      class="approve-dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="design-approve-title"
      data-testid="design-approve-dialog"
      @keydown.esc="cancel"
    >
      <h3 id="design-approve-title">{{ t("design.approval.dialogTitle") }}</h3>
      <p>{{ t("design.approval.dialogIntro") }}</p>
      <ul>
        <li>{{ t("design.approval.dialogSnapshot") }}</li>
        <li v-if="candidate.policy.files.length">
          {{ t("design.approval.dialogRetained") }}
          <ul class="files">
            <li v-for="file in candidate.policy.files" :key="file"><code>{{ file }}</code></li>
          </ul>
        </li>
        <li v-else>{{ t("design.approval.dialogNothingRetained") }}</li>
        <li>{{ t("design.approval.dialogNext", { stage: candidate.nextStage ?? "" }) }}</li>
        <li>{{ t("design.approval.dialogPrototype") }}</li>
      </ul>
      <p v-if="candidate.openThreads || candidate.undeliveredFeedback" class="warning">
        {{ t("design.approval.dialogOpenFeedback", { open: candidate.openThreads, waiting: candidate.undeliveredFeedback }) }}
      </p>
      <p v-if="error" class="error" role="alert">{{ error }}</p>
      <div class="dialog-actions">
        <button type="button" class="btn" :disabled="working === 'confirming'" @click="cancel">
          {{ t("design.approval.keepIterating") }}
        </button>
        <button
          type="button"
          class="btn approve"
          :disabled="working === 'confirming'"
          data-testid="design-approve-confirm"
          autofocus
          @click="approve"
        >
          {{ working === "confirming" ? t("design.approval.approving") : t("design.approval.confirm") }}
        </button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.design-approval-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  border-top: 1px solid var(--kn-border-default);
  background: var(--kn-bg-panel);
  font-size: 12px;
  color: var(--kn-text-secondary);
}
.state.failed,
.error {
  color: var(--kn-danger);
}
.spacer {
  flex: 1;
}
.btn {
  border: 1px solid var(--kn-border-strong);
  background: var(--kn-bg-panel-raised);
  color: var(--kn-text-primary);
  border-radius: 6px;
  padding: 5px 12px;
  font: inherit;
  font-size: 13px;
  cursor: pointer;
}
.btn:disabled {
  opacity: 0.6;
  cursor: default;
}
.btn.approve {
  background: var(--kn-success);
  border-color: var(--kn-success);
  color: var(--kn-text-inverse);
}
.modal-scrim {
  position: fixed;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--kn-overlay-scrim);
  z-index: 1000;
}
.approve-dialog {
  width: min(520px, calc(100vw - 32px));
  background: var(--kn-bg-panel);
  color: var(--kn-text-primary);
  border: 1px solid var(--kn-border-strong);
  border-radius: 10px;
  box-shadow: var(--kn-shadow-modal);
  padding: 18px 20px;
  font-size: 13px;
}
.approve-dialog h3 {
  margin: 0 0 8px;
  font-size: 15px;
}
.approve-dialog ul {
  padding-left: 18px;
}
.files {
  margin: 4px 0;
}
.warning {
  color: var(--kn-warning);
}
.dialog-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 14px;
}
</style>
