import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import TerminalEditorPicker from "../TerminalEditorPicker.vue";
import { fetchTerminalEditorChoices } from "../../services/desktopServerClient";
vi.mock("../../services/desktopServerClient", () => ({
  fetchTerminalEditorChoices: vi.fn(),
  DesktopServerRequestError: class extends Error {},
}));
describe("explicit terminal editing", () => {
  it("detects only after Edit and launches only after the visible choice is accepted", async () => {
    vi.mocked(fetchTerminalEditorChoices).mockResolvedValue([{ command: "vim", executable: "/usr/bin/vim", args: [] }]);
    const openEditor = vi.fn().mockResolvedValue(undefined);
    const wrapper = mount(TerminalEditorPicker, { props: { openEditor } });
    expect(openEditor).not.toHaveBeenCalled();
    await wrapper.get('[data-testid="edit-in-terminal"]').trigger("click");
    await flushPromises();
    expect(wrapper.get("select").text()).toContain("/usr/bin/vim");
    expect(openEditor).not.toHaveBeenCalled();
    await wrapper.get('[data-testid="start-terminal-editor"]').trigger("click");
    await flushPromises();
    expect(openEditor).toHaveBeenCalledWith("vim");
  });
  it("explains missing editors without launching anything", async () => {
    vi.mocked(fetchTerminalEditorChoices).mockRejectedValue(new Error("No terminal editor found"));
    const openEditor = vi.fn();
    const wrapper = mount(TerminalEditorPicker, { props: { openEditor } });
    await wrapper.get('[data-testid="edit-in-terminal"]').trigger("click");
    await flushPromises();
    expect(wrapper.get('[role="alert"]').text()).toContain("No terminal editor found");
    expect(wrapper.find('[data-testid="start-terminal-editor"]').exists()).toBe(false);
    expect(openEditor).not.toHaveBeenCalled();
  });
});
