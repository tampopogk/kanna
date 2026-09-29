<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { useThemeRuntime } from "../../theme/runtime";
import "./design-surface.css";

/**
 * An App Design task's stage chain, at the top of the design surface above
 * the artifact (docs/specs/app-design.md §4): the workflow's name, then its design positions, clickable in any
 * order (⇄) while designing; once approved for build, "⇢ Software factory".
 * As in the design prototype, a position is where the design is, inside one
 * stage: moving starts no session and forks nothing.
 */
const props = defineProps<{
  design: {
    inDesignStage: boolean;
    status?: string;
    position: string;
    positions: Array<{ name: string; label: string }>;
  };
  /** Persist the app-wide theme choice (the same setting Preferences uses). */
  setAppTheme?: (theme: "light" | "dark") => void;
}>();
const emit = defineEmits<{ pick: [position: string] }>();
const { t } = useI18n();
const { effectiveAppTheme } = useThemeRuntime();
const theme = computed<"light" | "dark">(() => (effectiveAppTheme.value === "light" ? "light" : "dark"));

const handedOff = computed(() => !props.design.inDesignStage || props.design.status === "handed_off");
</script>

<template>
  <div class="kd ladder" data-testid="task-header-design">
    <span class="wf">{{ t("design.workflowName") }}</span>
    <template v-for="(position, index) in design.positions" :key="position.name">
      <span v-if="index > 0" class="arrow" :title="t('design.freeMove')">⇄</span>
      <button
        type="button"
        class="step"
        :class="{ done: handedOff, now: !handedOff && position.name === design.position }"
        :disabled="handedOff"
        :aria-current="!handedOff && position.name === design.position ? 'step' : undefined"
        :title="handedOff ? t('design.handedOffStep') : t('design.goTo', { position: position.label })"
        :data-testid="`design-position-${position.name}`"
        @mousedown.stop
        @click="emit('pick', position.name)"
      >
        {{ position.label }}
      </button>
    </template>
    <template v-if="handedOff">
      <span class="arrow">⇢</span>
      <span class="step factory now">{{ t("design.softwareFactory") }}</span>
    </template>
    <span class="spacer" />
    <button
      v-if="setAppTheme"
      type="button"
      class="theme-toggle"
      :aria-label="theme === 'dark' ? t('design.lightTheme') : t('design.darkTheme')"
      :title="theme === 'dark' ? t('design.lightTheme') : t('design.darkTheme')"
      @mousedown.stop
      @click="setAppTheme(theme === 'dark' ? 'light' : 'dark')"
    >
      {{ theme === "dark" ? "☀" : "☾" }}
    </button>
  </div>
</template>

<style scoped>
.ladder {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  flex-wrap: wrap;
  padding: 8px 12px;
  background: var(--kd-panel);
  border-bottom: 1px solid var(--kd-line);
}
.wf {
  font-size: 10.5px;
  font-weight: 700;
  color: var(--kd-muted);
  background: var(--kd-panel);
  border: 1px solid var(--kd-line);
  border-radius: 6px;
  padding: 3px 7px;
  margin-right: 4px;
}
.step {
  border: 0;
  font: inherit;
  font-size: 12px;
  padding: 4px 10px;
  border-radius: 14px;
  background: var(--kd-hover);
  color: var(--kd-muted);
  cursor: pointer;
}
.step:disabled {
  cursor: default;
}
.step:not(.now):not(:disabled):hover {
  background: var(--kd-accent-soft);
  color: var(--kd-accent-ink);
}
.step.done {
  background: var(--kd-accent-soft);
  color: var(--kd-accent-ink);
}
.step.now {
  background: var(--kd-accent);
  color: #fff;
  font-weight: 600;
}
.step.factory {
  background: var(--kd-ok-soft);
  color: var(--kd-ok);
}
.step.factory.now {
  background: var(--kd-ok);
  color: #fff;
}
.arrow {
  color: var(--kd-muted);
  opacity: 0.7;
}
.spacer {
  flex: 1;
}
.theme-toggle {
  border: 1px solid var(--kd-line);
  background: var(--kd-panel);
  color: var(--kd-ink);
  border-radius: 999px;
  width: 30px;
  height: 26px;
  cursor: pointer;
}
.theme-toggle:hover {
  background: var(--kd-hover);
}
</style>
