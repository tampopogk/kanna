<script setup lang="ts">
import TerminalView from "./TerminalView.vue";
import type { TerminalEditorSession } from "../services/desktopServerClient";
withDefaults(defineProps<{ session: TerminalEditorSession; active: boolean; visible?: boolean }>(), { visible: undefined });
</script>
<template>
  <div class="editor-view">
    <!-- Attach only: a missing editor must never silently restart or relocate. -->
    <TerminalView attach-only :key="session.sessionId" :session-id="session.sessionId" :active="active" :visible="visible" :worktree-path="session.worktreePath" />
  </div>
</template>
<style scoped>
.editor-view { display: flex; flex: 1; flex-direction: column; min-height: 0; height: 100%; }
</style>
