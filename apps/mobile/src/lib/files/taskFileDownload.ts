import { Directory, File, Paths } from "expo-file-system";
import * as Sharing from "expo-sharing";

export interface DownloadableTaskFile {
  dataBase64: string;
  fileName: string;
  mediaType: string;
}

export interface TaskFileDownloadAdapter {
  createTemporaryFile(
    fileName: string,
    bytes: Uint8Array
  ): { cleanup(): void; uri: string };
  isSharingAvailable(): Promise<boolean>;
  share(uri: string, mediaType: string, fileName: string): Promise<void>;
}

function decodeBase64(dataBase64: string): Uint8Array {
  if (
    dataBase64.length % 4 !== 0 ||
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(
      dataBase64
    )
  ) {
    throw new Error("The desktop returned invalid file bytes.");
  }
  const binary = atob(dataBase64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

function validateFileName(fileName: string): void {
  if (
    !fileName ||
    fileName === "." ||
    fileName === ".." ||
    fileName.includes("/") ||
    fileName.includes("\\") ||
    fileName.includes("\0")
  ) {
    throw new Error("The desktop returned an invalid filename.");
  }
}

const nativeAdapter: TaskFileDownloadAdapter = {
  createTemporaryFile(fileName, bytes) {
    const directory = new Directory(
      Paths.cache,
      `kanna-file-share-${Date.now()}-${Math.random().toString(36).slice(2)}`
    );
    directory.create();
    try {
      const file = new File(directory, fileName);
      file.create();
      file.write(bytes);
      return {
        uri: file.uri,
        cleanup() {
          if (directory.exists) directory.delete();
        }
      };
    } catch (error) {
      if (directory.exists) directory.delete();
      throw error;
    }
  },
  isSharingAvailable: Sharing.isAvailableAsync,
  share(uri, mediaType, fileName) {
    return Sharing.shareAsync(uri, {
      dialogTitle: `Save or share ${fileName}`,
      mimeType: mediaType
    });
  }
};

export async function shareTaskFile(
  file: DownloadableTaskFile,
  adapter: TaskFileDownloadAdapter = nativeAdapter
): Promise<void> {
  validateFileName(file.fileName);
  if (!(await adapter.isSharingAvailable())) {
    throw new Error("File sharing is not available on this device.");
  }

  const temporary = adapter.createTemporaryFile(
    file.fileName,
    decodeBase64(file.dataBase64)
  );
  try {
    await adapter.share(temporary.uri, file.mediaType, file.fileName);
  } finally {
    temporary.cleanup();
  }
}
