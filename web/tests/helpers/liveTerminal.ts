// Shared setup for the mocked live-terminal specs: device descriptors, the
// session boot, and the locators/byte readers the assertions key on.

import { devices, expect, type Page } from "@playwright/test";
import { clickSidebarSession, openMobileSidebar } from "./sidebar";
import { makeLiveFrame, mockTerminalApis, seedSettings, type MockHandle } from "./terminal-mocks";

/** iPhone 13 minus `defaultBrowserType`, which `test.use` inside a describe forbids. */
export const iPhone13 = (({ defaultBrowserType: _browser, ...rest }) => rest)(devices["iPhone 13"]);

export const DESKTOP = { viewport: { width: 1280, height: 800 }, hasTouch: false };

export const scroller = (page: Page) => page.locator("[data-live-terminal] > div").first();
export const liveContent = (page: Page) => page.locator("[data-live-content]");

export const liveTexts = (h: MockHandle) => h.liveMessages.map((b) => b.toString("latin1"));
export const liveMatches = (h: MockHandle, re: RegExp) => liveTexts(h).some((s) => re.test(s));

/** Mock the terminal APIs and open the live view of the `pinch-test` session. */
export async function openLiveTerminal(
  page: Page,
  opts: { mobile?: boolean; settings?: Parameters<typeof seedSettings>[1] | null } = {},
): Promise<MockHandle> {
  const handle = await mockTerminalApis(page);
  await page.goto("/");
  if (opts.settings !== null) {
    await seedSettings(page, opts.settings ?? { mobileFontSize: 14 });
    await page.reload();
  }
  if (opts.mobile) await openMobileSidebar(page);
  await clickSidebarSession(page, "pinch-test");
  await page.locator("[data-live-terminal]").first().waitFor({ state: "visible", timeout: 10_000 });
  await handle.waitForLiveReady();
  return handle;
}

/** A standard 24-row frame carrying the alt-screen / mouse flags under test. */
export async function pushModeFrame(
  handle: MockHandle,
  flags: { altScreen: boolean; mouse: boolean; mouseSgr: boolean },
) {
  await handle.pushLiveFrame({ ...makeLiveFrame({ rows: 24, history: 120, window: 24 }), ...flags });
}

/** Wait for the scroller to settle into forwarding (pinned) or reading mode. */
export async function expectScrollMode(page: Page, mode: "forward" | "read") {
  await expect
    .poll(() => scroller(page).getAttribute("class"))
    .toContain(mode === "forward" ? "overflow-hidden" : "overflow-y-auto");
}
