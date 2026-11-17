// Playwright tests for the web version (tests/web/, SPEC §7). They load the
// built site from target/site/: run `pnpm build:web` first, then
// `pnpm test:web`.
import { defineConfig, devices } from "@playwright/test";

const PORT = 8123;

export default defineConfig({
  testDir: "tests/web",
  timeout: 60_000,
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? [["list"], ["github"]] : "list",
  use: {
    baseURL: `http://127.0.0.1:${PORT}/`,
    ...devices["Desktop Chrome"],
    deviceScaleFactor: 1,
    // CI runners have no GPU: Chromium's WebGL2 then runs on SwiftShader,
    // which it only allows with this flag.
    launchOptions: { args: ["--enable-unsafe-swiftshader"] },
  },
  projects: [{ name: "chromium" }],
  webServer: {
    command: `node scripts/serve-web.mjs ${PORT}`,
    url: `http://127.0.0.1:${PORT}/`,
    reuseExistingServer: !process.env.CI,
  },
});
