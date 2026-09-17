// #1452: on mobile a picker promotes right-panel views into the single full-viewport pane, so the paired terminal
// stays tall under the keyboard.

import { test, expect } from "./helpers/mockedTest";
import { devices, type Page } from "@playwright/test";
import { clickSidebarSession, openMobileSidebar } from "./helpers/sidebar";
import { mockTerminalApis, seedSettings } from "./helpers/terminal-mocks";

test.use({ ...devices["iPhone 13"] });

async function simulateKeyboardOpen(page: Page, keyboardPx: number) {
  await page.evaluate((keyboardPx) => {
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
    vv.dispatchEvent(new Event("resize"));
  }, keyboardPx);
}

async function setupAndOpenSession(page: Page) {
  await mockTerminalApis(page);
  await page.goto("/");
  await seedSettings(page, { mobileFontSize: 10 });
  await page.reload();
  await openMobileSidebar(page);
  await clickSidebarSession(page, "pinch-test");
  await page.locator("[data-live-terminal]").first().waitFor({ state: "visible", timeout: 10_000 });
}

async function openPicker(page: Page) {
  await page.getByRole("button", { name: "Toggle panels" }).click();
  await page.getByTestId("mobile-right-panel-picker").waitFor({
    state: "visible",
    timeout: 5_000,
  });
}

test.describe("Mobile right panel picker (#1452)", () => {
  test("picker promotes the paired terminal and it survives the keyboard", async ({ page }) => {
    await setupAndOpenSession(page);
    await openPicker(page);

    await page.getByTestId("mobile-right-panel-pick-paired").click();
    await expect(page.getByTestId("mobile-right-panel-picker")).toHaveCount(0);
    const paired = page.locator('[data-term="paired"]');
    await paired.waitFor({ state: "visible", timeout: 10_000 });

    // The paired shell reserves the home-indicator inset like the other panes.
    const pairedInset = await page
      .getByTestId("mobile-paired-layer")
      .evaluate((el) => (el as HTMLElement).style.paddingBottom);
    expect(pairedInset).toContain("safe-area-inset-bottom");

    await simulateKeyboardOpen(page, 300);
    await expect
      .poll(async () => (await paired.boundingBox())?.height ?? 0, {
        message: "paired terminal collapsed under the keyboard",
      })
      .toBeGreaterThan(150);
  });

  test("picker promotes the diff view, opens a file, and the back chip returns to the agent", async ({ page }) => {
    await mockTerminalApis(page);
    await page.route("**/api/sessions/*/diff/files", (r) =>
      r.fulfill({
        json: {
          files: [
            {
              path: "src/foo.ts",
              old_path: null,
              status: "modified",
              additions: 2,
              deletions: 1,
            },
          ],
          per_repo_bases: [{ base_branch: "main" }],
          warning: null,
        },
      }),
    );
    await page.goto("/");
    await openMobileSidebar(page);
    await clickSidebarSession(page, "pinch-test");
    await page.locator("[data-live-terminal]").first().waitFor({ state: "visible", timeout: 10_000 });

    await openPicker(page);
    await page.getByTestId("mobile-right-panel-pick-diff").click();
    await expect(page.getByTestId("mobile-right-panel-picker")).toHaveCount(0);
    const back = page.getByTestId("mobile-back-to-agent");
    await expect(back).toBeVisible();

    const row = page.locator('button[data-index="0"]').first();
    await row.hover();
    await row.click();
    await expect(page.locator('button[data-index="0"]')).toHaveCount(0);
    await expect(back).toBeVisible();

    await back.click();
    await expect(page.getByTestId("mobile-back-to-agent")).toHaveCount(0);
    await expect(page.locator("[data-live-terminal]").first()).toBeVisible();
  });

  test("agent and paired terminals stay mounted across view switches", async ({ page }) => {
    await setupAndOpenSession(page);

    await openPicker(page);
    await page.getByTestId("mobile-right-panel-pick-paired").click();
    await page.locator('[data-term="paired"]').waitFor({
      state: "visible",
      timeout: 10_000,
    });

    // The paired shell stays mounted, keeping its PTY and scrollback.
    await page.getByTestId("mobile-back-to-agent").click();
    await expect(page.locator('[data-term="paired"]')).toHaveCount(1);
    await expect(page.locator("[data-live-terminal]").first()).toBeVisible();
  });
});

test.describe("Desktop right panel split is unchanged (#1452)", () => {
  test.use({ viewport: { width: 1400, height: 900 }, hasTouch: false });

  test("renders the side-by-side split, not the mobile picker", async ({ page }) => {
    await mockTerminalApis(page);
    await page.goto("/");
    await clickSidebarSession(page, "pinch-test");
    await page.locator("[data-live-terminal]").first().waitFor({ state: "visible", timeout: 10_000 });

    await expect(page.getByTestId("content-split-resize-handle")).toBeVisible();
    await expect(page.getByTestId("activity-bar")).toBeVisible();
    await expect(page.getByRole("button", { name: "Toggle panels" })).toHaveCount(0);
    await expect(page.getByTestId("mobile-right-panel-picker")).toHaveCount(0);
  });
});

async function setupAcpSession(page: Page) {
  await mockTerminalApis(page);
  await page.route("**/api/sessions", (r) => {
    if (r.request().method() === "POST") return r.fulfill({ status: 400 });
    return r.fulfill({
      json: {
        sessions: [
          {
            id: "pinch-test",
            title: "acp-mobile",
            project_path: "/tmp/acp-mobile",
            group_path: "/tmp",
            tool: "claude",
            status: "Running",
            yolo_mode: false,
            created_at: new Date().toISOString(),
            last_accessed_at: null,
            last_error: null,
            branch: null,
            main_repo_path: null,
            is_sandboxed: false,
            has_terminal: true,
            profile: "default",
            workspace_repos: [],
            view: "structured",
            acp_worker_state: "running",
          },
        ],
        workspace_ordering: [],
      },
    });
  });
  await page.route("**/api/sessions/*/acp/**", (r) => r.fulfill({ json: {} }));
  await page.goto("/");
  await openMobileSidebar(page);
  await clickSidebarSession(page, "acp-mobile");
  await page.getByRole("button", { name: "Toggle panels" }).waitFor({ state: "visible", timeout: 10_000 });
}

test.describe("Mobile picker on a structured view session (#1452)", () => {
  test("promotes the paired shell over a structured view session and survives the keyboard", async ({ page }) => {
    await setupAcpSession(page);
    await openPicker(page);

    await page.getByTestId("mobile-right-panel-pick-paired").click();
    await expect(page.getByTestId("mobile-right-panel-picker")).toHaveCount(0);
    const paired = page.locator('[data-term="paired"]');
    await paired.waitFor({ state: "visible", timeout: 10_000 });

    await simulateKeyboardOpen(page, 300);
    await expect
      .poll(async () => (await paired.boundingBox())?.height ?? 0, {
        message: "paired terminal collapsed on a structured view session",
      })
      .toBeGreaterThan(150);

    await page.getByTestId("mobile-back-to-agent").click();
    await expect(page.getByTestId("mobile-back-to-agent")).toHaveCount(0);
  });
});
