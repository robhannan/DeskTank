//! Where the grid sits in a target: even integer scale, centred or bottom-aligned.
//!
//! Creature sprites have two texels per cell side, so an even scale gives
//! every texel the same whole number of pixels. Scale 1 is the fallback
//! when 2 doesn't fit: each pixel then shows one texel of its 2×2 block.

/// Placement of a `grid` of cells in a target of `target` pixels: the largest
/// even integer scale that fits, or 1 if 2 doesn't, with the rest
/// letterboxed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Screen pixels per cell side.
    pub scale: u32,
    /// Top-left of the grid in target pixels (negative if the grid overflows
    /// at scale 1).
    pub offset: (i64, i64),
    /// Grid size in cells.
    pub grid: (u32, u32),
}

impl Layout {
    /// Fits `grid` cells into `target` pixels.
    #[must_use]
    pub fn fit(grid: (u32, u32), target: (u32, u32)) -> Self {
        let sx = target.0 / grid.0.max(1);
        let sy = target.1 / grid.1.max(1);
        let scale = even_scale(sx.min(sy));
        let centre = |t: u32, g: u32| (i64::from(t) - i64::from(g) * i64::from(scale)) / 2;
        Self {
            scale,
            offset: (centre(target.0, grid.0), centre(target.1, grid.1)),
            grid,
        }
    }

    /// Fits the grid horizontally centred with its bottom edge flush to the target.
    #[must_use]
    pub fn fit_bottom(grid: (u32, u32), target: (u32, u32)) -> Self {
        let mut layout = Self::fit(grid, target);
        layout.offset.1 = i64::from(target.1) - i64::from(grid.1) * i64::from(layout.scale);
        layout
    }

    /// The cell under target pixel `(px, py)`. Signed and unclipped: a
    /// pixel in the letterbox maps to a cell outside the grid, so brushes
    /// can be centred just off the edge.
    #[must_use]
    pub fn cell_at(&self, px: f64, py: f64) -> (i64, i64) {
        let s = f64::from(self.scale);
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // pixel coords
        let cell = |p: f64, o: i64| ((p - o as f64) / s).floor() as i64;
        (cell(px, self.offset.0), cell(py, self.offset.1))
    }
}

/// `scale` rounded down to an even number, or 1 below 2.
#[must_use]
pub fn even_scale(scale: u32) -> u32 {
    if scale >= 2 { scale & !1 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_fit_has_no_letterbox() {
        let l = Layout::fit((640, 360), (2560, 1440));
        assert_eq!(l.scale, 4);
        assert_eq!(l.offset, (0, 0));
    }

    #[test]
    fn picks_the_largest_even_scale_and_centres() {
        // Scale 3 would fit; sprites want an even one.
        let l = Layout::fit((640, 360), (2000, 1500));
        assert_eq!(l.scale, 2);
        assert_eq!(l.offset, ((2000 - 1280) / 2, (1500 - 720) / 2));
        assert_eq!(Layout::fit((640, 360), (3000, 2000)).scale, 4);
        // Under 2, the fallback is 1.
        assert_eq!(Layout::fit((640, 360), (1000, 600)).scale, 1);
        assert_eq!(
            (0..10).map(even_scale).collect::<Vec<_>>(),
            [1, 1, 2, 2, 4, 4, 6, 6, 8, 8]
        );
    }

    #[test]
    fn never_scales_below_one() {
        let l = Layout::fit((640, 360), (320, 180));
        assert_eq!(l.scale, 1);
        assert_eq!(l.offset, (-160, -90));
    }

    #[test]
    fn bottom_alignment_keeps_the_floor_visible() {
        let l = Layout::fit_bottom((640, 360), (2000, 1500));
        assert_eq!(l.scale, 2);
        assert_eq!(l.offset, (360, 780));
        assert_eq!(l.cell_at(360.0, 1499.0), (0, 359));

        let cropped = Layout::fit_bottom((640, 360), (320, 180));
        assert_eq!(cropped.scale, 1);
        assert_eq!(cropped.offset, (-160, -180));
        assert_eq!(cropped.cell_at(0.0, 179.0), (160, 359));
    }

    #[test]
    fn maps_pixels_to_cells() {
        let l = Layout::fit((640, 360), (2000, 1500));
        assert_eq!(l.cell_at(360.0, 390.0), (0, 0));
        assert_eq!(l.cell_at(361.9, 391.9), (0, 0));
        assert_eq!(l.cell_at(362.0, 392.0), (1, 1));
        assert_eq!(l.cell_at(359.0, 389.0), (-1, -1));
    }
}
