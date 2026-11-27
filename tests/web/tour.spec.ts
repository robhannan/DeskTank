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

// Sets the tour and draws a frame of the result now, whatever the loop is
// doing: the frame cap drops a call that comes too soon after the last draw,
// so call until one is drawn. The count starts before the change, in one
// synchronous step, so the page's own loop can't draw the frame first and
// leave nothing to wait for. Returns the frames drawn.
const setTourAndDraw = (page: Page, on: boolean, card: boolean, seed: number) =>
  page.evaluate(
    async ([on, card, seed]) => {
      const tank = window.desktank.tank;
      const before = tank.framesDrawn();
      tank.setTour(on, card, seed);
      for (let i = 0; i < 100 && tank.framesDrawn() === before; i++) {
        tank.frame(performance.now());
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      return tank.framesDrawn() - before;
    },
    [on, card, seed] as const,
  );

test("?tour is off unless asked for", async ({ page }) => {
  await open(page, "?backend=webgl2");
  expect(await callTank(page, "touring")).toBe(false);
  await open(page, "?backend=webgl2&tour=0");
  expect(await callTank(page, "touring")).toBe(false);
  await open(page, "?backend=webgl2&tour");
  expect(await callTank(page, "touring")).toBe(true);
});

test("the tour keeps drawing and the first click ends it", async ({ page }) => {
  // Screenshots under software WebGL2 on a loaded runner are slow.
  test.slow();
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2&tour=card");
  expect(await callTank(page, "touring")).toBe(true);
  // No name input while the tour card shows.
  await expect(page.locator("#name-input")).toBeDisabled();

  // Frames keep coming and the pictures differ: the world is busy and the
  // tour moves the camera every frame. Wait on frames drawn, not on time: a
  // busy runner draws few a second.
  const shots: Buffer[] = [];
  for (let i = 0; i < 4; i++) {
    const before = await framesDrawn(page);
    shots.push(await page.locator("#tank").screenshot());
    await expect.poll(() => framesDrawn(page).then((n) => n - before), { timeout: 30_000 }).toBeGreaterThanOrEqual(8);
  }
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

test("a paused tour asks for no frames", async ({ context, page }) => {
  // A paused owner's tank and a viewer's (viewers are paused) both hold the
  // clock, so the tour's view stands still and nothing is worth drawing.
  await open(page, "?backend=webgl2&tour");
  await expect.poll(() => framesDrawn(page), { timeout: 30_000 }).toBeGreaterThan(3);
  await callTank(page, "setRunning", false);
  // Let a frame in flight finish, then count over 3 s.
  await page.waitForTimeout(500);
  const before = await framesDrawn(page);
  await page.waitForTimeout(3000);
  expect(await framesDrawn(page)).toBe(before);

  const viewer = await context.newPage();
  await open(viewer, "?backend=webgl2&tour");
  expect(await viewer.evaluate(() => window.desktank.owner)).toBe(false);
  expect(await callTank(viewer, "touring")).toBe(true);
  await viewer.waitForTimeout(500);
  const viewerBefore = await framesDrawn(viewer);
  await viewer.waitForTimeout(3000);
  expect(await framesDrawn(viewer)).toBe(viewerBefore);
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

// Records, before the page's own handlers can stop them, the input events
// that reach the document: synthetic events from the tests below and real
// ones alike.
async function probeInput(page: Page) {
  await page.evaluate(() => {
    const seen: string[] = [];
    (window as unknown as { seen: string[] }).seen = seen;
    for (const type of ["pointerdown", "pointerup", "pointercancel", "click", "auxclick", "contextmenu", "keydown", "keyup"]) {
      document.addEventListener(
        type,
        (e) => {
          const id = e instanceof KeyboardEvent ? `${e.key}${e.repeat ? "*" : ""}` : e instanceof PointerEvent ? `#${e.pointerId}` : "";
          seen.push(`${type}${id}`);
        },
        true,
      );
    }
  });
  return () => page.evaluate(() => (window as unknown as { seen: string[] }).seen.splice(0));
}

// Dispatches a synthetic event at the canvas that bubbles to the window.
const fire = (page: Page, type: string, init: Record<string, unknown>) =>
  page.evaluate(
    ([type, init]) => {
      const Event_ = type.startsWith("key") ? KeyboardEvent : type.startsWith("pointer") ? PointerEvent : MouseEvent;
      document.getElementById("tank")!.dispatchEvent(new Event_(type, { bubbles: true, cancelable: true, composed: true, ...init }));
    },
    [type, init] as const,
  );

test("a shifted key that wakes the tour is swallowed whole, key up included", async ({ page }) => {
  await open(page, "?backend=webgl2&tour");
  const seen = await probeInput(page);
  // Shift+A pressed, then A released after Shift: the key reads "A" down and
  // "a" up, but it is the same key.
  await fire(page, "keydown", { key: "A", code: "KeyA", shiftKey: true });
  expect(await callTank(page, "touring")).toBe(false);
  await fire(page, "keydown", { key: "A", code: "KeyA", shiftKey: true, repeat: true });
  await fire(page, "keyup", { key: "a", code: "KeyA" });
  expect(await seen()).toEqual([]);
  // The key is the user's again afterwards.
  await fire(page, "keydown", { key: "a", code: "KeyA" });
  await fire(page, "keyup", { key: "a", code: "KeyA" });
  expect(await seen()).toEqual(["keydowna", "keyupa"]);
});

test("a held key that wakes the tour doesn't act on its autorepeats", async ({ page }) => {
  await open(page, "?backend=webgl2&tour");
  const seen = await probeInput(page);
  await page.keyboard.down("Space");
  await page.keyboard.down("Space"); // autorepeat
  await page.keyboard.down("Space");
  await page.keyboard.up("Space");
  expect(await seen()).toEqual([]);
  expect(await callTank(page, "touring")).toBe(false);
  // Space would have paused the world.
  expect(await callTank(page, "running")).toBe(true);
  // Pressed again, it is the user's: it pauses.
  await page.keyboard.press("Space");
  expect(await callTank(page, "running")).toBe(false);
});

test("a pointer that wakes the tour has its release and late click swallowed, and no more", async ({ page }) => {
  await open(page, "?backend=webgl2&tour");
  const seen = await probeInput(page);
  const mouse = { pointerId: 1, pointerType: "mouse", isPrimary: true, clientX: 300, clientY: 300 };
  await fire(page, "pointerdown", mouse);
  expect(await callTank(page, "touring")).toBe(false);
  await fire(page, "pointerup", mouse);
  // A tap that fires no click must not swallow the next press, even within
  // 100 ms of it, and a click arriving late (a mobile tap delay) is dropped.
  await page.waitForTimeout(250);
  await fire(page, "click", mouse);
  expect(await seen()).toEqual([]);

  // A second press by another pointer is the user's own, and so is its click.
  const next = { ...mouse, pointerId: 2 };
  await fire(page, "pointerdown", next);
  await fire(page, "pointerup", next);
  await fire(page, "click", next);
  expect(await seen()).toEqual(["pointerdown#2", "pointerup#2", "click"]);
});

test("a press whose pointerup came without a click doesn't swallow the next tap", async ({ page }) => {
  await open(page, "?backend=webgl2&tour");
  const seen = await probeInput(page);
  const mouse = { pointerId: 1, pointerType: "touch", isPrimary: true, clientX: 300, clientY: 300 };
  await fire(page, "pointerdown", mouse);
  await fire(page, "pointerup", mouse);
  // A new tap straight away (inside 100 ms), whose click must reach the page.
  await fire(page, "pointerdown", { ...mouse, pointerId: 3 });
  await fire(page, "pointerup", { ...mouse, pointerId: 3 });
  await fire(page, "click", { ...mouse, pointerId: 3 });
  expect(await seen()).toEqual(["pointerdown#3", "pointerup#3", "click"]);
});

test("a right-click that wakes the tour doesn't open the menu or fire auxclick", async ({ page }) => {
  await open(page, "?backend=webgl2&tour");
  const seen = await probeInput(page);
  const mouse = { pointerId: 1, pointerType: "mouse", isPrimary: true, clientX: 300, clientY: 300, button: 2 };
  await fire(page, "pointerdown", mouse);
  await fire(page, "contextmenu", mouse);
  await fire(page, "pointerup", mouse);
  await fire(page, "auxclick", mouse);
  expect(await seen()).toEqual([]);
});

test("a tour frame of the whole tank has the same colours as a plain one", async ({ page }) => {
  // An sRGB canvas, where the camera's post pass could encode twice. A tour
  // starts from the view of the whole tank, which the camera draws pixel for
  // pixel as the plain draw does, so the two should match. Hold the world
  // still and restart the tour, so no tick has moved it off that view.
  await open(page, "?backend=webgl2&srgb=1&tour=card");
  await callTank(page, "setRunning", false);
  expect(await setTourAndDraw(page, false, false, 0)).toBeGreaterThanOrEqual(1);
  const plain = await meanColour(page);

  expect(await setTourAndDraw(page, true, true, 5)).toBeGreaterThanOrEqual(1);
  expect(await callTank(page, "touring")).toBe(true);
  const touring = await meanColour(page);

  console.log(`mean colour without the tour ${plain.map((v) => v.toFixed(2))}, with ${touring.map((v) => v.toFixed(2))}`);
  // Encoding twice moves the means by tens of levels; allow one.
  for (let c = 0; c < 3; c++) expect(Math.abs(touring[c] - plain[c])).toBeLessThan(1);
});
