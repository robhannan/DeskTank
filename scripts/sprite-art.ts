// Helpers for tracing creature sprites from a reference picture (see
// .agents/skills/desktank-creature-sprites/SKILL.md): outlines are polygons
// in the reference's pixels, rasterised onto the fine grid (four texels per
// cell) and the coarse grid (two), then shaded rule by rule. Also a
// dependency-free PNG preview and the edits that put frames into a species
// TOML. A generator script imports this module; it has no CLI of its own.
import { writeFileSync } from "node:fs";
import { deflateSync } from "node:zlib";

export type Pt = readonly [number, number];
export type Poly = readonly Pt[];
/** One frame: rows of texel keys, `.` transparent. */
export type Frame = string[];

/** Reference pixels -> texels: texel (x, y) covers reference pixels from
 * (x0 + x * k, y0 + y * k), k pixels square. */
export interface Mapping {
  x0: number;
  y0: number;
  k: number;
}

/** The same mapping at half resolution (the coarse grid). */
export function coarse(m: Mapping): Mapping {
  return { x0: m.x0, y0: m.y0, k: m.k * 2 };
}

/** A reference point in texel coordinates (fractional). */
export function toTexel(m: Mapping, [px, py]: Pt): Pt {
  return [(px - m.x0) / m.k, (py - m.y0) / m.k];
}

export function inside([x, y]: Pt, poly: Poly): boolean {
  let c = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const [xi, yi] = poly[i];
    const [xj, yj] = poly[j];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) c = !c;
  }
  return c;
}

/** Whether `need` of 9 samples (3x3) of texel (x, y) fall in `poly`: 5 is
 * a majority; lower keeps thin parts (a tail spike, a feeler base). */
export function covers(m: Mapping, poly: Poly, x: number, y: number, need = 5): boolean {
  let n = 0;
  for (const sy of [0.2, 0.5, 0.8]) {
    for (const sx of [0.2, 0.5, 0.8]) {
      if (inside([m.x0 + (x + sx) * m.k, m.y0 + (y + sy) * m.k], poly)) n++;
    }
  }
  return n >= need;
}

/** Region names per texel, `null` outside: later regions win, so list
 * fins first and the body last, and carve sub-regions (throat, belly)
 * out of the body afterwards with {@link carve}. */
export type Regions = (string | null)[][];

export function rasterise(w: number, h: number, m: Mapping, regions: [string, Poly][]): Regions {
  const out: Regions = Array.from({ length: h }, () => Array<string | null>(w).fill(null));
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      for (const [name, poly] of regions) if (covers(m, poly, x, y)) out[y][x] = name;
    }
  }
  return out;
}

/** Relabel texels of region `from` that `poly` covers as `to`. */
export function carve(r: Regions, m: Mapping, from: string, poly: Poly, to: string): void {
  r.forEach((row, y) =>
    row.forEach((name, x) => {
      if (name === from && covers(m, poly, x, y)) row[x] = to;
    }),
  );
}

export function opaque(r: Regions, x: number, y: number): boolean {
  return y >= 0 && y < r.length && x >= 0 && x < r[0].length && r[y][x] !== null;
}

/** On the outline: a 4-neighbour is transparent. */
export function edge(r: Regions, x: number, y: number): boolean {
  return [[1, 0], [-1, 0], [0, 1], [0, -1]].some(([dx, dy]) => !opaque(r, x + dx, y + dy));
}

/** A blank w x h grid of texel keys. */
export function blank(w: number, h: number): string[][] {
  return Array.from({ length: h }, () => Array<string>(w).fill("."));
}

/** Draw a one-texel line between reference points `a` and `b` with key
 * `ch` wherever `ok(x, y)` allows (by default only on transparent texels,
 * for feelers, legs and antennae; pass a region test for fin rays).
 * `from` skips the start of the line (0..1), so rays clear their base. */
export function line(
  g: string[][],
  m: Mapping,
  a: Pt,
  b: Pt,
  ch: string,
  ok: (x: number, y: number) => boolean = (x, y) => g[y][x] === ".",
  from = 0,
): void {
  const [ax, ay] = toTexel(m, a);
  const [bx, by] = toTexel(m, b);
  const n = Math.ceil(Math.max(Math.abs(bx - ax), Math.abs(by - ay)) * 3) + 1;
  for (let i = 0; i <= n; i++) {
    const t = i / n;
    if (t < from) continue;
    const x = Math.floor(ax + (bx - ax) * t);
    const y = Math.floor(ay + (by - ay) * t);
    if (y >= 0 && y < g.length && x >= 0 && x < g[0].length && ok(x, y)) g[y][x] = ch;
  }
}

/** Stamp a small pattern (e.g. an eye, `.` keeps what is there) with its
 * top left at texel (x, y). */
export function stamp(g: string[][], x: number, y: number, pattern: readonly string[]): void {
  pattern.forEach((row, dy) =>
    [...row].forEach((ch, dx) => {
      if (ch !== "." && g[y + dy]?.[x + dx] !== undefined) g[y + dy][x + dx] = ch;
    }),
  );
}

export function rows(g: string[][]): Frame {
  return g.map((r) => r.join(""));
}

/** The swim cycle's offset per frame: rest, one way, rest, the other. */
export const SWING = [0, 1, 0, -1] as const;

// --- Preview -------------------------------------------------------------

export type Rgb = readonly [number, number, number];
export type Canvas = Rgb[][];

export function rgb(hex: string): Rgb {
  const v = Number.parseInt(hex.replace("#", ""), 16);
  return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
}

/** A canvas filled with tank water. */
export function canvas(w: number, h: number, bg: Rgb = [24, 58, 92]): Canvas {
  return Array.from({ length: h }, () => Array<Rgb>(w).fill(bg));
}

/** Draw `frame` at (x, y), each texel `scale` pixels square. */
export function blit(c: Canvas, frame: Frame, colours: Record<string, string>, x: number, y: number, scale: number): void {
  frame.forEach((row, ty) =>
    [...row].forEach((ch, tx) => {
      if (ch === ".") return;
      const hex = colours[ch];
      if (!hex) throw new Error(`no colour for key ${ch}`);
      const px = rgb(hex);
      for (let dy = 0; dy < scale; dy++) {
        for (let dx = 0; dx < scale; dx++) {
          const row2 = c[y + ty * scale + dy];
          if (row2 && x + tx * scale + dx < row2.length) row2[x + tx * scale + dx] = px;
        }
      }
    }),
  );
}

const CRC = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function chunk(kind: string, data: Buffer): Buffer {
  const body = Buffer.concat([Buffer.from(kind, "latin1"), data]);
  let crc = 0xffffffff;
  for (const b of body) crc = CRC[(crc ^ b) & 255] ^ (crc >>> 8);
  const out = Buffer.alloc(body.length + 8);
  out.writeUInt32BE(data.length, 0);
  body.copy(out, 4);
  out.writeUInt32BE((crc ^ 0xffffffff) >>> 0, body.length + 4);
  return out;
}

export function writePng(path: string, c: Canvas): void {
  const h = c.length;
  const w = c[0].length;
  const raw = Buffer.alloc(h * (w * 3 + 1));
  c.forEach((row, y) =>
    row.forEach(([r, g, b], x) => raw.set([r, g, b], y * (w * 3 + 1) + 1 + x * 3)),
  );
  const head = Buffer.alloc(13);
  head.writeUInt32BE(w, 0);
  head.writeUInt32BE(h, 4);
  head.set([8, 2, 0, 0, 0], 8);
  writeFileSync(
    path,
    Buffer.concat([
      Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
      chunk("IHDR", head),
      chunk("IDAT", deflateSync(raw, { level: 9 })),
      chunk("IEND", Buffer.alloc(0)),
    ]),
  );
}

// --- Species TOML --------------------------------------------------------

/** `text` with the top-level array `name = [ ... ]` (frames, fine_frames,
 * female_frames, carrying_fine_frames, ...) replaced by `frames`. */
export function setFrames(text: string, name: string, frames: Frame[]): string {
  const re = new RegExp(`^${name} = \\[\\n[\\s\\S]*?^\\]\\n`, "m");
  if (!re.test(text)) throw new Error(`no ${name} block`);
  const body = frames
    .map((f) => `    [\n${f.map((r) => `        "${r}",\n`).join("")}    ]`)
    .join(",\n");
  return text.replace(re, `${name} = [\n${body},\n]\n`);
}

/** `text` with the sprite's `colours = { ... }` line set to `colours`. A
 * morph's `[[genetics.morph]]` table has a `colours` line too, before
 * `[sprite]`: only the first one after `[sprite]` is the sprite's. */
export function setSpriteColours(text: string, colours: Record<string, string>): string {
  const at = text.indexOf("\n[sprite]\n");
  if (at < 0) throw new Error("no [sprite] table");
  const start = text.indexOf("\ncolours = {", at);
  const end = text.indexOf("\n", start + 1);
  if (start < 0) throw new Error("no sprite colours line");
  const line2 = `colours = { ${Object.entries(colours).map(([k, v]) => `${k} = "${v}"`).join(", ")} }`;
  return `${text.slice(0, start + 1)}${line2}${text.slice(end)}`;
}

/** The keys a set of frames uses. */
export function keysOf(...sets: Frame[][]): Set<string> {
  const out = new Set<string>();
  for (const frames of sets) for (const f of frames) for (const r of f) for (const ch of r) if (ch !== ".") out.add(ch);
  return out;
}
