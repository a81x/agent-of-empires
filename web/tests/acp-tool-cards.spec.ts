// Structured view tool cards against replayed ACP frames.

import { test, expect } from "./helpers/mockedTest";
import { mockAcpSession, openStructuredSession, toolCallStarted, toolCallCompleted, stopped } from "./helpers/acpMock";

// #1568: an edit card's diff scrolls horizontally inside the card; the transcript never does.
test.describe("edit card diff scroll", () => {
  test.use({ viewport: { width: 480, height: 800 } });

  const LONG_LINE = `const x = "${"a".repeat(300)}";`;

  test("edit card diff scrolls horizontally on a narrow viewport", async ({ page }) => {
    const mock = await mockAcpSession(page, {
      title: "story-edit-scroll",
      initialEvents: [
        toolCallStarted({
          id: "tc-edit-1",
          name: "Edit",
          kind: "edit",
          args_preview: JSON.stringify({
            file_path: "big.txt",
            old_string: "const x = 1;",
            new_string: LONG_LINE,
          }),
        }),
      ],
    });
    await openStructuredSession(page, mock);

    const cardHeader = page.getByRole("button").filter({ hasText: "big.txt" }).first();
    await expect(cardHeader).toBeVisible({ timeout: 10_000 });
    await cardHeader.click();

    const diff = page.getByTestId("string-diff");
    await expect(diff).toBeVisible({ timeout: 10_000 });

    const overflowX = await diff.evaluate((el) => getComputedStyle(el).overflowX);
    expect(["auto", "scroll"]).toContain(overflowX);

    // The content really overflows, so the scroll context is not vacuous.
    await expect
      .poll(async () => diff.evaluate((el) => (el as HTMLElement).scrollWidth - (el as HTMLElement).clientWidth))
      .toBeGreaterThan(0);

    const viewport = page.getByTestId("acp-viewport");
    await expect(viewport).toBeVisible();
    await expect
      .poll(async () => viewport.evaluate((el) => (el as HTMLElement).scrollWidth - (el as HTMLElement).clientWidth))
      .toBeLessThanOrEqual(0);
  });
});

// #1467: a failed tool card opens on its own but its header still folds it.
test("failed tool card auto-opens and folds via the chevron", async ({ page }) => {
  const ERROR_TEXT = "boom: the command exploded";
  const mock = await mockAcpSession(page, {
    title: "story-fold-fail",
    initialEvents: [
      toolCallStarted({
        id: "tc-fail-1",
        name: "Terminal",
        kind: "execute",
        args_preview: JSON.stringify({ command: "rm -rf /nope" }),
      }),
      toolCallCompleted({
        tool_call_id: "tc-fail-1",
        is_error: true,
        content: ERROR_TEXT,
      }),
      stopped(),
    ],
  });
  await openStructuredSession(page, mock);

  const errorText = page.getByText(ERROR_TEXT);
  await expect(errorText).toBeVisible({ timeout: 10_000 });
  await expect(page.getByText("tool failed")).toBeVisible();

  const cardHeader = page
    .getByRole("button")
    .filter({ hasText: /failed/i })
    .first();
  await cardHeader.click();
  await expect(errorText).toBeHidden({ timeout: 10_000 });

  await cardHeader.click();
  await expect(errorText).toBeVisible({ timeout: 10_000 });
});
