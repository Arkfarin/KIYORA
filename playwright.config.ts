import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "./tests/ui",
  testMatch: "*.spec.ts",
  fullyParallel: true,
  workers: 2,
  use: {
    baseURL: "http://127.0.0.1:1420",
    launchOptions: {
      executablePath:
        process.env.KIYORA_TEST_BROWSER || "/usr/bin/google-chrome-stable",
      args: ["--no-sandbox"],
    },
    screenshot: "only-on-failure",
  },
  webServer: {
    command: "pnpm dev --host 127.0.0.1",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: true,
  },
});
