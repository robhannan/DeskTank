// CIEDE2000 colour difference on sRGB pixels: a port of the render tests'
// comparison (crates/desktank/tests/support/mod.rs), so the web is judged by
// the same SPEC §7 tolerance as native captures.

export function srgbToLab([r, g, b]) {
  const lin = (c) => {
    c /= 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const [lr, lg, lb] = [lin(r), lin(g), lin(b)];
  const x = (0.4124564 * lr + 0.3575761 * lg + 0.1804375 * lb) / 0.95047;
  const y = 0.2126729 * lr + 0.7151522 * lg + 0.072175 * lb;
  const z = (0.0193339 * lr + 0.119192 * lg + 0.9503041 * lb) / 1.08883;
  const f = (t) => (t > 216 / 24389 ? Math.cbrt(t) : ((24389 / 27) * t + 16) / 116);
  const [fx, fy, fz] = [f(x), f(y), f(z)];
  return [116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz)];
}

const deg = (r) => (r * 180) / Math.PI;
const rad = (d) => (d * Math.PI) / 180;
const rem360 = (v) => ((v % 360) + 360) % 360;

export function deltaE2000([l1, a1, b1], [l2, a2, b2]) {
  const c1 = Math.hypot(a1, b1);
  const c2 = Math.hypot(a2, b2);
  const cBar7 = ((c1 + c2) / 2) ** 7;
  const g = 0.5 * (1 - Math.sqrt(cBar7 / (cBar7 + 25 ** 7)));
  const [a1p, a2p] = [(1 + g) * a1, (1 + g) * a2];
  const [c1p, c2p] = [Math.hypot(a1p, b1), Math.hypot(a2p, b2)];
  const hue = (b, a) => (a === 0 && b === 0 ? 0 : rem360(deg(Math.atan2(b, a))));
  const [h1p, h2p] = [hue(b1, a1p), hue(b2, a2p)];

  const dl = l2 - l1;
  const dc = c2p - c1p;
  let dh;
  if (c1p * c2p === 0) dh = 0;
  else if (Math.abs(h2p - h1p) <= 180) dh = h2p - h1p;
  else if (h2p <= h1p) dh = h2p - h1p + 360;
  else dh = h2p - h1p - 360;
  const dhh = 2 * Math.sqrt(c1p * c2p) * Math.sin(rad(dh) / 2);

  const lbar = (l1 + l2) / 2;
  const cbar = (c1p + c2p) / 2;
  let hbar;
  if (c1p * c2p === 0) hbar = h1p + h2p;
  else if (Math.abs(h1p - h2p) <= 180) hbar = (h1p + h2p) / 2;
  else if (h1p + h2p < 360) hbar = (h1p + h2p + 360) / 2;
  else hbar = (h1p + h2p - 360) / 2;
  const t =
    1 -
    0.17 * Math.cos(rad(hbar - 30)) +
    0.24 * Math.cos(rad(2 * hbar)) +
    0.32 * Math.cos(rad(3 * hbar + 6)) -
    0.2 * Math.cos(rad(4 * hbar - 63));
  const dTheta = 30 * Math.exp(-(((hbar - 275) / 25) ** 2));
  const cbar7 = cbar ** 7;
  const rc = 2 * Math.sqrt(cbar7 / (cbar7 + 25 ** 7));
  const l50 = (lbar - 50) ** 2;
  const sl = 1 + (0.015 * l50) / Math.sqrt(20 + l50);
  const sc = 1 + 0.045 * cbar;
  const sh = 1 + 0.015 * cbar * t;
  const rt = -Math.sin(2 * rad(dTheta)) * rc;
  return Math.sqrt((dl / sl) ** 2 + (dc / sc) ** 2 + (dhh / sh) ** 2 + rt * (dc / sc) * (dhh / sh));
}

// Compares RGBA8 images pixel by pixel (alpha ignored).
export function compare(a, b, threshold) {
  if (a.length !== b.length) throw new Error("image sizes differ");
  const diff = { over: 0, total: a.length / 4, max: 0 };
  for (let i = 0; i < a.length; i += 4) {
    if (a[i] === b[i] && a[i + 1] === b[i + 1] && a[i + 2] === b[i + 2]) continue;
    const de = deltaE2000(srgbToLab([a[i], a[i + 1], a[i + 2]]), srgbToLab([b[i], b[i + 1], b[i + 2]]));
    diff.max = Math.max(diff.max, de);
    if (de > threshold) diff.over++;
  }
  return diff;
}
