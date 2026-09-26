import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import ReadOnlyTerminal from "../ReadOnlyTerminal.vue";
const state = vi.hoisted(() => ({ terminals: [] as any[], fit: vi.fn() }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit = state.fit; proposeDimensions() { return { cols: 80, rows: 24 }; } } }));
vi.mock("../../e2eTerminalBuffers", () => ({ registerE2ETerminalBuffer: () => () => {} }));
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  options: any;
  buffer = { active: { viewportY: 0, baseY: 0 } };
  write = vi.fn((_text: string, done: () => void) => done());
  reset = vi.fn();
  scrollToBottom = vi.fn();
  dispose = vi.fn();
  onData = vi.fn();
  onBinary = vi.fn();
  attachCustomKeyEventHandler = vi.fn();
  loadAddon() {}
  open() {}
  hasSelection() { return false; }
  getSelection() { return ""; }
  constructor(options: unknown) { this.options = options; state.terminals.push(this); }
} }));
afterEach(() => { state.terminals.length = 0; state.fit.mockClear(); });
describe("read-only creation terminal", () => {
  it("tees only new ANSI bytes into a terminal with no input path", async () => {
    const wrapper = mount(ReadOnlyTerminal, { props: { identity: "creation:a", output: "\x1b[32mFIRST\x1b[0m\r\n" } });
    await flushPromises();
    const terminal = state.terminals[0];
    expect(terminal.options).toMatchObject({ disableStdin: true, convertEol: true });
    expect(terminal.write).toHaveBeenLastCalledWith("\x1b[32mFIRST\x1b[0m\r\n", expect.any(Function));
    await wrapper.setProps({ output: "\x1b[32mFIRST\x1b[0m\r\nSECOND" });
    await flushPromises();
    expect(terminal.write).toHaveBeenLastCalledWith("SECOND", expect.any(Function));
    expect(terminal.reset).not.toHaveBeenCalled();
    expect(terminal.onData).not.toHaveBeenCalled();
    expect(terminal.onBinary).not.toHaveBeenCalled();
    wrapper.unmount();
    expect(terminal.dispose).toHaveBeenCalledOnce();
  });
  it("preserves a scrolled-back viewport and can replace a snapshot", async () => {
    const wrapper = mount(ReadOnlyTerminal, { props: { identity: "creation:a", output: "FIRST" } });
    await flushPromises();
    const terminal = state.terminals[0];
    terminal.scrollToBottom.mockClear();
    terminal.buffer.active = { viewportY: 0, baseY: 10 };
    await wrapper.setProps({ output: "FIRST\nSECOND" });
    await flushPromises();
    expect(terminal.scrollToBottom).not.toHaveBeenCalled();
    await wrapper.setProps({ output: "replacement" });
    await flushPromises();
    expect(terminal.reset).toHaveBeenCalledOnce();
    expect(terminal.write).toHaveBeenLastCalledWith("replacement", expect.any(Function));
    wrapper.unmount();
  });
});
