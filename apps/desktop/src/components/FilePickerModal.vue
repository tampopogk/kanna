<script setup lang="ts">
import { ref, computed, watch, onMounted, nextTick } from "vue";
import { invoke } from "../invoke";
import { fuzzyMatch, type FuzzyResult } from "../utils/fuzzyMatch";
import { useModalZIndex } from "../composables/useModalZIndex";
import { useToast } from "../composables/useToast";
import { macOsTextInputAttrs } from "../utils/textInput";
const { zIndex, bringToFront } = useModalZIndex();
defineExpose({ zIndex, bringToFront });

const props = defineProps<{
  worktreePath: string;
  repoRoot?: string;
  ideCommand?: string;
  sourceKey: string;
  taskDirectoryLoader?: (
    path: string,
    showAllFiles: boolean,
  ) => Promise<{ entries: { path: string; isDir: boolean }[] }>;
}>();

const emit = defineEmits<{
  (e: "close"): void;
  (e: "select", filePath: string, sourceKey: string): void;
}>();

const query = ref("");
const files = ref<string[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const selectedIndex = ref(0);
const inputRef = ref<HTMLInputElement | null>(null);
const mouseMoved = ref(false);
let nextLoadId = 0;

// Quick open needs names, never contents, but a hostile or accidental tree
// still must not make the viewer download an unbounded remote index.
const MAX_TASK_FILE_INDEX_ENTRIES = 50_000;
const TASK_DIRECTORY_CONCURRENCY = 8;

interface ScoredFile {
  path: string;
  result: FuzzyResult;
}

const filtered = computed((): ScoredFile[] => {
  const q = query.value.trim();
  if (!q) return files.value.slice(0, 100).map((path) => ({ path, result: { score: 0, indices: [] } }));
  const scored: ScoredFile[] = [];
  for (const f of files.value) {
    const result = fuzzyMatch(q, f);
    if (result) scored.push({ path: f, result });
  }
  scored.sort((a, b) => b.result.score - a.result.score);
  return scored.slice(0, 100);
});

async function listTaskFiles(
  loader: NonNullable<typeof props.taskDirectoryLoader>,
  loadId: number,
): Promise<string[]> {
  const directories = [""];
  const listedFiles: string[] = [];
  let nextDirectory = 0;
  let indexedEntries = 0;

  while (nextDirectory < directories.length) {
    if (loadId !== nextLoadId) return [];
    const batch = directories.slice(
      nextDirectory,
      nextDirectory + TASK_DIRECTORY_CONCURRENCY,
    );
    nextDirectory += batch.length;
    const listings = await Promise.all(batch.map((directory) => loader(directory, false)));
    if (loadId !== nextLoadId) return [];

    for (const listing of listings) {
      for (const entry of listing.entries) {
        indexedEntries += 1;
        if (indexedEntries > MAX_TASK_FILE_INDEX_ENTRIES) {
          throw new Error(
            `workspace has more than ${MAX_TASK_FILE_INDEX_ENTRIES.toLocaleString()} visible entries`,
          );
        }
        if (entry.isDir) directories.push(entry.path);
        else listedFiles.push(entry.path);
      }
    }
  }

  return listedFiles.sort();
}

async function loadFiles() {
  const loadId = ++nextLoadId;
  loading.value = true;
  error.value = null;
  files.value = [];
  try {
    const listed = props.taskDirectoryLoader
      ? await listTaskFiles(props.taskDirectoryLoader, loadId)
      : await invoke<string[]>("list_files", { path: props.worktreePath });
    if (loadId !== nextLoadId) return;
    files.value = listed;
  } catch (caught) {
    if (loadId !== nextLoadId) return;
    // If worktree path doesn't exist, fall back to repo root
    if (!props.taskDirectoryLoader && props.repoRoot && props.repoRoot !== props.worktreePath) {
      try {
        const listed = await invoke<string[]>("list_files", {
          path: props.repoRoot,
        });
        if (loadId !== nextLoadId) return;
        files.value = listed;
        useToast().warning("Worktree missing — showing repo root");
        return;
      } catch (_fallback) {
        // Fall through to original error
      }
    }
    const message = caught instanceof Error ? caught.message : String(caught);
    error.value = message;
    console.error("Failed to list files:", caught);
  } finally {
    if (loadId === nextLoadId) loading.value = false;
  }
}

function selectFile(filePath: string) {
  if (loading.value || error.value) return;
  emit("select", filePath, props.sourceKey);
}

function handleKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") {
    e.preventDefault();
    emit("close");
  } else if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
    e.preventDefault();
    e.stopPropagation();
    selectedIndex.value = Math.min(selectedIndex.value + 1, filtered.value.length - 1);
  } else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) {
    e.preventDefault();
    e.stopPropagation();
    selectedIndex.value = Math.max(selectedIndex.value - 1, 0);
  } else if (e.key === "Enter") {
    e.preventDefault();
    const entry = filtered.value[selectedIndex.value];
    if (entry) selectFile(entry.path);
  }
}

interface HighlightSegment {
  text: string;
  highlight: boolean;
}

function highlightPath(entry: ScoredFile): HighlightSegment[] {
  const { path, result } = entry;
  if (result.indices.length === 0) return [{ text: path, highlight: false }];

  const matchSet = new Set(result.indices);
  const segments: HighlightSegment[] = [];
  let current = "";
  let inMatch = false;

  for (let i = 0; i < path.length; i++) {
    const isMatch = matchSet.has(i);
    if (isMatch !== inMatch) {
      if (current) segments.push({ text: current, highlight: inMatch });
      current = "";
      inMatch = isMatch;
    }
    current += path[i];
  }
  if (current) segments.push({ text: current, highlight: inMatch });
  return segments;
}

// Reset selection when query changes
watch(query, () => { selectedIndex.value = 0; });

// The picker is kept mounted while hidden, so `onMounted` alone leaves it
// showing whatever it listed the first time it opened — another repo's files,
// or nothing at all when that first load pointed at a path that had gone away.
watch(
  () => [props.sourceKey, props.worktreePath, props.repoRoot, props.taskDirectoryLoader],
  () => {
    query.value = "";
    selectedIndex.value = 0;
    void loadFiles();
  },
  { immediate: true },
);

onMounted(async () => {
  await nextTick();
  inputRef.value?.focus();
});
</script>

<template>
  <div class="modal-overlay" :style="{ zIndex }" @click.self="emit('close')" @keydown="handleKeydown" @mousemove.once="mouseMoved = true">
    <div class="picker-modal">
      <input
        ref="inputRef"
        v-model="query"
        v-bind="macOsTextInputAttrs"
        type="text"
        class="picker-input"
        :placeholder="$t('filePicker.placeholder')"
      />
      <div class="file-list">
        <div
          v-for="(entry, i) in filtered"
          :key="entry.path"
          class="file-item"
          :class="{ selected: i === selectedIndex }"
          @click="selectFile(entry.path)"
          @mouseenter="mouseMoved && (selectedIndex = i)"
        >
          <template v-for="(segment, si) in highlightPath(entry)" :key="si">
            <span v-if="segment.highlight" class="match">{{ segment.text }}</span>
            <template v-else>{{ segment.text }}</template>
          </template>
        </div>
        <div v-if="loading" class="empty" data-testid="file-picker-loading">{{ $t('filePicker.loading') }}</div>
        <div v-else-if="error" class="empty error" data-testid="file-picker-unavailable">
          {{ $t('filePicker.unavailable', { message: error }) }}
        </div>
        <div v-else-if="filtered.length === 0" class="empty">{{ $t('filePicker.noFiles') }}</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.modal-overlay {
  position: fixed;
  inset: 0;
  background: var(--kn-overlay-scrim);
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: 15vh;
}
.picker-modal {
  background: var(--kn-bg-panel);
  border: 1px solid var(--kn-border-strong);
  border-radius: 8px;
  width: 550px;
  max-width: 90vw;
  overflow: hidden;
}
.picker-input {
  width: 100%;
  padding: 10px 14px;
  background: var(--kn-bg-input);
  border: none;
  border-bottom: 1px solid var(--kn-border-default);
  color: var(--kn-text-primary);
  font-size: 14px;
  outline: none;
}
.file-list {
  max-height: 400px;
  overflow-y: auto;
}
.file-item {
  padding: 6px 14px;
  font-size: 13px;
  font-family: "JetBrains Mono", "SF Mono", Menlo, monospace;
  color: var(--kn-text-secondary);
  cursor: pointer;
}
.file-item.selected {
  background: var(--kn-accent);
  color: var(--kn-text-inverse);
}
.file-item:hover {
  background: var(--kn-bg-hover);
}
.file-item.selected:hover {
  background: var(--kn-accent);
}
.match {
  color: var(--kn-warning);
  font-weight: 600;
}
.file-item.selected .match {
  color: var(--kn-warning);
}
.empty {
  padding: 16px;
  color: var(--kn-text-muted);
  text-align: center;
  font-size: 13px;
}
.empty.error {
  color: var(--kn-danger);
}
</style>
