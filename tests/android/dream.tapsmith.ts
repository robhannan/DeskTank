// A2 smoke test (SPEC §7, §9): start the Dream, check it draws, end it with a
// key, and fail on any crash of the app's process. Tapsmith installs the APK
// and drives the device; the Dream is a SurfaceView with no accessibility
// tree, so the checks read pixels and logcat rather than locators.
import { mkdir, writeFile } from "node:fs/promises";
import { afterAll, beforeAll, expect, test } from "tapsmith";
import {
  DREAM,
  DREAM_FULL,
  PACKAGE,
  currentDream,
  decodePng,
  getSecure,
  logcat,
  logcatTime,
  putSecure,
  recordedFatals,
  tankShares,
  tourShares,
  shell,
  waitForFirstFrame,
} from "./support.ts";

const OUT = "tapsmith-results/dream";
let logStart: string;

const SETTINGS = ["screensaver_enabled", "screensaver_components"] as const;
const saved = new Map<string, string | undefined>();
const PREFS = "shared_prefs/dream.xml";
let savedPrefs: string | undefined;
let changedPrefs = false;

// The debug APK lets adb write SharedPreferences directly. Stop the process
// first so the next Dream reads from disk and no cached editor can race us.
async function prefsCommand(command: string): Promise<string> {
  return shell("run-as", PACKAGE, "sh", "-c", `'${command}'`);
}

async function writePrefs(xml: string): Promise<void> {
  const encoded = Buffer.from(xml).toString("base64");
  await prefsCommand(`mkdir -p shared_prefs && echo ${encoded} | base64 -d > ${PREFS}`);
}

async function setTour(enabled: boolean): Promise<void> {
  await shell("am", "force-stop", PACKAGE);
  changedPrefs = true;
  await writePrefs(`<map><boolean name="tour" value="${enabled}" /></map>`);
}

beforeAll(async () => {
  await mkdir(OUT, { recursive: true });
  logStart = await logcatTime();
  for (const name of SETTINGS) saved.set(name, await getSecure(name));
  savedPrefs = (await prefsCommand(`if [ -f ${PREFS} ]; then cat ${PREFS}; fi`)) || undefined;
  await putSecure("screensaver_enabled", "1");
  await putSecure("screensaver_components", DREAM);
});

afterAll(async () => {
  try {
    // IDreamManager.awaken(), in case the test stopped with the Dream up.
    if ((await currentDream()) !== "") await shell("service", "call", "dreams", "2");
    if (changedPrefs) {
      await shell("am", "force-stop", PACKAGE);
      if (savedPrefs === undefined) await prefsCommand(`rm -f ${PREFS}`);
      else await writePrefs(savedPrefs);
    }
  } finally {
    for (const [name, value] of saved) await putSecure(name, value);
  }
  const { crash, main } = await logcat(logStart);
  await writeFile(`${OUT}/logcat.txt`, main);
  await writeFile(`${OUT}/logcat-crash.txt`, crash);
});

test("the Dream starts, draws the full tank and ends on a key", async ({ device }) => {
  await setTour(false);
  // IDreamManager.dream(): the shell user may call it, unlike
  // `cmd dreams start-dreaming` (PROGRESS, A0).
  await shell("service", "call", "dreams", "1");
  await expect.poll(currentDream, { timeout: 30_000 }).toBe(DREAM_FULL);
  // The first draw compiles the pipelines (seconds on a device). Fail at
  // once, with the lines, if the app crashes meanwhile.
  expect((await waitForFirstFrame("Dream", logStart, 120_000)).camera).toBe(false);

  // The tank is drawn: mostly water, with sand along the bottom. The Dream
  // window fades in over what was on screen, so poll until the frame
  // settles; each screenshot is saved, so the artifact is the last one.
  await expect
    .poll(
      async () => {
        const res = await device.takeScreenshot();
        if (!res.success) throw new Error(`screenshot failed: ${res.errorMessage}`);
        await writeFile(`${OUT}/dreaming.png`, res.data);
        const { water, sand } = tankShares(decodePng(res.data));
        return water >= 0.5 && sand >= 0.1 ? "drawn" : `water ${water.toFixed(3)}, sand ${sand.toFixed(3)}`;
      },
      { timeout: 30_000, intervals: [1000] },
    )
    .toBe("drawn");
  expect(await recordedFatals(logStart)).toEqual([]);

  // Still up (not ended on its own); non-interactive, so any key ends it.
  expect(await currentDream()).toBe(DREAM_FULL);
  await device.pressKey("DPAD_CENTER");
  await expect.poll(currentDream, { timeout: 15_000 }).toBe("");
  expect(await recordedFatals(logStart)).toEqual([]);
});

test("the touring Dream draws across the screen without missing scanlines", async ({ device }) => {
  await setTour(true);
  const started = await logcatTime();
  await shell("service", "call", "dreams", "1");
  await expect.poll(currentDream, { timeout: 30_000 }).toBe(DREAM_FULL);
  expect((await waitForFirstFrame("Dream", started, 120_000)).camera).toBe(true);

  await expect
    .poll(
      async () => {
        const res = await device.takeScreenshot();
        if (!res.success) throw new Error(`screenshot failed: ${res.errorMessage}`);
        await writeFile(`${OUT}/touring.png`, res.data);
        const { water, drawnRows } = tourShares(decodePng(res.data));
        return water >= 0.35 && drawnRows >= 0.9
          ? "drawn"
          : `water ${water.toFixed(3)}, drawn rows ${drawnRows.toFixed(3)}`;
      },
      { timeout: 30_000, intervals: [1000] },
    )
    .toBe("drawn");

  expect(await currentDream()).toBe(DREAM_FULL);
  expect(await recordedFatals(started)).toEqual([]);
});
