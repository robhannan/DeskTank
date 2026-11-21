// The legacy-Chromium (webOS 5, Chrome 68) build and the startup
// diagnostics (SPEC D26). Chrome 68 itself can't be driven here, so this
// runs target/site-webos/ (`pnpm build:web:webos`, served at /webos/) in
// current Chromium: the lowered wasm and JS must still load and draw, and
// the canvas stays at 1 device pixel per CSS pixel on a DPR 2 screen like
// the TV's. scripts/check-webos.ts checks the build is Chrome 68 code.
import { existsSync } from "node:fs";
import path from "node:path";
import { expect, type Page, test } from "@playwright/test";
import { checkJs } from "../../scripts/check-webos.ts";
import { framesDrawn, watchErrors } from "./support.ts";

const built = existsSync(path.join(import.meta.dirname, "..", "..", "target", "site-webos", "index.html"));

async function ready(page: Page, url: string) {
  await page.goto(url);
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
  await expect.poll(() => framesDrawn(page), { timeout: 15_000 }).toBeGreaterThan(0);
}

const canvasPixels = (page: Page) =>
  page.evaluate(() => {
    const c = document.querySelector<HTMLCanvasElement>("#tank")!;
    const r = c.getBoundingClientRect();
    return { width: c.width, height: c.height, cssWidth: Math.round(r.width), cssHeight: Math.round(r.height) };
  });

// The checker itself, so a regression can't wave everything through.
test("check-webos rejects syntax Chrome 68 lacks and accepts what it has", () => {
  for (const bad of ["a?.b;", "a ?? b;", "1n;", 'export * as ns from "./x.js";', "class A { x = 1 }", "1_000;"]) {
    expect(checkJs(bad), bad).not.toEqual([]);
  }
  const ok = 'const u = new URL("x", import.meta.url); import("./m.js"); try {} catch {} async function f(a) { for await (const x of a) {} return { ...a }; }';
  expect(checkJs(ok)).toEqual([]);
});

test.describe("webOS build", () => {
  test.use({ deviceScaleFactor: 2 });
  // CI builds it before the tests; locally it is optional.
  test.skip(!built && !process.env.CI, "target/site-webos/ not built (pnpm build:web:webos)");

  test("loads, draws at 1 device px per CSS px and lists its startup", async ({ page }) => {
    const errors = watchErrors(page);
    await ready(page, "/webos/?debug=1");
    expect(await page.evaluate(() => document.documentElement.dataset.build)).toBe("webos");
    const px = await canvasPixels(page);
    expect(px.width).toBe(px.cssWidth);
    expect(px.height).toBe(px.cssHeight);
    const steps = await page.evaluate(() => window.desktank.startup);
    expect(steps.some((s) => s.includes("first frame drawn")), steps.join("\n")).toBe(true);
    await expect(page.locator("#debug")).toContainText("opening graphics");
    expect(errors).toEqual([]);
  });

  test("?dpr= overrides the cap", async ({ page }) => {
    await ready(page, "/webos/?dpr=2");
    const px = await canvasPixels(page);
    expect(px.width).toBe(px.cssWidth * 2);
  });
});

test.describe("modern build", () => {
  test.use({ deviceScaleFactor: 2 });

  test("uses the display's pixel ratio unless ?dpr= caps it, and shows no diagnostics", async ({ page }) => {
    await ready(page, "");
    let px = await canvasPixels(page);
    expect(px.width).toBe(px.cssWidth * 2);
    await expect(page.locator("#debug")).toHaveCount(0);
    expect(await page.evaluate(() => window.desktank.startup.at(-1))).toContain("first frame drawn");

    await ready(page, "?dpr=1");
    px = await canvasPixels(page);
    expect(px.width).toBe(px.cssWidth);
  });

  test("reports a startup step that stalls", async ({ page }) => {
    // The wasm never arrives.
    await page.route("**/pkg/desktank_web_bg.wasm", () => {});
    await page.goto("");
    await expect(page.locator("#status")).toContainText('stalled at "loading the WebAssembly module"', { timeout: 20_000 });
  });

  test("a page started hidden waits for it to be shown, then draws", async ({ page }) => {
    // As a webOS app launched in the background: hidden until shown.
    await page.addInitScript(() => {
      let hidden = true;
      Object.defineProperty(document, "hidden", { get: () => hidden });
      Object.defineProperty(document, "visibilityState", { get: () => (hidden ? "hidden" : "visible") });
      (window as unknown as { show(): void }).show = () => {
        hidden = false;
        document.dispatchEvent(new Event("visibilitychange"));
      };
    });
    await page.goto("?debug=1");
    await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
    await page.waitForTimeout(500);
    expect(await framesDrawn(page), "nothing is drawn while hidden").toBe(0);
    await expect(page.locator("#debug")).toContainText("hidden: waiting for the page to be shown");
    await page.evaluate(() => (window as unknown as { show(): void }).show());
    await expect.poll(() => framesDrawn(page), { timeout: 15_000 }).toBeGreaterThan(0);
    await expect(page.locator("#debug")).toContainText("first frame drawn");
  });
});
