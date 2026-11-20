// Playwright tests for the web version (tests/web/, SPEC §7). They load the
// built site from target/site/: run `pnpm build:web` first, then
// `pnpm test:web`.
import { defineConfig, devices } from "@playwright/test";

const PORT = 8123;

export default defineConfig({
  testDir: "tests/web",
  timeout: 60_000,
  forbidOnly: !!process.env.CI,
  // One worker on CI. The runner has no GPU, so every page draws on
  // SwiftShader in the CPU; with two workers each one's frames hold the
  // other's timers back by up to 400 ms, enough to miss the real-time
  // budgets some tests check (Take over's steal after 1 s, SPEC §3.5).
  workers: process.env.CI ? 1 : undefined,
  reporter: process.env.CI ? [["list"], ["github"]] : "list",
  use: {
    baseURL: `http://127.0.0.1:${PORT}/DeskTank/`,
    ...devices["Desktop Chrome"],
    deviceScaleFactor: 1,
    // CI runners have no GPU: Chromium's WebGL2 then runs on SwiftShader,
    // which it only allows with this flag.
    launchOptions: { args: ["--enable-unsafe-swiftshader"] },
  },
  projects: [{ name: "chromium" }],
  webServer: {
    command: `node scripts/serve-web.ts ${PORT} /DeskTank/`,
    url: `http://127.0.0.1:${PORT}/DeskTank/`,
    reuseExistingServer: !process.env.CI,
  },
});
