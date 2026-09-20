// Boot endpoints every mocked spec has to stub before the dashboard renders.

import type { Page } from "@playwright/test";

const STATIC_PATHS = ["settings", "themes", "agents", "profiles", "groups", "devices", "docker/status", "about"];

/** `login/status` plus the config GETs the shell fetches on mount. Later
 *  `page.route` registrations win, so a spec can override any of these after. */
export async function mockStaticApis(page: Page, overrides: Record<string, unknown> = {}) {
  await page.route("**/api/login/status", (r) => r.fulfill({ json: { required: false, authenticated: true } }));
  for (const path of STATIC_PATHS) {
    const json = path in overrides ? overrides[path] : path === "docker/status" ? {} : [];
    await page.route(`**/api/${path}`, (r) => r.fulfill({ json }));
  }
}

/** The per-session no-ops: terminal attach, an empty diff, and silent sockets. */
export async function mockSessionShellApis(page: Page) {
  await page.route("**/api/sessions/*/ensure", (r) => r.fulfill({ json: { ok: true } }));
  await page.route("**/api/sessions/*/terminal", (r) => r.fulfill({ status: 200, body: "" }));
  await page.route("**/api/sessions/*/diff/files", (r) =>
    r.fulfill({ json: { files: [], per_repo_bases: [], warning: null } }),
  );
  await page.routeWebSocket(/\/sessions\/.*\/(ws|acp-ws|container-ws)$/, () => {});
}
