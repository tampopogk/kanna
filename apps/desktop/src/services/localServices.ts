import { readonly, ref, type Ref } from "vue";

import { ensureDesktopReady } from "./desktopServerClient";

/**
 * This window's view of its own local services — the daemon handoff plus a
 * `kanna-server` that actually answers.
 *
 * It exists because an unresponsive local server used to be a *fatal* startup
 * condition. `main.ts` reached its first server-backed read (the saved window
 * workspace) with nothing confirming the server was up, that read retried for
 * 15 seconds and then threw, and the window died behind a screen whose only
 * advice was to quit and reopen. Losing a saved window layout is not worth
 * refusing to start, and neither is a server that is thirty seconds late.
 *
 * So the wait is a wait: it retries for as long as it takes, a window that has
 * waited out its grace period comes up degraded and says so, and readiness
 * lands on its own whenever the server answers. Nothing here papers over a
 * dead server — `unavailable` is a state the app reports on screen.
 *
 * `unavailable` is the *window's* verdict — "I stopped waiting and local
 * services are not ready" — not "the last attempt threw". Those are different
 * facts and conflating them hid a whole failure shape: a sidecar that spawns
 * but never binds its port leaves `ensure_desktop_ready` pending for its full
 * status budget, so nothing rejects, and a window that reported only thrown
 * errors sat there uncovered and silent for the difference. The thrown error,
 * when there is one, is `localServicesFailure()`.
 */
export type LocalServicesState = "pending" | "ready" | "unavailable";

/**
 * How long a window covers its workspace waiting for local services before it
 * comes up degraded instead. A healthy launch is far inside this; a launch
 * that is not has nothing more to gain from a blank screen.
 */
export const LOCAL_SERVICES_STARTUP_GRACE_MS = 15_000;

/** Delay before the first retry. `ensureDesktopReady` already owns a 30s
 * native budget of its own, so this only paces the attempts after it gives up. */
const LOCAL_SERVICES_RETRY_DELAY_MS = 1_000;

/** Ceiling for the backoff. Some failures are terminal for this app process —
 * a daemon whose startup failed publishes that verdict once and nothing
 * republishes readiness — so the retry has to be able to run for the window's
 * whole life without becoming a busy loop or a log flood. It is also the worst
 * case for how long a recovered server waits to be noticed, which is why the
 * ceiling is seconds rather than minutes. */
const LOCAL_SERVICES_MAX_RETRY_DELAY_MS = 5_000;

const state = ref<LocalServicesState>("pending");
const lastFailure = ref<string | null>(null);
let attempt: Promise<void> | null = null;
/** Bumped only by the test reset below, which abandons the in-flight attempt.
 * A window never resets, so its one retry runs for the window's life. */
let attemptGeneration = 0;
let retryDelayMs = LOCAL_SERVICES_RETRY_DELAY_MS;
let startupGraceMs = LOCAL_SERVICES_STARTUP_GRACE_MS;
let startupGraceSpent = false;

const E2E_LOCAL_SERVICES_OUTAGE_KEY = "kanna.e2e.localServicesOutage";
let e2eOutageActive = false;
let releaseE2EOutage: (() => void) | null = null;

/** Hold the readiness attempt until the driver recovers it. */
function e2eOutageHold(): Promise<void> {
  return new Promise<void>((resolve) => {
    releaseE2EOutage = resolve;
  });
}

/**
 * DEV/E2E only. Simulates a `kanna-server` that is not answering for a whole
 * launch, so a driver can look at a degraded window in a real app and then let
 * it recover. The condition has to be installed before the first readiness
 * attempt — `main.ts` waits on it before anything is on screen — so it is
 * taken from `localStorage` at page load. One-shot: the flag is consumed as it
 * is read, so a driver that never recovers cannot wedge the next launch.
 *
 * It *hangs* rather than refusing, because that is the measured shape: a
 * sidecar that spawns but never binds keeps `ensure_desktop_ready` pending for
 * its whole status budget instead of rejecting. A seam that threw instantly
 * would exercise the one path this window was already reporting correctly.
 */
function installE2EOutage(): void {
  if (!import.meta.env.DEV) return;
  let held: string | null = null;
  try {
    held = window.localStorage.getItem(E2E_LOCAL_SERVICES_OUTAGE_KEY);
    if (held) window.localStorage.removeItem(E2E_LOCAL_SERVICES_OUTAGE_KEY);
  } catch (error: unknown) {
    console.debug("[localServices] E2E outage flag unreadable:", error);
    return;
  }
  if (!held) return;
  e2eOutageActive = true;
  window.__KANNA_E2E_LOCAL_SERVICES__ = {
    recover: () => {
      e2eOutageActive = false;
      releaseE2EOutage?.();
      releaseE2EOutage = null;
    },
  };
}

installE2EOutage();

async function sleep(ms: number): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, ms));
}

/** Read through a call so a check before an `await` cannot narrow the one after it. */
function isReady(): boolean {
  return state.value === "ready";
}

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return String(error);
}

/**
 * One shared retry, however many callers wait on it. It runs until local
 * services answer, so a window that stopped waiting is still served by it —
 * which is what lets a degraded window recover without a reload.
 */
function startAttempt(): Promise<void> {
  if (attempt) return attempt;
  const generation = attemptGeneration;
  const superseded = () => generation !== attemptGeneration;
  attempt = (async () => {
    let delayMs = retryDelayMs;
    while (!superseded()) {
      try {
        if (e2eOutageActive) await e2eOutageHold();
        if (superseded()) return;
        await ensureDesktopReady();
        if (superseded()) return;
        lastFailure.value = null;
        state.value = "ready";
        return;
      } catch (error: unknown) {
        if (superseded()) return;
        lastFailure.value = describe(error);
        state.value = "unavailable";
        console.warn("[localServices] local services are not ready; retrying:", error);
      }
      await sleep(delayMs);
      delayMs = Math.min(delayMs * 2, LOCAL_SERVICES_MAX_RETRY_DELAY_MS);
    }
  })();
  return attempt;
}

/**
 * Wait until local services answer, however long that takes. Never throws: an
 * unresponsive local server is a state the app reports, not a startup failure.
 */
export async function waitForLocalServices(): Promise<boolean> {
  if (isReady()) return true;
  await startAttempt();
  return isReady();
}

/**
 * Wait out this window's startup grace period and answer what is known then.
 * The retry keeps running either way, so a caller that gives up here is
 * choosing to come up degraded rather than abandoning readiness.
 *
 * The grace belongs to the *window*, not to the call: `main.ts` spends it
 * before it mounts anything and `useAppLifecycle` asks again after mounting,
 * and a window that has already waited it out must not sit through a second
 * one behind the same screen.
 */
export async function waitForLocalServicesStartupGrace(): Promise<boolean> {
  if (isReady()) return true;
  if (startupGraceSpent) return false;
  const ready = startAttempt();
  let graceTimer: ReturnType<typeof setTimeout> | undefined;
  const grace = new Promise<void>((resolve) => {
    graceTimer = setTimeout(resolve, startupGraceMs);
  });
  try {
    await Promise.race([ready, grace]);
  } finally {
    if (graceTimer !== undefined) clearTimeout(graceTimer);
  }
  if (!isReady()) {
    startupGraceSpent = true;
    // Not conditional on anything having thrown: the readiness call can still
    // be in flight here, and `lastFailure` still null. The window has stopped
    // covering its workspace either way, and what it shows has to say so.
    state.value = "unavailable";
  }
  return isReady();
}

/** This window's local-service state, for anything that has to show it. */
export function localServicesState(): Readonly<Ref<LocalServicesState>> {
  return readonly(state) as Readonly<Ref<LocalServicesState>>;
}

/** The last readiness error, so a caller can say what is actually wrong. */
export function localServicesFailure(): Readonly<Ref<string | null>> {
  return readonly(lastFailure) as Readonly<Ref<string | null>>;
}

export interface ResetLocalServicesForTestsOptions {
  /** Start already confirmed, which is what an ordinary launch reaches. */
  ready?: boolean;
  retryDelayMs?: number;
  startupGraceMs?: number;
}

export function resetLocalServicesForTests(
  options: ResetLocalServicesForTestsOptions = {},
): void {
  state.value = options.ready ? "ready" : "pending";
  lastFailure.value = null;
  // The previous attempt never ends on its own, and a loop left running across
  // tests writes this module's state underneath the next one.
  attemptGeneration += 1;
  attempt = null;
  retryDelayMs = options.retryDelayMs ?? LOCAL_SERVICES_RETRY_DELAY_MS;
  startupGraceMs = options.startupGraceMs ?? LOCAL_SERVICES_STARTUP_GRACE_MS;
  startupGraceSpent = false;
  e2eOutageActive = false;
  releaseE2EOutage?.();
  releaseE2EOutage = null;
}
