// #3460: Android's native contextmenu after the long-press timer opened the menu must not dismiss it.
// Chromium emits none on a touch hold, so a CDP hold arms the timer and the event is synthesized at both plausible
// targets: the menu under the finger and the row.

import { devices } from "@playwright/test";
import { test, expect } from "./helpers/mockedTest";
import { installSidebarMocks, threeSessionsInOneRepo } from "./helpers/sidebarMocks";
import { openMobileSidebar } from "./helpers/sidebar";

test.use({ ...devices["iPhone 13"] });

const LONG_PRESS_MS = 500;

test("a native contextmenu after the long-press does not dismiss the row menu", async ({ page }) => {
  await installSidebarMocks(page, { sessions: threeSessionsInOneRepo() });

  await page.goto("/");
  await openMobileSidebar(page);

  const row = page.getByTestId("sidebar-session-row").first();
  await expect(row).toBeVisible();
  const box = await row.boundingBox();
  if (!box) throw new Error("session row has no bounding box");
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;

  const menu = page.getByTestId("sidebar-context-menu");
  const cdp = await page.context().newCDPSession(page);

  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x, y, id: 1 }] });
  await page.waitForTimeout(LONG_PRESS_MS + 100);
  await expect(menu).toBeVisible();

  // The menu sits under the finger, which is what makes the inside-menu guard matter.
  const topmostIsMenu = await page.evaluate(
    ({ px, py }) => {
      const target = document.elementFromPoint(px, py);
      const el = document.querySelector('[data-testid="sidebar-context-menu"]');
      return !!target && !!el && el.contains(target);
    },
    { px: x, py: y },
  );
  expect(topmostIsMenu).toBe(true);

  await page.evaluate(
    ({ px, py }) => {
      document.elementFromPoint(px, py)?.dispatchEvent(
        new MouseEvent("contextmenu", {
          bubbles: true,
          cancelable: true,
          composed: true,
          button: 2,
          clientX: px,
          clientY: py,
        }),
      );
    },
    { px: x, py: y },
  );
  await expect(menu).toBeVisible();

  await row.evaluate(
    (el, point) => {
      el.dispatchEvent(
        new MouseEvent("contextmenu", {
          bubbles: true,
          cancelable: true,
          composed: true,
          button: 2,
          clientX: point.px,
          clientY: point.py,
        }),
      );
    },
    { px: x, py: y },
  );
  await expect(menu).toBeVisible();

  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });

  // The guard is a time window: later a tap outside still dismisses. Tap beside the menu, which is nearly full height
  // on a phone and attracts near-miss taps above or below.
  await page.waitForTimeout(LONG_PRESS_MS + 100);
  const outside = await menu.evaluate((el) => {
    const r = el.getBoundingClientRect();
    const gapLeft = r.left;
    const gapRight = window.innerWidth - r.right;
    if (Math.max(gapLeft, gapRight) < 12) throw new Error("no horizontal gap beside the menu");
    return {
      x: Math.round(gapLeft >= gapRight ? gapLeft / 2 : (r.right + window.innerWidth) / 2),
      y: Math.round(r.top + r.height / 2),
    };
  });
  const urlBefore = page.url();
  await page.touchscreen.tap(outside.x, outside.y);
  await expect(menu).toBeHidden();
  // Dismissed by the document listener, not by navigating.
  expect(page.url()).toBe(urlBefore);
});
