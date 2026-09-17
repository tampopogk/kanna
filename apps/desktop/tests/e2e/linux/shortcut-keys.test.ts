import { setTimeout as sleep } from "node:timers/promises";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { WebDriverClient } from "../helpers/webdriver";
import { cleanupFixtureRepos, createSeedFixtureRepo } from "../helpers/fixture-repo";
import { cleanupWorktrees, importTestRepo, resetDatabase } from "../helpers/reset";
import { callVueMethod, getVueState, tauriInvoke } from "../helpers/vue";
import {
  dismissDesktopGrab,
  focusNextWindow,
  inspectRealKeyboard,
  pressRealChord,
  type RealKeyboardStatus,
} from "../helpers/realKeys";
import {
  inspectRealClipboard,
  writeRealClipboard,
  type RealClipboardStatus,
} from "../helpers/realClipboard";
import {
  CLEAR_KEY_PROBE_SCRIPT,
  INSTALL_KEY_PROBE_SCRIPT,
  READ_KEY_PROBE_SCRIPT,
  REMOVE_KEY_PROBE_SCRIPT,
  matchesChord,
  type ObservedKey,
} from "../helpers/realKeyProbe";

/**
 * The Linux keymap, pressed for real.
 *
 * Every other keyboard suite in this tree builds a `KeyboardEvent` in the page
 * and dispatches it. That tests the app's handler wiring, and it was green
 * while three chords were dead on the owner's desktop: the keys never reached
 * the webview at all, because GNOME and IBus take theirs before any
 * application is asked. A mapping table cannot know that. So this lane injects
 * at `/dev/uinput` and lets each keystroke travel the whole chain — kernel,
 * compositor, input method, GTK, WebKit, the page.
 *
 * Two signals are recorded for every chord, because "nothing happened" has two
 * causes with two different fixes:
 *
 *   arrived — the probe saw the keydown at all. No means something above the
 *             app claimed it, and the binding has to move; no handler change
 *             can rescue it.
 *   claimed — the app's own handler called `preventDefault()`. No, with
 *             arrived yes, means the binding is simply wrong.
 *
 * The lane needs a Linux desktop session it can type into, and the window
 * under test must own the keyboard focus, because injected keys go to whatever
 * the *seat* is focused on and not to this process. Both are checked rather
 * than assumed: an unusable host fails with its reason, and never passes
 * quietly. See `README.md` beside this file for how to run it.
 */

const TAB_BAR = '[data-testid="main-tab-bar"]';
const SIDEBAR_SEARCH = ".sidebar .search-input";
const TERMINAL_ROWS = ".main-panel .xterm-rows > div";
const AGENT_COMPOSER = '[data-testid="agent-composer"]';

/**
 * The app has renamed its selection entry point before, and a helper that
 * silently missed it selects nothing while every later assertion still looks
 * plausible. Resolve it by trying each spelling, as the mock suites do.
 */
const SELECT_SIDEBAR_ITEM_SCRIPT = `
  function selectSidebarItem(ctx, id) {
    const select = ctx.selectSidebarItemById || ctx.handleSelectItem
      || (ctx.store && ctx.store.selectItem && ctx.store.selectItem.bind(ctx.store));
    if (!select) throw new Error("no sidebar selection entry point on setupState");
    return select(id);
  }
`;

interface ShortcutRow {
  action: string;
  keys: string;
}

interface Selection {
  start: number;
  end: number;
  value: string;
}

describe("Linux keyboard shortcuts, pressed for real", () => {
  const client = new WebDriverClient();
  let keyboard: RealKeyboardStatus = { usable: false, reason: "not inspected" };
  let clipboard: RealClipboardStatus = { usable: false, reason: "not inspected" };
  let fixtureRepoRoot = "";
  let testRepoPath = "";
  let taskId = "";

  /**
   * An unusable host stops the lane with its reason. A skip that reads like a
   * pass is exactly the failure this suite exists to remove: the unit tests
   * were green while the keymap was broken, and a green run from a machine
   * that never pressed a key would be the same lie one layer up.
   */
  function requireRealKeyboard(): void {
    if (!keyboard.usable) throw new Error(`no real key injection on this host: ${keyboard.reason}`);
  }

  /** Same rule for the clipboard: a paste verdict needs a real selection. */
  function requireRealClipboard(): void {
    if (!clipboard.usable) throw new Error(`no real clipboard on this host: ${clipboard.reason}`);
  }

  async function windowHasFocus(): Promise<boolean> {
    return client.executeSync<boolean>("return document.hasFocus();");
  }

  /**
   * Injected keys go to whichever window the seat has focused, so the window
   * under test has to be the one holding the keyboard before any chord means
   * anything. Nothing in the app can arrange that: a Wayland client may not
   * raise itself, and `set_focus` answers `ok` while changing nothing.
   *
   * Two things take the keyboard away, and each has its own move. The
   * desktop's own overview or a panel menu holds a grab no window can see
   * past — Escape ends it. Another window simply has focus — the desktop's
   * switcher walks past it. Both are only ever pressed while this window does
   * *not* have focus, so neither can reach anything the lane opened.
   */
  async function focusWindowUnderTest(): Promise<void> {
    if (await windowHasFocus()) return;
    await dismissDesktopGrab();
    await sleep(500);
    if (await windowHasFocus()) return;
    for (let attempt = 0; attempt < 8; attempt += 1) {
      await focusNextWindow();
      await sleep(700);
      if (await windowHasFocus()) return;
    }
    throw new Error(
      "the window under test never took keyboard focus, so injected keys would land elsewhere; " +
        "run this lane on a desktop session where it is the frontmost Kanna window",
    );
  }

  /** Press a chord for real, and report every matching keydown the page saw. */
  async function press(chord: string): Promise<ObservedKey[]> {
    await focusWindowUnderTest();
    await client.executeSync(`return ${CLEAR_KEY_PROBE_SCRIPT};`);
    await pressRealChord(chord);
    await sleep(350);
    const observed = (await client.executeSync<ObservedKey[] | null>(`return ${READ_KEY_PROBE_SCRIPT};`)) ?? [];
    return observed.filter((event) => matchesChord(event, chord));
  }

  async function expectArrived(chord: string): Promise<ObservedKey> {
    const matches = await press(chord);
    expect(
      matches.length,
      `${chord} never reached the webview. Something above the app — the compositor or the input ` +
        "method — claimed it, so this binding cannot be made to work and has to move.",
    ).toBeGreaterThan(0);
    return matches[0];
  }

  /**
   * Press a chord and require the app's own handler to have taken it.
   *
   * `preventDefault()` is called on the way into the action, so a claimed
   * chord is an action that ran — the difference between a binding that works
   * and one that is merely listed. The failure prints the whole keydown,
   * because "arrived but unclaimed" has several causes that look identical
   * from outside: a modifier the desktop rewrote, a key spelled differently
   * than the table expects, or a text field that was still focused and
   * conceded the chord.
   */
  async function expectClaimed(chord: string): Promise<ObservedKey> {
    const event = await expectArrived(chord);
    expect(
      event.defaultPrevented,
      `${chord} reached the webview and no handler claimed it — the keydown was ${JSON.stringify(event)}`,
    ).toBe(true);
    return event;
  }

  async function activeTabId(): Promise<string | null> {
    return client.executeSync<string | null>(
      `const active = document.querySelector('${TAB_BAR} [role="tab"][aria-selected="true"]');
       return active ? (active.getAttribute("data-testid") || "").replace(/^main-tab-/, "") : null;`,
    );
  }

  async function openTabIds(): Promise<string[]> {
    return client.executeSync<string[]>(
      `return Array.from(document.querySelectorAll('${TAB_BAR} [role="tab"]'))
        .map((tab) => (tab.getAttribute("data-testid") || "").replace(/^main-tab-/, ""));`,
    );
  }

  async function waitForActiveTab(id: string, timeoutMs = 10_000): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    let latest: string | null = null;
    while (Date.now() < deadline) {
      latest = await activeTabId();
      if (latest === id) return;
      await sleep(150);
    }
    throw new Error(`expected the active tab to be ${id}; it is ${latest} of ${JSON.stringify(await openTabIds())}`);
  }

  /**
   * Every row the shortcuts modal shows a person, read once in full mode.
   * What the modal lists is the claim the owner was testing against, so the
   * lane asserts against the same text rather than against the mapping table.
   */
  async function listedShortcuts(): Promise<ShortcutRow[]> {
    await callVueMethod(client, "keyboardActions.showAllShortcuts");
    await client.waitForElement(".shortcuts-modal", 8_000);
    const rows = await client.executeSync<ShortcutRow[]>(
      `return Array.from(document.querySelectorAll(".shortcuts-modal .shortcut-entry"))
        .filter((entry) => entry.querySelector(".shortcut-keys"))
        .map((entry) => ({
          action: entry.querySelector(".shortcut-action").textContent.trim(),
          keys: entry.querySelector(".shortcut-keys").textContent.replace(/\\s+/g, " ").trim(),
        }));`,
    );
    // Closed through the same action that opened it. `showShortcutsModal` is a
    // ref on `setupState`; assigning to the property would replace the ref with
    // a boolean and leave the modal unopenable for every later test.
    await callVueMethod(client, "keyboardActions.showAllShortcuts");
    await client.waitForNoElement(".shortcuts-modal");
    expect(rows.length, "the shortcuts modal listed nothing").toBeGreaterThan(10);
    return rows;
  }

  async function terminalText(): Promise<string> {
    return client.executeSync<string>(
      `return Array.from(document.querySelectorAll('${TERMINAL_ROWS}'))
        .map((row) => row.textContent).join("\\n");`,
    );
  }

  async function focusTerminal(): Promise<void> {
    await client.waitForElement(".main-panel .terminal-container", 25_000);
    await client.waitForElement(".main-panel .xterm-helper-textarea", 10_000);
    await client.executeSync(
      `const el = document.querySelector(".main-panel .xterm-helper-textarea");
       if (el instanceof HTMLElement) el.focus();
       return document.activeElement === el;`,
    );
    await sleep(250);
  }

  /**
   * Put the caret where it lives in normal use, and say where that was.
   *
   * The agent view's composer takes focus on its own and keeps it, so a chord
   * a text field concedes is a chord that does nothing in the view the app is
   * used from — which is how repo navigation was measured working and was
   * dead. Any binding meant to work "while using the app" has to be pressed
   * from a focused field to have been tested at all.
   *
   * The composer is the true scene and is preferred, but it exists only while
   * the task has an agent session; the sidebar search is always there and is
   * the same kind of element as far as the guard is concerned. Either is a
   * real text field, and the run reports which one it used rather than
   * quietly testing a different thing than it claims.
   */
  async function focusTextField(): Promise<string> {
    const deadline = Date.now() + 20_000;
    let diagnosis = "never polled";
    while (Date.now() < deadline) {
      const state = await client.executeSync<{ focused: string | null; diagnosis: string }>(
        `const composer = document.querySelector('${AGENT_COMPOSER}');
         if (composer instanceof HTMLElement) {
           composer.focus();
           if (document.activeElement === composer) return { focused: "agent composer", diagnosis: "" };
         }
         const search = document.querySelector('${SIDEBAR_SEARCH}');
         if (search instanceof HTMLElement) {
           search.focus();
           if (document.activeElement === search) return { focused: "sidebar search", diagnosis: "" };
         }
         const panel = document.querySelector(".main-panel");
         return {
           focused: null,
           diagnosis: "composer=" + !!composer + " search=" + !!search +
             " panel=" + (panel ? panel.textContent.replace(/\\s+/g, " ").trim().slice(0, 120) : "absent"),
         };`,
      );
      if (state.focused) {
        await sleep(200);
        return state.focused;
      }
      diagnosis = state.diagnosis;
      await sleep(250);
    }
    throw new Error(`no text field would take focus, so this chord cannot be tested where it matters: ${diagnosis}`);
  }

  async function blurActiveElement(): Promise<void> {
    await client.executeSync(
      "if (document.activeElement && document.activeElement.blur) document.activeElement.blur(); return true;",
    );
    await sleep(200);
  }

  /** Tabs persist per task, so each test starts from the agent session alone. */
  async function closeViewTabs(): Promise<void> {
    for (let attempt = 0; attempt < 8; attempt += 1) {
      const closed = await client.executeSync<boolean>(
        `const close = document.querySelector('[data-testid^="main-tab-close-"]');
         if (!close) return false;
         close.click();
         return true;`,
      );
      if (!closed) return;
      await sleep(150);
    }
    throw new Error(`tabs would not close: ${JSON.stringify(await openTabIds())}`);
  }

  beforeAll(async () => {
    keyboard = await inspectRealKeyboard();
    clipboard = await inspectRealClipboard();
    await client.createSession();
    await client.waitForAppReady(30_000);
    await resetDatabase(client);

    // The seed name selects a template under `tests/e2e/fixtures/repos`;
    // this lane needs nothing from the repo but a task to attach a terminal to.
    fixtureRepoRoot = await createSeedFixtureRepo("task-switch-minimal");
    testRepoPath = fixtureRepoRoot;
    await importTestRepo(client, testRepoPath, "linux-shortcut-keys");

    const repoId = (await getVueState(client, "selectedRepoId")) as string;
    taskId = crypto.randomUUID();
    const branch = `task-${taskId}`;
    const worktreePath = `${testRepoPath}/.kanna-worktrees/${branch}`;
    await tauriInvoke(client, "git_worktree_add", { repoPath: testRepoPath, branch, path: worktreePath });
    await tauriInvoke(client, "run_script", {
      script: "printf '\\n# linux shortcut keys e2e\\n' >> README.md",
      cwd: worktreePath,
      env: {},
    });
    const created = await client.executeAsync<string>(
      `const cb = arguments[arguments.length - 1];
       const ctx = window.__KANNA_E2E__.setupState;
       ${SELECT_SIDEBAR_ITEM_SCRIPT}
       const db = ctx.db.value || ctx.db;
       db.execute("INSERT INTO pipeline_item (id, repo_id, prompt, stage, branch, agent_type) VALUES (?, ?, ?, ?, ?, ?)",
         ["${taskId}", "${repoId}", "linux shortcut keys", "in progress", "${branch}", "agent"])
         .then(function () {
           return db.execute("INSERT INTO worktree (id, pipeline_item_id, path, branch) VALUES (?, ?, ?, ?)",
             ["wt-${taskId}", "${taskId}", "${worktreePath}", "${branch}"]);
         })
         .then(function () { return ctx.loadItems("${repoId}"); })
         .then(function () {
           selectSidebarItem(ctx, "${taskId}");
           return ctx.refreshAllItems ? ctx.refreshAllItems() : null;
         })
         .then(function () { cb("ok"); })
         .catch(function (e) { cb("err:" + (e && e.message ? e.message : String(e))); });`,
    );
    if (typeof created === "string" && created.startsWith("err:")) {
      throw new Error(`creating the lane's task failed: ${created.slice(4)}`);
    }
    await client.waitForText(".sidebar", "linux shortcut keys");

    await client.executeSync(`return ${INSTALL_KEY_PROBE_SCRIPT};`);
  }, 240_000);

  afterAll(async () => {
    await client.executeSync(`return ${REMOVE_KEY_PROBE_SCRIPT};`).catch(() => undefined);
    if (testRepoPath) await cleanupWorktrees(client, testRepoPath);
    await cleanupFixtureRepos(fixtureRepoRoot ? [fixtureRepoRoot] : []);
    await client.deleteSession();
  });

  it("records which desktop the evidence came from", async () => {
    requireRealKeyboard();
    // Printed rather than asserted. A keymap verdict is a verdict about one
    // set of compositor and input-method grabs; a reader of a passing run has
    // to be able to tell which set that was.
    // eslint-disable-next-line no-console
    console.log(
      `[linux-keys] ${process.env.XDG_CURRENT_DESKTOP ?? "unknown desktop"} / ` +
        `${process.env.XDG_SESSION_TYPE ?? "unknown session"}; input method: ` +
        `${process.env.GTK_IM_MODULE ?? process.env.XMODIFIERS ?? "unset"}`,
    );
    await focusWindowUnderTest();
    expect(await windowHasFocus()).toBe(true);
  });

  describe("tab cycling", () => {
    /**
     * The owner's first finding, both halves. Tab cycling worked on a chord
     * the modal never showed, while the chord the modal did show beside an
     * arrow did nothing he could see.
     */
    it("lists the chord it actually cycles tabs on", async () => {
      requireRealKeyboard();
      const rows = await listedShortcuts();
      const keys = rows.map((row) => row.keys);
      expect(keys).toContain("Ctrl+Page Up");
      expect(keys).toContain("Ctrl+Page Down");
      // The literally transformed macOS chord, which is what used to work
      // while being listed nowhere.
      expect(keys.join(" ")).not.toMatch(/Ctrl\+Alt\+[[\]]/);
    });

    it("cycles the main-area tabs on Ctrl+Page Up and Ctrl+Page Down", async () => {
      requireRealKeyboard();
      await closeViewTabs();
      await callVueMethod(client, "keyboardActions.showDiff");
      await waitForActiveTab("diff");
      expect(await openTabIds()).toEqual(["agent", "diff"]);

      await expectArrived("Ctrl+PageUp");
      await waitForActiveTab("agent");

      await expectArrived("Ctrl+PageDown");
      await waitForActiveTab("diff");
      await closeViewTabs();
    }, 120_000);
  });

  describe("the arrow bindings", () => {
    /**
     * This is where the owner's "the listed ctrl+shift+left/right arrow does
     * not [work]" was two separate bugs stacked on each other, and where this
     * lane earned its cost: pressing the chords for real is what showed that
     * the vertical arrows the app was bound to never arrive at all.
     *
     * Ctrl+Shift+←/→ arrives and is pane focus, which does nothing until the
     * main area is split — a label problem, now fixed in the label. But
     * Ctrl+Shift+↑/↓, which repo navigation sat on, and Alt+↑/↓, which task
     * navigation sat on, deliver their modifier keydowns to the webview and
     * never the arrow. `gsettings` names no owner for either; it does not
     * matter who takes them, because a chord that does not arrive cannot be
     * bound. So both moved, and this asserts the new ones arrive and are
     * claimed while the old ones are gone from the list.
     */
    const DEAD_VERTICAL_CHORDS = ["Ctrl+Shift+ArrowUp", "Ctrl+Shift+ArrowDown", "Alt+ArrowUp", "Alt+ArrowDown"];

    it("does not deliver the vertical chords the keymap had to abandon", async () => {
      requireRealKeyboard();
      for (const chord of DEAD_VERTICAL_CHORDS) {
        const matches = await press(chord);
        expect(
          matches.length,
          `${chord} reached the webview after all. If this desktop delivers it, the measurement ` +
            "behind moving task and repo navigation off it no longer holds and the keymap " +
            "comment in shortcutPlatform.ts is stale.",
        ).toBe(0);
      }
    }, 120_000);

    it("lists no binding on a chord that never arrives", async () => {
      requireRealKeyboard();
      const keys = (await listedShortcuts()).map((row) => row.keys);
      for (const chord of ["Ctrl+Shift+↑", "Ctrl+Shift+↓", "Alt+↑", "Alt+↓", "Ctrl+Alt+↑", "Ctrl+Alt+↓"]) {
        expect(keys, `${chord} is listed, and this desktop does not deliver it`).not.toContain(chord);
      }
    });

    it("moves between tasks on Ctrl+Up and Ctrl+Down", async () => {
      requireRealKeyboard();
      const keys = (await listedShortcuts()).map((row) => row.keys);
      expect(keys).toContain("Ctrl+↑");
      expect(keys).toContain("Ctrl+↓");
      // One task exists, so the selection cannot move. What has to be true is
      // that the app took the chord — `preventDefault()` is called on the way
      // into the action, so a claimed chord is an action that ran. That is the
      // whole difference between this binding and the one it replaced.
      await expectClaimed("Ctrl+ArrowDown");
      await expectClaimed("Ctrl+ArrowUp");
    }, 120_000);

    /**
     * Pressed from a focused text field, because that is where the caret is
     * whenever the app is being used, and the first version of this test —
     * pressed against no particular element — passed while the binding was
     * dead there. The text-editing guard conceded every Shift+caret chord to
     * any editable element, and the agent composer holds focus in the task
     * view, so repo navigation worked only in the one state nobody is in.
     */
    it("moves between repos on Alt+Shift+Up and Alt+Shift+Down, from a text field", async () => {
      requireRealKeyboard();
      const before = (await getVueState(client, "selectedRepoId")) as string;
      const keys = (await listedShortcuts()).map((row) => row.keys);
      expect(keys).toContain("Alt+Shift+↑");
      expect(keys).toContain("Alt+Shift+↓");

      const field = await focusTextField();
      // eslint-disable-next-line no-console
      console.log(`[linux-keys] repo navigation pressed from the ${field}`);
      await expectClaimed("Alt+Shift+ArrowDown");
      await expectClaimed("Alt+Shift+ArrowUp");

      // One repo is imported, so the selection cannot move either.
      expect(await getVueState(client, "selectedRepoId")).toBe(before);
      await blurActiveElement();
    }, 120_000);

    it("reaches the webview on the horizontal pane arrows", async () => {
      requireRealKeyboard();
      for (const chord of ["Ctrl+Shift+ArrowLeft", "Ctrl+Shift+ArrowRight"]) {
        const event = await expectArrived(chord);
        expect(event.ctrl).toBe(true);
        expect(event.shift).toBe(true);
        expect(event.alt).toBe(false);
      }
    }, 120_000);

    it("labels pane focus as the split-view binding it is", async () => {
      requireRealKeyboard();
      const rows = await listedShortcuts();
      const panes = rows.filter((row) => row.keys === "Ctrl+Shift+←" || row.keys === "Ctrl+Shift+→");
      expect(panes.map((row) => row.action).sort()).toEqual([
        "Focus Next Pane (split view)",
        "Focus Previous Pane (split view)",
      ]);
    });
  });

  describe("chords the desktop takes first", () => {
    /**
     * The owner pressed Ctrl+Shift+U and got a literal "u" in the terminal.
     * That is IBus Unicode entry, on by default on Ubuntu, which no
     * application outranks — so the fix was never a handler change but to stop
     * claiming the chord. This asserts the part he can see.
     */
    it("lists nothing on Ctrl+Shift+U", async () => {
      requireRealKeyboard();
      const keys = (await listedShortcuts()).map((row) => row.keys);
      expect(keys.filter((chord) => /^Ctrl\+Shift\+U$/i.test(chord))).toEqual([]);
    });

    it("goes to the oldest unread on Ctrl+Alt+U, which does arrive", async () => {
      requireRealKeyboard();
      const keys = (await listedShortcuts()).map((row) => row.keys);
      expect(keys).toContain("Ctrl+Alt+U");
      await expectClaimed("Ctrl+Alt+U");
    });

    /**
     * Back and forward used to sit on Ctrl+- and Ctrl+Shift+-, which is zoom
     * out and zoom in in every browser engine, including the one the app runs
     * on. Alt+Arrow is what a Linux desktop means by back and forward.
     */
    it("navigates history on Alt+Left and Alt+Right", async () => {
      requireRealKeyboard();
      const keys = (await listedShortcuts()).map((row) => row.keys);
      expect(keys).toContain("Alt+←");
      expect(keys).toContain("Alt+→");
      expect(keys.join(" ")).not.toMatch(/Ctrl\+(Shift\+)?-/);
      await expectClaimed("Alt+ArrowLeft");
      await expectClaimed("Alt+ArrowRight");
    });
  });

  describe("the terminal's claim on the keyboard", () => {
    /**
     * Plain Ctrl+C is the one key an agent session cannot lose: it is how a
     * runaway turn is stopped. Ctrl+Shift+C is what every Linux terminal uses
     * for copy precisely because Ctrl+C is spoken for. The app has to route
     * them apart, and only a real keystroke through a real PTY shows that it
     * does.
     */
    it("keeps Ctrl+Shift+C out of the PTY and lets plain Ctrl+C in", async () => {
      requireRealKeyboard();
      await closeViewTabs();
      await callVueMethod(client, "keyboardActions.openShell");
      await waitForActiveTab("shell");
      await focusTerminal();

      const beforeCopy = await terminalText();
      const copy = await expectArrived("Ctrl+Shift+c");
      expect(copy.target, "Ctrl+Shift+C did not land on the terminal").toContain("xterm-helper-textarea");
      await sleep(600);
      const afterCopy = await terminalText();
      expect(
        afterCopy.slice(beforeCopy.length).includes("^C"),
        "Ctrl+Shift+C reached the PTY as an interrupt; on Linux it is the copy chord",
      ).toBe(false);

      await expectArrived("Ctrl+c");
      const deadline = Date.now() + 10_000;
      let sawInterrupt = false;
      while (Date.now() < deadline) {
        if ((await terminalText()).includes("^C")) {
          sawInterrupt = true;
          break;
        }
        await sleep(250);
      }
      expect(sawInterrupt, "plain Ctrl+C never reached the PTY, so no agent could be interrupted").toBe(true);
      await closeViewTabs();
    }, 180_000);

    /**
     * The other half of the same chord pair, and the one that was dead.
     *
     * `Ctrl+Shift+V` arrived on the terminal the whole time, and the
     * terminal's own handler took it and called `preventDefault()`. What it
     * then did was `navigator.clipboard.readText()`, which WebKitGTK refuses
     * by policy on every call, so the chord the modal advertises pasted
     * nothing and logged a `NotAllowedError` where nobody looks. Neither
     * signal this lane records could catch that: the keystroke was never the
     * problem, and the terminal claims the chord in the target phase, after
     * the probe on `window` has already read `defaultPrevented`.
     *
     * So this one asserts the payload instead. The selection is put on the
     * desktop clipboard by `wl-copy` — a separate Wayland client, as the other
     * window a person copies from would be — and the text has to come out of a
     * real PTY on the other side.
     */
    it("pastes the desktop clipboard into the PTY on Ctrl+Shift+V", async () => {
      requireRealKeyboard();
      requireRealClipboard();
      await closeViewTabs();
      await callVueMethod(client, "keyboardActions.openShell");
      await waitForActiveTab("shell");
      await focusTerminal();

      // Unique per run: the assertion is "this paste arrived", and a fixed
      // token could be satisfied by scrollback from an earlier one.
      const token = `kanna-paste-${crypto.randomUUID().slice(0, 8)}`;
      await writeRealClipboard(token);

      // Read it back through the app's own command first. This separates a
      // clipboard the app cannot see at all — which on this desktop means the
      // session's DISPLAY/XAUTHORITY are missing from the app's environment,
      // so `arboard` has no Xwayland connection to read the bridged selection
      // through — from a chord that failed to paste what it could see.
      //
      // Polled, because `wl-copy` returning means the selection was offered:
      // the compositor publishes it, and Xwayland mirrors it onto the X11
      // selection the app reads, a moment later.
      let nativeRead: string | null = null;
      const readDeadline = Date.now() + 5_000;
      while (Date.now() < readDeadline) {
        nativeRead = (await tauriInvoke(client, "read_clipboard_text", {})) as string | null;
        if (nativeRead === token) break;
        await sleep(200);
      }
      expect(
        nativeRead,
        "the app could not read a selection wl-copy had just published; if this failed rather than " +
          "returned the wrong text, export the graphical session's DISPLAY and XAUTHORITY into the lane",
      ).toBe(token);

      // Record what the page logs, so a silent denial cannot pass as a paste
      // that merely lost a race.
      await client.executeSync(
        `window.__KANNA_PASTE_LOG__ = [];
         if (!window.__KANNA_PASTE_LOG_PATCHED__) {
           window.__KANNA_PASTE_LOG_PATCHED__ = true;
           for (const level of ["warn", "error"]) {
             const original = console[level].bind(console);
             console[level] = function () {
               try {
                 (window.__KANNA_PASTE_LOG__ || []).push(
                   Array.from(arguments).map((arg) => {
                     if (arg instanceof Error) return arg.name + ": " + arg.message;
                     if (typeof arg === "object" && arg !== null) return JSON.stringify(arg);
                     return String(arg);
                   }).join(" "),
                 );
               } catch (e) { /* recording must never break the app's own logging */ }
               return original.apply(console, arguments);
             };
           }
         }
         return true;`,
      );

      const pasted = await expectArrived("Ctrl+Shift+v");
      expect(pasted.target, "Ctrl+Shift+V did not land on the terminal").toContain("xterm-helper-textarea");

      // Search the whole buffer, never a tail of it. `terminalText()` renders
      // the screen, so a shell redraw rewrites earlier rows and the text is
      // shorter or longer than the paste alone would make it — an offset taken
      // before the chord points somewhere else afterwards. The token is unique
      // per run, so finding it anywhere is proof it came from this paste.
      const deadline = Date.now() + 15_000;
      let arrived = false;
      while (Date.now() < deadline) {
        if ((await terminalText()).includes(token)) {
          arrived = true;
          break;
        }
        await sleep(250);
      }

      const logged = (await client.executeSync<string[]>("return window.__KANNA_PASTE_LOG__ || [];")) ?? [];
      const denials = logged.filter((line) => /clipboard/i.test(line));
      expect(
        denials,
        "the paste path logged a clipboard failure; the webview clipboard API is denied on this platform " +
          "and the read has to go through the native command",
      ).toEqual([]);
      expect(
        arrived,
        `Ctrl+Shift+V pasted nothing into the PTY. The terminal holds ${JSON.stringify(
          (await terminalText()).slice(-200),
        )}`,
      ).toBe(true);

      await closeViewTabs();
    }, 180_000);
  });

  describe("keystrokes that belong to the text field they landed in", () => {
    /**
     * The global handler listens in the capture phase and calls
     * `preventDefault()`. Before the guard, every Ctrl+Shift+Arrow typed in a
     * text field was swallowed there: no selection, no caret movement, and the
     * app navigating repos behind a field the person was editing.
     */
    it("extends the selection with Ctrl+Shift+Left in the sidebar search", async () => {
      requireRealKeyboard();
      const input = await client.waitForElement(SIDEBAR_SEARCH, 8_000);
      await client.clear(input);
      await client.sendKeys(input, "keymap check");
      const before = await client.executeSync<Selection>(
        `const el = document.querySelector('${SIDEBAR_SEARCH}');
         el.focus();
         el.setSelectionRange(el.value.length, el.value.length);
         return { start: el.selectionStart, end: el.selectionEnd, value: el.value };`,
      );
      expect(before.value).toContain("keymap check");

      const event = await expectArrived("Ctrl+Shift+ArrowLeft");
      expect(
        event.defaultPrevented,
        "the app claimed a selection chord out of a focused text field",
      ).toBe(false);

      const after = await client.executeSync<Selection>(
        `const el = document.querySelector('${SIDEBAR_SEARCH}');
         return { start: el.selectionStart, end: el.selectionEnd, value: el.value };`,
      );
      expect(after.end).toBe(before.end);
      expect(
        after.start,
        "Ctrl+Shift+Left selected nothing in a focused text field",
      ).toBeLessThan(before.start);

      await client.clear(input);
      await client.executeSync(
        `const el = document.querySelector('${SIDEBAR_SEARCH}'); if (el) el.blur(); return true;`,
      );
    }, 120_000);

    /**
     * The other half of the same rule. A guard that disarmed the chord
     * everywhere would make the fix for one bug into the next one.
     */
    it("still claims the same chord away from a text field", async () => {
      requireRealKeyboard();
      await blurActiveElement();
      expect(
        (await expectArrived("Ctrl+Shift+ArrowLeft")).defaultPrevented,
        "the text-field guard disarmed the chord outside a text field",
      ).toBe(true);
    });
  });
});
