// WB3 web persistence and ownership (SPEC §3.5, §9 MW): IndexedDB saves,
// the visibility pause, Web Locks ownership with Take over, and export and
// import in the native app's save format.
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, type Page, test } from "@playwright/test";
import { boxOf, cellPoint, choose, framesDrawn, gridSize, open, stateHash, watchErrors } from "./support.ts";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
// Written by the native app (desktank-sim's T8 fixture: 200×120 cells, tick
// 1200). crates/desktank-web/src/tests.rs checks the same hash natively.
const NATIVE_SAVE = path.join(root, "crates/desktank-sim/tests/fixtures/v1.dtank");
// The same tank as native re-saves it with this build's name tables
// (`desktank-web`'s `a_native_save_opens` keeps it current).
const NATIVE_RESAVED = path.join(root, "crates/desktank-web/tests/fixtures/v1-resaved.dtank");
const NATIVE_HASH = "54685e4c472e75d9";
// The `layers` render scene at seed 1, saved natively by desktank-web's
// tests (re-bless there with DESKTANK_BLESS=1).
const SCENE = path.join(root, "crates/desktank/tests/scenes/layers.toml");
const SCENE_SAVE = path.join(root, "crates/desktank-web/tests/fixtures/layers-seed1.dtank");

// The tank's simulation tick (desktank_sim::TICKS_PER_SECOND, 20 Hz).
const TICK_MS = 50;

const ticks = (page: Page) => page.evaluate(() => window.desktank.tank.tickCount());
const owner = (page: Page) => page.evaluate(() => window.desktank.owner);
const writes = (page: Page) => page.evaluate(() => window.desktank.writes);

// Makes the page hidden as a background tab or a minimised window would.
async function setHidden(page: Page, hidden: boolean) {
  await page.evaluate((hidden) => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => (hidden ? "hidden" : "visible") });
    document.dispatchEvent(new Event("visibilitychange"));
  }, hidden);
}

async function pour(page: Page) {
  const box = await boxOf(page.locator("#tank"));
  await page.mouse.move(box.x + box.width * 0.3, box.y + box.height * 0.2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * 0.6, box.y + box.height * 0.2, { steps: 8 });
  await page.mouse.up();
}

async function pauseAndDrainFrames(page: Page) {
  await page.getByRole("button", { name: "Pause" }).click();
  await page.evaluate(() => new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  }));
}

async function download(page: Page, button: string) {
  // dispatchEvent: the render scene pins the canvas over the menu.
  const [file] = await Promise.all([page.waitForEvent("download"), page.getByRole("button", { name: button }).dispatchEvent("click")]);
  expect(file.suggestedFilename()).toMatch(/^desktank-\d{8}-\d{4}\.dtank$/);
  return readFile(await file.path());
}

test("a named creature keeps its name across a reload", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  // Add a neon tetra in mid water and name it.
  const [w, h] = await gridSize(page);
  await choose(page, "Neon tetra");
  const p = await cellPoint(page, Math.round(w / 2), Math.round(h / 2));
  await page.mouse.click(p.x, p.y);
  const id = await page.evaluate(() => {
    const t = window.desktank.tank;
    for (let id = 1; id < 1000; id++) {
      if (t.creatureName(id) !== undefined) {
        t.nameCreature(id, "Señor Fin");
        return id;
      }
    }
    return 0;
  });
  expect(id, "the starter tank has creatures").toBeGreaterThan(0);
  await setHidden(page, true);
  await page.evaluate(() => window.desktank.saving);
  await page.reload();
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
  expect(await page.evaluate((id) => window.desktank.tank.creatureName(id), id)).toBe("Señor Fin");
  expect(errors).toEqual([]);
});

test("a hidden tab runs no ticks, and a reload restores the hidden state", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  expect(await owner(page)).toBe(true);
  await pour(page);
  await expect.poll(() => ticks(page), { timeout: 15_000 }).toBeGreaterThan(20);

  await setHidden(page, true);
  const hiddenAt = await ticks(page);
  expect(await page.evaluate(() => window.desktank.saving), "saved when hidden").toBe(true);
  const hash = await stateHash(page);
  await page.waitForTimeout(1500);
  expect(await ticks(page), "no ticks while hidden").toBe(hiddenAt);
  expect(await stateHash(page)).toBe(hash);

  // Shown again, world time carries on from where it stopped: it resumes,
  // and never runs more ticks than the page's own clock allows since it was
  // shown (catching up the hidden 1.5 s would run 4 at once, MAX_CATCH_UP).
  // No wall-clock window: on a loaded CI runner a frame drawn on SwiftShader
  // can hold the page's timers back by 400 ms and the pacer drops the
  // backlog, so a wall second holds anywhere from 5 to 20 ticks.
  // The bound allows a tick per 50 ms since showing, so a catch-up is caught
  // only when the first loop after showing runs within ~150 ms (it ran at
  // 17 ms in the mutation check); a runner stalling it longer lets one by.
  const shownAt = await page.evaluate(() => performance.now());
  await setHidden(page, false);
  const samples: { ran: number; ms: number }[] = [];
  await expect
    .poll(async () => {
      const s = await page.evaluate(
        ([hiddenAt, shownAt]) => ({ ran: window.desktank.tank.tickCount() - hiddenAt, ms: performance.now() - shownAt }),
        [hiddenAt, shownAt],
      );
      samples.push(s);
      return s.ran;
    })
    .toBeGreaterThan(5);
  for (const { ran, ms } of samples) {
    // The clock re-anchors at the first frame after showing, which can be
    // stamped up to a vsync before `shownAt`: one tick of slack.
    expect(ran, `${ran} ticks ${ms.toFixed(0)} ms after showing: the hidden time was caught up`).toBeLessThanOrEqual(Math.floor(ms / TICK_MS) + 1);
  }

  await setHidden(page, true);
  await page.evaluate(() => window.desktank.saving);
  const saved = { hash: await stateHash(page), tick: await ticks(page) };
  await page.reload();
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
  const loaded = await page.evaluate(() => window.desktank.loaded);
  expect({ hash: loaded?.hash, tick: loaded?.tick }).toEqual(saved);
  await expect(page.locator("#saved")).toHaveText(/^Saved \d/);
  expect(errors).toEqual([]);
});

test("a compositor that holds animation frames back doesn't hold world time back", async ({ page }) => {
  // CI's SwiftShader on a busy runner can hold the first animation frame
  // after a tab is shown for most of a second; ticks must not wait for it.
  // Once shown here, no frame comes at all: a loop gated on rAF never ticks
  // again, whatever the machine's speed.
  await page.addInitScript(() => {
    const raf = window.requestAnimationFrame.bind(window);
    const cancel = window.cancelAnimationFrame.bind(window);
    let held = 0;
    window.requestAnimationFrame = (cb) => (document.documentElement.dataset.holdFrames ? --held : raf(cb));
    window.cancelAnimationFrame = (id) => {
      if (id > 0) cancel(id);
    };
  });
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  // No pour: it would wake chunks, and `tank.frame` would then run full
  // SwiftShader draws from inside the tick loop even with rAF held.
  await expect.poll(() => framesDrawn(page)).toBeGreaterThan(0);
  await expect.poll(() => ticks(page), { timeout: 15_000 }).toBeGreaterThan(20);
  await setHidden(page, true);
  const hiddenAt = await ticks(page);
  await page.evaluate(() => (document.documentElement.dataset.holdFrames = "1"));
  await setHidden(page, false);
  await expect.poll(async () => (await ticks(page)) - hiddenAt).toBeGreaterThan(12);
  expect(errors).toEqual([]);
});

test("a second tab is read-only until it takes over, within 1 s and without losing state", async ({ context }) => {
  const a = await context.newPage();
  const errors = watchErrors(a);
  await open(a, "?backend=webgl2");
  await expect.poll(() => framesDrawn(a)).toBeGreaterThan(0);
  await pour(a);
  expect(await owner(a)).toBe(true);
  await expect.poll(() => ticks(a), { timeout: 15_000 }).toBeGreaterThan(20);
  // Drain pending SwiftShader draws before the viewer opens on the same GPU process.
  await pauseAndDrainFrames(a);

  const b = await context.newPage();
  errors.push(...watchErrors(b));
  await open(b, "?backend=webgl2");
  // Its first shader compile is done before Take over is timed.
  await expect.poll(() => framesDrawn(b)).toBeGreaterThan(0);
  expect(await owner(b)).toBe(false);
  await expect(b.getByText("open in another tab")).toBeVisible();
  await expect(b.getByRole("button", { name: "Take over" })).toBeVisible();

  // The viewer runs nothing and never writes, hidden, on pagehide or shown.
  const bTicks = await ticks(b);
  await b.waitForTimeout(500);
  expect(await ticks(b), "a viewer doesn't tick").toBe(bTicks);
  await setHidden(b, true);
  await b.evaluate(() => window.dispatchEvent(new Event("pagehide")));
  await setHidden(b, false);
  await b.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
  expect(await writes(b), "a viewer never writes").toBe(0);
  expect(await b.locator("nav.menu").evaluate((el) => (el as HTMLElement).inert)).toBe(true);

  const aWrites = await writes(a);
  await b.getByRole("button", { name: "Take over" }).click();
  await expect.poll(() => owner(b), { timeout: 5000 }).toBe(true);
  await b.evaluate(() => window.desktank.claiming);
  expect(await b.evaluate(() => window.desktank.stole)).toBe(false);
  const took = await b.evaluate(() => window.desktank.ownedAt - window.desktank.takeOverAt);
  expect(took, "an owner that answers hands over fast").toBeLessThan(1000);

  // The new owner opens exactly the state the old one released.
  const released = await a.evaluate(() => window.desktank.released);
  const loaded = await b.evaluate(() => window.desktank.loaded);
  expect({ hash: loaded?.hash, tick: loaded?.tick }).toEqual({ hash: released?.hash, tick: released?.tick });
  expect(await writes(a), "the old owner saved once on release").toBe(aWrites + 1);
  await expect(b.locator("#elsewhere")).toBeHidden();
  await expect.poll(() => ticks(b), { timeout: 15_000 }).toBeGreaterThan(released!.tick);

  // The old owner is a viewer now.
  expect(await owner(a)).toBe(false);
  await expect(a.getByText("open in another tab")).toBeVisible();
  const aTicks = await ticks(a);
  const aWrites2 = await writes(a);
  await setHidden(a, true);
  await a.waitForTimeout(300);
  expect(await ticks(a)).toBe(aTicks);
  expect(await writes(a)).toBe(aWrites2);

  // Closing the new owner hands the tank back, with its last save.
  await setHidden(b, true);
  await b.evaluate(() => window.desktank.saving);
  const last = await stateHash(b);
  await b.close();
  await setHidden(a, false);
  await expect.poll(() => owner(a), { timeout: 5000 }).toBe(true);
  await a.evaluate(() => window.desktank.claiming);
  expect((await a.evaluate(() => window.desktank.loaded))?.hash).toBe(last);
  expect(errors).toEqual([]);
});

test("Take over steals the lock from an owner that doesn't answer", async ({ context }) => {
  const a = await context.newPage();
  const errors = watchErrors(a);
  await open(a, "?backend=webgl2");
  // Neither tab's first shader compile nor background SwiftShader frames
  // may skew the steal timer: draw once, then pause.
  await expect.poll(() => framesDrawn(a)).toBeGreaterThan(0);
  await pauseAndDrainFrames(a);
  // A frozen owner: it never hears the request.
  await a.evaluate(() => window.desktank.ownership!.channel.close());
  const b = await context.newPage();
  errors.push(...watchErrors(b));
  await open(b, "?backend=webgl2");
  await expect.poll(() => framesDrawn(b)).toBeGreaterThan(0);
  expect(await owner(b)).toBe(false);
  await b.getByRole("button", { name: "Take over" }).click();
  await expect.poll(() => owner(b), { timeout: 3000 }).toBe(true);
  await b.evaluate(() => window.desktank.claiming);
  expect(await b.evaluate(() => window.desktank.stole)).toBe(true);
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
  // Importing reuses the WebGL2 graphics and pipelines for the new grid size.
  await expect(page.locator("#notice")).toContainText("Imported v1.dtank: 200×120 cells", { timeout: 15_000 });
  expect(await page.evaluate(() => [window.desktank.tank.width(), window.desktank.tank.height()])).toEqual([200, 120]);
  expect(await ticks(page)).toBe(1200);
  expect(await stateHash(page)).toBe(NATIVE_HASH);
  expect(await page.evaluate(() => window.desktank.tank.running()), "keeps play/pause").toBe(false);
  // A different aspect than the canvas: drawn whole at the largest even
  // scale that fits, with a letterbox.
  const fit = await page.evaluate(() => {
    const t = window.desktank.tank;
    const c = document.querySelector<HTMLCanvasElement>("#tank")!;
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
  expect(await page.evaluate(() => window.desktank.loaded?.hash)).toBe(NATIVE_HASH);

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
