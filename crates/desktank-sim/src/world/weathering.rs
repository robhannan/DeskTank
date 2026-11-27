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
//! * **Layered** (dragon stone): a world-fixed field, not per-dab. A quarter
//!   of a dab's disc is dropped when it lies less than `d(p)` inside the
//!   disc's edge, where `d` depends only on the quarter's world position,
//!   the world seed and the material, so overlapping dabs (joined by
//!   [`super::World::paint_into`]) agree on the edge and drawing over a spot
//!   again changes nothing. Strata run along one ridge direction; each
//!   stratum, in ledges along the ridge, sits in or out by its own amount,
//!   so the edge steps, and hash-placed bites take notches out of it.
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
const MAX_CUT: f32 = 0.40;
/// Deepest cut, in texels (quarter cells), however big the brush.
const MAX_CUT_TEXELS: f32 = 22.0;
/// Deepest chip out of a polygon's edge, in texels.
const CHIP_TEXELS: f32 = 2.5;
/// The ridge direction of layered stone, as a unit vector (along the
/// strata, tilted a little downwards to the right) and its normal. The
/// renderer's dragon stone texture follows the same direction.
const RIDGE: (f32, f32) = (0.990_268, 0.139_173);
const RIDGE_NORMAL: (f32, f32) = (-0.139_173, 0.990_268);

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
                    match &facets {
                        Some(facets) => {
                            // How far inside the polygon's nearest face, less
                            // the chips taken out of its edge.
                            let inset = facets
                                .iter()
                                .map(|&(n, d)| d - (n.0 * ox + n.1 * oy))
                                .fold(f32::INFINITY, f32::min);
                            inset >= self.chip(px, py) * CHIP_TEXELS
                        }
                        None => radius - dist >= self.layered(px, py, radius, cut_max),
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
        let turn = (h >> 8) as usize % 32;
        (0..n)
            .map(|k| {
                let hk = splitmix64(h ^ splitmix64(k as u64));
                // Evenly spread directions, each nudged by up to a step.
                let jitter = (hk >> 8) as usize % 3 + 31;
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

    /// How deep, in texels, ledges and bites cut at world texel `(px, py)`.
    fn layered(self, px: f32, py: f32, radius: f32, cut_max: f32) -> f32 {
        let thick = (radius / 5.0).round().clamp(4.0, 9.0);
        let along = px * RIDGE.0 + py * RIDGE.1;
        let across = px * RIDGE_NORMAL.0 + py * RIDGE_NORMAL.1;
        let layer = to_i64_f((across / thick).floor());
        let ledge = to_i64_f((along / (2.5 * thick)).floor());
        // Each ledge of each layer sits in or out by its own amount.
        let step = unit(self.hash(3, layer, ledge));
        let layer_bias = unit(self.hash(4, layer, 0));
        let mut cut = 0.05 + 0.95 * step * step * (0.4 + layer_bias);
        // Bites: a circle per lattice square now and then.
        let size = (radius / 3.0).round().clamp(6.0, 16.0);
        let (col, row) = ((px / size).floor(), (py / size).floor());
        let hash = self.hash(5, to_i64_f(col), to_i64_f(row));
        if hash & 0b111 < 3 {
            let (bx, by) = (
                (col + unit(hash >> 8)) * size,
                (row + unit(hash >> 24)) * size,
            );
            let bite = (0.18 + 0.2 * unit(hash >> 40)) * size;
            if (px - bx) * (px - bx) + (py - by) * (py - by) < bite * bite {
                cut = cut.max(1.3);
            }
        }
        (cut * cut_max).clamp(0.0, cut_max)
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

#[allow(clippy::cast_possible_truncation)] // floored lattice coordinates are small
fn to_i64_f(x: f32) -> i64 {
    x as i64
}

#[allow(clippy::cast_possible_wrap)] // quarter indices are 0..4
const fn to_i64(x: usize) -> i64 {
    x as i64
}
