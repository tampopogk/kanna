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
    expect(wrapper.get('[data-testid="edit-in-terminal"]').text()).toBe("Edit");
    expect(openEditor).not.toHaveBeenCalled();
    await wrapper.get('[data-testid="edit-in-terminal"]').trigger("click");
    await flushPromises();
    expect(wrapper.get("select").text()).toContain("/usr/bin/vim");
    expect(openEditor).not.toHaveBeenCalled();
    await wrapper.get('[data-testid="start-terminal-editor"]').trigger("click");
    await flushPromises();
    expect(openEditor).toHaveBeenCalledWith("vim");
  });
  it("shows the notice by default and persists opt-out on normal dismissal", async () => {
    vi.mocked(fetchTerminalEditorChoices).mockResolvedValue([{ command: "vim", executable: "/usr/bin/vim", args: [] }]);
    let noticeDismissed = false;
    const dismissNotice = vi.fn(async () => { noticeDismissed = true; });
    const openEditor = vi.fn().mockResolvedValue(undefined);
    const wrapper = mount(TerminalEditorPicker, {
      props: { openEditor, dismissNotice },
    });

    await wrapper.get('[data-testid="edit-in-terminal"]').trigger("click");
    await flushPromises();
    expect(wrapper.get('[data-testid="terminal-editor-notice"]').text()).toContain("Use the editor’s own save and quit commands");
    await wrapper.get('input[type="checkbox"]').setValue(true);
    await wrapper.get("button:last-child").trigger("click");
    await flushPromises();
    expect(dismissNotice).toHaveBeenCalledOnce();

    wrapper.unmount();
    const reopened = mount(TerminalEditorPicker, {
      props: { openEditor, dismissNotice, noticeDismissed },
    });
    await reopened.get('[data-testid="edit-in-terminal"]').trigger("click");
    await flushPromises();
    expect(reopened.find('[data-testid="terminal-editor-notice"]').exists()).toBe(false);
    expect(reopened.get("select").text()).toContain("/usr/bin/vim");
    await reopened.get('[data-testid="start-terminal-editor"]').trigger("click");
    await flushPromises();
    expect(openEditor).toHaveBeenCalledWith("vim");
    reopened.unmount();
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
