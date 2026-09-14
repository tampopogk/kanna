import { describe, expect, it, vi } from "vitest";
import type {
  TaskFileDownloadAdapter
} from "./taskFileDownload";

vi.mock("expo-file-system", () => ({
  Directory: class {},
  File: class {},
  Paths: { cache: {} }
}));
vi.mock("expo-sharing", () => ({
  isAvailableAsync: vi.fn(),
  shareAsync: vi.fn()
}));

const { shareTaskFile } = await import("./taskFileDownload");

function adapter(overrides: Partial<TaskFileDownloadAdapter> = {}) {
  const cleanup = vi.fn();
  const result: TaskFileDownloadAdapter = {
    createTemporaryFile: vi.fn(() => ({ cleanup, uri: "file:///cache/logo.PNG" })),
    isSharingAvailable: vi.fn().mockResolvedValue(true),
    share: vi.fn().mockResolvedValue(undefined),
    ...overrides
  };
  return { adapter: result, cleanup };
}

describe("shareTaskFile", () => {
  it("writes exact binary bytes and shares the original filename and MIME", async () => {
    const harness = adapter();

    await shareTaskFile(
      {
        dataBase64: "iVBORw0KGgr/",
        fileName: "logo.PNG",
        mediaType: "image/png"
      },
      harness.adapter
    );

    expect(harness.adapter.createTemporaryFile).toHaveBeenCalledOnce();
    const [fileName, bytes] = vi.mocked(harness.adapter.createTemporaryFile).mock
      .calls[0];
    expect(fileName).toBe("logo.PNG");
    expect(Array.from(bytes)).toEqual([
      0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0xff
    ]);
    expect(harness.adapter.share).toHaveBeenCalledWith(
      "file:///cache/logo.PNG",
      "image/png",
      "logo.PNG"
    );
    expect(harness.cleanup).toHaveBeenCalledOnce();
  });

  it("cleans up after the share sheet rejects or is cancelled", async () => {
    const harness = adapter({
      share: vi.fn().mockRejectedValue(new Error("Share cancelled"))
    });

    await expect(
      shareTaskFile(
        {
          dataBase64: "AA==",
          fileName: "cancel.bin",
          mediaType: "application/octet-stream"
        },
        harness.adapter
      )
    ).rejects.toThrow("Share cancelled");
    expect(harness.cleanup).toHaveBeenCalledOnce();
  });

  it("does not create a temporary file when native sharing is unavailable", async () => {
    const harness = adapter({
      isSharingAvailable: vi.fn().mockResolvedValue(false)
    });

    await expect(
      shareTaskFile(
        {
          dataBase64: "AA==",
          fileName: "file.bin",
          mediaType: "application/octet-stream"
        },
        harness.adapter
      )
    ).rejects.toThrow("not available");
    expect(harness.adapter.createTemporaryFile).not.toHaveBeenCalled();
  });

  it.each(["../secret", "nested/file.png", "nested\\file.png", ""])(
    "rejects an unsafe response filename: %s",
    async (fileName) => {
      const harness = adapter();
      await expect(
        shareTaskFile(
          { dataBase64: "AA==", fileName, mediaType: "application/octet-stream" },
          harness.adapter
        )
      ).rejects.toThrow("invalid filename");
      expect(harness.adapter.createTemporaryFile).not.toHaveBeenCalled();
    }
  );
});
