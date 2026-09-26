// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import type { DesignThread } from "@kanna/design-editor";
import en from "../../../i18n/locales/en.json";
import DesignFeedbackPanel from "../DesignFeedbackPanel.vue";
import DesignApprovalBar from "../DesignApprovalBar.vue";

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
  return mount(DesignFeedbackPanel, { props: { threads, canWrite }, global: { plugins: [i18n()] } });
}

describe("DesignFeedbackPanel", () => {
  it("orders threads by creation number, newest last, whatever order they arrive in", () => {
    const wrapper = panel([thread(3), thread(1), thread(2)]);
    const numbers = wrapper.findAll(".thread .number").map((node) => node.text());
    expect(numbers).toEqual(["1", "2", "3"]);
    expect(wrapper.get('[data-testid="design-feedback-open-count"]').text()).toBe("3 open");
  });

  it("hides resolved threads behind a toggle at the top and keeps their numbers", async () => {
    const wrapper = panel([thread(1, { status: "resolved" }), thread(2), thread(3, { status: "resolved" })]);
    expect(wrapper.findAll(".thread").map((node) => node.find(".number").text())).toEqual(["2"]);
    const toggle = wrapper.get('[data-testid="design-feedback-resolved-toggle"]');
    expect(toggle.text()).toContain("2 resolved");
    await toggle.trigger("click");
    expect(wrapper.findAll(".thread").map((node) => node.find(".number").text())).toEqual(["1", "3", "2"]);
    await wrapper.get('[data-testid="design-thread-1-resolve"]').trigger("click");
    expect(wrapper.emitted("resolve")?.[0]).toEqual(["th-1", false]);
    await wrapper.get('[data-testid="design-thread-2-resolve"]').trigger("click");
    expect(wrapper.emitted("resolve")?.[1]).toEqual(["th-2", true]);
  });

  it("shows delivery apart from resolution, the agent's reply, and resends only uncertain feedback", async () => {
    const replied = thread(1, {
      deliveryStatus: "agent_replied",
      comments: [
        { id: "cm-1", author: "operator", body: "tighten it", createdAt: "x", delivery: { id: "dl-1", state: "delivered" } },
        { id: "cm-2", author: "agent", body: "Tightened the intro.", createdAt: "y" },
      ],
    });
    const uncertain = thread(2, {
      deliveryStatus: "uncertain",
      comments: [
        { id: "cm-3", author: "operator", body: "?", createdAt: "x", delivery: { id: "dl-3", state: "uncertain", detail: "daemon restarted" } },
      ],
    });
    const message = thread(3, { kind: "message", anchor: null });
    const wrapper = panel([replied, uncertain, message]);
    const cards = wrapper.findAll(".thread");
    expect(cards[0].text()).toContain("agent replied");
    expect(cards[0].text()).toContain("Tightened the intro.");
    expect(cards[1].text()).toContain("not delivered");
    expect(cards[2].text()).toContain("message to the agent");
    expect(wrapper.find('[data-testid="design-thread-1-retry"]').exists()).toBe(false);
    await wrapper.get('[data-testid="design-thread-2-retry"]').trigger("click");
    expect(wrapper.emitted("retry")?.[0]).toEqual(["dl-3"]);
  });

  it("replies in place and becomes read-only once handed off", async () => {
    const wrapper = panel([thread(1)]);
    const reply = wrapper.findAll(".actions button").find((button) => button.text() === "Reply")!;
    await reply.trigger("click");
    await wrapper.get(".reply-form input").setValue("shorter please");
    await wrapper.get(".reply-form").trigger("submit");
    expect(wrapper.emitted("reply")?.[0]).toEqual(["th-1", "shorter please"]);

    const readOnly = panel([thread(1)], false);
    expect(readOnly.findAll(".actions button").map((button) => button.text())).toEqual(["Resolve"]);
  });
});

describe("DesignApprovalBar", () => {
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
    const wrapper = mount(DesignApprovalBar, {
      props: {
        approval: null,
        status: "designing",
        inDesignStage: true,
        nextStage: "plan",
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
    await wrapper.get('[data-testid="design-approve"]').trigger("click");
    await flushPromises();
    const dialog = document.querySelector('[data-testid="design-approve-dialog"]')!;
    expect(dialog.textContent).toContain("docs/design-results/t/design.md");
    expect(dialog.textContent).toContain("the plan stage starts");
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
    expect(approving.wrapper.text()).toContain("waiting for the design session to commit");
    expect(approving.wrapper.find('[data-testid="design-approve"]').exists()).toBe(false);
    await approving.wrapper.get('[data-testid="design-approval-reopen"]').trigger("click");
    await flushPromises();
    expect(approving.calls).toEqual(["reopen"]);
    approving.wrapper.unmount();

    const committing = bar({ approval: { id: "ap-1", phase: "committing", error: null }, status: "handing_off" });
    expect(committing.wrapper.find('[data-testid="design-approval-reopen"]').exists()).toBe(false);
    committing.wrapper.unmount();

    const failed = bar({ approval: { id: "ap-1", phase: "failed", error: "the commit also changed prototype.js" } });
    expect(failed.wrapper.text()).toContain("prototype.js");
    await failed.wrapper.get('[data-testid="design-approval-retry"]').trigger("click");
    await flushPromises();
    expect(failed.calls).toEqual(["retry"]);
    failed.wrapper.unmount();
  });
});
