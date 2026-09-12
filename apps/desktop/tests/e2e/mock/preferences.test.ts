import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { buildGlobalKeydownScript } from "../helpers/keyboard";
import { WebDriverClient } from "../helpers/webdriver";
import { cleanupFixtureRepos, createSeedFixtureRepo } from "../helpers/fixture-repo";
import { importTestRepo, resetDatabase } from "../helpers/reset";
import { callVueMethod, tauriInvoke } from "../helpers/vue";

async function activeTabLabel(client: WebDriverClient): Promise<string> {
  return client.executeSync<string>(
    `return document.querySelector(".prefs-panel .tab.active")?.textContent?.trim() ?? "";`
  );
}

async function mainTabKinds(client: WebDriverClient): Promise<string[]> {
  return client.executeSync<string[]>(
    `const tabs = window.__KANNA_E2E__?.setupState?.mainTabs;
     if (!tabs) throw new Error("main tabs are unavailable on setupState");
     return (tabs.tabs?.value ?? []).map((tab) => tab.kind);`
  );
}

describe("preferences", () => {
  const client = new WebDriverClient();
  let fixtureRepoRoot = "";

  beforeAll(async () => {
    await client.createSession();
    await resetDatabase(client);
    fixtureRepoRoot = await createSeedFixtureRepo("task-switch-minimal");
    await importTestRepo(client, fixtureRepoRoot, "preferences-stacking");
    await client.executeSync(`
      if (window.__KANNA_E2E__) {
        window.__KANNA_E2E__.mobileInstallUrl = "https://kanna.build/mobile";
      }
      return true;
    `);
    const configuredInstallUrl = await client.executeSync<string>(
      `return window.__KANNA_E2E__?.mobileInstallUrl || "";`,
    );
    expect(configuredInstallUrl).toBe("https://kanna.build/mobile");
  });

  afterAll(async () => {
    await client.deleteSession();
    await cleanupFixtureRepos(fixtureRepoRoot ? [fixtureRepoRoot] : []);
  });

  it("opens preferences panel when settings button clicked", async () => {
    await client.executeSync(buildGlobalKeydownScript({ key: ",", meta: true }));
    const panel = await client.waitForElement(".prefs-panel", 2000);
    expect(panel).toBeTruthy();
  });

  it("shows preference fields", async () => {
    const panelText = await client.executeSync<string>(
      `return document.querySelector(".prefs-panel")?.textContent || ""`
    );
    // Should contain labels for common settings
    expect(panelText.toLowerCase()).toContain("suspend");
    expect(panelText.toLowerCase()).toContain("ide");
  });

  it("closes preferences panel", async () => {
    await client.executeSync(buildGlobalKeydownScript({ key: "Escape" }));
    await client.waitForNoElement(".prefs-panel", 2_000);
  });

  it("keeps one app-level Preferences dialog open without creating a main tab", async () => {
    await client.executeSync(buildGlobalKeydownScript({ key: ",", meta: true }));
    const panel = await client.waitForElement(".prefs-panel", 2_000);
    expect(panel).toBeTruthy();

    await client.executeSync(buildGlobalKeydownScript({ key: ",", meta: true }));
    await client.waitForElement(".prefs-panel", 2_000);
    expect(await client.findElements(".prefs-panel")).toHaveLength(1);
    expect(await mainTabKinds(client)).not.toContain("preferences");

    await client.executeSync(buildGlobalKeydownScript({ key: "Escape" }));
    await client.waitForNoElement(".prefs-panel", 2_000);
    expect(await mainTabKinds(client)).not.toContain("preferences");
  });

  it("dismisses whichever of New Task and Preferences is visibly on top without changing tabs", async () => {
    await client.executeSync(`
      const tabs = window.__KANNA_E2E__?.setupState?.mainTabs;
      if (!tabs) throw new Error("main tabs are unavailable on setupState");
      tabs.openTab({ kind: "analytics" });
      return true;
    `);
    const tabState = `
      const tabs = window.__KANNA_E2E__.setupState.mainTabs;
      return {
        activeTabId: tabs.activeTabId.value,
        kinds: tabs.tabs.value.map((tab) => tab.kind),
      };
    `;
    const before = await client.executeSync<{ activeTabId: string | null; kinds: string[] }>(tabState);

    await client.executeSync(buildGlobalKeydownScript({ key: ",", meta: true }));
    await client.waitForElement(".prefs-panel", 2_000);
    await client.executeSync(buildGlobalKeydownScript({ key: "N", meta: true, shift: true }));
    await client.waitForText(".modal h3", "New Task", 5_000);
    expect(await client.executeSync<boolean>(`
      const preferences = document.querySelector(".prefs-panel")?.closest(".modal-overlay");
      const newTask = document.querySelector(".modal h3")?.closest(".modal-overlay");
      return Number(getComputedStyle(newTask).zIndex) > Number(getComputedStyle(preferences).zIndex);
    `)).toBe(true);

    await client.executeSync(buildGlobalKeydownScript({ key: "Escape" }));
    await client.waitForNoElement(".modal h3", 2_000);
    expect(await client.findElements(".prefs-panel")).toHaveLength(1);
    expect(await client.executeSync(tabState)).toEqual(before);

    await client.executeSync(buildGlobalKeydownScript({ key: "N", meta: true, shift: true }));
    await client.waitForText(".modal h3", "New Task", 5_000);
    const raised = await callVueMethod(client, "keyboardActions.openPreferences");
    if (raised && typeof raised === "object" && "__error" in raised) {
      throw new Error(String((raised as { __error: string }).__error));
    }
    await expect.poll(() => client.executeSync<boolean>(`
        const preferences = document.querySelector(".prefs-panel")?.closest(".modal-overlay");
        const newTask = document.querySelector(".modal h3")?.closest(".modal-overlay");
        return Number(getComputedStyle(preferences).zIndex) > Number(getComputedStyle(newTask).zIndex);
      `), { timeout: 2_000 })
      .toBe(true);

    await client.executeSync(buildGlobalKeydownScript({ key: "Escape" }));
    await client.waitForNoElement(".prefs-panel", 2_000);
    expect(await client.findElements(".modal h3")).toHaveLength(1);
    expect(await client.executeSync(tabState)).toEqual(before);

    await client.executeSync(buildGlobalKeydownScript({ key: "Escape" }));
    await client.waitForNoElement(".modal h3", 2_000);
  });

  it("shows default settings in the UI", async () => {
    await client.executeSync(buildGlobalKeydownScript({ key: ",", meta: true }));
    const panel = await client.waitForElement(".prefs-panel", 2_000);
    expect(panel).toBeTruthy();

    const values = await client.executeSync<string[]>(
      `return Array.from(document.querySelectorAll(".prefs-panel input, .prefs-panel select"))
        .map((element) => element.value);`
    );
    expect(values).toContain("5");
    expect(values).toContain("30");
  });

  it("shows the current desktop ID on the Account tab", async () => {
    await client.executeSync(buildGlobalKeydownScript({ key: "Escape" }));
    await client.waitForNoElement(".prefs-panel", 2_000);

    const status = await tauriInvoke(client, "mobile_server_status") as { desktopId?: string };
    expect(status.desktopId?.trim()).toBeTruthy();
    const desktopId = status.desktopId!.trim();

    await client.executeSync(buildGlobalKeydownScript({ key: ",", meta: true }));
    await client.waitForElement(".prefs-panel", 2_000);

    const accountTab = await client.findElement('[data-testid="preferences-account-tab"]');
    await client.click(accountTab);

    await client.waitForText(".prefs-panel", "Desktop ID", 2_000);
    await client.waitForText(".prefs-panel", desktopId, 2_000);
  });

  it("shows the mobile access pairing panel on the Mobile tab", async () => {
    await client.executeSync(buildGlobalKeydownScript({ key: "Escape" }));
    await client.waitForNoElement(".prefs-panel", 2_000);

    await client.executeSync(buildGlobalKeydownScript({ key: ",", meta: true }));
    await client.waitForElement(".prefs-panel", 2_000);

    const mobileTab = await client.findElement('[data-testid="preferences-mobile-tab"]');
    await client.click(mobileTab);

    await client.waitForElement('[data-testid="mobile-access-panel"]', 2_000);
    await client.waitForText(".prefs-panel", "Use Kanna on your phone", 2_000);
    const artifacts = resolve(process.cwd(), "../../.tmp/mobile-preferences");
    await mkdir(artifacts, { recursive: true });
    await client.screenshot(resolve(artifacts, "first-use.png"));
    expect(await client.findElements('[data-testid="mobile-access-install-qr"]')).toHaveLength(0);
    await client.click(await client.findElement('[data-testid="mobile-access-install-toggle"]'));
    await client.waitForElement('[data-testid="mobile-access-install-qr"]', 2_000);
    await client.waitForText(".prefs-panel", "phone camera to open the App Store", 2_000);
    expect(await client.findElements('[data-testid="mobile-access-install-link"]')).toHaveLength(0);
    expect(await client.findElements('[data-testid="mobile-access-install-copy"]')).toHaveLength(0);

    await client.click(await client.findElement('[data-testid="mobile-access-start-pairing"]'));
    await client.waitForElement('[data-testid="mobile-access-pairing-qr"]', 2_000);
    expect(await client.findElements('[data-testid="mobile-access-install-qr"]')).toHaveLength(0);
    const code = await client.getText(await client.findElement('[data-testid="mobile-access-pairing-code"]'));
    await client.click(await client.findElement('[data-testid="mobile-access-install-toggle"]'));
    await client.waitForElement('[data-testid="mobile-access-install-qr"]', 2_000);
    expect(await client.findElements('[data-testid="mobile-access-pairing-qr"]')).toHaveLength(0);
    await client.click(await client.findElement('[data-testid="mobile-access-pairing-toggle"]'));
    expect(await client.getText(await client.findElement('[data-testid="mobile-access-pairing-code"]'))).toBe(code);
    await client.click(await client.findElement('[data-testid="mobile-access-troubleshooting-toggle"]'));
    await client.click(await client.findElement('[data-testid="mobile-access-status-refresh"]'));
    expect(await client.getText(await client.findElement('[data-testid="mobile-access-pairing-code"]'))).toBe(code);
    const originalTheme = await client.executeSync<string>("return document.documentElement.dataset.theme || 'dark';");
    for (const theme of ["dark", "light"]) {
      await client.executeSync(`document.documentElement.dataset.theme = ${JSON.stringify(theme)}; return true;`);
      await client.executeSync(`document.querySelector('[data-testid="mobile-access-pairing-qr"]').scrollIntoView({ block: 'center' }); return true;`);
      await client.screenshot(resolve(artifacts, `active-${theme}.png`));
    }
    // The driver dispatches synthetic KeyboardEvents without native button
    // activation. Check focus and semantic button activation directly.
    await client.executeSync(`document.querySelector('[data-testid="mobile-access-install-toggle"]').focus(); return true;`);
    expect(await client.executeSync("return document.activeElement?.tagName;")).toBe("BUTTON");
    await client.executeSync("document.activeElement.click(); return true;");
    await client.waitForElement('[data-testid="mobile-access-install-qr"]');
    expect(await client.executeSync("return document.activeElement?.getAttribute('data-testid');")).toBe("mobile-access-install-toggle");
    await client.executeSync(`
      document.querySelector('.prefs-panel').style.width = '320px';
      document.querySelector('.mobile-body').style.maxHeight = '350px';
      return true;
    `);
    const layout = await client.executeSync<{ overflow: boolean; scrollable: boolean }>(`
      const body = document.querySelector('.mobile-body');
      return { overflow: body.scrollWidth > body.clientWidth, scrollable: body.scrollHeight > body.clientHeight };
    `);
    expect(layout).toEqual({ overflow: false, scrollable: true });
    await client.executeSync(`document.querySelector('[data-testid="mobile-access-install-qr"]').scrollIntoView({ block: 'center' }); return true;`);
    await client.screenshot(resolve(artifacts, "narrow.png"));
    await client.executeSync(`
      document.documentElement.dataset.theme = ${JSON.stringify(originalTheme)};
      document.querySelector('.prefs-panel').style.width = '';
      document.querySelector('.mobile-body').style.maxHeight = '';
      window.__KANNA_E2E__.failNextInvoke = 'create_mobile_pairing_session';
      return true;
    `);
    await client.click(await client.findElement('[data-testid="mobile-access-start-pairing"]'));
    await client.waitForText('.mobile-body', 'Could not create a pairing code');
    await client.screenshot(resolve(artifacts, "pairing-error.png"));
    await client.click(await client.findElement('[data-testid="mobile-access-start-pairing"]'));
    await client.waitForElement('[data-testid="mobile-access-pairing-code"]');

  });

  it("persists app and terminal code theme preferences", async () => {
    await client.executeSync(buildGlobalKeydownScript({ key: "Escape" }));
    await client.waitForNoElement(".prefs-panel", 2_000);

    await client.executeSync(buildGlobalKeydownScript({ key: ",", meta: true }));
    await client.waitForElement(".prefs-panel", 2_000);

    await client.executeSync(`
      const appTheme = document.querySelector('[data-testid="app-theme-select"]');
      const codeTheme = document.querySelector('[data-testid="code-theme-select"]');
      appTheme.value = "light";
      appTheme.dispatchEvent(new Event("change", { bubbles: true }));
      codeTheme.value = "dark";
      codeTheme.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    `);
    await client.executeAsync(`
      const cb = arguments[arguments.length - 1];
      setTimeout(() => cb(true), 250);
    `);

    const attrs = await client.executeSync<{ theme?: string; codeTheme?: string }>(`
      return {
        theme: document.documentElement.dataset.theme,
        codeTheme: document.documentElement.dataset.codeTheme,
      };
    `);
    expect(attrs).toEqual({ theme: "light", codeTheme: "dark" });

    await client.executeSync(buildGlobalKeydownScript({ key: "Escape" }));
    await client.waitForNoElement(".prefs-panel", 2_000);
    await client.deleteSession();
    await client.createSession();

    const persisted = await client.executeSync<{ appTheme?: string; codeTheme?: string }>(`
      const unwrap = (value) => value && value.__v_isRef ? value.value : value;
      return window.__KANNA_E2E__?.setupState
        ? {
            appTheme: unwrap(window.__KANNA_E2E__.setupState.store?.appTheme),
            codeTheme: unwrap(window.__KANNA_E2E__.setupState.store?.codeTheme),
          }
        : {};
    `);
    expect(persisted).toEqual({ appTheme: "light", codeTheme: "dark" });
  });

  // The palette stacks on top of Preferences rather than replacing it, so its
  // tab commands dispatch through AppModalLayer into the dialog underneath.
  it("cycles Preferences sections from the command palette", async () => {
    await client.executeSync(buildGlobalKeydownScript({ key: ",", meta: true }));
    await client.waitForElement(".prefs-panel", 2_000);
    expect(await activeTabLabel(client)).toBe("Preferences");

    await client.executeSync(buildGlobalKeydownScript({ key: "P", meta: true, shift: true }));
    await client.waitForElement(".palette-modal", 2_000);
    expect(await client.findElements(".prefs-panel")).toHaveLength(1);

    const labels = await client.executeSync<string[]>(
      `return Array.from(document.querySelectorAll(".palette-modal .command-label"))
        .map((element) => element.textContent?.trim() ?? "");`
    );
    expect(labels).toContain("Previous Tab");
    expect(labels).toContain("Next Tab");
    // The original bug: untranslated keys leaking into the palette.
    expect(labels.filter((label) => label.startsWith("shortcuts."))).toEqual([]);

    const input = await client.waitForElement(".palette-modal .palette-input");
    await client.sendKeys(input, "Next Tab");
    await client.waitForText(".palette-modal .command-item", "Next Tab", 2_000);
    const clicked = await client.executeSync<boolean>(
      `const command = Array.from(document.querySelectorAll(".palette-modal .command-item"))
        .find((element) => element.textContent?.includes("Next Tab"));
       if (!command) return false;
       command.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
       return true;`
    );
    expect(clicked).toBe(true);

    await client.waitForNoElement(".palette-modal", 5_000);
    await client.waitForElement(".prefs-panel", 2_000);
    expect(await activeTabLabel(client)).toBe("Account");
  });
});
