//! Fields (SPEC §2.1): coarse grids at 1/8 of the cell resolution holding
//! values that spread, such as nutrients and CO₂, plus the light and water
//! current the tank derives from its cells and devices.
//!
//! Fields step on their own schedule, every [`FIELD_EVERY`] ticks, not with
//! the chunks: a field update never wakes a chunk. Diffusion is the explicit
//! five-point scheme with closed (zero-flux) walls, so a field's total is
//! conserved apart from its configured decay, and a blob's variance grows by
//! exactly `2·D·t` per axis away from the walls.
//!
//! Units: a field value is a concentration per field cell (arbitrary units;
//! 1.0 is "plenty"), diffusion constants are in cells² per second and rates
//! per world second (SPEC §2.1).

use serde::Deserialize;

use crate::TICKS_PER_SECOND;

/// Cells per field cell along each side.
pub const FIELD_SCALE: usize = 8;
/// Ticks between field updates (0.2 s of world time).
pub const FIELD_EVERY: u64 = 4;
/// Largest diffusion constant, in cells² per second: the explicit scheme is
/// stable while `D·dt/h² ≤ 1/4`, with `dt` = 0.2 s and `h` = 8 cells.
pub const MAX_DIFFUSION: f32 = 80.0;
/// A field whose largest change in a step is below this stops stepping
/// until something adds to it.
const REST_EPSILON: f32 = 1e-6;

/// World seconds between field updates.
#[allow(clippy::cast_precision_loss)] // small constants
pub const FIELD_DT: f32 = FIELD_EVERY as f32 / TICKS_PER_SECOND as f32;

/// The fields a rule can read or write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    /// Dissolved nutrients: from fertilisers, detritus and aquasoil.
    Nutrient,
    /// Dissolved CO₂: from CO₂ bubbles.
    Co2,
    /// Light reaching each field cell from the surface (0 to the surface
    /// level), derived from the cells above.
    Light,
}

impl FieldKind {
    /// Every field, in a fixed order.
    pub const ALL: [Self; 3] = [Self::Nutrient, Self::Co2, Self::Light];

    /// The name used in TOML and on the command line.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Nutrient => "nutrient",
            Self::Co2 => "co2",
            Self::Light => "light",
        }
    }

    /// The field with this name, if any.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// A coarse grid of `f32` values, one per 8×8 block of cells.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    width: usize,
    height: usize,
    values: Vec<f32>,
    scratch: Vec<f32>,
    /// Whether the next step can change anything.
    active: bool,
}

impl Field {
    /// A zero field covering a `cells_w`×`cells_h` cell grid.
    #[must_use]
    pub fn new(cells_w: usize, cells_h: usize) -> Self {
        let (width, height) = (
            cells_w.div_ceil(FIELD_SCALE).max(1),
            cells_h.div_ceil(FIELD_SCALE).max(1),
        );
        Self {
            width,
            height,
            values: vec![0.0; width * height],
            scratch: Vec::new(),
            active: false,
        }
    }

    /// Width in field cells.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in field cells.
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// All values, row-major from the top-left.
    #[must_use]
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// The value at field cell `(fx, fy)` (0 outside).
    #[must_use]
    pub fn get(&self, fx: usize, fy: usize) -> f32 {
        if fx < self.width && fy < self.height {
            self.values[fy * self.width + fx]
        } else {
            0.0
        }
    }

    /// The value over grid cell `(x, y)`.
    #[must_use]
    pub fn at_cell(&self, x: usize, y: usize) -> f32 {
        self.get(x / FIELD_SCALE, y / FIELD_SCALE)
    }

    /// Adds `amount` (which may be negative) over grid cell `(x, y)`,
    /// keeping the value at or above zero.
    pub fn add_at_cell(&mut self, x: usize, y: usize, amount: f32) {
        let (fx, fy) = (x / FIELD_SCALE, y / FIELD_SCALE);
        if fx < self.width && fy < self.height && amount != 0.0 {
            let v = &mut self.values[fy * self.width + fx];
            *v = (*v + amount).max(0.0);
            self.active = true;
        }
    }

    /// The sum over every field cell.
    #[must_use]
    pub fn total(&self) -> f64 {
        self.values.iter().map(|&v| f64::from(v)).sum()
    }

    /// Makes the next step run even if nothing was added (the current
    /// changed, so a resting field may start moving).
    pub(crate) fn wake(&mut self) {
        self.active = true;
    }

    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn set(&mut self, i: usize, v: f32) {
        self.values[i] = v;
    }

    /// One update: diffusion with constant `diffusion` (cells²/s), sideways
    /// advection by `current` (cells/s per field cell, positive rightwards;
    /// empty for none), then exponential `decay` (per second), over
    /// [`FIELD_DT`]. Does nothing while the field is at rest. Returns
    /// whether it stepped.
    #[allow(clippy::many_single_char_names)] // stencil maths
    pub(crate) fn step(&mut self, diffusion: f32, decay: f32, current: &[f32]) -> bool {
        if !self.active {
            return false;
        }
        #[allow(clippy::cast_precision_loss)] // FIELD_SCALE is small
        let h2 = (FIELD_SCALE * FIELD_SCALE) as f32;
        let alpha = diffusion * FIELD_DT / h2;
        let keep = (1.0 - decay * FIELD_DT).max(0.0);
        let (w, h) = (self.width, self.height);
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.values);
        let u = &self.scratch;
        let mut max_delta = 0.0_f32;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let c = u[i];
                // Closed walls: a missing neighbour exchanges nothing.
                let mut flow = 0.0;
                if x > 0 {
                    flow += u[i - 1] - c;
                }
                if x + 1 < w {
                    flow += u[i + 1] - c;
                }
                if y > 0 {
                    flow += u[i - w] - c;
                }
                if y + 1 < h {
                    flow += u[i + w] - c;
                }
                let mut v = c + alpha * flow;
                if !current.is_empty() {
                    v += advection(u, current, i, x, w);
                }
                let v = (v * keep).max(0.0);
                max_delta = max_delta.max((v - c).abs());
                self.values[i] = v;
            }
        }
        self.active = max_delta >= REST_EPSILON;
        true
    }
}

/// Net upwind inflow into field cell `i` (column `x`) from sideways current,
/// over one step. Conservative: what leaves a cell enters its neighbour.
#[allow(clippy::many_single_char_names)] // stencil maths
fn advection(u: &[f32], current: &[f32], i: usize, x: usize, w: usize) -> f32 {
    #[allow(clippy::cast_precision_loss)] // FIELD_SCALE is small
    let k = FIELD_DT / FIELD_SCALE as f32;
    // Flux across the left face (positive = rightwards), then the right face.
    let face = |left: usize, right: usize| {
        let v = f32::midpoint(current[left], current[right]) * k;
        if v > 0.0 { v * u[left] } else { v * u[right] }
    };
    let mut d = 0.0;
    if x > 0 {
        d += face(i - 1, i);
    }
    if x + 1 < w {
        d -= face(i, i + 1);
    }
    d
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss)] // small test grids
mod tests {
    use super::*;

    /// Variance per axis, in cells², of a field's mass about its centroid.
    fn variance(f: &Field) -> (f64, f64) {
        let total = f.total();
        let (mut mx, mut my) = (0.0, 0.0);
        for y in 0..f.height() {
            for x in 0..f.width() {
                let v = f64::from(f.get(x, y)) / total;
                mx += v * x as f64;
                my += v * y as f64;
            }
        }
        let (mut vx, mut vy) = (0.0, 0.0);
        for y in 0..f.height() {
            for x in 0..f.width() {
                let v = f64::from(f.get(x, y)) / total;
                vx += v * (x as f64 - mx).powi(2);
                vy += v * (y as f64 - my).powi(2);
            }
        }
        let s = (FIELD_SCALE * FIELD_SCALE) as f64;
        (vx * s, vy * s)
    }

    #[test]
    fn diffusion_spreads_at_the_configured_rate_and_conserves() {
        let mut f = Field::new(512, 512);
        f.add_at_cell(256, 256, 10.0);
        let d = 16.0;
        let steps = 100;
        for _ in 0..steps {
            f.step(d, 0.0, &[]);
        }
        let t = f64::from(FIELD_DT) * f64::from(steps);
        let (vx, vy) = variance(&f);
        let want = 2.0 * f64::from(d) * t;
        assert!((vx / want - 1.0).abs() < 0.01, "vx {vx} want {want}");
        assert!((vy / want - 1.0).abs() < 0.01, "vy {vy} want {want}");
        assert!((f.total() - 10.0).abs() < 1e-3);
    }

    #[test]
    fn rests_once_flat_and_wakes_on_input() {
        let mut f = Field::new(16, 16);
        f.step(10.0, 0.0, &[]);
        assert!(!f.active);
        f.add_at_cell(3, 3, 1.0);
        assert!(f.active);
        for _ in 0..2000 {
            f.step(80.0, 0.0, &[]);
        }
        assert!(!f.active, "a 2x2 field should flatten");
        assert!((f.get(1, 1) - 0.25).abs() < 1e-3);
    }

    #[test]
    fn current_carries_and_decay_drains() {
        let mut f = Field::new(320, 8);
        f.add_at_cell(40, 0, 1.0);
        let current = vec![10.0; f.width() * f.height()];
        for _ in 0..50 {
            f.step(0.0, 0.0, &current);
        }
        // 10 cells/s for 10 s: the centroid moves 100 cells right, from the
        // centre of field cell 5 (44) to 144.
        let cx: f64 = (0..f.width())
            .map(|x| f64::from(f.get(x, 0)) * (x as f64 + 0.5) * FIELD_SCALE as f64)
            .sum::<f64>()
            / f.total();
        assert!((cx - 144.0).abs() < 2.0, "centroid {cx}");
        assert!((f.total() - 1.0).abs() < 1e-4);
        let mut g = Field::new(8, 8);
        g.add_at_cell(0, 0, 1.0);
        for _ in 0..5 {
            g.step(0.0, 0.5, &[]);
        }
        assert!((g.total() - 0.9_f64.powi(5)).abs() < 1e-4);
    }
}
