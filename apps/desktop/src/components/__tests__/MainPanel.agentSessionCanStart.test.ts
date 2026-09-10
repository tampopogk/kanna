// @vitest-environment happy-dom

import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PipelineItem } from "../../types/kanna";
import type { TaskUiSlot } from "../../types/taskUi";

const invokeMock = vi.fn();
const fetchTaskDetailMock = vi.fn();

vi.mock("../../invoke", () => ({ invoke: invokeMock }));
vi.mock("../../services/desktopServerClient", () => ({
  fetchDesktopTaskDetail: fetchTaskDetailMock,
}));

function durableTask(overrides: Partial<PipelineItem> = {}): PipelineItem {
  return {
    id: "task-1",
    repo_id: "repo-1",
    prompt: "do the thing",
    stage: "in progress",
    tags: "[]",
    pr_number: null,
    pr_url: null,
    branch: "task-task-1",
    agent_type: "pty",
    agent_provider: "claude",
    port_offset: null,
    port_env: null,
    activity: "working",
    created_at: "2026-09-09 00:00:00",
    updated_at: "2026-09-09 00:00:00",
    activity_changed_at: "2026-09-09 00:00:00",
    unread_at: null,
    pinned: 0,
    pin_order: null,
    display_name: "Task one",
    closed_at: null,
    workflow: "default",
    stage_result: null,
    issue_number: null,
    issue_title: null,
    base_ref: null,
    agent_session_id: null,
    previous_stage: null,
    teardown_started_at: null,
    last_output_preview: null,
    parent_task_id: null,
    ...overrides,
  };
}

function readySlot(task: PipelineItem): TaskUiSlot {
  return { slot_id: "slot-1", task_id: task.id, state: "ready", task };
}

/**
 * What the agent view is told about whether a launch could still produce its
 * session — read where it is actually handed over, not rebuilt inline.
 *
 * The predicate once read a `props.item` that MainPanel does not declare. It
 * was therefore always false at runtime, so an agent view attaching while
 * setup was still running gave up instead of waiting, and `vue-tsc` failed the
 * build. A unit test that recomposed the expression could not see either
 * fault; mounting the panel and reading the prop it passes can.
 */
async function agentSessionCanStart(
  task: PipelineItem,
  agentLaunchPending?: boolean,
): Promise<unknown> {
  const { default: MainPanel } = await import("../MainPanel.vue");
  const wrapper = mount(MainPanel, {
    props: {
      uiSlot: readySlot(task),
      repoPath: "/tmp/repo",
      hasRepos: true,
      ...(agentLaunchPending === undefined ? {} : { agentLaunchPending }),
    },
    global: {
      mocks: { $t: (key: string) => key },
      stubs: {
        TaskHeader: true,
        TerminalTabs: {
          name: "TerminalTabs",
          // Declared so the stub receives it as a prop rather than a fallthrough
          // attribute, which is what `props()` reads back.
          props: ["agentSessionCanStart"],
          template: "<div />",
        },
      },
    },
  });
  await flushPromises();
  return wrapper.findComponent({ name: "TerminalTabs" }).props("agentSessionCanStart");
}

describe("MainPanel agentSessionCanStart", () => {
  beforeEach(() => {
    vi.resetModules();
    invokeMock.mockReset();
    fetchTaskDetailMock.mockReset();
    fetchTaskDetailMock.mockImplementation(async (taskId: string) => ({
      id: taskId,
      stage: "in progress",
      closedAt: null,
      latestRun: null,
      revisionRounds: 0,
      revisionLimit: 3,
      childTaskIds: [],
    }));
    invokeMock.mockImplementation((command: string) => {
      if (command === "read_env_var") return Promise.resolve("0.0.0");
      return Promise.reject(new Error("missing"));
    });
    vi.stubGlobal("__KANNA_MOBILE__", false);
    localStorage.clear();
  });

  it("keeps looking while a launch has not been read yet", async () => {
    // Undefined is "not yet known", which is what a launch genuinely is in its
    // first moments — the view waits rather than declaring the agent gone.
    expect(await agentSessionCanStart(durableTask())).toBe(true);
  });

  it("stops for a closed task", async () => {
    expect(await agentSessionCanStart(durableTask({ closed_at: "2026-09-09 01:00:00" }))).toBe(
      false,
    );
  });

  it("stops when the server says the session exited", async () => {
    expect(await agentSessionCanStart(durableTask({ runtime_state: "exited" }))).toBe(false);
  });

  it("stops when the server says no launch can still start the agent", async () => {
    expect(await agentSessionCanStart(durableTask(), false)).toBe(false);
  });
});
