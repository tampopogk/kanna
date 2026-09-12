import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import TerminalEditorView from "../TerminalEditorView.vue";

describe("TerminalEditorView", () => {
  it("shows only the attach-only editor terminal", () => {
    const wrapper = mount(TerminalEditorView, {
      props: {
        active: true,
        session: {
          sessionId: "shell-editor-task-file",
          worktreePath: "/repo/.kanna-worktrees/task-file",
          filePath: "README.md",
          command: "nvim",
        },
      },
      global: {
        stubs: {
          TerminalView: {
            name: "TerminalView",
            props: {
              attachOnly: Boolean,
              sessionId: String,
              active: Boolean,
              worktreePath: String,
            },
            template: '<div data-testid="terminal-view" />',
          },
        },
      },
    });

    const terminal = wrapper.getComponent({ name: "TerminalView" });
    expect(terminal.props()).toMatchObject({
      attachOnly: true,
      sessionId: "shell-editor-task-file",
      active: true,
      worktreePath: "/repo/.kanna-worktrees/task-file",
    });
    expect(wrapper.find("button").exists()).toBe(false);
    expect(wrapper.text()).toBe("");
  });
});
