// Types shared by the page (main.ts) and the Playwright tests and bench
// (tests/web/, scripts/bench-web.ts), which read window.desktank.
import type { WebTank } from "./pkg/desktank_web.js";
import type { Ownership } from "./ownership.ts";

// The settings panel's values, as stored in localStorage.
export interface Settings {
  scale: number; // 0 is auto, else 1 or an even scale up to 8
  fps: string; // "auto", "30" or "15"
  field: string; // "" or a field overlay name
  avoidCursor: boolean;
  easyMode: boolean;
  catchUp: boolean;
  sway: boolean;
  swayOnBattery: boolean;
  oled: boolean; // true black water, air and letterbox
  ruler: boolean; // centimetre ruler along the bottom and left edges
  stats: boolean;
  seed: number;
  radius: number;
  timeRatio: number; // life-time multiplier for new tanks; a loaded or imported tank keeps its own (1 is real time)
}

// Test and benchmark hooks (window.desktank).
export interface Hooks {
  frameTimes: number[];
  rafTimes: number[];
  readonly tank: WebTank;
  readonly backend: string;
  // [scale, columns, rows] of the current tank in the current canvas.
  readonly grid: [number, number, number];
  readonly owner: boolean;
  readonly ownership: Ownership | null;
  lastEpoch: number | null; // the epoch this tab last claimed
  // A write with that epoch, bypassing the owner check (a stale owner).
  lateWrite(): Promise<string>;
  writes: number; // saves this tab wrote
  saving: Promise<boolean>; // the latest write
  loaded: { hash: string; tick: number; width: number; height: number } | null; // the save opened, if any
  lastCatchUp: { seconds: number; births: number; deaths: number } | null;
  released: { hash: string; tick: number } | null; // when this tab handed the tank over
  ownedAt: number; // performance.now() when this tab was granted the lock
  stole: boolean; // whether that lock grant came from a steal
  claiming: Promise<void>; // the current claim and its state restore
  takeOverAt: number; // performance.now() of the last Take over click
  persisted?: boolean; // navigator.storage.persist()'s answer
  // Startup steps and errors, "<ms> ms <what>" (main.ts; on screen with ?debug=1).
  readonly startup: string[];
  readonly settings: Settings;
  loadScene(toml: string, seed: number, width: number, height: number): Promise<void>;
}

declare global {
  interface Window {
    desktank: Hooks;
  }
}
