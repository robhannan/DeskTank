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

// Opens the page on WebGL2 and pauses it, so only the tools change cells.
export async function openPaused(page) {
  await open(page, "?backend=webgl2");
  await page.getByRole("button", { name: "Pause" }).click();
  await expect(page.getByRole("button", { name: "Play" })).toBeVisible();
}

// Viewport (CSS px) point at the centre of cell (x, y), after scrolling the
// canvas into view.
export async function cellPoint(page, x, y) {
  await page.locator("#tank").scrollIntoViewIfNeeded();
  return page.evaluate(
    ([x, y]) => {
      const canvas = document.querySelector("#tank");
      const r = canvas.getBoundingClientRect();
      const [px, py] = window.desktank.tank.pixelAt(x, y);
      return { x: r.left + (px / canvas.width) * r.width, y: r.top + (py / canvas.height) * r.height };
    },
    [x, y],
  );
}

export const materialAt = (page, x, y) => page.evaluate(([x, y]) => window.desktank.tank.materialAt(x, y), [x, y]);
export const cellCount = (page, name) => page.evaluate((n) => window.desktank.tank.materialCount(n), name);
export const stateHash = (page) => page.evaluate(() => window.desktank.tank.stateHash());
export const gridSize = (page) => page.evaluate(() => [window.desktank.tank.width(), window.desktank.tank.height()]);

// Selects the palette item labelled `name`: opens its group's tab, then
// clicks it.
export async function choose(page, name) {
  const group = await page
    .locator("#palette .swatch")
    .evaluateAll((els, n) => els.find((b) => b.textContent === n)?.dataset.group, name);
  expect(group, `${name} in the palette`).toBeTruthy();
  await page.locator(`#groups [data-group="${group}"]`).click();
  await page.getByRole("radio", { name, exact: true }).click();
}

// Emulated multi-touch through the Chrome DevTools Protocol (Playwright's
// touchscreen only taps). Points are viewport CSS px; `fingers` lists every
// finger down after the event.
export async function touchscreen(page) {
  const cdp = await page.context().newCDPSession(page);
  const send = (type, fingers) =>
    cdp.send("Input.dispatchTouchEvent", {
      type,
      touchPoints: fingers.map((p, id) => ({ x: p.x, y: p.y, id, radiusX: 1, radiusY: 1, force: 1 })),
    });
  return {
    start: (fingers) => send("touchStart", fingers),
    move: (fingers) => send("touchMove", fingers),
    end: (fingers = []) => send("touchEnd", fingers),
    // Moves every finger from `from` to `to` in `steps` touchMoves.
    async drag(from, to, steps = 10) {
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
