// WB2 (SPEC §9): the interact tools with mouse and emulated touch, the
// palette (the sim's list, keyboard navigation), the menu, the settings
// panel and the canvas's gesture suppression. The tank is paused so only the
// tools change cells.
import { expect, type Page, test } from "@playwright/test";
import {
  boxOf,
  cellCount,
  cellPoint,
  choose,
  gridSize,
  materialAt,
  open,
  openPaused,
  stateHash,
  touchscreen,
  watchErrors,
} from "./support.ts";

const glued = (page: Page) => page.evaluate(() => window.desktank.tank.gluedBodies());

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

    // Inspect shows its card and the cell under it; that changes nothing.
    const hash = await stateHash(page);
    await choose(page, "Inspect");
    p = await cellPoint(page, x, y);
    await page.mouse.click(p.x, p.y);
    await expect(page.locator("#inspect")).toHaveText(
      /^Neon tetra #1 · adult male · 180 days old · health 100% · energy 100% · over Water · cell \d+, \d+$/,
    );
    expect(await stateHash(page)).toBe(hash);

    // N opens its name field; Enter names it, and the card shows the name.
    await expect(page.locator("#name")).toBeVisible();
    await page.keyboard.press("n");
    await expect(page.locator("#name-input")).toBeFocused();
    await page.keyboard.type("  Big   Blue ");
    await page.keyboard.press("Enter");
    await expect(page.locator("#inspect")).toHaveText(/^Big Blue · Neon tetra #1 · adult male · /);
    await expect(page.locator("#name-input")).toHaveValue("Big Blue");
    // Inspecting it again shows the name it keeps.
    await page.mouse.click(p.x, p.y);
    await expect(page.locator("#inspect")).toHaveText(/^Big Blue · Neon tetra #1 · /);
    await expect(page.locator("#name-input")).toHaveValue("Big Blue");
    // Inspecting water hides the name field.
    const water = await cellPoint(page, 2, y);
    await page.mouse.click(water.x, water.y);
    await expect(page.locator("#inspect")).toHaveText(/^Water · cell /);
    await expect(page.locator("#name")).toBeHidden();

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
    const box = await boxOf(canvas);
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
    expect(await page.evaluate(() => visualViewport!.scale)).toBe(1);
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
    const tap = async (cx: number, cy: number) => {
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

    // D27: the internal filter goes whole onto the nearer (left) glass, and
    // inspect shows its card with load, maturity, Rinse and the guard toggle (DV3).
    await choose(page, "Internal filter");
    await tap(Math.round(w * 0.2), y);
    expect(await cellCount(page, "device")).toBeGreaterThan(300);
    await choose(page, "Inspect");
    await tap(4, y);
    await expect(page.locator("#inspect")).toHaveText(
      /^Internal filter #1 · on the left glass · flow [\d.]+ cells\/s · load 0% · maturity 0% · guard off · cell /,
    );
    await expect(page.locator("#device-actions")).toBeVisible();
    await page.locator("#device-guard").check();
    await expect(page.locator("#inspect")).toContainText("guard on");
    await page.locator("#device-rinse").click();
    await expect(page.locator("#inspect")).toContainText("load 0%");

    // DV4: the sponge filter drops onto the bed, is intrinsically fry-safe
    // (no guard checkbox), and supports Rinse.
    await choose(page, "Sponge filter");
    const spongeX = Math.round(w * 0.7);
    await tap(spongeX, Math.round(h * 0.4));
    await choose(page, "Inspect");
    await tap(spongeX, h - 6);
    await expect(page.locator("#inspect")).toHaveText(
      /^Sponge filter #2 · on the bed · flow [\d.]+ cells\/s · load 0% · maturity 0% · fry-safe · cell /,
    );
    await expect(page.locator("#device-actions")).toBeVisible();
    await expect(page.locator("#device-guard-label")).toBeHidden();
    await page.locator("#device-rinse").click();
    await expect(page.locator("#inspect")).toContainText("fry-safe");

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
    const box = await boxOf(page.locator("#tank"));
    const [cx, cy] = [box.x + box.width / 2, box.y + box.height / 2];
    const scroll = () => page.evaluate(() => [scrollX, scrollY, visualViewport!.scale]);
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
    const about = await boxOf(page.locator(".about"));
    const ax = about.x + about.width / 2;
    const ay = Math.min(about.y + 40, 840);
    await touch.drag([{ x: ax, y: ay }], [{ x: ax, y: ay - 300 }]);
    await expect.poll(async () => (await scroll())[1]).toBeGreaterThan(before[1]);
  });

  test("the palette tree fits a phone without scrolling sideways", async ({ page }) => {
    await open(page, "?backend=webgl2");
    await choose(page, "Hygrophila");
    for (const id of ["#tabs", "#subtabs", "#palette", "#propagules"]) {
      const scrolls = await page.locator(id).evaluate((el) => el.scrollWidth > el.clientWidth);
      expect(scrolls, id).toBe(false);
    }
    // Touch-sized targets.
    for (const sel of ['#tabs [role="tab"]', '#subtabs [role="tab"]:not([hidden])', "#palette .swatch:not([hidden])", "#propagules .chip"]) {
      const heights = await page.locator(sel).evaluateAll((els: HTMLElement[]) => els.map((b) => b.offsetHeight));
      expect(heights.length, sel).toBeGreaterThan(0);
      expect(Math.min(...heights), sel).toBeGreaterThanOrEqual(44);
    }
    // Every item can be reached through its tab and sub-tab.
    await choose(page, "Neon tetra");
    await expect(page.getByRole("tab", { name: "Fish", exact: true })).toHaveAttribute("aria-selected", "true");
    await expect(page.getByRole("tab", { name: "Schooling", exact: true })).toHaveAttribute("aria-selected", "true");
    // The nine tabs and Plants' six sub-tabs wrap rather than scroll.
    await expect(page.locator("#tabs").getByRole("tab")).toHaveCount(9);
    await choose(page, "Hygrophila");
    await expect(page.locator("#subtabs").getByRole("tab")).toHaveCount(6);
    for (const id of ["#tabs", "#subtabs"]) {
      const scrolls = await page.locator(id).evaluate((el) => el.scrollWidth > el.clientWidth);
      expect(scrolls, `${id} with Plants open`).toBe(false);
      const rows = await page.locator(id).getByRole("tab").evaluateAll((els: HTMLElement[]) => {
        const right = Math.max(...els.map((b) => b.getBoundingClientRect().right));
        return { rows: new Set(els.map((b) => b.offsetTop)).size, fits: right <= innerWidth };
      });
      expect(rows.rows, `${id} wrap onto several rows`).toBeGreaterThan(1);
      expect(rows.fits, `every one of ${id} is on screen`).toBe(true);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no page overflow").toBe(true);
  });
});

test("the palette follows the sim's category tree and navigates with the keyboard", async ({ page }) => {
  await open(page, "?backend=webgl2");
  const entries = await page.evaluate(() => window.desktank.tank.palette());
  const cats = await page.evaluate(() => window.desktank.tank.paletteCategories());
  const all = page.locator("#palette .swatch");
  await expect(all).toHaveCount(entries.length);
  const names = await all.evaluateAll((els) => els.map((b) => `${b.dataset.kind} ${b.dataset.name}`));
  expect(names).toEqual(entries);
  const materials = await page.evaluate(() => window.desktank.tank.materials());
  const species = await page.evaluate(() => window.desktank.tank.species());
  const liveFoods = await page.evaluate(() => window.desktank.tank.liveFoods());
  expect(liveFoods).toContain("live_bloodworm");
  const path = (i: number) => cats[i].split("\t")[0];
  for (const [i, e] of entries.entries()) {
    const [kind, name] = e.split(" ");
    if (kind === "creature") {
      expect(species, e).toContain(name);
      expect(path(i), `${e} is under Fish, Invertebrates or Amphibians`).toMatch(/^(fish\/|invertebrates\/|amphibians$)/);
    } else if (kind === "live_food") {
      expect(liveFoods, e).toContain(name);
      expect(path(i), `${e} is in Food › Live`).toBe("food/live");
    } else if (kind === "plant") expect(path(i), e).toMatch(/^plants\//);
    // Devices are named by kind (D27): the bubbler is still a material; the
    // internal, sponge and HOB filters are prefab devices.
    else if (kind === "device") expect(["bubbler", "internal_filter", "sponge_filter", "hob_filter"], e).toContain(name);
    else if (kind !== "tool") expect(materials, e).toContain(name);
  }
  expect(entries.filter((e) => e.startsWith("creature ")).length).toBe(species.length);
  expect(entries.filter((e) => e.startsWith("live_food ")).length).toBe(liveFoods.length);
  expect(entries.filter((e) => e.startsWith("plant ")).length, "one item per plant species").toBeGreaterThan(10);

  // One tab per top category, in order; it opens on the selection's
  // (sand, in Build › Substrate), showing only that sub-category's items.
  const tabs = page.locator("#tabs").getByRole("tab");
  await expect(tabs).toHaveText(["Tools", "Build", "Water", "Food", "Care", "Plants", "Fish", "Invertebrates", "Amphibians"]);
  // On a desktop all nine sit in one row, so the open tab joins its panel.
  const tops = await tabs.evaluateAll((els: HTMLElement[]) => new Set(els.map((b) => b.offsetTop)).size);
  expect(tops, "one row of tabs").toBe(1);
  const subs = page.locator("#subtabs").getByRole("tab");
  const tab = (name: string) => page.getByRole("tab", { name, exact: true });
  await expect(tab("Build")).toHaveAttribute("aria-selected", "true");
  await expect(subs).toHaveText(["Substrate", "Hardscape", "Devices"]);
  await expect(tab("Substrate")).toHaveAttribute("aria-selected", "true");
  const radios = page.locator("#palette").getByRole("radio");
  await expect(radios).toHaveText(["Sand", "Gravel", "Aquasoil"]);
  await expect(page.getByRole("radio", { name: "Sand" })).toHaveAttribute("aria-checked", "true");
  // Fish are split by stocking role; invertebrates into shrimp and snails;
  // amphibians are one sub-category, with no sub-tab row.
  await tab("Fish").click();
  await expect(subs).toHaveText(["Schooling", "Centrepiece", "Coldwater", "Clean-up crew"]);
  await expect(radios).toHaveText(["Neon tetra", "Guppy", "Harlequin rasbora"]);
  await tab("Invertebrates").click();
  await expect(subs).toHaveText(["Shrimp & crabs", "Snails"]);
  await expect(radios).toHaveText(["Cherry shrimp", "Amano shrimp", "Thai micro crab"]);
  await tab("Snails").click();
  await expect(radios).toHaveText(["Nerite", "Mystery snail", "Ramshorn snail"]);
  await tab("Amphibians").click();
  await expect(page.locator("#subtabs")).toBeHidden();
  await expect(radios).toHaveText(["African dwarf frog", "Axolotl", "Common frog"]);
  await tab("Fish").click();
  await tab("Clean-up crew").click();
  await expect(radios).toHaveText(["Corydoras", "Otocinclus", "Bristlenose pleco", "Kuhli loach"]);
  await expect(page.getByRole("radio", { name: "Sand" })).toBeHidden();
  await tab("Build").click();
  await expect(tab("Substrate")).toHaveAttribute("aria-selected", "true");

  // One tab, one sub-tab and one radio are in the tab order; arrows move
  // the selection within the sub-category and wrap.
  await expect(page.locator('#tabs [tabindex="0"]')).toHaveCount(1);
  await expect(page.locator('#subtabs [tabindex="0"]')).toHaveCount(1);
  await expect(page.locator('#palette [tabindex="0"]')).toHaveCount(1);
  await radios.first().focus();
  await page.keyboard.press("ArrowRight");
  await expect(radios.nth(1)).toBeFocused();
  await expect(radios.nth(1)).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("End");
  await expect(radios.last()).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("ArrowRight");
  await expect(radios.first()).toHaveAttribute("aria-checked", "true");

  // Arrows on the sub-tabs open the next sub-category; the selection
  // stays, marked on its sub-tab.
  await tab("Substrate").focus();
  await page.keyboard.press("ArrowRight");
  await expect(tab("Hardscape")).toBeFocused();
  await expect(tab("Hardscape")).toHaveAttribute("aria-selected", "true");
  await expect(radios).toHaveText(["Rock", "Driftwood"]);
  await expect(page.locator("#sub-build-substrate")).toHaveAttribute("data-holds", "");

  // Arrows on the tabs open the next tab; one with a single sub-category
  // has no sub-tab row. The selection is marked on its tab, and the open
  // sub-category's first radio takes the tab stop.
  await tab("Build").focus();
  await page.keyboard.press("ArrowRight");
  await expect(tab("Water")).toBeFocused();
  await expect(tab("Water")).toHaveAttribute("aria-selected", "true");
  await expect(page.locator("#subtabs")).toBeHidden();
  await expect(radios.first()).toHaveText("Water");
  await expect(page.locator("#tab-build")).toHaveAttribute("data-holds", "");
  await expect(page.locator('#palette [tabindex="0"]')).toHaveText("Water");

  // Digits pick items of the open sub-category anywhere on the page; G and
  // Shift+G step through the tabs, H and Shift+H through the sub-tabs.
  await page.locator("h1").click();
  await page.keyboard.press("2");
  await expect(page.getByRole("radio", { name: "Bubbles" })).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("9");
  await expect(page.getByRole("radio", { name: "Bubbles" })).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("g");
  await expect(tab("Food")).toHaveAttribute("aria-selected", "true");
  await expect(tab("Fish food")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("h");
  await expect(tab("Live")).toHaveAttribute("aria-selected", "true");
  await expect(radios).toHaveText(["Live bloodworms"]);
  await page.keyboard.press("h");
  await expect(tab("Fish food")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Shift+H");
  await expect(tab("Live")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Shift+G");
  await page.keyboard.press("Shift+G");
  // A tab opens on its first sub-category unless it holds the selection.
  await expect(tab("Build")).toHaveAttribute("aria-selected", "true");
  await expect(tab("Substrate")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("1");
  await expect(page.getByRole("radio", { name: "Sand" })).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("h");
  await page.keyboard.press("g");
  await page.keyboard.press("Shift+G");
  await expect(tab("Substrate"), "back on the selection's sub-category").toHaveAttribute("aria-selected", "true");
  // G steps through all nine tabs and wraps; Shift+G back. The selection's
  // tab is marked wherever the others are open.
  const order = ["Build", "Water", "Food", "Care", "Plants", "Fish", "Invertebrates", "Amphibians", "Tools", "Build"];
  for (const name of order.slice(1)) {
    await page.keyboard.press("g");
    await expect(tab(name)).toHaveAttribute("aria-selected", "true");
  }
  for (const name of [...order].reverse().slice(1)) {
    await page.keyboard.press("Shift+G");
    await expect(tab(name)).toHaveAttribute("aria-selected", "true");
  }
  await choose(page, "Nerite");
  await tab("Fish").click();
  await expect(page.locator("#tab-invertebrates")).toHaveAttribute("data-holds", "");
  await expect(page.locator("#tab-fish")).not.toHaveAttribute("data-holds", "");
  await tab("Invertebrates").click();
  await expect(tab("Snails"), "opens on the selection's sub-category").toHaveAttribute("aria-selected", "true");
  await expect(page.locator("#sub-invertebrates-snails")).not.toHaveAttribute("data-holds", "");
  await tab("Shrimp & crabs").click();
  await expect(page.locator("#sub-invertebrates-snails")).toHaveAttribute("data-holds", "");
  await choose(page, "Axolotl");
  await tab("Fish").click();
  await expect(page.locator("#tab-amphibians")).toHaveAttribute("data-holds", "");
  await expect(page.locator("#tab-invertebrates")).not.toHaveAttribute("data-holds", "");
  await choose(page, "Sand");

  // Scrolling over the palette steps through the sub-categories, on into
  // the next tab. Over the tab row, which stays put while the panel below
  // it changes height.
  await page.locator("#tabs").hover();
  await page.mouse.wheel(0, 40);
  await expect(tab("Hardscape")).toHaveAttribute("aria-selected", "true");
  await page.mouse.wheel(0, 80);
  await expect(tab("Water")).toHaveAttribute("aria-selected", "true");
  await page.mouse.wheel(0, -40);
  await expect(tab("Devices")).toHaveAttribute("aria-selected", "true");
  // Focus on an item follows to the new sub-tab, so the keys keep working.
  await page.getByRole("radio", { name: "Bubbler" }).focus();
  await page.mouse.wheel(0, -40);
  await expect(tab("Hardscape")).toBeFocused();

  // G or H with a radio focused moves focus to the new tab or sub-tab (the
  // radio is hidden), and a reset keeps the open sub-category.
  await choose(page, "Sand");
  await page.keyboard.press("h");
  await expect(tab("Hardscape")).toBeFocused();
  await page.keyboard.press("g");
  await expect(tab("Water")).toBeFocused();
  await page.getByRole("button", { name: "Reset" }).click();
  await expect(tab("Water")).toHaveAttribute("aria-selected", "true");

  // P picks the selected plant's next propagule; without a plant it says
  // what it needs.
  await page.keyboard.press("p");
  await expect(page.locator("#inspect")).toHaveText(/Select a plant/);
  await choose(page, "Hygrophila");
  await page.keyboard.press("p");
  await expect(page.locator("#propagules").getByRole("radio", { name: "Fragments" })).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("p");
  await expect(page.locator("#propagules").getByRole("radio", { name: "Seeds" })).toHaveAttribute("aria-checked", "true");
});

test("settings persist across reloads", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  const autoScale = await page.evaluate(() => window.desktank.grid[0]);
  const scale = autoScale === 4 ? 2 : 4;
  await page.getByRole("button", { name: "Settings" }).click();
  await page.locator("#set-scale").selectOption(String(scale));
  await expect.poll(() => page.evaluate(() => window.desktank.grid[0])).toBe(scale);
  await page.locator("#set-fps").selectOption("15");
  await page.locator("#set-field").selectOption("flow");
  await page.getByLabel("Fish avoid the pointer").check();
  await page.getByLabel("Easy mode (only old age kills)").check();
  expect(await page.evaluate(() => window.desktank.tank.easyMode())).toBe(true);
  expect(await page.evaluate(() => window.desktank.tank.sway())).toBe(false);
  await page.getByLabel("Plant sway (10 fps, off on battery)").check();
  expect(await page.evaluate(() => window.desktank.tank.sway())).toBe(true);
  await page.locator("#set-stats").uncheck();
  await page.locator("#seed").fill("42");
  await page.locator("#seed").dispatchEvent("change");
  await page.locator("#brush").fill("7");
  await expect(page.locator("#stats")).toBeHidden();
  expect(await page.evaluate(() => window.desktank.tank.fieldOverlay())).toBe("flow");

  await page.reload();
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
  expect(await page.evaluate(() => window.desktank.settings)).toEqual({
    scale,
    fps: "15",
    field: "flow",
    stats: false,
    avoidCursor: true,
    easyMode: true,
    sway: true,
    seed: 42,
    radius: 7,
  });
  expect(await page.evaluate(() => window.desktank.grid[0]), "the tank uses the saved scale").toBe(scale);
  expect(await page.evaluate(() => window.desktank.tank.fieldOverlay())).toBe("flow");
  expect(await page.evaluate(() => window.desktank.tank.radius())).toBe(7);
  expect(await page.evaluate(() => window.desktank.tank.easyMode()), "easy mode applies to the new tank").toBe(true);
  expect(await page.evaluate(() => window.desktank.tank.sway()), "sway applies to the new tank").toBe(true);
  await expect(page.locator("#stats")).toBeHidden();
  await page.getByRole("button", { name: "Settings" }).click();
  await expect(page.locator("#set-scale")).toHaveValue(String(scale));
  await expect(page.locator("#seed")).toHaveValue("42");

  // Defaults restore auto scale and clear the overlay.
  await page.getByRole("button", { name: "Defaults" }).click();
  await expect.poll(() => page.evaluate(() => window.desktank.grid[0])).toBe(autoScale);
  expect(await page.evaluate(() => window.desktank.tank.fieldOverlay())).toBe("");
  expect(await page.evaluate(() => window.desktank.tank.easyMode())).toBe(false);
  expect(await page.evaluate(() => window.desktank.tank.sway())).toBe(false);
  // Garbage in storage falls back to the defaults.
  await page.evaluate(() => localStorage.setItem("desktank.settings.v1", "{not json"));
  await page.reload();
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
  expect(await page.evaluate(() => window.desktank.settings.scale)).toBe(0);
  expect(errors).toEqual([]);
});

test("creature cap setting and refused spawn messages", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  const [w, h] = await gridSize(page);
  const waterY = Math.round(h * 0.5);
  const stock = () => page.evaluate(() => window.desktank.tank.stockCount());

  await page.getByRole("button", { name: "Settings" }).click();
  const capInput = page.locator("#set-creature-cap");
  const capStock = page.locator("#set-creature-cap-stock");
  await expect(capInput).toHaveValue("60");
  await expect(capStock).toHaveText("(0/60)");

  // Deep inside a solid block reports no room.
  const [bx, by] = [Math.round(w * 0.15), Math.round(h * 0.8)];
  await choose(page, "Rock");
  await page.locator("#brush").fill("24");
  await page.locator("#brush").dispatchEvent("input");
  let blocked = await cellPoint(page, bx, by);
  await page.mouse.click(blocked.x, blocked.y);
  await choose(page, "Neon tetra");
  blocked = await cellPoint(page, bx, by);
  await page.mouse.click(blocked.x, blocked.y);
  await expect(page.locator("#inspect")).toHaveText(/No room for Neon tetra/);
  expect(await stock()).toBe(0);

  // Lowering the cap to 2 refuses a third creature with a clear message.
  await capInput.fill("2");
  await capInput.dispatchEvent("change");
  expect(await page.evaluate(() => window.desktank.tank.creatureCap())).toBe(2);
  await expect(capStock).toHaveText("(0/2)");

  for (const fx of [0.3, 0.5]) {
    const p = await cellPoint(page, Math.round(w * fx), waterY);
    await page.mouse.click(p.x, p.y);
  }
  expect(await stock()).toBe(2);
  await expect(capStock).toHaveText("(2/2)");

  const p3 = await cellPoint(page, Math.round(w * 0.7), waterY);
  await page.mouse.click(p3.x, p3.y);
  await expect(page.locator("#inspect")).toHaveText(/Tank full \(2\/2 creatures\)/);
  expect(await stock()).toBe(2);

  // Lowering below current stock clamps back and informs the user.
  await capInput.fill("1");
  await capInput.dispatchEvent("change");
  await expect(capInput).toHaveValue("2");
  await expect(page.locator("#inspect")).toHaveText(/Creature cap kept at 2/);

  // Raising the cap to 3 immediately allows spawning another creature.
  await capInput.fill("3");
  await capInput.dispatchEvent("change");
  await expect(capInput).toHaveValue("3");
  const p3After = await cellPoint(page, Math.round(w * 0.7), waterY);
  await page.mouse.click(p3After.x, p3After.y);
  expect(await stock()).toBe(3);
  await expect(capStock).toHaveText("(3/3)");

  // Lowering from above stock down below stock says "set to" rather than "kept at".
  await capInput.fill("10");
  await capInput.dispatchEvent("change");
  await capInput.fill("1");
  await capInput.dispatchEvent("change");
  await expect(capInput).toHaveValue("3");
  await expect(page.locator("#inspect")).toHaveText(/Creature cap set to 3/);

  // Live food has its own cap and reports when full.
  await choose(page, "Live bloodworms");
  const mid = await cellPoint(page, Math.round(w * 0.5), waterY);
  for (let i = 0; i < 7; i++) await page.mouse.click(mid.x, mid.y);
  await page.mouse.click(mid.x, mid.y);
  await expect(page.locator("#inspect")).toHaveText(/Live bloodworms limit reached \(32 max\)/);

  // Save (via visibilitychange) and reload: the tank's cap persists.
  await page.evaluate(async () => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
    await window.desktank.saving;
  });
  await page.reload();
  await expect(page.locator("body[data-ready]")).toBeAttached({ timeout: 30_000 });
  expect(await page.evaluate(() => window.desktank.tank.creatureCap())).toBe(3);
  expect(await stock()).toBe(3);
  await page.getByRole("button", { name: "Settings" }).click();
  await expect(capInput).toHaveValue("3");
  await expect(capStock).toHaveText("(3/3)");

  // Defaults resets the cap back to 60.
  await page.getByRole("button", { name: "Defaults" }).click();
  expect(await page.evaluate(() => window.desktank.tank.creatureCap())).toBe(60);
  await expect(capInput).toHaveValue("60");
  await expect(capStock).toHaveText("(3/60)");
  expect(errors).toEqual([]);
});

test("creature cap input is disabled and ignores edits on a viewer tab", async ({ context }) => {
  const ownerPage = await context.newPage();
  const errors = watchErrors(ownerPage);
  await open(ownerPage, "?backend=webgl2");

  const viewerPage = await context.newPage();
  errors.push(...watchErrors(viewerPage));
  await open(viewerPage, "?backend=webgl2");

  expect(await viewerPage.evaluate(() => window.desktank.owner)).toBe(false);
  const capInput = viewerPage.locator("#set-creature-cap");
  await expect(capInput).toBeDisabled();
  // Even a synthetic change event on a viewer tab is ignored and restored.
  await viewerPage.evaluate(() => {
    const input = document.querySelector<HTMLInputElement>("#set-creature-cap")!;
    input.value = "10";
    input.dispatchEvent(new Event("change"));
  });
  expect(await viewerPage.evaluate(() => window.desktank.tank.creatureCap())).toBe(60);
  await expect(capInput).toHaveValue("60");
  expect(errors).toEqual([]);
});

test("magnify tool zooms on hover, cycles on click, and adjusts via wheel, keys and selector", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  const [w, h] = await gridSize(page);
  const [cx, cy] = [Math.round(w * 0.5), Math.round(h * 0.5)];
  const zoom = () => page.evaluate(() => window.desktank.tank.zoom());
  const radius = () => page.evaluate(() => window.desktank.tank.radius());
  const frames = () => page.evaluate(() => window.desktank.tank.framesDrawn());
  const zoomSelect = page.locator("#zoom");

  // Pour a rock disc so the magnified view has a crisp feature to zoom in on.
  await choose(page, "Rock");
  const p = await cellPoint(page, cx, cy);
  await page.mouse.click(p.x, p.y);
  const hash = await stateHash(page);
  const initialRadius = await radius();

  // Select Magnify from the Tools tab: starts at 2×.
  await choose(page, "Magnify");
  expect(await page.evaluate(() => window.desktank.tank.isMagnify())).toBe(true);
  expect(await zoom()).toBe(2);
  await expect(zoomSelect).toHaveValue("2");

  // Capture canvas before hovering (no lens), then hover over the rock (lens visible).
  const beforeHover = await page.locator("#tank").screenshot();
  await page.mouse.move(p.x, p.y);
  const afterHover = await page.locator("#tank").screenshot();
  expect(Buffer.compare(beforeHover, afterHover)).not.toBe(0);

  // Keeping the cursor still on a paused tank draws no extra frames.
  await page.waitForTimeout(150);
  const settledFrames = await frames();
  await page.waitForTimeout(250);
  expect(await frames()).toBe(settledFrames);

  // Clicking cycles 2 → 4 → 6 → 8 → 2 without modifying the tank.
  for (const want of [4, 6, 8, 2]) {
    await page.mouse.click(p.x, p.y);
    expect(await zoom()).toBe(want);
    await expect(zoomSelect).toHaveValue(String(want));
  }
  expect(await stateHash(page)).toBe(hash);

  // Mouse wheel zooms without changing brush radius.
  await page.mouse.wheel(0, -80);
  await expect.poll(zoom).toBe(6);
  await expect(zoomSelect).toHaveValue("6");
  expect(await radius()).toBe(initialRadius);
  await page.mouse.wheel(0, 40);
  await expect.poll(zoom).toBe(4);

  // Keyboard controls step zoom.
  await page.keyboard.press("]");
  expect(await zoom()).toBe(6);
  await page.keyboard.press("+");
  expect(await zoom()).toBe(8);
  await page.keyboard.press("-");
  expect(await zoom()).toBe(6);
  await page.keyboard.press("[");
  expect(await zoom()).toBe(4);
  expect(await radius()).toBe(initialRadius);

  // Switching to another tool and picking a zoom level from #zoom re-selects Magnify.
  await choose(page, "Sand");
  expect(await page.evaluate(() => window.desktank.tank.isMagnify())).toBe(false);
  await zoomSelect.selectOption("8");
  expect(await page.evaluate(() => window.desktank.tank.isMagnify())).toBe(true);
  expect(await zoom()).toBe(8);
  expect(await stateHash(page)).toBe(hash);
  expect(errors).toEqual([]);
});

test("flip device button only appears for flippable device tools and X ignores other tools", async ({ page }) => {
  const errors = watchErrors(page);
  await openPaused(page);
  const flipBtn = page.locator("#flip");
  const canFlip = () => page.evaluate(() => window.desktank.tank.canFlipDevice());
  const flipped = () => page.evaluate(() => window.desktank.tank.deviceFlipped());

  // Hidden by default (Sand is selected) and X does not toggle flip state.
  await expect(flipBtn).toBeHidden();
  expect(await canFlip()).toBe(false);
  expect(await flipped()).toBe(false);
  await page.keyboard.press("x");
  await expect(flipBtn).toBeHidden();
  expect(await flipped()).toBe(false);

  // Stays hidden for non-device tools and non-flippable devices (Bubbler, Sponge filter).
  for (const name of [
    "Scoop",
    "Inspect",
    "Magnify",
    "Java moss",
    "Neon tetra",
    "Bubbler",
    "Sponge filter",
  ]) {
    await choose(page, name);
    await expect(flipBtn, `${name} should keep Flip device hidden`).toBeHidden();
    expect(await canFlip(), `${name} should not be flippable`).toBe(false);
    await page.keyboard.press("x");
    expect(await flipped(), `X while ${name} is selected should not flip`).toBe(false);
  }

  // Appears when Internal filter is selected; button and X toggle aria-pressed.
  await choose(page, "Internal filter");
  await expect(flipBtn).toBeVisible();
  await expect(flipBtn).toHaveAttribute("aria-pressed", "false");
  expect(await canFlip()).toBe(true);

  await flipBtn.click();
  await expect(flipBtn).toHaveAttribute("aria-pressed", "true");
  expect(await flipped()).toBe(true);

  // Repeated X keydown events are ignored.
  await page.evaluate(() => {
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "x", repeat: true, bubbles: true }));
  });
  await expect(flipBtn).toHaveAttribute("aria-pressed", "true");
  expect(await flipped()).toBe(true);

  // Clicking on the left half while flipped mounts the filter on the right glass.
  const [w, h] = await gridSize(page);
  const p = await cellPoint(page, Math.round(w * 0.2), Math.round(h * 0.5));
  await page.mouse.click(p.x, p.y);
  expect(await cellCount(page, "device")).toBeGreaterThan(300);
  await choose(page, "Inspect");
  await expect(flipBtn).toBeHidden();
  const rightSide = await cellPoint(page, w - 3, Math.round(h * 0.5));
  await page.mouse.click(rightSide.x, rightSide.y);
  await expect(page.locator("#inspect")).toContainText("on the right glass");

  // Pressing X while Inspect is selected does not change flip state.
  await page.keyboard.press("x");
  expect(await flipped()).toBe(true);

  // Returning to Internal filter shows the button with its pressed state intact,
  // and X toggles it back to unflipped.
  await choose(page, "Internal filter");
  await expect(flipBtn).toBeVisible();
  await expect(flipBtn).toHaveAttribute("aria-pressed", "true");
  await page.keyboard.press("x");
  await expect(flipBtn).toHaveAttribute("aria-pressed", "false");
  expect(await flipped()).toBe(false);

  // Hiding #flip while it has keyboard focus moves focus to the newly selected swatch.
  await flipBtn.focus();
  await expect(flipBtn).toBeFocused();
  await page.keyboard.press("1");
  await expect(flipBtn).toBeHidden();
  await expect(page.getByRole("radio", { name: "Bubbler" })).toBeFocused();

  // Switching back to Sand hides it again.
  await choose(page, "Sand");
  await expect(flipBtn).toBeHidden();
  expect(errors).toEqual([]);
});
