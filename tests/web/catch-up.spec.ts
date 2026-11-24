import { expect, type Page, test } from "@playwright/test";
import { callTank, open, openPaused, setHidden, watchErrors } from "./support.ts";

const SAVED_AT = Date.UTC(2025, 0, 1, 12);
const TICKS_PER_SECOND = 20;
const lastCatchUp = (page: Page) => page.evaluate(() => window.desktank.lastCatchUp);
const ticks = (page: Page) => callTank(page, "tickCount");

async function catchUpSetting(page: Page) {
  await page.getByRole("button", { name: "Settings" }).click();
  return page.getByLabel("Catch up after time away");
}

// Save through the same path as a tab that goes into the background.
async function saveWhileHidden(page: Page) {
  await setHidden(page, true);
  expect(await page.evaluate(() => window.desktank.saving)).toBe(true);
  return ticks(page);
}

async function reload(page: Page) {
  await page.reload();
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
}

for (const enabled of [false, true]) {
  test(`reopening an old save with catch-up ${enabled ? "enabled" : "off by default"}`, async ({ page }) => {
    const errors = watchErrors(page);
    const logs: string[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "info" && msg.text().startsWith("desktank web: caught up")) logs.push(msg.text());
    });
    await page.clock.setFixedTime(SAVED_AT);
    await openPaused(page);
    const setting = await catchUpSetting(page);
    await expect(setting).not.toBeChecked();
    if (enabled) await setting.check();
    await callTank(page, "step");
    const savedTick = await saveWhileHidden(page);

    const seconds = 3600;
    await page.clock.setFixedTime(SAVED_AT + seconds * 1000);
    await reload(page);
    expect(await page.evaluate(() => window.desktank.loaded?.tick)).toBe(savedTick);
    expect(await page.evaluate(() => window.desktank.settings.catchUp)).toBe(enabled);
    if (enabled) {
      expect(await lastCatchUp(page)).toMatchObject({ seconds, births: expect.any(Number), deaths: expect.any(Number) });
      expect(await ticks(page)).toBeGreaterThanOrEqual(savedTick + seconds * TICKS_PER_SECOND);
      expect(logs).toEqual([expect.stringMatching(/^desktank web: caught up 1h 0m \(\d+ births?, \d+ deaths?\)$/)]);
    } else {
      expect(await lastCatchUp(page)).toBeNull();
      expect(await ticks(page)).toBeLessThan(savedTick + seconds * TICKS_PER_SECOND);
      expect(logs).toEqual([]);
    }
    expect(errors).toEqual([]);
  });
}

test("a hidden tank uses the setting at resume and saves the applied time", async ({ page }) => {
  const errors = watchErrors(page);
  await page.clock.setFixedTime(SAVED_AT);
  await openPaused(page);
  const setting = await catchUpSetting(page);
  await setting.check();
  const start = await ticks(page);
  for (const gap of [-1000, 59_999]) {
    await page.clock.setFixedTime(SAVED_AT);
    await setHidden(page, true);
    await page.clock.setFixedTime(SAVED_AT + gap);
    await setHidden(page, false);
    expect(await lastCatchUp(page)).toBeNull();
    expect(await ticks(page)).toBe(start);
  }

  await setting.uncheck();
  await page.clock.setFixedTime(SAVED_AT);
  await setHidden(page, true);
  await setting.check();
  const seconds = 87;
  await page.clock.setFixedTime(SAVED_AT + seconds * 1000);
  await setHidden(page, false);
  const caughtUpTick = start + seconds * TICKS_PER_SECOND;
  expect(await ticks(page)).toBe(caughtUpTick);
  expect(await lastCatchUp(page)).toMatchObject({ seconds });
  expect(await saveWhileHidden(page)).toBe(caughtUpTick);
  await reload(page);
  expect(await page.evaluate(() => window.desktank.loaded?.tick)).toBe(caughtUpTick);
  expect(await lastCatchUp(page)).toBeNull();
  expect(errors).toEqual([]);
});

test("a viewer skips catch-up, including when it takes over a paused tank", async ({ context, page }) => {
  const errors = watchErrors(page);
  await page.clock.setFixedTime(SAVED_AT);
  await openPaused(page);
  await (await catchUpSetting(page)).check();
  await callTank(page, "step");
  const savedTick = await saveWhileHidden(page);
  await setHidden(page, false);
  expect(await callTank(page, "saveNeeded")).toBe(false);

  const viewer = await context.newPage();
  const viewerErrors = watchErrors(viewer);
  await viewer.clock.setFixedTime(SAVED_AT + 3_600_000);
  await open(viewer, "?backend=webgl2");
  expect(await viewer.evaluate(() => window.desktank.owner)).toBe(false);
  expect(await viewer.evaluate(() => window.desktank.settings.catchUp)).toBe(true);
  expect(await lastCatchUp(viewer)).toBeNull();
  expect(await ticks(viewer)).toBe(savedTick);
  await setHidden(viewer, true);
  const transferAt = SAVED_AT + 7_200_000;
  await viewer.clock.setFixedTime(transferAt);
  await setHidden(viewer, false);
  expect(await lastCatchUp(viewer)).toBeNull();
  expect(await ticks(viewer)).toBe(savedTick);

  await page.clock.setFixedTime(transferAt);
  await viewer.getByRole("button", { name: "Take over" }).click();
  await expect(viewer.locator("body")).toHaveAttribute("data-owner", "true");
  await viewer.evaluate(() => window.desktank.claiming);
  expect(await viewer.evaluate(() => window.desktank.loaded?.tick)).toBe(savedTick);
  expect(await lastCatchUp(viewer)).toBeNull();
  expect(errors).toEqual([]);
  expect(viewerErrors).toEqual([]);
});
