<script setup lang="ts">
import { computed, onBeforeUnmount, ref, useId, watch } from "vue";
import { fetchDesktopOpenCodeModels, type OpenCodeModelOption } from "../services/desktopServerClient";

const props = defineProps<{ repoId?: string; modelValue: string }>();
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
const models = ref<OpenCodeModelOption[]>([]);
const error = ref("");
const loading = ref(false);
const inputRef = ref<HTMLInputElement | null>(null);
const listId = useId();
const selected = computed(() => models.value.find(model => model.id === props.modelValue));
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
    const result = await fetchDesktopOpenCodeModels(props.repoId);
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

watch(() => props.repoId, load, { immediate: true });

// WebKit renders datalist suggestions in a native overlay. Dismiss it while
// the input still exists so changing away from OpenCode cannot leave that
// overlay composited over the next provider's form.
onBeforeUnmount(() => inputRef.value?.blur());
</script>

<template>
  <div class="opencode-model-select">
    <label :for="`${listId}-input`">OpenCode model</label>
    <div class="model-input">
      <input
        ref="inputRef"
        :id="`${listId}-input`"
        :list="listId"
        :value="modelValue"
        placeholder="Native default, or backend/model"
        aria-label="OpenCode model"
        @input="handleInput"
        @change="handleChange"
      />
      <button type="button" :disabled="loading || !repoId" @click="load">Refresh</button>
    </div>
    <datalist :id="listId">
      <option v-for="model in models" :key="model.id" :value="model.id">{{ model.local ? 'Local · ' : '' }}{{ model.name }}</option>
    </datalist>
    <small v-if="loading" role="status">Reading OpenCode models…</small>
    <small v-if="error" role="alert">{{ error }}. You can enter a native backend/model ID directly.</small>
    <small v-if="selected">
      {{ selected.local ? 'Local connection' : 'Configured connection' }}{{ selected.connection ? ` · ${selected.connection}` : '' }}{{ selected.context ? ` · ${selected.context.toLocaleString()} context` : '' }}.
      Server readiness has not been checked.
    </small>
    <small>The backend namespace and connections are configured in OpenCode on the machine running this task. An explicit model also handles auxiliary inference for this stage.</small>
  </div>
</template>

<style scoped>
.opencode-model-select { display: grid; gap: 6px; margin: 10px 0; font-size: 12px; }
.model-input { display: flex; gap: 6px; }
input { flex: 1; min-width: 0; padding: 6px; color: var(--kn-text-primary); background: var(--kn-bg-panel-raised); border: 1px solid var(--kn-border-default); border-radius: 4px; }
small { color: var(--kn-text-muted); }
button { color: var(--kn-text-primary); background: var(--kn-bg-panel-raised); border: 1px solid var(--kn-border-default); border-radius: 4px; }
</style>
