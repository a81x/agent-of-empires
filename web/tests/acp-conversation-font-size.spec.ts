import { test, expect } from "./helpers/mockedTest";
import { devices, type Page } from "@playwright/test";
import { agentMessageChunk, mockAcpSession, openStructuredSession, stopped } from "./helpers/acpMock";

// The transcript font size has mobile and desktop values chosen by clientFormFactor() (coarse pointer and under
// 768px). A browser test because jsdom evaluates neither pointer media nor rem; pointer capability is fixed per
// context, so each describe owns one and resizes live.

const MOBILE_SIZE = 11;
const DESKTOP_SIZE = 20;

async function openTranscript(page: Page) {
  await page.addInitScript(
    ([mobile, desktop]) => {
      window.localStorage.setItem(
        "aoe-web-settings",
        JSON.stringify({ structuredMobileFontSize: mobile, structuredDesktopFontSize: desktop }),
      );
    },
    [MOBILE_SIZE, DESKTOP_SIZE],
  );

  const mock = await mockAcpSession(page, {
    title: "story-font-size",
    initialEvents: [agentMessageChunk("# heading\n\nplain paragraph text\n\n```\nfenced code\n```"), stopped()],
  });
  await openStructuredSession(page, mock);

  const body = page.locator(".acp-markdown-body").first();
  await expect(body).toBeVisible({ timeout: 10_000 });
  return body;
}

const fontSizeOf = (locator: ReturnType<Page["locator"]>) => locator.evaluate((el) => getComputedStyle(el).fontSize);

const leadingRatioOf = (locator: ReturnType<Page["locator"]>) =>
  locator.evaluate((el) => {
    const cs = getComputedStyle(el);
    return Number.parseFloat(cs.lineHeight) / Number.parseFloat(cs.fontSize);
  });

test.describe("structured view conversation font size (fine pointer)", () => {
  test.use({ viewport: { width: 1200, height: 800 }, hasTouch: false });

  test("uses the desktop size at any width and scales it with the browser root font size", async ({ page }) => {
    const body = await openTranscript(page);
    const heading = body.locator("h1").first();

    expect(await fontSizeOf(body)).toBe("20px");
    expect(await fontSizeOf(heading)).toBe("28.6px");

    // Code keeps its tight leading: --tw-leading does not inherit, so without its own it would take leading-relaxed.
    const codeBlock = body.locator("pre").first();
    expect(await fontSizeOf(codeBlock)).toBe("17.2px");
    expect(await leadingRatioOf(codeBlock)).toBeCloseTo(1.3333, 3);

    await page.setViewportSize({ width: 500, height: 800 });
    await expect.poll(() => fontSizeOf(body)).toBe("20px");

    // Published in rem, so a larger root scales the transcript.
    await page.evaluate(() => {
      document.documentElement.style.fontSize = "20px";
    });
    await expect.poll(() => fontSizeOf(body)).toBe("25px");
  });
});

// defaultBrowserType would force a new worker in a describe-level use.
const { defaultBrowserType: _iphoneBrowser, ...iPhone13 } = devices["iPhone 13"];

test.describe("structured view conversation font size (coarse pointer)", () => {
  test.use(iPhone13);

  test("uses the mobile size when narrow and the desktop size once the viewport widens", async ({ page }) => {
    const body = await openTranscript(page);
    const heading = body.locator("h1").first();

    expect(await fontSizeOf(body)).toBe("11px");
    expect(await fontSizeOf(heading)).toBe("15.73px");

    await page.setViewportSize({ width: 900, height: 800 });
    await expect.poll(() => fontSizeOf(body)).toBe("20px");
    expect(await fontSizeOf(heading)).toBe("28.6px");
  });
});
