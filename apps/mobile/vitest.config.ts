import { defineConfig } from "vitest/config";
import { fileURLToPath } from "node:url";
import { sharedTestOptions } from "../../vitest.shared";

export default defineConfig({
  test: {
    ...sharedTestOptions,
    setupFiles: [
      ...sharedTestOptions.setupFiles,
      fileURLToPath(new URL("./src/test/vitest.setup.ts", import.meta.url)),
    ],
  },
});
