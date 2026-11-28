#!/usr/bin/env node
// Runs the webOS build (target/site-webos/, `pnpm build:web:webos`) in a real
// Chromium 68, the engine of LG webOS 5's WebAppManager (SPEC D26):
//
//   node scripts/check-chrome68.ts <path to Chromium 68>
//
// Playwright can't drive a browser that old, so this speaks the DevTools
// protocol over a WebSocket itself. Headless, Chromium 68 draws WebGL2 with
// its 2018 SwiftShader, which caps a fragment shader at 4096 temporary
// registers: the one-pass grid shader exceeds it, as the TV's driver seems
// to, so the check catches a pass that grows past the limit. It checks that
// the page starts and draws with no GPU error, that the depth, rock varieties,
// and algae scenes match the native references like the render tests
// (at most 0.5% of pixels over CIEDE2000 ΔE 2), and that the one-pass
// shaders (?grid=single) fail with the error on screen, not a black tank.
// Screenshots go to target/shots/chrome68-*.png.
//
// CI uses the Linux snapshot r561732 (68.0.3440.0); on macOS, r561733's
// chrome-mac.zip runs headless under Rosetta. Both are at
// https://storage.googleapis.com/chromium-browser-snapshots/.
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { inflateSync } from "node:zlib";
import { compare } from "../tests/web/delta-e.ts";

const root = path.resolve(import.meta.dirname, "..");
const chrome = process.argv[2];
if (!chrome) {
  console.error("usage: node scripts/check-chrome68.ts <path to Chromium 68>");
  process.exit(2);
}
const shots = path.join(root, "target", "shots");
mkdirSync(shots, { recursive: true });
const PORT = 8168;
const DEBUG_PORT = 9268;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// A small DevTools protocol client: commands, and console lines collected.
class Cdp {
  private id = 0;
  readonly pending = new Map<number, (m: { result?: unknown; error?: { message: string } }) => void>();
  readonly log: string[] = [];
  private ws: WebSocket;
  private constructor(ws: WebSocket) {
    this.ws = ws;
    ws.addEventListener("message", (e) => {
      const m = JSON.parse(String(e.data));
      if (m.id !== undefined) this.pending.get(m.id)?.(m);
      else if (m.method === "Runtime.consoleAPICalled") {
        this.log.push(`console.${m.params.type}: ${m.params.args.map((a: { value?: unknown; description?: string }) => a.value ?? a.description).join(" ")}`);
      } else if (m.method === "Runtime.exceptionThrown") this.log.push(`exception: ${m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text}`);
    });
  }
  static async connect(url: string) {
    const ws = new WebSocket(url);
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve);
      ws.addEventListener("error", reject);
    });
    const cdp = new Cdp(ws);
    ws.addEventListener("close", () => {
      for (const reply of [...cdp.pending.values()]) reply({ error: { message: "the browser closed the connection" } });
    });
    await cdp.send("Runtime.enable");
    await cdp.send("Page.enable");
    return cdp;
  }
  // A command's result; fails after 60 s or if the browser goes away, so a
  // crashed browser can't hang the check.
  send(method: string, params: object = {}): Promise<Record<string, unknown>> {
    const id = ++this.id;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => this.pending.get(id)?.({ error: { message: "no reply in 60 s" } }), 60_000);
      this.pending.set(id, (m) => {
        clearTimeout(timer);
        this.pending.delete(id);
        if (m.error) reject(new Error(`${method}: ${m.error.message}`));
        else resolve(m.result as Record<string, unknown>);
      });
    });
  }
  // Evaluates `expression` (awaiting a promise) and returns its value.
  async eval<T>(expression: string): Promise<T> {
    const r = (await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })) as {
      result: { value: T };
      exceptionDetails?: { exception?: { description?: string }; text: string };
    };
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  }
  async until<T>(what: string, expression: string, ms: number): Promise<T> {
    const end = Date.now() + ms;
    for (;;) {
      const v = await this.eval<T>(expression);
      if (v) return v;
      if (Date.now() > end) throw new Error(`timed out after ${ms / 1000} s waiting for ${what}\n${this.log.join("\n")}`);
      await sleep(250);
    }
  }
  async screenshot(name: string, clip?: { width: number; height: number }) {
    const r = await this.send("Page.captureScreenshot", { format: "png", ...(clip && { clip: { x: 0, y: 0, ...clip, scale: 1 } }) });
    const png = Buffer.from(r.data as string, "base64");
    writeFileSync(path.join(shots, `chrome68-${name}.png`), png);
    return png;
  }
}

// 8-bit RGB or RGBA, non-interlaced PNGs (screenshots and the references).
function decodePng(png: Buffer) {
  let at = 8;
  let width = 0;
  let height = 0;
  let channels = 0;
  const data: Buffer[] = [];
  while (at < png.length) {
    const len = png.readUInt32BE(at);
    const type = png.toString("ascii", at + 4, at + 8);
    const body = png.subarray(at + 8, at + 8 + len);
    if (type === "IHDR") {
      [width, height] = [body.readUInt32BE(0), body.readUInt32BE(4)];
      channels = { 2: 3, 6: 4 }[body[9]] ?? 0;
      if (body[8] !== 8 || !channels || body[12]) throw new Error("unsupported PNG");
    } else if (type === "IDAT") data.push(body);
    at += 12 + len;
  }
  const raw = inflateSync(Buffer.concat(data));
  const stride = width * channels;
  const rgba = new Uint8Array(width * height * 4);
  let prev = new Uint8Array(stride);
  for (let y = 0; y < height; y++) {
    const row = new Uint8Array(stride);
    const filter = raw[y * (stride + 1)];
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? row[x - channels] : 0;
      const b = prev[x];
      const c = x >= channels ? prev[x - channels] : 0;
      const p = a + b - c;
      const paeth = Math.abs(p - a) <= Math.abs(p - b) && Math.abs(p - a) <= Math.abs(p - c) ? a : Math.abs(p - b) <= Math.abs(p - c) ? b : c;
      const predictor = [0, a, b, (a + b) >> 1, paeth][filter];
      row[x] = raw[y * (stride + 1) + 1 + x] + predictor;
    }
    for (let x = 0; x < width; x++) {
      rgba.set([row[x * channels], row[x * channels + 1], row[x * channels + 2], 255], (y * width + x) * 4);
    }
    prev = row;
  }
  return { width, height, rgba };
}

async function openPage(cdp: Cdp, query: string) {
  cdp.log.length = 0;
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 720, deviceScaleFactor: 1, mobile: false });
  await cdp.send("Page.navigate", { url: `http://127.0.0.1:${PORT}/webos/${query}` });
  await cdp.until("the page to start", "document.body && document.body.dataset.ready === 'true'", 60_000);
}

// The status overlay's text if shown (errors, stalls), else "".
const statusText = "(function (s) { return s.hidden ? '' : s.textContent; })(document.getElementById('status'))";

const server = spawn(process.execPath, [path.join(root, "scripts", "serve-web.ts"), String(PORT)], { stdio: "ignore" });
const profile = mkdtempSync(path.join(tmpdir(), "desktank-chrome68-"));
const browser = spawn(
  chrome,
  ["--headless", "--no-sandbox", "--no-first-run", "--use-gl=swiftshader", `--remote-debugging-port=${DEBUG_PORT}`, `--user-data-dir=${profile}`, "about:blank"],
  { stdio: "ignore" },
);
let failed = false;
try {
  let target: string | undefined;
  for (let i = 0; i < 120 && !target; i++) {
    await sleep(500);
    try {
      const list = (await (await fetch(`http://127.0.0.1:${DEBUG_PORT}/json`)).json()) as { type: string; webSocketDebuggerUrl: string }[];
      target = list.find((t) => t.type === "page")?.webSocketDebuggerUrl;
    } catch {
      // Not listening yet.
    }
  }
  if (!target) throw new Error("Chromium 68 didn't open its DevTools port");
  for (let i = 0; ; i++) {
    try {
      if ((await fetch(`http://127.0.0.1:${PORT}/webos/`)).ok) break;
    } catch {
      // Not listening yet.
    }
    if (i > 60) throw new Error("the web server didn't start (is target/site-webos/ built?)");
    await sleep(250);
  }
  const cdp = await Cdp.connect(target);
  // The fixed port could be another browser's: make sure it is ours.
  const agent = await cdp.eval<string>("navigator.userAgent");
  console.log(`browser: ${agent}`);
  if (!/Chrome\/68\./.test(agent)) throw new Error("the DevTools port isn't Chromium 68's");

  // 1. The webOS build starts and draws with the split grid, no GPU error.
  await openPage(cdp, "?debug=1");
  await cdp.until("the first frame", "window.desktank.startup.some(function (s) { return s.indexOf('first frame drawn') >= 0; })", 120_000);
  await cdp.screenshot("startup");
  const startup = await cdp.eval<string[]>("window.desktank.startup");
  console.log(startup.map((s) => `  ${s}`).join("\n"));
  const error = await cdp.eval<string>(statusText);
  if (error) throw new Error(`the page shows an error: ${error}`);

  // 2. The depth, hardscape and plant scenes match the native captures.
  // The algae passes draw only while algae shows, so SwiftShader compiles
  // them (and can run out of memory) only then; the varieties scene draws
  // every look from the baked atlas.
  // Out of the way of the scene's pixels (it sits over the canvas).
  await cdp.eval("document.getElementById('debug').style.display = 'none'");
  for (const [scene, reference] of [
    ["depth", "depth_x4"],
    ["rock_varieties", "rock_varieties_x4"],
    ["algae_film", "algae_film_x4"],
    ["algae_varieties", "algae_varieties_x4"],
    ["plant_epiphytes", "plant_epiphytes_x4"],
  ]) {
    const toml = readFileSync(path.join(root, "crates", "desktank", "tests", "scenes", `${scene}.toml`), "utf8");
    await cdp.eval(`window.desktank.loadScene(${JSON.stringify(toml)}, 7, 640, 360)`);
    const got = decodePng(await cdp.screenshot(reference, { width: 640, height: 360 }));
    const want = decodePng(readFileSync(path.join(root, "crates", "desktank", "tests", "reference", `${reference}.png`)));
    const diff = compare(got.rgba, want.rgba, 2);
    const share = diff.over / diff.total;
    console.log(`${reference}: ${diff.over} of ${diff.total} pixels over ΔE 2 (${(share * 100).toFixed(3)}%), max ΔE ${diff.max.toFixed(2)}`);
    const sceneError = await cdp.eval<string>(statusText);
    if (sceneError) throw new Error(`the page shows an error in ${scene}: ${sceneError}`);
    if (share > 0.005) throw new Error(`the ${scene} scene doesn't match the native reference`);
  }

  // 3. The one-pass shaders fail here, and say so on screen.
  await openPage(cdp, "?grid=single");
  const shown = await cdp.until<string>("the GPU error on screen", statusText, 120_000);
  await cdp.screenshot("grid-single");
  console.log(`?grid=single shows: ${shown.split("\n").slice(0, 5).join(" | ")}`);
  if (!shown.includes("GPU renderer failed")) throw new Error(`unexpected status: ${shown}`);
  console.log("Chromium 68 check passed.");
} catch (err) {
  failed = true;
  console.error(String(err instanceof Error ? (err.stack ?? err.message) : err));
} finally {
  browser.kill();
  server.kill();
  await sleep(500);
  rmSync(profile, { recursive: true, force: true });
}
process.exit(failed ? 1 : 0);
