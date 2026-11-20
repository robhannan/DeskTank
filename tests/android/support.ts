// Helpers for the Android tests (tests/android/, SPEC §7): adb for what
// Tapsmith has no API for (secure settings, the dream manager, logcat), and
// a PNG decoder for its screenshots.
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { inflateSync } from "node:zlib";

export const PACKAGE = "io.github.robhannan.desktank";
export const DREAM = `${PACKAGE}/.DeskTankDream`;
// The same component as `dumpsys` prints it.
export const DREAM_FULL = `${PACKAGE}/${PACKAGE}.DeskTankDream`;

const run = promisify(execFile);

// Runs `adb shell <args>` on the device under test. adb itself reads
// ANDROID_SERIAL; set it (tapsmith.config.ts passes the same one to
// Tapsmith) when more than one device is attached.
export async function shell(...args: string[]): Promise<string> {
  const { stdout } = await run("adb", ["shell", ...args], { maxBuffer: 64 << 20 });
  return stdout.replaceAll("\r\n", "\n");
}

export async function adb(...args: string[]): Promise<string> {
  const { stdout } = await run("adb", args, { maxBuffer: 64 << 20 });
  return stdout.replaceAll("\r\n", "\n");
}

// A secure setting's value, or undefined when it isn't set.
export async function getSecure(name: string): Promise<string | undefined> {
  const value = (await shell("settings", "get", "secure", name)).trim();
  return value === "null" ? undefined : value;
}

export async function putSecure(name: string, value: string | undefined): Promise<void> {
  if (value === undefined) await shell("settings", "delete", "secure", name);
  else await shell("settings", "put", "secure", name, value);
}

// `dumpsys dreams`'s current Dream, or "" when none is running.
export async function currentDream(): Promise<string> {
  const out = await shell("dumpsys", "dreams");
  return /mCurrentDream[^\n]*?ComponentInfo\{([^}]+)\}/.exec(out)?.[1] ?? "";
}

// Lines in the crash buffer and fatal lines in the main buffer that belong
// to our app's process. Other processes' crashes don't count: the TV
// emulator's SystemUI logs an unrelated FATAL EXCEPTION (PROGRESS, A0).
export function appFatals(crash: string, main: string): string[] {
  const found: string[] = [];
  // AndroidRuntime's "FATAL EXCEPTION: <thread>" is followed by
  // "Process: <name>, PID: <pid>"; native crashes are ">>> <name> <<<".
  const lines = (crash + "\n" + main).split("\n");
  lines.forEach((line, i) => {
    if (/FATAL EXCEPTION/.test(line) && lines.slice(i + 1, i + 3).some((l) => l.includes(`Process: ${PACKAGE}`))) {
      found.push(line, ...lines.slice(i + 1, i + 6));
    } else if (line.includes(`>>> ${PACKAGE} <<<`) || /Fatal signal/.test(line) && line.includes(PACKAGE)) {
      found.push(line);
    } else if (/\bpanicked at\b/.test(line) || / F DeskTank\b/.test(line)) {
      found.push(line);
    }
  });
  return found;
}

export interface Image {
  width: number;
  height: number;
  // RGBA8, row-major.
  rgba: Uint8Array;
}

// Decodes an 8-bit, non-interlaced RGB or RGBA PNG (what `screencap` and
// Tapsmith's screenshots are).
export function decodePng(png: Uint8Array): Image {
  const view = new DataView(png.buffer, png.byteOffset, png.byteLength);
  let pos = 8;
  let width = 0;
  let height = 0;
  let channels = 0;
  const idat: Uint8Array[] = [];
  while (pos < png.length) {
    const len = view.getUint32(pos);
    const type = String.fromCharCode(...png.subarray(pos + 4, pos + 8));
    const data = png.subarray(pos + 8, pos + 8 + len);
    if (type === "IHDR") {
      const d = new DataView(data.buffer, data.byteOffset, data.byteLength);
      width = d.getUint32(0);
      height = d.getUint32(4);
      const [depth, colour, , , interlace] = data.subarray(8, 13);
      channels = colour === 6 ? 4 : colour === 2 ? 3 : 0;
      if (depth !== 8 || channels === 0 || interlace !== 0) {
        throw new Error(`unsupported PNG: depth ${depth}, colour type ${colour}, interlace ${interlace}`);
      }
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") {
      break;
    }
    pos += 12 + len;
  }
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const out = new Uint8Array(width * height * 4);
  let prev = new Uint8Array(stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const row = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    const cur = new Uint8Array(stride);
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? cur[x - channels] : 0;
      const b = prev[x];
      const c = x >= channels ? prev[x - channels] : 0;
      let p: number;
      switch (filter) {
        case 0: p = 0; break;
        case 1: p = a; break;
        case 2: p = b; break;
        case 3: p = (a + b) >> 1; break;
        case 4: {
          const pa = Math.abs(b - c);
          const pb = Math.abs(a - c);
          const pc = Math.abs(a + b - 2 * c);
          p = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
          break;
        }
        default: throw new Error(`bad PNG filter ${filter} on row ${y}`);
      }
      cur[x] = (row[x] + p) & 0xff;
    }
    for (let x = 0; x < width; x++) {
      const o = (y * width + x) * 4;
      out[o] = cur[x * channels];
      out[o + 1] = cur[x * channels + 1];
      out[o + 2] = cur[x * channels + 2];
      out[o + 3] = channels === 4 ? cur[x * channels + 3] : 255;
    }
    prev = cur;
  }
  return { width, height, rgba: out };
}

// The share of pixels within `tolerance` (per channel) of `rgb`.
export function shareNear(image: Image, rgb: readonly [number, number, number], tolerance: number): number {
  let near = 0;
  const n = image.width * image.height;
  for (let i = 0; i < n; i++) {
    const o = i * 4;
    if (
      Math.abs(image.rgba[o] - rgb[0]) <= tolerance &&
      Math.abs(image.rgba[o + 1] - rgb[1]) <= tolerance &&
      Math.abs(image.rgba[o + 2] - rgb[2]) <= tolerance
    ) {
      near++;
    }
  }
  return n === 0 ? 0 : near / n;
}
