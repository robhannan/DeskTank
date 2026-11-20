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

test("the plants slot pours the species picked beside it", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  const tank = <K extends TankMethod>(f: K, ...a: Parameters<WebTank[K]>) => callTank(page, f, ...a);
  const picker = page.locator("#plant-species");
  const names = await tank("plantSpecies");
  await expect(picker.locator("option")).toHaveCount(names.length);
  // Picking a species selects the plants slot and colours its swatch.
  await picker.selectOption({ label: "Java moss" });
  const slot = page.getByRole("radio", { name: "Plants" });
  await expect(slot).toHaveAttribute("aria-checked", "true");
  const moss = names.findIndex((n) => n.startsWith("java_moss "));
  expect(await tank("plantSpeciesSelected")).toBe(moss);
  const swatch = await slot.locator("i").evaluate((el) => el.style.getPropertyValue("--c"));
  const colours = await tank("paletteColours");
  const entries = await tank("palette");
  expect(swatch).toBe(colours[entries.indexOf("plant plants")]);
  // A click pours its cuttings.
  const canvas = page.locator("#tank");
  await canvas.scrollIntoViewIfNeeded();
  const b = await boxOf(canvas);
  await page.mouse.click(b.x + b.width * 0.5, b.y + b.height * 0.3);
  expect(await tank("materialCount", "cutting")).toBeGreaterThan(0);
  // The pick survives a reset.
  await page.getByRole("button", { name: "Reset" }).click();
  await expect.poll(() => tank("plantSpeciesSelected")).toBe(moss);
  await expect(picker).toHaveValue(String(moss));
  expect(errors).toEqual([]);
});

test("the propagule picker offers the species' own methods and pours seeds", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  const tank = <K extends TankMethod>(f: K, ...a: Parameters<WebTank[K]>) => callTank(page, f, ...a);
  const species = page.locator("#plant-species");
  const way = page.locator("#plant-propagule");
  // Hygrophila (the default) grows from all three; cuttings come first.
  await expect(way.locator("option")).toHaveText(["Seeds", "Cuttings", "Fragments"]);
  await expect(way).toHaveValue("cuttings");
  await expect(way).toBeEnabled();
  // Picking seeds selects the plants slot, and a click pours seeds.
  await way.selectOption("seeds");
  await expect(page.getByRole("radio", { name: "Plants" })).toHaveAttribute("aria-checked", "true");
  expect(await tank("plantPropagule")).toBe("seeds");
  const canvas = page.locator("#tank");
  await canvas.scrollIntoViewIfNeeded();
  const b = await boxOf(canvas);
  await page.mouse.click(b.x + b.width * 0.5, b.y + b.height * 0.3);
  expect(await tank("materialCount", "seed")).toBeGreaterThan(0);
  // Rotala only grows from cuttings; java moss only from fragments.
  await species.selectOption({ label: "Rotala" });
  await expect(way.locator("option")).toHaveText(["Cuttings"]);
  await expect(way).toBeDisabled();
  await species.selectOption({ label: "Java moss" });
  await expect(way.locator("option")).toHaveText(["Fragments"]);
  // Carpets and rosettes spread by runners (T6c).
  await species.selectOption({ label: "Dwarf hairgrass" });
  await expect(way.locator("option")).toHaveText(["Fragments", "Runners"]);
  await species.selectOption({ label: "Vallisneria" });
  await expect(way.locator("option")).toHaveText(["Runners"]);
  await species.selectOption({ label: "Monte Carlo" });
  // Back on hygrophila the seeds pick still holds; it survives a reset,
  // even one made while on a species without seeds.
  await species.selectOption({ label: "Hygrophila" });
  await expect(way).toHaveValue("seeds");
  await species.selectOption({ label: "Rotala" });
  await page.getByRole("button", { name: "Reset" }).click();
  await expect(species).toHaveValue(String((await tank("plantSpecies")).findIndex((n) => n.startsWith("rotala "))));
  await expect(way).toHaveValue("cuttings");
  await species.selectOption({ label: "Hygrophila" });
  await expect.poll(() => tank("plantPropagule")).toBe("seeds");
  await expect(way).toHaveValue("seeds");
  expect(errors).toEqual([]);
});
