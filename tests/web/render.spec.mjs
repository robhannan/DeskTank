// WB1 acceptance: on WebGL2, a seeded scripted scene matches the native
// `desktank capture` reference of the same scene within the SPEC §7
// tolerance (≤ 0.5% of pixels over CIEDE2000 ΔE 2), as the native render
// tests check it (crates/desktank/tests/render.rs).
import { readFile } from "node:fs/promises";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { compare } from "./delta-e.mjs";
import { decodePng, open, watchErrors } from "./support.mjs";

const crate = path.resolve(import.meta.dirname, "..", "..", "crates", "desktank", "tests");
const MAX_DELTA_E = 2;
const MAX_FRACTION = 0.005;

test("layers scene on WebGL2 matches the native capture", async ({ page }) => {
  const errors = watchErrors(page);
  await open(page, "?backend=webgl2");
  const toml = await readFile(path.join(crate, "scenes", "layers.toml"), "utf8");
  // Same seed and size as the render tests: 160×90 cells at scale 2.
  await page.evaluate((src) => window.desktank.loadScene(src, 7, 320, 180), toml);

  const got = await decodePng(page, await page.locator("#tank").screenshot());
  const want = await decodePng(page, await readFile(path.join(crate, "reference", "layers.png")));
  expect([got.width, got.height]).toEqual([want.width, want.height]);
  const diff = compare(got.rgba, want.rgba, MAX_DELTA_E);
  const pct = ((diff.over / diff.total) * 100).toFixed(3);
  console.log(`layers on webgl2: ${diff.over} of ${diff.total} pixels over ΔE ${MAX_DELTA_E} (${pct}%), max ΔE ${diff.max.toFixed(2)}`);
  test.info().annotations.push({ type: "diff", description: `${pct}% over ΔE 2, max ${diff.max.toFixed(2)}` });
  expect(diff.over / diff.total).toBeLessThanOrEqual(MAX_FRACTION);
  expect(errors).toEqual([]);
});
