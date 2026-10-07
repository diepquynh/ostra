import { defineConfig } from "@playwright/test";
import { BROWSER, EVIL_HOST } from "./lib/env";

export default defineConfig({
  testDir: "specs",
  globalSetup: "./global-setup.ts",
  // One scratch server and one PTY per run; specs that type into it must not overlap.
  workers: 1,
  fullyParallel: false,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  reporter: [["list"], ["html", { open: "never", outputFolder: "playwright-report" }]],
  outputDir: "test-results",
  use: {
    headless: true,
    trace: "retain-on-failure",
    launchOptions: {
      ...BROWSER,
      args: [`--host-resolver-rules=MAP ${EVIL_HOST} 127.0.0.1`],
    },
  },
});
