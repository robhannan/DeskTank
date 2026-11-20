// A2 smoke test (SPEC §7, §9): start the Dream, check it draws, end it with a
// key, and fail on any crash of the app's process. Tapsmith installs the APK
// and drives the device; the Dream is a SurfaceView with no accessibility
// tree, so the checks read pixels and logcat rather than locators.
import { mkdir, writeFile } from "node:fs/promises";
import { afterAll, beforeAll, expect, test } from "tapsmith";
import {
  DREAM,
  DREAM_FULL,
  adb,
  appFatals,
  currentDream,
  decodePng,
  getSecure,
  putSecure,
  shareNear,
  shell,
} from "./support.ts";

// assets/materials/water.toml. With A0's skeleton the Dream clears the screen
// to it; once A1 draws the tank this becomes "the tank is drawn" (sand, rock
// and plants as well as water).
const WATER = [0x1f, 0x4e, 0x79] as const;
// Allows RGB565 (the emulator's GLES config quantises #1f4e79 to #214d7b).
const TOLERANCE = 4;
const OUT = "tapsmith-results/dream";

const SETTINGS = ["screensaver_enabled", "screensaver_components"] as const;
const saved = new Map<string, string | undefined>();

async function logcat(): Promise<{ crash: string; main: string }> {
  return {
    crash: await adb("logcat", "-d", "-b", "crash", "-v", "threadtime"),
    main: await adb("logcat", "-d", "-v", "threadtime"),
  };
}

async function expectNoFatals(): Promise<void> {
  const { crash, main } = await logcat();
  expect(appFatals(crash, main)).toEqual([]);
}

beforeAll(async () => {
  for (const name of SETTINGS) saved.set(name, await getSecure(name));
  await putSecure("screensaver_enabled", "1");
  await putSecure("screensaver_components", DREAM);
  await adb("logcat", "-b", "all", "-c");
  await mkdir(OUT, { recursive: true });
});

afterAll(async () => {
  // IDreamManager.awaken(), in case the test stopped with the Dream up.
  if ((await currentDream()) !== "") await shell("service", "call", "dreams", "2");
  for (const [name, value] of saved) await putSecure(name, value);
  const { crash, main } = await logcat();
  await writeFile(`${OUT}/logcat.txt`, main);
  await writeFile(`${OUT}/logcat-crash.txt`, crash);
});

test("the Dream starts, draws the tank and ends on a key", async ({ device }) => {
  // IDreamManager.dream(): the shell user may call it, unlike
  // `cmd dreams start-dreaming` (PROGRESS, A0).
  await shell("service", "call", "dreams", "1");
  await expect.poll(currentDream, { timeout: 30_000 }).toBe(DREAM_FULL);
  await expect
    .poll(async () => (await adb("logcat", "-d", "-s", "DeskTank")).includes("first frame"), {
      timeout: 30_000,
    })
    .toBe(true);

  // The Dream window fades in over what was on screen, so poll until the
  // frame settles. The last screenshot is kept for the artifacts.
  let shot: Uint8Array = new Uint8Array();
  await expect
    .poll(
      async () => {
        const res = await device.takeScreenshot();
        expect(res.errorMessage ?? "").toBe("");
        shot = res.data;
        return shareNear(decodePng(shot), WATER, TOLERANCE);
      },
      { timeout: 15_000, intervals: [500, 1000] },
    )
    .toBeGreaterThanOrEqual(0.95);
  await writeFile(`${OUT}/dreaming.png`, shot);
  await expectNoFatals();

  // Non-interactive, so any key ends it.
  await device.pressKey("DPAD_CENTER");
  await expect.poll(currentDream, { timeout: 15_000 }).toBe("");
  await expectNoFatals();
});
