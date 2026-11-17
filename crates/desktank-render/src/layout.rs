//! Where the grid sits in a target: integer scale, centred.

/// Placement of a `grid` of cells in a target of `target` pixels: the largest
/// integer scale that fits (at least 1), centred, with the rest letterboxed.
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
        let scale = sx.min(sy).max(1);
        let centre = |t: u32, g: u32| (i64::from(t) - i64::from(g) * i64::from(scale)) / 2;
        Self {
            scale,
            offset: (centre(target.0, grid.0), centre(target.1, grid.1)),
            grid,
        }
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
    fn picks_the_largest_integer_scale_and_centres() {
        let l = Layout::fit((640, 360), (2000, 1500));
        assert_eq!(l.scale, 3);
        assert_eq!(l.offset, ((2000 - 1920) / 2, (1500 - 1080) / 2));
    }

    #[test]
    fn never_scales_below_one() {
        let l = Layout::fit((640, 360), (320, 180));
        assert_eq!(l.scale, 1);
        assert_eq!(l.offset, (-160, -90));
    }

    #[test]
    fn maps_pixels_to_cells() {
        let l = Layout::fit((640, 360), (2000, 1500));
        assert_eq!(l.cell_at(40.0, 210.0), (0, 0));
        assert_eq!(l.cell_at(42.9, 212.9), (0, 0));
        assert_eq!(l.cell_at(43.0, 213.0), (1, 1));
        assert_eq!(l.cell_at(39.0, 209.0), (-1, -1));
    }
}
