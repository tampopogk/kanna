// @vitest-environment happy-dom

import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { describe, expect, it, vi } from "vitest";
import OpenCodeModelSelect from "../OpenCodeModelSelect.vue";
import {
  fetchDesktopCopilotModels,
  fetchDesktopOpenCodeModels,
} from "../../services/desktopServerClient";
vi.mock("../../services/desktopServerClient", () => ({
  fetchDesktopCopilotModels: vi.fn(),
  fetchDesktopOpenCodeModels: vi.fn(),
}));

describe("OpenCode model selection", () => {
  it("dismisses a committed discovered suggestion without blurring custom model entry", async () => {
    vi.mocked(fetchDesktopOpenCodeModels).mockResolvedValue([
      { id: "local/qwen", name: "Qwen", local: true },
    ]);
    const wrapper = mount(OpenCodeModelSelect, {
      attachTo: document.body,
      props: { repoId: "repo-local", modelValue: "" },
    });
    await vi.waitFor(() => expect(wrapper.find("option").exists()).toBe(true));

    const input = wrapper.get("input");
    const blur = vi.spyOn(input.element as HTMLInputElement, "blur");
    await input.setValue("local/qwen");
    expect(blur).toHaveBeenCalledOnce();

    blur.mockClear();
    await input.setValue("custom/backend-model");
    expect(blur).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("shows native local models with connection and context, without claiming readiness", async () => {
    vi.mocked(fetchDesktopOpenCodeModels).mockResolvedValue([
      { id: "omlx/qwen-coder", name: "Qwen", local: true, connection: "http://127.0.0.1:8000", context: 32768 },
    ]);
    const wrapper = mount(OpenCodeModelSelect, { props: { repoId: "repo-local", modelValue: "omlx/qwen-coder" } });
    await vi.waitFor(() => expect(wrapper.text()).toContain("http://127.0.0.1:8000"));
    expect(fetchDesktopOpenCodeModels).toHaveBeenCalledWith("repo-local");
    expect(wrapper.text()).toContain("Server readiness has not been checked");
    await wrapper.get("input").setValue("cloud/other-model");
    expect(wrapper.emitted("update:modelValue")?.[0]).toEqual(["cloud/other-model"]);
  });
  it("retains direct entry after discovery fails", async () => {
    vi.mocked(fetchDesktopOpenCodeModels).mockRejectedValue(new Error("OpenCode is unavailable"));
    const wrapper = mount(OpenCodeModelSelect, { props: { repoId: "repo-local", modelValue: "" } });
    await vi.waitFor(() => expect(wrapper.find('[role="alert"]').exists()).toBe(true));
    await wrapper.get("input").setValue("local/qwen");
    expect(wrapper.emitted("update:modelValue")?.[0]).toEqual(["local/qwen"]);
  });
  it("discards an inventory returned after switching repos", async () => {
    let resolve!: (value: []) => void;
    vi.mocked(fetchDesktopOpenCodeModels).mockReturnValue(new Promise(done => { resolve = done; }));
    const wrapper = mount(OpenCodeModelSelect, { props: { repoId: "old", modelValue: "" } });
    await wrapper.setProps({ repoId: undefined });
    resolve([]);
    await nextTick();
    expect(wrapper.find('[role="status"]').exists()).toBe(false);
  });

  it("shows Copilot recent model IDs as suggestions while retaining unrestricted entry", async () => {
    vi.mocked(fetchDesktopCopilotModels).mockResolvedValue([{ id: "gpt-5.6-terra" }]);
    const wrapper = mount(OpenCodeModelSelect, {
      props: { provider: "copilot", repoId: "repo-local", modelValue: "" },
    });
    await vi.waitFor(() => expect(wrapper.find("option").exists()).toBe(true));
    expect(wrapper.text()).toContain("recently used Copilot models on this machine, not a catalog");
    await wrapper.get('[aria-label="GitHub Copilot model"]').setValue("newly-released-model");
    expect(wrapper.emitted("update:modelValue")?.[0]).toEqual(["newly-released-model"]);
  });
});
