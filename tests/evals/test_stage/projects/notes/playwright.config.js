import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "e2e",
  use: { baseURL: "http://127.0.0.1:3100" },
  webServer: {
    command: "PORT=3100 node server/main.js",
    url: "http://127.0.0.1:3100",
    reuseExistingServer: false,
  },
});
