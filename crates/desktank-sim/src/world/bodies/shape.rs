//! A rigid body's reference shape and its rasterisation at any angle
//! (SPEC §2.1 *Rigid bodies*, T4c), in fixed point so every platform
//! (native and wasm) gets the same cells.
//!
//! Units: positions are in 1/256 cell ([`ONE`]); angles in 1/65536 turn
//! ([`TURN`]); `sin`/`cos` come from a table built at compile time from an
//! integer Taylor series, never from the platform's maths library.

use crate::save::{SaveError, v1};
use crate::world::MAX_SIDE;
use crate::world::hardscape::{FULL_MASK, quarter_bit};

/// One cell in position units.
pub(in crate::world) const ONE: i64 = 256;
/// A full turn in angle units.
pub(super) const TURN: i64 = 65_536;
/// Angle units per raster step: the drawn angle moves in 1/256 turn
/// (1.4°) steps, so a slowly turning body is redrawn only every few ticks.
pub(super) const RASTER_STEP: i64 = 256;
/// `sin`/`cos` fixed-point scale (Q16).
const TRIG_ONE: i64 = 65_536;
/// Table entries per quarter turn (each 1/1024 turn), plus the end point.
const QUARTER_STEPS: usize = 256;

/// `sin` over a quarter turn at 1/1024-turn steps, Q16.
const SIN_TABLE: [i64; QUARTER_STEPS + 1] = sin_table();

/// `sin(k · 2π / 1024)` for `k` in `0..=256`, Q16, by a Taylor series in
/// Q60 integer arithmetic (exact to the last Q16 bit, and evaluated at
/// compile time, so no platform maths library is involved).
const fn sin_table() -> [i64; QUARTER_STEPS + 1] {
    // π · 2^60.
    const PI_Q60: i128 = 3_622_009_729_038_561_421;
    let mut t = [0; QUARTER_STEPS + 1];
    let mut k = 0;
    while k <= QUARTER_STEPS {
        let x = k as i128 * PI_Q60 / 512;
        let x2 = (x * x) >> 60;
        let mut term = x;
        let mut sum = x;
        let mut n = 1;
        while n < 12 {
            term = -(((term * x2) >> 60) / ((2 * n) * (2 * n + 1)));
            sum += term;
            n += 1;
        }
        #[allow(clippy::cast_possible_truncation)] // |sin| ≤ 1: fits Q16
        {
            t[k] = ((sum + (1 << 43)) >> 44) as i64;
        }
        k += 1;
    }
    t
}

/// `sin(angle)`, Q16, for an angle in 1/65536 turn (any integer; wraps).
pub(super) fn sin(angle: i64) -> i64 {
    let a = angle.rem_euclid(TURN);
    let quarter = TURN / 4;
    let (r, neg) = match a / quarter {
        0 => (a, false),
        1 => (2 * quarter - a, false),
        2 => (a - 2 * quarter, true),
        _ => (TURN - a, true),
    };
    // r in 0..=16384; table steps are 64 angle units.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=256
    let idx = (r >> 6) as usize;
    let frac = r & 63;
    let s = if frac == 0 {
        SIN_TABLE[idx]
    } else {
        SIN_TABLE[idx] + (((SIN_TABLE[idx + 1] - SIN_TABLE[idx]) * frac) >> 6)
    };
    if neg { -s } else { s }
}

/// `cos(angle)`, Q16.
pub(super) fn cos(angle: i64) -> i64 {
    sin(angle + TURN / 4)
}

/// Rotates `(x, y)` (any fixed-point unit) by `angle` (clockwise on screen,
/// as y points down), rounding to the nearest unit.
pub(super) fn rotate(x: i64, y: i64, angle: i64) -> (i64, i64) {
    let (c, s) = (cos(angle), sin(angle));
    let half = TRIG_ONE / 2;
    (
        (c * x - s * y + half).div_euclid(TRIG_ONE),
        (s * x + c * y + half).div_euclid(TRIG_ONE),
    )
}

/// A body's shape at angle 0: a bitmap of its cells (with each cell's
/// colour-noise variant), its centre of mass and moment of inertia.
#[derive(Clone, Debug, Default)]
pub(super) struct Shape {
    /// Bitmap width and height, in cells.
    w: usize,
    h: usize,
    /// Row-major: 0 for empty, else the cell's variant + 1.
    grid: Vec<u16>,
    /// Row-major sub-cell outline masks of the cells (`world::hardscape`);
    /// empty when every cell is whole.
    masks: Vec<u16>,
    /// Number of cells.
    pub(super) n: i64,
    /// Centre of mass relative to the bitmap's top-left corner, in
    /// position units (every cell weighs the same: a body is one material).
    pub(super) com: (i64, i64),
    /// Moment of inertia about the centre of mass per unit cell mass, in
    /// position units squared (Σ r² plus each cell's own 1/6).
    pub(super) inertia: i64,
    /// Farthest cell corner from the centre of mass, in whole cells
    /// (rounded up): the reach of any rotation.
    radius: i64,
}

impl Shape {
    /// The shape of `cells` (world `(x, y, variant, mask)`, non-empty);
    /// returns it with the world cell of the bitmap's top-left corner.
    pub(super) fn new(cells: &[(usize, usize, u8, u16)]) -> (Self, (i64, i64)) {
        let x0 = cells.iter().map(|c| c.0).min().unwrap_or(0);
        let y0 = cells.iter().map(|c| c.1).min().unwrap_or(0);
        let w = cells.iter().map(|c| c.0).max().unwrap_or(0) + 1 - x0;
        let h = cells.iter().map(|c| c.1).max().unwrap_or(0) + 1 - y0;
        let mut grid = vec![0; w * h];
        let partial = cells.iter().any(|c| c.3 != FULL_MASK);
        let mut masks = if partial {
            vec![FULL_MASK; w * h]
        } else {
            Vec::new()
        };
        let (mut sx, mut sy) = (0_i64, 0_i64);
        for &(x, y, v, m) in cells {
            grid[(y - y0) * w + (x - x0)] = u16::from(v) + 1;
            if partial {
                masks[(y - y0) * w + (x - x0)] = m;
            }
            sx += to_i64(x - x0);
            sy += to_i64(y - y0);
        }
        let n = to_i64(cells.len());
        // Centre of each cell is at +0.5.
        let com = (
            (sx * ONE + n * ONE / 2) / n.max(1),
            (sy * ONE + n * ONE / 2) / n.max(1),
        );
        let mut inertia = n * ONE * ONE / 6;
        let mut r2max = 0;
        for &(x, y, _, _) in cells {
            let cx = to_i64(x - x0) * ONE + ONE / 2 - com.0;
            let cy = to_i64(y - y0) * ONE + ONE / 2 - com.1;
            inertia += cx * cx + cy * cy;
            let fx = cx.abs() + ONE / 2;
            let fy = cy.abs() + ONE / 2;
            r2max = r2max.max(fx * fx + fy * fy);
        }
        let shape = Self {
            w,
            h,
            grid,
            masks,
            n,
            com,
            inertia,
            radius: isqrt(r2max).div_euclid(ONE) + 1,
        };
        (shape, (to_i64(x0), to_i64(y0)))
    }

    /// The shape as saved (T8).
    #[allow(clippy::cast_possible_truncation)] // within MAX_SIDE
    pub(super) fn to_v1(&self) -> v1::Shape {
        v1::Shape {
            w: self.w as u32,
            h: self.h as u32,
            grid: self.grid.clone(),
            n: self.n,
            com: self.com,
            inertia: self.inertia,
            radius: self.radius,
        }
    }

    /// A saved shape, checked for consistency: the bitmap's size and cell
    /// count, and a centre and reach within the grid's scale.
    pub(super) fn from_v1(s: v1::Shape) -> Result<Self, SaveError> {
        let (w, h) = (s.w as usize, s.h as usize);
        let side = to_i64(MAX_SIDE);
        let cells = s.grid.iter().filter(|&&v| v != 0).count();
        let sane = w <= MAX_SIDE
            && h <= MAX_SIDE
            && s.grid.len() == w * h
            && to_i64(cells) == s.n
            && (0..=2 * side).contains(&s.radius)
            && s.com.0.abs() <= side * ONE
            && s.com.1.abs() <= side * ONE
            && s.inertia >= 0;
        if !sane {
            return Err(SaveError::Invalid(
                "a body's shape is inconsistent".to_owned(),
            ));
        }
        Ok(Self {
            w,
            h,
            grid: s.grid,
            masks: Vec::new(),
            n: s.n,
            com: s.com,
            inertia: s.inertia,
            radius: s.radius,
        })
    }

    /// The outline masks of the shape's cells, row-major (empty when all
    /// are whole), as saved (version 10).
    pub(super) fn masks(&self) -> &[u16] {
        &self.masks
    }

    /// Restores saved masks: one per bitmap cell, or none.
    pub(super) fn set_masks(&mut self, masks: Vec<u16>) -> Result<(), SaveError> {
        if !masks.is_empty() && masks.len() != self.grid.len() {
            return Err(SaveError::Invalid(
                "a body's outline masks don't fit its shape".to_owned(),
            ));
        }
        self.masks = masks;
        Ok(())
    }

    /// The mask of local cell `(i, j)` (inside the bitmap).
    fn mask_at(&self, i: usize, j: usize) -> u16 {
        self.masks.get(j * self.w + i).copied().unwrap_or(FULL_MASK)
    }

    /// Whether the local point `(u, v)` (position units) falls in a covered
    /// quarter of a shape cell.
    fn covers(&self, u: i64, v: i64) -> bool {
        let (i, j) = (u.div_euclid(ONE), v.div_euclid(ONE));
        if self.at(i, j) == 0 {
            return false;
        }
        let (Ok(i), Ok(j)) = (usize::try_from(i), usize::try_from(j)) else {
            return false;
        };
        let quarter = |f: i64| usize::try_from(f.rem_euclid(ONE) * 4 / ONE).unwrap_or(0);
        self.mask_at(i, j) & quarter_bit(quarter(u), quarter(v)) != 0
    }

    /// The variant + 1 of local cell `(i, j)`, 0 if empty or outside.
    fn at(&self, i: i64, j: i64) -> u16 {
        if i < 0 || j < 0 {
            return 0;
        }
        let (Ok(i), Ok(j)) = (usize::try_from(i), usize::try_from(j)) else {
            return 0;
        };
        if i >= self.w || j >= self.h {
            return 0;
        }
        self.grid[j * self.w + i]
    }

    /// The shape drawn with its bitmap corner at world cell `anchor`,
    /// turned by `angle` about its centre of mass: exactly `n` world cells
    /// `(x, y, variant, mask)`, sorted by `(y, x)`, or `None` if they can't
    /// be found (never for a sane shape).
    ///
    /// Each candidate cell's centre is mapped back into the shape. Cells
    /// that land inside it are ranked by how far they are from its edge,
    /// cells outside by how close; the best `n` are taken, ties by
    /// position. So the count is exact, interior cells are never dropped
    /// (no holes), and angle 0 gives back the shape itself.
    ///
    /// Each cell's outline mask samples the turned shape at its quarters'
    /// centres; one that covers none of them (a cell the count needed past
    /// the outline) is whole.
    pub(super) fn raster(
        &self,
        anchor: (i64, i64),
        angle: i64,
    ) -> Option<Vec<(i64, i64, u8, u16)>> {
        if angle.rem_euclid(TURN) == 0 {
            let mut out = Vec::with_capacity(self.grid.len());
            for j in 0..self.h {
                for i in 0..self.w {
                    let v = self.grid[j * self.w + i];
                    if v != 0 {
                        let m = self.mask_at(i, j);
                        out.push((anchor.0 + to_i64(i), anchor.1 + to_i64(j), low_byte(v), m));
                    }
                }
            }
            return Some(out);
        }
        // World position of the centre of mass.
        let pc = (anchor.0 * ONE + self.com.0, anchor.1 * ONE + self.com.1);
        let reach = self.radius + 1;
        let (x0, x1) = (pc.0.div_euclid(ONE) - reach, pc.0.div_euclid(ONE) + reach);
        let (y0, y1) = (pc.1.div_euclid(ONE) - reach, pc.1.div_euclid(ONE) + reach);
        // (score, y, x, variant); higher scores are better.
        let mut picks: Vec<(i64, i64, i64, u8)> = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x * ONE + ONE / 2 - pc.0, y * ONE + ONE / 2 - pc.1);
                let (lx, ly) = rotate(dx, dy, -angle);
                let (u, v) = (lx + self.com.0, ly + self.com.1);
                if let Some((score, variant)) = self.score(u, v) {
                    picks.push((score, y, x, variant));
                }
            }
        }
        let n = usize::try_from(self.n).ok()?;
        if picks.len() < n {
            return None;
        }
        picks.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        picks.truncate(n);
        let mut out: Vec<_> = picks
            .into_iter()
            .map(|(_, y, x, v)| (x, y, v, self.turned_mask(pc, angle, x, y)))
            .collect();
        out.sort_unstable_by_key(|&(x, y, _, _)| (y, x));
        Some(out)
    }

    /// The outline mask of world cell `(x, y)` for the shape turned by
    /// `angle` about its centre of mass at world position `pc`: each
    /// quarter whose centre maps back into a covered quarter.
    fn turned_mask(&self, pc: (i64, i64), angle: i64, x: i64, y: i64) -> u16 {
        let mut m = 0;
        for qy in 0..4 {
            for qx in 0..4 {
                let px = x * ONE + (2 * to_i64(qx) + 1) * ONE / 8;
                let py = y * ONE + (2 * to_i64(qy) + 1) * ONE / 8;
                let (lx, ly) = rotate(px - pc.0, py - pc.1, -angle);
                if self.covers(lx + self.com.0, ly + self.com.1) {
                    m |= quarter_bit(qx, qy);
                }
            }
        }
        if m == 0 { FULL_MASK } else { m }
    }

    /// How well the local point `(u, v)` (position units) belongs to the
    /// shape, and the variant of the cell it takes: inside, 1 + the squared
    /// distance to the nearest empty neighbour square (deeper is better);
    /// outside, minus the squared distance to the nearest shape square
    /// within one cell; `None` if farther.
    fn score(&self, u: i64, v: i64) -> Option<(i64, u8)> {
        let (i, j) = (u.div_euclid(ONE), v.div_euclid(ONE));
        let (fu, fv) = (u - i * ONE, v - j * ONE);
        // Distance from the point to the neighbour square (a, b), per axis.
        let gap = |a: i64, f: i64| match a {
            -1 => f,
            1 => ONE - f,
            _ => 0,
        };
        let own = self.at(i, j);
        let mut best: Option<(i64, u16)> = None;
        for b in -1..=1 {
            for a in -1..=1 {
                if a == 0 && b == 0 {
                    continue;
                }
                let filled = self.at(i + a, j + b);
                // Inside: look for empty neighbours; outside: filled ones.
                if (own != 0) == (filled != 0) {
                    continue;
                }
                let (gx, gy) = (gap(a, fu), gap(b, fv));
                let d2 = gx * gx + gy * gy;
                if best.is_none_or(|(d, _)| d2 < d) {
                    best = Some((d2, filled));
                }
            }
        }
        if own != 0 {
            let depth = best.map_or(2 * ONE * ONE, |(d, _)| d);
            Some((1 + depth, low_byte(own)))
        } else {
            best.map(|(d, filled)| (-d, low_byte(filled)))
        }
    }
}

/// Integer square root (floor) of a non-negative value.
pub(super) fn isqrt(x: i64) -> i64 {
    // u64::isqrt is exact and the same on every platform.
    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)] // non-negative, < 2^63
    {
        (x.max(0) as u64).isqrt() as i64
    }
}

/// `usize` → `i64` (grid sizes are at most 8192²).
#[allow(clippy::cast_possible_wrap)]
pub(super) fn to_i64(x: usize) -> i64 {
    x as i64
}

/// The variant byte stored as variant + 1.
#[allow(clippy::cast_possible_truncation)] // stored values are 1..=256
fn low_byte(v: u16) -> u8 {
    (v - 1) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn table_trig_matches_libm() {
        for a in (0..TURN).step_by(7) {
            let want = (a as f64 / TURN as f64 * core::f64::consts::TAU).sin();
            let got = sin(a) as f64 / TRIG_ONE as f64;
            assert!(
                (got - want).abs() < 2.0 / TRIG_ONE as f64,
                "{a}: {got} vs {want}"
            );
            let wc = (a as f64 / TURN as f64 * core::f64::consts::TAU).cos();
            assert!((cos(a) as f64 / TRIG_ONE as f64 - wc).abs() < 2.0 / TRIG_ONE as f64);
        }
        assert_eq!(sin(0), 0);
        assert_eq!(sin(TURN / 4), TRIG_ONE);
        assert_eq!(cos(TURN / 2), -TRIG_ONE);
    }

    fn disc_and_bar() -> Vec<(usize, usize, u8, u16)> {
        let mut cells = Vec::new();
        for y in 0..20_usize {
            for x in 0..20_usize {
                let (dx, dy) = (x.abs_diff(10), y.abs_diff(10));
                if dx * dx + dy * dy <= 64 || (y == 10 && x < 20) {
                    #[allow(clippy::cast_possible_truncation)]
                    cells.push((x + 5, y + 7, (x * 7 + y) as u8, FULL_MASK));
                }
            }
        }
        cells
    }

    #[test]
    fn raster_keeps_the_count_and_has_no_holes() {
        let cells = disc_and_bar();
        let (shape, anchor) = Shape::new(&cells);
        // Angle 0 gives the shape back exactly.
        let same = shape.raster(anchor, 0).unwrap();
        let mut want: Vec<_> = cells
            .iter()
            .map(|&(x, y, v, m)| (to_i64(x), to_i64(y), v, m))
            .collect();
        want.sort_unstable_by_key(|&(x, y, _, _)| (y, x));
        assert_eq!(same, want);
        for angle in (0..TURN).step_by(1000) {
            let r = shape.raster(anchor, angle).unwrap();
            assert_eq!(r.len(), cells.len(), "angle {angle}");
            // No duplicates.
            let mut pos: Vec<_> = r.iter().map(|&(x, y, _, _)| (y, x)).collect();
            pos.dedup();
            assert_eq!(pos.len(), r.len());
            // No holes: every cell with all four neighbours filled around
            // an empty one would be a hole; check no empty cell is fully
            // enclosed.
            let filled = |x: i64, y: i64| pos.binary_search(&(y, x)).is_ok();
            for &(x, y, _, _) in &r {
                for (ex, ey) in [(x + 1, y), (x, y + 1)] {
                    if !filled(ex, ey) {
                        let around = [(ex - 1, ey), (ex + 1, ey), (ex, ey - 1), (ex, ey + 1)];
                        assert!(
                            !around.iter().all(|&(a, b)| filled(a, b)),
                            "hole at ({ex}, {ey}), angle {angle}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_quarter_turn_of_a_bar_stands_it_up() {
        let cells: Vec<_> = (0..9).map(|x| (10 + x, 10, 0, FULL_MASK)).collect();
        let (shape, anchor) = Shape::new(&cells);
        let r = shape.raster(anchor, TURN / 4).unwrap();
        assert!(r.iter().all(|&(x, _, _, _)| x == 14), "{r:?}");
        let ys: Vec<_> = r.iter().map(|c| c.1).collect();
        assert_eq!(ys, (6..15).collect::<Vec<_>>());
    }

    /// A disc's outline, as quarter rows: (first, last) covered quarter
    /// column per quarter row.
    fn outline(cells: &[(i64, i64, u8, u16)]) -> std::collections::BTreeMap<i64, (i64, i64)> {
        let mut rows = std::collections::BTreeMap::new();
        for &(x, y, _, m) in cells {
            for qy in 0..4 {
                for qx in 0..4 {
                    if m & quarter_bit(qx, qy) != 0 {
                        let (px, py) = (4 * x + to_i64(qx), 4 * y + to_i64(qy));
                        let e = rows.entry(py).or_insert((px, px));
                        e.0 = e.0.min(px);
                        e.1 = e.1.max(px);
                    }
                }
            }
        }
        rows
    }

    #[test]
    fn masks_turn_with_the_shape() {
        use crate::world::hardscape::disc_mask;
        let r = 10_i64;
        let mut cells = Vec::new();
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r + r {
                    let (x, y) = (
                        usize::try_from(20 + dx).unwrap(),
                        usize::try_from(20 + dy).unwrap(),
                    );
                    cells.push((x, y, 0, disc_mask(dx, dy, r)));
                }
            }
        }
        let (shape, anchor) = Shape::new(&cells);
        let flat = outline(&shape.raster(anchor, 0).unwrap());
        // A disc turned by any angle keeps a round outline: each quarter
        // row spans about as wide as unturned.
        for angle in [TURN / 16, TURN / 8, TURN / 5, TURN / 4] {
            let turned = outline(&shape.raster(anchor, angle).unwrap());
            let width = |o: &std::collections::BTreeMap<i64, (i64, i64)>| -> Vec<i64> {
                o.values().map(|&(a, b)| b - a + 1).collect()
            };
            let (a, b) = (width(&flat), width(&turned));
            assert!(
                a.len().abs_diff(b.len()) <= 2,
                "angle {angle}: {} vs {} rows",
                a.len(),
                b.len()
            );
            let wa: i64 = a.iter().sum();
            let wb: i64 = b.iter().sum();
            assert!(
                (wa - wb).abs() * 50 <= wa,
                "angle {angle}: area {wa} vs {wb}"
            );
        }
    }
}
