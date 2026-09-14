// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";

import { STARTUP_LONG_WAIT_MS, createStartupScreen } from "./startup";

function mountTarget(): HTMLElement {
  const target = document.createElement("div");
  document.body.appendChild(target);
  return target;
}

describe("startup screen controller", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("starts covering the window in the preparing phase", () => {
    const startup = createStartupScreen();

    expect(startup.phase.value).toBe("preparing");
    expect(startup.active.value).toBe(true);

    startup.dispose();
  });

  it("advances through the phases the window actually waits on", () => {
    const startup = createStartupScreen();

    startup.enterPhase("services");
    expect(startup.phase.value).toBe("services");
    startup.enterPhase("restoring");
    expect(startup.phase.value).toBe("restoring");

    startup.dispose();
  });

  it("releases the window when the workspace is ready", () => {
    const startup = createStartupScreen();

    startup.markReady();

    expect(startup.phase.value).toBe("ready");
    expect(startup.active.value).toBe(false);
  });

  it("keeps covering the window on a real failure and explains it", () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const startup = createStartupScreen();

    startup.fail("Kanna could not restore your workspace.", new Error("boom"));

    expect(startup.phase.value).toBe("failed");
    expect(startup.active.value).toBe(true);
    expect(startup.state.failureDetail.value).toBe("Kanna could not restore your workspace.");
    expect(errorSpy).toHaveBeenCalled();

    startup.dispose();
    errorSpy.mockRestore();
  });

  it("does not let work that runs after readiness put the screen back", () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const startup = createStartupScreen();
    startup.markReady();

    startup.fail("Kanna could not restore your workspace.", new Error("late"));
    startup.enterPhase("restoring");

    expect(startup.phase.value).toBe("ready");
    expect(startup.active.value).toBe(false);
    expect(startup.state.failureDetail.value).toBeNull();
    // The cause is still reported; only the screen refuses to come back.
    expect(errorSpy).toHaveBeenCalled();

    errorSpy.mockRestore();
  });

  it("does not let a later success clear a recorded failure", () => {
    const startup = createStartupScreen();
    startup.fail("Kanna could not restore your workspace.");

    startup.markReady();

    expect(startup.phase.value).toBe("failed");
    expect(startup.active.value).toBe(true);

    startup.dispose();
  });

  it("disposes without claiming the workspace became ready", () => {
    const startup = createStartupScreen();

    startup.dispose();

    expect(startup.phase.value).toBe("preparing");
    expect(startup.active.value).toBe(false);
  });

  it("explains a wait that runs long, and only while it is still waiting", () => {
    vi.useFakeTimers();
    const waiting = createStartupScreen();
    const released = createStartupScreen();
    released.markReady();

    vi.advanceTimersByTime(STARTUP_LONG_WAIT_MS);

    expect(waiting.state.longWait.value).toBe(true);
    expect(released.state.longWait.value).toBe(false);

    waiting.dispose();
  });

  it("clears the long wait explanation when the wait ends in failure", () => {
    vi.useFakeTimers();
    const startup = createStartupScreen();
    vi.advanceTimersByTime(STARTUP_LONG_WAIT_MS);
    expect(startup.state.longWait.value).toBe(true);

    startup.fail("Kanna could not restore your workspace.");

    expect(startup.state.longWait.value).toBe(false);

    startup.dispose();
  });

  it("mounts the screen into its host element and removes it on readiness", async () => {
    const target = mountTarget();
    const startup = createStartupScreen({ target });
    await nextTick();

    expect(target.querySelector('[data-testid="startup-screen"]')).not.toBeNull();
    expect(target.querySelector('[data-testid="startup-status"]')?.textContent?.trim()).toBe(
      "Starting Kanna…",
    );

    startup.markReady();
    await nextTick();

    expect(target.querySelector('[data-testid="startup-screen"]')).toBeNull();
  });

  it("keeps the same icon element across phase changes so the flow never restarts", async () => {
    const target = mountTarget();
    const startup = createStartupScreen({ target });
    await nextTick();
    const icon = target.querySelector('[data-testid="startup-icon-flow"]');
    expect(icon).not.toBeNull();

    startup.enterPhase("services");
    await nextTick();
    startup.enterPhase("restoring");
    await nextTick();

    expect(target.querySelector('[data-testid="startup-icon-flow"]')).toBe(icon);
    expect(target.querySelector('[data-testid="startup-status"]')?.textContent?.trim()).toBe(
      "Restoring your workspace…",
    );

    startup.dispose();
  });

  it("removes the screen immediately, without waiting out an animation cycle", async () => {
    vi.useFakeTimers();
    const target = mountTarget();
    const startup = createStartupScreen({ target });
    await nextTick();

    startup.markReady();
    await nextTick();

    // No timer advance: readiness must not be queued behind an exit animation.
    expect(target.querySelector('[data-testid="startup-screen"]')).toBeNull();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("mounts nothing when the window has no host element", () => {
    const startup = createStartupScreen({ target: null });

    expect(document.body.innerHTML).toBe("");

    startup.dispose();
  });
});
