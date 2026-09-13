<script setup lang="ts">
import { computed, ref, nextTick, onBeforeUnmount } from "vue";
import { useI18n } from "vue-i18n";

import AgentStageSelector from "./AgentStageSelector.vue";
import type { AgentTerminalAttempt } from "../services/desktopServerClient";
import type { MainTab } from "../composables/useMainTabs";

const props = defineProps<{
  tabs: MainTab[];
  agentAttempts?: AgentTerminalAttempt[];
  selectedAttempt?: string;
  activeTabId: string;
  worktreePath?: string;
  newViews?: { id: string; label: string }[];
  paneId?: string;
  scopeKey?: string | null;
}>();

const emit = defineEmits<{
  (e: "select", id: string): void;
  (e: "selectAttempt", id: string): void;
  (e: "close", id: string): void;
  (e: "new", id: string): void;
  (e: "dropTab", id: string, beforeId?: string): void;
}>();

const { t } = useI18n();

interface MainTabPresentation {
  id: string;
  label: string;
  title: string;
  closable: boolean;
}

/** Views with exactly one tab per scope carry a fixed label. */
const FIXED_LABEL_KEYS: Partial<Record<MainTab["kind"], string>> = {
  agent: "mainTabs.agent",
  diff: "mainTabs.diff",
  tree: "mainTabs.files",
  graph: "mainTabs.graph",
  analytics: "mainTabs.analytics",
};

function lastPathSegment(value: string): string {
  const segments = value.split(/[/?#]/).filter(Boolean);
  return segments.at(-1) ?? value;
}

function present(tab: MainTab): MainTabPresentation {
  const closable = tab.kind !== "agent";
  if (tab.kind === "preview") return { id: tab.id, label: `Preview: ${tab.portName}`, title: `Task preview · ${tab.portName}`, closable };
  if (tab.kind === "editor") {
    const session = tab.editorSession;
    const workspaceLabel = session && props.worktreePath && session.worktreePath !== props.worktreePath
      ? ` · ${lastPathSegment(session.worktreePath)}` : "";
    return { id: tab.id, label: `Edit: ${lastPathSegment(session?.filePath ?? "")}${workspaceLabel}`, title: `${session?.command} — ${session?.worktreePath}/${session?.filePath}`, closable };
  }
  if (tab.kind === "file") {
    const filePath = tab.filePath ?? "";
    return { id: tab.id, label: lastPathSegment(filePath), title: filePath, closable };
  }
  if (tab.kind === "image") {
    const imageUrl = tab.imageUrl ?? "";
    return {
      id: tab.id,
      label: lastPathSegment(imageUrl) || t("mainTabs.image"),
      title: imageUrl,
      closable,
    };
  }
  if (tab.kind === "shell") {
    const label = t(tab.shellScope === "repo" ? "mainTabs.repoShell" : "mainTabs.shell");
    return { id: tab.id, label, title: label, closable };
  }
  const label = t(FIXED_LABEL_KEYS[tab.kind] ?? "mainTabs.agent");
  return { id: tab.id, label, title: label, closable };
}

const presented = computed<MainTabPresentation[]>(() => props.tabs.map(present));
const menu = ref<HTMLElement | null>(null);
const menuButton = ref<HTMLButtonElement | null>(null);
const menuOpen = ref(false);
const menuPosition = ref({ left: "0px", top: "0px" });
function closeMenu(event?: PointerEvent) {
  if (event && (menu.value?.contains(event.target as Node) || menuButton.value?.contains(event.target as Node))) return;
  menuOpen.value = false;
  document.removeEventListener("pointerdown", closeMenu);
}
function toggleMenu() {
  if (menuOpen.value) { closeMenu(); return; }
  const rect = menuButton.value?.getBoundingClientRect();
  if (!rect) return;
  menuPosition.value = { left: `${Math.max(8, Math.min(rect.left, window.innerWidth - 248))}px`, top: `${rect.bottom + 4}px` };
  menuOpen.value = true;
  document.addEventListener("pointerdown", closeMenu);
  void nextTick(() => menu.value?.querySelector('button')?.focus());
}
function openView(id: string) { closeMenu(); emit("new", id); }
function dragTab(event: DragEvent, id: string) {
  event.dataTransfer?.setData('application/x-kanna-tab', JSON.stringify({ id, scope: props.scopeKey }));
  if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move';
}
function dropTab(event: DragEvent, beforeId?: string) {
  const raw = event.dataTransfer?.getData('application/x-kanna-tab');
  if (!raw) return;
  try {
    const value = JSON.parse(raw) as { id?: unknown; scope?: unknown };
    if (typeof value.id === 'string' && value.scope === props.scopeKey) emit('dropTab', value.id, beforeId);
  } catch (error) { console.debug('[main-tabs] ignored malformed tab drag', error); }
}
function menuKey(event: KeyboardEvent) {
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
  event.preventDefault();
  const buttons = Array.from(menu.value?.querySelectorAll('button') ?? []);
  const current = buttons.findIndex(button => button === document.activeElement);
  const next = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : (current + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length;
  buttons[next]?.focus();
}
onBeforeUnmount(() => closeMenu());
</script>

<template>
  <div class="main-tab-bar" role="tablist" data-testid="main-tab-bar" @dragover.prevent @drop.prevent="dropTab($event)">
    <div
      v-for="tab in presented"
      :key="tab.id"
      class="main-tab"
      :draggable="!!paneId"
      @dragstart="dragTab($event, tab.id)"
      @drop.stop.prevent="dropTab($event, tab.id)"
      :class="{ active: tab.id === activeTabId }"
      role="tab"
      tabindex="0"
      @keydown.enter.prevent="emit('select', tab.id)"
      @keydown.space.prevent="emit('select', tab.id)"
      :aria-selected="tab.id === activeTabId"
      :title="tab.title"
      :data-testid="`main-tab-${tab.id}`"
      @click="emit('select', tab.id)"
      @auxclick.middle.prevent="tab.closable && emit('close', tab.id)"
    >
      <span class="main-tab-label">{{ tab.label }}</span>
      <AgentStageSelector v-if="tab.id === 'agent' && agentAttempts" :attempts="agentAttempts" :selected="selectedAttempt ?? ''" @select="emit('selectAttempt', $event)" />
      <button
        v-if="tab.closable"
        type="button"
        class="main-tab-close"
        :aria-label="$t('actions.close')"
        :data-testid="`main-tab-close-${tab.id}`"
        @click.stop="emit('close', tab.id)"
      >
        ×
      </button>
    </div>
    <button v-if="newViews?.length" ref="menuButton" class="new-tab" aria-label="New tab" :aria-expanded="menuOpen" aria-haspopup="menu" @click="toggleMenu">+</button>
    <Teleport to="body">
      <div v-if="menuOpen" ref="menu" class="new-tab-menu" :style="menuPosition" role="menu" @keydown="menuKey" @keydown.esc.stop="closeMenu(); menuButton?.focus()">
        <button v-for="view in newViews" :key="view.id" role="menuitem" @click="openView(view.id)">{{ view.label }}</button>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.new-tab { border: 0; background: transparent; color: var(--kn-text-secondary); font-size: 22px; padding: 0 10px; cursor: pointer; }
.new-tab-menu { position: fixed; z-index: 1000; display: flex; flex-direction: column; width: 240px; padding: 6px; border: 1px solid var(--kn-border-default); border-radius: 10px; background: var(--kn-bg-sidebar); box-shadow: 0 10px 30px #0004; }
.new-tab-menu button { text-align: left; padding: 10px; border: 0; border-radius: 5px; background: transparent; color: var(--kn-text-primary); cursor: pointer; }
.new-tab-menu button:hover, .new-tab-menu button:focus-visible { background: var(--kn-bg-hover); }
.main-tab-bar {
  display: flex;
  align-items: stretch;
  gap: 2px;
  padding: 0 8px;
  border-bottom: 1px solid var(--kn-border-default);
  background: var(--kn-bg-sidebar);
  overflow-x: auto;
  scrollbar-width: none;
  flex-shrink: 0;
}

.main-tab-bar::-webkit-scrollbar {
  display: none;
}

.main-tab {
  display: flex;
  align-items: center;
  gap: 6px;
  max-width: 220px;
  padding: 6px 8px 5px;
  border-bottom: 2px solid transparent;
  color: var(--kn-text-muted);
  font-size: 12px;
  white-space: nowrap;
  cursor: pointer;
  user-select: none;
}

.main-tab:hover {
  color: var(--kn-text-secondary);
  background: var(--kn-bg-hover);
}

.main-tab.active {
  color: var(--kn-text-primary);
  border-bottom-color: var(--kn-accent);
}

.main-tab-label {
  overflow: hidden;
  text-overflow: ellipsis;
}

.main-tab-close {
  border: 0;
  padding: 0 2px;
  background: transparent;
  color: var(--kn-text-muted);
  font-size: 14px;
  line-height: 1;
  cursor: pointer;
  visibility: hidden;
}

.main-tab:hover .main-tab-close,
.main-tab.active .main-tab-close {
  visibility: visible;
}

.main-tab-close:hover {
  color: var(--kn-text-primary);
}
</style>
