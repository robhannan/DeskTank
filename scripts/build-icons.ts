// Builds DeskTank's icons from the live sprite and plant assets, so they
// follow the art instead of drifting from it:
//
//   assets/icons/DeskTank.icon/        macOS Icon Composer document (icon.json
//     Assets/{Gravel,Reeds,Betta}.png  plus three 1024x1024 RGBA layers)
//   web/icons/tank-{512,192}.png       flat web/PWA icons: the same layers
//                                      composited over a water gradient
//   android/app/src/main/res/          Android's adaptive launcher layers
//     drawable-nodpi/ic_launcher_{background,foreground,monochrome}.png
//     (432x432) and the Leanback banner drawable-xhdpi/tv_banner.png (320x180)
//   assets/icons/windows/desktank.ico  PNG-in-ICO frames, 16..256 px
//   assets/icons/linux/hicolor/<n>x<n>/apps/desktank.png  freedesktop sizes
//   web/icons/favicon-32.png, favicon.ico, apple-touch-icon.png
//
// Layers: the betta's 80x64 fine sprite (assets/species/betta.toml) at 8x,
// two plant clumps cut from the plant reference render
// (crates/desktank/tests/reference/plant_rosettes_x4.png) at 3x, and a bed
// of fine-scale gravel shaded by a port of the STYLE_GRAVEL pebble shader
// (crates/desktank-render/src/tank.wgsl) at 4x. Dependency-free and
// deterministic: no clock, no randomness, no Xcode. The Android art reuses
// the same betta, reeds, water and gravel at other sizes, with the betta's
// original dark outline and the wordmark set in the 3x6 pixel font of
// crates/desktank-render/src/font.rs.
//
//   node scripts/build-icons.ts                 write every output
//   node scripts/build-icons.ts --check         exit 1 if an output is stale
//   node scripts/build-icons.ts --export-macos  also render the Icon
//       Composer document (Default, Dark, ClearLight) to target/icon-exports/
//       with Xcode's ictool (macOS with Xcode only; not part of the build)
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync, inflateSync } from "node:zlib";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ICON_DIR = "assets/icons/DeskTank.icon";
const SIZE = 1024;

// --- Images ---------------------------------------------------------------

/** 8-bit RGBA pixels, row-major. */
interface Image {
  w: number;
  h: number;
  data: Uint8Array;
}

function blank(w: number, h: number): Image {
  return { w, h, data: new Uint8Array(w * h * 4) };
}

function put(img: Image, x: number, y: number, r: number, g: number, b: number, a = 255): void {
  img.data.set([r, g, b, a], (y * img.w + x) * 4);
}

/** Alpha-composites `src` onto `dst` at (dx, dy), each source pixel repeated
 * `scale` times in both directions (nearest neighbour, clipped to `dst`).
 * Assumes binary source alpha (0 or 255) or an opaque destination. */
function over(dst: Image, src: Image, dx = 0, dy = 0, scale = 1): void {
  for (let y = 0; y < src.h * scale; y++) {
    const ty = dy + y;
    if (ty < 0 || ty >= dst.h) continue;
    for (let x = 0; x < src.w * scale; x++) {
      const tx = dx + x;
      if (tx < 0 || tx >= dst.w) continue;
      const s = (Math.floor(y / scale) * src.w + Math.floor(x / scale)) * 4;
      const a = src.data[s + 3];
      if (a === 0) continue;
      const d = (ty * dst.w + tx) * 4;
      for (let c = 0; c < 3; c++) {
        dst.data[d + c] = Math.round((src.data[s + c] * a + dst.data[d + c] * (255 - a)) / 255);
      }
      dst.data[d + 3] = Math.round(a + (dst.data[d + 3] * (255 - a)) / 255);
    }
  }
}

// --- PNG ------------------------------------------------------------------

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

/** An unfiltered 8-bit PNG: RGBA, or RGB when `alpha` is false. */
function encodePng(img: Image, alpha: boolean): Buffer {
  const channels = alpha ? 4 : 3;
  const stride = img.w * channels + 1;
  const raw = Buffer.alloc(img.h * stride);
  for (let y = 0; y < img.h; y++) {
    for (let x = 0; x < img.w; x++) {
      const s = (y * img.w + x) * 4;
      const d = y * stride + 1 + x * channels;
      raw[d] = img.data[s];
      raw[d + 1] = img.data[s + 1];
      raw[d + 2] = img.data[s + 2];
      if (alpha) raw[d + 3] = img.data[s + 3];
    }
  }
  const head = Buffer.alloc(13);
  head.writeUInt32BE(img.w, 0);
  head.writeUInt32BE(img.h, 4);
  head.set([8, alpha ? 6 : 2, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", head),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/** Decodes a non-interlaced 8-bit RGB or RGBA PNG. */
function decodePng(file: Buffer): Image {
  const sig = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
  if (file.length < 8 || sig.some((b, i) => file[i] !== b)) throw new Error("not a PNG");
  let w = 0;
  let h = 0;
  let channels = 0;
  const idat: Buffer[] = [];
  for (let at = 8; at + 8 <= file.length; ) {
    const len = file.readUInt32BE(at);
    if (at + 12 + len > file.length) throw new Error("corrupt PNG: truncated chunk");
    const kind = file.toString("latin1", at + 4, at + 8);
    const body = file.subarray(at + 8, at + 8 + len);
    if (kind === "IHDR") {
      w = body.readUInt32BE(0);
      h = body.readUInt32BE(4);
      const [depth, colour, , , interlace] = body.subarray(8, 13);
      if (depth !== 8 || (colour !== 2 && colour !== 6) || interlace !== 0) {
        throw new Error(`unsupported PNG (depth ${depth}, colour type ${colour}, interlace ${interlace})`);
      }
      channels = colour === 6 ? 4 : 3;
    } else if (kind === "IDAT") {
      idat.push(body);
    } else if (kind === "IEND") {
      break;
    }
    at += 12 + len;
  }
  const raw = inflateSync(Buffer.concat(idat));
  const stride = w * channels;
  if (raw.length !== h * (stride + 1)) {
    throw new Error(`corrupt PNG: expected ${h * (stride + 1)} decompressed bytes, got ${raw.length}`);
  }
  const img = blank(w, h);
  let prev = new Uint8Array(stride);
  for (let y = 0; y < h; y++) {
    const filter = raw[y * (stride + 1)];
    const line = Uint8Array.from(raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1)));
    for (let i = 0; i < stride; i++) {
      const a = i >= channels ? line[i - channels] : 0;
      const b = prev[i];
      const c = i >= channels ? prev[i - channels] : 0;
      let add = 0;
      if (filter === 1) {
        add = a;
      } else if (filter === 2) {
        add = b;
      } else if (filter === 3) {
        add = (a + b) >> 1;
      } else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a);
        const pb = Math.abs(p - b);
        const pc = Math.abs(p - c);
        add = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      } else if (filter !== 0) {
        throw new Error(`bad PNG filter ${filter}`);
      }
      line[i] = (line[i] + add) & 255;
    }
    for (let x = 0; x < w; x++) {
      put(img, x, y, line[x * channels], line[x * channels + 1], line[x * channels + 2], channels === 4 ? line[x * channels + 3] : 255);
    }
    prev = line;
  }
  return img;
}

// --- Betta ------------------------------------------------------------------

function hexRgb(hex: string): [number, number, number] {
  const n = Number.parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** The text of the first bracketed or braced value after `name =` in the
 * `[sprite]` table of a species TOML, comments dropped. */
function spriteValue(toml: string, name: string, open: string, close: string): string {
  const tableStart = toml.search(/^\[sprite\]\s*(?:#.*)?$/m);
  if (tableStart < 0) throw new Error("no [sprite] table");
  const afterHeader = toml.indexOf("\n", tableStart);
  const rest = afterHeader < 0 ? "" : toml.slice(afterHeader + 1);
  const nextTable = rest.search(/^\[/m);
  const sprite = nextTable < 0 ? rest : rest.slice(0, nextTable);
  const start = sprite.search(new RegExp(`^\\s*${name}\\s*=\\s*\\${open}`, "m"));
  if (start < 0) throw new Error(`no ${name} in [sprite]`);
  const body = sprite.slice(sprite.indexOf(open, start));
  // Strip comments while keeping quoted strings intact and matching the closing bracket.
  let out = "";
  let depth = 0;
  for (let i = 0; i < body.length; i++) {
    const ch = body[i];
    if (ch === '"') {
      const next = body.indexOf('"', i + 1);
      if (next < 0) throw new Error(`unterminated string in ${name}`);
      out += body.slice(i, next + 1);
      i = next;
    } else if (ch === "#") {
      const next = body.indexOf("\n", i);
      if (next < 0) break;
      out += "\n";
      i = next;
    } else {
      out += ch;
      if (ch === open) {
        depth++;
      } else if (ch === close && --depth === 0) {
        return out;
      }
    }
  }
  throw new Error(`unterminated ${name}`);
}

// The two darkest outline shades are lightened so the glass edge keeps the
// fish legible against the tank.
const GLASS_EDGE: Record<string, string> = { "#480a1c": "#901c36", "#5a0c1e": "#9b213b" };

interface Betta {
  rows: string[];
  colours: Map<string, string>;
}

/** The betta's first fine frame (80x64) and its palette, as authored. */
function loadBetta(): Betta {
  const toml = readFileSync(join(ROOT, "assets/species/betta.toml"), "utf8");
  const colours = new Map<string, string>();
  for (const m of spriteValue(toml, "colours", "{", "}").matchAll(/(\S)\s*=\s*"(#[0-9a-fA-F]{6})"/g)) {
    colours.set(m[1], m[2].toLowerCase());
  }
  const frames = spriteValue(toml, "fine_frames", "[", "]");
  const first = frames.slice(frames.indexOf("[", 1));
  const rows = [...first.slice(0, first.indexOf("]")).matchAll(/"([^"]*)"/g)].map((m) => m[1]);
  if (rows.length !== 64 || rows.some((r) => r.length !== 80)) throw new Error("betta fine frame is not 80x64");
  return { rows, colours };
}

/** The sprite as an image, one pixel per fine pixel. `glassEdge` lightens the
 * dark outline (macOS and web); `crop` trims it to its non-empty bounds
 * (Android). `ink`, when given, paints every pixel it accepts white
 * (`ink` gets the key and the pixel's position in the 80x64 sprite) and
 * leaves the rest clear: a monochrome silhouette. */
function bettaImage(betta: Betta, opts: { glassEdge: boolean; crop: boolean; ink?: (key: string, x: number, y: number) => boolean }): Image {
  let [x0, y0, x1, y1] = [0, 0, 80, 64];
  if (opts.crop) {
    [x0, y0, x1, y1] = [80, 64, 0, 0];
    betta.rows.forEach((row, y) => {
      [...row].forEach((key, x) => {
        if (key === ".") return;
        [x0, y0, x1, y1] = [Math.min(x0, x), Math.min(y0, y), Math.max(x1, x + 1), Math.max(y1, y + 1)];
      });
    });
  }
  const img = blank(x1 - x0, y1 - y0);
  for (let y = y0; y < y1; y++) {
    for (let x = x0; x < x1; x++) {
      const key = betta.rows[y][x];
      if (key === ".") continue;
      if (opts.ink) {
        if (opts.ink(key, x, y)) put(img, x - x0, y - y0, 255, 255, 255);
        continue;
      }
      const hex = betta.colours.get(key);
      if (!hex) throw new Error(`betta sprite key ${key} has no colour`);
      const [r, g, b] = hexRgb(opts.glassEdge ? (GLASS_EDGE[hex] ?? hex) : hex);
      put(img, x - x0, y - y0, r, g, b);
    }
  }
  return img;
}

function bettaLayer(betta: Betta, glassEdge = true): Image {
  const img = blank(SIZE, SIZE);
  over(img, bettaImage(betta, { glassEdge, crop: false }), 208, 160, 8);
  return img;
}

// --- Reeds ------------------------------------------------------------------

/** The plant reference render's two clumps, cut out of the water behind them
 * (one pixel per fine pixel, clear elsewhere). */
function reedClumps(): { tall: Image; rosette: Image } {
  const ref = decodePng(readFileSync(join(ROOT, "crates/desktank/tests/reference/plant_rosettes_x4.png")));
  if (ref.w !== 640 || ref.h !== 360) throw new Error(`plant reference is ${ref.w}x${ref.h}, expected 640x360`);
  const px = (x: number, y: number): [number, number, number] => {
    const i = (y * ref.w + x) * 4;
    return [ref.data[i], ref.data[i + 1], ref.data[i + 2]];
  };
  // Plant pixels are those that differ from the water behind them, which
  // varies by row only: sampled in a clear column.
  const cut = (x0: number, y0: number, x1: number, y1: number): Image => {
    const img = blank(x1 - x0, y1 - y0);
    for (let y = y0; y < y1; y++) {
      const [br, bg, bb] = px(200, y);
      for (let x = x0; x < x1; x++) {
        const [r, g, b] = px(x, y);
        if (Math.abs(r - br) + Math.abs(g - bg) + Math.abs(b - bb) > 12) put(img, x - x0, y - y0, r, g, b);
      }
    }
    return img;
  };
  return { tall: cut(268, 184, 372, 332), rosette: cut(92, 264, 144, 332) };
}

function reedsLayer(): Image {
  const { tall, rosette } = reedClumps();
  const img = blank(SIZE, SIZE);
  over(img, tall, 32, 500, 3);
  over(img, rosette, 792, 740, 3);
  return img;
}

// --- Gravel -----------------------------------------------------------------

// A port of the fine-scale STYLE_GRAVEL branch of tank.wgsl `ground` (with
// `hash`, `hash_grain` and `loose_top`): 32-bit unsigned arithmetic, f32
// colour maths. Keep it in step with the shader.

const f32 = Math.fround;

function hash(x: number, y: number): number {
  let h = (Math.imul(x, 0x8da6b343) ^ Math.imul(y, 0xd8163841)) >>> 0;
  h = Math.imul((h ^ (h >>> 13)) >>> 0, 0x5bd1e995) >>> 0;
  return (h ^ (h >>> 15)) >>> 0;
}

function hashGrain(px: number, py: number, v: number): number {
  const ox = (v & 15) * 7;
  const oy = ((v >>> 4) & 15) * 11;
  return (hash(px + ox, py + oy) ^ Math.imul(v, 0x9e3779b1)) >>> 0;
}

/** Quarter rows trimmed off the top of an exposed cell (`loose_top`, fine). */
function looseTop(openUp: boolean, openL: boolean, openR: boolean, cx: number, cy: number, qx: number): number {
  if (!openUp) return 0;
  const ripple = (hash(cx * 4 + qx, cy) & 3) === 0 ? 1 : 0;
  if (openL && openR) return 1 + (qx >= 2 ? qx - 2 : 1 - qx) + ripple;
  if (openL) return 3 - qx;
  if (openR) return qx;
  return ripple;
}

// First solid coarse row of each of the 64 columns (rows 58..63 are the bed).
const GRAVEL_TOP = [
  58, 58, 58, 58, 59, 59, 58, 58, 59, 59, 59, 59, 59, 59, 59, 59, 58, 58, 58, 59, 59, 59, 58, 58, 59, 59, 59, 59, 59, 59,
  59, 59, 58, 58, 58, 58, 58, 58, 58, 58, 59, 59, 59, 59, 59, 59, 59, 59, 58, 58, 58, 58, 58, 58, 58, 59, 59, 59, 59, 59,
  59, 59, 59, 59,
];
// Warmed and lightened from `assets/materials/gravel.toml` (#7d7468) so the
// gravel bed stays legible against the dark water backdrop at Dock sizes.
const GRAVEL_BASE = [152, 132, 96].map((c) => f32(c / 255));

/** A gravel bed: `tops[cx]` is the first solid coarse row of column `cx`, the
 * grid is `tops.length` columns by `rows` rows of 4x4 fine pixels, and each
 * fine pixel is `scale` image pixels square. */
function gravelLayer(tops: number[], rows: number, scale: number): Image {
  const grid = tops.length;
  const img = blank(grid * 4 * scale, rows * 4 * scale);
  for (let cx = 0; cx < grid; cx++) {
    const top = tops[cx];
    for (let cy = top; cy < rows; cy++) {
      const v = hash(cx + 17, cy + 31) & 0xff;
      const openUp = cy === top;
      const openL = cx > 0 && cy < tops[cx - 1];
      const openR = cx < grid - 1 && cy < tops[cx + 1];
      // Cell-constant layout bits (glue bit dropped).
      const cv = hash(v >>> 1, 0x4b1d);
      const cellN = f32(f32((v >>> 3) & 3) - 1.5) * f32(0.025);
      const ph = hash(cv >>> 3, 1);
      const stone = cv & 7;
      // Stone box (x0, y0, x1, y1) within the cell.
      const box = stone === 4 ? [0, 0, 3, 2] : stone === 5 ? [0, 0, 2, 3] : stone === 6 ? [0, 1, 3, 3] : [0, 0, 3, 3];
      const tone = (ph >>> 3) & 3;
      for (let qy = 0; qy < 4; qy++) {
        for (let qx = 0; qx < 4; qx++) {
          const topRow = looseTop(openUp, openL, openR, cx, cy, qx);
          if (qy < topRow) continue; // water above the trimmed contour
          const spek = (hashGrain(cx * 4 + qx, cy * 4 + qy, v) & 7) === 0;
          const fx = ((cv >>> 8) & 1) !== 0 ? 3 - qx : qx;
          const inBox = fx >= box[0] && fx <= box[2] && qy >= box[1] && qy <= box[3];
          const stoneTop = Math.max(box[1], topRow);
          let k: number;
          let warm: number[];
          if (!inBox) {
            // Dark grit between stones.
            k = f32(f32(spek ? 0.7 : 0.64) + cellN);
            warm = [0.97, 0.98, 1.02];
          } else {
            k = f32(f32(f32(0.92) + f32(f32(ph & 7) * f32(0.04))) + cellN);
            warm = tone === 1 ? [1.06, 1.0, 0.91] : tone === 2 ? [1.02, 1.0, 0.97] : [0.96, 0.99, 1.04];
            const left = fx === box[0];
            const right = fx === box[2];
            const crown = qy === stoneTop;
            const bottom = qy === box[3];
            if (right && bottom) {
              k = f32(k * f32(0.62));
            } else if ((left || right) && (crown || bottom)) {
              k = f32(k * f32(crown ? 0.74 : 0.68));
            } else if (bottom || right) {
              k = f32(k * f32(0.86));
            } else if (crown && fx === box[0] + 1) {
              k = f32(k * f32(1.18));
            } else if ((crown && fx === box[0] + 2) || (qy === stoneTop + 1 && fx === box[0] + 1)) {
              k = f32(k * f32(1.08));
            } else if (spek) {
              k = f32(k * f32(0.95));
            }
            if (crown && openUp) k = f32(k * f32(1.06));
          }
          if (!openUp) k = f32(k * f32(0.96));
          const rgb = GRAVEL_BASE.map((b, i) => {
            const c = f32(f32(b * f32(warm[i])) * k);
            return Math.round(Math.min(Math.max(c, 0), 1) * 255);
          });
          for (let sy = 0; sy < scale; sy++) {
            for (let sx = 0; sx < scale; sx++) {
              put(img, (cx * 4 + qx) * scale + sx, (cy * 4 + qy) * scale + sy, rgb[0], rgb[1], rgb[2]);
            }
          }
        }
      }
    }
  }
  return img;
}

// --- Web icons --------------------------------------------------------------

/** A vertical water gradient, opaque. */
function waterGradient(w: number, h: number): Image {
  const top = hexRgb("#2d6086");
  const bottom = hexRgb("#103b5a");
  const img = blank(w, h);
  for (let y = 0; y < h; y++) {
    const t = y / (h - 1);
    const [r, g, b] = top.map((c, i) => Math.round(c + (bottom[i] - c) * t));
    for (let x = 0; x < w; x++) put(img, x, y, r, g, b);
  }
  return img;
}

/** `layers` over a vertical water gradient, opaque. */
function composite(layers: Image[]): Image {
  const img = waterGradient(SIZE, SIZE);
  for (const layer of layers) over(img, layer);
  return img;
}

/** Area-average downsample of an opaque image to `size` squared. */
function downsample(src: Image, size: number): Image {
  const out = blank(size, size);
  const ratio = src.w / size;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const acc = [0, 0, 0];
      let total = 0;
      for (let sy = Math.floor(y * ratio); sy < Math.ceil((y + 1) * ratio); sy++) {
        const wy = Math.min(sy + 1, (y + 1) * ratio) - Math.max(sy, y * ratio);
        for (let sx = Math.floor(x * ratio); sx < Math.ceil((x + 1) * ratio); sx++) {
          const w = wy * (Math.min(sx + 1, (x + 1) * ratio) - Math.max(sx, x * ratio));
          const i = (sy * src.w + sx) * 4;
          for (let c = 0; c < 3; c++) acc[c] += src.data[i + c] * w;
          total += w;
        }
      }
      put(out, x, y, Math.round(acc[0] / total), Math.round(acc[1] / total), Math.round(acc[2] / total));
    }
  }
  return out;
}

// --- Icon Composer document --------------------------------------------------

function iconDocument(): string {
  const group = (name: string) => ({
    name,
    layers: [{ name, "image-name": `${name}.png` }],
    shadow: { kind: "neutral", opacity: 0.3 },
    translucency: { enabled: true, value: 0.45 },
    specular: true,
  });
  return `${JSON.stringify(
    {
      fill: { "automatic-gradient": "extended-srgb:0.075,0.25,0.37,1.0" },
      groups: [group("Gravel"), group("Reeds"), group("Betta")],
      "supported-platforms": { squares: ["macOS"] },
    },
    null,
    2,
  )}\n`;
}

// --- Android ----------------------------------------------------------------

const ANDROID_RES = "android/app/src/main/res";

/** The 3x6 glyphs of `crates/desktank-render/src/font.rs` for ASCII 32..126,
 * parsed from the source so the wordmark follows the font. */
function loadFont(): string[][] {
  const src = readFileSync(join(ROOT, "crates/desktank-render/src/font.rs"), "utf8");
  const start = src.indexOf("const ASCII:");
  if (start < 0) throw new Error("no ASCII table in font.rs");
  const open = src.indexOf("= [", start);
  const rest = src.slice(open);
  const end = rest.search(/\r?\n\];/);
  if (open < 0 || end < 0) throw new Error("unterminated ASCII table in font.rs");
  const table = rest.slice(0, end);
  const glyphs = [...table.matchAll(/\[((?:\s*"[.#]{3}",?){6})\s*\]/g)].map((m) => [...m[1].matchAll(/"([.#]{3})"/g)].map((r) => r[1]));
  if (glyphs.length !== 95) throw new Error(`font.rs ASCII table has ${glyphs.length} glyphs, expected 95`);
  return glyphs;
}

/** `text` in the pixel font, `scale` image pixels per font pixel, its top left at (x, y). */
function drawText(img: Image, font: string[][], text: string, x: number, y: number, scale: number, rgb: number[]): void {
  const advance = 4;
  [...text].forEach((ch, i) => {
    const glyph = font[ch.charCodeAt(0) - 32];
    if (!glyph) throw new Error(`no glyph for ${JSON.stringify(ch)}`);
    glyph.forEach((row, gy) => {
      [...row].forEach((ink, gx) => {
        if (ink !== "#") return;
        for (let sy = 0; sy < scale; sy++) {
          for (let sx = 0; sx < scale; sx++) {
            put(img, x + (i * advance + gx) * scale + sx, y + gy * scale + sy, rgb[0], rgb[1], rgb[2]);
          }
        }
      });
    });
  });
}

/** `img` centred horizontally on a `size` canvas (nudged 4 px right, where
 * the betta's mass sits) at `y`, scaled `scale` times, on a clear layer. */
function androidLayer(img: Image, scale: number, y: (h: number) => number): Image {
  const size = 432;
  const layer = blank(size, size);
  const [w, h] = [img.w * scale, img.h * scale];
  over(layer, img, Math.floor((size - w) / 2) + 4, y(h), scale);
  return layer;
}

/** The adaptive icon's layers (432x432: 108 dp at xxxhdpi, 72 dp visible under
 * a circular mask) and the TV banner, as [path, image, has alpha]. */
function androidIcons(betta: Betta): [string, Image, boolean][] {
  const { tall, rosette } = reedClumps();

  // Water, a gravel bed low in the circle's arc, and the two reeds at 2x.
  const background = waterGradient(432, 432);
  const tops = Array.from({ length: 54 }, (_, cx) => ([14, 15, 16, 26, 27, 28, 29, 38, 39, 40].includes(cx) ? 39 : 40));
  over(background, gravelLayer(tops, 54, 2));
  over(background, tall, 18, 326 - 296, 2);
  over(background, rosette, 286, 326 - 136, 2);

  const outline = bettaImage(betta, { glassEdge: false, crop: true });
  const foreground = androidLayer(outline, 3, (h) => 200 - Math.floor(h / 2));
  // The themed icon: a white silhouette with the eye cut out.
  const silhouette = bettaImage(betta, {
    glassEdge: false,
    crop: true,
    ink: (key, x, y) => !((key === "k" || key === "e") && x > 55 && y < 30),
  });
  const monochrome = androidLayer(silhouette, 3, (h) => Math.floor((432 - h) / 2));

  // The Leanback banner: the same scene at 1x with the fish and the name.
  const banner = waterGradient(320, 180);
  const bannerTops = Array.from({ length: 80 }, (_, cx) => ([2, 3, 4, 5, 11, 12, 13].includes(cx % 19) ? 39 : 40));
  over(banner, gravelLayer(bannerTops, 45, 1));
  over(banner, tall, 4, 162 - 148);
  over(banner, rosette, 262, 162 - 68);
  over(banner, outline, 18, 26, 2);
  const font = loadFont();
  drawText(banner, font, "DeskTank", 182, 64, 4, hexRgb("#0a2034"));
  drawText(banner, font, "DeskTank", 180, 62, 4, hexRgb("#f2f8fc"));

  return [
    [`${ANDROID_RES}/drawable-nodpi/ic_launcher_background.png`, background, false],
    [`${ANDROID_RES}/drawable-nodpi/ic_launcher_foreground.png`, foreground, true],
    [`${ANDROID_RES}/drawable-nodpi/ic_launcher_monochrome.png`, monochrome, true],
    [`${ANDROID_RES}/drawable-xhdpi/tv_banner.png`, banner, false],
  ];
}

// --- Windows, Linux and favicon icons -----------------------------------------

/** Per-pixel coverage (0..1) of the rounded rectangle at integer position
 * (rx, ry), size rw x rh and corner radius `r`, on a `w` x `h` canvas.
 * Corner pixels are sampled 8x8; everything else is exactly 0 or 1. */
function roundedCoverage(w: number, h: number, rx: number, ry: number, rw: number, rh: number, r: number): Float64Array {
  const out = new Float64Array(w * h);
  const grid = 8;
  const inside = (u: number, v: number): boolean => {
    if (u < rx || u > rx + rw || v < ry || v > ry + rh) return false;
    const dx = Math.max(rx + r - u, u - (rx + rw - r), 0);
    const dy = Math.max(ry + r - v, v - (ry + rh - r), 0);
    return dx * dx + dy * dy <= r * r;
  };
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const inCorner = (x < rx + r || x + 1 > rx + rw - r) && (y < ry + r || y + 1 > ry + rh - r);
      if (!inCorner) {
        out[y * w + x] = x >= rx && x + 1 <= rx + rw && y >= ry && y + 1 <= ry + rh ? 1 : 0;
        continue;
      }
      let hits = 0;
      for (let sy = 0; sy < grid; sy++) {
        for (let sx = 0; sx < grid; sx++) {
          if (inside(x + (sx + 0.5) / grid, y + (sy + 0.5) / grid)) hits++;
        }
      }
      out[y * w + x] = hits / (grid * grid);
    }
  }
  return out;
}

/** A box blur of `map` (w x h), `passes` times in each direction, with clear
 * pixels beyond the edge. */
function boxBlur(map: Float64Array, w: number, h: number, radius: number, passes: number): Float64Array {
  let cur = map;
  const span = 2 * radius + 1;
  for (let pass = 0; pass < passes * 2; pass++) {
    const horizontal = pass % 2 === 0;
    const next = new Float64Array(cur.length);
    const [along, across] = horizontal ? [w, h] : [h, w];
    for (let line = 0; line < across; line++) {
      for (let i = 0; i < along; i++) {
        let sum = 0;
        for (let k = -radius; k <= radius; k++) {
          const j = i + k;
          if (j >= 0 && j < along) sum += cur[horizontal ? line * w + j : j * w + line];
        }
        next[horizontal ? line * w + i : i * w + line] = sum / span;
      }
    }
    cur = next;
  }
  return cur;
}

const SHADOW_RGB = [4, 14, 26];
const SHADOW_ALPHA = 0.35;

/** The hi-res (1024 px) simplified icon scene: water with the cropped betta
 * (its original dark outline) centred at 80% of the width. */
function smallScene(betta: Betta): Image {
  const scene = waterGradient(SIZE, SIZE);
  const fish = bettaImage(betta, { glassEdge: false, crop: true });
  const k = Math.floor((SIZE * 0.8) / Math.max(fish.w, fish.h));
  over(scene, fish, Math.floor((SIZE - fish.w * k) / 2), Math.floor((SIZE - fish.h * k) / 2), k);
  return scene;
}

/** A desktop icon of `size` px, RGBA with clear corners. Up to 32 px: the
 * betta on water in a rounded square, edge to edge. Larger: the whole tank
 * scene in a rounded square inset by a margin, over a soft drop shadow. */
function desktopIcon(size: number, small: Image, tank: Image): Image {
  const out = blank(size, size);
  if (size <= 32) {
    const scene = downsample(small, size);
    const cover = roundedCoverage(size, size, 0, 0, size, size, size * 0.2);
    for (let i = 0; i < size * size; i++) {
      out.data.set([scene.data[i * 4], scene.data[i * 4 + 1], scene.data[i * 4 + 2], Math.round(cover[i] * 255)], i * 4);
    }
    return out;
  }
  const margin = Math.round(size * 0.04);
  const tile = size - 2 * margin;
  const scene = downsample(tank, tile);
  const dy = Math.max(1, Math.round(size * 0.015));
  const radius = size * 0.18;
  const cover = roundedCoverage(size, size, margin, margin, tile, tile, radius);
  const shadow = boxBlur(
    roundedCoverage(size, size, margin, margin + dy, tile, tile, radius),
    size,
    size,
    Math.max(1, Math.round(size * 0.015)),
    2,
  );
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const i = y * size + x;
      const ta = cover[i];
      const sa = shadow[i] * SHADOW_ALPHA;
      const alpha = ta + sa * (1 - ta);
      if (alpha === 0) continue;
      const inTile = x >= margin && x < margin + tile && y >= margin && y < margin + tile;
      const t = inTile ? (y - margin) * tile + (x - margin) : 0;
      const rgb = [0, 1, 2].map((c) => {
        const tc = inTile ? scene.data[t * 4 + c] : 0;
        return Math.round((tc * ta + SHADOW_RGB[c] * sa * (1 - ta)) / alpha);
      });
      out.data.set([rgb[0], rgb[1], rgb[2], Math.round(alpha * 255)], i * 4);
    }
  }
  return out;
}

/** An ICO file of PNG frames (Vista and later), in the order given. */
function encodeIco(frames: { size: number; png: Buffer }[]): Buffer {
  const head = Buffer.alloc(6 + 16 * frames.length);
  head.writeUInt16LE(0, 0);
  head.writeUInt16LE(1, 2);
  head.writeUInt16LE(frames.length, 4);
  let offset = head.length;
  frames.forEach(({ size, png }, i) => {
    const at = 6 + 16 * i;
    head[at] = size === 256 ? 0 : size;
    head[at + 1] = size === 256 ? 0 : size;
    head.writeUInt16LE(1, at + 4); // planes
    head.writeUInt16LE(32, at + 6); // bits per pixel
    head.writeUInt32LE(png.length, at + 8);
    head.writeUInt32LE(offset, at + 12);
    offset += png.length;
  });
  return Buffer.concat([head, ...frames.map((f) => f.png)]);
}

/** The PNG frames of an ICO file written by `encodeIco`. */
function decodeIco(file: Buffer): Buffer[] {
  if (file.length < 6 || file.readUInt16LE(0) !== 0 || file.readUInt16LE(2) !== 1) throw new Error("not an ICO");
  const count = file.readUInt16LE(4);
  if (file.length < 6 + 16 * count) throw new Error("corrupt ICO: truncated directory");
  return Array.from({ length: count }, (_, i) => {
    const at = 6 + 16 * i;
    const [len, offset] = [file.readUInt32LE(at + 8), file.readUInt32LE(at + 12)];
    if (offset + len > file.length) throw new Error("corrupt ICO: frame out of bounds");
    return file.subarray(offset, offset + len);
  });
}

const WINDOWS_SIZES = [16, 24, 32, 48, 64, 128, 256];
const LINUX_SIZES = [16, 32, 48, 128, 256, 512];
const FAVICON_SIZES = [16, 32];

/** The Windows, Linux and web favicon icons, as [path, bytes]. */
function desktopIcons(betta: Betta, tank: Image, apple: Image): [string, Buffer][] {
  const small = smallScene(betta);
  const cache = new Map<number, Buffer>();
  const png = (size: number): Buffer => {
    let buf = cache.get(size);
    if (!buf) {
      buf = encodePng(desktopIcon(size, small, tank), true);
      cache.set(size, buf);
    }
    return buf;
  };
  const ico = (sizes: number[]): Buffer => encodeIco(sizes.map((size) => ({ size, png: png(size) })));
  return [
    ["assets/icons/windows/desktank.ico", ico(WINDOWS_SIZES)],
    ...LINUX_SIZES.map((size): [string, Buffer] => [`assets/icons/linux/hicolor/${size}x${size}/apps/desktank.png`, png(size)]),
    ["web/icons/favicon-32.png", png(32)],
    ["web/icons/favicon.ico", ico(FAVICON_SIZES)],
    ["web/icons/apple-touch-icon.png", encodePng(downsample(apple, 180), false)],
  ];
}

// --- Outputs ----------------------------------------------------------------

function build(): Map<string, Buffer> {
  const sprite = loadBetta();
  const gravel = gravelLayer(GRAVEL_TOP, 64, 4);
  const reeds = reedsLayer();
  const betta = bettaLayer(sprite);
  const flat = composite([gravel, reeds, betta]);
  const tank = composite([gravel, reeds, bettaLayer(sprite, false)]);
  return new Map<string, Buffer>([
    [`${ICON_DIR}/icon.json`, Buffer.from(iconDocument())],
    [`${ICON_DIR}/Assets/Betta.png`, encodePng(betta, true)],
    [`${ICON_DIR}/Assets/Reeds.png`, encodePng(reeds, true)],
    [`${ICON_DIR}/Assets/Gravel.png`, encodePng(gravel, true)],
    ["web/icons/tank-512.png", encodePng(downsample(flat, 512), false)],
    ["web/icons/tank-192.png", encodePng(downsample(flat, 192), false)],
    ...androidIcons(sprite).map(([path, img, alpha]): [string, Buffer] => [path, encodePng(img, alpha)]),
    ...desktopIcons(sprite, tank, flat),
  ]);
}

/** Whether the file at `path` already holds `want`: the same bytes, or for
 * a PNG (or the frames of an ICO) the same pixels (another zlib may compress the same image differently). */
function upToDate(path: string, want: Buffer): boolean {
  const file = join(ROOT, path);
  if (!existsSync(file)) return false;
  const have = readFileSync(file);
  if (have.equals(want)) return true;
  if (path.endsWith(".json")) {
    return have.toString("utf8").replace(/\r\n/g, "\n") === want.toString("utf8").replace(/\r\n/g, "\n");
  }
  const samePixels = (x: Buffer, y: Buffer): boolean => {
    const a = decodePng(x);
    const b = decodePng(y);
    return a.w === b.w && a.h === b.h && Buffer.from(a.data).equals(Buffer.from(b.data));
  };
  try {
    if (path.endsWith(".png")) return samePixels(have, want);
    if (path.endsWith(".ico")) {
      const a = decodeIco(have);
      const b = decodeIco(want);
      return a.length === b.length && a.every((frame, i) => samePixels(frame, b[i]));
    }
  } catch {
    return false;
  }
  return false;
}

function exportMacos(): void {
  const ictool = "/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool";
  if (!existsSync(ictool)) throw new Error(`ictool not found at ${ictool}`);
  const out = join(ROOT, "target/icon-exports");
  mkdirSync(out, { recursive: true });
  for (const rendition of ["Default", "Dark", "ClearLight"]) {
    const file = join(out, `DeskTank-${rendition}.png`);
    execFileSync(
      ictool,
      [
        join(ROOT, ICON_DIR),
        "--export-image",
        "--output-file",
        file,
        "--platform",
        "macOS",
        "--rendition",
        rendition,
        "--width",
        "1024",
        "--height",
        "1024",
        "--scale",
        "1",
        "--design-generation",
        "26",
      ],
      { stdio: "inherit" },
    );
    console.log(`wrote ${relative(ROOT, file)}`);
  }
}

function main(): void {
  const args = new Set(process.argv.slice(2));
  const unknown = [...args].filter((a) => !["--check", "--export-macos"].includes(a));
  if (unknown.length > 0) throw new Error(`unknown argument ${unknown.join(" ")}`);
  const outputs = build();
  if (args.has("--check")) {
    const stale = [...outputs].filter(([path, want]) => !upToDate(path, want)).map(([path]) => path);
    if (stale.length > 0) {
      console.error(`stale icons (run pnpm build:icons):\n${stale.map((p) => `  ${p}`).join("\n")}`);
      process.exit(1);
    }
    console.log("icons are up to date");
  } else {
    for (const [path, data] of outputs) {
      if (upToDate(path, data)) continue;
      mkdirSync(dirname(join(ROOT, path)), { recursive: true });
      writeFileSync(join(ROOT, path), data);
      console.log(`wrote ${path}`);
    }
  }
  if (args.has("--export-macos")) exportMacos();
}

main();
