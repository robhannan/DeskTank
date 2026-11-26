// The species card and the inspect card on a phone: a DPR-3 canvas is about
// 1100 device pixels wide, so card scales of 2 would be 3 CSS pixels a row.
// The page tells the tank its device-pixel ratio as the cards' minimum
// scale, and both cards (and both inspect pages) share it. Set SHOTS=<dir>
// to write screenshots there.
import { expect, test } from "@playwright/test";
import { boxOf, choose, openPaused, cellPoint, gridSize, touchscreen, watchErrors } from "./support.ts";

test.use({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true });

const shots = process.env.SHOTS;

test("cards are drawn at the device-pixel ratio, share one scale and the name row is tappable", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  const tank = () => page.evaluate(() => window.desktank.tank.cardRect());
  const canvasWidth = await page.evaluate(() => document.querySelector("canvas")!.width);
  expect(canvasWidth).toBeGreaterThan(1000);

  // Species card: 144 columns at scale 3 (432 device pixels, under half the
  // canvas).
  await choose(page, "Guppy");
  const species = await tank();
  expect(species.length).toBe(4);
  expect(species[2] % 144).toBe(0);
  const scale = species[2] / 144;
  expect(scale).toBe(3);
  if (shots) await page.screenshot({ path: `${shots}/phone-species-card.png` });

  // Inspect card on a guppy: both pages at the same scale.
  const touch = await touchscreen(page);
  const [w, h] = await gridSize(page);
  const [x, y] = [Math.round(w / 2), Math.round(h / 2)];
  const tap = async (p: { x: number; y: number }) => {
    await touch.start([p]);
    await touch.end();
  };
  await tap(await cellPoint(page, x, y));
  await choose(page, "Inspect");
  await tap(await cellPoint(page, x, y));
  await expect(page.locator("#inspect-sr")).toHaveText(/^Guppy #1 · /);
  const live = await page.evaluate(() => window.desktank.tank.inspectRect());
  expect(live[2] % 144).toBe(0);
  expect(live[2] / 144).toBe(scale);

  // The name input is a finger tall and a tap on it edits, not turns the page.
  const input = await boxOf(page.locator("#name-input"));
  expect(input.height).toBeGreaterThanOrEqual(32 - 0.5);
  await tap({ x: input.x + input.width / 2, y: input.y + input.height / 2 });
  await expect(page.locator("#name-input")).toBeFocused();
  expect(await page.evaluate(() => window.desktank.tank.inspectNameEditing())).toBe(true);
  expect(await page.evaluate(() => window.desktank.tank.inspectPage())).toBe(1);
  await page.keyboard.type("Zip");
  if (shots) await page.screenshot({ path: `${shots}/phone-inspect-naming.png` });
  await page.keyboard.press("Enter");
  expect(await page.evaluate(() => window.desktank.tank.creatureName(1))).toBe("Zip");

  // A tap near the card's bottom-right corner turns it to page two, the
  // species card, at the same scale.
  const canvas = await boxOf(page.locator("#tank"));
  const k = canvas.width / canvasWidth;
  await tap({ x: canvas.x + (live[0] + live[2] - 6) * k, y: canvas.y + (live[1] + live[3] - 6) * k });
  expect(await page.evaluate(() => window.desktank.tank.inspectPage())).toBe(2);
  const two = await page.evaluate(() => window.desktank.tank.inspectRect());
  expect(two[2] / 144).toBe(scale);
  if (shots) await page.screenshot({ path: `${shots}/phone-inspect-species-page.png` });
  expect(errors).toEqual([]);
});
