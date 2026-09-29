// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import type { DesignThread } from "@kanna/design-editor";
import en from "../../../i18n/locales/en.json";
import DesignFeed from "../DesignFeed.vue";
import DesignSignoffBar from "../DesignSignoffBar.vue";
import DesignMockupRoom from "../DesignMockupRoom.vue";
import DesignLadder from "../DesignLadder.vue";

const i18n = () => createI18n({ legacy: false, locale: "en", messages: { en } });

function thread(number: number, overrides: Partial<DesignThread> = {}): DesignThread {
  return {
    id: `th-${number}`,
    number,
    kind: "comment",
    status: "open",
    anchor: { blockId: "b1", quotedText: `quote ${number}`, state: "attached" },
    comments: [
      {
        id: `cm-${number}`,
        author: "operator",
        body: `feedback ${number}`,
        createdAt: "2026-09-26T10:00:00.000Z",
        delivery: { id: `dl-${number}`, state: "queued" },
      },
    ],
    deliveryStatus: "queued",
    createdAt: "2026-09-26T10:00:00.000Z",
    resolvedAt: null,
    resolvedBy: null,
    ...overrides,
  };
}

function panel(threads: DesignThread[], canWrite = true) {
  return mount(DesignFeed, { props: { threads, canWrite }, global: { plugins: [i18n()] } });
}

describe("DesignFeed", () => {
  const numbers = (wrapper: ReturnType<typeof panel>) => wrapper.findAll(".fb .kd-num").map((node) => node.text());

  it("orders threads by creation number, newest last, whatever order they arrive in", () => {
    const wrapper = panel([thread(3), thread(1), thread(2)]);
    expect(numbers(wrapper)).toEqual(["1", "2", "3"]);
    expect(wrapper.get("h4").text()).toContain("Feedback → agent");
    expect(wrapper.get('[data-testid="design-feedback-open-count"]').text()).toBe("3 open");
  });

  it("hides resolved threads behind a toggle at the top, then an open divider, and keeps their numbers", async () => {
    const wrapper = panel([thread(1, { status: "resolved" }), thread(2), thread(3, { status: "resolved" })]);
    expect(numbers(wrapper)).toEqual(["2"]);
    const toggle = wrapper.get('[data-testid="design-feedback-resolved-toggle"]');
    expect(toggle.text()).toBe("▸ 2 resolved");
    await toggle.trigger("click");
    expect(numbers(wrapper)).toEqual(["1", "3", "2"]);
    expect(wrapper.get(".feed-divider").text()).toBe("open");
    await wrapper.get('[data-testid="design-thread-1-resolve"]').trigger("click");
    expect(wrapper.emitted("resolve")?.[0]).toEqual(["th-1", false]);
    await wrapper.get('[data-testid="design-thread-2-resolve"]').trigger("click");
    expect(wrapper.emitted("resolve")?.[1]).toEqual(["th-2", true]);
  });

  it("reads like the prototype's cards: what it is on, the comment, who and delivery, the agent's reply", async () => {
    const replied = thread(1, {
      deliveryStatus: "agent_replied",
      comments: [
        { id: "cm-1", author: "operator", body: "tighten it", createdAt: "x", delivery: { id: "dl-1", state: "delivered" } },
        { id: "cm-2", author: "agent", body: "Tightened the intro.", createdAt: "y" },
      ],
    });
    const delivered = thread(2, { deliveryStatus: "delivered" });
    const uncertain = thread(3, {
      deliveryStatus: "uncertain",
      comments: [
        { id: "cm-3", author: "operator", body: "?", createdAt: "x", delivery: { id: "dl-3", state: "uncertain", detail: "daemon restarted" } },
      ],
    });
    const message = thread(4, { kind: "message", anchor: null });
    const wrapper = panel([replied, delivered, uncertain, message]);
    const cards = wrapper.findAll(".fb");
    expect(cards[0].get(".on-el").text()).toBe("1quote 1");
    expect(cards[0].get("small").text()).toBe("You · agent replied");
    expect(cards[0].get(".reply").text()).toBe("Agent: Tightened the intro.");
    expect(cards[1].get("small").text()).toBe("You · delivered ✓");
    expect(cards[2].get("small").text()).toBe("You · not delivered ✕");
    expect(cards[3].get(".on-el").text()).toContain("✦ message to the agent");
    expect(wrapper.find('[data-testid="design-thread-1-resend"]').exists()).toBe(false);
    await wrapper.get('[data-testid="design-thread-3-resend"]').trigger("click");
    expect(wrapper.emitted("retry")?.[0]).toEqual(["dl-3"]);
    await cards[1].trigger("click");
    expect(wrapper.emitted("open")?.[0]).toEqual(["th-2"]);
  });

  it("is read-only once handed off, and says how to start when empty", () => {
    const readOnly = panel([thread(1)], false);
    expect(readOnly.find('[data-testid="design-thread-1-resolve"]').exists()).toBe(false);
    expect(panel([]).get(".empty").text()).toContain("press 💬 or ⌘↵");
  });
});

describe("DesignLadder", () => {
  const positions = [
    { name: "static", label: "Static mockup" },
    { name: "interactive", label: "Interactive mockup" },
    { name: "prototype", label: "Prototype" },
  ];

  it("shows the workflow and its positions, the current one lit, moving freely", async () => {
    const wrapper = mount(DesignLadder, {
      props: { design: { inDesignStage: true, status: "designing", position: "interactive", positions } },
      global: { plugins: [i18n()] },
    });
    expect(wrapper.get(".wf").text()).toBe("App Design");
    expect(wrapper.findAll(".arrow").map((node) => node.text())).toEqual(["⇄", "⇄"]);
    expect(wrapper.get(".step.now").text()).toBe("Interactive mockup");
    await wrapper.get('[data-testid="design-position-static"]').trigger("click");
    expect(wrapper.emitted("pick")?.[0]).toEqual(["static"]);
    expect(wrapper.text()).not.toContain("Software factory");
  });

  it("once handed off, locks the positions and shows the software factory", () => {
    const wrapper = mount(DesignLadder, {
      props: { design: { inDesignStage: false, status: "handed_off", position: "prototype", positions } },
      global: { plugins: [i18n()] },
    });
    expect(wrapper.findAll("button.step").every((node) => node.attributes("disabled") !== undefined)).toBe(true);
    expect(wrapper.get(".step.factory.now").text()).toBe("Software factory");
  });
});

describe("DesignSignoffBar", () => {
  const candidate = {
    approval: { id: "ap-1", phase: "candidate" },
    confirmationToken: "token-1",
    policy: { retain: "results-and-summary", path: "docs/design-results/t", files: ["docs/design-results/t/design.md", "docs/design-results/t/SUMMARY.md"] },
    nextStage: "plan",
    openThreads: 1,
    undeliveredFeedback: 0,
    skippedSourceFiles: [],
  };

  function bar(overrides: Record<string, unknown> = {}) {
    const calls: string[] = [];
    const wrapper = mount(DesignSignoffBar, {
      props: {
        approval: null,
        status: "designing",
        inDesignStage: true,
        nextStage: "plan",
        scratchRepository: "/tasks/t/design/scratch-1",
        syncProblem: null,
        prepare: async () => {
          calls.push("prepare");
          return candidate as never;
        },
        confirm: async (shown: { confirmationToken: string }) => {
          calls.push(`confirm:${shown.confirmationToken}`);
          return {};
        },
        reopen: async () => calls.push("reopen"),
        retry: async () => calls.push("retry"),
        ...overrides,
      },
      global: { plugins: [i18n()] },
      attachTo: document.body,
    });
    return { wrapper, calls };
  }

  it("says what approving keeps and what happens next, and confirms only on the person's click", async () => {
    const { wrapper, calls } = bar();
    expect(wrapper.get(".repo").text()).toBe("disposable repo · /tasks/t/design/scratch-1");
    expect(wrapper.get('[data-testid="design-approve"]').text()).toBe("Approve for build →");
    await wrapper.get('[data-testid="design-approve"]').trigger("click");
    await flushPromises();
    const dialog = document.querySelector('[data-testid="design-approve-dialog"]')!;
    expect(dialog.textContent).toContain("docs/design-results/t/design.md");
    expect(dialog.querySelector("h3")!.textContent).toBe("Approve for build?");
    expect(dialog.textContent).toContain("The prototype repo is committed; that commit is what you approve");
    expect(dialog.textContent).toContain("the plan stage starts");
    expect([...dialog.querySelectorAll(".row button")].map((node) => node.textContent?.trim())).toEqual(["Keep iterating", "Approve"]);
    expect(dialog.textContent).toContain("1 thread(s) are still open");
    // No version number anywhere.
    expect(dialog.textContent).not.toMatch(/\bv\d|version \d|revision \d/i);
    expect(calls).toEqual(["prepare"]);
    (document.querySelector('[data-testid="design-approve-confirm"]') as HTMLButtonElement).click();
    await flushPromises();
    expect(calls).toEqual(["prepare", "confirm:token-1"]);
    expect(document.querySelector('[data-testid="design-approve-dialog"]')).toBeNull();
    wrapper.unmount();
  });

  it("shows hand-off progress, lets the person reopen before the commit, and retry a failure", async () => {
    const approving = bar({ approval: { id: "ap-1", phase: "exported", error: null }, status: "handing_off" });
    expect(approving.wrapper.get(".repo").text()).toContain("approving… Approved for build: waiting for the design session to commit");
    expect(approving.wrapper.find('[data-testid="design-approve"]').exists()).toBe(false);
    await approving.wrapper.get('[data-testid="design-approval-reopen"]').trigger("click");
    await flushPromises();
    expect(approving.calls).toEqual(["reopen"]);
    approving.wrapper.unmount();

    const committing = bar({ approval: { id: "ap-1", phase: "committing", error: null }, status: "handing_off" });
    expect(committing.wrapper.find('[data-testid="design-approval-reopen"]').exists()).toBe(false);
    committing.wrapper.unmount();

    const failed = bar({ approval: { id: "ap-1", phase: "failed", error: "the commit also changed prototype.js" } });
    expect(failed.wrapper.get(".repo").text()).toContain("approval failed: the commit also changed prototype.js");
    await failed.wrapper.get('[data-testid="design-approval-retry"]').trigger("click");
    await flushPromises();
    expect(failed.calls).toEqual(["retry"]);
    failed.wrapper.unmount();
  });

  it("shows what was approved once handed off", () => {
    const { wrapper } = bar({
      approval: { id: "ap-1", phase: "entered", error: null, committedSha: "5b93047b10a2", sourceCommit: "aaaa", artifactId: "01e43bb75d4df7aa" },
      status: "handed_off",
      inDesignStage: false,
    });
    expect(wrapper.get(".repo").text()).toBe("approved · commit 5b93047 · artifact 01e43bb75d…");
    expect(wrapper.find('[data-testid="design-approve"]').exists()).toBe(false);
    wrapper.unmount();
  });
});

describe("DesignMockupRoom", () => {
  const pin = (number: number, overrides: Partial<DesignThread> = {}): DesignThread =>
    thread(number, {
      anchor: {
        blockId: null,
        quotedText: "Save",
        state: "attached",
        element: {
          position: "static",
          artifactId: "a".repeat(40),
          page: "index.html",
          selector: "#save",
          excerpt: "Save",
          tag: "button",
          label: "button#save",
          context: "main#list",
          html: "<button id=\"save\">Save</button>",
        },
      },
      ...overrides,
    });

  function room(threads: DesignThread[], canWrite = true) {
    const calls: unknown[] = [];
    const wrapper = mount(DesignMockupRoom, {
      props: {
        mockup: null,
        positionLabel: "Static mockup",
        threads,
        canWrite,
        createPin: async (descriptor: unknown, body: string) => {
          calls.push(["create", descriptor, body]);
          return "th-new";
        },
        reply: async (id: string, body: string) => calls.push(["reply", id, body]),
        resolve: async (id: string, resolved: boolean) => calls.push(["resolve", id, resolved]),
        retry: async (id: string) => calls.push(["retry", id]),
      },
      global: { plugins: [i18n()] },
      attachTo: document.body,
    });
    return { wrapper, calls };
  }

  it("is the prototype's review room: the mockup, then Comments with each pin's element and its thread", async () => {
    const { wrapper, calls } = room([pin(1), pin(2, { status: "resolved" })]);
    expect(wrapper.get('[data-testid="design-mockup-waiting"]').text()).toBe("Waiting for the agent to publish a mockup…");
    expect(wrapper.get("h2").text()).toBe("Comments");
    expect(wrapper.get(".hint").text()).toBe("⌘-click any element in the mockup to comment on it. A plain click uses the mockup.");
    const first = wrapper.get('[data-testid="design-pin-thread-1"]');
    expect(first.get(".anchor").text()).toBe("1 button#save “Save”");
    expect(first.get(".c").text()).toBe("Youfeedback 1");
    expect(wrapper.get('[data-testid="design-pin-thread-2"]').classes()).toContain("resolved");
    const input = first.get(".reply input");
    await input.setValue("bigger");
    await input.trigger("keydown", { key: "Enter" });
    await first.get(".reply button").trigger("click");
    expect(calls).toEqual([["reply", "th-1", "bigger"], ["resolve", "th-1", true]]);
    wrapper.unmount();
  });

  it("is read-only once handed off, and empty until a pin is placed", () => {
    const { wrapper } = room([], false);
    expect(wrapper.get(".empty").text()).toBe("No comments yet.");
    wrapper.unmount();
    const readOnly = room([pin(1)], false);
    expect(readOnly.wrapper.find(".reply input").exists()).toBe(false);
    readOnly.wrapper.unmount();
  });
});
