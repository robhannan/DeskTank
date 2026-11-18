// DeskTank web host: drives crates/desktank-web, which runs desktank-render's
// Tank through wgpu (WebGPU, or WebGL2 as the fallback). SPEC §3.5.
//
// Pacing lives in the tank: a fixed 20 Hz simulation tick, and frames drawn
// only when something changed, capped at 30 fps (15 on battery). The tools
// (pour, scoop, glue, net, inspect, devices; brush; stroke interpolation)
// are desktank_sim::tools inside the tank: this file maps mouse and touch
// onto pointer events in canvas pixels. It also owns the rAF loop, the
// menu, the settings panel (localStorage), visibility, battery and
// device/context loss.
//
// URL parameters: ?backend=webgpu|webgl2 forces a backend; ?bench records
// rAF callback times in window.desktank.rafTimes and drawn-frame times in
// window.desktank.frameTimes (scripts/bench-web.mjs).
import init, { Graphics, WebTank, brushRange, gridFor } from "./pkg/desktank_web.js";

const FPS_AC = 30;
const FPS_BATTERY = 15;
// Flags returned by WebTank.frame (crates/desktank-web).
const DREW = 1;
const WANTED = 2;
const LOST = 4;
const TICKS_SHIFT = 8;
// Pointer buttons WebTank takes: primary uses the tool, secondary scoops.
const PRIMARY = 0;
const SECONDARY = 2;
// Touch: a finger held this long without moving inspects (SPEC §3.5); one
// that moves further than the slop (CSS px) first starts a stroke.
const LONG_PRESS_MS = 500;
const TOUCH_SLOP = 8;
// Wheel delta (px) per brush step, so trackpads don't race.
const WHEEL_STEP = 40;
const INSPECT_SHOW_MS = 5000;

// Display labels for palette entries (the entries come from the sim; an
// unlisted name falls back to its humanised form).
const LABELS = {
  air_bubble: "Bubbles",
  flake_food: "Flakes",
  sinking_pellet: "Pellets",
  bloodworm: "Bloodworms",
  algae_wafer: "Algae wafers",
  root_tab: "Root tabs",
  co2_bubble: "CO₂",
};
// What holds a plant back, by `Limit::key` (the inspect card).
const LIMITS = {
  none: "growing well",
  "light-low": "light too low",
  "light-high": "light too strong",
  "nutrient-low": "nutrients too low",
  "nutrient-high": "nutrients too high",
  "co2-low": "CO₂ too low",
  "co2-high": "CO₂ too high",
};
const FIELDS = ["", "nutrient", "co2", "light"];

const SETTINGS_KEY = "desktank.settings.v1";
const DEFAULTS = Object.freeze({ scale: 0, fps: "auto", field: "", avoidCursor: false, stats: true, seed: 1, radius: 3 });

const params = new URLSearchParams(location.search);
const wantBackend = params.get("backend") ?? "auto";
const bench = params.has("bench");

const $ = (id) => document.getElementById(id);
const canvas = $("tank");
const statusEl = $("status");
const statsEl = $("stats");
const inspectEl = $("inspect");
const playBtn = $("play");
const stepBtn = $("step");
const seedInput = $("seed");
const brushInput = $("brush");
const paletteEl = $("palette");
const groupsEl = $("groups");
const plantPickEl = $("plant-species");

let settings = loadSettings();
let tank;
let openGroup; // name of the palette group shown
let grid; // [scale, columns, rows] of the current tank
let backend = "";
let adapter = "";
let rafId = 0;
let timerId = 0;
let hidden = document.hidden;
let batteryFps = FPS_AC;
let restoring = false; // a graphics open is in flight
let contextLost = false; // WebGL2 context lost, not yet restored
let lossCount = 0; // bumped on each loss, so a stale open is discarded
let tickMs = 0;
let frames = 0;
let fps = 0;
let fpsSince = 0;
let inspectTimer = 0;
let wheelRest = 0;

// Test and benchmark hooks (tests/web/).
const hooks = {
  frameTimes: [],
  rafTimes: [],
  get tank() {
    return tank;
  },
  get backend() {
    return backend;
  },
  get grid() {
    return grid;
  },
  get settings() {
    return { ...settings };
  },
  loadScene,
};
window.desktank = hooks;

// --- Settings (localStorage) ---------------------------------------------

function loadSettings() {
  let saved = {};
  try {
    saved = JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "{}") ?? {};
  } catch {
    // Private mode, blocked storage or a corrupt value: use the defaults.
  }
  const int = (v, lo, hi, d) => (Number.isInteger(v) && v >= lo && v <= hi ? v : d);
  return {
    scale: int(saved.scale, 0, 8, DEFAULTS.scale),
    fps: ["auto", "30", "15"].includes(saved.fps) ? saved.fps : DEFAULTS.fps,
    field: FIELDS.includes(saved.field) ? saved.field : DEFAULTS.field,
    avoidCursor: saved.avoidCursor === true,
    stats: typeof saved.stats === "boolean" ? saved.stats : DEFAULTS.stats,
    seed: int(saved.seed, 0, 0xffffffff, DEFAULTS.seed),
    radius: int(saved.radius, 0, 24, DEFAULTS.radius), // the tank clamps it too
  };
}

function saveSettings() {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
  } catch {
    // Storage unavailable: settings last for this page only.
  }
}

function update(changes) {
  settings = { ...settings, ...changes };
  saveSettings();
  showSettings();
}

// Mirrors the settings into the panel's fields and applies the cheap ones.
function showSettings() {
  $("set-scale").value = String(settings.scale);
  $("set-fps").value = settings.fps;
  $("set-field").value = settings.field;
  $("set-avoid-cursor").checked = settings.avoidCursor;
  $("set-stats").checked = settings.stats;
  seedInput.value = settings.seed;
  brushInput.value = settings.radius;
  $("brush-out").value = settings.radius;
  statsEl.hidden = !settings.stats;
  if (tank) {
    tank.setMaxFps(maxFps());
    tank.setFieldOverlay(settings.field);
    tank.setCursorAvoidance(settings.avoidCursor);
    tank.setRadius(settings.radius);
    kick();
  }
}

function maxFps() {
  return settings.fps === "auto" ? batteryFps : Number(settings.fps);
}

// --- Tank and graphics -------------------------------------------------------

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

// Swaps in a new tank; the caller attaches graphics to it.
function replaceTank(next) {
  // A new tank keeps the selection; the first starts on sand.
  const selected = tank?.selected() ?? next.selected();
  tank?.free();
  tank = next;
  tank.select(selected);
  showSettings();
  setRunning(tank.running());
  buildPlantPicker();
  buildPalette();
}

// Opens graphics and attaches them to the current tank. An open that a
// context loss overtook is discarded and retried once the context is back;
// returns false if the context is lost now (the restore event reattaches).
async function attachGraphics() {
  for (;;) {
    if (contextLost) return false;
    const losses = lossCount;
    const g = await Graphics.open(canvas, backend || wantBackend);
    if (losses === lossCount && !contextLost) {
      backend = g.backend();
      adapter = g.adapterName();
      tank.attach(g);
      canvas.dataset.backend = backend;
      return true;
    }
    g.free();
  }
}

// A new tank for the current canvas, cell scale setting and seed.
function newTank() {
  const [w, h] = canvasPixels();
  canvas.width = w;
  canvas.height = h;
  grid = Array.from(gridFor(w, h, settings.scale));
  return new WebTank(grid[1], grid[2], settings.seed, w, h);
}

async function reset() {
  endGestures();
  // Graphics belong to the old tank; reopen them for the new one.
  replaceTank(newTank());
  // During a loss or recovery, the recovery attaches to the new tank.
  if (!restoring) await recover(false);
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
  tank.setFieldOverlay("");
  await attachGraphics();
  kick();
  const drawn = tank.framesDrawn();
  while (tank.framesDrawn() === drawn) {
    await new Promise((r) => requestAnimationFrame(r));
  }
}

function updateStats() {
  if (!settings.stats || !tank) return;
  statsEl.textContent =
    `${backend || "no GPU"} · fps ${fps} · tick ${tank.tickCount()} · awake chunks ` +
    `${tank.awakeChunks()}/${tank.chunkCount()} · tick time ${tickMs.toFixed(2)} ms` +
    ` · ${grid[1]}×${grid[2]} cells at ${grid[0]}×`;
}

// --- Loop --------------------------------------------------------------------

// One callback: the tank decides whether a tick or a frame is due, and when
// it next needs us.
function loop(now) {
  rafId = 0;
  if (hidden || !tank) return;
  if (bench) hooks.rafTimes.push(now);
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
  // A frame due now and something to draw it on: the next vsync. Running:
  // sleep until the next tick (20 wake-ups/s, not one per vsync; also the
  // back-off while no graphics are attached). Paused and idle: sleep until
  // input wakes us.
  // A frame held back by the cap sleeps until the cap allows it.
  if (flags & WANTED && tank.backend() && tank.wakeAt() <= performance.now() + 1) kick();
  else if (tank.running() || flags & WANTED) sleepUntil(tank.wakeAt());
  else updateStats();
}

function kick() {
  if (rafId || hidden || restoring || contextLost) return;
  clearTimeout(timerId);
  timerId = 0;
  rafId = requestAnimationFrame(loop);
}

// Sleeps on a timer and then runs the loop directly (not through rAF, whose
// vsync timestamp can fall just before the tick and cost a second wake-up).
// Drawing outside rAF is fine: the canvas presents at the next composite.
function sleepUntil(t) {
  clearTimeout(timerId);
  timerId = setTimeout(() => {
    timerId = 0;
    if (!rafId && !hidden && !restoring && !contextLost) loop(performance.now());
  }, Math.max(0, Math.ceil(t - performance.now())));
}

function stopLoop() {
  if (rafId) cancelAnimationFrame(rafId);
  clearTimeout(timerId);
  rafId = 0;
  timerId = 0;
}

// Device or context loss: drop the graphics and reopen them; the world is
// unaffected.
async function recover(lost = true) {
  if (restoring) return;
  restoring = true;
  tank.detach();
  try {
    await attachGraphics();
  } catch (err) {
    showError(lost ? `Lost the GPU and couldn't get it back: ${err}` : `Couldn't start the GPU renderer: ${err}`);
    return;
  } finally {
    restoring = false;
  }
  kick();
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

// --- Palette -----------------------------------------------------------------

function label(name) {
  return LABELS[name] ?? name.charAt(0).toUpperCase() + name.slice(1).replaceAll("_", " ");
}

// The plants slot pours the species picked here (the sim's list).
function buildPlantPicker() {
  // A new tank keeps the species picked for the last one.
  const kept = plantPickEl.value;
  plantPickEl.replaceChildren(
    ...tank.plantSpecies().map((entry, i) => {
      const [name, ...description] = entry.split(" ");
      const o = document.createElement("option");
      o.value = String(i);
      o.textContent = label(name);
      o.title = description.join(" ");
      return o;
    }),
  );
  if (kept !== "") tank.setPlantSpecies(Number(kept));
  plantPickEl.value = String(tank.plantSpeciesSelected());
}

function pickPlant() {
  tank.setPlantSpecies(Number(plantPickEl.value));
  const slot = tank.palette().findIndex((e) => e.startsWith("plant "));
  const colour = tank.paletteColours()[slot];
  const button = paletteEl.children[slot];
  if (button) {
    button.firstElementChild?.style.setProperty("--c", colour);
    openGroup = button.dataset.group;
    choose(slot);
  }
}

// The palette is the sim's (WebTank.palette): materials, tools, devices,
// creatures and the plants slot, in groups (WebTank.paletteGroups). The
// group tabs show one group's items at a time; the selection can be in a
// closed group.
function buildPalette() {
  const entries = tank.palette();
  const colours = tank.paletteColours();
  const groups = tank.paletteGroups().map((g) => g.split(" "));
  const tabs = [...new Map(groups).entries()];
  groupsEl.replaceChildren(
    ...tabs.map(([name, text]) => {
      const t = document.createElement("button");
      t.type = "button";
      t.id = `group-${name}`;
      t.dataset.group = name;
      t.setAttribute("role", "tab");
      t.setAttribute("aria-controls", "palette-panel");
      t.textContent = text;
      t.addEventListener("click", () => showGroup(name));
      return t;
    }),
  );
  let slot = 0;
  paletteEl.replaceChildren(
    ...entries.map((entry, i) => {
      const [kind, name] = entry.split(" ");
      const group = groups[i][0];
      slot = i > 0 && groups[i - 1][0] === group ? slot + 1 : 0;
      const b = document.createElement("button");
      b.type = "button";
      b.className = "swatch";
      b.dataset.kind = kind;
      b.dataset.name = name;
      b.dataset.group = group;
      b.setAttribute("role", "radio");
      const i_ = document.createElement("i");
      if (colours[i]) i_.style.setProperty("--c", colours[i]);
      b.append(i_, label(name));
      if (slot < 10) b.title = `Key ${(slot + 1) % 10}`;
      b.addEventListener("click", () => choose(i));
      return b;
    }),
  );
  // A rebuilt palette (reset, new scale) keeps the open group if it still
  // exists; the first opens on the selection's.
  if (!tabs.some(([name]) => name === openGroup)) openGroup = groups[tank.selected()]?.[0] ?? tabs[0]?.[0];
  showSelection();
}

function choose(i, focus = false) {
  tank.select(i);
  showSelection(focus);
  kick();
}

// Shows group `name`'s items (the selection stays as it is).
function showGroup(name, focus = false) {
  openGroup = name;
  showSelection();
  if (focus) $(`group-${name}`)?.focus();
}

// Opens the group `steps` after (or before) the open one, wrapping.
function stepGroup(steps, focus = false) {
  const tabs = [...groupsEl.children];
  const at = tabs.findIndex((t) => t.dataset.group === openGroup);
  const n = tabs.length;
  showGroup(tabs[(((at + steps) % n) + n) % n].dataset.group, focus);
}

// The open group's buttons.
const openItems = () => [...paletteEl.children].filter((b) => !b.hidden);

// Roving tabindex: one tab and one radio are in the tab order, the open
// tab and the checked radio (or, if the selection is in a closed group,
// the open group's first).
function showSelection(focus = false) {
  const selected = tank.selected();
  const holder = paletteEl.children[selected]?.dataset.group;
  for (const t of groupsEl.children) {
    const open = t.dataset.group === openGroup;
    t.setAttribute("aria-selected", String(open));
    t.tabIndex = open ? 0 : -1;
    t.toggleAttribute("data-holds", !open && t.dataset.group === holder);
  }
  $("palette-panel").setAttribute("aria-labelledby", `group-${openGroup}`);
  [...paletteEl.children].forEach((b, i) => {
    const on = i === selected;
    b.hidden = b.dataset.group !== openGroup;
    b.setAttribute("aria-checked", String(on));
    b.tabIndex = on && !b.hidden ? 0 : -1;
    if (on && focus) b.focus({ preventScroll: true });
  });
  const items = openItems();
  if (items.length && !items.some((b) => b.tabIndex === 0)) items[0].tabIndex = 0;
}

// Selects slot `n` of the open group, if it has one.
function chooseSlot(n, focus = false) {
  const b = openItems()[n];
  if (b) choose([...paletteEl.children].indexOf(b), focus);
}

// Arrow keys: within the tabs, open the next or previous group; within the
// radio group, select the next or previous item of the open group. Home and
// End jump to the first and last.
function arrowStep(e, at, n) {
  return { ArrowRight: at + 1, ArrowDown: at + 1, ArrowLeft: at - 1, ArrowUp: at - 1, Home: 0, End: n - 1 }[e.key];
}

function groupKeys(e) {
  const tabs = [...groupsEl.children];
  const next = arrowStep(e, Math.max(0, tabs.indexOf(document.activeElement)), tabs.length);
  if (next === undefined) return;
  e.preventDefault();
  e.stopPropagation();
  const n = tabs.length;
  showGroup(tabs[(next + n) % n].dataset.group, true);
}

function paletteKeys(e) {
  const items = openItems();
  const n = items.length;
  const next = arrowStep(e, Math.max(0, items.indexOf(document.activeElement)), n);
  if (next === undefined || !n) return;
  e.preventDefault();
  e.stopPropagation();
  chooseSlot((next + n) % n, true);
}

// --- Input -------------------------------------------------------------------

function canvasPoint(e) {
  const rect = canvas.getBoundingClientRect();
  return [((e.clientX - rect.left) / rect.width) * canvas.width, ((e.clientY - rect.top) / rect.height) * canvas.height];
}

// Shows what inspect last found under the tank for a few seconds.
function showInspection() {
  const found = tank.takeInspection();
  if (!found) return;
  const [kind, ...rest] = found.split(" ");
  if (kind === "cell") {
    const [name, x, y] = rest;
    inspectEl.textContent = `${label(name)} · cell ${x}, ${y}`;
  } else if (kind === "plant") {
    const [species, id, age, health, cells, limit, x, y] = rest;
    const s = Number(age);
    const old = s >= 60 ? `${Math.floor(s / 60)} min ${s % 60} s` : `${s} s`;
    inspectEl.textContent = `${label(species)} #${id} · ${old} old · health ${health}% · ${cells} cells · ${LIMITS[limit] ?? limit} · cell ${x}, ${y}`;
  } else if (kind === "creature") {
    const [species, id, over, x, y] = rest;
    inspectEl.textContent = `${label(species)} #${id} · over ${label(over)} · cell ${x}, ${y}`;
  } else {
    return;
  }
  inspectEl.hidden = false;
  clearTimeout(inspectTimer);
  inspectTimer = setTimeout(() => {
    inspectEl.hidden = true;
  }, INSPECT_SHOW_MS);
}

function after() {
  showInspection();
  kick();
}

// Mouse and pen: buttons map straight onto the tools.
const mouse = {
  down(e) {
    const button = e.button === 2 ? SECONDARY : e.button === 0 ? PRIMARY : -1;
    if (button < 0) return;
    canvas.setPointerCapture(e.pointerId);
    tank.pointerDown(button, ...canvasPoint(e));
  },
  move(e) {
    tank.pointerMove(...canvasPoint(e));
  },
  up(e) {
    tank.pointerUp(e.button === 2 ? SECONDARY : PRIMARY);
  },
};

// Touch (SPEC §3.5): drag pours (the selected tool), two-finger drag
// scoops, long-press inspects. A finger only starts a stroke once it moves
// past the slop, so a long-press or a second finger doesn't pour first; a
// quick tap uses the tool once.
const touches = new Map(); // pointerId -> [x, y] canvas px
let gesture = null; // { mode: "pending" | "stroke" | "scoop" | "done", id, start, timer }

function midpoint() {
  const [a, b] = [...touches.values()];
  return [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
}

const touch = {
  down(e) {
    canvas.setPointerCapture(e.pointerId);
    touches.set(e.pointerId, canvasPoint(e));
    if (touches.size === 1) {
      const start = touches.get(e.pointerId);
      gesture = {
        mode: "pending",
        id: e.pointerId,
        start,
        timer: setTimeout(() => {
          if (gesture?.mode !== "pending") return;
          gesture.mode = "done";
          tank.inspectAt(...start);
          after();
        }, LONG_PRESS_MS),
      };
    } else if (touches.size === 2 && gesture) {
      // A second finger turns the gesture into a scoop at the midpoint.
      clearTimeout(gesture.timer);
      if (gesture.mode === "stroke") tank.pointerUp(PRIMARY);
      if (gesture.mode !== "done") {
        gesture.mode = "scoop";
        tank.pointerDown(SECONDARY, ...midpoint());
      }
    }
  },
  move(e) {
    if (!touches.has(e.pointerId) || !gesture) return;
    const p = canvasPoint(e);
    touches.set(e.pointerId, p);
    if (gesture.mode === "pending" && e.pointerId === gesture.id) {
      const slop = TOUCH_SLOP * (window.devicePixelRatio || 1);
      if (Math.hypot(p[0] - gesture.start[0], p[1] - gesture.start[1]) < slop) return;
      clearTimeout(gesture.timer);
      gesture.mode = "stroke";
      tank.pointerDown(PRIMARY, ...gesture.start);
    }
    if (gesture.mode === "stroke" && e.pointerId === gesture.id) tank.pointerMove(...p);
    else if (gesture.mode === "scoop" && touches.size >= 2) tank.pointerMove(...midpoint());
  },
  up(e) {
    if (!touches.delete(e.pointerId) || !gesture) return;
    if (gesture.mode === "pending" && e.type === "pointercancel") {
      // The browser took the touch: no tap.
      clearTimeout(gesture.timer);
      gesture.mode = "done";
    } else if (gesture.mode === "pending") {
      // A tap: use the tool once where it landed.
      clearTimeout(gesture.timer);
      tank.pointerDown(PRIMARY, ...gesture.start);
      tank.pointerUp(PRIMARY);
      gesture.mode = "done";
    } else if (gesture.mode === "stroke" && e.pointerId === gesture.id) {
      tank.pointerUp(PRIMARY);
      gesture.mode = "done";
    } else if (gesture.mode === "scoop") {
      // Any finger lifting ends the scoop, so the midpoint never jumps.
      tank.pointerUp(SECONDARY);
      gesture.mode = "done";
    }
    if (touches.size === 0) {
      gesture = null;
      tank.pointerLeave();
    }
  },
};

const handler = (e) => (e.pointerType === "touch" ? touch : mouse);

function endGestures() {
  if (gesture) clearTimeout(gesture.timer);
  gesture = null;
  touches.clear();
  tank?.pointerLeave();
}

function wireInput() {
  canvas.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    handler(e).down(e);
    after();
  });
  canvas.addEventListener("pointermove", (e) => {
    handler(e).move(e);
    after();
  });
  const up = (e) => {
    handler(e).up(e);
    after();
  };
  canvas.addEventListener("pointerup", up);
  canvas.addEventListener("pointercancel", up);
  canvas.addEventListener("lostpointercapture", (e) => {
    // After a normal pointerup this repeats it harmlessly; otherwise the
    // capture was taken away mid-stroke.
    if (e.pointerType === "touch") up(e);
    else if (e.buttons === 0) {
      tank.pointerUp(PRIMARY);
      tank.pointerUp(SECONDARY);
      after();
    }
  });
  canvas.addEventListener("pointerleave", (e) => {
    // Hide the brush outline once the mouse is off the canvas.
    if (e.pointerType !== "touch" && e.buttons === 0) {
      tank.pointerLeave();
      kick();
    }
  });
  canvas.addEventListener("contextmenu", (e) => e.preventDefault());
  // The wheel (and trackpad pinch, which arrives as ctrl+wheel) sizes the
  // brush and never scrolls or zooms the page.
  canvas.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const unit = e.deltaMode === 1 ? WHEEL_STEP / 3 : e.deltaMode === 2 ? WHEEL_STEP * 10 : 1;
      wheelRest += -e.deltaY * unit;
      const steps = Math.trunc(wheelRest / WHEEL_STEP);
      if (!steps) return;
      wheelRest -= steps * WHEEL_STEP;
      update({ radius: tank.resizeBrush(steps) });
    },
    { passive: false },
  );
  // Safari's own pinch gesture events.
  for (const type of ["gesturestart", "gesturechange"]) canvas.addEventListener(type, (e) => e.preventDefault());

  paletteEl.addEventListener("keydown", paletteKeys);
  groupsEl.addEventListener("keydown", groupKeys);
  plantPickEl.addEventListener("change", pickPlant);
  playBtn.addEventListener("click", () => setRunning(!tank.running()));
  stepBtn.addEventListener("click", step);
  $("feed").addEventListener("click", () => {
    tank.feed();
    kick();
  });
  const resetOrShow = () => reset().catch((err) => showError(String(err)));
  $("reset").addEventListener("click", resetOrShow);
  brushInput.addEventListener("input", () => update({ radius: Number(brushInput.value) }));
  wireSettings(resetOrShow);
  document.addEventListener("keydown", (e) => {
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === " " && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      setRunning(!tank.running());
    } else if (e.key === "." && !tank.running()) {
      step();
    } else if (e.key === "[" || e.key === "]") {
      update({ radius: tank.resizeBrush(e.key === "]" ? 1 : -1) });
    } else if (e.key === "f" || e.key === "F") {
      const next = FIELDS[(FIELDS.indexOf(settings.field) + 1) % FIELDS.length];
      update({ field: next });
    } else if (e.key === "g" || e.key === "G") {
      // G opens the next group, Shift+G the previous (as in desktank window;
      // Shift, not the letter's case, so Caps Lock doesn't flip it). Focus
      // in the palette moves to the new tab, as its radio may be hidden.
      const inPalette = paletteEl.contains(e.target) || groupsEl.contains(e.target);
      stepGroup(e.shiftKey ? -1 : 1, inPalette);
    } else if (/^[0-9]$/.test(e.key)) {
      // 1–9 are the open group's first nine slots, 0 the tenth.
      chooseSlot((Number(e.key) + 9) % 10);
    }
  });
}

function wireSettings(resetTank) {
  const toggle = $("settings-toggle");
  toggle.addEventListener("click", () => {
    const open = toggle.getAttribute("aria-expanded") !== "true";
    toggle.setAttribute("aria-expanded", String(open));
    $("settings").hidden = !open;
  });
  $("set-scale").addEventListener("change", (e) => {
    update({ scale: Number(e.target.value) });
    resetTank();
  });
  $("set-fps").addEventListener("change", (e) => update({ fps: e.target.value }));
  $("set-field").addEventListener("change", (e) => update({ field: e.target.value }));
  $("set-avoid-cursor").addEventListener("change", (e) => update({ avoidCursor: e.target.checked }));
  $("set-stats").addEventListener("change", (e) => {
    update({ stats: e.target.checked });
    updateStats();
  });
  seedInput.addEventListener("change", () => {
    const s = Math.max(0, Math.min(0xffffffff, Math.floor(Number(seedInput.value) || 0)));
    update({ seed: s });
    resetTank();
  });
  $("set-defaults").addEventListener("click", () => {
    const rescale = settings.scale !== DEFAULTS.scale || settings.seed !== DEFAULTS.seed;
    update({ ...DEFAULTS });
    if (rescale) resetTank();
  });
}

function wireLifecycle() {
  // Hidden tabs pause: no ticks, no frames, and world time stops.
  document.addEventListener("visibilitychange", () => {
    hidden = document.hidden;
    if (hidden) {
      tank?.pause();
      stopLoop();
    } else {
      kick();
    }
  });

  // WebGL2 context loss: stop drawing until the browser restores it.
  canvas.addEventListener("webglcontextlost", (e) => {
    e.preventDefault();
    contextLost = true;
    lossCount++;
    tank?.detach();
  });
  canvas.addEventListener("webglcontextrestored", () => {
    contextLost = false;
    // An open in flight retries by itself; otherwise start one.
    recover();
  });

  // The grid is fixed; resizing only changes the scale and letterbox. The
  // size is the CSS box × devicePixelRatio, as for the initial grid
  // (devicePixelContentBoxSize would be exact, but Chromium reports CSS
  // pixels there under DPR emulation, as in the Playwright tests).
  new ResizeObserver(() => setCanvasSize(canvasPixels())).observe(canvas);

  // 15 fps on battery where the Battery Status API exists (Chromium),
  // unless the settings fix the cap.
  navigator.getBattery?.().then((battery) => {
    const onChange = () => {
      batteryFps = battery.charging ? FPS_AC : FPS_BATTERY;
      tank?.setMaxFps(maxFps());
    };
    battery.addEventListener("chargingchange", onChange);
    onChange();
  }, () => {});
}

async function main() {
  try {
    await init();
  } catch (err) {
    showError(`Couldn't load the WebAssembly module: ${err}`);
    return;
  }
  const [rMin, rMax] = brushRange();
  brushInput.min = rMin;
  brushInput.max = rMax;
  try {
    replaceTank(newTank());
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
