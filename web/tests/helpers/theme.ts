// Custom theme fixtures (#1405).

import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import { appDirFor, resolveAoeBinary } from "./aoeServe";

const MINIMAL_THEME_TOML = `appearance = "dark"

background = "#11131c"
border = "#2b2f3f"
terminal_border = "#5fd7ff"
selection = "#2b2f3f"
session_selection = "#3c4154"
title = "#c4b5fd"
text = "#e6e8ee"
dimmed = "#7a829a"
hint = "#7a829a"
running = "#5fd7af"
waiting = "#ffd178"
fresh_idle = "#ff8a5b"
idle = "#7a829a"
error = "#ff6b6b"
terminal_active = "#5fd7ff"
group = "#5fd7ff"
search = "#fff59d"
accent = "#c4b5fd"
diff_add = "#5fd7af"
diff_delete = "#ff6b6b"
diff_modified = "#ffd178"
diff_header = "#c4b5fd"
help_key = "#c4b5fd"
branch = "#5fd7ff"
sandbox = "#c4b5fd"

[syntax]
shiki_theme = "github-dark"
`;

/** Custom themes need the full Theme struct; partial files are filtered out. */
export const VALID_CUSTOM_THEME_TOML = MINIMAL_THEME_TOML;

/** Fails to parse, but discovery still lists the file stem. */
export const MALFORMED_CUSTOM_THEME_TOML = `appearance = "dark"
this-is-not = "valid theme schema"
[syntax
shiki_theme = "missing closing bracket"
`;

export function customThemesDir(home: string, xdg: string): string {
  const appDir = appDirFor(home, xdg, resolveAoeBinary());
  return join(appDir, "themes");
}

/** Write `<name>.toml` into the app dir's themes/ so the server discovers it on boot. */
export function seedCustomTheme(home: string, xdg: string, name: string, body: string): string {
  const dir = customThemesDir(home, xdg);
  mkdirSync(dir, { recursive: true });
  const path = join(dir, `${name}.toml`);
  writeFileSync(path, body);
  return path;
}

export interface ThemeDocumentSnapshot {
  datasetTheme: string | undefined;
  datasetAppearance: string | undefined;
  colorScheme: string;
  surface900: string;
}

/** The root element state applyResolvedTheme writes. */
export async function readThemeFromDocument(page: Page): Promise<ThemeDocumentSnapshot> {
  return await page.evaluate(() => {
    const root = document.documentElement;
    return {
      datasetTheme: root.dataset.theme,
      datasetAppearance: root.dataset.themeAppearance,
      colorScheme: root.style.colorScheme,
      surface900: root.style.getPropertyValue("--color-surface-900").trim(),
    };
  });
}
