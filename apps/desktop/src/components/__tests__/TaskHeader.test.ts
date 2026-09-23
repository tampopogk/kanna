// @vitest-environment happy-dom

import type { PipelineItem } from "../../types/kanna";
import { mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import { openUrl } from "@tauri-apps/plugin-opener";

vi.mock("vue-i18n", () => ({
  useI18n: () => ({
    t: (key: string, fallback?: string) => fallback ?? key,
  }),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(() => Promise.resolve()),
}));

vi.mock("../../tauri-mock", () => ({
  isTauri: true,
}));

function makeItem(overrides: Partial<PipelineItem> = {}): PipelineItem {
  return {
    id: "task-1",
    repo_id: "repo-1",
    issue_number: null,
    issue_title: null,
    prompt: "Fix port ordering",
    workflow: "default",
    stage: "in progress",
    stage_result: null,
    tags: "[]",
    pr_number: null,
    pr_url: null,
    branch: "task-1",
    closed_at: null,
    agent_type: null,
    agent_provider: "claude",
    agent_session_id: null,
    activity: "idle",
    activity_changed_at: null,
    unread_at: null,
    port_offset: null,
    display_name: "Fix port ordering",
    port_env: JSON.stringify({
      API_PORT: 3001,
      KANNA_DEV_PORT: 1421,
    }),
    pinned: 0,
    pin_order: null,
    base_ref: null,
    previous_stage: null,
    teardown_started_at: null,
    created_at: "2026-04-20T00:00:00.000Z",
    updated_at: "2026-04-20T00:00:00.000Z",
    ...overrides,
  };
}

describe("TaskHeader", () => {
  it("leaves provider and model presentation to the agent TUI", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, { props: { item: { ...makeItem(), launchProvider: "opencode", launchModel: "local/Qwen-Coder" } }, global: { mocks: { $t: (key: string) => key } } });
    expect(wrapper.text()).not.toContain("Launched with");
    expect(wrapper.text()).not.toContain("local/Qwen-Coder");
  });
  it("renders a draft presentation when durable metadata is unavailable", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, {
      props: {
        item: {
          display_name: "Initializing task",
          issue_title: null,
          prompt: "Prepare a stable task slot",
          stage: "in progress",
          branch: null,
          port_env: null,
          issue_number: null,
          pr_number: null,
          pr_url: null,
        },
      },
      global: {
        mocks: {
          $t: (key: string, fallback?: string) => fallback ?? key,
        },
      },
    });

    expect(wrapper.get(".stage-badge").text()).toBe("in progress");
    expect(wrapper.get(".task-title").text()).toBe("Initializing task");
    expect(wrapper.get(".task-title").attributes("title")).toBe("Prepare a stable task slot");
    expect(wrapper.findAll(".meta-item")).toHaveLength(0);
  });

  it("renders all configured port badges", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, {
      props: {
        item: makeItem({
          port_env: JSON.stringify({
            APP_PORT: 1421,
            API_PORT: 3001,
            STORYBOOK_PORT: 6006,
            RELAY_PORT: 7555,
            MOBILE_PORT: 8081,
          }),
        }),
      },
      global: {
        mocks: {
          $t: (key: string, fallback?: string) => fallback ?? key,
        },
      },
    });

    expect(wrapper.findAll(".meta-item.port")).toHaveLength(5);
  });

  it("distinguishes a projected stage from a durable stage change", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, {
      props: {
        item: makeItem({
          stage: "in progress",
          stage_advance_pending: true,
          stage_advance_from: "plan",
        }),
      },
      global: {
        mocks: {
          $t: (key: string, fallback?: string) => fallback ?? key,
        },
      },
    });

    const badge = wrapper.get(".stage-badge");
    expect(badge.text()).toBe("plan → in progress…");
    expect(badge.classes()).toContain("stage-badge-pending");
    expect(badge.attributes("title")).toBe("taskHeader.stageAdvancePending");
  });

  it("renders port badges in ascending numeric order", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, {
      props: {
        item: makeItem(),
      },
      global: {
        mocks: {
          $t: (key: string, fallback?: string) => fallback ?? key,
        },
      },
    });

    expect(
      wrapper.findAll(".meta-item.port").map((node) => node.text().trim()),
    ).toEqual([":1421", ":3001"]);
  });

  it("shows the source environment variable in each port badge tooltip", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, {
      props: {
        item: makeItem(),
      },
      global: {
        mocks: {
          $t: (key: string, fallback?: string) => fallback ?? key,
        },
      },
    });

    expect(
      wrapper.findAll(".meta-item.port").map((node) => node.attributes("title")),
    ).toEqual(["KANNA_DEV_PORT=1421", "API_PORT=3001"]);
  });

  it("uses the full task prompt as the truncated title tooltip", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const prompt = "Add a tooltip to task titles so long prompts remain inspectable when the visible title is truncated";
    const wrapper = mount(TaskHeader, {
      props: {
        item: makeItem({
          display_name: "Tooltip task titles",
          prompt,
        }),
      },
      global: {
        mocks: {
          $t: (key: string, fallback?: string) => fallback ?? key,
        },
      },
    });

    expect(wrapper.get(".task-title").text()).toBe("Tooltip task titles");
    expect(wrapper.get(".task-title").attributes("title")).toBe(prompt);
  });

  it("opens localhost for a port badge on click", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, {
      props: {
        item: makeItem(),
      },
      global: {
        mocks: {
          $t: (key: string, fallback?: string) => fallback ?? key,
        },
      },
    });

    await wrapper.find(".meta-item.port").trigger("click");

    expect(openUrl).toHaveBeenCalledWith("http://localhost:1421");
  });

  it("does not let the header mousedown guard cancel port interaction", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, {
      props: {
        item: makeItem(),
      },
      global: {
        mocks: {
          $t: (key: string, fallback?: string) => fallback ?? key,
        },
      },
    });

    const event = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
    wrapper.find(".meta-item.port").element.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(false);
  });
});

describe("TaskHeader latest result (T11a)", () => {
  const SIX_VERDICTS = ["success", "unverified", "partial", "needs-input", "declined", "failure"];

  it.each(SIX_VERDICTS)("renders the %s verdict verbatim", async (verdict) => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, {
      props: {
        item: makeItem(),
        latestRun: { verdict, summary: null },
      },
      global: { mocks: { $t: (key: string, fallback?: string) => fallback ?? key } },
    });

    expect(wrapper.get(".verdict-badge").text()).toBe(verdict);
  });

  it("shows the result message", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, {
      props: {
        item: makeItem(),
        latestRun: { verdict: "success", summary: "Tests pass; verified in browser." },
      },
      global: { mocks: { $t: (key: string, fallback?: string) => fallback ?? key } },
    });

    expect(wrapper.get(".latest-result-message").text()).toBe("Tests pass; verified in browser.");
  });

  it("shows the exit taken when present", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, {
      props: {
        item: makeItem(),
        latestRun: { verdict: "success", summary: "Done.", exit: "needs-followup" },
      },
      global: { mocks: { $t: (key: string, fallback?: string) => fallback ?? key } },
    });

    expect(wrapper.get(".meta-item.exit").text()).toContain("needs-followup");
  });

  it("shows no exit element when absent", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, {
      props: {
        item: makeItem(),
        latestRun: { verdict: "success", summary: "Done." },
      },
      global: { mocks: { $t: (key: string, fallback?: string) => fallback ?? key } },
    });

    expect(wrapper.find(".meta-item.exit").exists()).toBe(false);
  });

  it("opens a named stored artifact reference in the artifact viewer on click", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, {
      props: {
        item: makeItem(),
        latestRun: {
          verdict: "success",
          summary: "Done.",
          artifacts: {
            "review-notes": { type: "stored", repoId: "repo-1", artifactId: "abc123", kind: "report" },
          },
        },
      },
      global: { mocks: { $t: (key: string, fallback?: string) => fallback ?? key } },
    });

    await wrapper.get(".artifact-chip").trigger("click");

    expect(wrapper.emitted("open-artifact")).toEqual([
      [{ type: "stored", repoId: "repo-1", artifactId: "abc123", kind: "report" }],
    ]);
  });

  it("renders exactly as today when the server omits latestRun entirely", async () => {
    const { default: TaskHeader } = await import("../TaskHeader.vue");
    const wrapper = mount(TaskHeader, {
      props: { item: makeItem() },
      global: { mocks: { $t: (key: string, fallback?: string) => fallback ?? key } },
    });

    expect(wrapper.find('[data-testid="latest-result"]').exists()).toBe(false);
  });
});
