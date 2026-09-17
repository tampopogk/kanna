import { createServer, type IncomingHttpHeaders, type Server } from "node:http";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import * as smokeModule from "./list-detail-back.e2e";
import {
  assertPtyTerminalFixtureAvailable,
  ensureTaskListVisible,
  REPO_CHIP_STRIP_MAX_SCROLL_STEPS,
  exerciseTaskPromptExpansion,
  exerciseActivityDismissSwipe,
  exerciseTaskPinSwipe,
  exerciseListDetailBackFromOrigin,
  inspectTerminalWebView,
  openPtyFixtureTask,
  performTaskDetailEdgeSwipeBack,
  PTY_SNAPSHOT_MIN_DECODED_BYTES,
  resolveRequiredPtyTerminalFixture,
  resolveMobileRepoChipId,
  assertPtyFixtureTaskRow,
  waitForRenderedPtyTerminal,
  waitForTaskTerminalLive
} from "./list-detail-back.e2e";
import { selectors } from "../../helpers/selectors";

describe("origin-preserving list/detail/back", () => {
  it("returns to Activity after opening a task from Activity", async () => {
    const calls: string[] = [];
    const ui = {
      selectOrigin: vi.fn(async (origin: string) => {
        calls.push(`select:${origin}`);
      }),
      openTask: vi.fn(async (taskId: string) => {
        calls.push(`open:${taskId}`);
      }),
      goBack: vi.fn(async () => {
        calls.push("back");
      }),
      assertOrigin: vi.fn(async (origin: string) => {
        calls.push(`assert:${origin}`);
      })
    };

    await exerciseListDetailBackFromOrigin(ui, "recent", "task-1");

    expect(calls).toEqual([
      "select:recent",
      "open:task-1",
      "back",
      "assert:recent"
    ]);
  });
});

interface FakeElement {
  click: ReturnType<typeof vi.fn>;
  isDisplayed?: ReturnType<typeof vi.fn>;
  isExisting: ReturnType<typeof vi.fn>;
  waitForDisplayed: ReturnType<typeof vi.fn>;
}

function createElement(exists: () => boolean, onClick?: () => void): FakeElement {
  return {
    click: vi.fn(async () => {
      onClick?.();
    }),
    isExisting: vi.fn(async () => exists()),
    waitForDisplayed: vi.fn(async () => undefined)
  };
}

interface FakeRepoChip extends FakeElement {
  isDisplayed: ReturnType<typeof vi.fn>;
}

function createRepoChip(
  exists: () => boolean,
  displayed: () => boolean,
  onClick?: () => void
): FakeRepoChip {
  return {
    ...createElement(exists, onClick),
    isDisplayed: vi.fn(async () => displayed())
  };
}

/**
 * A repo chip strip measured in scroll steps from its start. The chip is
 * displayed only while the strip sits at `chipPosition`; the strip opens at
 * `initialPosition` and, like a real ScrollView, stops at its start and at
 * `endPosition` instead of scrolling on forever. A chip positioned beyond
 * the end models one that exists but never scrolls in.
 */
function createRepoChipStrip(options: {
  chipPosition: number;
  endPosition?: number;
  exists?: boolean;
  initialPosition?: number;
  onClick?: () => void;
}) {
  const exists = options.exists ?? true;
  const endPosition =
    options.endPosition ??
    Math.max(options.chipPosition, options.initialPosition ?? 0);
  let position = options.initialPosition ?? 0;
  const chip = createRepoChip(
    () => exists,
    () => exists && position === options.chipPosition,
    options.onClick
  );
  const scrollRepoChipStrip = vi.fn(async (direction: "forward" | "backward") => {
    position = Math.min(
      endPosition,
      Math.max(0, position + (direction === "forward" ? 1 : -1))
    );
  });
  return { chip, scrollRepoChipStrip };
}

function createRepoScopedUi(
  strip: ReturnType<typeof createRepoChipStrip>,
  options: { repoChipId: string; rowsFor: () => FakeElement[] }
) {
  return {
    getBackButton: vi.fn(async () => createElement(() => false)),
    getRepoChip: vi.fn(async (repoChipId: string) => {
      if (repoChipId !== options.repoChipId) {
        throw new Error(`no repo chip renders for ${repoChipId}`);
      }
      return strip.chip;
    }),
    getTaskRows: vi.fn(async () => options.rowsFor()),
    pause: vi.fn(async () => undefined),
    scrollRepoChipStrip: strip.scrollRepoChipStrip,
    waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
      if (await condition()) {
        return;
      }

      throw new Error(options.timeoutMsg);
    })
  };
}

describe("performTaskDetailEdgeSwipeBack", () => {
  it("drags from the native iOS left edge and waits for Tasks to replace TaskDetail", async () => {
    let taskDetailVisible = true;
    let tasksVisible = false;
    const taskDetail = createElement(() => taskDetailVisible);
    const tasksScreen = {
      ...createElement(() => tasksVisible),
      isDisplayed: vi.fn(async () => tasksVisible)
    };
    const driver = {
      $: vi.fn(async (selector: string) =>
        selector === selectors.taskDetailScreen ? taskDetail : tasksScreen
      ),
      execute: vi.fn(async () => {
        taskDetailVisible = false;
        tasksVisible = true;
      }),
      getWindowSize: vi.fn(async () => ({ width: 390, height: 844 })),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        if (await condition()) {
          return;
        }
        throw new Error(options.timeoutMsg);
      })
    };

    await performTaskDetailEdgeSwipeBack(driver as never);

    expect(driver.execute).toHaveBeenCalledWith(
      "mobile: dragFromToForDuration",
      {
        duration: 0.5,
        fromX: 1,
        fromY: 422,
        toX: 293,
        toY: 422
      }
    );
    expect(driver.$).toHaveBeenCalledWith(selectors.taskDetailScreen);
    expect(driver.$).toHaveBeenCalledWith(selectors.tasksScreen);
    expect(taskDetail.isExisting).toHaveBeenCalled();
    expect(tasksScreen.isExisting).toHaveBeenCalled();
    expect(tasksScreen.isDisplayed).toHaveBeenCalled();
  });
});

describe("exerciseTaskPinSwipe", () => {
  function createPinFixtureDriver() {
    // Pinning is local to the phone now, so the fixture models the app's own
    // record and leaves the desktop's pin columns alone. The swipe commits
    // when it is released, so the drag itself is what writes the pin.
    let pinnedLocally = false;
    const row = {
      getLocation: vi.fn(async () => ({ x: 10, y: 100 })),
      getSize: vi.fn(async () => ({ width: 360, height: 90 })),
      waitForDisplayed: vi.fn(async () => undefined)
    };
    const repo = {
      click: vi.fn(async () => undefined),
      isDisplayed: vi.fn(async () => true),
      isExisting: vi.fn(async () => true)
    };
    let renderedRowIds: () => string[] = () =>
      pinnedLocally ? ["task-1", "task-2"] : ["task-2", "task-1"];
    const driver = {
      // The phone renders the hashed repo under its canonical `git:<hash>`
      // chip; the desktop-local `repo-1` chip does not exist on screen.
      $: vi.fn(async (selector: string) => {
        if (selector === "~mobile.tasks.repo.git:hash-1") return repo;
        if (selector.startsWith("~mobile.tasks.repo.")) {
          throw new Error(`no repo chip renders for ${selector}`);
        }
        return row;
      }),
      $$: vi.fn(async () =>
        renderedRowIds().map((taskId) => ({
          getAttribute: vi.fn(async (name: string) =>
            name === "name" ? `mobile.task-row.${taskId}` : null
          )
        }))
      ),
      execute: vi.fn(async () => {
        pinnedLocally = !pinnedLocally;
      }),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        if (!(await condition())) throw new Error(options.timeoutMsg);
      })
    };
    return {
      driver,
      repo,
      row,
      isPinnedLocally: () => pinnedLocally,
      renderRows: (next: () => string[]) => {
        renderedRowIds = next;
      }
    };
  }

  function createPinFetchMock(desktopPinned = false) {
    return vi.fn(async (url: string) => {
      if (url.endsWith("/v1/tasks/task-1")) {
        return {
          ok: true,
          status: 200,
          json: async () => ({ repoId: "repo-1" })
        };
      }
      if (url.endsWith("/v1/repos")) {
        return {
          ok: true,
          status: 200,
          json: async () => [
            { id: "repo-1", name: "fixture", remoteUrlHash: "hash-1" }
          ]
        };
      }
      return {
        ok: true,
        status: 200,
        json: async () => [
          { id: "task-1", repoId: "repo-1", pinned: desktopPinned },
          { id: "task-2", repoId: "repo-1", pinned: false }
        ]
      };
    });
  }

  it("commits the pin on release, lifts the row locally, and leaves the desktop alone", async () => {
    const fixture = createPinFixtureDriver();
    const { driver, repo } = fixture;
    const fetchImpl = createPinFetchMock();

    await exerciseTaskPinSwipe(
      driver as never,
      "http://127.0.0.1:48120",
      "task-1",
      fetchImpl
    );

    expect(driver.execute).toHaveBeenCalledWith(
      "mobile: dragFromToForDuration",
      {
        duration: 0.35,
        fromX: 298,
        fromY: 145,
        toX: 136,
        toY: 145
      }
    );
    expect(repo.click).toHaveBeenCalledOnce();
    expect(driver.$).toHaveBeenCalledWith("~mobile.tasks.repo.git:hash-1");
    expect(driver.$).not.toHaveBeenCalledWith("~mobile.tasks.repo.repo-1");
    // Pin, then unpin: the phone is the only place the pin lives, so the
    // journey puts it back itself — two swipes, and no tap in between.
    expect(driver.execute).toHaveBeenCalledTimes(2);
    expect(fixture.isPinnedLocally()).toBe(false);
    // No pin route is ever called.
    expect(fetchImpl).not.toHaveBeenCalledWith(
      expect.stringContaining("/actions/pin"),
      expect.anything()
    );
    expect(fetchImpl).not.toHaveBeenCalledWith(
      expect.stringContaining("/actions/unpin"),
      expect.anything()
    );
  });

  it("fails when the pinned task does not become the first rendered row", async () => {
    const fixture = createPinFixtureDriver();
    fixture.renderRows(() => ["task-2", "task-1"]);

    await expect(
      exerciseTaskPinSwipe(
        fixture.driver as never,
        "http://127.0.0.1:48120",
        "task-1",
        createPinFetchMock()
      )
    ).rejects.toThrow(
      "Expected the pinned task to render as the first row of its repo list"
    );
    // The phone is still put back on the way out.
    expect(fixture.driver.execute).toHaveBeenCalledTimes(2);
  });

  it("fails when the local pin reaches the desktop's own pin state", async () => {
    const fixture = createPinFixtureDriver();

    await expect(
      exerciseTaskPinSwipe(
        fixture.driver as never,
        "http://127.0.0.1:48120",
        "task-1",
        // The desktop reports the task pinned only after the swipe.
        vi.fn(async (url: string) => {
          if (url.endsWith("/v1/tasks/task-1")) {
            return {
              ok: true,
              status: 200,
              json: async () => ({ repoId: "repo-1" })
            };
          }
          if (url.endsWith("/v1/repos")) {
            return {
              ok: true,
              status: 200,
              json: async () => [
                { id: "repo-1", name: "fixture", remoteUrlHash: "hash-1" }
              ]
            };
          }
          return {
            ok: true,
            status: 200,
            json: async () => [
              {
                id: "task-1",
                repoId: "repo-1",
                pinned: fixture.isPinnedLocally()
              },
              { id: "task-2", repoId: "repo-1", pinned: false }
            ]
          };
        })
      )
    ).rejects.toThrow("leave the desktop's pin state alone");
  });
});

describe("exerciseActivityDismissSwipe", () => {
  it("hides the row on the phone, leaves the desktop unread, and observes a later revision", async () => {
    let activity = "idle";
    let dismissedRevision: string | null = null;
    const rowVisible = () => activity === "unread" && dismissedRevision !== activityRevision();
    let revision = 0;
    const activityRevision = () => String(revision);
    const row = {
      getLocation: vi.fn(async () => ({ x: 10, y: 100 })),
      getSize: vi.fn(async () => ({ width: 360, height: 90 })),
      isExisting: vi.fn(async () => rowVisible()),
      waitForDisplayed: vi.fn(async () => undefined)
    };
    const activityTab = { click: vi.fn(async () => undefined) };
    const screen = { waitForDisplayed: vi.fn(async () => undefined) };
    const driver = {
      $: vi.fn(async (selector: string) => {
        if (selector === selectors.recentTab) return activityTab;
        if (selector === selectors.recentScreen) return screen;
        return row;
      }),
      // The released swipe is the dismissal: it records the generation it hid,
      // and the desktop keeps reporting the task unread.
      execute: vi.fn(async () => {
        dismissedRevision = activityRevision();
      }),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        if (!(await condition())) throw new Error(options.timeoutMsg);
      })
    };
    const fetchImpl = vi.fn(async (url: string, init?: { body?: string }) => {
      if (url.endsWith("/actions/runtime-status")) {
        const body = JSON.parse(init?.body ?? "{}") as { status?: string };
        if (body.status === "busy") {
          activity = "working";
        } else {
          activity = "unread";
          revision += 1;
        }
        return { ok: true, status: 200, json: async () => ({}) };
      }
      return {
        ok: true,
        status: 200,
        json: async () => [{ id: "task-1", activity }]
      };
    });

    await exerciseActivityDismissSwipe(
      driver as never,
      "http://127.0.0.1:48120",
      "task-1",
      fetchImpl
    );

    expect(driver.execute).toHaveBeenCalledWith(
      "mobile: dragFromToForDuration",
      {
        duration: 0.35,
        fromX: 298,
        fromY: 145,
        toX: 136,
        toY: 145
      }
    );
    expect(driver.execute).toHaveBeenCalledOnce();
    // Nothing marked the task read: only the phone stopped showing it.
    expect(activity).toBe("unread");
    expect(fetchImpl).not.toHaveBeenCalledWith(
      expect.stringContaining("/actions/mark-read"),
      expect.anything()
    );
    expect(row.waitForDisplayed).toHaveBeenCalledTimes(2);
  });
});

describe("ensureTaskListVisible", () => {
  it("backs out of persisted task detail before waiting for task rows", async () => {
    let taskDetailVisible = true;
    const backButton = createElement(
      () => taskDetailVisible,
      () => {
        taskDetailVisible = false;
      }
    );
    const ui = {
      getBackButton: vi.fn(async () => backButton),
      getTaskRows: vi.fn(async () => (taskDetailVisible ? [] : [createElement(() => true)])),
      pause: vi.fn(async () => undefined),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        for (let index = 0; index < 3; index += 1) {
          if (await condition()) {
            return;
          }
        }

        throw new Error(options.timeoutMsg);
      })
    };

    await ensureTaskListVisible(ui);

    expect(backButton.click).toHaveBeenCalledTimes(1);
    expect(ui.pause).toHaveBeenCalledWith(500);
  });

  it("waits for task rows without navigating when already on the task list", async () => {
    const backButton = createElement(() => false);
    const ui = {
      getBackButton: vi.fn(async () => backButton),
      getTaskRows: vi.fn(async () => [createElement(() => true)]),
      pause: vi.fn(async () => undefined),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        if (await condition()) {
          return;
        }

        throw new Error(options.timeoutMsg);
      })
    };

    await ensureTaskListVisible(ui);

    expect(backButton.click).not.toHaveBeenCalled();
    expect(ui.pause).not.toHaveBeenCalled();
  });

  it("selects the fixture's repo chip before waiting for rows, because the auto-selected repo may hold none", async () => {
    // The app auto-selects the desktop's most recently opened repo. Here that
    // repo has no open task: rows exist only once the fixture repo is chosen.
    let selectedRepoChipId: string | null = null;
    const strip = createRepoChipStrip({
      chipPosition: 0,
      onClick: () => {
        selectedRepoChipId = "git:hash-fixture";
      }
    });
    const ui = createRepoScopedUi(strip, {
      repoChipId: "git:hash-fixture",
      rowsFor: () =>
        selectedRepoChipId === "git:hash-fixture" ? [createElement(() => true)] : []
    });

    await ensureTaskListVisible(ui, { repoChipId: "git:hash-fixture" });

    expect(ui.getRepoChip).toHaveBeenCalledWith("git:hash-fixture");
    expect(strip.chip.isExisting).toHaveBeenCalled();
    expect(strip.chip.isDisplayed).toHaveBeenCalled();
    // A chip already on screen is clicked as is: the strip never moves.
    expect(strip.scrollRepoChipStrip).not.toHaveBeenCalled();
    expect(strip.chip.click).toHaveBeenCalledTimes(1);
    expect(ui.getTaskRows).toHaveBeenCalled();
  });

  it("scrolls the repo chip strip until an off-screen chip is displayed, then clicks it", async () => {
    // Five repositories on the desktop put the fixture's chip fourth in the
    // strip, past an iPhone's right edge: it exists, but is not displayed
    // until the strip has moved. This is the production QA failure of
    // 2026-09-17, where the chip was found and then waited on forever.
    let clicked = false;
    const strip = createRepoChipStrip({
      chipPosition: 2,
      onClick: () => {
        clicked = true;
      }
    });
    const ui = createRepoScopedUi(strip, {
      repoChipId: "git:hash-fixture",
      rowsFor: () => (clicked ? [createElement(() => true)] : [])
    });

    await ensureTaskListVisible(ui, { repoChipId: "git:hash-fixture" });

    expect(strip.scrollRepoChipStrip.mock.calls).toEqual([["forward"], ["forward"]]);
    expect(ui.pause).toHaveBeenCalledWith(500);
    expect(strip.chip.click).toHaveBeenCalledTimes(1);
    // The click lands only once the chip is on screen, never before a scroll.
    const lastScrollOrder = Math.max(
      ...strip.scrollRepoChipStrip.mock.invocationCallOrder
    );
    expect(strip.chip.click.mock.invocationCallOrder[0]).toBeGreaterThan(lastScrollOrder);
    expect(ui.getTaskRows).toHaveBeenCalled();
  });

  it("scrolls back toward the start when the chip lies behind the strip's current position", async () => {
    // An earlier selection left the strip two steps in, with the chip at the
    // start. Forward steps pile up against the strip's end first; the chip
    // surfaces on the backward pass once the strip has returned to its start.
    const strip = createRepoChipStrip({
      chipPosition: 0,
      endPosition: 3,
      initialPosition: 2
    });
    const ui = createRepoScopedUi(strip, {
      repoChipId: "git:hash-fixture",
      rowsFor: () => [createElement(() => true)]
    });

    await ensureTaskListVisible(ui, { repoChipId: "git:hash-fixture" });

    expect(strip.scrollRepoChipStrip.mock.calls).toEqual([
      ...Array.from({ length: REPO_CHIP_STRIP_MAX_SCROLL_STEPS }, () => ["forward"]),
      ["backward"],
      ["backward"],
      ["backward"]
    ]);
    expect(strip.chip.click).toHaveBeenCalledTimes(1);
  });

  it("fails naming the chip when it exists but never scrolls into view", async () => {
    // The strip ends three steps in and the chip is never at any position
    // the strip can reach, so both passes run out against its ends.
    const strip = createRepoChipStrip({ chipPosition: 4, endPosition: 3 });
    const ui = createRepoScopedUi(strip, {
      repoChipId: "git:hash-fixture",
      rowsFor: () => [createElement(() => true)]
    });

    await expect(
      ensureTaskListVisible(ui, { repoChipId: "git:hash-fixture" })
    ).rejects.toThrow(
      "Repo chip git:hash-fixture exists but never became displayed after " +
        `scrolling the repo chip strip ${REPO_CHIP_STRIP_MAX_SCROLL_STEPS} ` +
        `steps forward and ${REPO_CHIP_STRIP_MAX_SCROLL_STEPS} steps back`
    );
    // The scroll is bounded: one full pass each way, then the failure.
    expect(strip.scrollRepoChipStrip).toHaveBeenCalledTimes(
      REPO_CHIP_STRIP_MAX_SCROLL_STEPS * 2
    );
    expect(strip.chip.click).not.toHaveBeenCalled();
    expect(ui.getTaskRows).not.toHaveBeenCalled();
  });

  it("fails on existence, without scrolling, when the chip is absent entirely", async () => {
    // A wrong id or a repository the phone does not list is a different
    // failure from an off-screen chip, and scrolling must not blur the two.
    const strip = createRepoChipStrip({ chipPosition: 0, exists: false });
    const ui = createRepoScopedUi(strip, {
      repoChipId: "git:hash-fixture",
      rowsFor: () => [createElement(() => true)]
    });

    await expect(
      ensureTaskListVisible(ui, { repoChipId: "git:hash-fixture" })
    ).rejects.toThrow(
      "Repo chip git:hash-fixture never rendered: the repository is absent " +
        "from the phone's chip strip or the chip id is wrong"
    );
    expect(ui.waitUntil).toHaveBeenCalledWith(
      expect.any(Function),
      expect.objectContaining({ timeout: 30_000 })
    );
    expect(strip.scrollRepoChipStrip).not.toHaveBeenCalled();
    expect(strip.chip.isDisplayed).not.toHaveBeenCalled();
    expect(strip.chip.click).not.toHaveBeenCalled();
  });

  it("still fails when the fixture repo itself shows no rows", async () => {
    const strip = createRepoChipStrip({ chipPosition: 0 });
    const ui = createRepoScopedUi(strip, {
      repoChipId: "git:hash-fixture",
      rowsFor: () => []
    });

    await expect(
      ensureTaskListVisible(ui, { repoChipId: "git:hash-fixture" })
    ).rejects.toThrow("Expected at least one task row in the mobile task list");
  });
});

describe("resolveMobileRepoChipId", () => {
  function reposFetch(repos: unknown) {
    return vi.fn(async () => ({ ok: true, status: 200, json: async () => repos }));
  }

  it("maps a hashed desktop-local repo id to the canonical git:<hash> chip the phone renders", async () => {
    const fetchImpl = reposFetch([
      { id: "repo-fixture", name: "Disposable acceptance", remoteUrlHash: "hash-fixture" },
      { id: "ca99c7ee", name: "kanna", remoteUrlHash: "hash-kanna" }
    ]);

    await expect(
      resolveMobileRepoChipId("http://127.0.0.1:48120", "ca99c7ee", fetchImpl)
    ).resolves.toBe("git:hash-kanna");
    expect(fetchImpl).toHaveBeenCalledWith("http://127.0.0.1:48120/v1/repos");
  });

  it("keeps the desktop-local id for a repo without a remote URL hash", async () => {
    await expect(
      resolveMobileRepoChipId(
        "http://127.0.0.1:48120",
        "repo-local",
        reposFetch([{ id: "repo-local", name: "local only" }])
      )
    ).resolves.toBe("repo-local");
  });

  it("fails when the desktop does not list the fixture's repo", async () => {
    await expect(
      resolveMobileRepoChipId(
        "http://127.0.0.1:48120",
        "repo-missing",
        reposFetch([{ id: "repo-other", name: "other", remoteUrlHash: "hash-other" }])
      )
    ).rejects.toThrow("Repository repo-missing is not listed");
  });

  it("fails when the repository list cannot be read", async () => {
    await expect(
      resolveMobileRepoChipId(
        "http://127.0.0.1:48120",
        "repo-1",
        vi.fn(async () => ({ ok: false, status: 403, json: async () => null }))
      )
    ).rejects.toThrow("Could not list repositories");
  });
});

describe("PTY fixture selection", () => {
  it("requires a known live PTY fixture instead of opening an arbitrary task row", () => {
    expect(() => resolveRequiredPtyTerminalFixture({})).toThrow(
      "KANNA_E2E_PTY_TASK_ID is required"
    );
  });

  it("rejects a fixture task that is not a PTY task", async () => {
    const fetchImpl = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => ({
        id: "task-agent",
        agentType: "agent",
        closedAt: null
      })
    }));

    await expect(
      assertPtyTerminalFixtureAvailable(
        "http://127.0.0.1:48120",
        {
          taskId: "task-agent",
          sentinel: "Kanna PTY sentinel",
          expectedCols: 80,
          expectedRows: 48,
          minDecodedBytes: PTY_SNAPSHOT_MIN_DECODED_BYTES
        },
        fetchImpl
      )
    ).rejects.toThrow("expected a live PTY task");
  });

  it("loads the distinct renamed title and multiline prompt end from the real task API fixture", async () => {
    const fetchImpl = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => ({
        id: "task-pty",
        repoId: "repo-pty",
        title: "Short renamed task",
        prompt:
          `${"Detailed canonical prompt line. ".repeat(12)}\nMOBILE_PROMPT_END_SENTINEL`,
        agentType: "pty",
        closedAt: null
      })
    }));

    await expect(
      assertPtyTerminalFixtureAvailable(
        "http://127.0.0.1:48120",
        {
          taskId: "task-pty",
          sentinel: "Kanna PTY sentinel",
          expectedCols: 80,
          expectedRows: 48,
          minDecodedBytes: PTY_SNAPSHOT_MIN_DECODED_BYTES
        },
        fetchImpl
      )
    ).resolves.toEqual({
      expectedTitle: "Short renamed task",
      promptEndSentinel: "MOBILE_PROMPT_END_SENTINEL",
      repoId: "repo-pty",
      taskId: "task-pty"
    });
  });

  it("rejects a fixture task that does not report its repository", async () => {
    const fetchImpl = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => ({
        id: "task-pty",
        title: "Short renamed task",
        prompt:
          `${"Detailed canonical prompt line. ".repeat(12)}\nMOBILE_PROMPT_END_SENTINEL`,
        agentType: "pty",
        closedAt: null
      })
    }));

    await expect(
      assertPtyTerminalFixtureAvailable(
        "http://127.0.0.1:48120",
        {
          taskId: "task-pty",
          sentinel: "Kanna PTY sentinel",
          expectedCols: 80,
          expectedRows: 48,
          minDecodedBytes: PTY_SNAPSHOT_MIN_DECODED_BYTES
        },
        fetchImpl
      )
    ).rejects.toThrow("did not expose a repository id");
  });

  it("opens the exact fixture task row by id and never clicks the first arbitrary row", async () => {
    const firstRow = createElement(() => true);
    const fixtureRow = createElement(() => true);
    const ui = {
      getTaskRowById: vi.fn(async (taskId: string) =>
        taskId === "task-fixture" ? fixtureRow : firstRow
      ),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        if (await condition()) {
          return;
        }

        throw new Error(options.timeoutMsg);
      })
    };

    await openPtyFixtureTask(ui, "task-fixture");

    expect(ui.getTaskRowById).toHaveBeenCalledWith("task-fixture");
    expect(fixtureRow.click).toHaveBeenCalledTimes(1);
    expect(firstRow.click).not.toHaveBeenCalled();
  });
});

describe("task prompt expansion journey", () => {
  it("copies the complete task ID without collapsing, then preserves both collapse paths", async () => {
    const exerciseTaskPromptExpansion = (
      smokeModule as typeof smokeModule & {
        exerciseTaskPromptExpansion?: (
          ui: Record<string, unknown>,
          fixture: {
            expectedTitle: string;
            promptEndSentinel: string;
            taskId: string;
          }
        ) => Promise<void>;
      }
    ).exerciseTaskPromptExpansion;
    expect(exerciseTaskPromptExpansion).toBeTypeOf("function");
    if (!exerciseTaskPromptExpansion) return;

    const taskId = "019f6c9d6ed40000000120e4307b4591";
    let expanded = false;
    let copyMenuVisible = false;
    let clipboard = "preexisting clipboard";
    const titleButton = {
      ...createElement(() => true, () => {
        expanded = !expanded;
      }),
      getText: vi.fn(async () => "Short renamed task")
    };
    const expandedPrompt = {
      ...createElement(() => expanded),
      getText: vi.fn(async () =>
        expanded
          ? "First prompt line\nSecond detailed line\nPROMPT_END_SENTINEL"
          : ""
      )
    };
    const collapsedTaskId = {
      ...createElement(() => !expanded),
      isDisplayed: vi.fn(async () => !expanded),
      getText: vi.fn(async () => (expanded ? "" : taskId))
    };
    const expandedTaskId = {
      ...createElement(() => expanded),
      isDisplayed: vi.fn(async () => expanded),
      getText: vi.fn(async () => (expanded ? taskId : "")),
      longPress: vi.fn(async ({ duration }: { duration: number }) => {
        if (duration === 1_500) {
          copyMenuVisible = true;
        }
      })
    };
    const copyMenuItem = {
      ...createElement(
        () => copyMenuVisible,
        () => {
          clipboard = taskId;
          copyMenuVisible = false;
        }
      ),
      isDisplayed: vi.fn(async () => copyMenuVisible)
    };
    const dismissLayer = createElement(
      () => expanded,
      () => {
        expanded = false;
      }
    );
    const backButton = createElement(() => true);
    const backDisplayed = vi.fn(async () => true);
    const backEnabled = vi.fn(async () => true);
    Object.assign(backButton, {
      isDisplayed: backDisplayed,
      isEnabled: backEnabled,
      getSize: vi.fn(async () => ({ height: 48, width: 48 }))
    });
    const ui = {
      getBackButton: vi.fn(async () => backButton),
      getClipboard: vi.fn(async () =>
        Buffer.from(clipboard, "utf8").toString("base64")
      ),
      setClipboard: vi.fn(async (encodedClipboard: string) => {
        clipboard = Buffer.from(encodedClipboard, "base64").toString("utf8");
      }),
      getCollapsedTaskId: vi.fn(async () => collapsedTaskId),
      getCollapsedTitle: vi.fn(async () => titleButton),
      getCopyMenuItem: vi.fn(async () => copyMenuItem),
      getExpandedPrompt: vi.fn(async () => expandedPrompt),
      getExpandedTaskId: vi.fn(async () => expandedTaskId),
      getTitleButton: vi.fn(async () => titleButton),
      getTitleDismissLayer: vi.fn(async () => dismissLayer),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        for (let index = 0; index < 3; index += 1) {
          if (await condition()) return;
        }
        throw new Error(options.timeoutMsg);
      })
    };

    await exerciseTaskPromptExpansion(ui, {
      expectedTitle: "Short renamed task",
      promptEndSentinel: "PROMPT_END_SENTINEL",
      taskId
    });

    expect(titleButton.click).toHaveBeenCalledTimes(3);
    expect(collapsedTaskId.isDisplayed).toHaveBeenCalled();
    expect(collapsedTaskId.getText).toHaveBeenCalled();
    expect(expandedPrompt.getText).toHaveBeenCalled();
    expect(expandedTaskId.isDisplayed).toHaveBeenCalled();
    expect(expandedTaskId.getText).toHaveBeenCalled();
    expect(expandedTaskId.longPress).toHaveBeenCalledWith({ duration: 1_500 });
    expect(copyMenuItem.click).toHaveBeenCalledTimes(1);
    expect(ui.getClipboard).toHaveBeenCalled();
    expect(ui.setClipboard).toHaveBeenCalledTimes(2);
    expect(clipboard).toBe("preexisting clipboard");
    expect(dismissLayer.click).toHaveBeenCalledTimes(1);
    expect(await backButton.isExisting()).toBe(true);
    expect(backDisplayed).toHaveBeenCalledTimes(1);
    expect(backEnabled).toHaveBeenCalledTimes(1);
    expect(await expandedPrompt.isExisting()).toBe(false);
    expect(await expandedTaskId.isExisting()).toBe(false);
    expect(expanded).toBe(false);
  });

  it("fails when an existing collapsed task ID is off-screen", async () => {
    const taskId = "019f6c9d6ed40000000120e4307b4591";
    const collapsedTaskId = {
      ...createElement(() => true),
      getText: vi.fn(async () => taskId),
      isDisplayed: vi.fn(async () => false)
    };
    const ui = {
      getCollapsedTaskId: vi.fn(async () => collapsedTaskId),
      getCollapsedTitle: vi.fn(async () => ({
        ...createElement(() => true),
        getText: vi.fn(async () => "Task title")
      })),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        if (!(await condition())) throw new Error(options.timeoutMsg);
      })
    };

    await expect(
      exerciseTaskPromptExpansion(ui as never, {
        expectedTitle: "Task title",
        promptEndSentinel: "PROMPT_END_SENTINEL",
        taskId
      })
    ).rejects.toThrow(`Expected complete collapsed task ID ${JSON.stringify(taskId)}`);
    expect(collapsedTaskId.isDisplayed).toHaveBeenCalled();
  });

  it("fails when an existing expanded task ID is off-screen immediately after expansion", async () => {
    const taskId = "019f6c9d6ed40000000120e4307b4591";
    let expanded = false;
    const titleButton = createElement(() => true, () => {
      expanded = true;
    });
    const expandedTaskId = {
      ...createElement(() => expanded),
      getText: vi.fn(async () => taskId),
      isDisplayed: vi.fn(async () => false)
    };
    const ui = {
      getCollapsedTaskId: vi.fn(async () => ({
        ...createElement(() => !expanded),
        getText: vi.fn(async () => taskId),
        isDisplayed: vi.fn(async () => !expanded)
      })),
      getCollapsedTitle: vi.fn(async () => ({
        ...createElement(() => true),
        getText: vi.fn(async () => "Task title")
      })),
      getExpandedPrompt: vi.fn(async () => ({
        ...createElement(() => expanded),
        getText: vi.fn(async () => "PROMPT_END_SENTINEL")
      })),
      getExpandedTaskId: vi.fn(async () => expandedTaskId),
      getTitleButton: vi.fn(async () => titleButton),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        if (!(await condition())) throw new Error(options.timeoutMsg);
      })
    };

    await expect(
      exerciseTaskPromptExpansion(ui as never, {
        expectedTitle: "Task title",
        promptEndSentinel: "PROMPT_END_SENTINEL",
        taskId
      })
    ).rejects.toThrow(`Expected complete expanded task ID ${JSON.stringify(taskId)}`);
    expect(expandedTaskId.isDisplayed).toHaveBeenCalled();
  });
});

describe("assertPtyFixtureTaskRow", () => {
  const fixture = {
    expectedTitle: "A task title that remains legible",
    promptEndSentinel: "PROMPT_END_SENTINEL",
    taskId: "019f6c9d6ed40000000120e4307b4591"
  };

  function createRowUi(displayed: boolean) {
    const row = {
      ...createElement(() => true),
      getText: vi.fn(async () => `${fixture.expectedTitle} Task ID ${fixture.taskId}`),
      isDisplayed: vi.fn(async () => displayed)
    };
    return {
      row,
      ui: {
        getTaskRowById: vi.fn(async () => row),
        waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
          if (!(await condition())) throw new Error(options.timeoutMsg);
        })
      }
    };
  }

  it("accepts a displayed fixture row with both title and ID", async () => {
    const { row, ui } = createRowUi(true);

    await assertPtyFixtureTaskRow(ui as never, fixture);

    expect(row.isDisplayed).toHaveBeenCalled();
  });

  it("fails when the fixture row exists but is off-screen", async () => {
    const { row, ui } = createRowUi(false);

    await expect(assertPtyFixtureTaskRow(ui as never, fixture)).rejects.toThrow(
      "Expected fixture row to show"
    );
    expect(row.isDisplayed).toHaveBeenCalled();
  });
});

describe("waitForTaskTerminalLive", () => {
  it("does not treat an absent overlay as live before task detail mounts", async () => {
    const taskDetail = createElement(() => false);
    const agentMessageView = createElement(() => false);
    const overlay = createElement(() => false);
    const ui = {
      getTaskDetailScreen: vi.fn(async () => taskDetail),
      getAgentMessageView: vi.fn(async () => agentMessageView),
      getTerminalOverlay: vi.fn(async () => overlay),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        if (await condition()) {
          return;
        }

        throw new Error(options.timeoutMsg);
      })
    };

    await expect(waitForTaskTerminalLive(ui)).rejects.toThrow(
      "Expected the mobile task terminal"
    );
  });

  it("waits for the terminal overlay to disappear after opening a task", async () => {
    let overlayVisible = true;
    const taskDetail = createElement(() => true);
    const agentMessageView = createElement(() => false);
    const overlay = createElement(() => overlayVisible);
    const ui = {
      getTaskDetailScreen: vi.fn(async () => taskDetail),
      getAgentMessageView: vi.fn(async () => agentMessageView),
      getTerminalOverlay: vi.fn(async () => overlay),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        if (!(await condition())) {
          overlayVisible = false;
        }

        if (await condition()) {
          return;
        }

        throw new Error(options.timeoutMsg);
      })
    };

    await waitForTaskTerminalLive(ui);

    expect(ui.getTerminalOverlay).toHaveBeenCalled();
  });

  it("does not accept an agent message view until its stream is ready", async () => {
    const taskDetail = createElement(() => true);
    const agentMessageView = createElement(() => true);
    const agentMessageReady = createElement(() => false);
    const overlay = createElement(() => false);
    const ui = {
      getTaskDetailScreen: vi.fn(async () => taskDetail),
      getAgentMessageView: vi.fn(async () => agentMessageView),
      getAgentMessageReady: vi.fn(async () => agentMessageReady),
      getTerminalOverlay: vi.fn(async () => overlay),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        await condition();
        throw new Error(options.timeoutMsg);
      })
    };

    await expect(waitForTaskTerminalLive(ui)).rejects.toThrow(
      "Expected the mobile task terminal"
    );

    expect(ui.getAgentMessageReady).toHaveBeenCalled();
    expect(ui.getTerminalOverlay).not.toHaveBeenCalled();
  });

  it("accepts an agent message view after its stream is ready", async () => {
    const taskDetail = createElement(() => true);
    const agentMessageView = createElement(() => true);
    const agentMessageReady = createElement(() => true);
    const overlay = createElement(() => true);
    const ui = {
      getTaskDetailScreen: vi.fn(async () => taskDetail),
      getAgentMessageView: vi.fn(async () => agentMessageView),
      getAgentMessageReady: vi.fn(async () => agentMessageReady),
      getTerminalOverlay: vi.fn(async () => overlay),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        if (await condition()) {
          return;
        }

        throw new Error(options.timeoutMsg);
      })
    };

    await waitForTaskTerminalLive(ui);

    expect(ui.getAgentMessageReady).toHaveBeenCalled();
    expect(ui.getTerminalOverlay).not.toHaveBeenCalled();
  });
});

describe("inspectTerminalWebView", () => {
  it("uses the native terminal diagnostic bridge without switching WebView context", async () => {
    const switchContext = vi.fn(async () => undefined);
    const inspection = {
      byteCount: 1024,
      cols: 80,
      frameCount: 3,
      rows: 24,
      text: "SCRIPT_READY"
    };

    await expect(inspectTerminalWebView({
      execute: vi.fn(),
      getContexts: vi.fn(async () => ["NATIVE_APP", "WEBVIEW_build.kanna.app.dev"]),
      getNativeInspection: vi.fn(async () => JSON.stringify(inspection)),
      switchContext
    })).resolves.toEqual({ kind: "rendered", ...inspection });
    expect(switchContext).not.toHaveBeenCalled();
  });

  it("reports why WebView terminal inspection is unavailable", async () => {
    await expect(
      inspectTerminalWebView({
        execute: vi.fn()
      })
    ).resolves.toEqual({
      kind: "unavailable",
      reason: "Appium driver does not expose WebView context APIs"
    });
  });

  it("switches into the WebView context and restores the previous native context", async () => {
    const switchContext = vi.fn(async () => undefined);
    const execute = vi.fn(async () => ({
      kind: "rendered" as const,
      byteCount: 1024,
      cols: 132,
      frameCount: 1,
      rows: 43,
      text: "large snapshot"
    }));

    const result = await inspectTerminalWebView({
      execute,
      getContext: vi.fn(async () => "NATIVE_APP"),
      getContexts: vi.fn(async () => ["NATIVE_APP", "WEBVIEW_build.kanna.app.dev"]),
      switchContext
    });

    expect(result).toEqual({
      kind: "rendered",
      byteCount: 1024,
      cols: 132,
      frameCount: 1,
      rows: 43,
      text: "large snapshot"
    });
    expect(switchContext).toHaveBeenNthCalledWith(1, "WEBVIEW_build.kanna.app.dev");
    expect(switchContext).toHaveBeenNthCalledWith(2, "NATIVE_APP");
    expect(execute).toHaveBeenCalledTimes(1);
  });
});

describe("waitForRenderedPtyTerminal", () => {
  const expectation = {
    sentinel: "Kanna PTY sentinel",
    expectedCols: 80,
    expectedRows: 24,
    minDecodedBytes: PTY_SNAPSHOT_MIN_DECODED_BYTES
  };

  it("waits until WebView terminal output and desktop dimensions are rendered", async () => {
    const agentMessageView = createElement(() => false);
    const inspections = [
      { kind: "unavailable" as const, reason: "No WEBVIEW context was available" },
      {
        kind: "rendered" as const,
        byteCount: PTY_SNAPSHOT_MIN_DECODED_BYTES,
        cols: 80,
        frameCount: 1,
        rows: 24,
        text: "large snapshot\nKanna PTY sentinel"
      }
    ];
    const ui = {
      getAgentMessageView: vi.fn(async () => agentMessageView),
      inspectTerminalWebView: vi.fn(async () => inspections.shift() ?? inspections[0]),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        for (let index = 0; index < 3; index += 1) {
          if (await condition()) {
            return;
          }
        }

        throw new Error(options.timeoutMsg);
      })
    };

    await waitForRenderedPtyTerminal(ui, expectation);

    expect(ui.inspectTerminalWebView).toHaveBeenCalledTimes(2);
  });

  it("fails on tiny decoded byte counts that would not catch the old mid-base64 slice", async () => {
    const agentMessageView = createElement(() => false);
    const ui = {
      getAgentMessageView: vi.fn(async () => agentMessageView),
      inspectTerminalWebView: vi.fn(async () => ({
        kind: "rendered" as const,
        byteCount: 9_000,
        cols: 80,
        frameCount: 1,
        rows: 24,
        text: "Kanna PTY sentinel"
      })),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        await condition();
        throw new Error(options.timeoutMsg);
      })
    };

    await expect(waitForRenderedPtyTerminal(ui, expectation)).rejects.toThrow(
      "byteCount"
    );
  });

  it("fails when the rendered terminal text is blank", async () => {
    const agentMessageView = createElement(() => false);
    const ui = {
      getAgentMessageView: vi.fn(async () => agentMessageView),
      inspectTerminalWebView: vi.fn(async () => ({
        kind: "rendered" as const,
        byteCount: PTY_SNAPSHOT_MIN_DECODED_BYTES,
        cols: 80,
        frameCount: 1,
        rows: 24,
        text: "   "
      })),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        await condition();
        throw new Error(options.timeoutMsg);
      })
    };

    await expect(waitForRenderedPtyTerminal(ui, expectation)).rejects.toThrow(
      "terminal text"
    );
  });

  it("fails when the expected terminal sentinel is missing", async () => {
    const agentMessageView = createElement(() => false);
    const ui = {
      getAgentMessageView: vi.fn(async () => agentMessageView),
      inspectTerminalWebView: vi.fn(async () => ({
        kind: "rendered" as const,
        byteCount: PTY_SNAPSHOT_MIN_DECODED_BYTES,
        cols: 80,
        frameCount: 1,
        rows: 24,
        text: "large snapshot without the expected marker"
      })),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        await condition();
        throw new Error(options.timeoutMsg);
      })
    };

    await expect(waitForRenderedPtyTerminal(ui, expectation)).rejects.toThrow(
      "sentinel"
    );
  });

  it("fails with the last WebView inspection reason when Appium cannot inspect the terminal", async () => {
    const agentMessageView = createElement(() => false);
    const ui = {
      getAgentMessageView: vi.fn(async () => agentMessageView),
      inspectTerminalWebView: vi.fn(async () => ({
        kind: "unavailable" as const,
        reason: "No WEBVIEW context was available"
      })),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        await condition();
        throw new Error(options.timeoutMsg);
      })
    };

    await expect(waitForRenderedPtyTerminal(ui, expectation)).rejects.toThrow(
      "No WEBVIEW context was available"
    );
  });
});

/**
 * The smoke's own reads and actions against `kanna-server` must go out as a
 * local process. `lan_trust.rs` classifies any request carrying `Origin` or a
 * `Sec-Fetch-*` header as browser-originated and refuses it 403 without the
 * desktop credential — and Node 24's global `fetch` (undici) stamps
 * `Sec-Fetch-Mode` on every request it sends. The release-owned simulator
 * smoke hit exactly that: `curl` got 200, the harness got 403.
 *
 * A mocked `fetchImpl` cannot see this, so these tests run the exported
 * helpers with *no* injected client against a real loopback server that
 * applies the same classification rule.
 */
describe("smoke local reads use local-process authority", () => {
  const FIXTURE = {
    taskId: "task-1",
    sentinel: "Kanna PTY sentinel",
    expectedCols: 80,
    expectedRows: 48,
    minDecodedBytes: PTY_SNAPSHOT_MIN_DECODED_BYTES
  };
  const BROWSER_HEADERS = [
    "origin",
    "sec-fetch-mode",
    "sec-fetch-site",
    "sec-fetch-dest",
    "sec-fetch-user"
  ];

  interface ReceivedRequest {
    method: string;
    url: string;
    headers: IncomingHttpHeaders;
    classifiedAsBrowser: boolean;
  }

  let server: Server;
  let baseUrl = "";
  let received: ReceivedRequest[] = [];
  let activity = "idle";

  beforeAll(async () => {
    server = createServer((request, response) => {
      const chunks: Buffer[] = [];
      request.on("data", (chunk: Buffer) => chunks.push(chunk));
      request.on("end", () => {
        const classifiedAsBrowser = BROWSER_HEADERS.some(
          (header) => request.headers[header] !== undefined
        );
        received.push({
          method: request.method ?? "",
          url: request.url ?? "",
          headers: request.headers,
          classifiedAsBrowser
        });
        // The same rule as lan_trust.rs: a browser-classified request without
        // the local control credential is refused, whatever its path.
        if (classifiedAsBrowser) {
          response.statusCode = 403;
          response.end();
          return;
        }
        response.setHeader("content-type", "application/json");
        const url = request.url ?? "";
        if (url === "/v1/tasks/task-1") {
          response.end(
            JSON.stringify({
              id: "task-1",
              repoId: "repo-1",
              title: "Fixture task",
              prompt: `${"Fixture prompt line. ".repeat(20)}\nMOBILE_PROMPT_END_SENTINEL`,
              agentType: "pty",
              closedAt: null
            })
          );
        } else if (url === "/v1/repos") {
          response.end(
            JSON.stringify([
              { id: "repo-1", name: "fixture", remoteUrlHash: "hash-1" }
            ])
          );
        } else if (url === "/v1/repos/repo-1/tasks") {
          response.end(
            JSON.stringify([
              { id: "task-1", repoId: "repo-1", pinned: false, pinOrder: null },
              { id: "task-2", repoId: "repo-1", pinned: false, pinOrder: null }
            ])
          );
        } else if (url === "/v1/tasks/task-1/actions/runtime-status") {
          const body = JSON.parse(Buffer.concat(chunks).toString("utf8")) as {
            status?: string;
          };
          if (body.status === "idle") activity = "unread";
          response.statusCode = 204;
          response.end();
        } else if (url === "/v1/tasks/recent") {
          response.end(JSON.stringify([{ id: "task-1", activity }]));
        } else {
          response.statusCode = 404;
          response.end();
        }
      });
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("no test server port");
    baseUrl = `http://127.0.0.1:${address.port}`;
  });

  afterAll(async () => {
    await new Promise<void>((resolve, reject) => {
      server.close((error) => (error ? reject(error) : resolve()));
    });
  });

  function expectLocalProcessRequests(): void {
    expect(received.length).toBeGreaterThan(0);
    for (const call of received) {
      expect(
        call.classifiedAsBrowser,
        `${call.method} ${call.url} carried fetch metadata and would be refused 403`
      ).toBe(false);
    }
  }

  it("reads the PTY fixture as a local process by default", async () => {
    received = [];

    await expect(assertPtyTerminalFixtureAvailable(baseUrl, FIXTURE)).resolves.toEqual({
      expectedTitle: "Fixture task",
      promptEndSentinel: "MOBILE_PROMPT_END_SENTINEL",
      repoId: "repo-1",
      taskId: "task-1"
    });

    expect(received.map((call) => `${call.method} ${call.url}`)).toEqual([
      "GET /v1/tasks/task-1"
    ]);
    expectLocalProcessRequests();
  });

  it("reproduces the refusal when the same read is made with Node's global fetch", async () => {
    received = [];

    await expect(
      // local-fetch-exempt: deliberately the wrong client, to prove the server stand-in tells the two apart
      assertPtyTerminalFixtureAvailable(baseUrl, FIXTURE, fetch)
    ).rejects.toThrow("was not available from");

    expect(received).toHaveLength(1);
    expect(received[0].classifiedAsBrowser).toBe(true);
    expect(received[0].headers).toHaveProperty("sec-fetch-mode");
  });

  it("drives the pin journey's desktop reads as a local process by default", async () => {
    received = [];
    let pinnedLocally = false;
    const row = {
      getLocation: vi.fn(async () => ({ x: 10, y: 100 })),
      getSize: vi.fn(async () => ({ width: 360, height: 90 })),
      waitForDisplayed: vi.fn(async () => undefined)
    };
    const repo = {
      click: vi.fn(async () => undefined),
      isDisplayed: vi.fn(async () => true),
      isExisting: vi.fn(async () => true)
    };
    const driver = {
      $: vi.fn(async (selector: string) =>
        selector === "~mobile.tasks.repo.git:hash-1" ? repo : row
      ),
      $$: vi.fn(async () =>
        (pinnedLocally ? ["task-1", "task-2"] : ["task-2", "task-1"]).map((taskId) => ({
          getAttribute: vi.fn(async (name: string) =>
            name === "name" ? `mobile.task-row.${taskId}` : null
          )
        }))
      ),
      execute: vi.fn(async () => {
        pinnedLocally = !pinnedLocally;
      }),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        if (!(await condition())) throw new Error(options.timeoutMsg);
      })
    };

    await exerciseTaskPinSwipe(driver as never, baseUrl, "task-1");

    expect(received.map((call) => `${call.method} ${call.url}`)).toEqual([
      "GET /v1/tasks/task-1",
      "GET /v1/repos/repo-1/tasks",
      "GET /v1/repos",
      "GET /v1/repos/repo-1/tasks"
    ]);
    expectLocalProcessRequests();
  });

  it("drives the Activity dismissal's desktop actions as a local process by default", async () => {
    received = [];
    activity = "idle";
    let dismissedRevision: string | null = null;
    let revision = 0;
    const activityRevision = () => String(revision);
    const row = {
      getLocation: vi.fn(async () => ({ x: 10, y: 100 })),
      getSize: vi.fn(async () => ({ width: 360, height: 90 })),
      isExisting: vi.fn(async () => dismissedRevision !== activityRevision()),
      waitForDisplayed: vi.fn(async () => {
        // The later revision is what the row's return waits on; a POST that
        // reached the server as a local process is what advances it.
        if (dismissedRevision !== null) revision += 1;
      })
    };
    const activityTab = { click: vi.fn(async () => undefined) };
    const screen = { waitForDisplayed: vi.fn(async () => undefined) };
    const driver = {
      $: vi.fn(async (selector: string) => {
        if (selector === selectors.recentTab) return activityTab;
        if (selector === selectors.recentScreen) return screen;
        return row;
      }),
      execute: vi.fn(async () => {
        dismissedRevision = activityRevision();
      }),
      waitUntil: vi.fn(async (condition: () => Promise<boolean>, options) => {
        if (!(await condition())) throw new Error(options.timeoutMsg);
      })
    };

    await exerciseActivityDismissSwipe(driver as never, baseUrl, "task-1");

    expect(received.map((call) => `${call.method} ${call.url}`)).toEqual([
      "POST /v1/tasks/task-1/actions/runtime-status",
      "POST /v1/tasks/task-1/actions/runtime-status",
      "GET /v1/tasks/recent",
      "POST /v1/tasks/task-1/actions/runtime-status",
      "POST /v1/tasks/task-1/actions/runtime-status"
    ]);
    expectLocalProcessRequests();
  });
});
