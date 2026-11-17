// Web smoke test (SPEC §7): the built site loads, pours, and draws, once on
// WebGL2 and once with the default backend (WebGPU, falling back to WebGL2
// where the browser has no adapter), with no console errors.
import { expect, test } from "@playwright/test";
import { framesDrawn, open, watchErrors } from "./support.mjs";

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

    const box = await canvas.boundingBox();
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
    window.loseExt = document.querySelector("#tank").getContext("webgl2").getExtension("WEBGL_lose_context");
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
  const box = await page.locator("#tank").boundingBox();
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
    window.loseExt = document.querySelector("#tank").getContext("webgl2").getExtension("WEBGL_lose_context");
    window.loseExt.loseContext();
  });
  await page.getByRole("button", { name: "Reset" }).click();
  await page.waitForTimeout(200);
  expect(await page.evaluate(() => window.desktank.tank.framesDrawn()), "the new tank waits for the context").toBe(0);
  await page.evaluate(() => window.loseExt.restoreContext());
  await expect.poll(() => page.evaluate(() => window.desktank.tank.framesDrawn()), { timeout: 10_000 }).toBeGreaterThan(0);
  expect(errors).toEqual([]);
});
