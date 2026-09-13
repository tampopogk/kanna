import { join, resolve } from "node:path";
import { mkdir, writeFile } from "node:fs/promises";
import { setTimeout as sleep } from "node:timers/promises";
import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { WebDriverClient } from "../helpers/webdriver";
import { importTestRepo, resetDatabase } from "../helpers/reset";
import { cleanupFixtureRepos, createSeedFixtureRepo } from "../helpers/fixture-repo";
import { execDb } from "../helpers/vue";
import { pauseForSlowMode } from "../helpers/slowMode";
import {
  assertNativeWindowIdentity,
  resolveExpectedNativeWindowIdentity,
} from "../helpers/windowIdentity";

describe("app launch", () => {
  const client = new WebDriverClient();
  const evidence = resolve("../../.tmp/startup-screen");

  beforeAll(async () => {
    await client.createSession({ dismissStartupShortcuts: false });
    // Nothing below may touch a window that is not this worktree's own build.
    await assertNativeWindowIdentity(
      client,
      await resolveExpectedNativeWindowIdentity(resolve("../..")),
      "app launch",
    );
    await mkdir(evidence, { recursive: true });
    await pauseForSlowMode("app-launch session created");
    await resetDatabase(client);
    await pauseForSlowMode("app-launch database reset");
    // Reload to get fresh UI after reset
    await client.executeSync("location.reload()");
    await sleep(1000);
    await pauseForSlowMode("app-launch UI reloaded");
  });

  afterAll(async () => {
    await client.deleteSession();
  });

  it("loads the frontend from this instance's dev server", async () => {
    // Tauri compiles `build.devUrl` into the binary, but each instance gets its own port
    // at launch. When a cached build keeps an earlier run's port the window is refused and
    // sits at about:blank — see apps/desktop/src-tauri/src/dev_url.rs.
    const devPort = process.env.KANNA_DEV_PORT;
    expect(devPort).toBeTruthy();
    const origin = await client.executeSync<string>("return location.origin;");
    expect(origin).toBe(`http://localhost:${devPort}`);
  });

  it("renders and screenshots without pulling the app into the foreground", async () => {
    // E2E runs launch real macOS apps. `KANNA_E2E_NO_ACTIVATE=1` — set for every
    // harness-launched instance — gives them a non-activating activation policy so a
    // run cannot take the operator's keyboard focus. An app that never activates is
    // only useful here if WebKit still paints it, so assert both halves.
    const screenshot = await client.screenshot();
    expect(Buffer.from(screenshot, "base64").byteLength).toBeGreaterThan(1024);
    if (process.env.KANNA_E2E_NO_ACTIVATE === "0") return;
    const hasFocus = await client.executeSync<boolean>("return document.hasFocus();");
    expect(hasFocus).toBe(false);
  });

  it("renders with title Kanna", async () => {
    await pauseForSlowMode("before title assertion");
    const title = await client.getTitle();
    expect(title).toBe("Kanna");
  });

  it("shows empty sidebar message", async () => {
    await pauseForSlowMode("before empty sidebar assertion");
    const el = await client.waitForText(".sidebar", "No repos yet.");
    expect(el).toBeTruthy();
  });

  it("shows onboarding guidance in main panel", async () => {
    await pauseForSlowMode("before onboarding guidance assertion");
    const shellShortcut = process.platform === "darwin" ? "⇧⌘J" : "Ctrl+Alt+J";
    const el = await client.waitForText(".main-panel", `Press ${shellShortcut} to open a shell`);
    expect(el).toBeTruthy();
  });

  it("shows detected agent CLI versions", async () => {
    await client.waitForText(".main-panel", "v0.125.0-beta.1+20260429");
    const bodyText = await client.executeSync<string>("return document.body.innerText;");
    expect(bodyText).toContain("Claude Code");
    expect(bodyText).toContain("v2.1.118");
    expect(bodyText).toContain("GitHub Copilot");
    expect(bodyText).toContain("v1.0.32");
    expect(bodyText).toContain("Codex");
    expect(bodyText).toContain("v0.125.0-beta.1+20260429");
  });

  it("shows repo creation shortcut hint for the platform running the app", async () => {
    await pauseForSlowMode("before repo creation hint assertion");
    const bodyText = await client.executeSync<string>("return document.body.innerText;");
    // Not a glyph substitution: a Linux keyboard has no Command key, and the
    // binding itself moves (see `composables/shortcutPlatform.ts`). Asking the
    // app which platform it thinks it is on would let a wrong answer pass, so
    // this asserts against the host's platform instead.
    const expected =
      process.platform === "darwin" ? "Press ⌘I to create one." : "Press Ctrl+Shift+I to create one.";
    expect(bodyText).toContain(expected);
  });

  it("shows keyboard shortcuts reference", async () => {
    await pauseForSlowMode("before keyboard shortcuts assertion");
    const bodyText = await client.executeSync<string>("return document.body.innerText;");
    expect(bodyText).toContain("Keyboard Shortcuts");
  });

  // The startup screen exists before the app mounts, so it cannot be reached
  // through the app's own state. These reload the real window with the
  // DEV-only local-service hold taken, look at what is actually painted, and
  // then release or fail it.
  describe("startup screen", () => {
    async function waitForPage<T>(
      script: string,
      accept: (value: T) => boolean,
      label: string,
      timeoutMs = 20000,
    ): Promise<T> {
      const deadline = Date.now() + timeoutMs;
      let last: T | undefined;
      while (Date.now() < deadline) {
        try {
          last = await client.executeSync<T>(script);
          if (accept(last)) return last;
        } catch {
          // The window is still navigating.
        }
        await sleep(150);
      }
      throw new Error(`${label} never happened; last value ${JSON.stringify(last)}`);
    }

    async function reloadHoldingLocalServices(): Promise<void> {
      await client.executeSync(
        `window.localStorage.setItem("kanna.e2e.startupHold", "1");
         delete window.__KANNA_E2E_STARTUP_HOLD__;
         location.reload();`,
      );
      await waitForPage<boolean>(
        "return Boolean(window.__KANNA_E2E_STARTUP_HOLD__);",
        (held) => held,
        "the held launch reached the local-service wait",
      );
    }

    const focusTaskId = "e2e-startup-focus";
    const focusBranch = `task-${focusTaskId}`;
    const TERMINAL_TEXTAREA = ".terminal-container .xterm-helper-textarea";
    let focusFixtureRepoPath = "";

    afterAll(async () => {
      await cleanupFixtureRepos(focusFixtureRepoPath ? [focusFixtureRepoPath] : []);
      // Leave the window in its ordinary started state for anything after this.
      await client.reload({ dismissStartupShortcuts: false });
      await resetDatabase(client);
    });

    /** Leaves a task with a live agent terminal selected in this window. */
    async function selectRestoredAgentTerminal(): Promise<void> {
      await resetDatabase(client);
      // The startup shortcuts modal would take the caret itself, and the
      // terminal's own focus rules deliberately yield to an open modal.
      await execDb(
        client,
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)",
        ["hideShortcutsOnStartup", "true"],
      );
      focusFixtureRepoPath = await createSeedFixtureRepo("task-switch-minimal");
      const repoId = await importTestRepo(client, focusFixtureRepoPath, "startup-focus-fixture");
      await mkdir(join(focusFixtureRepoPath, ".kanna-worktrees", focusBranch), { recursive: true });
      await execDb(
        client,
        `INSERT INTO pipeline_item
           (id, repo_id, prompt, display_name, stage, branch, agent_type, agent_provider, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
        [
          focusTaskId,
          repoId,
          "Keep a terminal on screen for the startup focus check",
          "Startup focus fixture",
          "in progress",
          focusBranch,
          "pty",
          "claude",
          "2026-09-12T10:00:00.000Z",
          "2026-09-12T10:00:00.000Z",
        ],
      );

      // Importing the fixture leaves its own setup task selected; keep asking
      // until this task owns the terminal view.
      const deadline = Date.now() + 45_000;
      while (Date.now() < deadline) {
        const selected = await client.executeAsync<string>(
          `const cb = arguments[arguments.length - 1];
           const ctx = window.__KANNA_E2E__.setupState;
           Promise.resolve(ctx.refreshAllItems())
             .then(function () { return ctx.store.selectRepo(${JSON.stringify(repoId)}); })
             .then(function () { return ctx.store.selectItem(${JSON.stringify(focusTaskId)}); })
             .then(function () { cb("ok"); })
             .catch(function (error) { cb("err:" + error); });`,
        );
        expect(selected).toBe("ok");
        const registered = await client.executeSync<boolean>(
          `return window.__KANNA_E2E__.terminalBuffers?.sessionIds().includes(${JSON.stringify(focusTaskId)}) === true;`,
        );
        if (registered) {
          await client.waitForElement(TERMINAL_TEXTAREA, 10_000);
          return;
        }
        await sleep(500);
      }
      throw new Error(`terminal buffer ${focusTaskId} was not registered`);
    }

    async function readTerminalFocus(): Promise<{ present: boolean; focused: boolean; inert: boolean }> {
      return await client.executeSync(`
        const textarea = document.querySelector(${JSON.stringify(TERMINAL_TEXTAREA)});
        if (!textarea) return { present: false, focused: false, inert: false };
        return {
          present: true,
          focused: document.activeElement === textarea,
          inert: Boolean(textarea.closest('[inert]')),
        };
      `);
    }

    it("covers the window with the animated app icon while local services start", async () => {
      await reloadHoldingLocalServices();

      expect(
        await client.executeSync<string>(
          `return document.querySelector('[data-testid="startup-status"]').textContent.trim();`,
        ),
      ).toBe("Starting local services…");

      // The app icon itself, clipped and flowing — not a spinner, and not a
      // percentage or an invented phase.
      const paint = await client.executeSync<{
        capsules: number;
        repeatingGradients: number;
        animationName: string;
        opacities: string[];
      }>(`
        const flow = document.querySelector('[data-testid="startup-icon-flow"]');
        const svg = document.querySelector('[data-testid="startup-icon"]');
        return {
          capsules: svg.querySelectorAll('clipPath rect').length,
          repeatingGradients: svg.querySelectorAll('linearGradient[spreadMethod="repeat"]').length,
          animationName: getComputedStyle(flow).animationName,
          opacities: [...flow.querySelectorAll('rect')].slice(0, 3)
            .map((rect) => getComputedStyle(rect).opacity),
        };
      `);
      expect(paint.capsules).toBe(14);
      expect(paint.repeatingGradients).toBeGreaterThan(1);
      // Vue's scoped styles suffix the keyframes name.
      expect(paint.animationName).toContain("kn-startup-flow");
      expect(paint.opacities.every((opacity) => Number(opacity) === 1)).toBe(true);

      const timing = await client.executeSync<{
        count: number;
        playState: string;
        duration: number;
        iterations: string;
      }>(`
        const animations = document.querySelector('[data-testid="startup-icon-flow"]').getAnimations();
        const timing = animations[0].effect.getComputedTiming();
        return {
          count: animations.length,
          playState: animations[0].playState,
          duration: Number(timing.duration),
          iterations: String(timing.iterations),
        };
      `);
      expect(timing.count).toBe(1);
      expect(timing.playState).toBe("running");
      expect(timing.duration).toBe(2875);
      expect(timing.iterations).toBe("Infinity");

      // An E2E window is deliberately never activated, so WebKit suspends its
      // animation clock and elapsed wall time paints nothing new. Driving the
      // animation's own time instead shows what each moment of the loop
      // actually renders — including that one full period lands back exactly
      // where it started, which is what makes the wrap seamless.
      const frameAt = async (currentTime: number, name: string) => {
        await client.executeSync(`
          const animation = document.querySelector('[data-testid="startup-icon-flow"]').getAnimations()[0];
          animation.pause();
          animation.currentTime = ${currentTime};
        `);
        await sleep(250);
        return await client.screenshot(resolve(evidence, name));
      };
      const atStart = await frameAt(0, "startup-flow-t0.png");
      const atMidLoop = await frameAt(1437.5, "startup-flow-half.png");
      const atWrap = await frameAt(2875, "startup-flow-wrap.png");
      expect(atMidLoop).not.toBe(atStart);
      expect(atWrap).toBe(atStart);
      await client.executeSync(`
        document.querySelector('[data-testid="startup-icon-flow"]').getAnimations()[0].play();
      `);

      // Autonomous motion. The frames above prove what each moment of the loop
      // renders; they cannot prove the clock runs by itself, because a
      // non-activating window has its animation clock suspended. Run with
      // KANNA_E2E_NO_ACTIVATE=0 to launch this instance with the ordinary
      // activation policy and observe it for real.
      const readCurrentTime = async () =>
        await client.executeSync<number>(`
          return Number(
            document.querySelector('[data-testid="startup-icon-flow"]').getAnimations()[0].currentTime,
          );
        `);
      if (process.env.KANNA_E2E_NO_ACTIVATE === "0") {
        // Activates this very process: the command reads NSApplication from the
        // app that owns this webview and refuses unless this instance was
        // launched with normal activation. It resolves no app name or bundle
        // id, so it cannot reach an installed Kanna. macOS may still decline
        // to make it the frontmost application — the reported pids say whether
        // it did, and what this check needs is the window's own document focus
        // and a running compositor, not NSApp frontmost-ness.
        const activation = await client.executeAsync<Record<string, unknown>>(
          `const done = arguments[arguments.length - 1];
           window.__TAURI_INTERNALS__.invoke("e2e_activate_current_app")
             .then(done, (error) => done({ error: String(error) }));`,
        );
        await sleep(500);

        const focused = await client.executeSync<boolean>("return document.hasFocus();");
        const waitedMs = 700;
        const before = await readCurrentTime();
        await sleep(waitedMs);
        const after = await readCurrentTime();
        await writeFile(
          resolve(evidence, "autonomous-motion.json"),
          `${JSON.stringify(
            { activation, focused, before, after, advancedMs: after - before, waitedMs },
            null,
            2,
          )}\n`,
        );

        expect(activation.error).toBeUndefined();
        expect(focused).toBe(true);
        // Nothing here touched the animation: its own clock moved on while the
        // test waited, which is the autonomous motion the frame captures above
        // cannot show. The *rate* is deliberately not asserted — macOS may
        // decline to bring this app to the front (the recorded pids say
        // whether it did), and WebKit throttles a window whose app is not
        // frontmost, so the elapsed animation time is real but reduced.
        expect(after).toBeGreaterThan(before);
      }

      // The screen paints before any saved theme is known, so it starts on the
      // current one and has to stay legible either way.
      await client.executeSync(`document.documentElement.dataset.theme = "light";`);
      await sleep(250);
      const lightFrame = await client.screenshot(resolve(evidence, "startup-light.png"));
      expect(lightFrame).not.toBe(atStart);
      await client.executeSync(`document.documentElement.dataset.theme = "dark";`);

      // This wait happens before the app mounts at all, which is the whole
      // reason the screen cannot live inside App.vue.
      expect(
        await client.executeSync<number>(`return document.getElementById('app').childElementCount;`),
      ).toBe(0);
      await client.screenshot(resolve(evidence, "startup-local-services.png"));

      await client.executeSync("window.__KANNA_E2E_STARTUP_HOLD__.release();");
      await client.waitForAppReady();

      expect(
        await client.executeSync<boolean>(
          `return Boolean(document.querySelector('[data-testid="startup-screen"]'));`,
        ),
      ).toBe(false);
      expect(
        await client.executeSync<boolean>(
          `return document.querySelector('.app').hasAttribute('inert');`,
        ),
      ).toBe(false);
      await client.screenshot(resolve(evidence, "startup-released-workspace.png"));
    });

    it("hands the caret to the restored terminal when the screen lifts", async () => {
      await selectRestoredAgentTerminal();

      // Hold the readiness edge, which sits after restoration has already
      // mounted the workspace, so the terminal makes its own first focus
      // attempt while the screen still covers it.
      await client.executeSync(
        `window.localStorage.setItem("kanna.e2e.readinessHold", "1");
         delete window.__KANNA_E2E_READINESS_HOLD__;
         location.reload();`,
      );
      await waitForPage<boolean>(
        "return Boolean(window.__KANNA_E2E_READINESS_HOLD__);",
        (held) => held,
        "the held launch reached the readiness edge",
      );
      // A reload is a new window: re-verify before touching it.
      await assertNativeWindowIdentity(
        client,
        await resolveExpectedNativeWindowIdentity(resolve("../..")),
        "app launch (held readiness)",
      );
      await waitForPage<boolean>(
        `return Boolean(document.querySelector(${JSON.stringify(TERMINAL_TEXTAREA)}));`,
        (present) => present,
        "the restored terminal mounted behind the startup screen",
      );
      // Its focus request runs through a tick, the native webview and an
      // animation frame; give it room to finish and fail on its own.
      await sleep(1500);

      const covered = await readTerminalFocus();
      expect(covered.present).toBe(true);
      // This is the half happy-dom cannot stand in for: a real browser refuses
      // focus through an inert ancestor, so the terminal's own attempt is lost.
      expect(covered.inert).toBe(true);
      expect(covered.focused).toBe(false);

      await client.executeSync("window.__KANNA_E2E_READINESS_HOLD__.release();");
      await client.waitForAppReady();

      const revealed = await waitForPage<{ present: boolean; focused: boolean; inert: boolean }>(
        `const textarea = document.querySelector(${JSON.stringify(TERMINAL_TEXTAREA)});
         if (!textarea) return { present: false, focused: false, inert: false };
         return {
           present: true,
           focused: document.activeElement === textarea,
           inert: Boolean(textarea.closest('[inert]')),
         };`,
        (state) => state.focused,
        "the revealed terminal took the caret without a click",
      );
      expect(revealed.inert).toBe(false);
      // Focused helper textarea is how xterm receives typing: the terminal
      // accepts input with no click and no activation change.
      expect(revealed.focused).toBe(true);
      await client.screenshot(resolve(evidence, "startup-terminal-focus.png"));
    });

    it("stops on a real startup failure and asks for a restart", async () => {
      await reloadHoldingLocalServices();
      await client.executeSync("window.__KANNA_E2E_STARTUP_HOLD__.fail();");

      const failure = await waitForPage<string>(
        `const el = document.querySelector('[data-testid="startup-failure"]');
         return el ? el.innerText : "";`,
        (text) => text.length > 0,
        "the startup failure surface appeared",
      );
      expect(failure).toContain("Kanna couldn’t start");
      expect(failure).toContain("Kanna could not start its local services.");
      expect(failure).toContain("Quit and reopen Kanna to try again.");

      const stopped = await client.executeSync<{ buttons: number; flow: boolean }>(`
        const screen = document.querySelector('[data-testid="startup-screen"]');
        return {
          buttons: screen.querySelectorAll('button').length,
          flow: Boolean(screen.querySelector('[data-testid="startup-icon-flow"]')),
        };
      `);
      // No invented retry action, and the animation is stopped rather than
      // frozen part-way through a row.
      expect(stopped.buttons).toBe(0);
      expect(stopped.flow).toBe(false);
      await client.screenshot(resolve(evidence, "startup-failure.png"));
    });
  });
});
