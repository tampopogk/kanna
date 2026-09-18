<script setup lang="ts">
import { computed, onBeforeUnmount, ref, useId, watch } from "vue";
import {
  fetchDesktopCopilotModels,
  fetchDesktopOpenCodeModels,
} from "../services/desktopServerClient";

interface ModelOption {
  id: string;
  name?: string;
  connection?: string | null;
  local?: boolean;
  context?: number | null;
}

type ModelSelectorProvider = "opencode" | "copilot";

const props = withDefaults(defineProps<{
  repoId?: string;
  modelValue: string;
  provider?: ModelSelectorProvider;
}>(), {
  provider: "opencode",
});
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
const models = ref<ModelOption[]>([]);
const error = ref("");
const loading = ref(false);
const inputRef = ref<HTMLInputElement | null>(null);
const listId = useId();
const selected = computed(() => models.value.find(model => model.id === props.modelValue));
const isOpenCode = computed(() => props.provider === "opencode");
let revision = 0;

async function load() {
  const request = ++revision;
  models.value = [];
  error.value = "";
  if (!props.repoId) {
    loading.value = false;
    return;
  }
  loading.value = true;
  try {
    const result = props.provider === "opencode"
      ? await fetchDesktopOpenCodeModels(props.repoId)
      : await fetchDesktopCopilotModels(props.repoId);
    if (request === revision) models.value = result;
  } catch (reason) {
    if (request === revision) error.value = reason instanceof Error ? reason.message : String(reason);
  } finally {
    if (request === revision) loading.value = false;
  }
}

function handleInput(event: Event) {
  emit("update:modelValue", (event.target as HTMLInputElement).value.trim());
}

function handleChange(event: Event) {
  const input = event.currentTarget as HTMLInputElement;
  if (models.value.some(model => model.id === input.value.trim())) input.blur();
}

watch([() => props.repoId, () => props.provider], load, { immediate: true });

// WebKit renders datalist suggestions in a native overlay. Dismiss it while
// the input still exists so changing away from OpenCode cannot leave that
// overlay composited over the next provider's form.
onBeforeUnmount(() => inputRef.value?.blur());
</script>

<template>
  <div class="opencode-model-select">
    <label :for="`${listId}-input`">{{ isOpenCode ? "OpenCode model" : "GitHub Copilot model" }}</label>
    <div class="model-input">
      <input
        ref="inputRef"
        :id="`${listId}-input`"
        :list="listId"
        :value="modelValue"
        :placeholder="isOpenCode ? 'Native default, or backend/model' : 'Native default, or model ID'"
        :aria-label="isOpenCode ? 'OpenCode model' : 'GitHub Copilot model'"
        @input="handleInput"
        @change="handleChange"
      />
      <button type="button" :disabled="loading || !repoId" @click="load">Refresh</button>
    </div>
    <datalist :id="listId">
      <option v-for="model in models" :key="model.id" :value="model.id">
        {{ isOpenCode && model.local ? 'Local · ' : '' }}{{ model.name ?? model.id }}
      </option>
    </datalist>
    <small v-if="loading" role="status">Reading {{ isOpenCode ? "OpenCode" : "Copilot" }} models…</small>
    <small v-if="error" role="alert">
      {{ error }}. You can enter {{ isOpenCode ? "a native backend/model ID" : "a Copilot model ID" }} directly.
    </small>
    <small v-if="selected && isOpenCode">
      {{ selected.local ? 'Local connection' : 'Configured connection' }}{{ selected.connection ? ` · ${selected.connection}` : '' }}{{ selected.context ? ` · ${selected.context.toLocaleString()} context` : '' }}.
      Server readiness has not been checked.
    </small>
    <small v-if="isOpenCode">
      The backend namespace and connections are configured in OpenCode on the machine running this task. An explicit model also handles auxiliary inference for this stage.
    </small>
    <small v-else>
      Suggestions are recently used Copilot models on this machine, not a catalog. You can enter any Copilot model ID.
    </small>
  </div>
</template>

<style scoped>
.opencode-model-select { display: grid; gap: 6px; margin: 10px 0; font-size: 12px; }
.model-input { display: flex; gap: 6px; }
input { flex: 1; min-width: 0; padding: 6px; color: var(--kn-text-primary); background: var(--kn-bg-panel-raised); border: 1px solid var(--kn-border-default); border-radius: 4px; }
small { color: var(--kn-text-muted); }
button { color: var(--kn-text-primary); background: var(--kn-bg-panel-raised); border: 1px solid var(--kn-border-default); border-radius: 4px; }
</style>
