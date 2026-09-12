<script setup lang="ts">
import { computed, ref, shallowRef, watch } from "vue";
import { parseAgentProviderSelector } from "../../../../packages/core/src/config/agent-providers";
import type { PipelineItem } from "../types/kanna";
import { useKannaStore } from "../stores/kanna";
import {
  fetchDesktopTaskDetail,
  replaceDesktopTaskWorkflow,
  type PinnedTaskWorkflow,
} from "../services/desktopServerClient";
import OpenCodeModelSelect from "./OpenCodeModelSelect.vue";

const props = defineProps<{ task: PipelineItem }>();
const open = ref(false);
const model = ref("");
const nextStage = ref("");
const message = ref("");
const pending = ref(false);
const throughPost = ref(false);
const pinnedWorkflow = shallowRef<PinnedTaskWorkflow | null>(null);
const valid = computed(() => Boolean(nextStage.value && model.value.includes('/') && !pending.value));
const actionLabel = computed(() => pending.value
  ? "Applying…"
  : throughPost.value ? `Save model for ${nextStage.value}` : `Advance to ${nextStage.value}`);

watch([() => props.task.id, () => props.task.stage], () => {
  open.value = false;
  model.value = "";
  pinnedWorkflow.value = null;
});

async function show() {
  open.value = !open.value;
  if (!open.value) return;
  nextStage.value = "";
  pinnedWorkflow.value = null;
  message.value = "";
  const id = props.task.id;
  const stage = props.task.stage;
  try {
    // Existing tasks use their pinned definition, not today's repo default.
    const detail = await fetchDesktopTaskDetail(id);
    const workflow = detail.workflowDefinition;
    if (props.task.id !== id || props.task.stage !== stage) return;
    if (!workflow || detail.id !== id || detail.stage !== stage) {
      message.value = "The task changed or its pinned workflow is unavailable. Reopen this control to refresh.";
      return;
    }
    const index = workflow.stages.findIndex(candidate => candidate.name === stage);
    if (index < 0) {
      message.value = "The current stage is absent from this workflow.";
      return;
    }
    throughPost.value = Boolean(workflow.stages[index]?.post);
    pinnedWorkflow.value = workflow;
    nextStage.value = workflow.stages[index + 1]?.name ?? "";
    if (!nextStage.value) message.value = "This is the final stage.";
  } catch (error) {
    if (props.task.id === id && props.task.stage === stage) message.value = String(error);
  }
}

async function apply() {
  if (!valid.value || props.task.has_running_post || props.task.stage_advance_pending) return;
  pending.value = true;
  message.value = "";
  try {
    if (throughPost.value) {
      const before = pinnedWorkflow.value;
      if (!before) return;
      const selector = `opencode-${model.value}`;
      // Compact workflow selectors have an effort suffix; never silently
      // reinterpret part of a native model ID as reasoning effort.
      if (parseAgentProviderSelector(selector)?.model !== model.value) {
        message.value = "This model ID is ambiguous in workflow selector syntax. Use a native OpenCode model alias without an effort suffix for a saved stage selection.";
        return;
      }
      const after = {
        ...before,
        stages: before.stages.map(stage => stage.name === nextStage.value
          ? { ...stage, agent_provider: selector }
          : stage),
      };
      // Compare-and-set preserves concurrent edits. Saving does not dispatch
      // a post or advance, and all other workflow fields remain intact.
      pinnedWorkflow.value = await replaceDesktopTaskWorkflow(props.task.id, before, after);
      message.value = `Saved for ${nextStage.value}, including future reruns of that stage. Advance normally when ready; the current post and session are unchanged.`;
      return;
    }
    const result = await useKannaStore().advanceStage(props.task.id, {
      nextStageAgentProvider: "opencode",
      nextStageModel: model.value,
    });
    if (result === "advanced") open.value = false;
  } catch (error) {
    message.value = String(error);
  } finally {
    pending.value = false;
  }
}
</script>

<template>
  <div class="stage-model-control">
    <button
      type="button"
      :aria-expanded="open"
      :disabled="task.stage_advance_pending || Boolean(task.has_running_post)"
      @click="show"
    >Next stage model…</button>
    <div v-if="open" class="stage-model-panel">
      <p v-if="message" role="status">{{ message }}</p>
      <template v-if="nextStage">
        <p>
          Run <strong>{{ nextStage }}</strong> with OpenCode.
          {{ throughPost
            ? "Save this stage’s choice in this task’s workflow, then advance normally through the current post."
            : "This changes only the next stage advance." }}
        </p>
        <OpenCodeModelSelect v-model="model" :repo-id="task.repo_id" />
        <button type="button" :disabled="!valid" @click="apply">{{ actionLabel }}</button>
      </template>
    </div>
  </div>
</template>

<style scoped>
.stage-model-control {
  padding: 6px 16px;
  border-bottom: 1px solid var(--kn-border-default);
  font-size: 12px;
}
.stage-model-panel { max-width: 640px; padding: 6px 0; }
button {
  color: var(--kn-text-primary);
  background: var(--kn-bg-panel-raised);
  border: 1px solid var(--kn-border-default);
  border-radius: 4px;
  padding: 4px 8px;
}
button:disabled { opacity: .5; }
</style>
