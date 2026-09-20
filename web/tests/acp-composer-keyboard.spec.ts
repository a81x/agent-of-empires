import { test, expect } from "./helpers/mockedTest";
import { mockStructuredSessionApis, openStructuredViewFor } from "./helpers/structuredSessionMocks";
import { devices, type Page } from "@playwright/test";

// Mobile keyboard regression for the structured-view composer (#2011).
//
// On iOS regular Safari the layout viewport does NOT shrink when the soft
// keyboard opens (interactive-widget=resizes-content is Chromium/Android only,
// and dvh does not track the iOS keyboard), so the composer footer was left
// pinned to the full-height bottom edge, hidden behind the keyboard. The fix
// reserves `keyboardHeight` as bottom padding on the structured-view root so
// the chat viewport absorbs the shrink and the composer rises above the
// keyboard. On platforms where innerHeight shrinks with the keyboard (iOS PWA,
// iOS 26 Safari, Android Chrome) `keyboardHeight` is 0, so the reservation is a
// no-op and the existing dvh path is untouched.
//
// We render the structured view in mocked mode (one running ACP session, no
// live agent) and drive the iOS keyboard by overriding visualViewport, the
// same technique as mobile-keyboard.spec.ts.

test.use({ ...devices["iPhone 13"] });

const SESSION_ID = "sess-acp-kbd";
const TITLE = "acp-kbd";

async function setup(page: Page) {
  await mockStructuredSessionApis(page, { id: SESSION_ID, title: TITLE, projectPath: "/tmp/acp-kbd" });
}

const openStructuredSession = (page: Page) => openStructuredViewFor(page, TITLE);

// Override visualViewport.height (and optionally innerHeight) to mimic the soft
// keyboard, then fire the resize the hook listens for.
async function simulateKeyboardOpen(page: Page, keyboardPx: number, opts: { innerHeightShrinks?: boolean } = {}) {
  await page.evaluate(
    ({ keyboardPx, shrinkInner }) => {
      const vv = window.visualViewport;
      if (!vv) return;
      const newVvH = window.innerHeight - keyboardPx;
      Object.defineProperty(vv, "height", {
        get: () => newVvH,
        configurable: true,
      });
      Object.defineProperty(vv, "offsetTop", {
        get: () => 0,
        configurable: true,
      });
      if (shrinkInner) {
        Object.defineProperty(window, "innerHeight", {
          get: () => newVvH,
          configurable: true,
        });
      }
      vv.dispatchEvent(new Event("resize"));
    },
    { keyboardPx, shrinkInner: opts.innerHeightShrinks ?? false },
  );
}

async function rootPaddingBottom(page: Page): Promise<number> {
  return page.evaluate(() => {
    const root = document.querySelector<HTMLElement>('[data-testid="structured-view-root"]');
    return parseInt(root?.style.paddingBottom || "0") || 0;
  });
}

test.describe("Structured-view composer keyboard reservation (#2011)", () => {
  test("reserves keyboard height so the composer clears the keyboard on iOS Safari (innerHeight constant)", async ({
    page,
  }) => {
    await setup(page);
    await openStructuredSession(page);

    // No keyboard: the root carries no bottom reservation.
    expect(await rootPaddingBottom(page)).toBe(0);

    // iOS regular Safari: visualViewport shrinks but innerHeight stays full.
    await simulateKeyboardOpen(page, 300);
    // The root reserves ~keyboard height so the flex-1 viewport shrinks and the
    // composer lifts above the keyboard.
    await expect.poll(() => rootPaddingBottom(page)).toBeGreaterThanOrEqual(250);
  });

  test("does NOT reserve when the layout viewport already shrinks (PWA / Android, innerHeight shrinks)", async ({
    page,
  }) => {
    await setup(page);
    await openStructuredSession(page);

    const root = await page.getByTestId("structured-view-root").elementHandle();
    expect(root).not.toBeNull();
    const reservation = () =>
      root!.evaluate((element) => ({
        connected: element.isConnected,
        padding: parseInt(element.style.paddingBottom || "0") || 0,
      }));
    await simulateKeyboardOpen(page, 300);
    await expect.poll(async () => (await reservation()).padding).toBeGreaterThanOrEqual(250);
    await simulateKeyboardOpen(page, 300, { innerHeightShrinks: true });
    await expect.poll(reservation).toEqual({ connected: true, padding: 0 });
  });
});
