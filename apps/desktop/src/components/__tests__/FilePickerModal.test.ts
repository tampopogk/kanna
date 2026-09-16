// @vitest-environment happy-dom

import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";
import FilePickerModal from "../FilePickerModal.vue";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("../../invoke", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("../../composables/useToast", () => ({
  useToast: () => ({ warning: vi.fn(), error: vi.fn(), info: vi.fn() }),
}));

async function settle() {
  for (let round = 0; round < 4; round += 1) {
    await Promise.resolve();
    await nextTick();
  }
}

function listedPaths(wrapper: ReturnType<typeof mount>): string[] {
  return wrapper.findAll(".file-item").map((item) => item.text());
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((promiseResolve) => {
    resolve = promiseResolve;
  });
  return { promise, resolve };
}

describe("FilePickerModal", () => {
  afterEach(() => {
    invokeMock.mockReset();
  });

  it("lists the worktree's files", async () => {
    invokeMock.mockResolvedValue(["README.md", "src/index.txt"]);
    const wrapper = mount(FilePickerModal, {
      props: { worktreePath: "/repo-a", repoRoot: "/repo-a", sourceKey: "repo:a" },
      global: { mocks: { $t: (key: string) => key } },
    });
    await settle();

    expect(listedPaths(wrapper)).toEqual(["README.md", "src/index.txt"]);
  });

  it("reloads when the path it points at changes", async () => {
    // The picker stays mounted while hidden, so without this it keeps showing
    // the first path's listing — another repo's files, or nothing at all when
    // that first load pointed at a path that had gone away.
    invokeMock.mockRejectedValueOnce(new Error("not a directory: /gone"));
    const wrapper = mount(FilePickerModal, {
      props: { worktreePath: "/gone", repoRoot: "/gone", sourceKey: "repo:gone" },
      global: { mocks: { $t: (key: string) => key } },
    });
    await settle();
    expect(listedPaths(wrapper)).toEqual([]);

    invokeMock.mockResolvedValue(["README.md", "src/index.txt"]);
    await wrapper.setProps({
      worktreePath: "/repo-b",
      repoRoot: "/repo-b",
      sourceKey: "repo:b",
    });
    await settle();

    expect(listedPaths(wrapper)).toEqual(["README.md", "src/index.txt"]);
  });

  it("recursively lists a remote task through its contained directory loader", async () => {
    const taskDirectoryLoader = vi.fn(async (path: string) => ({
      entries: path === ""
        ? [
            { path: "src", isDir: true },
            { path: "README.md", isDir: false },
          ]
        : [{ path: "src/index.ts", isDir: false }],
    }));
    const wrapper = mount(FilePickerModal, {
      props: {
        worktreePath: "",
        repoRoot: "/viewer/repo-with-a-same-named-file",
        sourceKey: "remote:cloud:machine-b:task-1:task-1-2",
        taskDirectoryLoader,
      },
      global: { mocks: { $t: (key: string, params?: { message?: string }) => params?.message ? `${key}: ${params.message}` : key } },
    });

    await vi.waitFor(() => {
      expect(listedPaths(wrapper)).toEqual(["README.md", "src/index.ts"]);
    });
    expect(taskDirectoryLoader).toHaveBeenNthCalledWith(1, "", false);
    expect(taskDirectoryLoader).toHaveBeenNthCalledWith(2, "src", false);
    expect(invokeMock).not.toHaveBeenCalled();

    await wrapper.get(".file-item").trigger("click");
    expect(wrapper.emitted("select")?.[0]).toEqual([
      "README.md",
      "remote:cloud:machine-b:task-1:task-1-2",
    ]);
  });

  it("drops an old machine's in-flight results when the remote workspace changes", async () => {
    const oldListing = deferred<{ entries: { path: string; isDir: boolean }[] }>();
    const oldLoader = vi.fn(() => oldListing.promise);
    const newLoader = vi.fn(async () => ({
      entries: [
        { path: "README.md", isDir: false },
        { path: "machine-b.txt", isDir: false },
      ],
    }));
    const wrapper = mount(FilePickerModal, {
      props: {
        worktreePath: "",
        sourceKey: "remote:lan:machine-a:task-1:task-1",
        taskDirectoryLoader: oldLoader,
      },
      global: { mocks: { $t: (key: string) => key } },
    });
    await vi.waitFor(() => expect(oldLoader).toHaveBeenCalledWith("", false));

    await wrapper.setProps({
      sourceKey: "remote:lan:machine-b:task-1:task-1-2",
      taskDirectoryLoader: newLoader,
    });
    await vi.waitFor(() => {
      expect(listedPaths(wrapper)).toEqual(["README.md", "machine-b.txt"]);
    });

    oldListing.resolve({
      entries: [
        { path: "README.md", isDir: false },
        { path: "machine-a.txt", isDir: false },
      ],
    });
    await settle();

    expect(listedPaths(wrapper)).toEqual(["README.md", "machine-b.txt"]);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("shows an unavailable state when the owning machine cannot list files", async () => {
    const wrapper = mount(FilePickerModal, {
      props: {
        worktreePath: "",
        sourceKey: "remote:cloud:offline:task-1:task-1",
        taskDirectoryLoader: vi.fn(async () => {
          throw new Error("owner is offline");
        }),
      },
      global: { mocks: { $t: (key: string, params?: { message?: string }) => params?.message ? `${key}: ${params.message}` : key } },
    });

    await vi.waitFor(() => {
      expect(wrapper.get('[data-testid="file-picker-unavailable"]').text()).toContain(
        "owner is offline",
      );
    });
    expect(wrapper.find(".file-item").exists()).toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();
  });
});
