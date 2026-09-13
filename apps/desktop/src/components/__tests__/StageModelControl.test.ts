// @vitest-environment happy-dom
import { mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import StageModelControl from "../StageModelControl.vue";
import { fetchDesktopTaskDetail, replaceDesktopTaskWorkflow } from "../../services/desktopServerClient";
import type { PipelineItem } from "../../types/kanna";
const store = vi.hoisted(() => ({ advanceStage: vi.fn() }));
vi.mock("../../stores/kanna", () => ({ useKannaStore: () => store }));
vi.mock("../../services/desktopServerClient", () => ({ replaceDesktopTaskWorkflow: vi.fn(async (_id: string, _before: unknown, after: unknown) => after), fetchDesktopTaskDetail: vi.fn(), fetchDesktopOpenCodeModels: vi.fn(async () => []) }));
const task = { id: "task-one", repo_id: "repo-one", stage: "plan", pipeline: "test" } as PipelineItem;
describe("one-time OpenCode stage selection", () => {
  it("passes the native model ID only to the next stage override", async () => {
    const observed = { stages: [{ name: "plan" }, { name: "implement" }] };
    vi.mocked(fetchDesktopTaskDetail).mockResolvedValue({ id: "task-one", stage: "plan", workflowDefinition: observed } as never);
    store.advanceStage.mockResolvedValue("advanced");
    const wrapper = mount(StageModelControl, { props: { task } });
    await wrapper.get("button").trigger("click");
    await vi.waitFor(() => expect(wrapper.find("input").exists()).toBe(true));
    await wrapper.get("input").setValue("omlx/Qwen-Coder");
    await wrapper.get(".stage-model-panel > button").trigger("click");
    // The stage sequence this panel displayed rides along as the advance's
    // fence: "implement" was named from this document, so a tail published or
    // edited since must be a conflict rather than a stage nobody chose.
    expect(store.advanceStage).toHaveBeenCalledWith("task-one", { expectedDefinition: observed, nextStageAgentProvider: "opencode", nextStageModel: "omlx/Qwen-Coder" });
  });

  it("fences the advance on the displayed definition, not on a fresh read", async () => {
    store.advanceStage.mockClear();
    const displayed = { stages: [{ name: "plan" }, { name: "implement" }] };
    vi.mocked(fetchDesktopTaskDetail).mockResolvedValue({ id: "task-one", stage: "plan", workflowDefinition: displayed } as never);
    store.advanceStage.mockResolvedValue("ignored");
    const wrapper = mount(StageModelControl, { props: { task } });
    await wrapper.get("button").trigger("click");
    await vi.waitFor(() => expect(wrapper.find("input").exists()).toBe(true));
    // The server moves on after the panel read it. The action must still send
    // what the operator saw.
    vi.mocked(fetchDesktopTaskDetail).mockResolvedValue({ id: "task-one", stage: "plan", workflowDefinition: { stages: [{ name: "plan" }] } } as never);
    await wrapper.get("input").setValue("omlx/Qwen-Coder");
    await wrapper.get(".stage-model-panel > button").trigger("click");
    expect(store.advanceStage).toHaveBeenCalledWith("task-one", expect.objectContaining({ expectedDefinition: displayed }));
  });
  it("saves only the future stage binding across a post, without advancing", async () => {
    store.advanceStage.mockClear();
    const workflow = { name: "pinned", stages: [{ name: "plan", post: { name: "commit", prompt: "Commit" } }, { name: "implement", prompt: "Keep this prompt", policy: { transition: "manual" } }] };
    vi.mocked(fetchDesktopTaskDetail).mockResolvedValue({ id: "task-one", stage: "plan", workflowDefinition: workflow } as never);
    const canonical = { ...workflow, stages: [workflow.stages[0], { ...workflow.stages[1], agent_provider: [{ harness: "opencode", model: "local/Qwen-Coder" }] }] };
    vi.mocked(replaceDesktopTaskWorkflow).mockResolvedValueOnce(canonical);
    const wrapper = mount(StageModelControl, { props: { task } });
    await wrapper.get("button").trigger("click");
    await vi.waitFor(() => expect(wrapper.find("input").exists()).toBe(true));
    await wrapper.get("input").setValue("local/Qwen-Coder");
    await wrapper.get(".stage-model-panel > button").trigger("click");
    expect(replaceDesktopTaskWorkflow).toHaveBeenCalledWith("task-one", workflow, {
      ...workflow, stages: [workflow.stages[0], { ...workflow.stages[1], agent_provider: { harness: "opencode", model: "local/Qwen-Coder" } }],
    });
    expect(store.advanceStage).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain("Advance normally when ready");
    await wrapper.get("input").setValue("cloud/Coder");
    await wrapper.get(".stage-model-panel > button").trigger("click");
    expect(replaceDesktopTaskWorkflow).toHaveBeenLastCalledWith("task-one", canonical, expect.objectContaining({ stages: [canonical.stages[0], { ...canonical.stages[1], agent_provider: { harness: "opencode", model: "cloud/Coder" } }] }));
  });
  it("saves suffix-looking native model IDs and surfaces stale edits without retrying", async () => {
    vi.mocked(replaceDesktopTaskWorkflow).mockClear();
    vi.mocked(fetchDesktopTaskDetail).mockResolvedValue({ id: "task-one", stage: "plan", workflowDefinition: { stages: [{ name: "plan", post: { name: "commit" } }, { name: "implement" }] } } as never);
    const wrapper = mount(StageModelControl, { props: { task } });
    await wrapper.get("button").trigger("click");
    await vi.waitFor(() => expect(wrapper.find("input").exists()).toBe(true));
    await wrapper.get("input").setValue("local/model-high");
    await wrapper.get(".stage-model-panel > button").trigger("click");
    expect(replaceDesktopTaskWorkflow).toHaveBeenCalledWith("task-one", expect.anything(), expect.objectContaining({ stages: expect.arrayContaining([expect.objectContaining({ agent_provider: { harness: "opencode", model: "local/model-high" } })]) }));
    expect(wrapper.text()).not.toContain("ambiguous");
    vi.mocked(replaceDesktopTaskWorkflow).mockRejectedValue(new Error("409: pinned workflow changed"));
    await wrapper.get("input").setValue("local/Qwen");
    await wrapper.get(".stage-model-panel > button").trigger("click");
    expect(replaceDesktopTaskWorkflow).toHaveBeenCalledTimes(2);
    expect(wrapper.text()).toContain("pinned workflow changed");
  });
  it("keeps the selector open across snapshots of the same task and stage", async () => {
    vi.mocked(fetchDesktopTaskDetail).mockResolvedValue({ id: "task-one", stage: "plan", workflowDefinition: { stages: [{ name: "plan" }, { name: "implement" }] } } as never);
    const wrapper = mount(StageModelControl, { props: { task } });
    await wrapper.get("button").trigger("click");
    await vi.waitFor(() => expect(wrapper.find("input").exists()).toBe(true));
    await wrapper.get("input").setValue("local/Qwen");
    await wrapper.setProps({ task: { ...task, activity: "idle" } });
    expect(wrapper.find("input").exists()).toBe(true);
    expect(wrapper.get("input").element.value).toBe("local/Qwen");
    await wrapper.setProps({ task: { ...task, stage: "implement" } });
    expect(wrapper.find("input").exists()).toBe(false);
  });
  it("does not offer a model for a final stage close", async () => {
    vi.mocked(fetchDesktopTaskDetail).mockResolvedValue({ id: "task-one", stage: "plan", workflowDefinition: { stages: [{ name: "plan" }] } } as never);
    const wrapper = mount(StageModelControl, { props: { task } });
    await wrapper.get("button").trigger("click");
    await vi.waitFor(() => expect(wrapper.find('[role="status"]').exists()).toBe(true));
    expect(wrapper.find("input").exists()).toBe(false);
  });
});
