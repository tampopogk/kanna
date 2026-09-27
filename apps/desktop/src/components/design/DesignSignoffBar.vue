<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { DesignApproval, DesignCandidate } from "@kanna/design-editor";

/**
 * The sign-off bar under the design (docs/specs/app-design.md §6), as in the
 * design prototype: where the work lives on the left, "Approve for build →"
 * on the right.
 *
 * Approving first prepares a candidate — the disposable repository is
 * committed and an immutable snapshot published — then asks, naming what is
 * kept and what happens next. Only the person's click there confirms it, and
 * the confirmation is bound to exactly that candidate. No version number is
 * shown; the approval records the exact commit.
 */
const props = defineProps<{
  approval: DesignApproval | null;
  /** designing | handing_off | handed_off */
  status: string;
  inDesignStage: boolean;
  nextStage: string | null;
  scratchRepository: string | null;
  /** A sync problem worth saying, if any. */
  syncProblem: string | null;
  prepare: () => Promise<DesignCandidate>;
  confirm: (candidate: DesignCandidate) => Promise<unknown>;
  reopen: () => Promise<unknown>;
  retry: () => Promise<unknown>;
}>();
const { t } = useI18n();

const candidate = ref<DesignCandidate | null>(null);
const working = ref<"preparing" | "confirming" | "reopening" | "retrying" | null>(null);
const error = ref<string | null>(null);

const phase = computed(() => props.approval?.phase ?? null);
const handingOff = computed(() => ["approved", "exported", "committing", "committed"].includes(phase.value ?? ""));
const handedOff = computed(() => phase.value === "entered" || props.status === "handed_off");
const failed = computed(() => phase.value === "failed");
const canReopen = computed(
  () => (handingOff.value && phase.value !== "committing" && phase.value !== "committed") || failed.value,
);

const progress = computed(() => {
  switch (phase.value) {
    case "approved":
      return t("design.approval.progress.approved");
    case "exported":
      return props.approval?.error ?? t("design.approval.progress.exported");
    case "committing":
      return t("design.approval.progress.committing");
    case "committed":
      return t("design.approval.progress.committed");
    default:
      return "";
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
  <div class="signoff-bar" data-testid="design-approval-bar">
    <span class="repo">
      <span v-if="error && !candidate" class="kd-bad" role="alert">{{ error }}</span>
      <span v-else-if="failed" class="kd-bad" :title="approval?.error ?? undefined">
        {{ t("design.approval.failed", { error: (approval?.error ?? "").slice(0, 80) }) }}
      </span>
      <span v-else-if="handingOff" class="working">{{ t("design.approval.approving") }} {{ progress }}</span>
      <template v-else-if="handedOff && approval">
        {{ t("design.approval.approvedLine", {
          commit: (approval.committedSha ?? approval.sourceCommit ?? "").slice(0, 7),
          artifact: (approval.artifactId ?? "").slice(0, 10),
        }) }}
      </template>
      <span v-else-if="syncProblem" class="kd-bad">{{ syncProblem }}</span>
      <template v-else-if="scratchRepository">{{ t("design.approval.repoLine", { path: scratchRepository }) }}</template>
    </span>
    <span class="spacer" />
    <button
      v-if="failed"
      type="button"
      class="kd-btn"
      :disabled="working !== null"
      data-testid="design-approval-retry"
      @click="run('retrying', retry)"
    >
      {{ t("design.approval.retry") }}
    </button>
    <button
      v-if="canReopen"
      type="button"
      class="kd-btn"
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
      class="kd-btn approve"
      :disabled="!inDesignStage || working !== null"
      data-testid="design-approve"
      @click="askToApprove"
    >
      {{ working === "preparing" ? t("design.approval.preparing") : t("design.approval.approve") }}
    </button>
  </div>
  <div v-if="candidate" class="modal-bg" @click.self="cancel">
    <section
      class="modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby="design-approve-title"
      data-testid="design-approve-dialog"
      @keydown.esc="cancel"
    >
      <h3 id="design-approve-title">{{ t("design.approval.dialogTitle") }}</h3>
      <div>{{ t("design.approval.dialogIntro") }}</div>
      <ul>
        <li>{{ t("design.approval.dialogCommit") }}</li>
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
      <p v-if="error" class="kd-bad" role="alert">{{ error }}</p>
      <div class="row">
        <button type="button" class="kd-btn" :disabled="working === 'confirming'" @click="cancel">
          {{ t("design.approval.keepIterating") }}
        </button>
        <button
          type="button"
          class="kd-btn approve"
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
.signoff-bar {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 16px;
  border-top: 1px solid var(--kd-line);
  background: var(--kd-panel);
}
.repo {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
  font-family: ui-monospace, Menlo, monospace;
  font-size: 11.5px;
  color: var(--kd-muted);
}
.working {
  color: var(--kd-warn-ink);
}
.spacer {
  flex: 1;
}
.modal-bg {
  position: fixed;
  inset: 0;
  background: rgba(10, 9, 14, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 40;
}
.modal {
  background: var(--kd-panel);
  color: var(--kd-ink);
  border: 1px solid var(--kd-line);
  border-radius: 12px;
  padding: 20px 22px;
  width: 400px;
  max-width: calc(100vw - 32px);
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.3);
}
.modal h3 {
  margin: 0 0 8px;
  font-size: 16px;
}
.modal ul {
  padding-left: 18px;
  margin: 8px 0 14px;
  line-height: 1.6;
  color: var(--kd-ink-2);
}
.modal ul.files {
  margin: 2px 0;
}
.warning {
  color: var(--kd-warn-ink);
  background: var(--kd-warn-soft);
  border-radius: 8px;
  padding: 6px 10px;
}
.row {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
</style>
