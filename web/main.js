// DeskTank web demo: drives the wasm build of desktank-sim (crates/desktank-web).
// The sim ticks at a fixed 20 Hz; requestAnimationFrame only draws, and only
// uploads a frame when the world changed.
import init, { Demo } from "./pkg/desktank_web.js";

const WIDTH = 320;
const HEIGHT = 180;
const WATER_LEVEL = 0.9;
const TICK_MS = 1000 / 20;
// After a stall (e.g. a background tab), drop the backlog instead of
// fast-forwarding through it.
const MAX_CATCH_UP = 4;

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

const $ = (id) => document.getElementById(id);
const canvas = $("tank");
const ctx = canvas.getContext("2d", { alpha: false });
const statusEl = $("status");
const statsEl = $("stats");
const playBtn = $("play");
const stepBtn = $("step");
const seedInput = $("seed");
const brushInput = $("brush");

let wasm;
let demo;
let image;
let running = true;
let tool = TOOLS[0];
let pointer = null; // { x, y, erase } in cells while a drag is active
let lastTime = 0;
let backlog = 0;
let tickMs = 0;
let frames = 0;
let fps = 0;
let fpsSince = 0;

function showError(message) {
  statusEl.textContent = message;
  statusEl.hidden = false;
}

function reset() {
  const seed = Math.max(0, Math.min(0xffffffff, Math.floor(Number(seedInput.value) || 0)));
  seedInput.value = seed;
  demo?.free();
  demo = new Demo(WIDTH, HEIGHT, seed, WATER_LEVEL);
  canvas.width = demo.width();
  canvas.height = demo.height();
  image = ctx.createImageData(canvas.width, canvas.height);
  backlog = 0;
  draw();
  updateStats();
}

function draw() {
  if (!demo.render()) return;
  // Re-read the pointer every time: wasm memory may have grown.
  const frame = new Uint8ClampedArray(wasm.memory.buffer, demo.frame_ptr(), demo.frame_len());
  image.data.set(frame);
  ctx.putImageData(image, 0, 0);
}

function brushRadius() {
  return Number(brushInput.value);
}

// Applies the current tool at one cell.
function stamp(x, y, erase) {
  const r = brushRadius();
  if (erase || tool[1] === null) {
    demo.erase(x, y, r);
  } else {
    demo.pour(tool[1], x, y, r);
  }
}

// Stamps along the segment from (x0, y0) to (x1, y1) so fast drags leave
// no gaps.
function stroke(x0, y0, x1, y1, erase) {
  const steps = Math.max(1, Math.ceil(Math.hypot(x1 - x0, y1 - y0) / Math.max(1, brushRadius())));
  for (let i = 1; i <= steps; i++) {
    stamp(Math.round(x0 + ((x1 - x0) * i) / steps), Math.round(y0 + ((y1 - y0) * i) / steps), erase);
  }
}

function tick() {
  // Holding still keeps pouring, like a tap.
  if (pointer) stamp(pointer.x, pointer.y, pointer.erase);
  const t0 = performance.now();
  demo.tick();
  tickMs = performance.now() - t0;
}

function updateStats() {
  statsEl.textContent =
    `fps ${fps} · tick ${demo.tick_count()} · awake chunks ${demo.awake_chunks()}/${demo.chunk_count()}` +
    ` · tick time ${tickMs.toFixed(2)} ms`;
}

function loop(now) {
  const dt = Math.min(now - lastTime, TICK_MS * MAX_CATCH_UP);
  lastTime = now;
  if (running) {
    backlog += dt;
    while (backlog >= TICK_MS) {
      tick();
      backlog -= TICK_MS;
    }
  }
  draw();
  frames++;
  if (now - fpsSince >= 1000) {
    fps = Math.round((frames * 1000) / (now - fpsSince));
    frames = 0;
    fpsSince = now;
    updateStats();
  }
  requestAnimationFrame(loop);
}

function cellAt(event) {
  const rect = canvas.getBoundingClientRect();
  return {
    x: Math.floor(((event.clientX - rect.left) / rect.width) * canvas.width),
    y: Math.floor(((event.clientY - rect.top) / rect.height) * canvas.height),
  };
}

function setRunning(on) {
  running = on;
  playBtn.textContent = on ? "Pause" : "Play";
  stepBtn.disabled = on;
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
    });
    palette.append(b);
  }
}

function wireInput() {
  canvas.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 && e.button !== 2) return;
    e.preventDefault();
    canvas.setPointerCapture(e.pointerId);
    const c = cellAt(e);
    pointer = { ...c, erase: e.button === 2 };
    stamp(c.x, c.y, pointer.erase);
  });
  canvas.addEventListener("pointermove", (e) => {
    if (!pointer) return;
    const c = cellAt(e);
    stroke(pointer.x, pointer.y, c.x, c.y, pointer.erase);
    pointer.x = c.x;
    pointer.y = c.y;
  });
  const end = () => {
    pointer = null;
  };
  canvas.addEventListener("pointerup", end);
  canvas.addEventListener("pointercancel", end);
  canvas.addEventListener("lostpointercapture", end);
  canvas.addEventListener("contextmenu", (e) => e.preventDefault());

  playBtn.addEventListener("click", () => setRunning(!running));
  stepBtn.addEventListener("click", () => {
    tick();
    updateStats();
  });
  $("reset").addEventListener("click", reset);
  seedInput.addEventListener("change", reset);
  brushInput.addEventListener("input", () => {
    $("brush-out").value = brushInput.value;
  });
  document.addEventListener("keydown", (e) => {
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLButtonElement) return;
    if (e.key === " ") {
      e.preventDefault();
      setRunning(!running);
    } else if (e.key === "." && !running) {
      tick();
      updateStats();
    }
  });
}

async function main() {
  try {
    wasm = await init();
  } catch (err) {
    showError(`Couldn't load the WebAssembly module: ${err}`);
    return;
  }
  buildPalette();
  wireInput();
  reset();
  statusEl.hidden = true;
  lastTime = fpsSince = performance.now();
  requestAnimationFrame(loop);
}

main();
