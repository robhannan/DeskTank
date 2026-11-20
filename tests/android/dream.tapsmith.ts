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
  tankShares,
  shell,
} from "./support.ts";

const OUT = "tapsmith-results/dream";

const SETTINGS = ["screensaver_enabled", "screensaver_components"] as const;
const saved = new Map<string, string | undefined>();

async function logcat(): Promise<{ crash: string; main: string }> {
  return {
    crash: await adb("logcat", "-d", "-b", "crash", "-v", "threadtime"),
    main: await adb("logcat", "-d", "-v", "threadtime"),
  };
}

async function waitForFirstFrame(timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const { crash, main } = await logcat();
    const fatals = appFatals(crash, main);
    if (fatals.length > 0) throw new Error(`the app crashed before its first frame:\n${fatals.join("\n")}`);
    if (/ I DeskTank\s*: first frame /.test(main)) return;
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  throw new Error(`no first frame within ${timeoutMs / 1000} s`);
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
  try {
    // IDreamManager.awaken(), in case the test stopped with the Dream up.
    if ((await currentDream()) !== "") await shell("service", "call", "dreams", "2");
  } finally {
    for (const [name, value] of saved) await putSecure(name, value);
  }
  const { crash, main } = await logcat();
  await writeFile(`${OUT}/logcat.txt`, main);
  await writeFile(`${OUT}/logcat-crash.txt`, crash);
});

test("the Dream starts, draws the tank and ends on a key", async ({ device }) => {
  // IDreamManager.dream(): the shell user may call it, unlike
  // `cmd dreams start-dreaming` (PROGRESS, A0).
  await shell("service", "call", "dreams", "1");
  await expect.poll(currentDream, { timeout: 30_000 }).toBe(DREAM_FULL);
  // The first draw compiles the pipelines (seconds on a device). Fail at
  // once, with the lines, if the app crashes meanwhile.
  await waitForFirstFrame(120_000);

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
  await expectNoFatals();

  // Still up (not ended on its own); non-interactive, so any key ends it.
  expect(await currentDream()).toBe(DREAM_FULL);
  await device.pressKey("DPAD_CENTER");
  await expect.poll(currentDream, { timeout: 15_000 }).toBe("");
  await expectNoFatals();
});
