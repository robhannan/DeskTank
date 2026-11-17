// Shared helpers for the web tests.
import { expect } from "@playwright/test";

// Collects console errors and uncaught exceptions; tests assert it stays empty.
export function watchErrors(page) {
  const errors = [];
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });
  page.on("pageerror", (e) => errors.push(String(e)));
  return errors;
}

export async function open(page, query = "") {
  await page.goto(query);
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
}

// Decodes PNG bytes to RGBA8 in the page (no colour conversion).
export async function decodePng(page, png) {
  const data = await page.evaluate(async (b64) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }), {
      colorSpaceConversion: "none",
      premultiplyAlpha: "none",
    });
    const c = new OffscreenCanvas(bitmap.width, bitmap.height);
    const ctx = c.getContext("2d");
    ctx.drawImage(bitmap, 0, 0);
    const img = ctx.getImageData(0, 0, bitmap.width, bitmap.height);
    return { width: img.width, height: img.height, rgba: Array.from(img.data) };
  }, png.toString("base64"));
  return { ...data, rgba: Uint8Array.from(data.rgba) };
}

export const framesDrawn = (page) => page.evaluate(() => window.desktank.tank.framesDrawn());
