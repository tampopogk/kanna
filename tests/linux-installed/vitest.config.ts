import { join } from "node:path";
import { defineConfig } from "vitest/config";
import { sharedTestOptions } from "../../vitest.shared";

export default defineConfig({
  test: {
    ...sharedTestOptions,
    ...(process.env.KANNA_INSTALLED_EVIDENCE_DIR ? {
      reporters: ["default", "json"],
      outputFile: join(process.env.KANNA_INSTALLED_EVIDENCE_DIR, "vitest-result.json"),
    } : {}),
    // apt, a systemd user unit, a real daemon and a real agent session. The
    // upgrade lane installs two packages in one test.
    testTimeout: 480_000,
    hookTimeout: 480_000,
  },
});
