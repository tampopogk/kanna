// @vitest-environment happy-dom

import { defineComponent } from "vue";
import { mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import {
  getShortcutGroups,
  isAppShortcut,
  resetShortcutBindingsForTests,
  secondaryBindingsFor,
  useKeyboardShortcuts,
  shortcuts,
  type ActionName,
  type KeyboardActions,
} from "./useKeyboardShortcuts";
import type { ShortcutContext } from "./useShortcutContext";
import en from "../i18n/locales/en.json";
import ja from "../i18n/locales/ja.json";
import ko from "../i18n/locales/ko.json";

function identityTranslate(key: string): string {
  return key;
}

function englishTranslate(key: string): string {
  const values: Record<string, string> = {
    "shortcuts.groupOpenInspect": "Tools",
    "shortcuts.commandPalette": "Command Palette",
    "shortcuts.analytics": "Analytics",
    "shortcuts.commitGraph": "Commit Graph",
    "shortcuts.filePicker": "File Picker",
    "shortcuts.filePreview": "File Preview",
    "shortcuts.openInIDE": "Open in IDE",
    "shortcuts.openLatestAgentFile": "Open Latest Agent File",
    "shortcuts.shellRepoRoot": "Shell at Repo Root",
    "shortcuts.shellTerminal": "Shell Terminal",
    "shortcuts.treeExplorer": "Tree Explorer",
    "shortcuts.viewDiff": "View Diff",
  };
  return values[key] ?? key;
}

describe("getShortcutGroups", () => {
  it("groups full-menu shortcuts by workflow-first categories", () => {
    const groups = getShortcutGroups(identityTranslate);

    expect(groups.map((group) => group.title)).toEqual([
      "shortcuts.groupCreateOrganize",
      "shortcuts.groupMoveAround",
      "shortcuts.groupOpenInspect",
      "shortcuts.groupWorkspace",
      "shortcuts.groupAppHelp",
    ]);
  });

  it("assigns shortcuts to the expected workflow-first groups", () => {
    const groups = getShortcutGroups(identityTranslate);
    const groupMap = Object.fromEntries(
      groups.map((group) => [group.title, group.shortcuts.map((shortcut) => shortcut.action)]),
    );

    expect(groupMap["shortcuts.groupCreateOrganize"]).toEqual([
      "shortcuts.createRepo",
      "shortcuts.importClone",
      "shortcuts.newTask",
      "shortcuts.focusSearch",
      "shortcuts.advanceStage",
      "shortcuts.closeReject",
    ]);

    expect(groupMap["shortcuts.groupMoveAround"]).toEqual([
      "shortcuts.previousTask",
      "shortcuts.nextTask",
      "shortcuts.previousRepo",
      "shortcuts.nextRepo",
      "shortcuts.previousPane",
      "shortcuts.nextPane",
      "shortcuts.goBack",
      "shortcuts.goForward",
      "shortcuts.oldestUnread",
      "shortcuts.oldestUnreadAllRepos",
      "shortcuts.oldestRead",
      "shortcuts.oldestReadAllRepos",
    ]);

    expect([...groupMap["shortcuts.groupOpenInspect"]].sort()).toEqual([
      "shortcuts.analytics",
      "shortcuts.commandPalette",
      "shortcuts.commitGraph",
      "shortcuts.filePicker",
      "shortcuts.filePreview",
      "shortcuts.openInIDE",
      "shortcuts.openLatestAgentFile",
      "shortcuts.shellRepoRoot",
      "shortcuts.shellTerminal",
      "shortcuts.treeExplorer",
      "shortcuts.viewDiff",
    ].sort());

    expect(groupMap["shortcuts.groupWorkspace"]).toEqual([
      "shortcuts.newWindow",
      "shortcuts.closeTab",
      "shortcuts.closeWindow",
      "shortcuts.toggleSidebar",
      "shortcuts.maximize",
    ]);

    expect(groupMap["shortcuts.groupAppHelp"]).toEqual([
      "shortcuts.preferences",
      "shortcuts.keyboardShortcuts",
    ]);
  });

  it("sorts tools alphabetically by their visible label", () => {
    const groups = getShortcutGroups(englishTranslate);
    const tools = groups.find((group) => group.title === "Tools");

    expect(tools?.shortcuts.map((shortcut) => shortcut.action)).toEqual([
      "Analytics",
      "Command Palette",
      "Commit Graph",
      "File Picker",
      "File Preview",
      "Open in IDE",
      "Open Latest Agent File",
      "Shell at Repo Root",
      "Shell Terminal",
      "Tree Explorer",
      "View Diff",
    ]);
  });
});

describe("isAppShortcut", () => {
  it("matches shifted letter shortcuts using the uppercase event key", () => {
    expect(isAppShortcut(new KeyboardEvent("keydown", {
      key: "E",
      metaKey: true,
      shiftKey: true,
    }))).toBe(true);
  });

  it("matches Option+Command+P for file preview recall", () => {
    expect(isAppShortcut(new KeyboardEvent("keydown", {
      key: "p",
      metaKey: true,
      altKey: true,
    }))).toBe(true);
  });

  it("matches the macOS Option+Command+P character event by physical key code", () => {
    expect(isAppShortcut(new KeyboardEvent("keydown", {
      key: "π",
      code: "KeyP",
      metaKey: true,
      altKey: true,
    }))).toBe(true);
  });

  it("matches the new window shortcut", () => {
    expect(isAppShortcut(new KeyboardEvent("keydown", {
      key: "n",
      metaKey: true,
    }))).toBe(true);
  });

  it("matches the close window shortcut", () => {
    expect(isAppShortcut(new KeyboardEvent("keydown", {
      key: "w",
      metaKey: true,
    }))).toBe(true);
  });

  it("matches Command+L for the latest agent file", () => {
    expect(isAppShortcut(new KeyboardEvent("keydown", {
      key: "l",
      metaKey: true,
    }))).toBe(true);
  });

  it("does not reserve Command+Z", () => {
    expect(isAppShortcut(new KeyboardEvent("keydown", {
      key: "z",
      metaKey: true,
    }))).toBe(false);
  });

  it("leaves bare brackets to focused views and terminals while reserving modified brackets for tabs", () => {
    expect(isAppShortcut(new KeyboardEvent("keydown", { key: "]" }))).toBe(false);
    expect(isAppShortcut(new KeyboardEvent("keydown", { key: "[" }))).toBe(false);
    expect(isAppShortcut(new KeyboardEvent("keydown", {
      key: "]",
      metaKey: true,
      shiftKey: true,
    }))).toBe(true);
  });
});

describe("shortcut contexts", () => {
  it("allows Command+P to open the file picker from the file preview", () => {
    const openFileShortcut = shortcuts.find((shortcut) => shortcut.action === "openFile");

    expect(openFileShortcut?.context).toContain("file");
  });

  it("maps Command+L to the latest agent file action", () => {
    const shortcut = shortcuts.find((entry) => entry.action === "openLatestFileLink");

    expect(shortcut).toMatchObject({ key: "l", meta: true, display: "⌘L" });
    expect(shortcut?.context).toContain("file");
  });
});

describe("useKeyboardShortcuts", () => {
  const actionNames: ActionName[] = [
    "newTask",
    "newWindow",
    "openFile",
    "openLatestFileLink",
    "toggleFilePreview",
    "advanceStage",
    "closeTask",
    "undoClose",
    "navigateUp",
    "navigateDown",
    "navigateRepoUp",
    "navigateRepoDown",
    "dismiss",
    "openInIDE",
    "openShell",
    "showDiff",
    "showCommitGraph",
    "toggleMaximize",
    "previousPane",
    "nextPane",
    "showShortcuts",
    "showAllShortcuts",
    "toggleSidebar",
    "commandPalette",
    "showAnalytics",
    "goBack",
    "goForward",
    "createRepo",
    "importRepo",
    "blockTask",
    "editBlockedTask",
    "toggleTreeExplorer",
    "openPreferences",
    "openShellRepoRoot",
    "prevTab",
    "nextTab",
    "focusSearch",
    "goToOldestUnread",
    "goToOldestUnreadAllRepos",
    "goToOldestRead",
    "goToOldestReadAllRepos",
  ];

  function buildActions(): KeyboardActions {
    return Object.fromEntries(actionNames.map((name) => [name, vi.fn()])) as KeyboardActions;
  }

  function mountShortcutHarness(
    actions: KeyboardActions,
    context: () => ShortcutContext,
    enabled?: () => boolean,
  ) {
    const Harness = defineComponent({
      setup() {
        useKeyboardShortcuts(actions, { context, ...(enabled ? { enabled } : {}) });
        return () => null;
      },
    });

    return mount(Harness);
  }

  /**
   * The owner's report: "ctrl+shift+u etc doesn't work" and the listed
   * Ctrl+Shift+arrows "do not". Underneath both was the same thing — this
   * listener captures before anything else and calls `preventDefault()`, so a
   * chord it claims is gone from every text field in the app. Ctrl+Shift+←
   * (⇧⌘← on this suite's declared platform) is *the* word-selection chord.
   */
  describe("keystrokes that belong to the text field they landed in", () => {
    function pressAt(target: HTMLElement, init: KeyboardEventInit): KeyboardEvent {
      const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
      target.dispatchEvent(event);
      return event;
    }

    function withTarget(html: string, run: (target: HTMLElement) => void): void {
      const host = document.createElement("div");
      host.innerHTML = html;
      const target = host.firstElementChild as HTMLElement;
      document.body.appendChild(host);
      try {
        run(target);
      } finally {
        host.remove();
      }
    }

    it("leaves a selection chord to a focused input", () => {
      const actions = buildActions();
      const wrapper = mountShortcutHarness(actions, () => "main");
      withTarget(`<input type="text">`, (input) => {
        const event = pressAt(input, { key: "ArrowUp", metaKey: true, shiftKey: true });
        expect(actions.navigateRepoUp).not.toHaveBeenCalled();
        expect(event.defaultPrevented).toBe(false);
      });
      wrapper.unmount();
    });

    it("leaves it to a textarea and a contenteditable too", () => {
      const actions = buildActions();
      const wrapper = mountShortcutHarness(actions, () => "main");
      withTarget(`<textarea></textarea>`, (area) => {
        pressAt(area, { key: "ArrowUp", metaKey: true, shiftKey: true });
      });
      withTarget(`<div contenteditable="true"></div>`, (editable) => {
        pressAt(editable, { key: "ArrowUp", metaKey: true, shiftKey: true });
      });
      expect(actions.navigateRepoUp).not.toHaveBeenCalled();
      wrapper.unmount();
    });

    it("still acts on the same chord anywhere else", () => {
      const actions = buildActions();
      const wrapper = mountShortcutHarness(actions, () => "main");
      withTarget(`<div></div>`, (plain) => {
        pressAt(plain, { key: "ArrowUp", metaKey: true, shiftKey: true });
      });
      expect(actions.navigateRepoUp).toHaveBeenCalledTimes(1);
      wrapper.unmount();
    });

    it("still acts on a chord a text field has no use for", () => {
      // Without Shift there is no selection to extend, so moving between tasks
      // from a focused search field — which is how search is used at all —
      // keeps working.
      const actions = buildActions();
      const wrapper = mountShortcutHarness(actions, () => "main");
      withTarget(`<input type="search">`, (input) => {
        pressAt(input, { key: "ArrowUp", metaKey: true, altKey: true });
      });
      expect(actions.navigateUp).toHaveBeenCalledTimes(1);
      wrapper.unmount();
    });

    it("keeps acting from a focused terminal", () => {
      // xterm's helper textarea is an editable element in the DOM only: what
      // is typed into it goes to the PTY, and there is no selection in it to
      // protect.
      const actions = buildActions();
      const wrapper = mountShortcutHarness(actions, () => "main");
      withTarget(`<textarea class="xterm-helper-textarea"></textarea>`, (helper) => {
        pressAt(helper, { key: "ArrowUp", metaKey: true, shiftKey: true });
      });
      expect(actions.navigateRepoUp).toHaveBeenCalledTimes(1);
      wrapper.unmount();
    });

    /**
     * Shift+Backspace extends no selection, so the listed close chord is not
     * the field's to claim. It was conceded anyway for a while, and the whole
     * cost of that lands in the one view a person lives in: the agent composer
     * and the sidebar search hold the caret nearly all the time, so ⇧⌘⌫ /
     * Ctrl+Shift+Backspace closed nothing and said nothing about why.
     */
    it("still closes a task from a focused text field on both platforms", () => {
      const cases = [
        { platform: "MacIntel", init: { key: "Backspace", metaKey: true, shiftKey: true } },
        { platform: "Linux x86_64", init: { key: "Backspace", ctrlKey: true, shiftKey: true } },
      ] as const;

      for (const { platform, init } of cases) {
        for (const nav of [globalThis.navigator, window.navigator]) {
          Object.defineProperty(nav, "platform", { value: platform, configurable: true });
        }
        resetShortcutBindingsForTests();
        const actions = buildActions();
        const wrapper = mountShortcutHarness(actions, () => "main");

        try {
          for (const html of [`<input type="text">`, `<textarea></textarea>`]) {
            withTarget(html, (field) => {
              field.focus();
              const event = pressAt(field, init);
              expect(event.defaultPrevented, `${platform} ${html}`).toBe(true);
            });
          }
          expect(actions.closeTask, platform).toHaveBeenCalledTimes(2);
        } finally {
          wrapper.unmount();
          for (const nav of [globalThis.navigator, window.navigator]) {
            Object.defineProperty(nav, "platform", { value: "MacIntel", configurable: true });
          }
          resetShortcutBindingsForTests();
        }
      }
    });
  });

  it("ignores workspace shortcuts until the window is ready for them", () => {
    const actions = buildActions();
    let ready = false;
    const wrapper = mountShortcutHarness(actions, () => "main", () => ready);

    const pressNewTask = () => window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "N",
      metaKey: true,
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    }));

    // The listener is registered in the capture phase, so it would otherwise
    // act on a workspace that is still being restored behind the startup
    // screen — `inert` on the workspace does not reach it.
    pressNewTask();
    expect(actions.newTask).not.toHaveBeenCalled();

    ready = true;
    pressNewTask();
    expect(actions.newTask).toHaveBeenCalledTimes(1);

    wrapper.unmount();
  });

  it.each([
    { key: "U", action: "goToOldestUnreadAllRepos" as const, labelKey: "shortcuts.oldestUnreadAllRepos" },
    { key: "R", action: "goToOldestReadAllRepos" as const, labelKey: "shortcuts.oldestReadAllRepos" },
  ])("maps Shift+Command+$key to $action", ({ key, action, labelKey }) => {
    expect(shortcuts.find((shortcut) => shortcut.action === action)).toMatchObject({
      action,
      labelKey,
      key: [key, key.toLowerCase()],
      meta: true,
      shift: true,
    });

    const actions = buildActions();
    const wrapper = mountShortcutHarness(actions, () => "main");
    window.dispatchEvent(new KeyboardEvent("keydown", {
      key,
      metaKey: true,
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    }));

    expect(actions[action]).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("allows opening the file picker from the diff modal context", () => {
    const actions = buildActions();
    const wrapper = mountShortcutHarness(actions, () => "diff");

    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "p",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    }));

    expect(actions.openFile).toHaveBeenCalledTimes(1);
    expect(actions.newTask).not.toHaveBeenCalled();

    wrapper.unmount();
  });

  it("dispatches physical shifted-letter events to Linux actions", () => {
    for (const nav of [globalThis.navigator, window.navigator]) {
      Object.defineProperty(nav, "platform", { value: "Linux x86_64", configurable: true });
    }
    resetShortcutBindingsForTests();
    const actions = buildActions();
    const wrapper = mountShortcutHarness(actions, () => "main");

    try {
      for (const key of ["P", "B"]) {
        window.dispatchEvent(new KeyboardEvent("keydown", {
          key,
          code: `Key${key}`,
          ctrlKey: true,
          shiftKey: true,
          bubbles: true,
          cancelable: true,
        }));
      }

      // Ctrl+Shift+P is VS Code's command palette, and it is the palette here
      // too. It used to open the file picker, which is worse than a dead
      // chord: the wrong dialog appears and nothing says why.
      expect(actions.commandPalette).toHaveBeenCalledOnce();
      expect(actions.openFile).not.toHaveBeenCalled();
      expect(actions.toggleSidebar).toHaveBeenCalledOnce();
    } finally {
      wrapper.unmount();
      for (const nav of [globalThis.navigator, window.navigator]) {
        Object.defineProperty(nav, "platform", { value: "MacIntel", configurable: true });
      }
      resetShortcutBindingsForTests();
    }
  });

  it("dispatches and labels Linux Preferences on VS Code's plain Ctrl+, ", () => {
    for (const nav of [globalThis.navigator, window.navigator]) {
      Object.defineProperty(nav, "platform", { value: "Linux x86_64", configurable: true });
    }
    resetShortcutBindingsForTests();
    const actions = buildActions();
    const wrapper = mountShortcutHarness(actions, () => "main");

    try {
      const preferences = getShortcutGroups(identityTranslate)
        .flatMap((group) => group.shortcuts)
        .find((shortcut) => shortcut.action === "shortcuts.preferences");
      expect(preferences?.keys).toBe("Ctrl+,");

      window.dispatchEvent(new KeyboardEvent("keydown", {
        key: ",",
        code: "Comma",
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      }));

      expect(actions.openPreferences).toHaveBeenCalledOnce();

      // The chord the systematic mapping used to put it on is now nobody's.
      window.dispatchEvent(new KeyboardEvent("keydown", {
        key: "<",
        code: "Comma",
        ctrlKey: true,
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      }));
      expect(actions.openPreferences).toHaveBeenCalledOnce();
    } finally {
      wrapper.unmount();
      for (const nav of [globalThis.navigator, window.navigator]) {
        Object.defineProperty(nav, "platform", { value: "MacIntel", configurable: true });
      }
      resetShortcutBindingsForTests();
    }
  });

  it("dispatches the Linux Ctrl+Alt+P file picker, the tier the palette vacated", () => {
    for (const nav of [globalThis.navigator, window.navigator]) {
      Object.defineProperty(nav, "platform", { value: "Linux x86_64", configurable: true });
    }
    resetShortcutBindingsForTests();
    const actions = buildActions();
    const wrapper = mountShortcutHarness(actions, () => "main");

    try {
      window.dispatchEvent(new KeyboardEvent("keydown", {
        key: "p",
        code: "KeyP",
        ctrlKey: true,
        altKey: true,
        bubbles: true,
        cancelable: true,
      }));

      expect(actions.openFile).toHaveBeenCalledOnce();
      expect(actions.commandPalette).not.toHaveBeenCalled();
    } finally {
      wrapper.unmount();
      for (const nav of [globalThis.navigator, window.navigator]) {
        Object.defineProperty(nav, "platform", { value: "MacIntel", configurable: true });
      }
      resetShortcutBindingsForTests();
    }
  });

  it("dispatches the Linux Ctrl+Shift+/ shortcuts chord by physical code, even though Shift rewrites the key to '?'", () => {
    for (const nav of [globalThis.navigator, window.navigator]) {
      Object.defineProperty(nav, "platform", { value: "Linux x86_64", configurable: true });
    }
    resetShortcutBindingsForTests();
    const actions = buildActions();
    const wrapper = mountShortcutHarness(actions, () => "main");

    try {
      window.dispatchEvent(new KeyboardEvent("keydown", {
        key: "?",
        code: "Slash",
        ctrlKey: true,
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      }));

      expect(actions.showShortcuts).toHaveBeenCalledOnce();
      expect(actions.showAllShortcuts).not.toHaveBeenCalled();
    } finally {
      wrapper.unmount();
      for (const nav of [globalThis.navigator, window.navigator]) {
        Object.defineProperty(nav, "platform", { value: "MacIntel", configurable: true });
      }
      resetShortcutBindingsForTests();
    }
  });

  it("dispatches the Linux Ctrl+Alt+/ all-shortcuts chord, which never carries Shift so the key stays '/'", () => {
    for (const nav of [globalThis.navigator, window.navigator]) {
      Object.defineProperty(nav, "platform", { value: "Linux x86_64", configurable: true });
    }
    resetShortcutBindingsForTests();
    const actions = buildActions();
    const wrapper = mountShortcutHarness(actions, () => "main");

    try {
      window.dispatchEvent(new KeyboardEvent("keydown", {
        key: "/",
        code: "Slash",
        ctrlKey: true,
        altKey: true,
        bubbles: true,
        cancelable: true,
      }));

      expect(actions.showAllShortcuts).toHaveBeenCalledOnce();
      expect(actions.showShortcuts).not.toHaveBeenCalled();
    } finally {
      wrapper.unmount();
      for (const nav of [globalThis.navigator, window.navigator]) {
        Object.defineProperty(nav, "platform", { value: "MacIntel", configurable: true });
      }
      resetShortcutBindingsForTests();
    }
  });

  /**
   * Ctrl+J is VS Code's panel toggle and the worktree shell is the nearest
   * thing to it here — but Ctrl+J is also LF, a byte an agent composer needs
   * for a literal newline. VS Code takes it from its integrated terminal
   * through `commandsToSkipShell`; the terminal *is* the product here, so the
   * app takes it everywhere else and nowhere a PTY has the caret.
   */
  describe("the worktree shell's unlisted Linux Ctrl+J", () => {
    function onLinux(run: (actions: KeyboardActions) => void): void {
      for (const nav of [globalThis.navigator, window.navigator]) {
        Object.defineProperty(nav, "platform", { value: "Linux x86_64", configurable: true });
      }
      resetShortcutBindingsForTests();
      const actions = buildActions();
      const wrapper = mountShortcutHarness(actions, () => "main");
      try {
        run(actions);
      } finally {
        wrapper.unmount();
        for (const nav of [globalThis.navigator, window.navigator]) {
          Object.defineProperty(nav, "platform", { value: "MacIntel", configurable: true });
        }
        resetShortcutBindingsForTests();
      }
    }

    function pressCtrlJAt(target: EventTarget): KeyboardEvent {
      const event = new KeyboardEvent("keydown", {
        key: "j",
        code: "KeyJ",
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      });
      target.dispatchEvent(event);
      return event;
    }

    function withHelperTextarea(run: (helper: HTMLElement) => void): void {
      const host = document.createElement("div");
      host.innerHTML = `<textarea class="xterm-helper-textarea"></textarea>`;
      const helper = host.firstElementChild as HTMLElement;
      document.body.appendChild(host);
      try {
        run(helper);
      } finally {
        host.remove();
      }
    }

    it("opens the worktree shell when the caret is not in a PTY", () => {
      onLinux((actions) => {
        expect(pressCtrlJAt(window).defaultPrevented).toBe(true);
        expect(actions.openShell).toHaveBeenCalledOnce();
      });
    });

    it("leaves Ctrl+J to a focused terminal, where it is LF", () => {
      onLinux((actions) => {
        withHelperTextarea((helper) => {
          const event = pressCtrlJAt(helper);
          expect(event.defaultPrevented).toBe(false);
        });
        expect(actions.openShell).not.toHaveBeenCalled();
        // `isAppShortcut` is what the terminal consults to decide which keys
        // bubble up, so this is the same verdict from the PTY's side.
        expect(isAppShortcut(pressCtrlJAt(document.createElement("div")))).toBe(true);
      });
    });

    it("changes neither shell chord, and lists only the one that always works", () => {
      onLinux((actions) => {
        const listed = getShortcutGroups(identityTranslate)
          .flatMap((group) => group.shortcuts)
          .map((shortcut) => shortcut.keys);
        expect(listed).toContain("Ctrl+Shift+J");
        expect(listed).not.toContain("Ctrl+J");

        // Ctrl+Shift+J reaches the worktree shell from inside a terminal too,
        // which is why it, and not Ctrl+J, is the advertised one.
        withHelperTextarea((helper) => {
          helper.dispatchEvent(new KeyboardEvent("keydown", {
            key: "J",
            code: "KeyJ",
            ctrlKey: true,
            shiftKey: true,
            bubbles: true,
            cancelable: true,
          }));
        });
        expect(actions.openShell).toHaveBeenCalledOnce();

        // The main-checkout shell stays exactly where it was.
        window.dispatchEvent(new KeyboardEvent("keydown", {
          key: "J",
          code: "KeyJ",
          ctrlKey: true,
          altKey: true,
          bubbles: true,
          cancelable: true,
        }));
        expect(actions.openShellRepoRoot).toHaveBeenCalledOnce();
      });
    });

    it("gives macOS no secondary binding at all", () => {
      // ⌘J already reaches the app over a focused terminal, and the mac table
      // is frozen.
      expect(secondaryBindingsFor("mac").size).toBe(0);
      const actions = buildActions();
      const wrapper = mountShortcutHarness(actions, () => "main");
      try {
        pressCtrlJAt(window);
        expect(actions.openShell).not.toHaveBeenCalled();
      } finally {
        wrapper.unmount();
      }
    });
  });

  it("leaves WebKitGTK's inspector chord native and dispatches the Linux Create Repository chord", () => {
    for (const nav of [globalThis.navigator, window.navigator]) {
      Object.defineProperty(nav, "platform", { value: "Linux x86_64", configurable: true });
    }
    resetShortcutBindingsForTests();
    const actions = buildActions();
    const wrapper = mountShortcutHarness(actions, () => "main");

    try {
      window.dispatchEvent(new KeyboardEvent("keydown", {
        key: "I",
        code: "KeyI",
        ctrlKey: true,
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      }));
      expect(actions.createRepo).not.toHaveBeenCalled();

      window.dispatchEvent(new KeyboardEvent("keydown", {
        key: "I",
        code: "KeyI",
        ctrlKey: true,
        altKey: true,
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      }));
      expect(actions.createRepo).toHaveBeenCalledOnce();
    } finally {
      wrapper.unmount();
      for (const nav of [globalThis.navigator, window.navigator]) {
        Object.defineProperty(nav, "platform", { value: "MacIntel", configurable: true });
      }
      resetShortcutBindingsForTests();
    }
  });

  it.each([
    { key: "ArrowLeft", action: "previousPane" as const },
    { key: "ArrowRight", action: "nextPane" as const },
  ])("dispatches Option+Command+$key to $action in a pane view", ({ key, action }) => {
    const actions = buildActions();
    const wrapper = mountShortcutHarness(actions, () => "shell");

    window.dispatchEvent(new KeyboardEvent("keydown", {
      key,
      metaKey: true,
      altKey: true,
      bubbles: true,
      cancelable: true,
    }));

    expect(actions[action]).toHaveBeenCalledOnce();
    wrapper.unmount();
  });

  it("does not dispatch undo close for Command+Z", () => {
    const actions = buildActions();
    const wrapper = mountShortcutHarness(actions, () => "main");

    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "z",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    }));

    expect(actions.undoClose).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("does not expose stage approval shortcuts from the diff modal context", () => {
    const actions = buildActions();
    const wrapper = mountShortcutHarness(actions, () => "diff");

    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "s",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    }));
    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "s",
      metaKey: true,
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    }));

    expect(actions.advanceStage).toHaveBeenCalledTimes(0);
    wrapper.unmount();
  });

  it("allows preview modal shortcuts from every preview modal context", () => {
    const previewContexts: ShortcutContext[] = ["diff", "file", "shell", "tree", "graph"];
    const previewShortcuts: Array<{
      action: ActionName;
      event: { key: string; meta?: boolean; shift?: boolean };
    }> = [
      { action: "openFile", event: { key: "p", meta: true } },
      { action: "openLatestFileLink", event: { key: "l", meta: true } },
      { action: "showDiff", event: { key: "d", meta: true } },
      { action: "showCommitGraph", event: { key: "g", meta: true } },
      { action: "openShell", event: { key: "j", meta: true } },
      { action: "openShellRepoRoot", event: { key: "J", meta: true, shift: true } },
      { action: "toggleTreeExplorer", event: { key: "E", meta: true, shift: true } },
    ];

    for (const context of previewContexts) {
      for (const shortcut of previewShortcuts) {
        const actions = buildActions();
        const wrapper = mountShortcutHarness(actions, () => context);

        window.dispatchEvent(new KeyboardEvent("keydown", {
          key: shortcut.event.key,
          metaKey: shortcut.event.meta ?? false,
          shiftKey: shortcut.event.shift ?? false,
          bubbles: true,
          cancelable: true,
        }));

        expect(actions[shortcut.action], `${shortcut.action} in ${context}`).toHaveBeenCalledTimes(1);
        wrapper.unmount();
      }
    }
  });

  it("handles Escape as dismiss in the tree modal context", () => {
    const actions = buildActions();
    const wrapper = mountShortcutHarness(actions, () => "tree");

    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "Escape",
      bubbles: true,
      cancelable: true,
    }));

    expect(actions.dismiss).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });
});

describe("shortcut i18n labels", () => {
  const locales: Record<string, unknown> = { en, ja, ko };

  function lookup(messages: unknown, key: string): unknown {
    return key.split(".").reduce<unknown>((node, segment) => {
      if (typeof node !== "object" || node === null) return undefined;
      return (node as Record<string, unknown>)[segment];
    }, messages);
  }

  // Both the shortcuts modal and the command palette render these keys through
  // t() — a missing one leaks the raw "shortcuts.prevTab" string into the UI.
  it.each(Object.keys(locales))("resolves every shortcut label and group in %s", (locale) => {
    const messages = locales[locale];
    const missing = shortcuts
      .flatMap((shortcut) => [shortcut.labelKey, shortcut.groupKey])
      .filter((key) => typeof lookup(messages, key) !== "string");

    expect([...new Set(missing)]).toEqual([]);
  });
});
