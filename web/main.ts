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
// Saves (WB3): the tank is saved to IndexedDB (store.ts) in the native
// app's format when the page is hidden, on pagehide, every 5 minutes and
// after a reset or import; a reload opens the newest save at its own grid
// size, letterboxed. Only the tab holding the tank's Web Lock
// (ownership.ts) simulates and writes; another tab shows "open in another
// tab" with Take over. Export and Import move save files to and from the
// desktop app (`desktank export` / `desktank import`).
//
// URL parameters: ?backend=webgpu|webgl2 forces a backend; ?bench records
// rAF callback times in window.desktank.rafTimes and drawn-frame times in
// window.desktank.frameTimes (scripts/bench-web.ts).
import init, { Graphics, WebTank, brushRange, gridFor } from "./pkg/desktank_web.js";
import type { Hooks, Settings } from "./hooks.js";
import type { Save } from "./store.ts";
import { Ownership } from "./ownership.ts";
import { claim, openStore, readSaves, writeSave } from "./store.ts";

// The Battery Status API (Chromium only), which TypeScript's DOM types lack.
interface BatteryManager extends EventTarget {
  readonly charging: boolean;
}
declare global {
  interface Navigator {
    getBattery?(): Promise<BatteryManager>;
  }
}

type Point = [x: number, y: number];

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
const LABELS: Record<string, string> = {
  air_bubble: "Bubbles",
  flake_food: "Flakes",
  sinking_pellet: "Pellets",
  bloodworm: "Bloodworms",
  algae_wafer: "Algae wafers",
  root_tab: "Root tabs",
  co2_bubble: "CO₂",
  monte_carlo: "Monte Carlo",
};
// What holds a plant back, by `Limit::key` (the inspect card).
const LIMITS: Record<string, string> = {
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
// The one web tank: its saves in IndexedDB and its Web Lock / channel.
const TANK = "main";
const LOCK = "desktank.tank.main";
const AUTOSAVE_MS = 5 * 60 * 1000;
const NOTICE_MS = 6000;
const DEFAULTS: Readonly<Settings> = Object.freeze({
  scale: 0, fps: "auto", field: "", avoidCursor: false, easyMode: false, stats: true, seed: 1, radius: 3,
});

const params = new URLSearchParams(location.search);
const wantBackend = params.get("backend") ?? "auto";
const bench = params.has("bench");

// The page's elements are all in index.html, so a lookup never misses.
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const canvas = $<HTMLCanvasElement>("tank");
const statusEl = $("status");
const statsEl = $("stats");
const inspectEl = $("inspect");
const cheatForm = $<HTMLFormElement>("cheat");
const cheatInput = $<HTMLInputElement>("cheat-input");
const cheatOut = $<HTMLOutputElement>("cheat-out");
const nameForm = $<HTMLFormElement>("name");
const nameInput = $<HTMLInputElement>("name-input");
const nameOut = $<HTMLOutputElement>("name-out");
const playBtn = $<HTMLButtonElement>("play");
const stepBtn = $<HTMLButtonElement>("step");
const seedInput = $<HTMLInputElement>("seed");
const brushInput = $<HTMLInputElement>("brush");
const paletteEl = $("palette");
const groupsEl = $("groups");
const plantPickEl = $<HTMLSelectElement>("plant-species");
const propagulePickEl = $<HTMLSelectElement>("plant-propagule");

let settings = loadSettings();
let tank: WebTank; // set by main() before anything uses it
let openGroup: string | undefined; // name of the palette group shown
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
// The creature inspect last showed: its inspect line's parts and its name.
interface Inspected {
  id: number;
  parts: string[];
  name: string;
}
let inspected: Inspected | null = null;
let wheelRest = 0;
let db: IDBDatabase | null = null; // IndexedDB, or null where there is none
let ownership: Ownership | null = null;
let owner = false; // this tab simulates and saves the tank
let epoch: number | null = null; // this owner's claim (store.ts); writes need it
let handingOver = false;
let started = false; // the first tank is open
let nextAutosave = Infinity;
let saveFailed = false; // the last write failed: write again even if unchanged
let lastSaved = 0; // Date.now() of the last save written or loaded
let persistAsked = false;
let noticeTimer = 0;

// Test and benchmark hooks (tests/web/, hooks.d.ts).
const hooks: Hooks = {
  frameTimes: [],
  rafTimes: [],
  get tank() {
    return tank;
  },
  get backend() {
    return backend;
  },
  // [scale, columns, rows] of the current tank in the current canvas.
  get grid(): [number, number, number] {
    return tank && [tank.cellScale(), tank.width(), tank.height()];
  },
  get owner() {
    return owner;
  },
  get ownership() {
    return ownership;
  },
  lastEpoch: null, // the epoch this tab last claimed
  // A write with that epoch, bypassing the owner check (a stale owner).
  async lateWrite() {
    try {
      if (!db) return "no database";
      await writeSave(db, TANK, tank.exportSave(), tank.tickCount(), hooks.lastEpoch);
      return "written";
    } catch (err) {
      return (err as Error).message;
    }
  },
  writes: 0, // saves this tab wrote
  saving: Promise.resolve(false), // the latest write
  loaded: null, // { hash, tick, width, height } of the save opened, if any
  released: null, // { hash, tick } when this tab handed the tank over
  ownedAt: 0, // performance.now() when this tab last became the owner
  takeOverAt: 0, // performance.now() of the last Take over click
  get settings() {
    return { ...settings };
  },
  loadScene,
};
window.desktank = hooks;

// --- Settings (localStorage) ---------------------------------------------

function loadSettings(): Settings {
  // Whatever was stored: each field is checked below.
  let saved: Record<string, unknown> = {};
  try {
    saved = JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "{}") ?? {};
  } catch {
    // Private mode, blocked storage or a corrupt value: use the defaults.
  }
  const int = (v: unknown, lo: number, hi: number, d: number) =>
    typeof v === "number" && Number.isInteger(v) && v >= lo && v <= hi ? v : d;
  return {
    // Even scales only (or 1): an odd one saved by an older version rounds down.
    scale: ((s) => (s > 1 ? s & ~1 : s))(int(saved.scale, 0, 8, DEFAULTS.scale)),
    fps: typeof saved.fps === "string" && ["auto", "30", "15"].includes(saved.fps) ? saved.fps : DEFAULTS.fps,
    field: typeof saved.field === "string" && FIELDS.includes(saved.field) ? saved.field : DEFAULTS.field,
    avoidCursor: saved.avoidCursor === true,
    easyMode: saved.easyMode === true,
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

function update(changes: Partial<Settings>) {
  settings = { ...settings, ...changes };
  saveSettings();
  showSettings();
}

// Mirrors the settings into the panel's fields and applies the cheap ones.
function showSettings() {
  $<HTMLSelectElement>("set-scale").value = String(settings.scale);
  $<HTMLSelectElement>("set-fps").value = settings.fps;
  $<HTMLSelectElement>("set-field").value = settings.field;
  $<HTMLInputElement>("set-avoid-cursor").checked = settings.avoidCursor;
  $<HTMLInputElement>("set-easy-mode").checked = settings.easyMode;
  $<HTMLInputElement>("set-stats").checked = settings.stats;
  seedInput.value = String(settings.seed);
  brushInput.value = String(settings.radius);
  $<HTMLOutputElement>("brush-out").value = String(settings.radius);
  statsEl.hidden = !settings.stats;
  if (tank) {
    tank.setMaxFps(maxFps());
    tank.setFieldOverlay(settings.field);
    tank.setCursorAvoidance(settings.avoidCursor);
    tank.setEasyMode(settings.easyMode);
    tank.setRadius(settings.radius);
    kick();
  }
}

function maxFps() {
  return settings.fps === "auto" ? batteryFps : Number(settings.fps);
}

// --- Tank and graphics -------------------------------------------------------

function showError(message: string) {
  statusEl.textContent = message;
  statusEl.hidden = false;
}

// Canvas size in device pixels, from its CSS box.
function canvasPixels(): Point {
  const r = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  return [Math.max(1, Math.round(r.width * dpr)), Math.max(1, Math.round(r.height * dpr))];
}

function setCanvasSize([w, h]: Point) {
  if (canvas.width !== w) canvas.width = w;
  if (canvas.height !== h) canvas.height = h;
  tank?.resize(w, h);
  kick();
}

// Swaps in a new tank; the caller attaches graphics to it.
function replaceTank(next: WebTank) {
  // A new tank keeps the selection; the first starts on sand.
  const selected = tank?.selected() ?? next.selected();
  // It keeps the plant species and propagule picks too.
  if (tank) next.keepPlantPicks(tank);
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
async function attachGraphics(): Promise<boolean> {
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
function newTank(): WebTank {
  const [w, h] = sizeCanvas();
  const [, cols, rows] = gridFor(w, h, settings.scale);
  return new WebTank(cols, rows, settings.seed, w, h);
}

function sizeCanvas(): Point {
  const [w, h] = canvasPixels();
  canvas.width = w;
  canvas.height = h;
  return [w, h];
}

// Swaps in `next` as the shown tank and reopens the graphics for it.
async function showTank(next: WebTank) {
  endGestures();
  // Graphics belong to the old tank; reopen them for the new one.
  replaceTank(next);
  if (!owner) setRunning(false);
  // During a loss or recovery, the recovery attaches to the new tank.
  if (!restoring) await recover(false);
  updateStats();
  kick();
}

async function reset() {
  if (!owner) return;
  await showTank(newTank());
  persist("reset", true);
}

// --- Saves and ownership -----------------------------------------------------

// Shows a short message under the menu.
function notice(message: string) {
  const el = $("notice");
  el.textContent = message;
  el.hidden = false;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => {
    el.hidden = true;
  }, NOTICE_MS);
}

function showSaved() {
  const el = $("saved");
  if (!db) el.textContent = "Not saved: this browser has no storage here";
  else if (!lastSaved) el.textContent = "Not saved yet";
  else el.textContent = `Saved ${new Date(lastSaved).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
}

// Saves the tank to IndexedDB if this tab owns it and it changed (or
// `force`). Returns the write's promise (true once written).
function persist(why: string, force = false): Promise<boolean> {
  if (!owner || epoch === null || !db || !tank) return Promise.resolve(false);
  nextAutosave = performance.now() + AUTOSAVE_MS;
  if (!force && !saveFailed && !tank.saveNeeded()) return hooks.saving;
  const bytes = tank.save();
  const tick = tank.tickCount();
  hooks.writes++;
  let write: Promise<void>;
  try {
    write = writeSave(db, TANK, bytes, tick, epoch);
  } catch (err) {
    write = Promise.reject(err);
  }
  hooks.saving = write.then(
    () => {
      saveFailed = false;
      lastSaved = Date.now();
      showSaved();
      askPersist();
      return true;
    },
    (err) => {
      saveFailed = true;
      console.warn(`desktank: ${why} save failed: ${err}`);
      showSaved();
      return false;
    },
  );
  return hooks.saving;
}

// Asks the browser not to evict the saves (once, after the first save).
function askPersist() {
  if (persistAsked) return;
  persistAsked = true;
  navigator.storage?.persist?.().then(
    (granted) => {
      hooks.persisted = granted;
    },
    () => {},
  );
}

// The newest save that opens, as a tank for the current canvas, or null.
// Saves that don't open are logged and skipped, never deleted (one may be
// from a newer build); pruning drops them once newer saves replace them.
async function loadNewest(): Promise<WebTank | null> {
  if (!db) return null;
  let saves: Save[] = [];
  try {
    saves = await readSaves(db, TANK);
  } catch (err) {
    console.warn(`desktank: couldn't read the saves: ${err}`);
    return null;
  }
  const [w, h] = sizeCanvas();
  for (const save of saves) {
    try {
      const next = WebTank.fromSave(save.bytes, w, h);
      lastSaved = save.savedAt ?? 0;
      hooks.loaded = { hash: next.stateHash(), tick: next.tickCount(), width: next.width(), height: next.height() };
      return next;
    } catch (err) {
      console.warn(`desktank: a save doesn't open (${err}); skipped`);
    }
  }
  return null;
}

// This tab now owns the tank: open the newest save (the previous owner
// saved it when it handed over) and run.
async function becomeOwner() {
  owner = true;
  epoch = null;
  if (db) {
    try {
      epoch = await claim(db, TANK);
      hooks.lastEpoch = epoch;
    } catch (err) {
      console.warn(`desktank: couldn't claim the tank: ${err}`);
    }
  }
  hooks.ownedAt = performance.now();
  showOwnership();
  if (!started) return;
  const next = await loadNewest();
  if (next) await showTank(next);
  if (owner) {
    setRunning(true);
    nextAutosave = performance.now() + AUTOSAVE_MS;
  }
}

// Another tab owns the tank: this one shows it paused and never writes.
function becomeViewer() {
  owner = false;
  epoch = null;
  handingOver = false;
  showOwnership();
  if (tank) setRunning(false);
}

// The owner was asked to hand over: save first (and let a write in flight
// finish), so the next owner opens exactly this state.
async function handOver() {
  // Stop simulating and editing first, so the save is the final state.
  handingOver = true;
  showOwnership();
  endGestures();
  if (tank) setRunning(false);
  const forced = saveFailed;
  await persist("hand-over");
  if (!(await hooks.saving) && (forced || tank?.saveNeeded())) {
    notice("Couldn't save before handing over: the other tab opened the last save.");
  }
  if (tank) hooks.released = { hash: tank.stateHash(), tick: tank.tickCount() };
}

function showOwnership() {
  const active = owner && !handingOver;
  $("elsewhere").hidden = active || !started;
  $("elsewhere-text").textContent = handingOver
    ? "Handing the tank over to another tab…"
    : "DeskTank is open in another tab. This one shows its last save and won't change it.";
  $<HTMLButtonElement>("take-over").disabled = handingOver;
  document.body.dataset.owner = String(owner);
  // A viewer's controls are inert: nothing it does would be saved.
  for (const el of document.querySelectorAll<HTMLElement>("[data-owner-only]")) el.inert = !active;
  if (!active) {
    closeCheat();
    closeName();
  }
}

function takeOver() {
  hooks.takeOverAt = performance.now();
  $<HTMLButtonElement>("take-over").disabled = true;
  $("elsewhere-text").textContent = "Taking over…";
  ownership?.takeOver();
}

function exportSave() {
  const bytes = tank.exportSave();
  const stamp = new Date().toISOString().slice(0, 16).replace(/[-:]/g, "").replace("T", "-");
  const a = document.createElement("a");
  // wasm-bindgen copies the bytes out of wasm memory into a fresh ArrayBuffer.
  a.href = URL.createObjectURL(new Blob([bytes as Uint8Array<ArrayBuffer>], { type: "application/octet-stream" }));
  a.download = `desktank-${stamp}.dtank`;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
}

// Opens a save file (from Export, or a desktop `.dtank`) at its own grid
// size, keeping play/pause, and saves it as this browser's tank.
async function importSave(file: File | undefined) {
  if (!owner || !file) return;
  let next;
  try {
    const bytes = new Uint8Array(await file.arrayBuffer());
    // The tab may have handed the tank over while the file was read.
    if (!owner) return;
    const [w, h] = sizeCanvas();
    next = WebTank.fromSave(bytes, w, h);
  } catch (err) {
    notice(`Couldn't import ${file.name}: ${err}`);
    return;
  }
  const running = tank.running();
  await showTank(next);
  setRunning(running);
  notice(`Imported ${file.name}: ${next.width()}×${next.height()} cells`);
  await persist("import", true);
}

// Loads a `desktank capture` script (TOML) at a fixed canvas size, paused,
// for the render comparison test.
async function loadScene(toml: string, sceneSeed: number, width: number, height: number) {
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
  const [scale, w, h] = hooks.grid;
  statsEl.textContent =
    `${backend || "no GPU"} · fps ${fps} · tick ${tank.tickCount()} · awake chunks ` +
    `${tank.awakeChunks()}/${tank.chunkCount()} · tick time ${tickMs.toFixed(2)} ms` +
    ` · ${w}×${h} cells at ${scale}×`;
}

// --- Loop --------------------------------------------------------------------

// One callback: the tank decides whether a tick or a frame is due, and when
// it next needs us.
function loop(now: number) {
  rafId = 0;
  if (hidden || !tank) return;
  if (now >= nextAutosave) persist("autosave");
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
function sleepUntil(t: number) {
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

function setRunning(on: boolean) {
  // Only the owner simulates.
  on &&= owner;
  tank.setRunning(on);
  playBtn.textContent = on ? "Pause" : "Play";
  stepBtn.disabled = on;
  kick();
}

function step() {
  if (!owner) return;
  tank.step();
  updateStats();
  kick();
}

// --- Palette -----------------------------------------------------------------

function label(name: string) {
  return LABELS[name] ?? name.charAt(0).toUpperCase() + name.slice(1).replaceAll("_", " ");
}

// The plants slot pours the species picked here (the sim's list), as the
// propagule picked in the second picker (only the species' own methods).
function buildPlantPicker() {
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
  plantPickEl.value = String(tank.plantSpeciesSelected());
  buildPropagulePicker();
}

function buildPropagulePicker() {
  const ways = tank.plantPropagules();
  propagulePickEl.replaceChildren(
    ...ways.map((name) => {
      const o = document.createElement("option");
      o.value = name;
      o.textContent = label(name);
      return o;
    }),
  );
  propagulePickEl.value = tank.plantPropagule();
  // A species with one method has nothing to pick.
  propagulePickEl.disabled = ways.length < 2;
}

function pickPlant() {
  tank.setPlantSpecies(Number(plantPickEl.value));
  buildPropagulePicker();
  choosePlantSlot();
}

function pickPropagule() {
  tank.setPlantPropagule(propagulePickEl.value);
  choosePlantSlot();
}

// Selects the plants slot (after a pick), with the species' swatch.
function choosePlantSlot() {
  const slot = tank.palette().findIndex((e) => e.startsWith("plant "));
  const colour = tank.paletteColours()[slot];
  const button = swatch(slot);
  if (button) {
    (button.firstElementChild as HTMLElement | null)?.style.setProperty("--c", colour);
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
  // "name label" per entry: the group's name and its tab's text.
  const groups = tank.paletteGroups().map((g) => g.split(" ") as [name: string, text: string]);
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

function choose(i: number, focus = false) {
  tank.select(i);
  showSelection(focus);
  kick();
}

// Shows group `name`'s items (the selection stays as it is).
function showGroup(name: string | undefined, focus = false) {
  openGroup = name;
  showSelection();
  if (focus) $(`group-${name}`)?.focus();
}

// Opens the group `steps` after (or before) the open one, wrapping.
function stepGroup(steps: number, focus = false) {
  const tabs = groupTabs();
  const at = tabs.findIndex((t) => t.dataset.group === openGroup);
  const n = tabs.length;
  showGroup(tabs[(((at + steps) % n) + n) % n].dataset.group, focus);
}

// The open group's buttons.
// The group tabs and the palette's swatches, all buttons (buildPalette).
const groupTabs = () => [...groupsEl.children] as HTMLButtonElement[];
const swatches = () => [...paletteEl.children] as HTMLButtonElement[];
const swatch = (i: number) => paletteEl.children[i] as HTMLButtonElement | undefined;
const openItems = () => swatches().filter((b) => !b.hidden);

// Roving tabindex: one tab and one radio are in the tab order, the open
// tab and the checked radio (or, if the selection is in a closed group,
// the open group's first).
function showSelection(focus = false) {
  const selected = tank.selected();
  const holder = swatch(selected)?.dataset.group;
  for (const t of groupTabs()) {
    const open = t.dataset.group === openGroup;
    t.setAttribute("aria-selected", String(open));
    t.tabIndex = open ? 0 : -1;
    t.toggleAttribute("data-holds", !open && t.dataset.group === holder);
  }
  $("palette-panel").setAttribute("aria-labelledby", `group-${openGroup}`);
  swatches().forEach((b, i) => {
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
function chooseSlot(n: number, focus = false) {
  const b = openItems()[n];
  if (b) choose(swatches().indexOf(b), focus);
}

// Arrow keys: within the tabs, open the next or previous group; within the
// radio group, select the next or previous item of the open group. Home and
// End jump to the first and last.
function arrowStep(e: KeyboardEvent, at: number, n: number): number | undefined {
  const steps: Record<string, number> = { ArrowRight: at + 1, ArrowDown: at + 1, ArrowLeft: at - 1, ArrowUp: at - 1, Home: 0, End: n - 1 };
  return steps[e.key];
}

function groupKeys(e: KeyboardEvent) {
  const tabs = groupTabs();
  const next = arrowStep(e, Math.max(0, tabs.indexOf(document.activeElement as HTMLButtonElement)), tabs.length);
  if (next === undefined) return;
  e.preventDefault();
  e.stopPropagation();
  const n = tabs.length;
  showGroup(tabs[(next + n) % n].dataset.group, true);
}

function paletteKeys(e: KeyboardEvent) {
  const items = openItems();
  const n = items.length;
  const next = arrowStep(e, Math.max(0, items.indexOf(document.activeElement as HTMLButtonElement)), n);
  if (next === undefined || !n) return;
  e.preventDefault();
  e.stopPropagation();
  chooseSlot((next + n) % n, true);
}

// --- Input -------------------------------------------------------------------

function canvasPoint(e: PointerEvent): Point {
  const rect = canvas.getBoundingClientRect();
  return [((e.clientX - rect.left) / rect.width) * canvas.width, ((e.clientY - rect.top) / rect.height) * canvas.height];
}

// Shows a line in the inspect readout for a few seconds (longer while its
// name field is in use).
function flash(text: string) {
  inspectEl.textContent = text;
  inspectEl.hidden = false;
  clearTimeout(inspectTimer);
  inspectTimer = setTimeout(fade, INSPECT_SHOW_MS);
}

function fade() {
  if (document.activeElement === nameInput) {
    inspectTimer = setTimeout(fade, INSPECT_SHOW_MS);
    return;
  }
  inspectEl.hidden = true;
  closeName();
}

// The name field (SPEC §2.4, T9): shown with a creature's card for the
// owner; N or a click focuses it, Enter names the creature (blank removes
// its name), Escape closes it.
function closeName() {
  nameForm.hidden = true;
  if (document.activeElement === nameInput) nameInput.blur();
}

function runName(e: SubmitEvent) {
  e.preventDefault();
  if (!owner || handingOver || !inspected) return closeName();
  try {
    inspected.name = tank.nameCreature(inspected.id, nameInput.value);
    nameInput.value = inspected.name;
    nameOut.textContent = inspected.name ? "Named · Enter renames · Esc closes" : "No name · Esc closes";
    nameInput.blur();
    flash(creatureText(inspected));
  } catch (err) {
    nameOut.textContent = String(err);
    inspected = null;
  }
}

// A creature's card, from its inspect line's parts and its name.
function creatureText({ parts, name }: Inspected) {
  const [species, id, over, x, y, stage, age, health, energy, sex, morph, carrying] = parts;
  const days = Number(age);
  const old = days === 0 ? "under a day old" : `${days} day${days === 1 ? "" : "s"} old`;
  const card = [];
  if (name) card.push(name);
  card.push(`${label(species)} #${id}`, `${stage} ${sex}`, old, `health ${health}%`, `energy ${energy}%`);
  if (morph !== "-") card.push(label(morph));
  if (carrying !== "-") card.push(`carrying, ${carrying} day${carrying === "1" ? "" : "s"} to go`);
  card.push(`over ${label(over)}`, `cell ${x}, ${y}`);
  return card.join(" · ");
}

// The cheat field: the Konami code opens it, Enter runs the line (closing
// it if the cheat ran), Escape closes it. Only listeners run while it is
// shut; nothing polls.
function openCheat() {
  cheatInput.value = "";
  cheatOut.textContent = "Enter runs · Esc closes · try help";
  cheatForm.hidden = false;
  cheatInput.focus();
}

function closeCheat() {
  cheatForm.hidden = true;
  if (document.activeElement === cheatInput) cheatInput.blur();
}

function runCheat(e: SubmitEvent) {
  e.preventDefault();
  if (!owner || handingOver) return closeCheat();
  try {
    const done = tank.cheat(cheatInput.value);
    closeCheat();
    inspected = null;
    closeName();
    flash(`Cheat: ${done}`);
    kick();
  } catch (err) {
    cheatOut.textContent = String(err);
    cheatInput.select();
  }
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
    // The name is last and may hold spaces.
    inspected = { id: Number(rest[1]), parts: rest.slice(0, 12), name: rest.slice(12).join(" ") };
    flash(creatureText(inspected));
    if (owner && !handingOver) {
      nameInput.value = inspected.name;
      nameOut.textContent = "N or click to name · Enter names it · Esc closes";
      nameForm.hidden = false;
    }
    return;
  } else {
    return;
  }
  inspected = null;
  closeName();
  flash(inspectEl.textContent);
}

function after() {
  showInspection();
  kick();
}

interface PointerHandler {
  down(e: PointerEvent): void;
  move(e: PointerEvent): void;
  up(e: PointerEvent): void;
}

// Mouse and pen: buttons map straight onto the tools.
const mouse: PointerHandler = {
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
const touches = new Map<number, Point>(); // pointerId -> [x, y] canvas px
interface Gesture {
  mode: "pending" | "stroke" | "scoop" | "done";
  id: number; // the first finger's pointerId
  start: Point;
  timer: number;
}
let gesture: Gesture | null = null;

function midpoint(): Point {
  const [a, b] = [...touches.values()];
  return [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
}

const touch: PointerHandler = {
  down(e) {
    canvas.setPointerCapture(e.pointerId);
    touches.set(e.pointerId, canvasPoint(e));
    if (touches.size === 1) {
      const start = touches.get(e.pointerId)!;
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

const handler = (e: PointerEvent) => (e.pointerType === "touch" ? touch : mouse);

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
  const up = (e: PointerEvent) => {
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
  propagulePickEl.addEventListener("change", pickPropagule);
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
  $("export").addEventListener("click", exportSave);
  const importFile = $<HTMLInputElement>("import-file");
  $("import").addEventListener("click", () => importFile.click());
  importFile.addEventListener("change", () => {
    const file = importFile.files?.[0];
    importFile.value = "";
    importSave(file).catch((err) => showError(String(err)));
  });
  $("take-over").addEventListener("click", takeOver);
  cheatForm.addEventListener("submit", runCheat);
  nameForm.addEventListener("submit", runName);
  nameInput.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      closeName();
    }
  });
  cheatInput.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      closeCheat();
    }
  });
  document.addEventListener("keydown", (e) => {
    if (!owner) return;
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    // The page has keyboard focus, so the Konami code needs no chord.
    if (!e.repeat && !handingOver && tank.cheatKey(e.key)) {
      e.preventDefault();
      openCheat();
      return;
    }
    if (e.key === " " && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      setRunning(!tank.running());
    } else if (e.key === "." && !tank.running()) {
      step();
    } else if (e.key === "[" || e.key === "]") {
      update({ radius: tank.resizeBrush(e.key === "]" ? 1 : -1) });
    } else if ((e.key === "n" || e.key === "N") && !nameForm.hidden) {
      e.preventDefault();
      nameInput.focus();
      nameInput.select();
    } else if (e.key === "f" || e.key === "F") {
      const next = FIELDS[(FIELDS.indexOf(settings.field) + 1) % FIELDS.length];
      update({ field: next });
    } else if (e.key === "g" || e.key === "G") {
      // G opens the next group, Shift+G the previous (as in desktank window;
      // Shift, not the letter's case, so Caps Lock doesn't flip it). Focus
      // in the palette moves to the new tab, as its radio may be hidden.
      const target = e.target as Node | null;
      const inPalette = paletteEl.contains(target) || groupsEl.contains(target);
      stepGroup(e.shiftKey ? -1 : 1, inPalette);
    } else if (/^[0-9]$/.test(e.key)) {
      // 1–9 are the open group's first nine slots, 0 the tenth.
      chooseSlot((Number(e.key) + 9) % 10);
    }
  });
}

function wireSettings(resetTank: () => void) {
  const toggle = $("settings-toggle");
  toggle.addEventListener("click", () => {
    const open = toggle.getAttribute("aria-expanded") !== "true";
    toggle.setAttribute("aria-expanded", String(open));
    $("settings").hidden = !open;
  });
  // The settings' <select>s and checkboxes.
  const valueOf = (e: Event) => (e.target as HTMLSelectElement).value;
  const checkedOf = (e: Event) => (e.target as HTMLInputElement).checked;
  $("set-scale").addEventListener("change", (e) => {
    update({ scale: Number(valueOf(e)) });
    resetTank();
  });
  $("set-fps").addEventListener("change", (e) => update({ fps: valueOf(e) }));
  $("set-field").addEventListener("change", (e) => update({ field: valueOf(e) }));
  $("set-avoid-cursor").addEventListener("change", (e) => update({ avoidCursor: checkedOf(e) }));
  $("set-easy-mode").addEventListener("change", (e) => update({ easyMode: checkedOf(e) }));
  $("set-stats").addEventListener("change", (e) => {
    update({ stats: checkedOf(e) });
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
  // A hidden page is also the last reliable moment to save (SPEC §3.5).
  document.addEventListener("visibilitychange", () => {
    hidden = document.hidden;
    if (hidden) {
      tank?.pause();
      stopLoop();
      persist("hidden");
    } else {
      kick();
    }
  });
  // Best effort: usually already saved by visibilitychange.
  window.addEventListener("pagehide", () => persist("pagehide"));

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
  brushInput.min = String(rMin);
  brushInput.max = String(rMax);
  db = await openStore();
  // Without Web Locks (very old browsers) the tab owns the tank alone.
  if (navigator.locks && typeof BroadcastChannel !== "undefined") {
    ownership = new Ownership(LOCK, { own: becomeOwner, view: becomeViewer, handOver });
    await ownership.start();
  } else {
    await becomeOwner();
  }
  const ownedAtStart = owner;
  try {
    replaceTank((await loadNewest()) ?? newTank());
    if (!owner) setRunning(false);
    await attachGraphics();
  } catch (err) {
    showError(`Couldn't start the GPU renderer: ${err}`);
    return;
  }
  started = true;
  // Granted the lock while starting (the owner closed): open its last save.
  if (owner && !ownedAtStart) await becomeOwner();
  nextAutosave = owner ? performance.now() + AUTOSAVE_MS : Infinity;
  showOwnership();
  showSaved();
  wireInput();
  wireLifecycle();
  statusEl.hidden = true;
  fpsSince = performance.now();
  updateStats();
  document.body.dataset.ready = "true";
  kick();
}

main();
