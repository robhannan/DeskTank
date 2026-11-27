//! The camera (D29): a view rectangle in grid cells that the target shows.
//!
//! With no camera the grid is drawn at its integer [`Layout`]. With one, the
//! tank is drawn into an offscreen image at that same layout and a post
//! pass resamples `view` of it over the whole target, so the view can pan
//! and zoom by fractions of a cell. This module is the geometry: the view
//! rectangle, the full-tank "fit" view, "cover" views of a point at a zoom,
//! and the mapping between grid and screen. The GPU side is in `post.rs`.

use crate::layout::Layout;

/// Largest zoom [`ViewRect::cover`] gives (the tour uses about 3×).
pub const MAX_ZOOM: f32 = 8.0;

/// The part of the grid a camera shows: its top-left and size in cells,
/// fractional so zoom and pan are smooth. The view is stretched over the
/// whole target, so its aspect should be the target's (as [`ViewRect::fit`]
/// and [`ViewRect::cover`] give); any other aspect stretches the picture and
/// its texels stop being square. Parts of the view outside the grid show the
/// letterbox colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewRect {
    /// Left edge, in cells (negative if the view starts left of the grid).
    pub x: f32,
    /// Top edge, in cells.
    pub y: f32,
    /// Width in cells.
    pub w: f32,
    /// Height in cells.
    pub h: f32,
}

impl ViewRect {
    /// Whether this is a usable view: finite with a positive size.
    #[must_use]
    pub fn is_valid(self) -> bool {
        [self.x, self.y, self.w, self.h]
            .iter()
            .all(|v| v.is_finite())
            && self.w > 0.0
            && self.h > 0.0
    }

    /// The full-tank view: the whole `layout`-placed grid in `target`, with
    /// its letterbox. Drawn through the camera it matches the no-camera
    /// picture pixel for pixel.
    #[must_use]
    pub fn fit(layout: &Layout, target: (u32, u32)) -> Self {
        #[allow(clippy::cast_precision_loss)] // pixels and offsets, far below 2^24
        let (s, ox, oy) = (
            layout.scale as f32,
            layout.offset.0 as f32,
            layout.offset.1 as f32,
        );
        #[allow(clippy::cast_precision_loss)] // pixels
        let (tw, th) = (target.0 as f32, target.1 as f32);
        Self {
            x: -ox / s,
            y: -oy / s,
            w: tw / s,
            h: th / s,
        }
    }

    /// The view of `centre` (a point in cells) at `zoom` times the
    /// [`ViewRect::fit`] view: `fit`'s size divided by `zoom`, which is
    /// clamped to `1..=`[`MAX_ZOOM`] (NaN is 1), so zoom 1 is the fit view.
    /// A zoomed view is clamped inside the grid, so the glass edges and the
    /// letterbox leave the picture as it closes in. Where the view is still
    /// wider (or taller) than the grid, because the target's aspect differs
    /// from the grid's and the zoom is small, it stays centred on the fit
    /// view's centre on that axis and the letterbox shows there; from
    /// [`ViewRect::min_cover_zoom`] up, no letterbox shows on either.
    #[must_use]
    pub fn cover(layout: &Layout, target: (u32, u32), centre: (f32, f32), zoom: f32) -> Self {
        let fit = Self::fit(layout, target);
        let zoom = if zoom.is_nan() {
            1.0
        } else {
            zoom.clamp(1.0, MAX_ZOOM)
        };
        #[allow(clippy::cast_precision_loss)] // grid cells
        let grid = (layout.grid.0 as f32, layout.grid.1 as f32);
        let axis = |fit_at: f32, fit_len: f32, grid_len: f32, at: f32| {
            let len = fit_len / zoom;
            let start = if len >= grid_len {
                fit_at + (fit_len - len) / 2.0
            } else {
                (at - len / 2.0).clamp(0.0, grid_len - len)
            };
            (start, len)
        };
        let (x, w) = axis(fit.x, fit.w, grid.0, centre.0);
        let (y, h) = axis(fit.y, fit.h, grid.1, centre.1);
        Self { x, y, w, h }
    }

    /// The smallest zoom at which a [`ViewRect::cover`] view lies inside the
    /// grid on both axes (1 when the fit view already does).
    #[must_use]
    pub fn min_cover_zoom(layout: &Layout, target: (u32, u32)) -> f32 {
        let fit = Self::fit(layout, target);
        #[allow(clippy::cast_precision_loss)] // grid cells
        let grid = (layout.grid.0.max(1) as f32, layout.grid.1.max(1) as f32);
        (fit.w / grid.0).max(fit.h / grid.1).max(1.0)
    }

    /// Target pixels per cell, across and down, when this view fills
    /// `target`.
    #[must_use]
    pub fn pixels_per_cell(self, target: (u32, u32)) -> (f64, f64) {
        (
            f64::from(target.0) / f64::from(self.w),
            f64::from(target.1) / f64::from(self.h),
        )
    }

    /// Where grid point `cell` (in cells, fractional) lands in `target`, in
    /// pixels (fractional, off the target if it is out of view).
    #[must_use]
    pub fn grid_to_screen(self, target: (u32, u32), cell: (f64, f64)) -> (f64, f64) {
        let (sx, sy) = self.pixels_per_cell(target);
        (
            (cell.0 - f64::from(self.x)) * sx,
            (cell.1 - f64::from(self.y)) * sy,
        )
    }

    /// The grid point under `target` pixel position `px` (the inverse of
    /// [`ViewRect::grid_to_screen`]); floor it for the cell.
    #[must_use]
    pub fn screen_to_grid(self, target: (u32, u32), px: (f64, f64)) -> (f64, f64) {
        let (sx, sy) = self.pixels_per_cell(target);
        (px.0 / sx + f64::from(self.x), px.1 / sy + f64::from(self.y))
    }

    /// A [`Layout`] of the grid that puts cell `cell`'s centre where this
    /// view does, at the whole-pixel scale nearest the view's (even, or 1),
    /// for the magnifier, which draws at integer scales and only needs to
    /// know where the pointer's cell is.
    #[must_use]
    pub fn magnifier_grid(self, target: (u32, u32), layout: &Layout, cell: (i64, i64)) -> Layout {
        #[allow(clippy::cast_precision_loss)] // cells
        let centre = (cell.0 as f64 + 0.5, cell.1 as f64 + 0.5);
        let (px, py) = self.grid_to_screen(target, centre);
        let (per_cell, _) = self.pixels_per_cell(target);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped
        let scale = crate::layout::even_scale(per_cell.round().clamp(1.0, 4096.0) as u32);
        let half = i64::from(scale) / 2;
        #[allow(clippy::cast_possible_truncation)] // pixels
        let at = |p: f64, c: i64| p.round() as i64 - c * i64::from(scale) - half;
        Layout {
            scale,
            offset: (at(px, cell.0), at(py, cell.1)),
            grid: layout.grid,
        }
    }

    /// Where the cell rectangle `rect` (`x0, y0, x1, y1`) lands in
    /// `target`, as `[left, top, right, bottom]` pixels.
    #[must_use]
    pub fn rect_to_screen(self, target: (u32, u32), rect: [f32; 4]) -> [f64; 4] {
        let (x0, y0) = self.grid_to_screen(target, (f64::from(rect[0]), f64::from(rect[1])));
        let (x1, y1) = self.grid_to_screen(target, (f64::from(rect[2]), f64::from(rect[3])));
        [x0, y0, x1, y1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRID: (u32, u32) = (320, 180);

    fn assert_close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
    }

    #[test]
    fn the_fit_view_is_the_layout_seen_from_the_target() {
        // 1280×720 holds 320×180 at scale 4 with no letterbox.
        let l = Layout::fit(GRID, (1280, 720));
        let v = ViewRect::fit(&l, (1280, 720));
        assert_eq!(
            v,
            ViewRect {
                x: 0.0,
                y: 0.0,
                w: 320.0,
                h: 180.0
            }
        );
        // 2000×1500 at scale 6 is letterboxed: offset (40, 210).
        let l = Layout::fit(GRID, (2000, 1500));
        assert_eq!((l.scale, l.offset), (6, (40, 210)));
        let v = ViewRect::fit(&l, (2000, 1500));
        assert_close(f64::from(v.x), -40.0 / 6.0);
        assert_close(f64::from(v.y), -35.0);
        assert_close(f64::from(v.w), 2000.0 / 6.0);
        assert_close(f64::from(v.h), 250.0);
        // The fit view maps cells to the pixels the layout puts them at.
        let (px, py) = v.grid_to_screen((2000, 1500), (0.0, 0.0));
        assert_close(px, 40.0);
        assert_close(py, 210.0);
        let (px, py) = v.grid_to_screen((2000, 1500), (10.0, 10.0));
        assert_close(px, 100.0);
        assert_close(py, 270.0);
    }

    #[test]
    fn grid_and_screen_map_both_ways() {
        let v = ViewRect {
            x: 100.0,
            y: 50.0,
            w: 128.0,
            h: 72.0,
        };
        let target = (1280, 720); // 10 px per cell
        assert_eq!(v.pixels_per_cell(target), (10.0, 10.0));
        let (px, py) = v.grid_to_screen(target, (110.5, 60.25));
        assert_close(px, 105.0);
        assert_close(py, 102.5);
        let (gx, gy) = v.screen_to_grid(target, (105.0, 102.5));
        assert_close(gx, 110.5);
        assert_close(gy, 60.25);
        // Off-screen points map off the target; the round trip still holds.
        let (px, py) = v.grid_to_screen(target, (0.0, 0.0));
        assert_close(px, -1000.0);
        assert_close(py, -500.0);
        let (gx, gy) = v.screen_to_grid(target, (px, py));
        assert_close(gx, 0.0);
        assert_close(gy, 0.0);
        // A cell rectangle maps corner to corner.
        let r = v.rect_to_screen(target, [100.0, 50.0, 101.0, 52.0]);
        assert_eq!(r, [0.0, 0.0, 10.0, 20.0]);
    }

    #[test]
    fn zoom_one_is_the_fit_view_and_zoom_shrinks_it_about_the_point() {
        let target = (1280, 720);
        let l = Layout::fit(GRID, target);
        let fit = ViewRect::fit(&l, target);
        assert_eq!(ViewRect::cover(&l, target, (30.0, 20.0), 1.0), fit);
        assert_eq!(ViewRect::cover(&l, target, (30.0, 20.0), 0.2), fit);
        assert_eq!(ViewRect::cover(&l, target, (30.0, 20.0), f32::NAN), fit);

        let v = ViewRect::cover(&l, target, (160.0, 90.0), 2.5);
        assert_close(f64::from(v.w), 128.0);
        assert_close(f64::from(v.h), 72.0);
        // Centred on the point, and the same aspect as the target.
        assert_close(f64::from(v.x), 160.0 - 64.0);
        assert_close(f64::from(v.y), 90.0 - 36.0);
        assert_close(f64::from(v.w / v.h), 1280.0 / 720.0);
        // The centre lands in the middle of the screen.
        let (px, py) = v.grid_to_screen(target, (160.0, 90.0));
        assert_close(px, 640.0);
        assert_close(py, 360.0);
    }

    #[test]
    fn a_zoomed_view_is_clamped_inside_the_tank() {
        let target = (1280, 720);
        let l = Layout::fit(GRID, target);
        let corner = ViewRect::cover(&l, target, (0.0, 0.0), 2.0);
        assert_eq!((corner.x, corner.y), (0.0, 0.0));
        let corner = ViewRect::cover(&l, target, (1000.0, 1000.0), 2.0);
        assert_close(f64::from(corner.x + corner.w), 320.0);
        assert_close(f64::from(corner.y + corner.h), 180.0);
        // Zoom is capped.
        let max = ViewRect::cover(&l, target, (160.0, 90.0), 100.0);
        assert_close(f64::from(max.w), 320.0 / f64::from(MAX_ZOOM));
    }

    #[test]
    fn a_wider_target_letterboxes_until_the_view_closes_in() {
        // 21:9-ish target for a 16:9 grid: scale by height, bars at the sides.
        let target = (1680, 720);
        let l = Layout::fit(GRID, target);
        assert_eq!(l.scale, 4);
        let fit = ViewRect::fit(&l, target);
        assert_close(f64::from(fit.w), 420.0);
        let zoom = ViewRect::min_cover_zoom(&l, target);
        assert_close(f64::from(zoom), 420.0 / 320.0);
        // Below it the view is wider than the grid and stays centred on x.
        let v = ViewRect::cover(&l, target, (50.0, 90.0), 1.1);
        assert!(v.w > 320.0);
        assert_close(f64::from(v.x + v.w / 2.0), 160.0);
        // From it up, the view is inside the grid on both axes.
        for z in [zoom, 2.0, 3.0] {
            let v = ViewRect::cover(&l, target, (10.0, 10.0), z);
            assert!(v.x >= -1e-3 && v.y >= -1e-3, "{z}: {v:?}");
            assert!(v.x + v.w <= 320.001 && v.y + v.h <= 180.001, "{z}: {v:?}");
        }
        // A target the grid fills needs no extra zoom.
        let l = Layout::fit(GRID, (1280, 720));
        assert_eq!(ViewRect::min_cover_zoom(&l, (1280, 720)), 1.0);
    }

    #[test]
    fn invalid_views_are_rejected() {
        let ok = ViewRect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 5.0,
        };
        assert!(ok.is_valid());
        for bad in [
            ViewRect { w: 0.0, ..ok },
            ViewRect { h: -1.0, ..ok },
            ViewRect { x: f32::NAN, ..ok },
            ViewRect {
                w: f32::INFINITY,
                ..ok
            },
        ] {
            assert!(!bad.is_valid(), "{bad:?}");
        }
    }
}
