// Web smoke test (SPEC §7): the built site loads, pours, and draws, once on
// WebGL2 and once with the default backend (WebGPU, falling back to WebGL2
// where the browser has no adapter), with no console errors.
import { expect, test } from "@playwright/test";
import { boxOf, callTank, choose, framesDrawn, open, type TankMethod, watchErrors, type WebTank } from "./support.ts";

declare global {
  interface Window {
    loseExt: WEBGL_lose_context; // the context-loss tests' handle
  }
}

for (const [name, query] of [
  ["webgl2", "?backend=webgl2"],
  ["default backend", ""],
]) {
  test(`pours and draws (${name})`, async ({ page }) => {
    const errors = watchErrors(page);
    await open(page, query);
    const backend = await page.evaluate(() => window.desktank.backend);
    if (query) expect(backend).toBe("webgl2");
    else expect(["webgpu", "webgl2"]).toContain(backend);
    test.info().annotations.push({ type: "backend", description: backend });

    const canvas = page.locator("#tank");
    const hash = () => page.evaluate(() => window.desktank.tank.stateHash());
    const before = await hash();
    const pixelsBefore = await canvas.screenshot();
    const drawn = await framesDrawn(page);

    const box = await boxOf(canvas);
    await page.mouse.move(box.x + box.width * 0.3, box.y + box.height * 0.2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.7, box.y + box.height * 0.2, { steps: 10 });
    await page.waitForTimeout(500);
    await page.mouse.up();
    await page.waitForTimeout(500);

    expect(await hash(), "pouring changes cells").not.toBe(before);
    expect(await framesDrawn(page), "frames were drawn").toBeGreaterThan(drawn);
    expect(Buffer.compare(await canvas.screenshot(), pixelsBefore), "the canvas changed").not.toBe(0);
    expect(errors).toEqual([]);
  });
}

test("recovers from a lost WebGL2 context without a reload", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  await page.evaluate(() => {
    const gl = document.querySelector<HTMLCanvasElement>("#tank")!.getContext("webgl2")!;
    window.loseExt = gl.getExtension("WEBGL_lose_context")!;
    window.loseExt.loseContext();
  });
  await page.waitForTimeout(300);
  const lostAt = await framesDrawn(page);
  await page.waitForTimeout(300);
  expect(await framesDrawn(page), "nothing is drawn while the context is lost").toBe(lostAt);

  const ticks = await page.evaluate(() => window.desktank.tank.tickCount());
  await page.evaluate(() => window.loseExt.restoreContext());
  await expect.poll(() => framesDrawn(page), { timeout: 10_000 }).toBeGreaterThan(lostAt);
  expect(await page.evaluate(() => window.desktank.tank.tickCount()), "the world kept its state").toBeGreaterThanOrEqual(ticks);

  // Pour after recovery: the new renderer draws the change.
  const box = await boxOf(page.locator("#tank"));
  const drawn = await framesDrawn(page);
  await page.mouse.move(box.x + box.width / 2, box.y + box.height * 0.2);
  await page.mouse.down();
  await page.waitForTimeout(400);
  await page.mouse.up();
  await expect.poll(() => framesDrawn(page)).toBeGreaterThan(drawn);
  expect(errors).toEqual([]);
});

test("reset and a new seed start a fresh tank that draws", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  await page.waitForTimeout(300);
  await page.getByRole("button", { name: "Reset" }).click();
  await expect.poll(() => page.evaluate(() => window.desktank.tank.framesDrawn())).toBeGreaterThan(0);
  const hash = await page.evaluate(() => window.desktank.tank.stateHash());
  await page.getByRole("button", { name: "Settings" }).click();
  await page.locator("#seed").fill("42");
  await page.locator("#seed").dispatchEvent("change");
  await expect.poll(() => page.evaluate(() => window.desktank.tank.framesDrawn())).toBeGreaterThan(0);
  expect(await page.evaluate(() => window.desktank.tank.stateHash())).not.toBe(hash);
  expect(await page.evaluate(() => window.desktank.backend)).toBe("webgl2");
  expect(errors).toEqual([]);
});

test("reset during a lost WebGL2 context draws once restored", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  await page.evaluate(() => {
    const gl = document.querySelector<HTMLCanvasElement>("#tank")!.getContext("webgl2")!;
    window.loseExt = gl.getExtension("WEBGL_lose_context")!;
    window.loseExt.loseContext();
  });
  await page.getByRole("button", { name: "Reset" }).click();
  await page.waitForTimeout(200);
  expect(await page.evaluate(() => window.desktank.tank.framesDrawn()), "the new tank waits for the context").toBe(0);
  await page.evaluate(() => window.loseExt.restoreContext());
  await expect.poll(() => page.evaluate(() => window.desktank.tank.framesDrawn()), { timeout: 10_000 }).toBeGreaterThan(0);
  expect(errors).toEqual([]);
});

test("the bubbler and the new materials work from the palette", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  const canvas = page.locator("#tank");
  // Clicking a tab or swatch may scroll the palette into view (and the
  // canvas partly out), so bring the canvas back and measure it after each.
  const box = async () => {
    await canvas.scrollIntoViewIfNeeded();
    return boxOf(canvas);
  };
  // Material counts, not the state hash: that changes every tick anyway.
  const cells = (name: string) => page.evaluate((n) => window.desktank.tank.materialCount(n), name);
  expect(await cells("bubbler")).toBe(0);

  // A click with the bubbler places one (8 cells), which releases bubbles.
  await choose(page, "Bubbler");
  let b = await box();
  await page.mouse.click(b.x + b.width * 0.5, b.y + b.height * 0.5);
  expect(await cells("bubbler")).toBe(8);
  await expect.poll(() => cells("air_bubble"), { timeout: 10_000 }).toBeGreaterThan(0);

  // Liquid fertiliser pours like any material.
  await choose(page, "Liquid fertiliser");
  b = await box();
  await page.mouse.move(b.x + b.width * 0.2, b.y + b.height * 0.3);
  await page.mouse.down();
  await page.waitForTimeout(300);
  await page.mouse.up();
  expect(await cells("liquid_fertiliser")).toBeGreaterThan(0);
  expect(errors).toEqual([]);
});

test("each plant species has a palette item that pours it", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  const tank = <K extends TankMethod>(f: K, ...a: Parameters<WebTank[K]>) => callTank(page, f, ...a);
  // Java moss is under Plants › Moss & epiphytes, in its own colour.
  await choose(page, "Java moss");
  await expect(page.getByRole("tab", { name: "Plants" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("tab", { name: "Moss & epiphytes" })).toHaveAttribute("aria-selected", "true");
  const slot = page.getByRole("radio", { name: "Java moss" });
  await expect(slot).toHaveAttribute("aria-checked", "true");
  const swatch = await slot.locator("i").evaluate((el) => el.style.getPropertyValue("--c"));
  const colours = await tank("paletteColours");
  const entries = await tank("palette");
  expect(swatch).toBe(colours[entries.indexOf("plant java_moss")]);
  // Every plant item is under Plants, with its description as a tooltip.
  const cats = await tank("paletteCategories");
  for (const [i, e] of entries.entries()) if (e.startsWith("plant ")) expect(cats[i], e).toMatch(/^plants\//);
  await expect(slot).toHaveAttribute("title", /\S · Key \d/);
  // A click pours its fragments (the cutting material).
  const canvas = page.locator("#tank");
  await canvas.scrollIntoViewIfNeeded();
  const b = await boxOf(canvas);
  await page.mouse.click(b.x + b.width * 0.5, b.y + b.height * 0.3);
  expect(await tank("materialCount", "cutting")).toBeGreaterThan(0);
  // The selection survives a reset.
  await page.getByRole("button", { name: "Reset" }).click();
  await expect(page.getByRole("radio", { name: "Java moss" })).toHaveAttribute("aria-checked", "true");
  expect(errors).toEqual([]);
});

test("the propagule chips offer the species' own methods and pour seeds", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  const tank = <K extends TankMethod>(f: K, ...a: Parameters<WebTank[K]>) => callTank(page, f, ...a);
  const row = page.locator("#propagules");
  const chips = row.getByRole("radio");
  // No chips until a plant is selected.
  await expect(row).toBeHidden();
  // Hygrophila grows from all three; cuttings come first.
  await choose(page, "Hygrophila");
  await expect(row).toBeVisible();
  await expect(chips).toHaveText(["Seeds", "Cuttings", "Fragments"]);
  await expect(row.getByRole("radio", { name: "Cuttings" })).toHaveAttribute("aria-checked", "true");
  // Picking seeds keeps the plant selected, and a click pours seeds.
  await row.getByRole("radio", { name: "Seeds" }).click();
  await expect(row.getByRole("radio", { name: "Seeds" })).toHaveAttribute("aria-checked", "true");
  await expect(page.getByRole("radio", { name: "Hygrophila" })).toHaveAttribute("aria-checked", "true");
  expect(await tank("plantPropagule")).toBe("seeds");
  const canvas = page.locator("#tank");
  await canvas.scrollIntoViewIfNeeded();
  const b = await boxOf(canvas);
  await page.mouse.click(b.x + b.width * 0.5, b.y + b.height * 0.3);
  expect(await tank("materialCount", "seed")).toBeGreaterThan(0);
  // Rotala only grows from cuttings; java moss only from fragments.
  await choose(page, "Rotala");
  await expect(chips).toHaveText(["Cuttings"]);
  await choose(page, "Java moss");
  await expect(chips).toHaveText(["Fragments"]);
  // Carpets and rosettes spread by runners (T6c), floaters by buds.
  await choose(page, "Dwarf hairgrass");
  await expect(chips).toHaveText(["Fragments", "Runners"]);
  await choose(page, "Vallisneria");
  await expect(chips).toHaveText(["Runners"]);
  await choose(page, "Duckweed");
  await expect(chips).toHaveText(["Fragments", "Buds"]);
  // Arrow keys move the pick and wrap.
  await row.getByRole("radio", { name: "Fragments" }).focus();
  await page.keyboard.press("ArrowLeft");
  await expect(row.getByRole("radio", { name: "Buds" })).toBeFocused();
  expect(await tank("plantPropagule")).toBe("buds");
  // Another tab hides the chips; a non-plant item too.
  await page.getByRole("tab", { name: "Fish", exact: true }).click();
  await expect(row).toBeHidden();
  await choose(page, "Sand");
  await expect(row).toBeHidden();
  // Hygrophila keeps its seeds pick, through a reset too.
  await choose(page, "Hygrophila");
  await expect(row.getByRole("radio", { name: "Seeds" })).toHaveAttribute("aria-checked", "true");
  await page.getByRole("button", { name: "Reset" }).click();
  await expect(row.getByRole("radio", { name: "Seeds" })).toHaveAttribute("aria-checked", "true");
  await choose(page, "Duckweed");
  await expect.poll(() => tank("plantPropagule")).toBe("buds");
  expect(errors).toEqual([]);
});
