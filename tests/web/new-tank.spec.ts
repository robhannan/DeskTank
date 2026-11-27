// The New tank setup (#475): a paused live preview of the draft on the main
// canvas, with the centimetre ruler on; Cancel and Escape put the real tank
// back untouched, and Create replaces it and applies the initial settings.
import { readFile } from "node:fs/promises";
import { expect, type Page, test } from "@playwright/test";
import { cellCount, framesDrawn, openPaused, stateHash, watchErrors } from "./support.ts";

const panel = (page: Page) => page.locator("#new-tank-panel");
const open = async (page: Page) => {
  await page.getByRole("button", { name: "New tank" }).click();
  await expect(panel(page)).toBeVisible();
};
const grid = (page: Page) => page.evaluate(() => window.desktank.grid);
const ruler = (page: Page) => page.evaluate(() => window.desktank.tank.ruler());
const settings = (page: Page) => page.evaluate(() => window.desktank.settings);

// "160 × 90 cm (320 × 180 cells)" as numbers.
async function dimensions(page: Page) {
  const text = (await page.locator("#nt-dimensions").textContent()) ?? "";
  const m = /^([\d.]+) × ([\d.]+) cm \((\d+) × (\d+) cells\)$/.exec(text);
  expect(m, `dimensions readout "${text}"`).not.toBeNull();
  const [w, h, cols, rows] = m!.slice(1).map(Number);
  return { w, h, cols, rows };
}

test("opening New tank previews the tank with the ruler and its size in cm", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  const before = await stateHash(page);
  const writes = await page.evaluate(() => window.desktank.writes);
  expect(await ruler(page)).toBe(false);

  await open(page);
  await expect(page.locator("#settings")).toBeHidden();
  await expect(page.locator(".tools")).toBeHidden();
  await expect(page.getByRole("button", { name: "New tank" })).toHaveAttribute("aria-expanded", "true");
  await expect(page.locator("#nt-warning")).toContainText("replaces your current saved tank");
  expect(await ruler(page), "the preview shows the ruler").toBe(true);
  // The preview is a different tank on the same canvas, and it is drawn.
  await expect.poll(() => framesDrawn(page)).toBeGreaterThan(0);
  expect(await page.evaluate(() => window.desktank.tank.running()), "the preview is paused").toBe(false);
  expect(await page.evaluate(() => document.querySelectorAll("canvas").length), "one canvas, one GPU context").toBe(1);

  // Two cells to the centimetre.
  const d = await dimensions(page);
  const [, cols, rows] = await grid(page);
  expect({ cols: d.cols, rows: d.rows }).toEqual({ cols, rows });
  expect(d.w).toBeCloseTo(cols / 2, 1);
  expect(d.h).toBeCloseTo(rows / 2, 1);
  expect(await page.evaluate(() => window.desktank.writes), "nothing is saved while previewing").toBe(writes);

  await page.getByRole("button", { name: "Cancel" }).click();
  expect(await stateHash(page)).toBe(before);
  expect(errors).toEqual([]);
});

test("the cell scale changes the size and grid live", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  await open(page);
  await page.locator("#nt-scale").selectOption("8");
  await expect.poll(async () => (await grid(page))[0]).toBe(8);
  const coarse = await dimensions(page);
  const [, cols8, rows8] = await grid(page);
  expect({ cols: coarse.cols, rows: coarse.rows }).toEqual({ cols: cols8, rows: rows8 });

  await page.locator("#nt-scale").selectOption("2");
  await expect.poll(async () => (await grid(page))[0]).toBe(2);
  const fine = await dimensions(page);
  expect(fine.cols).toBeGreaterThan(coarse.cols);
  expect(fine.w).toBeGreaterThan(coarse.w);
  expect(fine.w).toBeCloseTo(fine.cols / 2, 1);
  // The readout and the grid agree with the canvas after the change.
  const [, cols, rows] = await grid(page);
  expect({ cols: fine.cols, rows: fine.rows }).toEqual({ cols, rows });
  expect(errors).toEqual([]);
});

test("presets, starter devices and Reroll update the preview", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  await open(page);
  const empty = await stateHash(page);
  await expect(page.locator("#nt-devices")).toBeDisabled();
  await expect(page.locator("#nt-preset-desc")).toContainText("clean water");

  await page.locator("#nt-preset").selectOption("artificial");
  await expect(page.locator("#nt-preset-desc")).toContainText("gravel");
  await expect(page.locator("#nt-devices")).toBeEnabled();
  const withDevices = await stateHash(page);
  expect(withDevices).not.toBe(empty);
  expect(await ruler(page), "a rebuilt preview keeps the ruler").toBe(true);

  await page.locator("#nt-devices").uncheck();
  const without = await stateHash(page);
  expect(without).not.toBe(withDevices);
  await page.locator("#nt-devices").check();
  expect(await stateHash(page), "the same draft is the same tank").toBe(withDevices);

  const seed = await page.locator("#nt-seed").inputValue();
  await page.getByRole("button", { name: "Reroll" }).click();
  expect(await page.locator("#nt-seed").inputValue()).not.toBe(seed);
  expect(await stateHash(page)).not.toBe(withDevices);

  // Each change draws one frame and no more: the paused preview sits idle.
  await expect.poll(() => framesDrawn(page)).toBeGreaterThan(0);
  const drawn = await framesDrawn(page);
  await page.waitForTimeout(500);
  expect(await framesDrawn(page)).toBe(drawn);
  expect(errors).toEqual([]);
});

test("the water level changes the preview's water", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  await open(page);
  const full = await cellCount(page, "water");
  await page.locator("#nt-water-level").fill("60");
  await expect(page.locator("#nt-water-level-out")).toHaveText("60%");
  await expect.poll(() => cellCount(page, "water")).toBeLessThan(full);
  expect(errors).toEqual([]);
});

test("Cancel and Escape put the real tank back untouched", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  const before = await stateHash(page);
  const gridBefore = await grid(page);
  const writes = await page.evaluate(() => window.desktank.writes);
  const saved = await settings(page);

  for (const leave of [
    () => page.getByRole("button", { name: "Cancel" }).click(),
    () => page.keyboard.press("Escape"),
    // The New tank button toggles it.
    () => page.getByRole("button", { name: "New tank" }).click(),
  ]) {
    await open(page);
    await page.locator("#nt-preset").selectOption("artificial");
    await page.locator("#nt-scale").selectOption("8");
    await page.locator("#nt-oled").check();
    await page.locator("#nt-time-ratio").fill("10");
    expect(await stateHash(page)).not.toBe(before);
    await leave();
    await expect(panel(page)).toBeHidden();
    await expect(page.locator(".tools")).toBeVisible();
    await expect(page.getByRole("button", { name: "New tank" })).toHaveAttribute("aria-expanded", "false");
    expect(await stateHash(page), "the same tank").toBe(before);
    expect(await grid(page)).toEqual(gridBefore);
    expect(await ruler(page), "the ruler is back to the user's choice").toBe(false);
    expect(await page.evaluate(() => window.desktank.tank.oled())).toBe(false);
    expect(await settings(page), "no setting changed").toEqual(saved);
    expect(await page.evaluate(() => window.desktank.writes), "no save was written").toBe(writes);
    await expect.poll(() => framesDrawn(page)).toBeGreaterThan(0);
  }
  // Still paused, as it was.
  await expect(page.getByRole("button", { name: "Play" })).toBeVisible();
  expect(errors).toEqual([]);
});

test("Cancel resumes a running tank", async ({ page }) => {
  const errors = watchErrors(page);
  await page.goto("?backend=webgl2");
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
  await open(page);
  await page.getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByRole("button", { name: "Pause" })).toBeVisible();
  const ticks = () => page.evaluate(() => window.desktank.tank.tickCount());
  const t = await ticks();
  await expect.poll(ticks).toBeGreaterThan(t);
  expect(errors).toEqual([]);
});

test("the canvas is not editable and the shortcuts wait while it is open", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  await open(page);
  await page.locator("#nt-preset").selectOption("artificial");
  const preview = await stateHash(page);
  const box = (await page.locator("#tank").boundingBox())!;
  await page.mouse.move(box.x + box.width * 0.5, box.y + box.height * 0.2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * 0.7, box.y + box.height * 0.2, { steps: 6 });
  await page.mouse.up();
  await page.locator("body").press("u");
  await page.locator("body").press(" ");
  expect(await stateHash(page), "pouring does nothing").toBe(preview);
  expect(await ruler(page), "U doesn't toggle the ruler").toBe(true);
  expect(await page.evaluate(() => window.desktank.tank.running()), "Space doesn't run the preview").toBe(false);
  expect(await settings(page)).toMatchObject({ ruler: false });
  expect(errors).toEqual([]);
});

test("Export current tank downloads the real tank, not the preview", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  const bytes = await page.evaluate(() => Array.from(window.desktank.tank.exportSave()));
  await open(page);
  await page.locator("#nt-preset").selectOption("artificial");
  const [file] = await Promise.all([page.waitForEvent("download"), page.getByRole("button", { name: "Export current tank" }).click()]);
  expect(file.suggestedFilename()).toMatch(/^desktank-\d{8}-\d{4}\.dtank$/);
  const got = await readFile(await file.path());
  // The real tank's save, byte for byte, not the preview's.
  expect(Array.from(got)).toEqual(bytes);
  expect(errors).toEqual([]);
});

test("Create tank replaces the tank, applies the initial settings and persists", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  const before = await stateHash(page);
  const fullWater = await cellCount(page, "water");
  // The user's own ruler choice is on, so it must survive the preview.
  await page.getByRole("button", { name: "Settings" }).click();
  await page.locator("#set-ruler").check();
  await page.getByRole("button", { name: "Settings" }).click();

  await open(page);
  await page.locator("#nt-preset").selectOption("artificial");
  await page.locator("#nt-seed").fill("777");
  await page.locator("#nt-water-level").fill("70");
  await page.locator("#nt-easy-mode").check();
  await page.locator("#nt-time-ratio").fill("5");
  await page.locator("#nt-catch-up").check();
  await page.locator("#nt-creature-cap").fill("12");
  await page.locator("#nt-oled").check();
  await page.getByRole("button", { name: "Create tank" }).click();
  await expect(panel(page)).toBeHidden();
  await expect(page.locator(".tools")).toBeVisible();

  const tank = () =>
    page.evaluate(() => {
      const t = window.desktank.tank;
      return { easy: t.easyMode(), ratio: t.timeRatio(), cap: t.creatureCap(), oled: t.oled(), ruler: t.ruler(), running: t.running() };
    });
  expect(await tank()).toEqual({ easy: true, ratio: 5, cap: 12, oled: true, ruler: true, running: true });
  expect(await settings(page)).toMatchObject({ seed: 777, easyMode: true, timeRatio: 5, catchUp: true, oled: true, ruler: true });
  expect(await cellCount(page, "water")).toBeLessThan(fullWater);
  await expect(page.getByRole("button", { name: "Pause" })).toBeVisible();
  await expect(page.locator("#set-creature-cap")).toHaveValue("12");
  expect(await stateHash(page)).not.toBe(before);

  // Saved: a reload opens the new tank with its settings.
  await page.evaluate(() => window.desktank.saving);
  await page.reload();
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
  expect(await settings(page)).toMatchObject({ seed: 777, easyMode: true, timeRatio: 5, catchUp: true, oled: true, ruler: true });
  expect(await tank()).toMatchObject({ easy: true, ratio: 5, cap: 12, oled: true, ruler: true });
  expect(await cellCount(page, "water")).toBeLessThan(fullWater);
  expect(errors).toEqual([]);
});

test("Create tank restores the ruler to off when the user had it off", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  await open(page);
  expect(await ruler(page)).toBe(true);
  await page.getByRole("button", { name: "Create tank" }).click();
  await expect(panel(page)).toBeHidden();
  expect(await ruler(page)).toBe(false);
  expect(await settings(page)).toMatchObject({ ruler: false, timeRatio: 1 });
  expect(errors).toEqual([]);
});

test("settings that don't shape the tank apply to the preview without rebuilding it", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  await open(page);
  type Marked = Window & { preview?: unknown };
  await page.evaluate(() => ((window as Marked).preview = window.desktank.tank));
  const same = () => page.evaluate(() => window.desktank.tank === (window as Marked).preview);
  await page.locator("#nt-oled").check();
  await page.locator("#nt-easy-mode").check();
  await page.locator("#nt-time-ratio").fill("3");
  await page.locator("#nt-creature-cap").fill("9");
  await expect
    .poll(() => page.evaluate(() => {
      const t = window.desktank.tank;
      return { oled: t.oled(), easy: t.easyMode(), ratio: t.timeRatio(), cap: t.creatureCap() };
    }))
    .toEqual({ oled: true, easy: true, ratio: 3, cap: 9 });
  expect(await same(), "the same preview tank").toBe(true);
  // A layout field does rebuild it.
  await page.locator("#nt-seed").fill("4242");
  await expect.poll(same).toBe(false);
  expect(errors).toEqual([]);
});

test("the setup fits a phone-width viewport", async ({ page }) => {
  const errors = watchErrors(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await openPaused(page);
  await open(page);
  for (const name of ["Create tank", "Cancel", "Export current tank", "Reroll"]) {
    const box = (await page.getByRole("button", { name }).boundingBox())!;
    expect(box.x, `${name} starts on screen`).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width, `${name} ends on screen`).toBeLessThanOrEqual(390);
  }
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), "no sideways scroll").toBe(true);
  expect(errors).toEqual([]);
});
