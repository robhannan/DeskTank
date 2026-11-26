// Builds DeskTank's icons from the live sprite and plant assets, so they
// follow the art instead of drifting from it:
//
//   assets/icons/DeskTank.icon/        macOS Icon Composer document (icon.json
//     Assets/{Gravel,Reeds,Betta}.png  plus three 1024x1024 RGBA layers)
//   web/icons/tank-{512,192}.png       flat web/PWA icons: the same layers
//                                      composited over a water gradient
//
// Layers: the betta's 80x64 fine sprite (assets/species/betta.toml) at 8x,
// two plant clumps cut from the plant reference render
// (crates/desktank/tests/reference/plant_rosettes_x4.png) at 3x, and a bed
// of fine-scale gravel shaded by a port of the STYLE_GRAVEL pebble shader
// (crates/desktank-render/src/tank.wgsl) at 4x. Dependency-free and
// deterministic: no clock, no randomness, no Xcode.
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

function bettaLayer(): Image {
  const toml = readFileSync(join(ROOT, "assets/species/betta.toml"), "utf8");
  const colours = new Map<string, [number, number, number]>();
  for (const m of spriteValue(toml, "colours", "{", "}").matchAll(/(\S)\s*=\s*"(#[0-9a-fA-F]{6})"/g)) {
    const hex = m[2].toLowerCase();
    colours.set(m[1], hexRgb(GLASS_EDGE[hex] ?? hex));
  }
  const frames = spriteValue(toml, "fine_frames", "[", "]");
  const first = frames.slice(frames.indexOf("[", 1));
  const rows = [...first.slice(0, first.indexOf("]")).matchAll(/"([^"]*)"/g)].map((m) => m[1]);
  if (rows.length !== 64 || rows.some((r) => r.length !== 80)) throw new Error("betta fine frame is not 80x64");

  const scale = 8;
  const [ox, oy] = [208, 160];
  const img = blank(SIZE, SIZE);
  rows.forEach((row, y) => {
    [...row].forEach((key, x) => {
      if (key === ".") return;
      const c = colours.get(key);
      if (!c) throw new Error(`betta sprite key ${key} has no colour`);
      for (let sy = 0; sy < scale; sy++) {
        for (let sx = 0; sx < scale; sx++) put(img, ox + x * scale + sx, oy + y * scale + sy, c[0], c[1], c[2]);
      }
    });
  });
  return img;
}

// --- Reeds ------------------------------------------------------------------

function reedsLayer(): Image {
  const ref = decodePng(readFileSync(join(ROOT, "crates/desktank/tests/reference/plant_rosettes_x4.png")));
  if (ref.w !== 640 || ref.h !== 360) throw new Error(`plant reference is ${ref.w}x${ref.h}, expected 640x360`);
  const px = (x: number, y: number): [number, number, number] => {
    const i = (y * ref.w + x) * 4;
    return [ref.data[i], ref.data[i + 1], ref.data[i + 2]];
  };
  // Plant pixels are those that differ from the water behind them, which
  // varies by row only: sampled in a clear column.
  const water = new Map<number, [number, number, number]>();
  for (let y = 170; y < 332; y++) water.set(y, px(200, y));
  const img = blank(SIZE, SIZE);
  const scale = 3;
  // [x0, y0, x1, y1) in the reference, and where it lands.
  const clumps: [number, number, number, number, number, number][] = [
    [268, 184, 372, 332, 32, 500], // tall vallisneria
    [92, 264, 144, 332, 792, 740], // compact rosette
  ];
  for (const [x0, y0, x1, y1, dx, dy] of clumps) {
    for (let y = y0; y < y1; y++) {
      const [br, bg, bb] = water.get(y) ?? px(200, y);
      for (let x = x0; x < x1; x++) {
        const [r, g, b] = px(x, y);
        if (Math.abs(r - br) + Math.abs(g - bg) + Math.abs(b - bb) <= 12) continue;
        for (let sy = 0; sy < scale; sy++) {
          for (let sx = 0; sx < scale; sx++) put(img, dx + (x - x0) * scale + sx, dy + (y - y0) * scale + sy, r, g, b);
        }
      }
    }
  }
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

function gravelLayer(): Image {
  const grid = GRAVEL_TOP.length;
  const img = blank(SIZE, SIZE);
  const scale = SIZE / (grid * 4);
  for (let cx = 0; cx < grid; cx++) {
    const top = GRAVEL_TOP[cx];
    for (let cy = top; cy < grid; cy++) {
      const v = hash(cx + 17, cy + 31) & 0xff;
      const openUp = cy === top;
      const openL = cx > 0 && cy < GRAVEL_TOP[cx - 1];
      const openR = cx < grid - 1 && cy < GRAVEL_TOP[cx + 1];
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

/** `layers` over a vertical water gradient, opaque. */
function composite(layers: Image[]): Image {
  const top = hexRgb("#2d6086");
  const bottom = hexRgb("#103b5a");
  const img = blank(SIZE, SIZE);
  for (let y = 0; y < SIZE; y++) {
    const t = y / (SIZE - 1);
    const [r, g, b] = top.map((c, i) => Math.round(c + (bottom[i] - c) * t));
    for (let x = 0; x < SIZE; x++) put(img, x, y, r, g, b);
  }
  for (const layer of layers) {
    for (let i = 0; i < SIZE * SIZE * 4; i += 4) {
      const a = layer.data[i + 3];
      if (a === 0) continue;
      for (let c = 0; c < 3; c++) {
        img.data[i + c] = Math.round((layer.data[i + c] * a + img.data[i + c] * (255 - a)) / 255);
      }
    }
  }
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

// --- Outputs ----------------------------------------------------------------

function build(): Map<string, Buffer> {
  const gravel = gravelLayer();
  const reeds = reedsLayer();
  const betta = bettaLayer();
  const flat = composite([gravel, reeds, betta]);
  return new Map<string, Buffer>([
    [`${ICON_DIR}/icon.json`, Buffer.from(iconDocument())],
    [`${ICON_DIR}/Assets/Betta.png`, encodePng(betta, true)],
    [`${ICON_DIR}/Assets/Reeds.png`, encodePng(reeds, true)],
    [`${ICON_DIR}/Assets/Gravel.png`, encodePng(gravel, true)],
    ["web/icons/tank-512.png", encodePng(downsample(flat, 512), false)],
    ["web/icons/tank-192.png", encodePng(downsample(flat, 192), false)],
  ]);
}

/** Whether the file at `path` already holds `want`: the same bytes, or for
 * a PNG the same pixels (another zlib may compress the same image differently). */
function upToDate(path: string, want: Buffer): boolean {
  const file = join(ROOT, path);
  if (!existsSync(file)) return false;
  const have = readFileSync(file);
  if (have.equals(want)) return true;
  if (path.endsWith(".json")) {
    return have.toString("utf8").replace(/\r\n/g, "\n") === want.toString("utf8").replace(/\r\n/g, "\n");
  }
  if (!path.endsWith(".png")) return false;
  try {
    const a = decodePng(have);
    const b = decodePng(want);
    return a.w === b.w && a.h === b.h && Buffer.from(a.data).equals(Buffer.from(b.data));
  } catch {
    return false;
  }
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
