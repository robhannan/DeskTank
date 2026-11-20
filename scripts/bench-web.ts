#!/usr/bin/env node
// Frame-interval benchmark for the web version (SPEC §9 WB1): loads the
// built site in a real browser window at DPR 2, pours continuously for N
// seconds, and prints the p50/p95/max interval between loop callbacks and
// between drawn frames, from performance.now(). Since WB2 the loop runs on
// rAF only while a frame is due or input arrives and otherwise sleeps on a
// timer until the next 20 Hz tick, so loop intervals are no longer vsync.
//
//   pnpm build:web && pnpm bench:web [chrome|chromium|firefox|webkit] [seconds]
import { type Browser, chromium, firefox, webkit } from "@playwright/test";
import { spawn } from "node:child_process";
import path from "node:path";

const which = process.argv[2] ?? "chrome";
const seconds = Number(process.argv[3] ?? 60);
const PORT = 8124;
const launchers: Record<string, () => Promise<Browser>> = {
  chrome: () => chromium.launch({ channel: "chrome", headless: false }),
  chromium: () => chromium.launch({ headless: false }),
  firefox: () => firefox.launch({ headless: false }),
  webkit: () => webkit.launch({ headless: false }),
};
const launch = launchers[which];
if (!launch) throw new Error(`unknown browser ${which}`);

const server = spawn(process.execPath, [path.join(import.meta.dirname, "serve-web.ts"), String(PORT)], {
  stdio: "ignore",
});
await new Promise((r) => setTimeout(r, 500));
const browser = await launch();
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, deviceScaleFactor: 2 });
  page.on("console", (m) => m.type() === "error" && console.error(`console: ${m.text()}`));
  await page.goto(`http://127.0.0.1:${PORT}/?bench`);
  await page.waitForSelector("body[data-ready]", { state: "attached", timeout: 30_000 });
  const info = await page.evaluate(() => ({
    backend: window.desktank.backend,
    dpr: devicePixelRatio,
    canvas: ((c) => [c.width, c.height])(document.querySelector<HTMLCanvasElement>("#tank")!),
    grid: [window.desktank.tank.width(), window.desktank.tank.height()],
    ua: navigator.userAgent,
  }));
  const box = await page.locator("#tank").boundingBox();
  if (!box) throw new Error("the canvas isn't visible");
  await page.evaluate(() => {
    window.desktank.rafTimes.length = 0;
    window.desktank.frameTimes.length = 0;
  });
  // Sweep the brush back and forth near the surface for the whole run.
  await page.mouse.move(box.x + box.width * 0.1, box.y + box.height * 0.15);
  await page.mouse.down();
  const end = Date.now() + seconds * 1000;
  for (let i = 0; Date.now() < end; i++) {
    const t = (Math.sin(i / 20) + 1) / 2;
    await page.mouse.move(box.x + box.width * (0.1 + 0.8 * t), box.y + box.height * 0.15);
    await page.waitForTimeout(50);
  }
  await page.mouse.up();
  const { raf, drawn } = await page.evaluate(() => ({
    raf: window.desktank.rafTimes,
    drawn: window.desktank.frameTimes,
  }));
  const stats = (times: number[]) => {
    const d = times.slice(1).map((t, i) => t - times[i]).sort((a, b) => a - b);
    const q = (p: number) => d[Math.min(d.length - 1, Math.floor(p * d.length))]?.toFixed(1);
    return `n=${d.length} p50 ${q(0.5)} ms, p95 ${q(0.95)} ms, max ${d.at(-1)?.toFixed(1)} ms`;
  };
  console.log(`${which} ${browser.version()} (${info.backend}), DPR ${info.dpr}, canvas ${info.canvas.join("×")}, grid ${info.grid.join("×")}, ${seconds} s pouring`);
  console.log(`  loop interval:  ${stats(raf)}`);
  console.log(`  drawn interval: ${stats(drawn)}`);
} finally {
  await browser.close();
  server.kill();
}
