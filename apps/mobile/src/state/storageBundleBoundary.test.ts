import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const stateDirectory = dirname(fileURLToPath(import.meta.url));
const ASYNC_STORAGE_MODULE = "@react-native-async-storage/async-storage";

describe("native storage bundle boundary", () => {
  it.each([
    "sessionPersistence.ts",
    "taskQuickReplyPreferences.ts"
  ])("keeps AsyncStorage in the startup bundle for %s", (filename) => {
    const source = readFileSync(resolve(stateDirectory, filename), "utf8");

    expect(source).toContain(`import AsyncStorage from "${ASYNC_STORAGE_MODULE}"`);
    expect(source).not.toContain(`import("${ASYNC_STORAGE_MODULE}")`);
  });
});
