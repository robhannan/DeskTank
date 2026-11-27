//! The centimetre ruler overlay: the layout maths `fs_ruler` in `tank.wgsl`
//! shares (art-pixel size, tick and label spacing, the edge strips it draws
//! in).
//!
//! The ruler runs along the tank's bottom and left edges with 0 cm at the
//! bottom-left corner and 1 cm = [`CELLS_PER_CM`] cells, the scale creatures
//! are drawn at. Everything is laid out on a grid of "art pixels" of
//! [`Plan::px`] target pixels a side, anchored at that corner, so ticks and
//! 3×5 digits stay crisp at every cell scale.

// Every cast here is of a pixel or centimetre count far under f32's 24 bits.
#![allow(clippy::cast_precision_loss)]

use desktank_sim::species::CELLS_PER_CM;

/// Height of the bottom strip, in art pixels (`RULER_BOTTOM` in `tank.wgsl`).
pub(crate) const BOTTOM_ART: u32 = 12;
/// Width of the left strip, in art pixels (`RULER_LEFT` in `tank.wgsl`).
pub(crate) const LEFT_ART: u32 = 19;

/// Tick spacings tried for the minor ticks, in cm.
const MINOR_STEPS: [u32; 5] = [1, 5, 10, 50, 100];
/// Label spacings tried, in cm.
const LABEL_STEPS: [u32; 8] = [5, 10, 20, 50, 100, 200, 500, 1000];

/// How the ruler is laid out in one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Plan {
    /// Target pixels per art pixel.
    pub px: u32,
    /// Target pixels per centimetre.
    pub cm_px: f32,
    /// Spacing of numeric labels, in cm.
    pub label_step: u32,
    /// Spacing of the smallest ticks, in cm.
    pub minor_step: u32,
}

impl Plan {
    /// The layout for a grid of `grid` cells drawn at `scale` target pixels
    /// per cell. The art pixel is a quarter of a cell (so the ruler grows
    /// with the tank), and at least `min_px`: the host's readability floor
    /// on a high-density display (`Tank::set_card_min_scale`).
    pub fn new(scale: u32, min_px: u32, grid: (u32, u32)) -> Self {
        let px = (scale / 4).max(min_px).max(1);
        let cm_px = scale as f32 * CELLS_PER_CM;
        let art_per_cm = cm_px / px as f32;
        let minor_step = MINOR_STEPS
            .into_iter()
            .find(|&s| s as f32 * art_per_cm >= 2.0)
            .unwrap_or(100);
        // A label is at most as wide as the tank's length in cm, and needs a
        // clear art pixel on each side; stacked labels need 7 rows.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // whole cm
        let longest_cm = (grid.0.max(grid.1) as f32 / CELLS_PER_CM) as u32;
        let digits = longest_cm.max(1).ilog10() + 1;
        let need = (4 * digits + 1).max(7) as f32;
        let label_step = LABEL_STEPS
            .into_iter()
            .find(|&s| s % minor_step == 0 && s as f32 * art_per_cm >= need)
            .unwrap_or(1000);
        Self {
            px,
            cm_px,
            label_step,
            minor_step,
        }
    }

    /// The `ruler` uniform: art pixel, pixels per cm, label step, minor step.
    pub fn uniform(&self) -> [f32; 4] {
        [
            self.px as f32,
            self.cm_px,
            self.label_step as f32,
            self.minor_step as f32,
        ]
    }

    /// The target rectangles `(x, y, width, height)` the ruler shades for a
    /// grid at `offset` (top-left, target pixels) of `grid` cells at `scale`,
    /// clamped to a `target`-pixel surface: the bottom strip across the
    /// tank, then the left strip above it (disjoint, so nothing is blended
    /// twice). Empty rectangles are left out.
    pub fn strips(
        &self,
        offset: (i64, i64),
        grid: (u32, u32),
        scale: u32,
        target: (u32, u32),
    ) -> Vec<(u32, u32, u32, u32)> {
        let px = i64::from(self.px);
        let right = offset.0 + i64::from(grid.0) * i64::from(scale);
        let bottom = offset.1 + i64::from(grid.1) * i64::from(scale);
        let strip_top = (bottom - i64::from(BOTTOM_ART) * px).max(offset.1);
        let left = offset.0 + i64::from(LEFT_ART) * px;
        let clamp = |x0: i64, y0: i64, x1: i64, y1: i64| {
            let (w, h) = (i64::from(target.0), i64::from(target.1));
            let (x0, x1) = (x0.clamp(0, w), x1.clamp(0, w));
            let (y0, y1) = (y0.clamp(0, h), y1.clamp(0, h));
            (x1 > x0 && y1 > y0).then(|| {
                let [x, y, w, h] =
                    [x0, y0, x1 - x0, y1 - y0].map(|v| u32::try_from(v).unwrap_or(0));
                (x, y, w, h)
            })
        };
        [
            clamp(offset.0, strip_top, right, bottom),
            clamp(offset.0, offset.1, left.min(right), strip_top),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font;

    #[test]
    fn spacing_never_crowds_ticks_or_labels() {
        for scale in [1, 2, 4, 6, 8, 12] {
            for min_px in [0, 1, 2, 3, 4] {
                let p = Plan::new(scale, min_px, (240, 120));
                let art_per_cm = p.cm_px / p.px as f32;
                assert!(p.px >= min_px.max(1), "scale {scale} min {min_px}");
                assert!(p.minor_step as f32 * art_per_cm >= 2.0, "{p:?}");
                assert_eq!(p.label_step % p.minor_step, 0, "{p:?}");
                // Three digits plus a gap on each side, or seven rows.
                assert!(p.label_step as f32 * art_per_cm >= 7.0, "{p:?}");
            }
        }
    }

    #[test]
    fn one_cm_ticks_sit_at_the_creature_scale() {
        let p = Plan::new(8, 0, (240, 120));
        assert_eq!(p.px, 2);
        assert!((p.cm_px - 8.0 * CELLS_PER_CM).abs() < f32::EPSILON);
        assert_eq!((p.minor_step, p.label_step), (1, 5));
    }

    #[test]
    fn strips_are_disjoint_clamped_and_skip_empty() {
        let p = Plan::new(4, 0, (16, 8));
        // 16×8 cells at 4 px in 80×32: letterbox 8 px left and right.
        let strips = p.strips((8, 0), (16, 8), 4, (80, 32));
        assert_eq!(
            strips,
            vec![(8, 20, 64, 12), (8, 0, 19, 20)],
            "bottom strip, then the left strip above it"
        );
        // A tank hanging off the target at scale 1 is clamped, never negative.
        let strips = p.strips((-4, -2), (16, 8), 1, (8, 4));
        assert_eq!(strips, vec![(0, 0, 8, 4)]);
        assert_eq!(p.strips((100, 100), (16, 8), 1, (8, 4)), []);
    }

    /// `tank.wgsl` carries its own copy of the strip sizes and the glyph
    /// bitmaps (`font.rs`'s 3×5 digits and the `c`, `m` of "cm", one bit per
    /// pixel, row-major from the top-left); keep them in step.
    #[test]
    fn the_shader_matches_the_font_and_strip_sizes() {
        let wgsl = include_str!("tank.wgsl");
        assert!(wgsl.contains(&format!("const RULER_BOTTOM: f32 = {BOTTOM_ART}.0;")));
        assert!(wgsl.contains(&format!("const RULER_LEFT: f32 = {LEFT_ART}.0;")));
        for (index, c) in "0123456789cm".chars().enumerate() {
            let glyph = font::glyph(c).unwrap();
            let mut mask = 0_u32;
            for (row, line) in glyph.iter().take(5).enumerate() {
                for (col, ink) in line.chars().enumerate() {
                    if ink == '#' {
                        mask |= 1 << (row * 3 + col);
                    }
                }
            }
            let case = format!("case {index}u: {{ return {mask:#06x}u; }}");
            assert!(wgsl.contains(&case), "tank.wgsl lacks `{case}` for {c:?}");
        }
    }
}
