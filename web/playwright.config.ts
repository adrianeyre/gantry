import { defineConfig, devices } from "@playwright/test";
// @ts-expect-error -- a plain .mjs module shared with the launcher script.
import { BASE_URL } from "./e2e/config.mjs";

// End-to-end tests run against the real binary with Simulated Printers. They
// run locally only (`pnpm --filter @gantry/web e2e`), never in Actions (PRD §11).
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  workers: 1,
  timeout: 30_000,
  expect: { timeout: 5_000 },
  reporter: [["list"]],
  use: {
    baseURL: BASE_URL as string,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: "node e2e/server.mjs",
    url: `${BASE_URL as string}/healthz`,
    timeout: 600_000,
    reuseExistingServer: false,
    stdout: "pipe",
    stderr: "pipe",
  },
});
