import { createApp, ref, type App, type Ref } from "vue";

import i18n from "./i18n";
import StartupScreen from "./components/StartupScreen.vue";

/**
 * The startup screen exists before `App.vue` mounts, because the first
 * server-backed read this window performs — its saved window settings —
 * happens inside `main.ts` while nothing is on screen yet.
 *
 * Its phases name only work this window is actually waiting on. Nothing here
 * observes the daemon, migrations, authentication or the relay: those neither
 * define nor gate local workspace readiness, and claiming them would be
 * describing a boot sequence rather than reporting one.
 */
export type StartupPhase = "preparing" | "services" | "restoring" | "ready" | "failed";

/** The phases a startup failure can be attributed to. */
export type StartupWaitPhase = Extract<StartupPhase, "preparing" | "services" | "restoring">;

export interface StartupState {
  phase: Ref<StartupPhase>;
  /** Sentence explaining a real failure. Never a retry offer: there is no
   * operation to retry, so the screen asks for a restart instead. */
  failureDetail: Ref<string | null>;
  /** True once the wait has run long enough to be worth explaining. */
  longWait: Ref<boolean>;
}

export interface StartupController {
  readonly state: StartupState;
  /** True while the screen is covering the workspace. */
  readonly active: Ref<boolean>;
  readonly phase: Ref<StartupPhase>;
  /** Advance to a later wait phase. Ignored once startup has settled. */
  enterPhase(phase: StartupWaitPhase): void;
  /** Stop the screen on a real failure. Ignored once startup has settled. */
  fail(detail: string, cause?: unknown): void;
  /** Release the screen: the local workspace is usable. */
  markReady(): void;
  /** Tear the screen down without claiming readiness. */
  dispose(): void;
}

/** Milliseconds before the screen adds a stationary explanation of the wait.
 * Long enough that an ordinary launch never reaches it. */
export const STARTUP_LONG_WAIT_MS = 15_000;

export function createStartupState(): StartupState {
  return {
    phase: ref<StartupPhase>("preparing"),
    failureDetail: ref<string | null>(null),
    longWait: ref(false),
  };
}

export interface CreateStartupScreenOptions {
  /** Where to mount the screen. Omitted in tests and in any window that has no
   * startup host element, which leaves the controller headless. */
  target?: Element | null;
}

export function createStartupScreen(
  options: CreateStartupScreenOptions = {},
): StartupController {
  const state = createStartupState();
  const active = ref(true);
  let settled = false;
  let screen: App<Element> | null = null;

  const longWaitTimer = setTimeout(() => {
    if (settled) return;
    state.longWait.value = true;
  }, STARTUP_LONG_WAIT_MS);

  function teardown() {
    clearTimeout(longWaitTimer);
    active.value = false;
    const mounted = screen;
    screen = null;
    try {
      mounted?.unmount();
    } catch (error: unknown) {
      console.error("[startup] failed to unmount the startup screen:", error);
    }
  }

  if (options.target) {
    screen = createApp(StartupScreen, { state });
    screen.use(i18n);
    screen.mount(options.target);
  }

  return {
    state,
    active,
    phase: state.phase,
    enterPhase(phase: StartupWaitPhase) {
      if (settled) return;
      state.phase.value = phase;
    },
    fail(detail: string, cause?: unknown) {
      if (cause !== undefined) {
        console.error(`[startup] ${detail}`, cause);
      }
      // A failure in optional work that runs after readiness must not put a
      // whole-window error screen back over a usable workspace.
      if (settled) return;
      settled = true;
      clearTimeout(longWaitTimer);
      state.longWait.value = false;
      state.failureDetail.value = detail;
      state.phase.value = "failed";
    },
    markReady() {
      if (settled) return;
      settled = true;
      state.phase.value = "ready";
      teardown();
    },
    dispose() {
      settled = true;
      teardown();
    },
  };
}
