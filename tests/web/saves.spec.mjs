// WB3 web persistence and ownership (SPEC §3.5, §9 MW): IndexedDB saves,
// the visibility pause, Web Locks ownership with Take over, and export and
// import in the native app's save format.
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { open, stateHash, watchErrors } from "./support.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
// Written by the native app (desktank-sim's T8 fixture: 200×120 cells, tick
// 1200). crates/desktank-web/src/tests.rs checks the same hash natively.
const NATIVE_SAVE = path.join(root, "crates/desktank-sim/tests/fixtures/v1.dtank");
// The same tank as native re-saves it with this build's name tables
// (`desktank-web`'s `a_native_save_opens` keeps it current).
const NATIVE_RESAVED = path.join(root, "crates/desktank-web/tests/fixtures/v1-resaved.dtank");
const NATIVE_HASH = "c49879bcf4b019f9";
// The `layers` render scene at seed 1, saved natively by desktank-web's
// tests (re-bless there with DESKTANK_BLESS=1).
const SCENE = path.join(root, "crates/desktank/tests/scenes/layers.toml");
const SCENE_SAVE = path.join(root, "crates/desktank-web/tests/fixtures/layers-seed1.dtank");

const ticks = (page) => page.evaluate(() => window.desktank.tank.tickCount());
const owner = (page) => page.evaluate(() => window.desktank.owner);
const writes = (page) => page.evaluate(() => window.desktank.writes);

// Makes the page hidden as a background tab or a minimised window would.
async function setHidden(page, hidden) {
  await page.evaluate((hidden) => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => (hidden ? "hidden" : "visible") });
    document.dispatchEvent(new Event("visibilitychange"));
  }, hidden);
}

async function pour(page) {
  const box = await page.locator("#tank").boundingBox();
  await page.mouse.move(box.x + box.width * 0.3, box.y + box.height * 0.2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * 0.6, box.y + box.height * 0.2, { steps: 8 });
  await page.mouse.up();
}

async function download(page, button) {
  // dispatchEvent: the render scene pins the canvas over the menu.
  const [file] = await Promise.all([page.waitForEvent("download"), page.getByRole("button", { name: button }).dispatchEvent("click")]);
  expect(file.suggestedFilename()).toMatch(/^desktank-\d{8}-\d{4}\.dtank$/);
  return readFile(await file.path());
}

test("a hidden tab runs no ticks, and a reload restores the hidden state", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  expect(await owner(page)).toBe(true);
  await pour(page);
  await expect.poll(() => ticks(page)).toBeGreaterThan(20);

  await setHidden(page, true);
  const hiddenAt = await ticks(page);
  expect(await page.evaluate(() => window.desktank.saving), "saved when hidden").toBe(true);
  const hash = await stateHash(page);
  await page.waitForTimeout(1500);
  expect(await ticks(page), "no ticks while hidden").toBe(hiddenAt);
  expect(await stateHash(page)).toBe(hash);

  // Shown again, world time carries on from where it stopped (no catch-up).
  await setHidden(page, false);
  await page.waitForTimeout(1000);
  const ran = (await ticks(page)) - hiddenAt;
  expect(ran).toBeGreaterThan(5);
  // 1.5 s hidden caught up would add 30 ticks to the ~20 of 1 s shown.
  expect(ran, "the hidden time isn't caught up").toBeLessThan(45);

  await setHidden(page, true);
  await page.evaluate(() => window.desktank.saving);
  const saved = { hash: await stateHash(page), tick: await ticks(page) };
  await page.reload();
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
  const loaded = await page.evaluate(() => window.desktank.loaded);
  expect({ hash: loaded.hash, tick: loaded.tick }).toEqual(saved);
  await expect(page.locator("#saved")).toHaveText(/^Saved \d/);
  expect(errors).toEqual([]);
});

test("a second tab is read-only until it takes over, within 1 s and without losing state", async ({ context }) => {
  const a = await context.newPage();
  const errors = watchErrors(a);
  await open(a, "?backend=webgl2");
  await pour(a);
  expect(await owner(a)).toBe(true);

  const b = await context.newPage();
  errors.push(...watchErrors(b));
  await open(b, "?backend=webgl2");
  expect(await owner(b)).toBe(false);
  await expect(b.getByText("open in another tab")).toBeVisible();
  await expect(b.getByRole("button", { name: "Take over" })).toBeVisible();
  // The viewer doesn't simulate, and never writes, even when hidden or
  // leaving, or when asked to save.
  const shown = await ticks(b);
  await b.waitForTimeout(500);
  expect(await ticks(b)).toBe(shown);
  await setHidden(b, true);
  await b.evaluate(() => window.dispatchEvent(new PageTransitionEvent("pagehide")));
  await setHidden(b, false);
  expect(await writes(b)).toBe(0);
  // Its controls are inert.
  expect(await b.locator("nav.menu").evaluate((el) => el.inert)).toBe(true);

  await expect.poll(() => ticks(a)).toBeGreaterThan(20);
  const aWrites = await writes(a);
  await b.getByRole("button", { name: "Take over" }).click();
  await expect.poll(() => owner(b), { timeout: 2000 }).toBe(true);
  const took = await b.evaluate(() => window.desktank.ownedAt - window.desktank.takeOverAt);
  console.log(`take over: ${took.toFixed(0)} ms`);
  expect(took).toBeLessThan(1000);

  // A saved on handing over; B opened exactly that state.
  const released = await a.evaluate(() => window.desktank.released);
  const loaded = await b.evaluate(() => window.desktank.loaded);
  expect({ hash: loaded.hash, tick: loaded.tick }).toEqual(released);
  expect(await writes(a)).toBe(aWrites + 1);
  await expect(b.locator("#elsewhere")).toBeHidden();
  await expect.poll(() => ticks(b)).toBeGreaterThan(released.tick);

  // A is now the viewer: paused, and it never writes again.
  expect(await owner(a)).toBe(false);
  await expect(a.getByText("open in another tab")).toBeVisible();
  const aTicks = await ticks(a);
  await setHidden(a, true);
  await a.waitForTimeout(300);
  expect(await ticks(a)).toBe(aTicks);
  expect(await writes(a)).toBe(aWrites + 1);

  // Closing the owner hands the tank back to the queued viewer.
  await setHidden(b, true);
  await b.evaluate(() => window.desktank.saving);
  const last = await stateHash(b);
  await b.close();
  await setHidden(a, false);
  await expect.poll(() => owner(a), { timeout: 5000 }).toBe(true);
  expect(await a.evaluate(() => window.desktank.loaded.hash)).toBe(last);
  expect(errors).toEqual([]);
});

test("Take over steals the lock from an owner that doesn't answer", async ({ context }) => {
  const a = await context.newPage();
  const errors = watchErrors(a);
  await open(a, "?backend=webgl2");
  // A frozen owner: it never hears the request.
  await a.evaluate(() => window.desktank.ownership.channel.close());
  const b = await context.newPage();
  errors.push(...watchErrors(b));
  await open(b, "?backend=webgl2");
  expect(await owner(b)).toBe(false);
  await b.getByRole("button", { name: "Take over" }).click();
  await expect.poll(() => owner(b), { timeout: 3000 }).toBe(true);
  const took = await b.evaluate(() => window.desktank.ownedAt - window.desktank.takeOverAt);
  expect(took).toBeGreaterThanOrEqual(1000);
  expect(took).toBeLessThan(2000);
  // The epoch refuses a write the old owner started before it learned.
  expect(await a.evaluate(() => window.desktank.lateWrite())).toBe("another tab owns the tank now");
  // The old owner learns it lost the lock, stops and never writes again.
  await expect.poll(() => owner(a)).toBe(false);
  await expect(a.getByText("open in another tab")).toBeVisible();
  const aWrites = await writes(a);
  const aTicks = await ticks(a);
  await setHidden(a, true);
  await a.waitForTimeout(300);
  expect(await writes(a)).toBe(aWrites);
  expect(await ticks(a)).toBe(aTicks);
  expect(errors).toEqual([]);
});

test("a native save imports, letterboxed, and exports what native writes", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  // Paused, so nothing ticks between import and export.
  await page.getByRole("button", { name: "Pause" }).click();
  const native = await readFile(NATIVE_RESAVED);
  await page.locator("#import-file").setInputFiles(NATIVE_SAVE);
  await expect(page.locator("#notice")).toContainText("Imported v1.dtank: 200×120 cells");
  expect(await page.evaluate(() => [window.desktank.tank.width(), window.desktank.tank.height()])).toEqual([200, 120]);
  expect(await ticks(page)).toBe(1200);
  expect(await stateHash(page)).toBe(NATIVE_HASH);
  expect(await page.evaluate(() => window.desktank.tank.running()), "keeps play/pause").toBe(false);
  // A different aspect than the canvas: drawn whole at the largest even
  // scale that fits, with a letterbox.
  const fit = await page.evaluate(() => {
    const t = window.desktank.tank;
    const c = document.querySelector("#tank");
    const s = t.cellScale();
    const [gw, gh] = [t.width(), t.height()];
    return { s, w: gw * s, h: gh * s, nw: gw * (s + 2), nh: gh * (s + 2), cw: c.width, ch: c.height };
  });
  expect(fit.s % 2 === 0 || fit.s === 1).toBe(true);
  expect(fit.w <= fit.cw && fit.h <= fit.ch).toBe(true);
  expect(fit.w < fit.cw || fit.h < fit.ch, "letterboxed").toBe(true);
  expect(fit.nw > fit.cw || fit.nh > fit.ch, "the largest even scale").toBe(true);

  expect(Buffer.compare(await download(page, "Export"), native), "the export is what native re-saves").toBe(0);

  // The import was saved: a reload opens it.
  await page.reload();
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
  expect(await page.evaluate(() => window.desktank.loaded.hash)).toBe(NATIVE_HASH);

  // Not a save: refused with a message, the tank unchanged.
  await page.getByRole("button", { name: "Pause" }).click();
  const hash = await stateHash(page);
  await page.locator("#import-file").setInputFiles({ name: "junk.dtank", mimeType: "application/octet-stream", buffer: Buffer.from("not a save") });
  await expect(page.locator("#notice")).toContainText("Couldn't import junk.dtank: not a DeskTank save");
  expect(await page.evaluate(() => window.desktank.tank.width())).toBe(200);
  expect(await stateHash(page)).toBe(hash);
  expect(errors).toEqual([]);
});

test("a web export is byte for byte what the native app writes", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  const toml = await readFile(SCENE, "utf8");
  await page.evaluate(([toml]) => window.desktank.loadScene(toml, 1, 1280, 720), [toml]);
  const web = await download(page, "Export");
  expect(Buffer.compare(web, await readFile(SCENE_SAVE)), "same bytes as the native save").toBe(0);
  expect(errors).toEqual([]);
});
