// Helpers for the Android tests (tests/android/, SPEC §7): adb for what
// Tapsmith has no API for (secure settings, the dream manager, logcat), and
// a PNG decoder for its screenshots.
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { inflateSync } from "node:zlib";

export const PACKAGE = "io.github.robhannan.desktank";
export const ACTIVITY = `${PACKAGE}/.DeskTankActivity`;
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

// Lines in the crash buffer and main logcat that show a crash of our app's
// process. Other processes' crashes don't count: the TV emulator's SystemUI
// logs an unrelated FATAL EXCEPTION (PROGRESS, A0). A Rust panic in a JNI
// callback becomes a Java exception the shim doesn't catch, so it shows as
// a FATAL EXCEPTION too.
export function appFatals(crash: string, main: string): string[] {
  const ours = new RegExp(`Process: ${PACKAGE.replaceAll(".", "\\.")}(?![\\w.])`);
  const found: string[] = [];
  const lines = (crash + "\n" + main).split("\n");
  lines.forEach((line, i) => {
    if (line.includes("FATAL EXCEPTION")) {
      // "Process: <name>, PID: <pid>" follows the header, possibly after
      // lines other processes interleave.
      const next = lines.slice(i + 1, i + 20);
      const end = next.findIndex((l) => l.includes("FATAL EXCEPTION") || l.trim() === "");
      if (next.slice(0, end === -1 ? next.length : end).some((l) => ours.test(l))) {
        found.push(line, ...next.slice(0, 5));
      }
    } else if (line.includes(`>>> ${PACKAGE} <<<`)) {
      // A native crash's tombstone header (a release build aborts on a
      // panic; libc's own "Fatal signal" line truncates the process name).
      found.push(line);
    } else if (/ E DeskTank\s*: panic: /.test(line)) {
      // The host's panic hook (crates/desktank-android).
      found.push(line);
    }
  });
  return found;
}

export async function logcat(): Promise<{ crash: string; main: string }> {
  return {
    crash: await adb("logcat", "-d", "-b", "crash", "-v", "threadtime"),
    main: await adb("logcat", "-d", "-v", "threadtime"),
  };
}

export async function waitForFirstFrame(
  host: "App" | "Dream",
  timeoutMs: number,
): Promise<{ width: number; height: number }> {
  const frame = new RegExp(` I DeskTank\\s*: first frame (\\d+)x(\\d+)[^\\n]*, host ${host}\\)`);
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const { crash, main } = await logcat();
    const fatals = appFatals(crash, main);
    if (fatals.length > 0) throw new Error(`the app crashed before its first frame:\n${fatals.join("\n")}`);
    const match = frame.exec(main);
    if (match) return { width: Number(match[1]), height: Number(match[2]) };
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  throw new Error(`no ${host} first frame within ${timeoutMs / 1000} s`);
}

export async function recordedFatals(): Promise<string[]> {
  const { crash, main } = await logcat();
  return appFatals(crash, main);
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

// What the frame shows, sampling every `step`th pixel each way: the share
// of the whole frame that is water (blue well above red and at least
// green: the lit water's gradient from #1f4e79 down to its dark depths) and
// the share of the bottom quarter that is sand (warm and light, #d8c08a and
// its texels). A blank, black or one-colour frame has no sand.
export function tankShares(image: Image, step = 4): { water: number; sand: number } {
  let water = 0;
  let all = 0;
  let sand = 0;
  let bottom = 0;
  const floor = Math.floor(image.height * 0.75);
  for (let y = 0; y < image.height; y += step) {
    for (let x = 0; x < image.width; x += step) {
      const o = (y * image.width + x) * 4;
      const [r, g, b] = [image.rgba[o], image.rgba[o + 1], image.rgba[o + 2]];
      all++;
      if (b >= r + 30 && b >= g) water++;
      if (y >= floor) {
        bottom++;
        if (r >= 150 && r >= b + 50) sand++;
      }
    }
  }
  return { water: all ? water / all : 0, sand: bottom ? sand / bottom : 0 };
}
