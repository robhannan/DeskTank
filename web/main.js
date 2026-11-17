// DeskTank web host: drives crates/desktank-web, which runs desktank-render's
// Tank through wgpu (WebGPU, or WebGL2 as the fallback). SPEC §3.5.
//
// Pacing lives in the tank: a fixed 20 Hz simulation tick, and frames drawn
// only when something changed, capped at 30 fps (15 on battery). This file
// owns the rAF loop, input, visibility, battery and device/context loss.
//
// URL parameters: ?backend=webgpu|webgl2 forces a backend; ?bench records
// drawn-frame times in window.desktank.frameTimes.
import init, { Graphics, WebTank, gridFor } from "./pkg/desktank_web.js";

const FPS_AC = 30;
const FPS_BATTERY = 15;
// Flags returned by WebTank.frame (crates/desktank-web).
const DREW = 1;
const WANTED = 2;
const LOST = 4;
const TICKS_SHIFT = 8;

// Tools in palette order: label, material name (null = erase), swatch colour.
const TOOLS = [
  ["Sand", "sand", "#e0c992"],
  ["Gravel", "gravel", "#857b6e"],
  ["Rock", "rock", "#5c5853"],
  ["Water", "water", "#2a6496"],
  ["Bubbles", "air_bubble", "#eaf7ff"],
  ["Food", "flake_food", "#e0673f"],
  ["Erase", null, "repeating-conic-gradient(#16283a 0 25%, #8fa8bc 0 50%) 0 0 / 8px 8px"],
];

const params = new URLSearchParams(location.search);
const wantBackend = params.get("backend") ?? "auto";
const bench = params.has("bench");

const $ = (id) => document.getElementById(id);
const canvas = $("tank");
const statusEl = $("status");
const statsEl = $("stats");
const playBtn = $("play");
const stepBtn = $("step");
const seedInput = $("seed");
const brushInput = $("brush");

let tank;
let grid; // [scale, columns, rows], fixed when the page first sizes the canvas
let backend = "";
let adapter = "";
let tool = TOOLS[0];
let pointer = null; // { x, y, erase, id } in cells while a drag is active
let rafId = 0;
let hidden = document.hidden;
let maxFps = FPS_AC;
let restoring = false;
let tickMs = 0;
let frames = 0;
let fps = 0;
let fpsSince = 0;

// Test and benchmark hooks (tests/web/).
const hooks = {
  frameTimes: [],
  get tank() {
    return tank;
  },
  get backend() {
    return backend;
  },
  loadScene,
};
window.desktank = hooks;

function showError(message) {
  statusEl.textContent = message;
  statusEl.hidden = false;
}

// Canvas size in device pixels, from its CSS box.
function canvasPixels() {
  const r = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  return [Math.max(1, Math.round(r.width * dpr)), Math.max(1, Math.round(r.height * dpr))];
}

function setCanvasSize([w, h]) {
  if (canvas.width !== w) canvas.width = w;
  if (canvas.height !== h) canvas.height = h;
  tank?.resize(w, h);
  kick();
}

function seed() {
  const s = Math.max(0, Math.min(0xffffffff, Math.floor(Number(seedInput.value) || 0)));
  seedInput.value = s;
  return s;
}

// Swaps in a new tank; the caller attaches graphics to it.
function replaceTank(next) {
  tank?.free();
  tank = next;
  tank.setMaxFps(maxFps);
  setRunning(tank.running());
}

async function attachGraphics() {
  const g = await Graphics.open(canvas, backend || wantBackend);
  backend = g.backend();
  adapter = g.adapterName();
  tank.attach(g);
  canvas.dataset.backend = backend;
}

async function reset() {
  const [w, h] = [canvas.width, canvas.height];
  // Graphics belong to the old tank; reopen them for the new one.
  replaceTank(new WebTank(grid[1], grid[2], seed(), w, h));
  await attachGraphics();
  updateStats();
  kick();
}

// Loads a `desktank capture` script (TOML) at a fixed canvas size, paused,
// for the render comparison test.
async function loadScene(toml, sceneSeed, width, height) {
  // Pinned to the viewport's top-left at whole pixels, so a screenshot of
  // it is exactly the drawn frame.
  const dpr = window.devicePixelRatio || 1;
  Object.assign(canvas.style, {
    position: "fixed",
    left: "0",
    top: "0",
    zIndex: "10",
    width: `${width / dpr}px`,
    height: `${height / dpr}px`,
    aspectRatio: "auto",
  });
  canvas.width = width;
  canvas.height = height;
  replaceTank(WebTank.fromScript(toml, sceneSeed, width, height));
  await attachGraphics();
  kick();
  const drawn = tank.framesDrawn();
  while (tank.framesDrawn() === drawn) {
    await new Promise((r) => requestAnimationFrame(r));
  }
}

function brushRadius() {
  return Number(brushInput.value);
}

function material(erase) {
  return erase || tool[1] === null ? undefined : tool[1];
}

function stamp(x, y, erase) {
  const m = material(erase);
  if (m === undefined) tank.erase(x, y, brushRadius());
  else tank.pour(m, x, y, brushRadius());
}

// Stamps along the segment from (x0, y0) to (x1, y1) so fast drags leave
// no gaps.
function stroke(x0, y0, x1, y1, erase) {
  const steps = Math.max(1, Math.ceil(Math.hypot(x1 - x0, y1 - y0) / Math.max(1, brushRadius())));
  for (let i = 1; i <= steps; i++) {
    stamp(Math.round(x0 + ((x1 - x0) * i) / steps), Math.round(y0 + ((y1 - y0) * i) / steps), erase);
  }
}

// Holding still keeps pouring every tick, like a tap.
function holdBrush() {
  if (pointer) tank.hold(material(pointer.erase), pointer.x, pointer.y, brushRadius());
  else tank.release();
}

function updateStats() {
  statsEl.textContent =
    `${backend || "no GPU"} · fps ${fps} · tick ${tank.tickCount()} · awake chunks ` +
    `${tank.awakeChunks()}/${tank.chunkCount()} · tick time ${tickMs.toFixed(2)} ms` +
    ` · ${grid[1]}×${grid[2]} cells at ${grid[0]}×`;
}

// One rAF callback: the tank decides whether a tick or a frame is due.
function loop(now) {
  rafId = 0;
  if (hidden || !tank) return;
  const t0 = performance.now();
  const flags = tank.frame(now);
  if (flags >> TICKS_SHIFT) tickMs = performance.now() - t0;
  if (flags & DREW) {
    frames++;
    if (bench) hooks.frameTimes.push(now);
  }
  if (flags & LOST) {
    recover();
    return;
  }
  if (now - fpsSince >= 1000) {
    fps = Math.round((frames * 1000) / (now - fpsSince));
    frames = 0;
    fpsSince = now;
    updateStats();
  }
  // Paused with nothing to draw and no drag: sleep until input wakes us.
  if (tank.running() || pointer || flags & WANTED) kick();
  else updateStats();
}

function kick() {
  if (!rafId && !hidden && !restoring) rafId = requestAnimationFrame(loop);
}

// Device or context loss: drop the graphics and reopen them; the world is
// unaffected.
async function recover() {
  if (restoring) return;
  restoring = true;
  tank.detach();
  try {
    await attachGraphics();
  } catch (err) {
    showError(`Lost the GPU and couldn't get it back: ${err}`);
    return;
  } finally {
    restoring = false;
  }
  kick();
}

function cellAt(event) {
  const rect = canvas.getBoundingClientRect();
  const px = ((event.clientX - rect.left) / rect.width) * canvas.width;
  const py = ((event.clientY - rect.top) / rect.height) * canvas.height;
  const [x, y] = tank.cellAt(px, py);
  // Clamped to just outside the grid, so a captured drag far off the
  // canvas doesn't emit a long run of stamps.
  const pad = brushRadius();
  const clamp = (v, len) => Math.max(-pad, Math.min(len - 1 + pad, v));
  return { x: clamp(x, tank.width()), y: clamp(y, tank.height()) };
}

function setRunning(on) {
  tank.setRunning(on);
  playBtn.textContent = on ? "Pause" : "Play";
  stepBtn.disabled = on;
  kick();
}

function step() {
  tank.step();
  updateStats();
  kick();
}

function buildPalette() {
  const palette = $("palette");
  for (const t of TOOLS) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "swatch";
    b.setAttribute("role", "radio");
    b.setAttribute("aria-checked", String(t === tool));
    b.innerHTML = `<i style="--c: ${t[2]}"></i>`;
    b.append(t[0]);
    b.addEventListener("click", () => {
      tool = t;
      for (const other of palette.children) other.setAttribute("aria-checked", String(other === b));
      if (pointer) holdBrush();
    });
    palette.append(b);
  }
}

function wireInput() {
  canvas.addEventListener("pointerdown", (e) => {
    // One stroke at a time: extra fingers are ignored.
    if (pointer || (e.button !== 0 && e.button !== 2)) return;
    e.preventDefault();
    canvas.setPointerCapture(e.pointerId);
    const c = cellAt(e);
    pointer = { ...c, id: e.pointerId, erase: e.button === 2 };
    stamp(c.x, c.y, pointer.erase);
    holdBrush();
    kick();
  });
  canvas.addEventListener("pointermove", (e) => {
    if (pointer?.id !== e.pointerId) return;
    const c = cellAt(e);
    stroke(pointer.x, pointer.y, c.x, c.y, pointer.erase);
    pointer.x = c.x;
    pointer.y = c.y;
    holdBrush();
    kick();
  });
  const end = (e) => {
    if (pointer?.id !== e.pointerId) return;
    pointer = null;
    holdBrush();
  };
  canvas.addEventListener("pointerup", end);
  canvas.addEventListener("pointercancel", end);
  canvas.addEventListener("lostpointercapture", end);
  canvas.addEventListener("contextmenu", (e) => e.preventDefault());

  playBtn.addEventListener("click", () => setRunning(!tank.running()));
  stepBtn.addEventListener("click", step);
  $("reset").addEventListener("click", () => reset().catch((err) => showError(String(err))));
  seedInput.addEventListener("change", () => reset().catch((err) => showError(String(err))));
  brushInput.addEventListener("input", () => {
    $("brush-out").value = brushInput.value;
    if (pointer) holdBrush();
  });
  document.addEventListener("keydown", (e) => {
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLButtonElement) return;
    if (e.key === " ") {
      e.preventDefault();
      setRunning(!tank.running());
    } else if (e.key === "." && !tank.running()) {
      step();
    }
  });
}

function wireLifecycle() {
  // Hidden tabs pause: no ticks, no frames, and world time stops.
  document.addEventListener("visibilitychange", () => {
    hidden = document.hidden;
    if (hidden) {
      tank?.pause();
      if (rafId) cancelAnimationFrame(rafId);
      rafId = 0;
    } else {
      kick();
    }
  });

  // WebGL2 context loss: stop drawing until the browser restores it.
  canvas.addEventListener("webglcontextlost", (e) => {
    e.preventDefault();
    restoring = true;
    tank?.detach();
  });
  canvas.addEventListener("webglcontextrestored", () => {
    restoring = false;
    recover();
  });

  // The grid is fixed; resizing only changes the scale and letterbox.
  const ro = new ResizeObserver((entries) => {
    const e = entries[0];
    const box = e.devicePixelContentBoxSize?.[0];
    const size = box
      ? [Math.max(1, box.inlineSize), Math.max(1, box.blockSize)]
      : canvasPixels();
    setCanvasSize(size);
  });
  try {
    ro.observe(canvas, { box: "device-pixel-content-box" });
  } catch {
    ro.observe(canvas);
  }

  // 15 fps on battery where the Battery Status API exists (Chromium).
  navigator.getBattery?.().then((battery) => {
    const update = () => {
      maxFps = battery.charging ? FPS_AC : FPS_BATTERY;
      tank?.setMaxFps(maxFps);
    };
    battery.addEventListener("chargingchange", update);
    update();
  }, () => {});
}

async function main() {
  try {
    await init();
  } catch (err) {
    showError(`Couldn't load the WebAssembly module: ${err}`);
    return;
  }
  const px = canvasPixels();
  canvas.width = px[0];
  canvas.height = px[1];
  grid = Array.from(gridFor(px[0], px[1]));
  buildPalette();
  try {
    replaceTank(new WebTank(grid[1], grid[2], seed(), px[0], px[1]));
    await attachGraphics();
  } catch (err) {
    showError(`Couldn't start the GPU renderer: ${err}`);
    return;
  }
  wireInput();
  wireLifecycle();
  statusEl.hidden = true;
  fpsSince = performance.now();
  updateStats();
  document.body.dataset.ready = "true";
  kick();
}

main();
