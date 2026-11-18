// WB2 (SPEC §9): the interact tools with mouse and emulated touch, the
// palette (the sim's list, keyboard navigation), the menu, the settings
// panel and the canvas's gesture suppression. The tank is paused so only the
// tools change cells.
import { expect, test } from "@playwright/test";
import {
  cellCount,
  cellPoint,
  gridSize,
  materialAt,
  open,
  openPaused,
  stateHash,
  touchscreen,
  watchErrors,
} from "./support.mjs";

const choose = (page, name) => page.getByRole("radio", { name, exact: true }).click();
const glued = (page) => page.evaluate(() => window.desktank.tank.gluedBodies());

test.describe("mouse", () => {
  test("pour, drag, scoop and the scoop tool change the cells", async ({ page }) => {
    const errors = watchErrors(page);
    await openPaused(page);
    const [w, h] = await gridSize(page);
    const y = Math.round(h * 0.6);

    // A click pours a disc of sand.
    await choose(page, "Sand");
    let p = await cellPoint(page, Math.round(w * 0.3), y);
    expect(await materialAt(page, Math.round(w * 0.3), y)).toBe("water");
    await page.mouse.click(p.x, p.y);
    expect(await materialAt(page, Math.round(w * 0.3), y)).toBe("sand");

    // A fast drag (two samples) pours an unbroken line of gravel.
    await choose(page, "Gravel");
    const [x0, x1] = [Math.round(w * 0.4), Math.round(w * 0.8)];
    const a = await cellPoint(page, x0, y);
    const b = await cellPoint(page, x1, y);
    await page.mouse.move(a.x, a.y);
    await page.mouse.down();
    await page.mouse.move(b.x, b.y, { steps: 1 });
    await page.mouse.up();
    for (let x = x0; x <= x1; x += 3) expect(await materialAt(page, x, y), `column ${x}`).toBe("gravel");

    // Right-drag scoops whatever is selected; so does the scoop tool.
    p = await cellPoint(page, Math.round(w * 0.3), y);
    await page.mouse.click(p.x, p.y, { button: "right" });
    expect(await materialAt(page, Math.round(w * 0.3), y)).toBe("water");
    await choose(page, "Scoop");
    p = await cellPoint(page, Math.round(w * 0.6), y);
    await page.mouse.click(p.x, p.y);
    expect(await materialAt(page, Math.round(w * 0.6), y)).toBe("water");
    expect(errors).toEqual([]);
  });

  test("glue, devices, inspect and net", async ({ page }) => {
    const errors = watchErrors(page);
    await openPaused(page);
    const [w, h] = await gridSize(page);
    const [x, y] = [Math.round(w / 2), Math.round(h / 2)];

    // A rock stroke, a tick (the body table updates on ticks), then glue.
    await choose(page, "Rock");
    let p = await cellPoint(page, x, y);
    await page.mouse.click(p.x, p.y);
    await page.getByRole("button", { name: "Step" }).click();
    expect(await glued(page)).toBe(0);
    await choose(page, "Glue");
    p = await cellPoint(page, x, y);
    await page.mouse.click(p.x, p.y);
    expect(await glued(page), "the rock is glued").toBe(1);

    // A device goes down once per click.
    await choose(page, "Bubbler");
    p = await cellPoint(page, Math.round(w * 0.2), y);
    await page.mouse.click(p.x, p.y);
    expect(await cellCount(page, "bubbler")).toBe(8);

    // Inspect shows the cell; inspect and net change nothing.
    const hash = await stateHash(page);
    await choose(page, "Inspect");
    p = await cellPoint(page, Math.round(w * 0.8), y);
    await page.mouse.click(p.x, p.y);
    await expect(page.locator("#inspect")).toHaveText(/^Water · cell \d+, \d+$/);
    await choose(page, "Net");
    await page.mouse.move(p.x, p.y);
    await page.mouse.down();
    await page.mouse.move(p.x - 50, p.y, { steps: 5 });
    await page.mouse.up();
    expect(await stateHash(page)).toBe(hash);
    expect(errors).toEqual([]);
  });

  test("creatures: add, inspect, net and lift out", async ({ page }) => {
    const errors = watchErrors(page);
    await openPaused(page);
    const [w, h] = await gridSize(page);
    const [x, y] = [Math.round(w / 2), Math.round(h / 2)];
    const creatures = () => page.evaluate(() => window.desktank.tank.creatures());

    // A palette slot adds one per click, in open water right where clicked.
    await choose(page, "Neon tetra");
    let p = await cellPoint(page, x, y);
    await page.mouse.click(p.x, p.y);
    expect(await creatures()).toBe(1);

    // Inspect names it and the cell under it; that changes nothing.
    const hash = await stateHash(page);
    await choose(page, "Inspect");
    p = await cellPoint(page, x, y);
    await page.mouse.click(p.x, p.y);
    await expect(page.locator("#inspect")).toHaveText(/^Neon tetra #1 · over Water · cell \d+, \d+$/);
    expect(await stateHash(page)).toBe(hash);

    // The net carries it up out of the water: it's gone.
    await choose(page, "Net");
    p = await cellPoint(page, x, y);
    await page.mouse.move(p.x, p.y);
    await page.mouse.down();
    const top = await cellPoint(page, x, 0);
    await page.mouse.move(top.x, top.y, { steps: 8 });
    await page.mouse.up();
    expect(await creatures()).toBe(0);
    expect(errors).toEqual([]);
  });

  test("the wheel sizes the brush and doesn't scroll or zoom the page", async ({ page }) => {
    await open(page, "?backend=webgl2");
    const canvas = page.locator("#tank");
    const box = await canvas.boundingBox();
    expect(await page.evaluate(() => document.documentElement.scrollHeight > innerHeight), "the page can scroll").toBe(true);
    const scroll = await page.evaluate(() => scrollY);
    const radius = () => page.evaluate(() => window.desktank.tank.radius());
    expect(await radius()).toBe(3);
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.wheel(0, -200);
    await expect.poll(radius).toBe(8);
    await expect(page.locator("#brush-out")).toHaveText("8");
    expect(await page.evaluate(() => scrollY), "no page scroll").toBe(scroll);
    // A trackpad pinch arrives as ctrl+wheel.
    await page.keyboard.down("Control");
    await page.mouse.wheel(0, 120);
    await page.keyboard.up("Control");
    await expect.poll(radius).toBe(5);
    expect(await page.evaluate(() => visualViewport.scale)).toBe(1);
    expect(await page.evaluate(() => scrollY)).toBe(scroll);
  });

  test("feed sprinkles flake food", async ({ page }) => {
    await openPaused(page);
    expect(await cellCount(page, "flake_food")).toBe(0);
    await page.getByRole("button", { name: "Feed" }).click();
    expect(await cellCount(page, "flake_food")).toBe(12);
  });
});

test.describe("touch", () => {
  test.use({ viewport: { width: 412, height: 860 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true });

  test("tap and drag pour, two fingers scoop, long-press inspects", async ({ page }) => {
    const errors = watchErrors(page);
    await openPaused(page);
    const touch = await touchscreen(page);
    const [w, h] = await gridSize(page);
    const y = Math.round(h * 0.6);

    // A tap pours once where it lands.
    await choose(page, "Sand");
    const target = Math.round(w * 0.3);
    let p = await cellPoint(page, target, y);
    await touch.start([p]);
    await touch.end();
    expect(await materialAt(page, target, y)).toBe("sand");

    // A one-finger drag pours along its path.
    await choose(page, "Gravel");
    const [x0, x1] = [Math.round(w * 0.5), Math.round(w * 0.9)];
    await touch.drag([await cellPoint(page, x0, y)], [await cellPoint(page, x1, y)]);
    for (let x = x0; x <= x1; x += 3) expect(await materialAt(page, x, y), `column ${x}`).toBe("gravel");

    // Two fingers either side of the sand, dragged down across it, scoop
    // along their midpoint (and pour nothing).
    const gravel = await cellCount(page, "gravel");
    const top = await cellPoint(page, target, y - 8);
    const bottom = await cellPoint(page, target, y + 8);
    const spread = 30;
    await touch.drag(
      [{ x: top.x - spread, y: top.y }, { x: top.x + spread, y: top.y }],
      [{ x: bottom.x - spread, y: bottom.y }, { x: bottom.x + spread, y: bottom.y }],
    );
    expect(await materialAt(page, target, y), "scooped").toBe("water");
    expect(await cellCount(page, "gravel"), "two fingers don't pour").toBe(gravel);

    // A long-press inspects without pouring.
    p = await cellPoint(page, Math.round(w * 0.7), Math.round(h * 0.3));
    const before = await cellCount(page, "gravel");
    await touch.start([p]);
    await page.waitForTimeout(1200); // well past the 500 ms long-press
    await touch.end();
    await expect(page.locator("#inspect")).toHaveText(/^Water · cell \d+, \d+$/);
    expect(await cellCount(page, "gravel")).toBe(before);
    expect(errors).toEqual([]);
  });

  test("glue and devices by tap; net and inspect change nothing", async ({ page }) => {
    const errors = watchErrors(page);
    await openPaused(page);
    const touch = await touchscreen(page);
    const [w, h] = await gridSize(page);
    const [x, y] = [Math.round(w / 2), Math.round(h / 2)];
    const tap = async (cx, cy) => {
      const p = await cellPoint(page, cx, cy);
      await touch.start([p]);
      await touch.end();
    };

    await choose(page, "Rock");
    await tap(x, y);
    await page.getByRole("button", { name: "Step" }).click();
    await choose(page, "Glue");
    await tap(x, y);
    expect(await glued(page)).toBe(1);

    await choose(page, "Filter outlet");
    await tap(Math.round(w * 0.2), y);
    expect(await cellCount(page, "filter_outlet")).toBe(9);

    const hash = await stateHash(page);
    await choose(page, "Inspect");
    await tap(Math.round(w * 0.8), Math.round(h * 0.3));
    await expect(page.locator("#inspect")).toHaveText(/^Water · cell/);
    await choose(page, "Net");
    await touch.drag([await cellPoint(page, x, y - 20)], [await cellPoint(page, x + 30, y - 20)]);
    expect(await stateHash(page)).toBe(hash);
    expect(errors).toEqual([]);
  });

  test("pinch and drag on the canvas don't zoom or scroll the page", async ({ page }) => {
    await openPaused(page);
    const touch = await touchscreen(page);
    // Keep part of the page below the canvas, so a scroll would show.
    await page.evaluate(() => scrollTo(0, 0));
    const box = await page.locator("#tank").boundingBox();
    const [cx, cy] = [box.x + box.width / 2, box.y + box.height / 2];
    const scroll = () => page.evaluate(() => [scrollX, scrollY, visualViewport.scale]);
    const before = await scroll();

    // Pinch out.
    await touch.drag(
      [{ x: cx - 20, y: cy }, { x: cx + 20, y: cy }],
      [{ x: cx - 150, y: cy }, { x: cx + 150, y: cy }],
    );
    // One-finger swipe up (would scroll the page down).
    await touch.drag([{ x: cx, y: box.y + box.height - 10 }], [{ x: cx, y: box.y + 10 }]);
    await page.waitForTimeout(300);
    expect(await scroll()).toEqual(before);

    // Control: the same swipe on the text below scrolls the page.
    const about = await page.locator(".about").boundingBox();
    const ax = about.x + about.width / 2;
    const ay = Math.min(about.y + 40, 840);
    await touch.drag([{ x: ax, y: ay }], [{ x: ax, y: ay - 300 }]);
    await expect.poll(async () => (await scroll())[1]).toBeGreaterThan(before[1]);
  });

  test("the palette is one row that scrolls sideways", async ({ page }) => {
    await open(page, "?backend=webgl2");
    const strip = await page.locator("#palette").evaluate((el) => ({
      height: el.clientHeight,
      scrolls: el.scrollWidth > el.clientWidth,
      button: el.firstElementChild.offsetHeight,
    }));
    expect(strip.scrolls).toBe(true);
    expect(strip.height).toBeLessThan(strip.button * 2);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no page overflow").toBe(true);
  });
});

test("the palette lists the sim's tools and navigates with the keyboard", async ({ page }) => {
  await open(page, "?backend=webgl2");
  const radios = page.getByRole("radio");
  const entries = await page.evaluate(() => window.desktank.tank.palette());
  await expect(radios).toHaveCount(entries.length);
  const names = await radios.evaluateAll((els) => els.map((b) => `${b.dataset.kind} ${b.dataset.name}`));
  expect(names).toEqual(entries);
  const materials = await page.evaluate(() => window.desktank.tank.materials());
  const species = await page.evaluate(() => window.desktank.tank.species());
  for (const e of entries) {
    const [kind, name] = e.split(" ");
    if (kind === "creature") expect(species, e).toContain(name);
    else if (kind !== "tool") expect(materials, e).toContain(name);
  }
  expect(entries.filter((e) => e.startsWith("creature ")).length).toBe(species.length);

  // One radio is in the tab order; arrows move the selection.
  await expect(page.locator('#palette [tabindex="0"]')).toHaveCount(1);
  await radios.first().focus();
  await page.keyboard.press("ArrowRight");
  await expect(radios.nth(1)).toBeFocused();
  await expect(radios.nth(1)).toHaveAttribute("aria-checked", "true");
  expect(await page.evaluate(() => window.desktank.tank.selected())).toBe(1);
  await page.keyboard.press("End");
  await expect(radios.last()).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("ArrowRight");
  await expect(radios.first()).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("ArrowLeft");
  await expect(radios.last()).toBeFocused();

  // Digits pick slots anywhere on the page (1 is the first, 0 the tenth).
  await page.locator("h1").click();
  await page.keyboard.press("3");
  await expect(radios.nth(2)).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("0");
  await expect(radios.nth(9)).toHaveAttribute("aria-checked", "true");
});

test("settings persist across reloads", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  const autoScale = await page.evaluate(() => window.desktank.grid[0]);
  const scale = autoScale === 3 ? 2 : 3;
  await page.getByRole("button", { name: "Settings" }).click();
  await page.locator("#set-scale").selectOption(String(scale));
  await expect.poll(() => page.evaluate(() => window.desktank.grid[0])).toBe(scale);
  await page.locator("#set-fps").selectOption("15");
  await page.locator("#set-field").selectOption("co2");
  await page.getByLabel("Fish avoid the pointer").check();
  await page.locator("#set-stats").uncheck();
  await page.locator("#seed").fill("42");
  await page.locator("#seed").dispatchEvent("change");
  await page.locator("#brush").fill("7");
  await expect(page.locator("#stats")).toBeHidden();
  expect(await page.evaluate(() => window.desktank.tank.fieldOverlay())).toBe("co2");

  await page.reload();
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
  expect(await page.evaluate(() => window.desktank.settings)).toEqual({
    scale,
    fps: "15",
    field: "co2",
    stats: false,
    avoidCursor: true,
    seed: 42,
    radius: 7,
  });
  expect(await page.evaluate(() => window.desktank.grid[0]), "the tank uses the saved scale").toBe(scale);
  expect(await page.evaluate(() => window.desktank.tank.fieldOverlay())).toBe("co2");
  expect(await page.evaluate(() => window.desktank.tank.radius())).toBe(7);
  await expect(page.locator("#stats")).toBeHidden();
  await page.getByRole("button", { name: "Settings" }).click();
  await expect(page.locator("#set-scale")).toHaveValue(String(scale));
  await expect(page.locator("#seed")).toHaveValue("42");

  // Defaults restore auto scale and clear the overlay.
  await page.getByRole("button", { name: "Defaults" }).click();
  await expect.poll(() => page.evaluate(() => window.desktank.grid[0])).toBe(autoScale);
  expect(await page.evaluate(() => window.desktank.tank.fieldOverlay())).toBe("");
  // Garbage in storage falls back to the defaults.
  await page.evaluate(() => localStorage.setItem("desktank.settings.v1", "{not json"));
  await page.reload();
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
  expect(await page.evaluate(() => window.desktank.settings.scale)).toBe(0);
  expect(errors).toEqual([]);
});
