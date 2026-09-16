// @vitest-environment happy-dom

import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";
import TreeExplorerModal from "../TreeExplorerModal.vue";
import { TaskFileUnreadableError } from "../../services/taskFileRead";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("../../invoke", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

async function settle() {
  await Promise.resolve();
  await nextTick();
  await Promise.resolve();
  await nextTick();
}

/**
 * Let the preview column's debounced cursor fire and its read land. The
 * explorer deliberately waits 50ms before previewing anything so that holding
 * `j` does not read every file it passes over.
 */
async function settlePreview() {
  await new Promise((resolve) => setTimeout(resolve, 80));
  await settle();
  await settle();
}

/**
 * Answer the explorer's two local reads separately. A single `mockResolvedValue`
 * hands a directory listing back for a file's contents too, which is not a
 * filesystem any reader has.
 */
function mockLocalFs(options: {
  entries?: { name: string; is_dir: boolean }[];
  entriesByPath?: Record<string, { name: string; is_dir: boolean }[]>;
  text?: string;
  textByPath?: Record<string, string>;
  textError?: Error;
}) {
  invokeMock.mockImplementation(async (command: string, args: { path: string }) => {
    if (command === "read_dir_entries") {
      return options.entriesByPath?.[args.path] ?? options.entries ?? [];
    }
    if (command === "read_text_file") {
      if (options.textError) throw options.textError;
      const byPath = options.textByPath?.[args.path];
      if (byPath !== undefined) return byPath;
      return options.text ?? "";
    }
    return undefined;
  });
}

describe("TreeExplorerModal task roots", () => {
  afterEach(() => {
    invokeMock.mockReset();
  });

  it("lists a remote owner task without reading the local filesystem", async () => {
    const remoteDirectoryLoader = vi.fn(async () => ({
      entries: [
        { name: "src", path: "src", isDir: true },
        { name: "README.md", path: "README.md", isDir: false },
      ],
    }));
    const wrapper = mount(TreeExplorerModal, {
      props: {
        worktreePath: "task-owner-branch",
        repoRoot: "task-owner-branch",
        remoteDirectoryLoader,
      },
    });

    await settle();

    expect(remoteDirectoryLoader).toHaveBeenCalledWith("", false);
    expect(invokeMock).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain("README.md");
    expect(wrapper.text()).not.toContain("(empty)");
    wrapper.unmount();
  });

  it("shows a missing local worktree as unavailable without falling back to the repo", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    invokeMock.mockRejectedValue(new Error("not a directory"));
    const wrapper = mount(TreeExplorerModal, {
      props: {
        worktreePath: "/repo/.kanna-worktrees/task-removed",
        repoRoot: "/repo",
      },
    });

    await settle();

    expect(wrapper.get('[data-testid="tree-explorer-unavailable"]').text()).toContain(
      "Task files unavailable: not a directory",
    );
    expect(invokeMock).toHaveBeenCalledWith("read_dir_entries", {
      path: "/repo/.kanna-worktrees/task-removed",
      repoRoot: "/repo",
      showAllFiles: false,
    });
    expect(invokeMock).not.toHaveBeenCalledWith(
      "read_dir_entries",
      expect.objectContaining({ path: "/repo" }),
    );
    expect(wrapper.text()).not.toContain("(empty)");
    consoleError.mockRestore();
    wrapper.unmount();
  });
});

/**
 * The explorer's half of `kanna_open_view` answers a waiting caller, so
 * "opened" has to mean the requested directory was read — not that the
 * explorer is mounted over an empty column.
 */
describe("TreeExplorerModal reveal for kanna_open_view", () => {
  afterEach(() => {
    invokeMock.mockReset();
  });

  function command(target?: Record<string, unknown>) {
    return {
      requestId: "view-1",
      taskId: "task-a",
      view: "tree" as const,
      ...(target ? { target } : {}),
    };
  }

  it("refuses a directory that vanished between validation and the read", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const wrapper = mount(TreeExplorerModal, {
      props: { worktreePath: "/repo/.kanna-worktrees/task-a", repoRoot: "/repo" },
    });
    await settle();

    // The server validated `build/`, and it is gone by the time the explorer
    // asks for it.
    invokeMock.mockRejectedValue(new Error("directory deleted after dispatch"));
    const outcome = await wrapper.vm.revealDesktopViewTarget(
      command({ path: "build", kind: "directory" }),
    );

    expect(outcome.opened).toBe(false);
    expect(outcome.message).toContain("directory deleted after dispatch");
    consoleError.mockRestore();
    wrapper.unmount();
  });

  it("refuses an untargeted root it could not read", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    invokeMock.mockRejectedValue(new Error("not a directory"));
    const wrapper = mount(TreeExplorerModal, {
      props: { worktreePath: "/repo/.kanna-worktrees/task-removed", repoRoot: "/repo" },
    });
    await settle();

    // No target at all: being mounted is not being on screen.
    const outcome = await wrapper.vm.revealDesktopViewTarget(command());

    expect(outcome.opened).toBe(false);
    expect(outcome.message).toContain("not a directory");
    consoleError.mockRestore();
    wrapper.unmount();
  });

  it("opens a root it could read", async () => {
    mockLocalFs({
      entries: [
        { name: "src", is_dir: true },
        { name: "README.md", is_dir: false },
      ],
      text: "# readme\n",
    });
    const wrapper = mount(TreeExplorerModal, {
      props: { worktreePath: "/repo/.kanna-worktrees/task-a", repoRoot: "/repo" },
    });
    await settle();

    expect(await wrapper.vm.revealDesktopViewTarget(command())).toEqual({ opened: true });
    wrapper.unmount();
  });

  it("reveals a file through the reading it can actually do", async () => {
    mockLocalFs({
      entries: [{ name: "notes.txt", is_dir: false }],
      text: "notes\n",
    });
    const wrapper = mount(TreeExplorerModal, {
      props: { worktreePath: "/repo/.kanna-worktrees/task-a", repoRoot: "/repo" },
    });
    await settle();

    expect(await wrapper.vm.revealDesktopViewTarget(command({ path: "notes.txt", kind: "file" })))
      .toEqual({ opened: true });
    expect(await wrapper.vm.revealDesktopViewTarget(command({ path: "absent.txt", kind: "file" })))
      .toMatchObject({ opened: false, code: "file_not_found" });
    wrapper.unmount();
  });
});

/**
 * The third column is the whole point of a Miller-columns browser: it says
 * what the cursor is on. Resting on a file used to say "(no preview)", which
 * is most of the time, so the pane read as inert.
 */
describe("TreeExplorerModal preview column", () => {
  afterEach(() => {
    invokeMock.mockReset();
  });

  const LOCAL_ROOT = "/repo/.kanna-worktrees/task-a";

  function mountLocal() {
    return mount(TreeExplorerModal, {
      props: { worktreePath: LOCAL_ROOT, repoRoot: "/repo" },
    });
  }

  it("shows the head of the file under the cursor", async () => {
    mockLocalFs({
      entries: [{ name: "README.md", is_dir: false }],
      textByPath: { [`${LOCAL_ROOT}/README.md`]: "# Kanna\n\nsecond line\n" },
    });
    const wrapper = mountLocal();
    await settlePreview();

    const preview = wrapper.get('[data-testid="tree-preview-content"]');
    expect(preview.text()).toContain("# Kanna");
    expect(preview.text()).toContain("second line");
    expect(wrapper.text()).not.toContain("(no preview)");
    expect(invokeMock).toHaveBeenCalledWith("read_text_file", {
      path: `${LOCAL_ROOT}/README.md`,
    });
    wrapper.unmount();
  });

  it("marks a file longer than the preview budget as truncated", async () => {
    const long = Array.from({ length: 900 }, (_, i) => `line ${i}`).join("\n");
    mockLocalFs({
      entries: [{ name: "long.txt", is_dir: false }],
      text: long,
    });
    const wrapper = mountLocal();
    await settlePreview();

    const preview = wrapper.get('[data-testid="tree-preview-content"]');
    expect(preview.text()).toContain("line 0");
    expect(preview.text()).not.toContain("line 899");
    expect(preview.text()).toContain("preview truncated");
    wrapper.unmount();
  });

  it("still lists the children of the directory under the cursor", async () => {
    mockLocalFs({
      entriesByPath: {
        [LOCAL_ROOT]: [{ name: "src", is_dir: true }],
        [`${LOCAL_ROOT}/src`]: [{ name: "main.ts", is_dir: false }],
      },
    });
    const wrapper = mountLocal();
    await settlePreview();

    expect(wrapper.find('[data-testid="tree-preview-content"]').exists()).toBe(false);
    expect(wrapper.text()).toContain("main.ts");
    expect(wrapper.text()).not.toContain("(no preview)");
    expect(invokeMock).not.toHaveBeenCalledWith("read_text_file", expect.anything());
    wrapper.unmount();
  });

  it("clicking a directory in the preview column still navigates into it", async () => {
    mockLocalFs({
      entriesByPath: {
        [LOCAL_ROOT]: [{ name: "src", is_dir: true }],
        [`${LOCAL_ROOT}/src`]: [{ name: "components", is_dir: true }],
        [`${LOCAL_ROOT}/src/components`]: [{ name: "Button.vue", is_dir: false }],
      },
    });
    const wrapper = mountLocal();
    await settlePreview();

    const previewItems = wrapper.findAll(".col-preview .tree-item");
    expect(previewItems).toHaveLength(1);
    await previewItems[0].trigger("click");
    await settlePreview();

    expect(wrapper.get(".breadcrumb-bar").text()).toContain("src");
    expect(wrapper.get(".breadcrumb-bar").text()).toContain("components");
    wrapper.unmount();
  });

  it("does not read a binary file, and says it has no preview", async () => {
    mockLocalFs({ entries: [{ name: "logo.png", is_dir: false }] });
    const wrapper = mountLocal();
    await settlePreview();

    expect(wrapper.find('[data-testid="tree-preview-content"]').exists()).toBe(false);
    expect(wrapper.text()).toContain("(no preview)");
    expect(invokeMock).not.toHaveBeenCalledWith("read_text_file", expect.anything());
    wrapper.unmount();
  });

  it("treats a file that is not text as no preview rather than an outage", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    mockLocalFs({
      entries: [{ name: "pack.idx", is_dir: false }],
      textError: new Error(
        "failed to read '/repo/pack.idx': stream did not contain valid UTF-8",
      ),
    });
    const wrapper = mountLocal();
    await settlePreview();

    expect(wrapper.text()).toContain("(no preview)");
    expect(wrapper.find('[data-testid="tree-explorer-unavailable"]').exists()).toBe(false);
    consoleError.mockRestore();
    wrapper.unmount();
  });

  it("shows no preview for a file past the size budget", async () => {
    mockLocalFs({
      entries: [{ name: "bundle.js", is_dir: false }],
      text: "x".repeat(128 * 1024 + 1),
    });
    const wrapper = mountLocal();
    await settlePreview();

    expect(wrapper.find('[data-testid="tree-preview-content"]').exists()).toBe(false);
    expect(wrapper.text()).toContain("(no preview)");
    wrapper.unmount();
  });

  it("reports a failed read through the explorer's error, not a blank column", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    mockLocalFs({
      entries: [{ name: "secret.txt", is_dir: false }],
      textError: new Error("permission denied"),
    });
    const wrapper = mountLocal();
    await settlePreview();

    expect(wrapper.get('[data-testid="tree-explorer-unavailable"]').text()).toContain(
      "Task files unavailable: permission denied",
    );
    consoleError.mockRestore();
    wrapper.unmount();
  });

  it("previews a browsed-elsewhere file through its own loader", async () => {
    const remoteDirectoryLoader = vi.fn(async () => ({
      entries: [{ name: "notes.md", path: "notes.md", isDir: false }],
    }));
    const remoteContentLoader = vi.fn(async () => "remote head\n");
    const wrapper = mount(TreeExplorerModal, {
      props: {
        worktreePath: "task-owner-branch",
        repoRoot: "task-owner-branch",
        remoteDirectoryLoader,
        remoteContentLoader,
      },
    });
    await settlePreview();

    expect(remoteContentLoader).toHaveBeenCalledWith("notes.md");
    expect(wrapper.get('[data-testid="tree-preview-content"]').text()).toContain("remote head");
    expect(invokeMock).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  /**
   * The server-backed loaders — the contained local one, LAN and relay — never
   * hand over an oversized or non-text file's bytes: `kanna-server` refuses it
   * first, so the explorer's own size guard is never reached and the refusal
   * arrives as a rejected read. It still means "nothing to preview", and their
   * own tests prove each adapter raises exactly this error for a 413/415.
   */
  it.each([
    ["too-large" as const, "Remote task file read failed with HTTP 413.", "huge.log"],
    ["not-text" as const, "Remote task file read failed with HTTP 415.", "pack.idx"],
  ])("shows no preview for a %s file a server-backed loader refused", async (
    reason,
    message,
    name,
  ) => {
    const remoteDirectoryLoader = vi.fn(async () => ({
      entries: [{ name, path: name, isDir: false }],
    }));
    const remoteContentLoader = vi.fn(async () => {
      throw new TaskFileUnreadableError(reason, message);
    });
    const wrapper = mount(TreeExplorerModal, {
      props: {
        worktreePath: "task-owner-branch",
        repoRoot: "task-owner-branch",
        remoteDirectoryLoader,
        remoteContentLoader,
      },
    });
    await settlePreview();

    expect(remoteContentLoader).toHaveBeenCalledWith(name);
    expect(wrapper.find('[data-testid="tree-preview-content"]').exists()).toBe(false);
    expect(wrapper.find('[data-testid="tree-explorer-unavailable"]').exists()).toBe(false);
    expect(wrapper.text()).toContain("(no preview)");
    wrapper.unmount();
  });

  it("still reports a server-backed loader's genuine failure as unavailable", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const remoteDirectoryLoader = vi.fn(async () => ({
      entries: [{ name: "notes.md", path: "notes.md", isDir: false }],
    }));
    const remoteContentLoader = vi.fn(async () => {
      throw new Error("Remote task file read failed with HTTP 503.");
    });
    const wrapper = mount(TreeExplorerModal, {
      props: {
        worktreePath: "task-owner-branch",
        repoRoot: "task-owner-branch",
        remoteDirectoryLoader,
        remoteContentLoader,
      },
    });
    await settlePreview();

    expect(wrapper.get('[data-testid="tree-explorer-unavailable"]').text()).toContain(
      "Task files unavailable: Remote task file read failed with HTTP 503.",
    );
    consoleError.mockRestore();
    wrapper.unmount();
  });

  it("shows no preview, rather than this machine's copy, without a content loader", async () => {
    const remoteDirectoryLoader = vi.fn(async () => ({
      entries: [{ name: "notes.md", path: "notes.md", isDir: false }],
    }));
    const wrapper = mount(TreeExplorerModal, {
      props: {
        worktreePath: "task-owner-branch",
        repoRoot: "task-owner-branch",
        remoteDirectoryLoader,
      },
    });
    await settlePreview();

    expect(wrapper.find('[data-testid="tree-preview-content"]').exists()).toBe(false);
    expect(wrapper.text()).toContain("(no preview)");
    expect(invokeMock).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});
