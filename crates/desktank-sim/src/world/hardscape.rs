//! Sub-cell outlines of rigid cells (rock, driftwood): a 4×4 coverage mask
//! per cell, so a round brush draws a round stone at 4× while the physics
//! stays per cell.
//!
//! Bit `qy * 4 + qx` covers quarter-cell `(qx, qy)` (x right, y down). A
//! cell is rigid by its material alone; the mask only says which of its
//! quarters are drawn. Fully covered cells, the common case, store
//! nothing: the world keeps a sparse map of partial masks by cell index,
//! so the grid stays at 4 bytes per cell (SPEC §2.1) and a settled tank
//! pays nothing per tick.
//!
//! Masks are set where rigid cells are made: a brush or pour
//! ([`World::paint_into`]) rasterises its disc at quarter-cell resolution
//! (weathered into a natural outline first for materials with a `shape`,
//! see [`super::weathering`]) and ORs the coverage into rigid cells of its
//! material that it covers. A
//! rectangle fill, `set_cell` or any other write gives a full cell. A mask
//! moves with its cell (swaps and body moves); a body that turns is redrawn
//! from its reference shape, which keeps every cell's mask, by sampling
//! each quarter's centre back into the shape. An entry left on a cell that
//! is no longer rigid is ignored, and saves and the state hash skip it.

use super::World;
use crate::hash::Fnv64;
use crate::material::MovementClass;
use crate::save::SaveError;

/// Every quarter covered.
pub const FULL_MASK: u16 = 0xFFFF;

/// The bit of quarter `(qx, qy)`, each in `0..4`.
#[must_use]
pub const fn quarter_bit(qx: usize, qy: usize) -> u16 {
    1 << (qy * 4 + qx)
}

/// The quarters of cell `(dx, dy)` (relative to a brush centred on cell
/// `(0, 0)`) inside the brush's disc of radius `r` cells: those whose
/// centre lies within about `r` (see `disc_limit`) of the centre cell's
/// centre. Radius 0 draws a rounded dot of radius ½. A cell the brush takes (`dx² + dy² ≤ r² + r`)
/// whose quarters all miss the disc (a few at some radii) gets the
/// quarters within its own selection radius instead, and full if still
/// none, so a rigid cell is never invisible.
#[must_use]
pub fn disc_mask(dx: i64, dy: i64, r: i64) -> u16 {
    let m = within(dx, dy, disc_limit(r));
    if m != 0 {
        return m;
    }
    match within(dx, dy, 64 * (r * r + r) + 16) {
        0 => FULL_MASK,
        m => m,
    }
}

/// The squared radius of a brush of radius `r` cells, in eighths of a
/// cell squared: √(r² + r/4), or ½ for radius 0. The cells a brush takes
/// reach √(r² + r) ≈ r + ½; this slightly smaller circle keeps every
/// quarter it covers inside those cells (the outline stays within a texel
/// of a true circle at 4×) while trimming straight stroke sides by little
/// more than a texel.
pub(super) fn disc_limit(r: i64) -> i64 {
    if r == 0 { 16 } else { 64 * r * r + 16 * r }
}

/// The quarters of cell `(dx, dy)` whose centres lie within √`limit`
/// eighths of a cell of cell `(0, 0)`'s centre.
fn within(dx: i64, dy: i64, limit: i64) -> u16 {
    // Positions in eighths of a cell: quarter centres sit at -3, -1, 1, 3.
    let mut m = 0;
    for qy in 0..4 {
        for qx in 0..4 {
            let px = 8 * dx + 2 * to_i64(qx) - 3;
            let py = 8 * dy + 2 * to_i64(qy) - 3;
            if px * px + py * py <= limit {
                m |= quarter_bit(qx, qy);
            }
        }
    }
    m
}

/// `usize` → `i64` for quarter indices (0..4).
#[allow(clippy::cast_possible_wrap)]
const fn to_i64(x: usize) -> i64 {
    x as i64
}

impl World {
    /// Whether cell `i` holds a rigid material.
    #[inline]
    pub(super) fn is_rigid(&self, i: usize) -> bool {
        self.props[usize::from(self.cells[i].material.0)].class == MovementClass::Rigid
    }

    /// The coverage mask of rigid cell `i` (bit `qy * 4 + qx` per
    /// quarter, [`FULL_MASK`] when whole), or `None` if the cell isn't
    /// rigid. The renderer draws only the covered quarters.
    #[must_use]
    pub fn rigid_mask(&self, i: usize) -> Option<u16> {
        if i >= self.cells.len() || !self.is_rigid(i) {
            return None;
        }
        Some(self.masks.get(&i).copied().unwrap_or(FULL_MASK))
    }

    /// [`World::rigid_mask`] of cell `(x, y)`: [`FULL_MASK`] for a cell
    /// that isn't rigid or is outside the grid.
    #[must_use]
    pub fn cell_mask(&self, x: usize, y: usize) -> u16 {
        if x >= self.width || y >= self.height {
            return FULL_MASK;
        }
        self.rigid_mask(y * self.width + x).unwrap_or(FULL_MASK)
    }

    /// Sets cell `i`'s mask (full or empty stores nothing: whole).
    pub(super) fn set_mask(&mut self, i: usize, mask: u16) {
        if mask == FULL_MASK || mask == 0 {
            self.masks.remove(&i);
        } else {
            self.masks.insert(i, mask);
        }
    }

    /// Adds quarters to cell `i`'s mask (a partial mask grows; a whole
    /// cell stays whole).
    pub(super) fn add_mask(&mut self, i: usize, mask: u16) {
        if let Some(m) = self.masks.get(&i).copied()
            && m | mask != m
        {
            self.set_mask(i, m | mask);
            self.mark_changed(i % self.width, i / self.width);
            self.outline_edited(i);
        }
    }

    /// Exchanges the masks of cells `i` and `j` (as their cells swap).
    pub(super) fn swap_masks(&mut self, i: usize, j: usize) {
        let a = self.masks.remove(&i);
        let b = self.masks.remove(&j);
        if let Some(a) = a {
            self.masks.insert(j, a);
        }
        if let Some(b) = b {
            self.masks.insert(i, b);
        }
    }

    /// Cuts an erasing disc's outline into the rigid cells around it that
    /// it didn't take, so a round scoop leaves a round hollow: the quarters
    /// within r + ½ cells of its centre (where the erased cells' edge
    /// lies). A cell the cut would empty keeps the outline it had.
    pub fn carve(&mut self, cx: i64, cy: i64, radius: u32) {
        let r = i64::from(radius).min(2 * to_i64(super::MAX_SIDE));
        for y in cy - r - 1..=cy + r + 1 {
            for x in cx - r - 1..=cx + r + 1 {
                let (dx, dy) = (x - cx, y - cy);
                if dx * dx + dy * dy <= r * r + r {
                    continue;
                }
                let (Ok(ux), Ok(uy)) = (usize::try_from(x), usize::try_from(y)) else {
                    continue;
                };
                if ux >= self.width || uy >= self.height {
                    continue;
                }
                let i = uy * self.width + ux;
                let cut = within(dx, dy, 64 * (r * r + r) + 16);
                if cut == 0
                    || !self.is_rigid(i)
                    || self.ornament_art(self.cells[i].material).is_some()
                {
                    continue;
                }
                let m = self.masks.get(&i).copied().unwrap_or(FULL_MASK) & !cut;
                // A cell the cut would empty keeps its outline: it is still
                // rigid, and an invisible rigid cell would be worse.
                if m != 0 && m != self.masks.get(&i).copied().unwrap_or(FULL_MASK) {
                    self.set_mask(i, m);
                    self.mark_changed(ux, uy);
                    self.outline_edited(i);
                }
            }
        }
    }

    /// The partial masks of rigid cells, by index, as saved (version 10).
    pub(super) fn save_masks(&self) -> Vec<(u32, u16)> {
        #[allow(clippy::cast_possible_truncation)] // cells < MAX_SIDE²
        self.masks
            .iter()
            .filter(|&(&i, _)| self.is_rigid(i))
            .map(|(&i, &m)| (i as u32, m))
            .collect()
    }

    /// Restores the partial masks from a save: each must name a rigid cell,
    /// once, in ascending order, with a partial, non-empty mask.
    pub(super) fn load_masks(&mut self, saved: &[(u32, u16)]) -> Result<(), SaveError> {
        self.masks.clear();
        let mut last = None;
        for &(i, m) in saved {
            let i = self.cell_index(i, "a mask")?;
            if last.is_some_and(|l| l >= i) || m == 0 || m == FULL_MASK || !self.is_rigid(i) {
                return Err(SaveError::Invalid(format!(
                    "the outline mask of cell {i} is out of order or not of a rigid cell"
                )));
            }
            last = Some(i);
            self.masks.insert(i, m);
        }
        Ok(())
    }

    /// Feeds the rigid cells' partial masks to the state hash.
    pub(super) fn hash_masks(&self, h: &mut Fnv64) {
        for (&i, &m) in &self.masks {
            if self.is_rigid(i) {
                h.write(&(i as u64).to_le_bytes());
                h.write(&m.to_le_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The disc's quarters as a 4r+4 square bitmap of rows.
    fn disc_rows(r: i64) -> Vec<Vec<bool>> {
        let side = usize::try_from(8 * r + 4).unwrap();
        let mut rows = vec![vec![false; side]; side];
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy > r * r + r {
                    continue;
                }
                let m = disc_mask(dx, dy, r);
                for qy in 0..4 {
                    for qx in 0..4 {
                        if m & quarter_bit(qx, qy) != 0 {
                            let x = usize::try_from(4 * (dx + r)).unwrap() + qx;
                            let y = usize::try_from(4 * (dy + r)).unwrap() + qy;
                            rows[y][x] = true;
                        }
                    }
                }
            }
        }
        rows
    }

    #[test]
    fn interior_cells_are_full_and_edges_partial() {
        assert_eq!(disc_mask(0, 0, 10), FULL_MASK);
        assert_eq!(disc_mask(3, -4, 10), FULL_MASK);
        let edge = disc_mask(0, -10, 10);
        assert!(edge != FULL_MASK && edge != 0, "{edge:#06x}");
        // The top cell keeps its lower rows.
        assert_eq!(edge & 0xF000, 0xF000);
        assert_eq!(edge & 0x000F, 0);
        // Radius 0: a rounded dot, corners off.
        let dot = disc_mask(0, 0, 0);
        assert_eq!(dot.count_ones(), 12);
        assert_eq!(dot & (quarter_bit(0, 0) | quarter_bit(3, 3)), 0);
    }

    #[test]
    fn every_brush_cell_has_coverage() {
        for r in 0..40 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dy * dy <= r * r + r {
                        assert_ne!(disc_mask(dx, dy, r), 0, "r {r} cell ({dx}, {dy})");
                    }
                }
            }
        }
    }

    #[test]
    fn disc_masks_are_symmetric_and_round() {
        for r in [1_i64, 3, 7, 10, 15] {
            let rows = disc_rows(r);
            let n = rows.len();
            for y in 0..n {
                for x in 0..n {
                    // Mirror left-right, top-bottom and across the diagonal.
                    assert_eq!(rows[y][x], rows[y][n - 1 - x], "r {r} ({x}, {y})");
                    assert_eq!(rows[y][x], rows[n - 1 - y][x], "r {r} ({x}, {y})");
                    assert_eq!(rows[y][x], rows[x][y], "r {r} ({x}, {y})");
                }
            }
            // Each texel row's ends follow the circle of radius
            // √(r² + r/4) cells within a texel on each side.
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
            for (y, row) in rows.iter().enumerate() {
                let w = row.iter().filter(|&&b| b).count();
                if w == 0 {
                    continue;
                }
                let first = row.iter().position(|&b| b).unwrap();
                assert!(
                    row[first..first + w].iter().all(|&b| b),
                    "r {r} row {y} has a gap"
                );
                let cy = (y as f64 + 0.5) - n as f64 / 2.0;
                let rr = 16.0 * (r * r) as f64 + 4.0 * r as f64;
                let half = (rr - cy * cy).max(0.0).sqrt();
                // Texel centres within `half` of the middle.
                let ideal_first = (n as f64 / 2.0 - half - 0.5).ceil() as i64;
                let got = i64::try_from(first).unwrap();
                assert!(
                    (got - ideal_first).abs() <= 1,
                    "r {r} row {y}: starts at {got}, circle at {ideal_first}"
                );
            }
        }
    }
}
