// @vitest-environment happy-dom
import { describe, expect, it, vi } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import en from "../../../i18n/locales/en.json";

const preview = vi.hoisted(() => ({
  acquired: [] as string[],
  released: [] as string[],
}));

vi.mock("../../../utils/artifactPreview", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../utils/artifactPreview")>();
  return {
    ...actual,
    acquireArtifactPreview: async (repoId: string, artifactId: string) => {
      preview.acquired.push(`${repoId}/${artifactId}`);
      return { url: `http://127.0.0.1:41000/a/${"c".repeat(32)}/` };
    },
    releaseArtifactPreview: async (repoId: string, artifactId: string) => {
      preview.released.push(`${repoId}/${artifactId}`);
    },
  };
});

import DesignMockupFrame from "../DesignMockupFrame.vue";

const i18n = () => createI18n({ legacy: false, locale: "en", messages: { en } });
const first = "a".repeat(40);
const second = "b".repeat(40);

describe("DesignMockupFrame", () => {
  it("frames the published page through the store's sandboxing shell and lets go of it", async () => {
    const wrapper = mount(DesignMockupFrame, {
      props: { repoId: "repo-1", artifactId: first, entrypoint: "screens/index.html", title: "Static mockup" },
      global: { plugins: [i18n()] },
    });
    await flushPromises();
    const frame = wrapper.get("[data-testid='design-mockup-frame']");
    expect(frame.attributes("src")).toBe(
      `http://127.0.0.1:41000/a/${"c".repeat(32)}/screens/index.html?kanna-shell`,
    );
    expect(frame.attributes("sandbox")).toBe("allow-scripts allow-same-origin");
    expect(frame.attributes("title")).toBe("Static mockup");

    // A newly published page replaces the old one, which is released.
    await wrapper.setProps({ artifactId: second, entrypoint: "index.html" });
    await flushPromises();
    expect(preview.acquired).toEqual([`repo-1/${first}`, `repo-1/${second}`]);
    expect(preview.released).toEqual([`repo-1/${first}`]);

    wrapper.unmount();
    expect(preview.released).toEqual([`repo-1/${first}`, `repo-1/${second}`]);
  });
});
