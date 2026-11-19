// Cheats: the Konami code opens the cheat field, Enter runs a cheat, an
// unknown one says so and leaves the field open, and Escape closes it.
import { expect, test } from "@playwright/test";
import { openPaused, watchErrors } from "./support.mjs";

const KONAMI = ["ArrowUp", "ArrowUp", "ArrowDown", "ArrowDown", "ArrowLeft", "ArrowRight", "ArrowLeft", "ArrowRight", "b", "a"];
const creatures = (page) => page.evaluate(() => window.desktank.tank.creatures());

async function konami(page) {
  // Keys go to the page, not a form control.
  await page.locator("h1").click();
  for (const key of KONAMI) await page.keyboard.press(key);
}

test("the Konami code opens the cheat field and cheats run", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  const field = page.getByRole("textbox", { name: "Cheat" });
  await expect(field).toBeHidden();

  // Most of the code, then a stray key, opens nothing.
  await page.locator("h1").click();
  for (const key of KONAMI.slice(0, 8)) await page.keyboard.press(key);
  await page.keyboard.press("x");
  await page.keyboard.press("b");
  await page.keyboard.press("a");
  await expect(field).toBeHidden();

  await konami(page);
  await expect(field).toBeVisible();
  await expect(field).toBeFocused();

  // An unknown cheat says so and keeps the field open.
  await field.fill("rosebud");
  await field.press("Enter");
  await expect(page.locator("#cheat-out")).toContainText("unknown cheat");
  await expect(field).toBeVisible();

  // A real one runs, closes the field and reports in the readout.
  const before = await creatures(page);
  await field.fill("spawn neon_tetra 3");
  await field.press("Enter");
  await expect(field).toBeHidden();
  await expect(page.locator("#inspect")).toHaveText("Cheat: spawned 3 neon_tetra");
  expect(await creatures(page)).toBe(before + 3);

  // Escape closes the field without running anything.
  await konami(page);
  await expect(field).toBeVisible();
  await field.fill("spawn guppy");
  await field.press("Escape");
  await expect(field).toBeHidden();
  expect(await creatures(page)).toBe(before + 3);
  expect(errors).toEqual([]);
});
