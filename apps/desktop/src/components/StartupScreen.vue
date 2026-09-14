<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { StartupState } from "../startup";

const { state } = defineProps<{ state: StartupState }>();
const { t } = useI18n();

/**
 * The app icon's own geometry, taken from `src-tauri/icons/icon.svg`. The same
 * capsules clip the animated colour field and render the static icon, so the
 * two can never drift apart.
 */
interface Capsule {
  x: number;
  y: number;
  w: number;
  h: number;
  rx: number;
  row: number;
  flat?: string;
}

const CAPSULES: Capsule[] = [
  { x: 113, y: 82, w: 45, h: 34, rx: 17, row: 0 },
  { x: 180, y: 82, w: 174, h: 34, rx: 17, row: 0 },
  { x: 377, y: 82, w: 24, h: 34, rx: 12, row: 0, flat: "#17d34c" },
  { x: 113, y: 161, w: 45, h: 34, rx: 17, row: 1 },
  { x: 180, y: 161, w: 113, h: 34, rx: 17, row: 1 },
  { x: 113, y: 240, w: 45, h: 33, rx: 16.5, row: 2 },
  { x: 180, y: 240, w: 46, h: 33, rx: 16.5, row: 2 },
  { x: 113, y: 318, w: 45, h: 33, rx: 16.5, row: 3 },
  { x: 180, y: 318, w: 47, h: 33, rx: 16.5, row: 3 },
  { x: 245, y: 318, w: 48, h: 33, rx: 16.5, row: 3 },
  { x: 113, y: 397, w: 45, h: 33, rx: 16.5, row: 4 },
  { x: 180, y: 397, w: 47, h: 33, rx: 16.5, row: 4 },
  { x: 245, y: 397, w: 48, h: 33, rx: 16.5, row: 4 },
  { x: 313, y: 397, w: 47, h: 33, rx: 16.5, row: 4 },
];

interface IconRow {
  /** Vertical centre of the row in icon user units. */
  center: number;
  left: string;
  right: string;
  /** x where the row's own horizontal gradient reaches `right`. */
  end: number;
}

const ROWS: IconRow[] = [
  { center: 99, left: "#f72b89", right: "#ff8a12", end: 354 },
  { center: 178, left: "#df1fb7", right: "#ef2d9b", end: 293 },
  { center: 256, left: "#9630dc", right: "#9e23dd", end: 226 },
  { center: 335, left: "#1957c3", right: "#6940a4", end: 293 },
  { center: 414, left: "#1458c5", right: "#736b91", end: 360 },
];

const GREEN = "#17d34c";
/**
 * The mark's own bounds, so the startup screen shows the stylized K alone. The
 * app icon's rounded tile is deliberately not drawn: it is the shape macOS puts
 * behind the mark on a home screen, and painting it here puts a white box on
 * the app background.
 */
const MARK_VIEW_BOX = "113 82 288 348";
const FIELD_LEFT = 113;
const FIELD_RIGHT = 397;
/** One x sample per 4 user units; each strip is widened to overlap its neighbour. */
const SAMPLE_STEP = 4;
const STRIP_WIDTH = 8.02;
/** Distance between row centres times the five rows: the field's repeat period. */
const FIELD_PERIOD = 393.125;
const FIELD_TOP = ROWS[0].center;
const FLOW_DURATION_MS = 2875;

function lerpChannel(from: number, to: number, t: number): number {
  return Math.round(from + (to - from) * t);
}

function lerpHex(from: string, to: string, t: number): string {
  const a = Number.parseInt(from.slice(1), 16);
  const b = Number.parseInt(to.slice(1), 16);
  const r = lerpChannel((a >> 16) & 0xff, (b >> 16) & 0xff, t);
  const g = lerpChannel((a >> 8) & 0xff, (b >> 8) & 0xff, t);
  const blue = lerpChannel(a & 0xff, b & 0xff, t);
  return `#${((r << 16) | (g << 8) | blue).toString(16).padStart(6, "0")}`;
}

function sampleRow(row: IconRow, sampleAt: number): string {
  const span = row.end - FIELD_LEFT;
  const t = Math.min(Math.max((sampleAt - FIELD_LEFT) / span, 0), 1);
  return lerpHex(row.left, row.right, t);
}

// Unique per instance: two icons on one page must not share gradient ids.
const iconId = `kn-startup-${Math.random().toString(36).slice(2, 10)}`;
const clipId = `${iconId}-clip`;
const rowGradientId = (row: number) => `${iconId}-row-${row}`;
const stripGradientId = (index: number) => `${iconId}-strip-${index}`;

interface Strip {
  id: string;
  x: number;
  stops: string[];
}

/**
 * The moving colour field. Each strip carries one vertical gradient whose six
 * stops are that column's colour in each of the five rows, with the first
 * colour repeated so the repeating gradient closes on itself. Translating the
 * whole field by exactly one period is therefore seamless.
 */
const strips = computed<Strip[]>(() => {
  const result: Strip[] = [];
  let index = 0;
  for (let x = FIELD_LEFT; x <= FIELD_RIGHT; x += SAMPLE_STEP) {
    const sampleAt = x + SAMPLE_STEP / 2;
    const stops = ROWS.map((row, rowIndex) =>
      rowIndex === 0 && sampleAt >= 377 ? GREEN : sampleRow(row, sampleAt),
    );
    stops.push(stops[0]);
    result.push({ id: stripGradientId(index), x, stops });
    index += 1;
  }
  return result;
});

const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";
// Read during setup, not on mount: a reduced-motion window must never paint a
// first animated frame before the preference is applied.
const reducedMotion = ref(
  typeof window.matchMedia === "function" && window.matchMedia(REDUCED_MOTION_QUERY).matches,
);
let reducedMotionQuery: MediaQueryList | null = null;
function syncReducedMotion(event: MediaQueryList | MediaQueryListEvent) {
  reducedMotion.value = event.matches;
}

onMounted(() => {
  if (typeof window.matchMedia !== "function") return;
  reducedMotionQuery = window.matchMedia(REDUCED_MOTION_QUERY);
  syncReducedMotion(reducedMotionQuery);
  reducedMotionQuery.addEventListener("change", syncReducedMotion);
});

onBeforeUnmount(() => {
  reducedMotionQuery?.removeEventListener("change", syncReducedMotion);
  reducedMotionQuery = null;
});

const failed = computed(() => state.phase.value === "failed");
/** A stopped animation would freeze the field mid-row, so failure and reduced
 * motion both fall back to the icon's own unanimated artwork. */
const animated = computed(() => !failed.value && !reducedMotion.value);

const statusText = computed(() => {
  switch (state.phase.value) {
    case "services":
      return t("startup.services");
    case "restoring":
      return t("startup.restoring");
    default:
      return t("startup.preparing");
  }
});

const failureExplanation = computed(() => state.failureDetail.value ?? t("startup.failedGeneric"));
const flowStyle = computed(() => ({
  animationDuration: `${FLOW_DURATION_MS}ms`,
  "--kn-startup-flow-distance": `${FIELD_PERIOD}px`,
}));
</script>

<template>
  <div class="kn-startup" data-testid="startup-screen" :data-phase="state.phase.value">
    <svg
      class="kn-startup__icon"
      :class="{ 'kn-startup__icon--static': !animated }"
      :viewBox="MARK_VIEW_BOX"
      aria-hidden="true"
      focusable="false"
      data-testid="startup-icon"
    >
      <defs>
        <clipPath :id="clipId">
          <rect
            v-for="capsule in CAPSULES"
            :key="`clip-${capsule.x}-${capsule.y}`"
            :x="capsule.x"
            :y="capsule.y"
            :width="capsule.w"
            :height="capsule.h"
            :rx="capsule.rx"
          />
        </clipPath>
        <template v-if="animated">
          <linearGradient
            v-for="strip in strips"
            :key="strip.id"
            :id="strip.id"
            x1="0"
            :y1="FIELD_TOP"
            x2="0"
            :y2="FIELD_TOP + FIELD_PERIOD"
            gradientUnits="userSpaceOnUse"
            spreadMethod="repeat"
          >
            <stop
              v-for="(color, stopIndex) in strip.stops"
              :key="stopIndex"
              :offset="stopIndex / (strip.stops.length - 1)"
              :stop-color="color"
            />
          </linearGradient>
        </template>
        <template v-else>
          <linearGradient
            v-for="(row, rowIndex) in ROWS"
            :key="rowGradientId(rowIndex)"
            :id="rowGradientId(rowIndex)"
            :x1="FIELD_LEFT"
            :y1="row.center"
            :x2="row.end"
            :y2="row.center"
            gradientUnits="userSpaceOnUse"
          >
            <stop :stop-color="row.left" />
            <stop offset="1" :stop-color="row.right" />
          </linearGradient>
        </template>
      </defs>

      <g v-if="animated" :clip-path="`url(#${clipId})`">
        <g class="kn-startup__flow" :style="flowStyle" data-testid="startup-icon-flow">
          <rect
            v-for="strip in strips"
            :key="`strip-${strip.id}`"
            :x="strip.x"
            y="-512"
            :width="STRIP_WIDTH"
            height="1536"
            :fill="`url(#${strip.id})`"
          />
        </g>
      </g>
      <template v-else>
        <rect
          v-for="capsule in CAPSULES"
          :key="`static-${capsule.x}-${capsule.y}`"
          :x="capsule.x"
          :y="capsule.y"
          :width="capsule.w"
          :height="capsule.h"
          :rx="capsule.rx"
          :fill="capsule.flat ?? `url(#${rowGradientId(capsule.row)})`"
        />
      </template>
    </svg>

    <div v-if="failed" class="kn-startup__failure" role="alert" data-testid="startup-failure">
      <h1 class="kn-startup__title">{{ t("startup.failedTitle") }}</h1>
      <p class="kn-startup__detail">{{ failureExplanation }}</p>
      <p class="kn-startup__detail">{{ t("startup.failedRestart") }}</p>
    </div>
    <div v-else class="kn-startup__status-region">
      <p class="kn-startup__status" role="status" data-testid="startup-status">
        {{ statusText }}
      </p>
      <p
        v-if="state.longWait.value"
        class="kn-startup__hint"
        data-testid="startup-long-wait-hint"
      >
        {{ t("startup.longWait") }}
      </p>
    </div>
  </div>
</template>

<style scoped>
.kn-startup {
  position: fixed;
  inset: 0;
  z-index: 2000;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 22px;
  background: var(--kn-bg-app, #1a1a1a);
  color: var(--kn-text-primary, #e0e0e0);
  -webkit-user-select: none;
  user-select: none;
}

.kn-startup__icon {
  width: 93px;
  height: 112px;
  flex: none;
}

.kn-startup__flow {
  animation-name: kn-startup-flow;
  animation-timing-function: linear;
  animation-iteration-count: infinite;
}

@keyframes kn-startup-flow {
  from {
    transform: translateY(0);
  }
  to {
    transform: translateY(var(--kn-startup-flow-distance));
  }
}

.kn-startup__status-region {
  /* Fixed region so a longer phase name never nudges the icon. */
  min-height: 40px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  text-align: center;
  padding: 0 24px;
}

.kn-startup__status {
  font-size: 13px;
  color: var(--kn-text-secondary, #bbbbbb);
}

.kn-startup__hint {
  font-size: 12px;
  color: var(--kn-text-muted, #888888);
}

.kn-startup__failure {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  text-align: center;
  max-width: 420px;
  padding: 0 24px;
}

.kn-startup__title {
  font-size: 15px;
  font-weight: 600;
}

.kn-startup__detail {
  font-size: 13px;
  color: var(--kn-text-secondary, #bbbbbb);
}
</style>
