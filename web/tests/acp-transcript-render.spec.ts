// Structured view transcript rendering against replayed ACP frames.

import type { Locator } from "@playwright/test";
import { test, expect } from "./helpers/mockedTest";
import {
  mockAcpSession,
  openStructuredSession,
  waitForComposerConnected,
  agentMessageChunk,
  stopped,
} from "./helpers/acpMock";

// #1469: unbreakable tokens wrap inside the bubble instead of scrolling the viewport; fenced code still scrolls itself.
test.describe("chat bubble overflow", () => {
  // Narrow viewport so the unbreakable tokens are wider than the bubble.
  test.use({ viewport: { width: 480, height: 800 } });

  const LONG_URL = "https://github.com/njbrake/agent-of-empires/actions/runs/26342421371/job/77546632641";

  // Un-indented so markdown renders a paragraph, not a code block.
  const PW_PROSE =
    "Failure at /Users/seluj78/aoe/agent-of-empires-worktrees/fix-flaky-pw-tests/web/tests/terminal-focus-shortcut.spec.ts:79:48 ────────────────────────────────────";

  const LONG_CODE_LINE = "const x = " + "a".repeat(200) + ";";

  test("long URL, PW paste, and code line stay inside the chat viewport", async ({ page }) => {
    const mock = await mockAcpSession(page, {
      title: "story-overflow",
      initialEvents: [
        agentMessageChunk(
          `Run link: ${LONG_URL}\n\n` + `${PW_PROSE}\n\n` + "```ts\n" + `${LONG_CODE_LINE}\n` + "```\n",
        ),
        stopped(),
      ],
    });
    await openStructuredSession(page, mock);

    const link = page.getByRole("link", { name: LONG_URL });
    await expect(link).toBeVisible({ timeout: 10_000 });

    const viewport = page.getByTestId("acp-viewport");
    await expect(viewport).toBeVisible();

    await expect
      .poll(async () => viewport.evaluate((el) => (el as HTMLElement).scrollWidth - (el as HTMLElement).clientWidth))
      .toBeLessThanOrEqual(0);

    await expect(viewport).toHaveCSS("overflow-x", "hidden");

    const codeScroller: Locator = viewport.locator(".acp-markdown .overflow-x-auto").first();
    await expect(codeScroller).toBeVisible();
    await expect
      .poll(async () => codeScroller.evaluate((el) => getComputedStyle(el).overflowX))
      .toMatch(/^(auto|scroll)$/);

    // The wrap rule does not reach code, so the line stays wider than its box.
    const codePre: Locator = codeScroller.locator("pre").first();
    await expect
      .poll(async () => codePre.evaluate((el) => (el as HTMLElement).scrollWidth > (el as HTMLElement).clientWidth))
      .toBe(true);

    // #2443: the <pre> scrolls rather than clipping.
    await expect.poll(async () => codePre.evaluate((el) => getComputedStyle(el).overflowX)).toMatch(/^(auto|scroll)$/);
  });
});

test("send message via Enter renders agent response", async ({ page }) => {
  const mock = await mockAcpSession(page, {
    title: "story-send-enter",
    onPrompt: () => [agentMessageChunk("Hello from fake ACP agent."), stopped()],
  });
  await openStructuredSession(page, mock);
  await waitForComposerConnected(page);

  const composer = page.getByRole("textbox", { name: /Send a message/i });
  await composer.fill("hello agent");
  await composer.press("Enter");

  await expect(page.getByText("Hello from fake ACP agent.")).toBeVisible({
    timeout: 10_000,
  });
  // The clear can land after the streamed chunk renders.
  await expect(composer).toHaveValue("", { timeout: 5_000 });

  expect(mock.promptBodies.map((b) => b.text)).toEqual(["hello agent"]);
});

// #1472: single newlines survive in the sent user bubble.
test("single newlines in a user message render as line breaks", async ({ page }) => {
  const mock = await mockAcpSession(page, { title: "story-single-newline" });
  await openStructuredSession(page, mock);
  await waitForComposerConnected(page);

  const composer = page.getByRole("textbox", { name: /Send a message/i });
  await composer.fill("line a\nline b\nline c");
  await composer.press("Enter");

  const userBubble = page.locator("div.rounded-br-sm").filter({ hasText: "line a" });
  await expect(userBubble).toBeVisible({ timeout: 10_000 });
  await expect(userBubble.locator("br")).toHaveCount(2);
  await expect(userBubble).toContainText("line b");
  await expect(userBubble).toContainText("line c");
});

// Multiple chunks in one turn render as one concatenated message.
test("multi-chunk agent response assembles in the transcript", async ({ page }) => {
  const mock = await mockAcpSession(page, {
    title: "story-stream",
    onPrompt: () => [agentMessageChunk("Once "), agentMessageChunk("upon "), agentMessageChunk("a time."), stopped()],
  });
  await openStructuredSession(page, mock);
  await waitForComposerConnected(page);

  const composer = page.getByRole("textbox", { name: /Send a message/i });
  await composer.fill("tell me a story");
  await composer.press("Enter");

  await expect(page.getByText("Once upon a time.")).toBeVisible({
    timeout: 10_000,
  });
});
