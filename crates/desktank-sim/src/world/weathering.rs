//! Weathered stone outlines: how a brush's round 4×4 sub-cell outline
//! ([`super::hardscape`]) is trimmed into an irregular, natural silhouette
//! for materials with a `shape` (SPEC §2.1).
//!
//! * **Angular** (Seiryu): each dab is a convex polygon of five or six
//!   flat faces (random directions, each cut back by its own amount) inside
//!   its disc, taller than wide, and chipped by a fine hash lattice along
//!   its edge. The faces come from the material, the world seed and the
//!   16-cell block the dab sits in, so every dab of a drag shares one
//!   polygon: the stroke sweeps it, keeping flat faces and sharp corners,
//!   and where a drag crosses into the next block the faces step.
//! * **Layered** (dragon stone): a world-fixed field, not per-dab. A texel of
//!   a dab's disc is dropped when it lies less than `d(p)` inside the disc's
//!   edge, where `d` depends only on the texel's world position, the world
//!   seed and the material, so overlapping dabs (joined by
//!   [`super::World::paint_into`]) agree on the edge. The field follows the
//!   grain the renderer's slots follow (the ridge direction): saw teeth in
//!   wide columns across the grain with blunt crowns and straight flanks,
//!   their edges stepping back along it; and
//!   the renderer's own slot field is mirrored here ([`slot_measure`]), so a
//!   hole that reaches the edge breaks through it as a notch. Teeth cut
//!   less on the underside of a dab than on its top and sides (the one
//!   dab-local factor), so a stone stands on a broad foot and tapers up to
//!   its teeth. Thin spurs left between cutouts are trimmed away.
//!
//! No clock and no unseeded randomness: the same seed and strokes give the
//! same stone. Each dab keeps an untrimmed core (the layered field cuts no
//! deeper than [`MAX_CUT`] of the brush radius, the polygon's faces stay at
//! least a third of it from the centre), so a stone stays about the size
//! painted and consecutive dabs of a stroke stay in one piece; only what
//! reaches the core through drawn quarters is kept, so no chip floats free.
//! Tiny dabs are left round.

use core::f32::consts::FRAC_1_SQRT_2;

use super::hardscape::{disc_limit, disc_mask, quarter_bit};
use crate::material::OutlineShape;
use crate::rng::splitmix64;

/// Smallest brush radius, in cells, that is weathered.
const MIN_RADIUS: i64 = 3;
/// Deepest cut, as a fraction of the brush radius.
const MAX_CUT: f32 = 0.60;
/// Deepest cut, in texels (quarter cells), however big the brush.
const MAX_CUT_TEXELS: f32 = 30.0;
/// Deepest chip out of a polygon's edge, in texels.
const CHIP_TEXELS: f32 = 2.5;
/// The ridge direction of layered stone, as a unit vector (along the
/// strata: up and a little to the right) and its normal. The renderer's
/// dragon stone texture (`DRAGON_ALONG` in `tank.wgsl`) follows the same
/// direction.
const RIDGE: (f32, f32) = (0.287, -0.958);
const RIDGE_NORMAL: (f32, f32) = (0.958, 0.287);

/// Unit vectors at 11.25° steps (a table, not `sin`/`cos`, so every
/// platform draws the same stone).
const DIRS: [(f32, f32); 32] = [
    (1.000_000, 0.000_000),
    (0.980_785, 0.195_090),
    (0.923_880, 0.382_683),
    (0.831_470, 0.555_570),
    (FRAC_1_SQRT_2, FRAC_1_SQRT_2),
    (0.555_570, 0.831_470),
    (0.382_683, 0.923_880),
    (0.195_090, 0.980_785),
    (0.000_000, 1.000_000),
    (-0.195_090, 0.980_785),
    (-0.382_683, 0.923_880),
    (-0.555_570, 0.831_470),
    (-FRAC_1_SQRT_2, FRAC_1_SQRT_2),
    (-0.831_470, 0.555_570),
    (-0.923_880, 0.382_683),
    (-0.980_785, 0.195_090),
    (-1.000_000, 0.000_000),
    (-0.980_785, -0.195_090),
    (-0.923_880, -0.382_683),
    (-0.831_470, -0.555_570),
    (-FRAC_1_SQRT_2, -FRAC_1_SQRT_2),
    (-0.555_570, -0.831_470),
    (-0.382_683, -0.923_880),
    (-0.195_090, -0.980_785),
    (0.000_000, -1.000_000),
    (0.195_090, -0.980_785),
    (0.382_683, -0.923_880),
    (0.555_570, -0.831_470),
    (FRAC_1_SQRT_2, -FRAC_1_SQRT_2),
    (0.831_470, -0.555_570),
    (0.923_880, -0.382_683),
    (0.980_785, -0.195_090),
];

/// The world-fixed noise source of one material in one world.
#[derive(Clone, Copy, Debug)]
pub(super) struct Weathering {
    shape: OutlineShape,
    key: u64,
}

impl Weathering {
    /// The weathering of `material` (its ID) for `shape` in a world with
    /// `seed`.
    pub(super) fn new(shape: OutlineShape, seed: u64, material: u8) -> Self {
        Self {
            shape,
            key: splitmix64(seed ^ splitmix64(0x57_0E_E0 ^ u64::from(material))),
        }
    }

    /// Whether any outline is cut (a plain brush skips the field).
    pub(super) fn is_shaped(self, r: i64) -> bool {
        self.shape != OutlineShape::Round && r >= MIN_RADIUS
    }

    /// Weathers the dab of radius `r` centred on cell `(cx, cy)`: which of
    /// its disc's quarters are drawn.
    pub(super) fn dab(self, cx: i64, cy: i64, r: i64) -> Dab {
        // Texels: quarter cells, the dab's centre at (4cx + 2, 4cy + 2).
        let radius = to_f32(disc_limit(r)).sqrt() / 2.0;
        let cut_max = (radius * MAX_CUT).min(MAX_CUT_TEXELS);
        let half = to_i64_f(radius.ceil()) + 2;
        let side = usize::try_from(2 * half).unwrap_or(0);
        let origin = (4 * cx + 2 - half, 4 * cy + 2 - half);
        let facets = (self.shape == OutlineShape::Angular).then(|| self.facets(cx, cy, radius));
        // The texels inside which nothing is trimmed, as a distance.
        let core = if facets.is_some() {
            0.3 * radius
        } else {
            radius - cut_max
        };
        let mut raw = vec![false; side * side];
        // Kept: the core, then whatever reaches it through kept texels.
        let mut kept = vec![false; side * side];
        let mut stack = Vec::new();
        for iy in 0..side {
            for ix in 0..side {
                let ox = to_f32(to_i64(ix) - half) + 0.5;
                let oy = to_f32(to_i64(iy) - half) + 0.5;
                let dist = (ox * ox + oy * oy).sqrt();
                let at = iy * side + ix;
                raw[at] = if dist > radius {
                    false
                } else if dist <= core {
                    true
                } else {
                    let (px, py) = (
                        to_f32(origin.0 + to_i64(ix)) + 0.5,
                        to_f32(origin.1 + to_i64(iy)) + 0.5,
                    );
                    if let Some(facets) = &facets {
                        // How far inside the polygon's nearest face, less
                        // the chips taken out of its edge.
                        let inset = facets
                            .iter()
                            .map(|&(n, d)| d - (n.0 * ox + n.1 * oy))
                            .fold(f32::INFINITY, f32::min);
                        inset >= self.chip(px, py) * CHIP_TEXELS
                    } else {
                        // Teeth cut less under the dab than on top and at its
                        // sides: a broad foot.
                        let under = ((oy / dist - 0.2) / 0.6).clamp(0.0, 1.0);
                        let top = ((-oy / dist - 0.3) / 0.5).clamp(0.0, 1.0);
                        let reach = (1.0 - 0.70 * under) * (1.0 + 0.3 * top);
                        radius - dist >= (self.layered(px, py, cut_max) * reach).min(1.0) * cut_max
                    }
                };
                if raw[at] && dist <= core {
                    kept[at] = true;
                    // The ring just inside the core seeds the flood.
                    if dist > core - 1.5 {
                        stack.push((ix, iy));
                    }
                }
            }
        }
        if facets.is_none() {
            fill_enclosed(&mut raw, side);
            raw = trim_spurs(&raw, &kept, side);
        }
        // Keep only what reaches the core, so nothing floats free of the stone.
        while let Some((ix, iy)) = stack.pop() {
            let around = [
                (ix.wrapping_sub(1), iy),
                (ix + 1, iy),
                (ix, iy.wrapping_sub(1)),
                (ix, iy + 1),
            ];
            for (nx, ny) in around {
                if nx < side && ny < side && raw[ny * side + nx] && !kept[ny * side + nx] {
                    kept[ny * side + nx] = true;
                    stack.push((nx, ny));
                }
            }
        }
        Dab {
            r,
            origin,
            side,
            kept,
        }
    }

    /// The faces of the polygon dabs near cell `(cx, cy)` take: a unit
    /// normal and how far from the centre, in texels, each cuts.
    fn facets(self, cx: i64, cy: i64, radius: f32) -> Vec<((f32, f32), f32)> {
        let h = self.hash(8, cx >> 4, cy >> 4);
        let n = 5 + usize::from(h & 1 == 1);
        // Reduce in u64: `as usize` first would truncate to 32 bits on wasm and
        // change the remainder, so a stone would differ between targets.
        let turn = usize::try_from((h >> 8) % 32).unwrap_or(0);
        (0..n)
            .map(|k| {
                let hk = splitmix64(h ^ splitmix64(k as u64));
                // Evenly spread directions, each nudged by up to a step.
                let jitter = usize::try_from((hk >> 8) % 3).unwrap_or(0) + 31;
                let dir = DIRS[(turn + k * 32 / n + jitter) % 32];
                // Sides are cut back harder than tops: taller than wide.
                let reach = 0.5 + 0.48 * unit(hk >> 24);
                let sides = 1.0 - 0.25 * dir.0.abs();
                (dir, radius * reach * sides)
            })
            .collect()
    }

    /// Chips in `0..1`: a fine lattice, sparse.
    fn chip(self, px: f32, py: f32) -> f32 {
        self.lattice(px / 4.5, py / 4.5, 2)
    }

    /// How deep the grain cuts at world texel `(px, py)`, as a fraction of
    /// the deepest cut: `1` where a hole of the renderer's slot field
    /// breaks the edge, otherwise saw teeth along the grain.
    fn layered(self, px: f32, py: f32, cut_max: f32) -> f32 {
        // Cells, then ridge coordinates: `a` across the grain, `b` along it
        // (the shader's `dragon_stone_colour`).
        let (wx, wy) = (px / 4.0, py / 4.0);
        let a = wx * RIDGE_NORMAL.0 + wy * RIDGE_NORMAL.1;
        let b = wx * RIDGE.0 + wy * RIDGE.1;
        if hole_at(a, b) {
            return 1.0;
        }
        self.teeth(
            (a, b),
            (0.42 * cut_max).clamp(3.5, 9.0),
            (0.85 * cut_max).clamp(6.0, 16.0),
        )
    }

    /// Each column has a broad, slightly off-centre crown and straight
    /// flanks that meet at shared valleys. A shallow step along the grain
    /// offsets the crown and flanks together.
    fn teeth(self, ridge: (f32, f32), width: f32, period: f32) -> f32 {
        let column = (ridge.0 / width).floor();
        let frac = ridge.0 / width - column;
        let col = to_i64_f(column);
        let hash = self.hash(1, col, 0);
        let crown_start = 0.22 + 0.16 * unit(hash >> 8);
        let crown_end = crown_start + 0.40;
        let crown = 0.10 + 0.10 * unit(hash >> 32);
        let valley = |c: i64| 0.40 + 0.14 * unit(self.hash(2, c, 0));
        let flank = if frac < crown_start {
            crown + (valley(col) - crown) * (crown_start - frac) / crown_start
        } else if frac > crown_end {
            crown + (valley(col + 1) - crown) * (frac - crown_end) / (1.0 - crown_end)
        } else {
            crown
        };
        let phase = ridge.1 / period + unit(hash >> 16);
        let ledge = (1.0 - (phase - phase.floor()) * 1.5).max(0.0);
        flank + 0.12 * ledge
    }

    /// Piecewise-linear value noise in `0..1` at lattice coordinates
    /// `(u, v)`, its values squared (so mostly low), on a lattice
    /// triangulated along a hashed diagonal.
    fn lattice(self, u: f32, v: f32, salt: u64) -> f32 {
        let (i, j) = (u.floor(), v.floor());
        let (fu, fv) = (u - i, v - j);
        let (i, j) = (to_i64_f(i), to_i64_f(j));
        let val = |a: i64, b: i64| {
            let v = unit(self.hash(salt, a, b));
            v * v
        };
        let (v00, v10, v01, v11) = (val(i, j), val(i + 1, j), val(i, j + 1), val(i + 1, j + 1));
        if self.hash(salt + 16, i, j) & 1 == 0 {
            // Split along v00–v11.
            if fu >= fv {
                v00 + fu * (v10 - v00) + fv * (v11 - v10)
            } else {
                v00 + fv * (v01 - v00) + fu * (v11 - v01)
            }
        } else if fu + fv <= 1.0 {
            // Split along v10–v01.
            v00 + fu * (v10 - v00) + fv * (v01 - v00)
        } else {
            v11 + (1.0 - fu) * (v01 - v11) + (1.0 - fv) * (v10 - v11)
        }
    }

    fn hash(self, salt: u64, a: i64, b: i64) -> u64 {
        #[allow(clippy::cast_sign_loss)] // a bit pattern
        splitmix64(self.key ^ splitmix64(salt ^ splitmix64(a as u64 ^ splitmix64(b as u64))))
    }
}

/// One weathered dab: which texels of its disc are drawn.
pub(super) struct Dab {
    r: i64,
    /// World texel of the bitmap's top left corner.
    origin: (i64, i64),
    side: usize,
    kept: Vec<bool>,
}

impl Dab {
    /// The mask of cell `(x, y)` (world coordinates) for the dab centred on
    /// cell `(cx, cy)`: [`disc_mask`] with the trimmed quarters removed.
    /// Empty when the whole cell is trimmed away.
    pub(super) fn mask(&self, cx: i64, cy: i64, x: i64, y: i64) -> u16 {
        let disc = disc_mask(x - cx, y - cy, self.r);
        let mut m = 0;
        for qy in 0..4 {
            for qx in 0..4 {
                if disc & quarter_bit(qx, qy) == 0 {
                    continue;
                }
                let ix = 4 * x + to_i64(qx) - self.origin.0;
                let iy = 4 * y + to_i64(qy) - self.origin.1;
                if let (Ok(ix), Ok(iy)) = (usize::try_from(ix), usize::try_from(iy))
                    && ix < self.side
                    && iy < self.side
                    && self.kept[iy * self.side + ix]
                {
                    m |= quarter_bit(qx, qy);
                }
            }
        }
        m
    }
}

/// Whether the renderer's dragon stone slot field has a hole at `(a, b)`
/// (ridge coordinates in cells), a little dilated so a notch is wider than
/// the dark slot it opens.
fn hole_at(a: f32, b: f32) -> bool {
    let dens = smoothstep(0.30, 0.70, value_noise(a * 0.10 + 5.0, b * 0.07 + 5.0));
    let big = slot_measure(a, b, 3.3, 9.5, 211, dens * 0.65);
    if big < 1.8 {
        big < 1.3
    } else {
        slot_measure(a, b, 1.7, 4.6, 57, dens * 0.40 + 0.03) < 1.35
    }
}

/// `dragon_slot` of `tank.wgsl`: columns `wid` wide across the grain, each
/// shifted along it by its own offset and cut into segments `seg` long,
/// a segment holding a slot with probability `dens`. Returns the slot
/// measure (below 1 inside, large when none). (The edge's exponent is 2.5,
/// not 2.4, to stay off `powf`, whose results differ between platforms.)
fn slot_measure(a: f32, b: f32, wid: f32, seg: f32, salt: i32, dens: f32) -> f32 {
    let col = to_i32((a / wid).floor());
    let hc = gpu_hash(col, salt);
    let bo = b + f32_from(hc & 255) / 255.0 * seg;
    let row = to_i32((bo / seg).floor());
    let h = gpu_hash(col + salt * 7, row + salt + 3);
    if f32_from(h & 255) / 255.0 >= dens {
        return 9.0;
    }
    let hw = (0.17 + f32_from((h >> 8) & 7) / 7.0 * 0.17) * wid;
    let mut hl = (0.13 + f32_from((h >> 11) & 7) / 7.0 * 0.19) * seg;
    if (h >> 14).trailing_zeros() >= 3 {
        hl *= 1.6;
    }
    hl = hl.min(seg * 0.46);
    let ca = hw + (wid - 2.0 * hw) * (f32_from((h >> 17) & 15) / 15.0);
    let cb = hl + (seg - 2.0 * hl) * (f32_from((h >> 21) & 15) / 15.0);
    let salt = to_f32(i64::from(salt));
    let mut dx = (a - to_f32(i64::from(col)) * wid - ca) / hw;
    let dy = (bo - to_f32(i64::from(row)) * seg - cb) / hl;
    dx += (value_noise(a * 1.3 + salt, b * 0.6 + salt) - 0.5) * 0.8;
    let dy = dy + (value_noise(a * 0.7 + salt + 31.0, b * 1.1 + salt + 31.0) - 0.5) * 0.4;
    dx /= (1.0 - 0.45 * dy * dy).max(0.3);
    dx * dx + dy.abs() * dy.abs().sqrt() * dy.abs()
}

/// The renderer's integer hash (`hash` in `tank.wgsl`).
fn gpu_hash(x: i32, y: i32) -> u32 {
    let mut h =
        x.cast_unsigned().wrapping_mul(0x8da6_b343) ^ y.cast_unsigned().wrapping_mul(0xd816_3841);
    h = (h ^ (h >> 13)).wrapping_mul(0x5bd1_e995);
    h ^ (h >> 15)
}

/// The renderer's smooth value noise (`vnoise`), one lattice step per unit.
fn value_noise(x: f32, y: f32) -> f32 {
    let (i, j) = (x.floor(), y.floor());
    let (fx, fy) = (x - i, y - j);
    let (ux, uy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (i, j) = (to_i32(i), to_i32(j));
    let at = |a: i32, b: i32| f32_from(gpu_hash(a, b) & 1023) / 1024.0;
    let low = at(i, j) + (at(i + 1, j) - at(i, j)) * ux;
    let high = at(i, j + 1) + (at(i + 1, j + 1) - at(i, j + 1)) * ux;
    low + (high - low) * uy
}

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Fills every undrawn texel of the `side`×`side` bitmap that the outside
/// (its border, through 8-connected undrawn texels) cannot reach, so a
/// notch only exists where it opens onto the edge.
fn fill_enclosed(raw: &mut [bool], side: usize) {
    let mut outside = vec![false; side * side];
    let mut stack = Vec::new();
    for i in 0..side {
        for (x, y) in [(i, 0), (i, side - 1), (0, i), (side - 1, i)] {
            if !raw[y * side + x] && !outside[y * side + x] {
                outside[y * side + x] = true;
                stack.push((x, y));
            }
        }
    }
    while let Some((x, y)) = stack.pop() {
        for ny in y.saturating_sub(1)..=(y + 1).min(side - 1) {
            for nx in x.saturating_sub(1)..=(x + 1).min(side - 1) {
                if !raw[ny * side + nx] && !outside[ny * side + nx] {
                    outside[ny * side + nx] = true;
                    stack.push((nx, ny));
                }
            }
        }
    }
    for (drawn, out) in raw.iter_mut().zip(&outside) {
        *drawn |= !out;
    }
}

/// Outside the protected core, keep only texels that fit within a solid
/// cell-sized square. This clips slivers between slot notches and the edge.
fn trim_spurs(raw: &[bool], core: &[bool], side: usize) -> Vec<bool> {
    const WIDTH: usize = 4;
    let mut solid = core.to_vec();
    if side < WIDTH {
        return solid;
    }
    for y in 0..=side - WIDTH {
        for x in 0..=side - WIDTH {
            let rows = (y..y + WIDTH).map(|row| row * side + x..row * side + x + WIDTH);
            if rows.clone().all(|row| raw[row].iter().all(|&drawn| drawn)) {
                for row in rows {
                    solid[row].fill(true);
                }
            }
        }
    }
    solid
}

/// The low 24 bits of a hash as a value in `0..1`.
fn unit(h: u64) -> f32 {
    #[allow(clippy::cast_precision_loss)] // 24 bits fit f32 exactly
    {
        (h & 0x00FF_FFFF) as f32 / 16_777_216.0
    }
}

#[allow(clippy::cast_precision_loss)] // texel coordinates are small
fn to_f32(x: i64) -> f32 {
    x as f32
}

#[allow(clippy::cast_precision_loss)] // small hash fields
fn f32_from(x: u32) -> f32 {
    x as f32
}

#[allow(clippy::cast_possible_truncation)] // floored lattice coordinates are small
fn to_i32(x: f32) -> i32 {
    x as i32
}

#[allow(clippy::cast_possible_truncation)] // floored lattice coordinates are small
fn to_i64_f(x: f32) -> i64 {
    x as i64
}

#[allow(clippy::cast_possible_wrap)] // quarter indices are 0..4
const fn to_i64(x: usize) -> i64 {
    x as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texels of a layered dab's disc that are trimmed away, split into
    /// those above the centre and those below it.
    fn trimmed_halves(cx: i64, cy: i64, r: i64) -> (usize, usize) {
        let dab = Weathering::new(OutlineShape::Layered, 3, 9).dab(cx, cy, r);
        let radius = to_f32(disc_limit(r)).sqrt() / 2.0;
        let half = to_f32(to_i64(dab.side) / 2);
        let (mut above, mut below) = (0, 0);
        for iy in 0..dab.side {
            for ix in 0..dab.side {
                let (ox, oy) = (
                    to_f32(to_i64(ix)) + 0.5 - half,
                    to_f32(to_i64(iy)) + 0.5 - half,
                );
                if ox * ox + oy * oy <= radius * radius && !dab.kept[iy * dab.side + ix] {
                    if oy < 0.0 {
                        above += 1;
                    } else {
                        below += 1;
                    }
                }
            }
        }
        (above, below)
    }

    #[test]
    fn layered_dabs_are_trimmed_harder_on_top_than_underneath() {
        for r in [5, 7, 9] {
            for (cx, cy) in [(40, 40), (47, 61), (90, 13), (13, 77)] {
                let (above, below) = trimmed_halves(cx, cy, r);
                assert!(
                    above * 10 > below * 13,
                    "r {r} at ({cx}, {cy}): {above} trimmed above, {below} below"
                );
            }
        }
    }

    #[test]
    fn layered_teeth_have_broad_crowns_and_shallow_valleys() {
        let field = Weathering::new(OutlineShape::Layered, 3, 9);
        let width = 8.0;
        for col in -8..8 {
            let cut = |frac| field.teeth(((to_f32(col) + frac) * width, 7.0), width, 15.0);
            let crown = cut(0.5);
            // Every slightly offset crown covers the middle fifth of its column.
            assert!((cut(0.4) - crown).abs() < f32::EPSILON);
            assert!((cut(0.6) - crown).abs() < f32::EPSILON);
            for valley in [cut(0.0), cut(0.999)] {
                assert!((0.18..0.46).contains(&(valley - crown)));
            }
        }
    }

    #[test]
    fn trimming_a_spur_preserves_a_broad_tooth_and_its_notch() {
        let rows = [
            "..#......",
            "..#......",
            "########.",
            "########.",
            "########.",
            "########.",
            "#####....",
            "#####....",
            "#####....",
        ];
        let raw: Vec<_> = rows
            .iter()
            .flat_map(|s| s.bytes().map(|b| b == b'#'))
            .collect();
        let solid = trim_spurs(&raw, &[false; 81], 9);
        assert!(solid[..18].iter().all(|&drawn| !drawn));
        assert_eq!(solid[18..], raw[18..]);
    }

    #[test]
    fn the_slot_field_has_holes_in_some_places_only() {
        let (mut holes, mut total) = (0, 0);
        for j in 0..120 {
            for i in 0..120 {
                holes += usize::from(hole_at(to_f32(i) * 0.5, to_f32(j) * 0.5));
                total += 1;
            }
        }
        assert!(holes * 50 > total, "{holes} of {total}: holes are rare");
        assert!(
            holes * 3 < total,
            "{holes} of {total}: holes are everywhere"
        );
        // No slot where the density says none.
        assert!((slot_measure(0.1, 0.1, 3.3, 9.5, 211, 0.0) - 9.0).abs() < f32::EPSILON);
    }
}
