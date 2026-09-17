import base from "./playwright.config";
import { defineConfig } from "@playwright/test";

export default defineConfig({
  ...base,
  workers: 4,
  reporter: "line",
  use: { ...base.use, baseURL: "http://localhost:4391" },
  webServer: { command: "npx vite preview --port 4391 --strictPort", port: 4391, reuseExistingServer: false },
});
