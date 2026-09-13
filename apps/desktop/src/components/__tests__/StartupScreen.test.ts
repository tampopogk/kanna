// @vitest-environment happy-dom

import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import appIconSvg from "../../../src-tauri/icons/icon.svg?raw";
import i18n from "../../i18n";
import { createStartupState, type StartupState } from "../../startup";
import StartupScreen from "../StartupScreen.vue";

function mountScreen(state: StartupState, reducedMotion = false) {
  window.matchMedia = ((query: string) => ({
    matches: reducedMotion && query.includes("prefers-reduced-motion"),
    media: query,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  })) as unknown as typeof window.matchMedia;

  return mount(StartupScreen, {
    props: { state },
    global: { plugins: [i18n] },
  });
}

/** The capsule rows in the shipped app icon, as `x,y,width,height,rx`. */
function appIconCapsules(): string[] {
  const capsules: string[] = [];
  for (const match of appIconSvg.matchAll(/<rect ([^>]*?)\/>/g)) {
    const attrs = match[1];
    const value = (name: string) =>
      new RegExp(`(?:^|\\s)${name}="([^"]+)"`).exec(attrs)?.[1] ?? "";
    // The rounded background tile and its border are not capsules.
    if (!value("rx") || Number(value("rx")) > 100) continue;
    capsules.push(
      [value("x"), value("y"), value("width"), value("height"), value("rx")].join(","),
    );
  }
  return capsules;
}

function renderedCapsules(html: string): string[] {
  const clip = /<clipPath[^>]*>([\s\S]*?)<\/clipPath>/.exec(html)?.[1] ?? "";
  return [...clip.matchAll(/<rect ([^>]*?)>/g)].map((match) => {
    const attrs = match[1];
    const value = (name: string) =>
      new RegExp(`(?:^|\\s)${name}="([^"]+)"`).exec(attrs)?.[1] ?? "";
    return [value("x"), value("y"), value("width"), value("height"), value("rx")].join(",");
  });
}

/** Largest per-channel difference between two `#rrggbb` colours. */
function channelDistance(a: string, b: string): number {
  const channels = (hex: string) => [1, 3, 5].map((at) => Number.parseInt(hex.slice(at, at + 2), 16));
  const [ar, ag, ab] = channels(a);
  const [br, bg, bb] = channels(b);
  return Math.max(Math.abs(ar - br), Math.abs(ag - bg), Math.abs(ab - bb));
}

describe("StartupScreen", () => {
  it("announces the current phase politely beside a decorative icon", async () => {
    const state = createStartupState();
    const wrapper = mountScreen(state);

    const status = wrapper.get('[data-testid="startup-status"]');
    expect(status.attributes("role")).toBe("status");
    expect(status.text()).toBe("Starting Kanna…");
    expect(wrapper.get('[data-testid="startup-icon"]').attributes("aria-hidden")).toBe("true");

    state.phase.value = "services";
    await wrapper.vm.$nextTick();
    expect(wrapper.get('[data-testid="startup-status"]').text()).toBe("Starting local services…");

    state.phase.value = "restoring";
    await wrapper.vm.$nextTick();
    expect(wrapper.get('[data-testid="startup-status"]').text()).toBe("Restoring your workspace…");

    wrapper.unmount();
  });

  it("holds the status in one region instead of moving it with the phase", () => {
    const wrapper = mountScreen(createStartupState());

    // One region, present in every phase, so the icon above it never shifts.
    expect(wrapper.findAll(".kn-startup__status-region")).toHaveLength(1);
    expect(wrapper.find('[data-testid="startup-long-wait-hint"]').exists()).toBe(false);

    wrapper.unmount();
  });

  it("adds a stationary explanation for a long wait without changing the phase", async () => {
    const state = createStartupState();
    state.phase.value = "services";
    const wrapper = mountScreen(state);

    state.longWait.value = true;
    await wrapper.vm.$nextTick();

    expect(wrapper.get('[data-testid="startup-long-wait-hint"]').text()).toBe(
      "Your workspace opens when local setup finishes.",
    );
    // Still the same truthful phase: elapsed time is not a new state.
    expect(wrapper.get('[data-testid="startup-status"]').text()).toBe("Starting local services…");

    wrapper.unmount();
  });

  it("reports a real failure as an alert asking for a restart, with no retry offer", async () => {
    const state = createStartupState();
    const wrapper = mountScreen(state);

    state.phase.value = "failed";
    state.failureDetail.value = "Kanna could not restore your workspace.";
    await wrapper.vm.$nextTick();

    const failure = wrapper.get('[data-testid="startup-failure"]');
    expect(failure.attributes("role")).toBe("alert");
    expect(failure.text()).toContain("Kanna couldn’t start");
    expect(failure.text()).toContain("Kanna could not restore your workspace.");
    expect(failure.text()).toContain("Quit and reopen Kanna to try again.");
    expect(wrapper.find("button").exists()).toBe(false);
    expect(wrapper.find('[data-testid="startup-status"]').exists()).toBe(false);
    // A stopped animation would freeze the colour field mid-row.
    expect(wrapper.find('[data-testid="startup-icon-flow"]').exists()).toBe(false);

    wrapper.unmount();
  });

  it("draws the mark alone, with no tile behind it", () => {
    const wrapper = mountScreen(createStartupState());
    const svg = wrapper.get('[data-testid="startup-icon"]');
    const html = svg.html();

    // Cropped to the mark's own bounds, not the icon's 512 square.
    expect(svg.attributes("viewBox")).toBe("113 82 288 348");
    // The icon file's rounded tile is the shape macOS puts behind the mark; on
    // the app background it reads as a white box, so nothing paints it here.
    expect(html).not.toContain("#ffffff");
    expect(html).not.toContain("#f3f4f6");
    expect([...html.matchAll(/rx="(\d+(?:\.\d+)?)"/g)].map((match) => Number(match[1]))).not.toContain(105);
    expect(html).not.toContain("stroke=");

    wrapper.unmount();
  });

  it("renders the app icon's own artwork when motion is reduced", () => {
    const wrapper = mountScreen(createStartupState(), true);

    expect(wrapper.find('[data-testid="startup-icon-flow"]').exists()).toBe(false);
    const html = wrapper.get('[data-testid="startup-icon"]').html();
    // The five original row gradients, unanimated.
    expect([...html.matchAll(/spreadMethod="repeat"/g)]).toHaveLength(0);
    expect(html).toContain("#f72b89");
    expect(html).toContain("#17d34c");

    wrapper.unmount();
  });

  it("flows a seamless colour field downward through the icon's own capsules", () => {
    const wrapper = mountScreen(createStartupState());
    const html = wrapper.get('[data-testid="startup-icon"]').html();

    // Geometry is the shipped icon's, not a redrawn mark.
    expect(renderedCapsules(html)).toEqual(appIconCapsules());

    const gradients = [...html.matchAll(/<linearGradient([^>]*)>([\s\S]*?)<\/linearGradient>/g)]
      .filter((match) => match[1].includes('spreadMethod="repeat"'));
    expect(gradients.length).toBeGreaterThan(1);

    for (const [, attrs, body] of gradients) {
      // Vertical, so the field travels down the rows rather than across them.
      expect(attrs).toContain('x1="0"');
      expect(attrs).toContain('x2="0"');
      expect(attrs).toContain('y1="99"');
      expect(attrs).toContain('y2="492.125"');
      const stops = [...body.matchAll(/stop-color="([^"]+)"/g)].map((stop) => stop[1]);
      // Six stops: one per row plus a repeat of the first, which is what makes
      // the wrap seamless.
      expect(stops).toHaveLength(6);
      expect(stops[5]).toBe(stops[0]);
      // Every segment stays fully opaque; the owner rejected transparency.
      expect(body).not.toContain("stop-opacity");
    }

    // One period of travel, matching the gradient's own repeat period.
    expect(html).toContain("--kn-startup-flow-distance: 393.125px");
    expect(wrapper.get('[data-testid="startup-icon-flow"]').attributes("style")).toContain(
      "animation-duration: 2875ms",
    );

    wrapper.unmount();
  });

  it("blends between neighbouring columns instead of stepping between them", () => {
    const wrapper = mountScreen(createStartupState());
    const html = wrapper.get('[data-testid="startup-icon"]').html();
    const firstRowColors = [...html.matchAll(/<linearGradient[^>]*spreadMethod="repeat"[^>]*>\s*<stop[^>]*stop-color="([^"]+)"/g)]
      .map((match) => match[1]);

    // The row's own horizontal gradient survives: the leftmost column is all
    // but the row's own left endpoint, adjacent columns differ only slightly,
    // and the green key stays a flat colour at its own end.
    expect(channelDistance(firstRowColors[0], "#f72b89")).toBeLessThan(4);
    expect(firstRowColors[1]).not.toBe(firstRowColors[0]);
    expect(channelDistance(firstRowColors[0], firstRowColors[1])).toBeLessThan(6);
    expect(firstRowColors.at(-1)).toBe("#17d34c");

    wrapper.unmount();
  });
});
