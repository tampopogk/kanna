import { defineConfig } from "vite";

/** The browser-test harness page as one script and stylesheet. */
export default defineConfig({
  define: { "process.env.NODE_ENV": JSON.stringify("development") },
  build: {
    outDir: "dist-harness",
    emptyOutDir: true,
    cssCodeSplit: false,
    assetsInlineLimit: 100_000_000,
    minify: false,
    lib: {
      entry: "src/testing/harness.tsx",
      name: "KannaDesignHarness",
      formats: ["iife"],
      fileName: () => "harness.js",
      cssFileName: "harness",
    },
    rollupOptions: {
      output: { inlineDynamicImports: true },
      // Mantine marks modules "use client" for React Server Components,
      // which a browser bundle has no use for.
      onwarn(warning, warn) {
        if (warning.code !== "MODULE_LEVEL_DIRECTIVE") warn(warning);
      },
    },
  },
});
