// Schema controls persist at their declared scope and survive reloads.
// Unrelated saves preserve temporary runtime settings; map edits allow removal.

import { test, expect } from "../helpers/liveTest";

test("a schema-driven select persists through the UI and a reload", async ({ serve, page }) => {
  const profiles: Array<{ name: string; is_default?: boolean }> = await fetch(`${serve.baseUrl}/api/profiles`).then(
    (r) => r.json(),
  );
  const defaultProfile = profiles.find((p) => p.is_default)?.name ?? profiles[0]?.name ?? "main";
  const profileUrl = `${serve.baseUrl}/api/profiles/${encodeURIComponent(defaultProfile)}/settings`;

  const before = await fetch(profileUrl).then((r) => r.json());
  const baseline = (before?.tmux?.status_bar as string | undefined) ?? "auto";
  const next = baseline === "enabled" ? "disabled" : "enabled";

  await page.goto(`${serve.baseUrl}/settings/tmux`);

  const statusBar = page
    .locator("label", { hasText: /^Status Bar$/ })
    .locator("..")
    .locator("select");
  await expect(statusBar).toBeVisible({ timeout: 10_000 });

  // Schema-driven SelectField saves on change.
  await statusBar.selectOption(next);

  // Server-side: the edit reached the profile config.
  await expect(async () => {
    const after = await fetch(profileUrl).then((r) => r.json());
    expect(after?.tmux?.status_bar).toBe(next);
  }).toPass({ timeout: 5_000 });

  // Frontend-side: the persisted value is read back after a reload.
  await page.reload();
  const statusBarAfter = page
    .locator("label", { hasText: /^Status Bar$/ })
    .locator("..")
    .locator("select");
  await expect(statusBarAfter).toHaveValue(next, { timeout: 10_000 });
});

test("Sidebar Position stays global across profile overrides and a server restart", async ({ serve, page }) => {
  const profiles: Array<{ name: string; is_default?: boolean }> = await fetch(`${serve.baseUrl}/api/profiles`).then(
    (r) => r.json(),
  );
  const profile = profiles.find((p) => p.is_default)!.name;
  const globalUrl = `${serve.baseUrl}/api/settings`;
  const profileUrl = `${serve.baseUrl}/api/profiles/${encodeURIComponent(profile)}/settings`;
  const effectiveUrl = `${globalUrl}?profile=${encodeURIComponent(profile)}`;
  const staleOverride = await fetch(profileUrl, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ session: { sidebar_position: "left" } }),
  });
  expect(staleOverride.ok).toBe(true);
  const logUrl = `${serve.baseUrl}/api/log-level`;
  const runtimeLog = await fetch(logUrl, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ level: "debug" }),
  });
  expect(runtimeLog.ok).toBe(true);
  const { current: temporaryFilter } = await runtimeLog.json();

  await page.goto(`${serve.baseUrl}/settings/session`);
  const position = page
    .locator("label", { hasText: /^Sidebar Position$/ })
    .locator("..")
    .locator("select");
  await expect(position).toHaveValue("left");
  const [saveResponse] = await Promise.all([
    page.waitForResponse((response) => response.url() === globalUrl && response.request().method() === "PATCH"),
    position.selectOption("right"),
  ]);
  expect(saveResponse.ok()).toBe(true);

  for (const url of [globalUrl, effectiveUrl]) {
    const saved = await fetch(url).then((r) => r.json());
    expect(saved.session.sidebar_position).toBe("right");
  }
  const overrides = await fetch(profileUrl).then((r) => r.json());
  expect(overrides.session.sidebar_position).toBe("left");
  const logStatus = await fetch(logUrl).then((r) => r.json());
  expect(logStatus.current).toBe(temporaryFilter);

  await serve.restart();
  await page.reload();
  await expect(position).toHaveValue("right");
  const persisted = await fetch(globalUrl).then((r) => r.json());
  expect(persisted.session.sidebar_position).toBe("right");
});

test("clearing a logging target removes its global override", async ({ serve, page }) => {
  await page.goto(`${serve.baseUrl}/settings/logging`);
  const target = page
    .locator("label", { hasText: /^acp\.protocol$/ })
    .locator("..")
    .locator("select");
  await expect(target).toBeVisible();
  await target.selectOption("debug");
  const globalUrl = `${serve.baseUrl}/api/settings`;
  await expect(async () => {
    const saved = await fetch(globalUrl).then((r) => r.json());
    expect(saved.logging.targets["acp.protocol"]).toBe("debug");
  }).toPass({ timeout: 5_000 });

  await target.selectOption("");
  await expect(async () => {
    const saved = await fetch(globalUrl).then((r) => r.json());
    expect(saved.logging.targets).toEqual({});
  }).toPass({ timeout: 5_000 });
  await page.reload();
  await expect(target).toHaveValue("");
});
