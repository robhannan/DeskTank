// A worked creature-sprite generator: the female honey gourami (#315),
// traced from the owner's photo. Copy it to the scratchpad, rename it, and
// replace the reference, mapping, shapes, colours and shading for the new
// creature (see SKILL.md). Run from the repo root:
//
//   node .agents/skills/desktank-creature-sprites/template.ts <preview.png> [--install]
//
// It writes a preview (fine frame x16, coarse frame x16, the four fine
// frames x4) and, with --install, puts the frames into the species TOML.
import { readFileSync, writeFileSync } from "node:fs";
import {
  SWING,
  blank,
  blit,
  canvas,
  carve,
  coarse,
  edge,
  line,
  opaque,
  rasterise,
  rows,
  setFrames,
  stamp,
  toTexel,
  writePng,
  type Frame,
  type Mapping,
  type Poly,
} from "../../../scripts/sprite-art.ts";

// Reference: the owner's photo, female panel cropped to 1450x710, facing
// right. Fine grid 40x20 (10x5 cells): 5 cm from tail tip to snout, 1360
// reference pixels over 40 texels.
const W = 40;
const H = 20;
const FINE: Mapping = { x0: 20, y0: 12, k: 1360 / W };
/** How far the fins swing, in reference pixels per unit of SWING. */
const AMP = 4;

const COLOURS: Record<string, string> = {
  b: "#f4dc4a", // body lemon
  i: "#fbec86", // back highlight
  m: "#e2c43a", // lower flank shade
  B: "#e6c840", // scale lattice
  x: "#8a6a1c", // outline: a dark shade of the body, never black
  s: "#5a4426", // lateral blotches
  E: "#7a5c2a", // blotches, mid-brown
  p: "#f2ecc4", // pearly belly
  q: "#d8d0a0", // belly scales
  a: "#8c8a78", // belly outline
  c: "#eab830", // fins
  S: "#a8781c", // fin rays
  M: "#c8a838", // soft fin rays
  O: "#ec9a2a", // orange fin edge
  l: "#fcf4b8", // feeler
  I: "#e87a2a", // iris
  k: "#0c1016", // pupil
  e: "#ffffff", // glint
};

/** The outlines in reference pixels for swim frame `phase`. */
function shapes(phase: number) {
  const s = SWING[phase] * AMP;
  const a = Math.abs(s);
  const body: Poly = [
    [1382, 236], [1366, 214], [1300, 186], [1200, 140], [1100, 106], [1000, 88],
    [900, 84], [800, 90], [700, 98], [600, 108], [500, 124], [400, 148], [330, 172],
    [260, 196], [230, 200], [230, 392], [300, 390], [400, 404], [500, 428], [600, 456],
    [700, 484], [800, 512], [900, 540], [1000, 560], [1100, 532], [1200, 472],
    [1280, 402], [1340, 322], [1370, 258],
  ];
  // The tail's rear edge sweeps (pulls in by |s|) and lifts and drops (s).
  const tail: Poly = [
    [234, 200], [180, 176], [120, 146 + 6 * s], [60, 118 + 10 * s], [20 + 14 * a, 106 + 12 * s],
    [36 + 12 * a, 200 + 10 * s], [60 + 10 * a, 320 + 8 * s], [100 + 6 * a, 446 + 6 * s],
    [160, 422], [234, 394],
  ];
  const dorsal: Poly = [
    [944, 86], [930, 66], [860, 44], [760, 30], [660, 28], [560, 36 + 2 * s],
    [460, 52 + 3 * s], [380, 70 + 5 * s], [310, 92 + 7 * s], [250, 120 + 9 * s],
    [232, 146 + 10 * s], [262, 196], [330, 174], [400, 150], [500, 126], [600, 110],
    [700, 100], [800, 92], [900, 86],
  ];
  const anal: Poly = [
    [1004, 556], [990, 580], [900, 600], [800, 602], [700, 592 - 2 * s], [600, 566 - 4 * s],
    [500, 534 - 6 * s], [400, 488 - 7 * s], [320, 436 - 6 * s], [280, 400 - 4 * s],
    [300, 388], [400, 402], [500, 426], [600, 454], [700, 482], [800, 510], [900, 538],
  ];
  const belly: Poly = [
    [1370, 258], [1340, 322], [1280, 402], [1200, 472], [1100, 532], [1000, 562],
    [940, 556], [960, 500], [1010, 446], [1060, 400], [1120, 360], [1200, 336],
    [1270, 330], [1330, 300],
  ];
  const feeler: [[number, number], [number, number]] = [[1060, 560], [440 + 10 * s, 636 + 12 * s]];
  return { body, tail, dorsal, anal, belly, feeler };
}

function render(phase: number, m: Mapping): Frame {
  const fine = m.k === FINE.k;
  const w = fine ? W : W / 2;
  const h = fine ? H : H / 2;
  const sh = shapes(phase);
  // Fins first, the body last (later regions win), then sub-regions.
  const r = rasterise(w, h, m, [["tail", sh.tail], ["dorsal", sh.dorsal], ["anal", sh.anal], ["body", sh.body]]);
  carve(r, m, "body", sh.belly, "belly");
  const g = blank(w, h);
  const gillX = toTexel(m, [1090, 0])[0];

  for (let x = 0; x < w; x++) {
    const col = r.map((row, y) => (row[x] === "body" || row[x] === "belly" ? y : -1)).filter((y) => y >= 0);
    const top = Math.min(...col);
    const bot = Math.max(...col);
    for (let y = 0; y < h; y++) {
      const name = r[y][x];
      if (name === null) continue;
      const out = edge(r, x, y);
      let c = "c";
      if (name === "body") {
        const depth = (y - top) / Math.max(1, bot - top);
        c = "b";
        // Highlight band along the back, a checker-dithered step below it.
        if (y === top || (y === top + 1 && (x + y) % 2 === 0)) c = "i";
        else if (depth > 0.75 && (x + y) % 2 === 0) c = "m";
        // Scale lattice behind the gill cover (fine frames only).
        if (fine && x < gillX && y > top && depth < 0.8 && ((y % 3 === 0 && x % 4 === 0) || (y % 3 === 1 && x % 4 === 2))) {
          c = c === "i" ? "b" : "B";
        }
        if (out) c = "x";
      } else if (name === "belly") {
        c = fine && ((y % 3 === 0 && x % 4 === 1) || (y % 3 === 1 && x % 4 === 3)) ? "q" : "p";
        if (r[y - 1]?.[x] === "body" && (x + y) % 2 === 0) c = "b"; // blend into the flank
        if (out) c = "a";
      } else if (name === "dorsal") {
        c = !opaque(r, x, y - 1) ? "O" : out && !opaque(r, x - 1, y) ? "S" : "c";
      } else if (name === "tail") {
        c = !opaque(r, x - 1, y) || !opaque(r, x, y - 1) || !opaque(r, x, y + 1) ? "O" : "c";
      } else if (name === "anal") {
        c = !opaque(r, x, y + 1) && fine ? "O" : out && !opaque(r, x - 1, y) ? "S" : "c";
      }
      g[y][x] = c;
    }
  }

  // Fin rays as explicit lines from the fin base (not a texture formula).
  const inFin = (fin: string) => (x: number, y: number) => r[y][x] === fin && g[y][x] !== "O";
  for (let sx = 920; sx > 250; sx -= 2 * m.k) {
    const lean = 0.3 + (1.2 * (920 - sx)) / 670; // steeper at the front
    line(g, m, [sx, 90 + (920 - sx) * 0.15], [sx - lean * 6 * m.k, 90 + (920 - sx) * 0.15 - 6 * m.k], "M", (x, y) => r[y][x] === "dorsal");
  }
  for (const ty of fine ? [150, 270, 390] : [270]) line(g, m, [234, 296], [30 + (ty - 106) * 0.2, ty], "S", inFin("tail"), 0.25);
  for (let sx = 990; sx > 300; sx -= 3 * m.k) {
    const base: [number, number] = [sx, 552 - (990 - sx) * 0.2];
    line(g, m, base, [base[0] - 2.5 * m.k, base[1] + 5 * m.k], "M", inFin("anal"));
  }

  // Markings: blotches along the flank, placed point by point.
  const blotches: [number, number, string][] = fine
    ? [[330, 290, "E"], [420, 300, "E"], [560, 300, "E"], [620, 312, "s"], [654, 312, "s"], [700, 306, "E"],
       [760, 316, "s"], [800, 300, "E"], [840, 318, "s"], [880, 312, "E"], [1130, 280, "s"], [1160, 310, "E"]]
    : [[560, 300, "s"], [760, 306, "s"], [940, 318, "s"]];
  for (const [px, py, ch] of blotches) {
    const [tx, ty] = toTexel(m, [px, py]).map(Math.floor);
    if (["body", "belly"].includes(r[ty][tx] ?? "") && !["x", "a"].includes(g[ty][tx])) g[ty][tx] = ch;
  }

  // Eye: a dark pupil, a coloured iris ring and a glint (fine only).
  const [ex, ey] = toTexel(m, [1248, 275]).map(Math.floor);
  if (fine) stamp(g, ex - 2, ey - 2, [".II.", "IekI", "IkkI", ".II."]);
  else stamp(g, ex - 1, ey, ["Ik"]);

  // Feeler: a one-texel thread on transparent texels.
  line(g, m, sh.feeler[0], sh.feeler[1], "l");
  return rows(g);
}

const fine = [0, 1, 2, 3].map((p) => render(p, FINE));
const coarseFrames = [0, 1, 2, 3].map((p) => render(p, coarse(FINE)));

const out = process.argv[2];
if (out) {
  const c = canvas(W * 16 + (W / 2) * 16 + 60, H * 16 + H * 4 + 50);
  blit(c, fine[0], COLOURS, 10, 10, 16);
  blit(c, coarseFrames[0], COLOURS, W * 16 + 40, 10, 16);
  fine.forEach((f, i) => blit(c, f, COLOURS, 10 + i * (W * 4 + 20), H * 16 + 30, 4));
  writePng(out, c);
}
if (process.argv.includes("--install")) {
  const path = "assets/species/honey_gourami.toml";
  let text = readFileSync(path, "utf8");
  text = setFrames(text, "female_frames", coarseFrames);
  text = setFrames(text, "female_fine_frames", fine);
  // Colours: merge into the sprite's table with setSpriteColours, keeping
  // every key the male's frames, the icon and the morphs use (SKILL.md).
  writeFileSync(path, text);
}
