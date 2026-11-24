// A3 smoke test: launch the tank, pour sand into the water, and back out.
import { mkdir, writeFile } from "node:fs/promises";
import { afterAll, beforeAll, expect, test } from "tapsmith";
import {
  ACTIVITY,
  PACKAGE,
  adb,
  decodePng,
  logcat,
  logcatTime,
  recordedFatals,
  shell,
  tankShares,
  waitForFirstFrame,
} from "./support.ts";

const OUT = "tapsmith-results/app";
let logStart: string;

beforeAll(async () => {
  await mkdir(OUT, { recursive: true });
  logStart = await logcatTime();
  // Start with a fresh tank, since this test paints into the shared save.
  expect(await shell("pm", "clear", PACKAGE)).toContain("Success");
});

afterAll(async () => {
  await shell("am", "force-stop", PACKAGE);
  const { crash, main } = await logcat(logStart);
  await writeFile(`${OUT}/logcat.txt`, main);
  await writeFile(`${OUT}/logcat-crash.txt`, crash);
});

test("the Activity draws, pours with touch and saves on Back", async ({ device }) => {
  const start = await shell(
    "am", "start", "-W", "-a", "android.intent.action.MAIN",
    "-c", "android.intent.category.LAUNCHER", "-n", ACTIVITY,
  );
  expect(start).toContain("Status: ok");
  const { width, height } = await waitForFirstFrame("App", logStart, 120_000);

  try {
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
  } catch (error) {
    // The first-frame log means the present call returned. Record which
    // buffers SurfaceFlinger actually latched before force-stopping the app.
    await Promise.allSettled([
      shell("dumpsys", "-t", "5", "SurfaceFlinger").then((dump) =>
        writeFile(`${OUT}/surfaceflinger.txt`, dump),
      ),
      adb("logcat", "-b", "events", "-d", "-T", logStart, "-v", "threadtime").then((events) =>
        writeFile(`${OUT}/events.txt`, events),
      ),
    ]);
    throw error;
  }

  // Use the surface size for input, since screencap can be at panel resolution.
  const x = (fraction: number) => String(Math.round(width * fraction));
  const y = String(Math.round(height * 0.38));
  await shell("input", "touchscreen", "swipe", x(0.35), y, x(0.65), y, "350");
  await expect
    .poll(async () => (await logcat(logStart)).main, { timeout: 10_000 })
    .toMatch(/ I DeskTank\s*: touch: poured [1-9]\d* cells/);

  await shell("input", "keyevent", "BACK");
  await expect
    .poll(
      async () => {
        const { main } = await logcat(logStart);
        const stroke = main.lastIndexOf("touch: poured ");
        return stroke === -1 ? "" : main.slice(stroke);
      },
      { timeout: 15_000 },
    )
    .toMatch(/ I DeskTank\s*: (?:host stopped|active surface destroyed): .*\.dtank/);
  expect(await recordedFatals(logStart)).toEqual([]);
});
