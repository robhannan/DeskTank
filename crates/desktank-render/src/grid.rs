//! The cell grid on the GPU: an `Rgba8Unorm` texture of (material,
//! variant, state low byte, state high byte), updated from the world's
//! dirty rectangles. The state bytes give the shader a plant cell's plant
//! id (T6b, SPEC §2.7), which it looks up in the per-plant tint texture.

use desktank_sim::{CHUNK_SIZE, Cell, Rect};

/// Dirty rectangles collected across simulation ticks until the next frame
/// uploads them, merged per chunk so a busy chunk is one upload.
#[derive(Clone, Debug)]
pub struct DirtyRects {
    chunks_x: usize,
    rects: Vec<Rect>,
    /// Upload the whole grid instead (first frame, new texture).
    full: bool,
}

impl DirtyRects {
    /// Tracking for a `width`×`height` grid, starting with a full upload.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        let chunks_x = width.div_ceil(CHUNK_SIZE);
        Self {
            chunks_x,
            rects: vec![Rect::EMPTY; chunks_x * height.div_ceil(CHUNK_SIZE)],
            full: true,
        }
    }

    /// Adds a rectangle that lies within one chunk (as
    /// `World::dirty_rects` yields them).
    pub fn add(&mut self, r: Rect) {
        let i = (r.y0 / CHUNK_SIZE) * self.chunks_x + r.x0 / CHUNK_SIZE;
        if let Some(slot) = self.rects.get_mut(i) {
            *slot = slot.union(r);
        }
    }

    /// Requests a full upload.
    pub fn mark_all(&mut self) {
        self.full = true;
    }

    /// Whether anything is waiting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.full && self.rects.iter().all(Rect::is_empty)
    }

    /// Hands out what to upload (`None` = the whole grid) and clears.
    pub fn take(&mut self, mut f: impl FnMut(Option<Rect>)) {
        if self.full {
            f(None);
        } else {
            for r in self.rects.iter().filter(|r| !r.is_empty()) {
                f(Some(*r));
            }
        }
        self.full = false;
        self.rects.fill(Rect::EMPTY);
    }
}

/// Bytes per cell texel.
pub const TEXEL_BYTES: usize = 4;

/// Packs the cells of `r` (row-major, `width` cells per grid row) as
/// (material, variant, state low, state high) texels into `out`, replacing
/// its contents. A rigid cell (`mask` gives its sub-cell outline, see
/// `World::rigid_mask`) sends its outline mask in place of its state (its
/// body id, which the shader never reads), so outlines cost no texture of
/// their own.
pub fn pack(
    cells: &[Cell],
    width: usize,
    r: Rect,
    mask: impl Fn(usize) -> Option<u16>,
    out: &mut Vec<u8>,
) {
    out.clear();
    out.reserve(r.area() * TEXEL_BYTES);
    for y in r.y0..r.y1 {
        let row = y * width;
        for (x, c) in (r.x0..r.x1).zip(&cells[row + r.x0..row + r.x1]) {
            let state = mask(row + x).unwrap_or(c.state);
            let [lo, hi] = state.to_le_bytes();
            out.extend_from_slice(&[c.material.0, c.variant, lo, hi]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktank_sim::MaterialId;

    fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
        Rect { x0, y0, x1, y1 }
    }

    fn collect(d: &mut DirtyRects) -> Vec<Option<Rect>> {
        let mut v = Vec::new();
        d.take(|r| v.push(r));
        v
    }

    #[test]
    fn starts_with_a_full_upload_then_tracks_rects() {
        let mut d = DirtyRects::new(200, 100);
        assert!(!d.is_empty());
        assert_eq!(collect(&mut d), vec![None]);
        assert!(d.is_empty());
        d.add(rect(1, 1, 2, 2));
        d.add(rect(5, 9, 6, 10));
        d.add(rect(70, 0, 71, 1));
        assert_eq!(
            collect(&mut d),
            vec![Some(rect(1, 1, 6, 10)), Some(rect(70, 0, 71, 1))]
        );
        assert!(d.is_empty());
        assert_eq!(collect(&mut d), Vec::new());
    }

    #[test]
    fn packs_material_variant_and_state() {
        let w = 3;
        let cells: Vec<Cell> = (0..9u8)
            .map(|i| Cell {
                state: u16::from(i) * 0x101,
                ..Cell::new(MaterialId(i), 100 + i)
            })
            .collect();
        let mut out = Vec::new();
        pack(&cells, w, rect(1, 1, 3, 3), |_| None, &mut out);
        assert_eq!(
            out,
            [4, 104, 4, 4, 5, 105, 5, 5, 7, 107, 7, 7, 8, 108, 8, 8]
        );
        // Rigid cells send their outline mask instead of their state.
        pack(
            &cells,
            w,
            rect(1, 1, 3, 2),
            |i| (i == 5).then_some(0x0ff0),
            &mut out,
        );
        assert_eq!(out, [4, 104, 4, 4, 5, 105, 0xf0, 0x0f]);
    }
}
