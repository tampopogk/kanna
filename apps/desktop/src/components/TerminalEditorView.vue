<script setup lang="ts">
import TerminalView from "./TerminalView.vue";
import type { TerminalEditorSession } from "../services/desktopServerClient";
defineProps<{ session: TerminalEditorSession; active: boolean; currentWorktree?: string }>();
defineEmits<{ (e: "agent"): void }>();
</script>
<template>
  <div class="editor-view">
    <div class="editor-info">
      <div class="editor-heading">
        <span :title="session.worktreePath">{{ session.command }} · {{ session.worktreePath }}</span>
        <button @click="$emit('agent')">Return to agent</button>
      </div>
      <p v-if="currentWorktree !== session.worktreePath">Earlier workspace — this editor has not moved to the task’s current stage.</p>
      <p>Save and quit in the editor. Closing this tab hides the session; closing the task ends it. <kbd>⌘</kbd><kbd>S</kbd> does not save or advance here.</p>
    </div>
    <!-- Attach only: a missing editor must never silently restart or relocate. -->
    <TerminalView attach-only :key="session.sessionId" :session-id="session.sessionId" :active="active" :worktree-path="session.worktreePath" />
  </div>
</template>
<style scoped>
.editor-view { display: flex; flex: 1; flex-direction: column; min-height: 0; height: 100%; }
.editor-info { padding: 6px 12px; font-size: 12px; color: var(--kn-text-muted); border-bottom: 1px solid var(--kn-border-default); }
p { margin: 4px 0 0; }
.editor-heading { display: flex; align-items: center; gap: 12px; }
.editor-heading span { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
kbd + kbd { margin-left: 2px; }
button { flex-shrink: 0; color: var(--kn-text-primary); background: var(--kn-bg-panel); border: 1px solid var(--kn-border-default); border-radius: 4px; cursor: pointer; }
</style>
