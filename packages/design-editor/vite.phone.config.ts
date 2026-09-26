import { defineConfig } from "vite";

/**
 * The phone's design page as one self-contained script and stylesheet
 * (no chunks, no fonts or assets fetched at runtime), for the mobile app to
 * embed. See scripts/build-phone-assets.ts.
 */
export default defineConfig({
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  build: {
    outDir: "dist-phone",
    emptyOutDir: true,
    cssCodeSplit: false,
    assetsInlineLimit: 100_000_000,
    minify: true,
    sourcemap: false,
    lib: {
      entry: "src/phone/main.tsx",
      name: "KannaDesignPhone",
      formats: ["iife"],
      fileName: () => "design-phone.js",
      cssFileName: "design-phone",
    },
    rollupOptions: { output: { inlineDynamicImports: true } },
  },
});
