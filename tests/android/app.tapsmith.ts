// A3 smoke test: launch the tank, pour sand into the water, and back out.
import { mkdir, writeFile } from "node:fs/promises";
import { afterAll, beforeAll, expect, test } from "tapsmith";
import {
  ACTIVITY,
  PACKAGE,
  adb,
  decodePng,
  logcat,
  recordedFatals,
  shell,
  tankShares,
  waitForFirstFrame,
} from "./support.ts";

const OUT = "tapsmith-results/app";

beforeAll(async () => {
  // Tapsmith may already have launched the package during setup.
  await shell("am", "force-stop", PACKAGE);
  await adb("logcat", "-b", "all", "-c");
  await mkdir(OUT, { recursive: true });
});

afterAll(async () => {
  await shell("am", "force-stop", PACKAGE);
  const { crash, main } = await logcat();
  await writeFile(`${OUT}/logcat.txt`, main);
  await writeFile(`${OUT}/logcat-crash.txt`, crash);
});

test("the Activity draws, pours with touch and saves on Back", async ({ device }) => {
  const start = await shell("am", "start", "-W", "-n", ACTIVITY);
  expect(start).toContain("Status: ok");
  const { width, height } = await waitForFirstFrame("App", 120_000);

  await expect
    .poll(
      async () => {
        const res = await device.takeScreenshot();
        if (!res.success) throw new Error(`screenshot failed: ${res.errorMessage}`);
        await writeFile(`${OUT}/activity.png`, res.data);
        const { water, sand } = tankShares(decodePng(res.data));
        return water >= 0.5 && sand >= 0.1 ? "drawn" : `water ${water.toFixed(3)}, sand ${sand.toFixed(3)}`;
      },
      { timeout: 15_000, intervals: [500] },
    )
    .toBe("drawn");

  // Use the surface size for input, since screencap can be at panel resolution.
  const x = (fraction: number) => String(Math.round(width * fraction));
  const y = String(Math.round(height * 0.38));
  await shell("input", "touchscreen", "swipe", x(0.35), y, x(0.65), y, "350");
  await expect
    .poll(async () => (await logcat()).main, { timeout: 10_000 })
    .toMatch(/ I DeskTank\s*: touch: poured [1-9]\d* cells/);

  await shell("input", "keyevent", "BACK");
  await expect
    .poll(
      async () => {
        const { main } = await logcat();
        const stroke = main.lastIndexOf("touch: poured ");
        return stroke === -1 ? "" : main.slice(stroke);
      },
      { timeout: 15_000 },
    )
    .toMatch(/ I DeskTank\s*: (?:host stopped|active surface destroyed): .*\.dtank/);
  expect(await recordedFatals()).toEqual([]);
});
