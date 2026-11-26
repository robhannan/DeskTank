// WB1 acceptance: on WebGL2, a seeded scripted scene matches the native
// `desktank capture` reference of the same scene within the SPEC §7
// tolerance (≤ 0.5% of pixels over CIEDE2000 ΔE 2), as the native render
// tests check it (crates/desktank/tests/render.rs).
import { readFile } from "node:fs/promises";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { compare } from "./delta-e.ts";
import { decodePng, open, watchErrors } from "./support.ts";

const crate = path.resolve(import.meta.dirname, "..", "..", "crates", "desktank", "tests");
const MAX_DELTA_E = 2;
const MAX_FRACTION = 0.005;

// Match the native render tests: seed 7 and 160×90 cells.
for (const { scene, reference, scale } of [
  { scene: "layers", reference: "layers", scale: 2 },
  { scene: "algae_film", reference: "algae_film_x4", scale: 4 },
]) {
  test(`${scene} scene on WebGL2 matches the native capture`, async ({ page }) => {
    const errors = watchErrors(page);
    await open(page, "?backend=webgl2");
    const toml = await readFile(path.join(crate, "scenes", `${scene}.toml`), "utf8");
    await page.evaluate(({ src, scale }) => window.desktank.loadScene(src, 7, 160 * scale, 90 * scale), {
      src: toml,
      scale,
    });

    const got = await decodePng(page, await page.locator("#tank").screenshot());
    const want = await decodePng(page, await readFile(path.join(crate, "reference", `${reference}.png`)));
    expect([got.width, got.height]).toEqual([want.width, want.height]);
    const diff = compare(got.rgba, want.rgba, MAX_DELTA_E);
    const pct = ((diff.over / diff.total) * 100).toFixed(3);
    console.log(`${scene} on webgl2: ${diff.over} of ${diff.total} pixels over ΔE ${MAX_DELTA_E} (${pct}%), max ΔE ${diff.max.toFixed(2)}`);
    test.info().annotations.push({ type: "diff", description: `${pct}% over ΔE 2, max ${diff.max.toFixed(2)}` });
    expect(diff.over / diff.total).toBeLessThanOrEqual(MAX_FRACTION);
    expect(errors).toEqual([]);
  });
}
