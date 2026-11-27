// The screensaver camera tour on the web (D29, SPEC §3.5): opt-in with
// ?tour=card, it keeps drawing, ends on the first input and swallows it, and
// the camera's post pass shows the same colours as the plain draw.
import { expect, test, type Page } from "@playwright/test";
import { callTank, decodePng, framesDrawn, open, watchErrors } from "./support.ts";

// Mean red, green and blue over a canvas screenshot, 0–255.
async function meanColour(page: Page): Promise<[number, number, number]> {
  const { rgba } = await decodePng(page, await page.locator("#tank").screenshot());
  const sum = [0, 0, 0];
  for (let i = 0; i < rgba.length; i += 4) for (let c = 0; c < 3; c++) sum[c] += rgba[i + c];
  const n = rgba.length / 4;
  return [sum[0] / n, sum[1] / n, sum[2] / n];
}

// Draws a frame now, whatever the loop is doing: the frame cap drops a call
// that comes too soon after the last draw, so call until one is drawn.
const drawNow = (page: Page) =>
  page.evaluate(async () => {
    const tank = window.desktank.tank;
    const before = tank.framesDrawn();
    for (let i = 0; i < 50 && tank.framesDrawn() === before; i++) {
      tank.frame(performance.now());
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    return tank.framesDrawn() - before;
  });

test("?tour is off unless asked for", async ({ page }) => {
  await open(page, "?backend=webgl2");
  expect(await callTank(page, "touring")).toBe(false);
  await open(page, "?backend=webgl2&tour=0");
  expect(await callTank(page, "touring")).toBe(false);
  await open(page, "?backend=webgl2&tour");
  expect(await callTank(page, "touring")).toBe(true);
});

test("the tour keeps drawing and the first click ends it", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2&tour=card");
  expect(await callTank(page, "touring")).toBe(true);
  // No name input while the tour card shows.
  await expect(page.locator("#name-input")).toBeDisabled();

  // Frames keep changing over a few seconds: drawn at a steady rate (the
  // world is busy, and the tour moves the camera every frame), and the
  // pictures differ.
  const before = await framesDrawn(page);
  const shots: Buffer[] = [];
  for (let i = 0; i < 4; i++) {
    shots.push(await page.locator("#tank").screenshot());
    await page.waitForTimeout(1000);
  }
  const drawn = (await framesDrawn(page)) - before;
  // About 30 a second on AC power; SwiftShader on a busy runner gets fewer.
  expect(drawn).toBeGreaterThan(20);
  for (let i = 1; i < shots.length; i++) expect(shots[i].equals(shots[i - 1])).toBe(false);

  // Hold the world still so only a click can change a cell, then click the
  // tank: the first click ends the tour and pours nothing, the second pours.
  await callTank(page, "setRunning", false);
  const hash = await callTank(page, "stateHash");
  const box = (await page.locator("#tank").boundingBox())!;
  const [x, y] = [box.x + box.width / 2, box.y + box.height / 3];
  await page.mouse.click(x, y);
  expect(await callTank(page, "touring")).toBe(false);
  expect(await callTank(page, "stateHash")).toBe(hash);
  await page.mouse.click(x, y);
  expect(await callTank(page, "stateHash")).not.toBe(hash);
  await expect(page.locator("#name-input")).toBeDisabled();
  expect(errors).toEqual([]);
});

test("a key or the wheel also ends the tour and does nothing else", async ({ page }) => {
  await open(page, "?backend=webgl2&tour");
  const radius = await callTank(page, "radius");
  await page.mouse.move(300, 300);
  await page.mouse.wheel(0, -400);
  expect(await callTank(page, "touring")).toBe(false);
  expect(await callTank(page, "radius")).toBe(radius);

  await open(page, "?backend=webgl2&tour");
  await page.keyboard.press("Space");
  expect(await callTank(page, "touring")).toBe(false);
  // Space would have paused the world.
  expect(await callTank(page, "running")).toBe(true);
});

test("a tour frame of the whole tank has the same colours as a plain one", async ({ page }) => {
  // An sRGB canvas, where the camera's post pass could encode twice. A tour
  // starts from the view of the whole tank, which the camera draws pixel for
  // pixel as the plain draw does, so the two should match. Hold the world
  // still and restart the tour, so no tick has moved it off that view.
  await open(page, "?backend=webgl2&srgb=1&tour=card");
  await callTank(page, "setRunning", false);
  await callTank(page, "setTour", false, false, 0);
  expect(await drawNow(page)).toBe(1);
  const plain = await meanColour(page);

  await callTank(page, "setTour", true, true, 5);
  expect(await callTank(page, "touring")).toBe(true);
  expect(await drawNow(page)).toBe(1);
  const touring = await meanColour(page);

  console.log(`mean colour without the tour ${plain.map((v) => v.toFixed(2))}, with ${touring.map((v) => v.toFixed(2))}`);
  // Encoding twice moves the means by tens of levels; allow one.
  for (let c = 0; c < 3; c++) expect(Math.abs(touring[c] - plain[c])).toBeLessThan(1);
});
