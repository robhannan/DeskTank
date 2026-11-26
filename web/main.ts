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
// window.desktank.frameTimes (scripts/bench-web.ts); ?dpr=<n> caps the
// canvas's device pixels per CSS pixel (1 by default on the webOS build);
// ?debug=1 lists the startup steps and any error on screen.
//
// The webOS build (`pnpm build:web -- --legacy`, SPEC D26) runs this same
// page on Chrome 68: <html data-build="webos">, MVP wasm, lowered syntax.
// Keep to Chrome 68's APIs or feature-detect (docs/notes/web-and-webos.md).
import init, { Graphics, WebTank, brushRange, gridFor, zoomLevels } from "./pkg/desktank_web.js";
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
// The tank's simulation tick (desktank_sim::TICKS_PER_SECOND, 20 Hz).
const TICK_MS = 50;
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
  under_gravel_filter: "Under-gravel filter",
  under_gravel_panel: "Under-gravel panel",
  air_bubble: "Bubbles",
  flake_food: "Flakes",
  sinking_pellet: "Pellets",
  bloodworm: "Frozen bloodworms",
  live_bloodworm: "Live bloodworms",
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
const FIELDS = ["", "nutrient", "co2", "light", "flow"];

const SETTINGS_KEY = "desktank.settings.v1";
// The one web tank: its saves in IndexedDB and its Web Lock / channel.
const TANK = "main";
const LOCK = "desktank.tank.main";
const AUTOSAVE_MS = 5 * 60 * 1000;
const NOTICE_MS = 6000;
const DEFAULTS: Readonly<Settings> = Object.freeze({
  scale: 0, fps: "auto", field: "", avoidCursor: false, easyMode: false, catchUp: false, sway: false, swayOnBattery: false, stats: true, seed: 1, radius: 3,
});

const params = new URLSearchParams(location.search);
const wantBackend = params.get("backend") ?? "auto";
const bench = params.has("bench");
const debug = params.get("debug") === "1";
// "webos" on the legacy-Chromium TV build (scripts/build-web.ts --legacy).
const build = document.documentElement.dataset.build ?? "";
// Device pixels per CSS pixel the canvas may use: ?dpr=, else 1 on the webOS
// build (a 1080p webOS 5 TV reports 2, which would draw a 4K canvas on its
// small GPU), else the display's own.
const dprCap = ((v) => (v > 0 ? v : build === "webos" ? 1 : Infinity))(Number(params.get("dpr")));
// Draw the grid in more passes with smaller shaders (?grid=split or
// ?grid=single, else split on the webOS build): Chrome 68's SwiftShader, and
// it seems the TV's driver, can't compile the one-pass shaders.
const splitGrid: boolean | undefined = params.has("grid")
  ? params.get("grid") === "split"
  : build === "webos"
    ? true
    : undefined;
// An sRGB canvas format (?srgb=1 or 0; not on the webOS build): the same
// colours either way, but WebGL2 presents a non-sRGB canvas with a blit,
// where wgpu's sRGB present shader draws nothing on Chrome 68's SwiftShader.
const srgbCanvas = params.has("srgb") ? params.get("srgb") === "1" : build !== "webos";

// The page's elements are all in index.html, so a lookup never misses.
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const canvas = $<HTMLCanvasElement>("tank");
const statusEl = $("status");
const statsEl = $("stats");
const inspectEl = $("inspect");
const inspectSr = $("inspect-sr");
const cheatForm = $<HTMLFormElement>("cheat");
const cheatInput = $<HTMLInputElement>("cheat-input");
const cheatOut = $<HTMLOutputElement>("cheat-out");
const nameForm = $<HTMLFormElement>("name");
const nameInput = $<HTMLInputElement>("name-input");
const nameOut = $<HTMLOutputElement>("name-out");
const deviceActions = $("device-actions");
const deviceRinseBtn = $<HTMLButtonElement>("device-rinse");
const deviceHardRinseBtn = $<HTMLButtonElement>("device-hard-rinse");
const deviceReplaceBtn = $<HTMLButtonElement>("device-replace");
const deviceGuardLabel = $("device-guard-label");
const deviceGuardInput = $<HTMLInputElement>("device-guard");
const playBtn = $<HTMLButtonElement>("play");
const stepBtn = $<HTMLButtonElement>("step");
const seedInput = $<HTMLInputElement>("seed");
const capInput = $<HTMLInputElement>("set-creature-cap");
const capStockOut = $<HTMLOutputElement>("set-creature-cap-stock");
const brushInput = $<HTMLInputElement>("brush");
const zoomSelect = $<HTMLSelectElement>("zoom");
const flipBtn = $<HTMLButtonElement>("flip");
const paletteEl = $("palette");
const paletteBox = $("palette-box");
const tabsEl = $("tabs");
const subtabsEl = $("subtabs");
const propagulesEl = $("propagules");
const morphsEl = $("morphs");

let settings = loadSettings();
let tank: WebTank; // set by main() before anything uses it
// Each palette item's place in the category tree (WebTank.paletteCategories).
interface Place {
  path: string; // "fish/schooling", or "water" for a tab's only sub-category
  tab: string; // the path's first part
  tabLabel: string;
  subLabel: string;
}
let places: Place[] = [];
let openTab: string | undefined; // the palette tab shown
let openSub: string | undefined; // and its sub-category's path
let backend = "";
let adapter = "";
let rafId = 0;
let timerId = 0;
let hidden = document.hidden;
let hiddenAt: number | null = null; // when the current owned tank went hidden
let batteryFps = FPS_AC;
let onBattery = false;
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
// The device inspect last showed (D27, DV3): its id for Rinse / guard actions.
let inspectedDeviceId: number | null = null;
let inspectedDeviceCell: [string, string] | null = null;
let wheelRest = 0;
let stripRest = 0; // the same over the palette, which it steps through
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
  lastCatchUp: null,
  released: null, // { hash, tick } when this tab handed the tank over
  ownedAt: 0, // performance.now() when this tab was granted the lock
  stole: false,
  claiming: Promise.resolve(),
  takeOverAt: 0, // performance.now() of the last Take over click
  get settings() {
    return { ...settings };
  },
  startup: [],
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
    catchUp: saved.catchUp === true,
    sway: saved.sway === true,
    swayOnBattery: saved.swayOnBattery === true,
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
  $<HTMLInputElement>("set-catch-up").checked = settings.catchUp;
  $<HTMLInputElement>("set-sway").checked = settings.sway;
  $<HTMLInputElement>("set-sway-on-battery").checked = settings.swayOnBattery;
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
    tank.setSway(settings.sway);
    tank.setAllowSwayOnBattery(settings.swayOnBattery);
    tank.setOnBattery(onBattery);
    tank.setHidden(hidden);
    tank.setRadius(settings.radius);
    showCreatureCap();
    kick();
  }
}

// Mirrors the tank's own population cap (saved in .dtank, not localStorage)
// into the Settings field and stock readout.
function showCreatureCap(forceValue = false) {
  if (!tank) return;
  const stock = tank.stockCount();
  const cap = tank.creatureCap();
  capInput.min = String(Math.max(1, stock));
  capInput.max = String(tank.maxCreatureCap());
  if (forceValue || document.activeElement !== capInput) capInput.value = String(cap);
  capStockOut.value = `(${stock}/${cap})`;
}

function maxFps() {
  return settings.fps === "auto" ? batteryFps : Number(settings.fps);
}

// --- Tank and graphics -------------------------------------------------------

function showError(message: string) {
  // A real error stays: no stall report over it later.
  clearTimeout(stallTimer);
  stallTimer = 0;
  stallShown = false;
  statusEl.textContent = message;
  statusEl.hidden = false;
}

// A GPU error (a shader the driver rejects leaves the tank unpainted) is
// shown once, with the first lines of wgpu's message.
let gpuErrorShown = false;
function showGpuError() {
  const err = tank.gpuError();
  if (!err) return;
  gpuErrorShown = true;
  const brief = err.split("\n").slice(0, 6).join("\n");
  note(`GPU error: ${brief}`);
  showError(`DeskTank's GPU renderer failed${splitGrid ? "" : " (try ?grid=split)"}:\n${brief}`);
}

// Element.replaceChildren is Chrome 86+; the webOS build runs on Chrome 68.
function setChildren(el: Element, ...children: Node[]) {
  el.textContent = "";
  el.append(...children);
}

// Canvas pixels per CSS pixel: the display's, at most dprCap (SPEC D26).
function pixelRatio() {
  return Math.min(window.devicePixelRatio || 1, dprCap);
}

// Canvas size in device pixels, from its CSS box.
function canvasPixels(): Point {
  const r = canvas.getBoundingClientRect();
  const dpr = pixelRatio();
  return [Math.max(1, Math.round(r.width * dpr)), Math.max(1, Math.round(r.height * dpr))];
}

function setCanvasSize([w, h]: Point) {
  if (canvas.width !== w) canvas.width = w;
  if (canvas.height !== h) canvas.height = h;
  tank?.resize(w, h);
  kick();
}

// Swaps in a new tank; it takes over the old tank's graphics where it can,
// otherwise the caller attaches graphics to it.
function replaceTank(next: WebTank) {
  // A new tank keeps the selection; the first starts on sand.
  const selected = tank?.selected() ?? next.selected();
  const zoom = tank?.zoom() ?? next.zoom();
  // It also keeps each species’ palette picks.
  if (tank) {
    next.keepPlantPicks(tank);
    next.keepCreaturePicks(tank);
    // Reuse the graphics and compiled pipelines (a lost context can't be).
    if (!contextLost && !restoring) next.takeGraphics(tank);
  }
  tank?.free();
  tank = next;
  hiddenAt = owner && !handingOver && hidden ? Date.now() : null;
  tank.select(selected);
  tank.setZoom(zoom);
  showSettings();
  showCreatureCap(true);
  showZoom();
  setRunning(tank.running());
  buildPalette();
}

// Opens graphics and attaches them to the current tank. An open that a
// context loss overtook is discarded and retried once the context is back;
// returns false if the context is lost now (the restore event reattaches).
async function attachGraphics(): Promise<boolean> {
  for (;;) {
    if (contextLost) return false;
    const losses = lossCount;
    const g = await Graphics.open(canvas, backend || wantBackend, splitGrid, srgbCanvas);
    if (losses === lossCount && !contextLost) {
      // New graphics report their own errors.
      gpuErrorShown = false;
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
  // The new tank takes the old one's graphics; reopen them only if it
  // couldn't (none yet, or a lost context).
  replaceTank(next);
  if (!owner) setRunning(false);
  // During a loss or recovery, the recovery attaches to the new tank.
  if (!tank.backend() && !restoring) await recover(false);
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

function catchUp(next: WebTank, since: number) {
  if (!owner || handingOver || !settings.catchUp || !Number.isFinite(since)) return;
  // Clamp to the wasm argument range before converting. The tank applies
  // the catch-up floor and cap shared with the desktop window.
  const elapsed = Math.max(0, Math.min(0xffffffff, Math.floor((Date.now() - since) / 1000)));
  const [seconds, births, deaths] = next.catchUp(elapsed);
  if (seconds === 0) return;
  hooks.lastCatchUp = { seconds, births, deaths };
  const minutes = Math.floor(seconds / 60);
  const time = minutes < 60 ? `${minutes}m` : `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
  console.info(`desktank web: caught up ${time} (${births} birth${births === 1 ? "" : "s"}, ${deaths} death${deaths === 1 ? "" : "s"})`);
}

// The newest save that opens, as a tank for the current canvas, or null.
// Saves that don't open are logged and skipped, never deleted (one may be
// from a newer build); pruning drops them once newer saves replace them.
async function loadNewest(keep = () => true): Promise<WebTank | null> {
  if (!db) return null;
  const ownedAtLoad = owner;
  let saves: Save[] = [];
  try {
    saves = await readSaves(db, TANK);
  } catch (err) {
    console.warn(`desktank: couldn't read the saves: ${err}`);
    return null;
  }
  if (!keep()) return null;
  const [w, h] = sizeCanvas();
  for (const save of saves) {
    let next: WebTank;
    try {
      next = WebTank.fromSave(save.bytes, w, h);
    } catch (err) {
      console.warn(`desktank: a save doesn't open (${err}); skipped`);
      continue;
    }
    lastSaved = save.savedAt ?? 0;
    hooks.loaded = { hash: next.stateHash(), tick: next.tickCount(), width: next.width(), height: next.height() };
    // If the lock arrived during a viewer's read, the owning read will
    // open the latest save again.
    if (ownedAtLoad) {
      next.setEasyMode(settings.easyMode);
      catchUp(next, save.savedAt);
    }
    return next;
  }
  return null;
}

// This tab now owns the tank: open the newest save (the previous owner
// saved it when it handed over) and run.
function becomeOwner(stolen = false) {
  hooks.ownedAt = performance.now();
  hooks.stole = stolen;
  owner = true;
  epoch = null;
  hooks.claiming = claimOwnership();
  return hooks.claiming;
}

async function claimOwnership() {
  if (db) {
    try {
      const claimed = await claim(db, TANK);
      if (!owner) return;
      epoch = claimed;
      hooks.lastEpoch = claimed;
    } catch (err) {
      console.warn(`desktank: couldn't claim the tank: ${err}`);
    }
  }
  showOwnership();
  if (owner && started) await resumeOwnership();
}

async function resumeOwnership() {
  const next = await loadNewest(() => owner);
  if (next) await showTank(next);
  if (owner) {
    if (hidden && hiddenAt === null) hiddenAt = Date.now();
    setRunning(true);
    nextAutosave = performance.now() + AUTOSAVE_MS;
  }
}

// Another tab owns the tank: this one shows it paused and never writes.
function becomeViewer() {
  owner = false;
  hiddenAt = null;
  epoch = null;
  handingOver = false;
  showOwnership();
  if (tank) {
    setRunning(false);
    if (drewFirst) stopLoop();
  }
}

// The owner was asked to hand over: save first (and let a write in flight
// finish), so the next owner opens exactly this state.
async function handOver() {
  // Stop simulating and editing first, so the save is the final state.
  handingOver = true;
  showOwnership();
  endGestures();
  if (tank) {
    setRunning(false);
    if (drewFirst) stopLoop();
  }
  // A visible owner rewrites the save even when clean, so its savedAt says
  // the tank was live until now and the next owner doesn't catch up. A
  // hidden owner keeps the savedAt from when it stopped, so the next owner
  // catches up for the hidden time.
  await persist("hand-over", !hidden);
  if (!(await hooks.saving) && (saveFailed || tank?.saveNeeded())) {
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
  capInput.disabled = !active;
  if (!active) {
    closeCheat();
    closeName();
    closeDeviceActions();
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
    // Response, not file.arrayBuffer(): Chrome 68 (webOS) lacks Blob.arrayBuffer.
    const bytes = new Uint8Array(await new Response(file).arrayBuffer());
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
  if (!tank.backend()) await attachGraphics();
  kick();
  const drawn = tank.framesDrawn();
  while (tank.framesDrawn() === drawn) {
    await new Promise((r) => requestAnimationFrame(r));
  }
}

function updateStats() {
  showCreatureCap();
  if (!settings.stats || !tank) return;
  const [scale, w, h] = hooks.grid;
  statsEl.textContent =
    `${backend || "no GPU"} · fps ${fps} · tick ${tank.tickCount()} · awake chunks ` +
    `${tank.awakeChunks()}/${tank.chunkCount()} · creatures ${tank.stockCount()}/${tank.creatureCap()} · tick time ${tickMs.toFixed(2)} ms` +
    ` · ${w}×${h} cells at ${scale}×`;
}

// --- Startup diagnostics -----------------------------------------------------

// For browsers without developer tools at hand (the webOS TV build, SPEC
// D26): main() names each startup step in hooks.startup, and a step stalled
// for STALL_MS is reported in the status line with the steps so far. With
// ?debug=1 the steps, visibility changes and any uncaught error are listed
// on screen as they happen. A few lines per page load, so the list is short.
const STALL_MS = 10_000;
const MAX_NOTES = 60;
let stallTimer = 0;
let stallShown = false; // the status line shows a stall, not an error
let looped = false; // the loop has run once
let drewFirst = false; // the first frame is drawn
let debugEl: HTMLElement | null = null;

function note(line: string) {
  if (hooks.startup.length >= MAX_NOTES) return;
  hooks.startup.push(`${Math.round(performance.now())} ms ${line}`);
  if (debugEl) debugEl.textContent = hooks.startup.join("\n");
}

// Starts startup step `name`; if it is still the current step after
// STALL_MS, the status line says so. `watch` false notes it unwatched.
function startStep(name: string, watch = true) {
  note(name);
  clearTimeout(stallTimer);
  stallTimer = 0;
  if (!watch) return;
  stallTimer = setTimeout(() => {
    showError(`DeskTank stalled at "${name}" for ${STALL_MS / 1000} s.\n${hooks.startup.join("\n")}`);
    stallShown = true;
  }, STALL_MS);
}

function endSteps(line: string) {
  note(line);
  clearTimeout(stallTimer);
  stallTimer = 0;
  if (stallShown) statusEl.hidden = true;
  stallShown = false;
}

function startDiagnostics() {
  if (debug) {
    debugEl = document.createElement("pre");
    debugEl.id = "debug";
    debugEl.className = "debug";
    document.body.append(debugEl);
  }
  note(`${navigator.userAgent} · build ${build || "web"} · dpr ${window.devicePixelRatio} (cap ${dprCap}) · ${document.visibilityState}`);
  window.addEventListener("error", (e) => note(`error: ${e.message} (${e.filename}:${e.lineno})`));
  window.addEventListener("unhandledrejection", (e) => note(`unhandled rejection: ${e.reason}`));
  if (debug) document.addEventListener("visibilitychange", () => note(`visibility: ${document.visibilityState}`));
}

// --- Loop --------------------------------------------------------------------

// One callback: the tank decides whether a tick or a frame is due, and when
// it next needs us.
function loop(now: number) {
  // Whichever of the vsync and the tick deadline came first runs; drop the
  // other, as this call reschedules.
  cancelAnimationFrame(rafId);
  clearTimeout(timerId);
  rafId = 0;
  timerId = 0;
  if (hidden || !tank) return;
  if (!looped) {
    looped = true;
    note("loop running");
  }
  if (now >= nextAutosave) persist("autosave");
  if (bench) hooks.rafTimes.push(now);
  const t0 = performance.now();
  const flags = tank.frame(now);
  if (flags >> TICKS_SHIFT) tickMs = performance.now() - t0;
  if (!gpuErrorShown) showGpuError();
  if (flags & DREW) {
    if (!drewFirst) {
      drewFirst = true;
      endSteps(`first frame drawn (${canvas.width}×${canvas.height} px)`);
    }
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

// Runs the loop on the next vsync. A running tank also keeps a timer at
// most one tick away: a compositor that holds rAF back (SwiftShader on a
// busy machine, a slow first frame after the tab is shown) must not hold
// world time back with it, so whichever comes first runs the loop. Paused,
// the rAF alone will do.
function kick() {
  if (hidden || restoring || contextLost) return;
  if (!rafId) rafId = requestAnimationFrame(loop);
  if (!tank?.running()) {
    clearTimeout(timerId);
    timerId = 0;
  } else if (!timerId) sleepUntil(performance.now() + TICK_MS);
}

// Sleeps on a timer and then runs the loop directly (not through rAF, whose
// vsync timestamp can fall just before the tick and cost a second wake-up).
// Drawing outside rAF is fine: the canvas presents at the next composite.
function sleepUntil(t: number) {
  clearTimeout(timerId);
  timerId = setTimeout(() => {
    timerId = 0;
    if (!hidden && !restoring && !contextLost) loop(performance.now());
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
  return LABELS[name] ?? name.charAt(0).toUpperCase() + name.slice(1).replace(/_/g, " ");
}

// The palette is the sim's (WebTank.palette): tools, materials, devices,
// plants and creatures, in the sim's category tree
// (WebTank.paletteCategories, SPEC §2.5). The top tabs pick a category,
// the sub-tabs one of its sub-categories (no sub-tab row where it has only
// one), and the radio group shows that sub-category's items; the selection
// can be in a closed one.
function buildPalette() {
  const entries = tank.palette();
  const colours = tank.paletteColours();
  const about = tank.paletteDescriptions();
  places = tank.paletteCategories().map((c) => {
    const [path, tabLabel, subLabel] = c.split("\t");
    return { path, tab: path.split("/")[0], tabLabel, subLabel };
  });
  const tabs = [...new Map(places.map((p) => [p.tab, p.tabLabel])).entries()];
  const subs = [...new Map(places.map((p) => [p.path, p])).values()];
  const tab = (id: string, text: string, open: () => void) => {
    const t = document.createElement("button");
    t.type = "button";
    t.id = id;
    t.setAttribute("role", "tab");
    t.setAttribute("aria-controls", "palette-panel");
    t.textContent = text;
    t.addEventListener("click", open);
    return t;
  };
  setChildren(
    tabsEl,
    ...tabs.map(([name, text]) => {
      const t = tab(`tab-${name}`, text, () => showTab(name));
      t.dataset.tab = name;
      return t;
    }),
  );
  setChildren(
    subtabsEl,
    ...subs.map(({ path, tab: top, subLabel }) => {
      const t = tab(subId(path), subLabel, () => showSub(path));
      t.dataset.tab = top;
      t.dataset.path = path;
      return t;
    }),
  );
  let slot = 0;
  setChildren(
    paletteEl,
    ...entries.map((entry, i) => {
      const [kind, name] = entry.split(" ");
      const { path, tab: top } = places[i];
      slot = i > 0 && places[i - 1].path === path ? slot + 1 : 0;
      const b = document.createElement("button");
      b.type = "button";
      b.className = "swatch";
      b.dataset.kind = kind;
      b.dataset.name = name;
      b.dataset.tab = top;
      b.dataset.path = path;
      b.setAttribute("role", "radio");
      const i_ = document.createElement("i");
      if (colours[i]) i_.style.setProperty("--c", colours[i]);
      b.append(i_, label(name));
      const title = [about[i], slot < 10 ? `Key ${(slot + 1) % 10}` : ""].filter(Boolean).join(" · ");
      if (title) b.title = title;
      b.addEventListener("click", () => choose(i));
      return b;
    }),
  );
  // A rebuilt palette (reset, new scale) keeps the open sub-category if it
  // still exists; the first opens on the selection's.
  if (!subs.some((s) => s.path === openSub)) openSub = places[tank.selected()]?.path ?? subs[0]?.path;
  openTab = openSub?.split("/")[0];
  showSelection();
}

const subId = (path: string) => `sub-${path.replace("/", "-")}`;

function choose(i: number, focus = false) {
  tank.select(i);
  showSelection(focus);
  kick();
}

// Opens top tab `name` on the sub-category holding the selection, if it is
// in that tab, else on its first (the selection stays as it is).
function showTab(name: string | undefined, focus = false) {
  const held = places[tank.selected()];
  openTab = name;
  openSub = held?.tab === name ? held.path : places.find((p) => p.tab === name)?.path;
  showSelection();
  if (focus) $(`tab-${name}`)?.focus();
}

// Opens sub-category `path` (and its tab).
function showSub(path: string | undefined, focus = false) {
  if (!path) return;
  openSub = path;
  openTab = path.split("/")[0];
  showSelection();
  if (focus) {
    const t = document.getElementById(subId(path));
    if (t && !t.hidden) t.focus();
    else $(`tab-${openTab}`)?.focus();
  }
}

const wrapAt = (at: number, steps: number, n: number) => (((at + steps) % n) + n) % n;

// Opens the tab `steps` after (or before) the open one, wrapping.
function stepTab(steps: number, focus = false) {
  const tabs = topTabs();
  const at = tabs.findIndex((t) => t.dataset.tab === openTab);
  showTab(tabs[wrapAt(at, steps, tabs.length)]?.dataset.tab, focus);
}

// Opens the open tab's sub-category `steps` after (or before) the open
// one, wrapping within the tab.
function stepSub(steps: number, focus = false) {
  const subs = subTabs().filter((t) => t.dataset.tab === openTab);
  const at = subs.findIndex((t) => t.dataset.path === openSub);
  showSub(subs[wrapAt(at, steps, subs.length)]?.dataset.path, focus);
}

// Steps through every sub-category in palette order, on into the next (or
// previous) tab and wrapping: what scrolling over the palette does.
// Focus in the palette follows to the new sub-tab (or tab), as the item
// it was on may now be hidden.
function stepThrough(steps: number) {
  const subs = subTabs();
  const at = subs.findIndex((t) => t.dataset.path === openSub);
  showSub(subs[wrapAt(at, steps, subs.length)]?.dataset.path, paletteBox.contains(document.activeElement));
}

// The tabs, sub-tabs, swatches and propagule chips, all buttons
// (buildPalette, showPropagules).
const topTabs = () => [...tabsEl.children] as HTMLButtonElement[];
const subTabs = () => [...subtabsEl.children] as HTMLButtonElement[];
const swatches = () => [...paletteEl.children] as HTMLButtonElement[];
const chips = () => [...propagulesEl.querySelectorAll("button")];
const morphChips = () => [...morphsEl.querySelectorAll("button")];
const swatch = (i: number) => paletteEl.children[i] as HTMLButtonElement | undefined;
const openItems = () => swatches().filter((b) => !b.hidden);

// Roving tabindex: one tab, one sub-tab and one radio are in the tab
// order, the open ones and the checked radio (or, if the selection is in
// a closed sub-category, the open one's first). A closed tab or sub-tab
// holding the selection is marked.
// Sets or clears `data-holds` (Element.toggleAttribute is Chrome 69+, and
// the webOS build runs on Chrome 68).
function markHolds(t: HTMLElement, holds: boolean) {
  if (holds) t.setAttribute("data-holds", "");
  else t.removeAttribute("data-holds");
}

function showSelection(focus = false) {
  const selected = tank.selected();
  const held = places[selected];
  for (const t of topTabs()) {
    const open = t.dataset.tab === openTab;
    t.setAttribute("aria-selected", String(open));
    t.tabIndex = open ? 0 : -1;
    markHolds(t, !open && t.dataset.tab === held?.tab);
  }
  const siblings = subTabs().filter((t) => t.dataset.tab === openTab);
  // A tab with one sub-category has no sub-tab row.
  const row = siblings.length > 1;
  subtabsEl.hidden = !row;
  for (const t of subTabs()) {
    const open = t.dataset.path === openSub;
    t.hidden = !row || t.dataset.tab !== openTab;
    t.setAttribute("aria-selected", String(open));
    t.tabIndex = open ? 0 : -1;
    markHolds(t, !open && t.dataset.path === held?.path);
  }
  $("palette-panel").setAttribute("aria-labelledby", row && openSub ? subId(openSub) : `tab-${openTab}`);
  swatches().forEach((b, i) => {
    const on = i === selected;
    b.hidden = b.dataset.path !== openSub;
    b.setAttribute("aria-checked", String(on));
    b.tabIndex = on && !b.hidden ? 0 : -1;
    if (on && focus) b.focus({ preventScroll: true });
  });
  const items = openItems();
  if (items.length && !items.some((b) => b.tabIndex === 0)) items[0].tabIndex = 0;
  showPropagules();
  showMorphs();
  showFlip();
}

// The selected plant's propagule chips (only its species' own methods),
// under the items while its sub-category is open.
function showPropagules(focus = false) {
  const ways = tank.plantPropagules();
  const picked = tank.plantPropagule();
  propagulesEl.hidden = !ways.length || swatch(tank.selected())?.hidden !== false;
  if (propagulesEl.hidden) return setChildren(propagulesEl);
  const as = document.createElement("span");
  as.className = "as";
  as.textContent = "as";
  as.setAttribute("aria-hidden", "true");
  setChildren(
    propagulesEl,
    as,
    ...ways.map((name) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "chip";
      b.dataset.propagule = name;
      b.setAttribute("role", "radio");
      const on = name === picked;
      b.setAttribute("aria-checked", String(on));
      b.tabIndex = on ? 0 : -1;
      b.textContent = label(name);
      b.addEventListener("click", () => pickPropagule(name));
      return b;
    }),
  );
  if (focus) chips().find((b) => b.tabIndex === 0)?.focus();
}

function pickPropagule(name: string, focus = false) {
  tank.setPlantPropagule(name);
  showPropagules(focus);
}

// P: the selected plant's next propagule.
function nextPropagule(focus = false) {
  if (tank.nextPlantPropagule()) showPropagules(focus);
  else flash("Select a plant, then P picks how it starts");
}

function showMorphs(focus = false) {
  const morphs = tank.creatureMorphs();
  morphsEl.hidden = morphs.length <= 1 || swatch(tank.selected())?.hidden !== false;
  if (morphsEl.hidden) return setChildren(morphsEl);
  const picked = tank.creatureMorph();
  const names = tank.creatureMorphCycles() ? ["", ...morphs] : morphs;
  const caption = document.createElement("span");
  caption.className = "as";
  caption.textContent = "morph";
  caption.setAttribute("aria-hidden", "true");
  setChildren(
    morphsEl,
    caption,
    ...names.map((name) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "chip";
      b.dataset.morph = name;
      b.setAttribute("role", "radio");
      const on = name === picked;
      b.setAttribute("aria-checked", String(on));
      b.tabIndex = on ? 0 : -1;
      b.textContent = name ? label(name) : "Cycle";
      b.addEventListener("click", () => pickMorph(name, true));
      return b;
    }),
  );
  if (focus) morphChips().find((b) => b.tabIndex === 0)?.focus({ preventScroll: true });
}

function pickMorph(name: string, focus = false) {
  if (!tank.setCreatureMorph(name)) return;
  showMorphs(focus);
  kick();
}

function nextMorph(focus = false) {
  if (tank.nextCreatureMorph() === undefined) {
    flash("Select a creature with morphs, then C picks its colour");
    return;
  }
  showMorphs(focus);
  kick();
}

// Selects slot `n` of the open sub-category, if it has one.
function chooseSlot(n: number, focus = false) {
  const b = openItems()[n];
  if (b) choose(swatches().indexOf(b), focus);
}

// Arrow keys: within the tabs or sub-tabs, open the next or previous one;
// within a radio group, select its next or previous item. Home and End
// jump to the first and last.
function arrowStep(e: KeyboardEvent, at: number, n: number): number | undefined {
  const steps: Record<string, number> = { ArrowRight: at + 1, ArrowDown: at + 1, ArrowLeft: at - 1, ArrowUp: at - 1, Home: 0, End: n - 1 };
  return steps[e.key];
}

// Runs `go` with the arrow-key target among `els` (wrapping), if the key
// is one.
function arrows(e: KeyboardEvent, els: HTMLButtonElement[], go: (b: HTMLButtonElement) => void) {
  const n = els.length;
  const next = arrowStep(e, Math.max(0, els.indexOf(document.activeElement as HTMLButtonElement)), n);
  if (next === undefined || !n) return;
  e.preventDefault();
  e.stopPropagation();
  go(els[wrapAt(next, 0, n)]);
}

function tabKeys(e: KeyboardEvent) {
  arrows(e, topTabs(), (t) => showTab(t.dataset.tab, true));
}

function subKeys(e: KeyboardEvent) {
  arrows(e, subTabs().filter((t) => !t.hidden), (t) => showSub(t.dataset.path, true));
}

function paletteKeys(e: KeyboardEvent) {
  arrows(e, openItems(), (b) => choose(swatches().indexOf(b), true));
}

function propaguleKeys(e: KeyboardEvent) {
  arrows(e, chips(), (b) => pickPropagule(b.dataset.propagule ?? "", true));
}

function morphKeys(e: KeyboardEvent) {
  arrows(e, morphChips(), (b) => pickMorph(b.dataset.morph ?? "", true));
}

// --- Input -------------------------------------------------------------------

function canvasPoint(e: PointerEvent): Point {
  const rect = canvas.getBoundingClientRect();
  return [((e.clientX - rect.left) / rect.width) * canvas.width, ((e.clientY - rect.top) / rect.height) * canvas.height];
}

// Tells screen readers what inspect found. The drawn card in the canvas
// shows it to everyone else, so nothing visible repeats it.
function announce(text: string) {
  inspectSr.textContent = text;
}

// Hides the name field and device actions once the inspect card has gone
// (another tool's stroke can't remove it, but netting, death or a new
// inspection can).
function syncInspectControls() {
  if (tank.inspectPage() !== 0) return;
  inspected = null;
  closeName();
  closeDeviceActions();
}

// Shows a message (a refusal, a cheat's result, an error) in the readout
// line for a few seconds.
function flash(text: string) {
  inspectEl.textContent = text;
  inspectEl.hidden = false;
  clearTimeout(inspectTimer);
  inspectTimer = setTimeout(fade, INSPECT_SHOW_MS);
}

function fade() {
  inspectEl.hidden = true;
}

function closeDeviceActions() {
  deviceActions.hidden = true;
  inspectedDeviceId = null;
  inspectedDeviceCell = null;
}

function showDeviceLine(rest: string[], preserveCell = false) {
  const [name, id, place] = rest;
  const placeText =
    place === "bed" || place === "rim" || place === "floor"
      ? `on the ${place}`
      : `on the ${place} glass`;
  const status = rest[3];
  if (status === "air" || status === "connected" || status === "disconnected") {
    const [, , , , rawX, rawY] = rest;
    const [x, y] = preserveCell && inspectedDeviceCell ? inspectedDeviceCell : [rawX, rawY];
    inspectedDeviceCell = [x, y];
    inspected = null;
    closeName();
    closeDeviceActions();
    announce(`${label(name)} #${id} · ${placeText} · ${status === "air" ? "air diffuser" : status} · cell ${x}, ${y}`);
    return;
  }
  const [, , , flow, load, maturity, guard, frySafe, rawX, rawY] = rest;
  const [x, y] = preserveCell && inspectedDeviceCell ? inspectedDeviceCell : [rawX, rawY];
  inspectedDeviceCell = [x, y];
  const guardText = frySafe === "yes" ? "fry-safe" : `guard ${guard}`;
  const text = `${label(name)} #${id} · ${placeText} · flow ${flow} cells/s · load ${load}% · maturity ${maturity}% · ${guardText} · cell ${x}, ${y}`;
  inspected = null;
  closeName();
  inspectedDeviceId = Number(id);
  announce(text);
  if (owner && !handingOver) {
    deviceGuardLabel.hidden = frySafe === "yes";
    deviceGuardInput.checked = guard === "on";
    deviceActions.hidden = false;
  } else {
    deviceActions.hidden = true;
  }
}

function runDeviceRinse(mode: "gentle" | "hard" | "replace") {
  if (!owner || handingOver || inspectedDeviceId === null) return closeDeviceActions();
  try {
    const line = tank.rinseDevice(inspectedDeviceId, mode);
    const [kind, ...rest] = line.split(" ");
    if (kind === "device") showDeviceLine(rest, true);
    kick();
  } catch (err) {
    closeDeviceActions();
    flash(String(err));
  }
}

function runDeviceGuardToggle() {
  if (!owner || handingOver || inspectedDeviceId === null) return closeDeviceActions();
  try {
    const line = tank.toggleDeviceGuard(inspectedDeviceId);
    const [kind, ...rest] = line.split(" ");
    if (kind === "device") showDeviceLine(rest, true);
    kick();
  } catch (err) {
    closeDeviceActions();
    flash(String(err));
  }
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
    announce(creatureText(inspected));
    kick();
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
    closeDeviceActions();
    flash(`Cheat: ${done}`);
    showCreatureCap();
    kick();
  } catch (err) {
    cheatOut.textContent = String(err);
    cheatInput.select();
  }
}

// Takes what inspect last found: the card shows it; this announces it and
// opens the name field or device actions that act on it.
function showInspection() {
  const found = tank.takeInspection();
  if (!found) return;
  const [kind, ...rest] = found.split(" ");
  let text: string;
  if (kind === "cell") {
    const [name, x, y] = rest;
    text = `${label(name)} · cell ${x}, ${y}`;
  } else if (kind === "plant") {
    const [species, id, age, health, cells, limit, x, y] = rest;
    const s = Number(age);
    const old = s >= 60 ? `${Math.floor(s / 60)} min ${s % 60} s` : `${s} s`;
    text = `${label(species)} #${id} · ${old} old · health ${health}% · ${cells} cells · ${LIMITS[limit] ?? limit} · cell ${x}, ${y}`;
  } else if (kind === "device") {
    // D27, DV3: flow, media load, maturity, Rinse and intake guard toggle.
    showDeviceLine(rest);
    return;
  } else if (kind === "creature") {
    // The name is last and may hold spaces.
    closeDeviceActions();
    inspected = { id: Number(rest[1]), parts: rest.slice(0, 12), name: rest.slice(12).join(" ") };
    announce(creatureText(inspected));
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
  closeDeviceActions();
  announce(text);
}

// Shows why adding a creature or live food was refused.
function showRefusal() {
  const refused = tank.takeRefusal();
  if (!refused) return;
  const [kind, ...rest] = refused.split(" ");
  let message = "";
  if (kind === "cap") {
    const [stock, cap] = rest;
    message = `Tank full (${stock}/${cap} creatures) — raise Creature cap in Settings or net one out`;
  } else if (kind === "live_cap") {
    const [, cap, species] = rest;
    message = `${label(species)} limit reached (${cap} max)`;
  } else if (kind === "no_room") {
    const [species] = rest;
    message = `No room for ${label(species)} there — click open water`;
  } else {
    return;
  }
  inspected = null;
  closeName();
  closeDeviceActions();
  flash(message);
}

function showZoom() {
  if (!tank) return;
  zoomSelect.value = String(tank.zoom());
}

function chooseMagnifySlot() {
  const slot = tank.palette().findIndex((e) => e === "tool magnify");
  const button = swatch(slot);
  if (button) {
    showSub(button.dataset.path);
    choose(slot);
  }
}

// D27: mirrors the next flippable device placed onto the other side glass;
// the placement ghost shows where it goes. Shown only while a flippable
// device tool is selected.
function showFlip() {
  if (!tank) return;
  const hide = !tank.canFlipDevice();
  const hadFocus = hide && document.activeElement === flipBtn;
  flipBtn.hidden = hide;
  flipBtn.setAttribute("aria-pressed", String(tank.deviceFlipped()));
  if (hadFocus) {
    const selected = swatch(tank.selected());
    const target = selected && !selected.hidden ? selected : openItems()[0];
    target?.focus({ preventScroll: true });
  }
}

function flipDevice() {
  if (!tank.canFlipDevice()) return;
  flipBtn.setAttribute("aria-pressed", String(tank.flipDevice()));
  kick();
}

function pickZoom() {
  tank.setZoom(Number(zoomSelect.value));
  showZoom();
  chooseMagnifySlot();
}

function after() {
  showInspection();
  syncInspectControls();
  showRefusal();
  showCreatureCap();
  showZoom();
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
      const slop = TOUCH_SLOP * pixelRatio();
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
  // brush (or steps magnification when Magnify is active) and never scrolls or
  // zooms the page.
  canvas.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const unit = e.deltaMode === 1 ? WHEEL_STEP / 3 : e.deltaMode === 2 ? WHEEL_STEP * 10 : 1;
      wheelRest += -e.deltaY * unit;
      const steps = Math.trunc(wheelRest / WHEEL_STEP);
      if (!steps) return;
      wheelRest -= steps * WHEEL_STEP;
      if (tank.isMagnify()) {
        tank.stepZoom(steps);
        showZoom();
        kick();
      } else {
        update({ radius: tank.resizeBrush(steps) });
      }
    },
    { passive: false },
  );
  // Safari's own pinch gesture events.
  for (const type of ["gesturestart", "gesturechange"]) canvas.addEventListener(type, (e) => e.preventDefault());

  paletteEl.addEventListener("keydown", paletteKeys);
  tabsEl.addEventListener("keydown", tabKeys);
  subtabsEl.addEventListener("keydown", subKeys);
  propagulesEl.addEventListener("keydown", propaguleKeys);
  morphsEl.addEventListener("keydown", morphKeys);
  // Scrolling over the palette steps through the sub-categories, on into
  // the next tab (as in desktank window).
  paletteBox.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const unit = e.deltaMode === 1 ? WHEEL_STEP / 3 : e.deltaMode === 2 ? WHEEL_STEP * 10 : 1;
      stripRest += e.deltaY * unit;
      const steps = Math.trunc(stripRest / WHEEL_STEP);
      if (!steps) return;
      stripRest -= steps * WHEEL_STEP;
      stepThrough(steps);
    },
    { passive: false },
  );
  zoomSelect.addEventListener("change", pickZoom);
  flipBtn.addEventListener("click", flipDevice);
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
  deviceRinseBtn.addEventListener("click", () => runDeviceRinse("gentle"));
  deviceHardRinseBtn.addEventListener("click", () => runDeviceRinse("hard"));
  deviceReplaceBtn.addEventListener("click", () => runDeviceRinse("replace"));
  deviceGuardInput.addEventListener("change", runDeviceGuardToggle);
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
      if (tank.isMagnify()) {
        tank.stepZoom(e.key === "]" ? 1 : -1);
        showZoom();
        kick();
      } else {
        update({ radius: tank.resizeBrush(e.key === "]" ? 1 : -1) });
      }
    } else if (tank.isMagnify() && (e.key === "+" || e.key === "=" || e.key === "-")) {
      tank.stepZoom(e.key === "-" ? -1 : 1);
      showZoom();
      kick();
    } else if ((e.key === "n" || e.key === "N") && !nameForm.hidden) {
      e.preventDefault();
      nameInput.focus();
      nameInput.select();
    } else if ((e.key === "x" || e.key === "X") && !e.repeat && tank.canFlipDevice()) {
      flipDevice();
    } else if (e.key === "f" || e.key === "F") {
      const next = FIELDS[(FIELDS.indexOf(settings.field) + 1) % FIELDS.length];
      update({ field: next });
    } else if (e.key === "g" || e.key === "G" || e.key === "h" || e.key === "H") {
      // G opens the next tab, Shift+G the previous; H and Shift+H the same
      // for the open tab's sub-categories (as in desktank window; Shift,
      // not the letter's case, so Caps Lock doesn't flip it). Focus in the
      // palette moves to the new tab, as its radio may be hidden.
      const inPalette = paletteBox.contains(e.target as Node | null);
      const steps = e.shiftKey ? -1 : 1;
      if (e.key.toLowerCase() === "g") stepTab(steps, inPalette);
      else stepSub(steps, inPalette);
    } else if (e.key === "p" || e.key === "P") {
      // The selected plant's next propagule.
      nextPropagule(propagulesEl.contains(e.target as Node | null));
    } else if (e.key === "c" || e.key === "C") {
      nextMorph(morphsEl.contains(e.target as Node | null));
    } else if (/^[0-9]$/.test(e.key)) {
      // 1–9 are the open sub-category's first nine slots, 0 the tenth.
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
  $("set-catch-up").addEventListener("change", (e) => update({ catchUp: checkedOf(e) }));
  $("set-sway").addEventListener("change", (e) => update({ sway: checkedOf(e) }));
  $("set-sway-on-battery").addEventListener("change", (e) => update({ swayOnBattery: checkedOf(e) }));
  $("set-stats").addEventListener("change", (e) => {
    update({ stats: checkedOf(e) });
    updateStats();
  });
  seedInput.addEventListener("change", () => {
    const s = Math.max(0, Math.min(0xffffffff, Math.floor(Number(seedInput.value) || 0)));
    update({ seed: s });
    resetTank();
  });
  capInput.addEventListener("change", () => {
    if (!owner || handingOver || !tank) {
      showCreatureCap(true);
      return;
    }
    const raw = capInput.value.trim();
    const parsed = Number(raw);
    if (!raw || !Number.isFinite(parsed)) {
      showCreatureCap(true);
      return;
    }
    const requested = Math.max(0, Math.floor(parsed));
    const prev = tank.creatureCap();
    const applied = tank.setCreatureCap(requested);
    showCreatureCap(true);
    updateStats();
    if (applied !== requested) {
      const stock = tank.stockCount();
      if (requested < Math.max(1, stock)) {
        const verb = applied === prev ? "kept at" : "set to";
        flash(stock > 0 ? `Creature cap ${verb} ${applied} (tank has ${stock} creatures)` : `Creature cap must be at least ${applied}`);
      } else {
        flash(`Creature cap clamped to max ${applied}`);
      }
    }
  });
  $("set-defaults").addEventListener("click", () => {
    if (!owner || handingOver) return;
    const rescale = settings.scale !== DEFAULTS.scale || settings.seed !== DEFAULTS.seed;
    update({ ...DEFAULTS });
    if (rescale) {
      resetTank();
    } else if (tank) {
      tank.setCreatureCap(tank.defaultCreatureCap());
      showCreatureCap(true);
      updateStats();
    }
  });
}

function wireLifecycle() {
  // Hidden tabs pause: no ticks, no frames, and world time stops.
  // A hidden page is also the last reliable moment to save (SPEC §3.5).
  const onVisibility = () => {
    if (hidden === document.hidden) return;
    hidden = document.hidden;
    tank?.setHidden(hidden);
    if (hidden) {
      hiddenAt = owner && !handingOver ? Date.now() : null;
      tank?.pause();
      stopLoop();
      persist("hidden");
      if (!drewFirst) startStep("hidden: waiting for the page to be shown", debug);
    } else {
      const since = hiddenAt;
      hiddenAt = null;
      if (since !== null && tank) catchUp(tank, since);
      if (!drewFirst) startStep("waiting for the first frame");
      kick();
    }
  };
  document.addEventListener("visibilitychange", onVisibility);
  onVisibility();
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
      onBattery = !battery.charging;
      batteryFps = battery.charging ? FPS_AC : FPS_BATTERY;
      tank?.setMaxFps(maxFps());
      tank?.setOnBattery(onBattery);
      kick();
    };
    battery.addEventListener("chargingchange", onChange);
    onChange();
  }, () => {});
}

async function main() {
  startDiagnostics();
  try {
    startStep("loading the WebAssembly module");
    await init();
  } catch (err) {
    endSteps(`error: ${err}`);
    showError(`Couldn't load the WebAssembly module: ${err}`);
    return;
  }
  const [rMin, rMax] = brushRange();
  brushInput.min = String(rMin);
  brushInput.max = String(rMax);
  setChildren(
    zoomSelect,
    ...Array.from(zoomLevels(), (z) => {
      const o = document.createElement("option");
      o.value = String(z);
      o.textContent = `${z}×`;
      return o;
    }),
  );
  startStep("opening IndexedDB");
  db = await openStore();
  // Without Web Locks (Chrome before 69, as on webOS 5) the tab owns the
  // tank alone.
  if (navigator.locks && typeof BroadcastChannel !== "undefined") {
    startStep("taking the tank's Web Lock");
    ownership = new Ownership(LOCK, { own: becomeOwner, view: becomeViewer, handOver });
    await ownership.start();
    await hooks.claiming;
  } else {
    startStep(`claiming the tank (no Web Locks)${db ? "" : ", no IndexedDB"}`);
    await becomeOwner();
  }
  const ownedAtStart = owner;
  try {
    startStep("reading the saves");
    const saved = await loadNewest();
    startStep("creating the tank");
    hidden = document.hidden;
    replaceTank(saved ?? newTank());
    if (!owner) setRunning(false);
    const look = `${splitGrid ? ", split grid" : ""}${srgbCanvas ? "" : ", non-sRGB"}`;
    startStep(`opening graphics (${wantBackend}${look}, canvas ${canvas.width}×${canvas.height} px)`);
    await attachGraphics();
    note(`graphics: ${backend}, ${adapter}`);
  } catch (err) {
    endSteps(`error: ${err}`);
    showError(`Couldn't start the GPU renderer: ${err}`);
    return;
  }
  // Granted the lock while starting (the owner closed): open its last save.
  if (owner && !ownedAtStart) await hooks.claiming;
  started = true;
  if (owner && !ownedAtStart) await (hooks.claiming = resumeOwnership());
  nextAutosave = owner ? performance.now() + AUTOSAVE_MS : Infinity;
  showOwnership();
  showSaved();
  wireInput();
  wireLifecycle();
  statusEl.hidden = true;
  fpsSince = performance.now();
  updateStats();
  document.body.dataset.ready = "true";
  // A page started hidden (a webOS app launched in the background) draws
  // once shown; the visibility handler kicks the loop then.
  if (hidden) startStep("hidden: waiting for the page to be shown", debug);
  else startStep("waiting for the first frame");
  kick();
}

main().catch((err: unknown) => {
  endSteps(`error: ${err}`);
  showError(`DeskTank couldn't start: ${err}`);
});

// Offline installation is optional on restricted origins and private profiles.
if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("./sw.js").catch((error: unknown) => {
    console.warn("DeskTank offline installation unavailable", error);
  });
}
