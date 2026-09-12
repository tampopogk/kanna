<script setup lang="ts">
import { ref } from "vue";
import {
  DesktopServerRequestError,
  fetchTerminalEditorChoices,
  type TerminalEditorChoice,
} from "../services/desktopServerClient";

const props = defineProps<{ openEditor: (command: string) => Promise<void> }>();
const choosing = ref(false);
const busy = ref(false);
const error = ref("");
const choices = ref<TerminalEditorChoice[]>([]);
const command = ref("");

function showError(e: unknown) {
  console.error("[terminal-editor]", e);
  error.value = e instanceof DesktopServerRequestError ? e.body : String(e);
}

async function choose() {
  choosing.value = true;
  busy.value = true;
  error.value = "";
  try {
    choices.value = await fetchTerminalEditorChoices();
    command.value = choices.value[0]?.command ?? "";
  } catch (e) {
    showError(e);
  } finally {
    busy.value = false;
  }
}

async function start() {
  busy.value = true;
  error.value = "";
  try {
    await props.openEditor(command.value);
    choosing.value = false;
  } catch (e) {
    showError(e);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="editor-picker">
    <button type="button" data-testid="edit-in-terminal" @click="choose" :disabled="busy">Edit in terminal…</button>
    <div v-if="choosing" class="editor-choice" @keydown.esc.stop="choosing = false">
      <p>Use the editor’s own save and quit commands. Closing its tab hides it; closing the task ends it and loses unsaved buffers. Stage changes leave it in its original workspace. The agent can also write these files.</p>
      <p v-if="error" role="alert">{{ error }}</p>
      <template v-else>
        <label>Terminal editor <select v-model="command" :disabled="busy" aria-label="Terminal editor">
          <option v-for="choice in choices" :key="choice.command" :value="choice.command">{{ choice.command }} ({{ choice.executable }})</option>
        </select></label>
        <button type="button" :disabled="busy || !command" data-testid="start-terminal-editor" @click="start">Open editor</button>
      </template>
      <button type="button" :disabled="busy" @click="choosing = false">Cancel</button>
    </div>
  </div>
</template>
<style scoped>
.editor-picker { position: relative; }
button, select { color: var(--kn-text-primary); background: var(--kn-bg-panel); border: 1px solid var(--kn-border-default); border-radius: 4px; padding: 4px 8px; }
.editor-choice { position: absolute; z-index: 10; right: 0; top: 100%; width: 380px; padding: 12px; background: var(--kn-bg-panel); border: 1px solid var(--kn-border-strong); border-radius: 6px; white-space: normal; font-size: 12px; }
p { margin: 0 0 10px; }
select { width: 100%; margin: 6px 0; }
button { cursor: pointer; }
</style>
