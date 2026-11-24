// Shared helpers for the web tests.
import { expect, type Locator, type Page } from "@playwright/test";
import type { Hooks } from "../../web/hooks.js";

export type WebTank = Hooks["tank"];

// A viewport point in CSS px.
export interface Point {
  x: number;
  y: number;
}

// The element's box in viewport CSS px; fails the test if it isn't visible.
export async function boxOf(locator: Locator) {
  const box = await locator.boundingBox();
  if (!box) throw new Error(`${locator} has no bounding box`);
  return box;
}

// The WebTank methods, by name.
export type TankMethod = { [K in keyof WebTank]: WebTank[K] extends (...args: never[]) => unknown ? K : never }[keyof WebTank];

// Calls WebTank method `f` with `args` in the page and returns its result.
export const callTank = <K extends TankMethod>(page: Page, f: K, ...args: Parameters<WebTank[K]>) =>
  page.evaluate(
    ([f, args]) => {
      const tank = window.desktank.tank;
      return (tank[f] as (...a: unknown[]) => ReturnType<WebTank[K]>).apply(tank, args);
    },
    [f, args] as const,
  );

// Collects console errors and uncaught exceptions; tests assert it stays empty.
export function watchErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });
  page.on("pageerror", (e) => errors.push(String(e)));
  return errors;
}

export async function open(page: Page, query = "") {
  await page.goto(query);
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
}

// Makes the page hidden as a background tab or a minimised window would.
export async function setHidden(page: Page, hidden: boolean) {
  await page.evaluate((hidden) => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => (hidden ? "hidden" : "visible") });
    document.dispatchEvent(new Event("visibilitychange"));
  }, hidden);
}

// Decodes PNG bytes to RGBA8 in the page (no colour conversion).
export async function decodePng(page: Page, png: Buffer) {
  const data = await page.evaluate(async (b64: string) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }), {
      colorSpaceConversion: "none",
      premultiplyAlpha: "none",
    });
    const c = new OffscreenCanvas(bitmap.width, bitmap.height);
    const ctx = c.getContext("2d")!;
    ctx.drawImage(bitmap, 0, 0);
    const img = ctx.getImageData(0, 0, bitmap.width, bitmap.height);
    return { width: img.width, height: img.height, rgba: Array.from(img.data) };
  }, png.toString("base64"));
  return { ...data, rgba: Uint8Array.from(data.rgba) };
}

export const framesDrawn = (page: Page) => page.evaluate(() => window.desktank.tank.framesDrawn());

// Opens the page on WebGL2 and pauses it, so only the tools change cells.
export async function openPaused(page: Page) {
  await open(page, "?backend=webgl2");
  await page.getByRole("button", { name: "Pause" }).click();
  await expect(page.getByRole("button", { name: "Play" })).toBeVisible();
}

// Viewport (CSS px) point at the centre of cell (x, y), after scrolling the
// canvas into view.
export async function cellPoint(page: Page, x: number, y: number): Promise<Point> {
  await page.locator("#tank").scrollIntoViewIfNeeded();
  return page.evaluate(
    ([x, y]) => {
      const canvas = document.querySelector<HTMLCanvasElement>("#tank")!;
      const r = canvas.getBoundingClientRect();
      const [px, py] = window.desktank.tank.pixelAt(x, y);
      return { x: r.left + (px / canvas.width) * r.width, y: r.top + (py / canvas.height) * r.height };
    },
    [x, y],
  );
}

export const materialAt = (page: Page, x: number, y: number) => page.evaluate(([x, y]) => window.desktank.tank.materialAt(x, y), [x, y]);
export const cellCount = (page: Page, name: string) => page.evaluate((n) => window.desktank.tank.materialCount(n), name);
export const stateHash = (page: Page) => page.evaluate(() => window.desktank.tank.stateHash());
export const gridSize = (page: Page) => page.evaluate(() => [window.desktank.tank.width(), window.desktank.tank.height()]);

// Selects the palette item labelled `name`: opens its tab and sub-tab,
// then clicks it.
export async function choose(page: Page, name: string) {
  const path = await page
    .locator("#palette .swatch")
    .evaluateAll((els: HTMLElement[], n) => els.find((b) => b.textContent === n)?.dataset.path, name);
  expect(path, `${name} in the palette`).toBeTruthy();
  await page.locator(`#tabs [data-tab="${path?.split("/")[0]}"]`).click();
  const sub = page.locator(`#subtabs [data-path="${path}"]`);
  if (await sub.isVisible()) await sub.click();
  await page.getByRole("radio", { name, exact: true }).click();
}

// Emulated multi-touch through the Chrome DevTools Protocol (Playwright's
// touchscreen only taps). Points are viewport CSS px; `fingers` lists every
// finger down after the event.
export async function touchscreen(page: Page) {
  const cdp = await page.context().newCDPSession(page);
  const send = (type: "touchStart" | "touchMove" | "touchEnd", fingers: Point[]) =>
    cdp.send("Input.dispatchTouchEvent", {
      type,
      touchPoints: fingers.map((p, id) => ({ x: p.x, y: p.y, id, radiusX: 1, radiusY: 1, force: 1 })),
    });
  return {
    start: (fingers: Point[]) => send("touchStart", fingers),
    move: (fingers: Point[]) => send("touchMove", fingers),
    end: (fingers: Point[] = []) => send("touchEnd", fingers),
    // Moves every finger from `from` to `to` in `steps` touchMoves.
    async drag(from: Point[], to: Point[], steps = 10) {
      await send("touchStart", from);
      for (let i = 1; i <= steps; i++) {
        const t = i / steps;
        await send(
          "touchMove",
          from.map((f, k) => ({ x: f.x + (to[k].x - f.x) * t, y: f.y + (to[k].y - f.y) * t })),
        );
      }
      await send("touchEnd", []);
    },
  };
}
