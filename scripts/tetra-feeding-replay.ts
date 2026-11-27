// Draws a labelled side-by-side replay of repeated tetra feeding from the
// positions the simulation produced (`cargo run --release -p desktank-sim
// --example tetra_feeding -- --trace SEED`, built at each commit under
// comparison). Inspect is closed: only the fish, the flake and the water.
// Fish are numbered; a fish that bit within the last 15 s is green, and one
// that is biting this second is white. Short trails show where each has been.
//
//   node scripts/tetra-feeding-replay.ts --out target/shots/tetra-feeding-pilot \
//     --panel "PRE-NEEDS 81A44BD=pre.csv" --panel "NEEDS 9D7B029=n439.csv" --panel "TUNED=tuned.csv"
//
// Writes frames/frame_0000.png... (every second tick, real time at 10 fps),
// stills/*.png for the moments worth reading, and contact-sheet.png. Make a GIF
// with: ffmpeg -framerate 10 -i frames/frame_%04d.png -vf "split[a][b];[a]palettegen[p];[b][p]paletteuse" replay.gif
// See docs/notes/tetra-feeding-pilot.md. No dependencies beyond Node.
import { mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";
import { type Canvas, type Rgb, canvas, writePng } from "./sprite-art.ts";

const WIDTH = 160;
const HEIGHT = 90;
const SCALE = 4;
/** Rows of the tank shown: the fish never go below them. */
const VIEW = 70;
const TICKS_PER_SECOND = 20;
const FLAKES = [{ tick: 0, x: 54, label: "FLAKE 1 LEFT" }, { tick: 300, x: 106, label: "FLAKE 2 RIGHT" }];
const TRAIL = 14;
const FED_TICKS = 15 * TICKS_PER_SECOND;
const HEADER = 46;
const FOOTER = 30;
const GAP = 8;

// --- Trace ----------------------------------------------------------------

interface Sample {
  x: number;
  y: number;
  left: boolean;
  eaten: number;
}
interface Trace {
  /** Odd ticks, as written. */
  ticks: number[];
  fish: Map<number, Map<number, Sample>>;
  flakes: Map<number, [number, number][]>;
}

function load(path: string): Trace {
  const fish = new Map<number, Map<number, Sample>>();
  const flakes = new Map<number, [number, number][]>();
  const ticks = new Set<number>();
  for (const line of readFileSync(path, "utf8").split("\n")) {
    const f = line.split(",");
    if (f[0] === "P") {
      const [tick, id] = [Number(f[1]), Number(f[2])];
      ticks.add(tick);
      const byTick = fish.get(id) ?? new Map<number, Sample>();
      byTick.set(tick, { x: Number(f[3]), y: Number(f[4]), left: f[5] === "1", eaten: Number(f[7]) });
      fish.set(id, byTick);
    } else if (f[0] === "F") {
      const tick = Number(f[1]);
      flakes.set(tick, [...(flakes.get(tick) ?? []), [Number(f[2]), Number(f[3])]]);
    }
  }
  return { ticks: [...ticks].sort((a, b) => a - b), fish, flakes };
}

// --- Text: a 5x7 pixel font ------------------------------------------------

const GLYPHS: Record<string, string> = {
  " ": "00000 00000 00000 00000 00000 00000 00000",
  A: "01110 10001 10001 11111 10001 10001 10001",
  B: "11110 10001 10001 11110 10001 10001 11110",
  C: "01110 10001 10000 10000 10000 10001 01110",
  D: "11110 10001 10001 10001 10001 10001 11110",
  E: "11111 10000 10000 11110 10000 10000 11111",
  F: "11111 10000 10000 11110 10000 10000 10000",
  G: "01110 10001 10000 10111 10001 10001 01111",
  H: "10001 10001 10001 11111 10001 10001 10001",
  I: "01110 00100 00100 00100 00100 00100 01110",
  J: "00111 00010 00010 00010 00010 10010 01100",
  K: "10001 10010 10100 11000 10100 10010 10001",
  L: "10000 10000 10000 10000 10000 10000 11111",
  M: "10001 11011 10101 10101 10001 10001 10001",
  N: "10001 11001 10101 10011 10001 10001 10001",
  O: "01110 10001 10001 10001 10001 10001 01110",
  P: "11110 10001 10001 11110 10000 10000 10000",
  Q: "01110 10001 10001 10001 10101 10010 01101",
  R: "11110 10001 10001 11110 10100 10010 10001",
  S: "01111 10000 10000 01110 00001 00001 11110",
  T: "11111 00100 00100 00100 00100 00100 00100",
  U: "10001 10001 10001 10001 10001 10001 01110",
  V: "10001 10001 10001 10001 10001 01010 00100",
  W: "10001 10001 10001 10101 10101 11011 10001",
  X: "10001 10001 01010 00100 01010 10001 10001",
  Y: "10001 10001 01010 00100 00100 00100 00100",
  Z: "11111 00001 00010 00100 01000 10000 11111",
  "0": "01110 10001 10011 10101 11001 10001 01110",
  "1": "00100 01100 00100 00100 00100 00100 01110",
  "2": "01110 10001 00001 00010 00100 01000 11111",
  "3": "11110 00001 00001 01110 00001 00001 11110",
  "4": "00010 00110 01010 10010 11111 00010 00010",
  "5": "11111 10000 11110 00001 00001 10001 01110",
  "6": "00110 01000 10000 11110 10001 10001 01110",
  "7": "11111 00001 00010 00100 01000 01000 01000",
  "8": "01110 10001 10001 01110 10001 10001 01110",
  "9": "01110 10001 10001 01111 00001 00010 01100",
  ".": "00000 00000 00000 00000 00000 01100 01100",
  ":": "00000 01100 01100 00000 01100 01100 00000",
  "/": "00001 00001 00010 00100 01000 10000 10000",
  "+": "00000 00100 00100 11111 00100 00100 00000",
  "-": "00000 00000 00000 11111 00000 00000 00000",
  ",": "00000 00000 00000 00000 01100 00100 01000",
  "(": "00010 00100 01000 01000 01000 00100 00010",
  ")": "01000 00100 00010 00010 00010 00100 01000",
  "%": "11001 11010 00010 00100 01000 01011 10011",
  "=": "00000 00000 11111 00000 11111 00000 00000",
  ">": "10000 01000 00100 00010 00100 01000 10000",
  "<": "00001 00010 00100 01000 00100 00010 00001",
};

function text(c: Canvas, s: string, x: number, y: number, colour: Rgb, scale = 1): void {
  let cx = x;
  for (const ch of s.toUpperCase()) {
    const glyph = GLYPHS[ch] ?? GLYPHS[" "];
    glyph.split(" ").forEach((row, gy) =>
      [...row].forEach((bit, gx) => {
        if (bit === "1") rect(c, cx + gx * scale, y + gy * scale, scale, scale, colour);
      }),
    );
    cx += 6 * scale;
  }
}

function width(s: string, scale = 1): number {
  return s.length * 6 * scale;
}

// --- Drawing --------------------------------------------------------------

function put(c: Canvas, x: number, y: number, colour: Rgb, alpha = 1): void {
  const row = c[y];
  if (!row || x < 0 || x >= row.length) return;
  const [r, g, b] = row[x];
  row[x] = [
    Math.round(r + (colour[0] - r) * alpha),
    Math.round(g + (colour[1] - g) * alpha),
    Math.round(b + (colour[2] - b) * alpha),
  ];
}

function rect(c: Canvas, x: number, y: number, w: number, h: number, colour: Rgb, alpha = 1): void {
  for (let dy = 0; dy < h; dy++) for (let dx = 0; dx < w; dx++) put(c, x + dx, y + dy, colour, alpha);
}

/** Cell coordinates to canvas pixels inside a tank at (ox, oy). */
function px(ox: number, oy: number, cx: number, cy: number): [number, number] {
  return [Math.round(ox + cx * SCALE), Math.round(oy + cy * SCALE)];
}

function water(c: Canvas, ox: number, oy: number): void {
  for (let y = 0; y < VIEW * SCALE; y++) {
    const t = y / (VIEW * SCALE);
    const colour: Rgb = [Math.round(30 - 18 * t), Math.round(72 - 40 * t), Math.round(112 - 52 * t)];
    rect(c, ox, oy + y, WIDTH * SCALE, 1, colour);
  }
  rect(c, ox, oy + 4 * SCALE, WIDTH * SCALE, 1, [120, 175, 215]);
}

function body(c: Canvas, ox: number, oy: number, s: Sample, colour: Rgb): void {
  // A fish about seven cells long and three tall, head to the facing side.
  const dir = s.left ? -1 : 1;
  const [cx, cy] = px(ox, oy, s.x, s.y);
  const k = SCALE / 3;
  for (let dy = -Math.ceil(4 * k); dy <= Math.ceil(4 * k); dy++) {
    for (let dx = -Math.ceil(11 * k); dx <= Math.ceil(11 * k); dx++) {
      const along = (dx * dir) / k;
      const inBody = (dx / (10.5 * k)) ** 2 + (dy / (4 * k)) ** 2 <= 1;
      const inTail = along < -8 && Math.abs(dy / k) <= (-along - 8) * 1.4;
      if (inBody || inTail) put(c, cx + dx, cy + dy, colour);
    }
  }
  // A neon stripe along the flank, and an eye.
  for (let dx = Math.round(-6 * k); dx <= Math.round(7 * k); dx++) rect(c, cx + dx, cy + Math.round(k), 1, Math.ceil(k), [90, 210, 255]);
  rect(c, cx + dir * Math.round(7 * k), cy - Math.round(k), Math.ceil(k), Math.ceil(k), [10, 10, 20]);
}

function frame(panels: { label: string; trace: Trace }[], tick: number): Canvas {
  const panelW = WIDTH * SCALE;
  const total = panels.length * panelW + (panels.length + 1) * GAP;
  const c = canvas(total, HEADER + VIEW * SCALE + FOOTER, [16, 18, 24]);
  const seconds = (tick + 1) / TICKS_PER_SECOND;
  panels.forEach(({ label, trace }, p) => {
    const ox = GAP + p * (panelW + GAP);
    const oy = HEADER;
    text(c, label, ox, 6, [235, 235, 240], 2);
    water(c, ox, oy);
    // Flakes.
    for (const [fx, fy] of trace.flakes.get(tick) ?? []) {
      const [x, y] = px(ox, oy, fx, fy);
      rect(c, x - 4, y - 4, 9, 9, [230, 190, 90]);
      rect(c, x - 2, y - 2, 5, 5, [255, 250, 220]);
    }
    // Trails, then fish.
    const fedColour: Rgb = [140, 230, 110];
    const idle: Rgb = [70, 150, 215];
    const ids = [...trace.fish.keys()].sort((a, b) => a - b);
    for (const id of ids) {
      const byTick = trace.fish.get(id);
      for (let k = TRAIL; k >= 1; k--) {
        const s = byTick?.get(tick - 2 * k);
        if (s) rect(c, ...px(ox, oy, s.x, s.y), 2, 2, [190, 215, 240], 0.45 * (1 - k / (TRAIL + 1)));
      }
    }
    ids.forEach((id, n) => {
      const byTick = trace.fish.get(id);
      const s = byTick?.get(tick);
      if (!s) return;
      let last = -Infinity;
      let prev = byTick?.get(trace.ticks[0])?.eaten ?? 0;
      for (const t of trace.ticks) {
        if (t > tick) break;
        const e = byTick?.get(t)?.eaten ?? prev;
        if (e > prev) last = t;
        prev = e;
      }
      const since = tick - last;
      const colour: Rgb = since <= TICKS_PER_SECOND ? [255, 255, 255] : since <= FED_TICKS ? fedColour : idle;
      body(c, ox, oy, s, colour);
      const [x, y] = px(ox, oy, s.x, s.y);
      text(c, String(n + 1), x - 2, y - 22, [225, 225, 235], 1);
    });
    // Status: how many fish moved at least ten cells closer within two seconds.
    const phase = [...FLAKES].reverse().find((f) => f.tick <= tick);
    let status = "NO FOOD IN THE TANK";
    if (phase) {
      const since = tick - phase.tick;
      const closer = ids.filter((id) => {
        const at = (t: number) => trace.fish.get(id)?.get(t);
        const start = at(phase.tick + 1);
        const now = at(Math.min(tick, phase.tick + 2 * TICKS_PER_SECOND + 1));
        if (!start || !now) return false;
        return Math.hypot(start.x - phase.x, start.y - 2) - Math.hypot(now.x - phase.x, now.y - 2) >= 10;
      }).length;
      status = `${phase.label}: ${(since / TICKS_PER_SECOND).toFixed(1)} S, CLOSER IN 2 S ${closer}/${ids.length}`;
      if (since > 10 * TICKS_PER_SECOND && !(trace.flakes.get(tick)?.length)) status = `${phase.label}: EATEN, NO FOOD NOW`;
    }
    text(c, status, ox, 28, [200, 215, 230]);
    text(c, `T = ${seconds.toFixed(1)} S`, ox, HEADER + VIEW * SCALE + 10, [170, 180, 195]);
    const legend = "GREEN = ATE IN LAST 15 S";
    text(c, legend, ox + panelW - width(legend), HEADER + VIEW * SCALE + 10, [140, 230, 110]);
  });
  return c;
}

// --- Main ----------------------------------------------------------------

const { values } = parseArgs({
  options: {
    out: { type: "string" },
    panel: { type: "string", multiple: true },
    every: { type: "string", default: "1" },
  },
});
if (!values.out || !values.panel?.length) {
  console.error('usage: tetra-feeding-replay.ts --out DIR --panel "LABEL=trace.csv" [--panel ...]');
  process.exit(1);
}
const panels = values.panel.map((spec) => {
  const [label, path] = spec.split("=");
  return { label, trace: load(path) };
});
const out = values.out;
mkdirSync(join(out, "frames"), { recursive: true });
mkdirSync(join(out, "stills"), { recursive: true });
const every = Number(values.every);
const ticks = panels[0].trace.ticks;
ticks.forEach((tick, i) => {
  if (i % every === 0) writePng(join(out, "frames", `frame_${String(i / every).padStart(4, "0")}.png`), frame(panels, tick));
});

// The moments worth reading: each flake early and late, and the quiet that follows.
const stills: [string, number][] = [
  ["1-flake1-0.5s", 10],
  ["2-flake1-2s", 40],
  ["3-flake1-5s", 100],
  ["4-flake2-0.5s", 310],
  ["5-flake2-2s", 340],
  ["6-flake2-5s", 400],
  ["7-no-food-30s", 600],
  ["8-no-food-40s", 800],
];
const frames: Canvas[] = [];
for (const [name, tick] of stills) {
  const t = ticks.find((x) => x >= tick) ?? ticks[ticks.length - 1];
  const f = frame(panels, t);
  frames.push(f);
  writePng(join(out, "stills", `${name}.png`), f);
}
const sheet = canvas(frames[0][0].length, frames.reduce((n, f) => n + f.length, 0), [16, 18, 24]);
let y = 0;
for (const f of frames) {
  f.forEach((row, r) => {
    sheet[y + r] = row;
  });
  y += f.length;
}
writePng(join(out, "contact-sheet.png"), sheet);
console.log(`wrote ${Math.ceil(ticks.length / every)} frames, ${stills.length} stills and a contact sheet to ${out}`);
