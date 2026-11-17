//! Browser demo of the DeskTank simulation core.
//!
//! A thin [`wasm-bindgen`](https://docs.rs/wasm-bindgen) wrapper around
//! [`desktank_sim::World`] for the GitHub Pages playground in `web/`: create
//! a world, tick it, pour and erase materials, and read an RGBA frame. It is
//! the WB0 demo; WB1 moves it onto `desktank-render` (SPEC §3.5).
//!
//! The bindings exist only on `wasm32`; natively the same API compiles as
//! plain Rust, so the workspace builds and the logic is tested on every OS.
//!
//! `unsafe_code`: the workspace denies it, and this crate needs no
//! exception. The `unsafe` that `#[wasm_bindgen]` generates comes from an
//! external proc macro, which rustc's `unsafe_code` lint doesn't report (CI
//! runs clippy for `wasm32` to catch it if that ever changes). Code written
//! here stays safe.

use desktank_sim::{Brush, MaterialId, MaterialTable, World, WorldConfig};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;

/// Largest side the demo accepts, in cells. The sim allows more, but a
/// browser tab doesn't need it.
pub const MAX_DEMO_SIDE: u32 = 1024;

/// Shades per material; a cell picks one from its variant byte.
const SHADES: usize = 4;

/// Pixel-art palettes for the built-in materials, lightest-first is not
/// implied; index 0 is the most common shade.
const PALETTES: &[(&str, [u32; SHADES])] = &[
    ("water", [0x2a_6496, 0x28_608f, 0x2d_6a9c, 0x26_5b8a]),
    ("air", [0xcf_e8f5, 0xcb_e5f3, 0xd4_ecf7, 0xc8_e2f1]),
    ("sand", [0xe0_c992, 0xd4_ba80, 0xc9_ad72, 0xec_d8a6]),
    ("gravel", [0x85_7b6e, 0x6f_665b, 0x9a_9184, 0x5e_574e]),
    ("rock", [0x5c_5853, 0x4c_4945, 0x68_645e, 0x55_524d]),
    ("air_bubble", [0xea_f7ff, 0xd6_eefb, 0xf7_fcff, 0xc4_e4f7]),
    ("flake_food", [0xe0_673f, 0xc9_532f, 0xf0_8657, 0xb5_4529]),
];

/// Darkening of water at the floor relative to the surface, in 1/256.
const DEPTH_DARKEN: u32 = 90;

/// A demo world plus its RGBA frame buffer.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct Demo {
    world: World,
    water: MaterialId,
    air: MaterialId,
    /// Shades per material ID.
    lut: Vec<[[u8; 3]; SHADES]>,
    /// Materials that darken with depth (water).
    depth_tinted: Vec<bool>,
    /// Brightness per row for depth-tinted materials, in 1/256.
    row_light: Vec<u32>,
    frame: Vec<u8>,
    /// The world's last change tick when the frame was drawn, or `None`
    /// after an edit.
    frame_tick: Option<u64>,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl Demo {
    /// A tank of `width`×`height` cells filled with water up to
    /// `water_level` (a fraction of the height) and air above.
    ///
    /// # Errors
    ///
    /// Returns a message if a side is 0 or above [`MAX_DEMO_SIDE`], or the
    /// water level is outside `0..=1`.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(constructor))]
    pub fn new(width: u32, height: u32, seed: u32, water_level: f32) -> Result<Demo, String> {
        if !(1..=MAX_DEMO_SIDE).contains(&width) || !(1..=MAX_DEMO_SIDE).contains(&height) {
            return Err(format!(
                "grid size {width}x{height} is outside 1..={MAX_DEMO_SIDE} per side"
            ));
        }
        let materials = MaterialTable::builtin().map_err(|e| e.to_string())?;
        let config = WorldConfig {
            width: width as usize,
            height: height as usize,
            seed: u64::from(seed),
            water_level,
        };
        let world = World::new(config, materials).map_err(|e| e.to_string())?;
        let id = |name| {
            world
                .materials()
                .id(name)
                .ok_or_else(|| format!("no {name} material"))
        };
        let (water, air) = (id("water")?, id("air")?);

        let mut lut = vec![[[255, 0, 255]; SHADES]; 256];
        let mut depth_tinted = vec![false; 256];
        for m in world.materials().iter() {
            let i = usize::from(m.id.0);
            lut[i] = PALETTES
                .iter()
                .find(|(name, _)| *name == m.name)
                .map_or_else(|| shades_of(m.colour), |(_, p)| p.map(rgb));
            depth_tinted[i] = m.id == water;
        }

        // Full brightness at the surface, darkening linearly to the floor.
        let (h, surface) = (world.height(), world.surface_row());
        let depth = (h - surface).max(1);
        let row_light = (0..h)
            .map(|y| {
                let d = y.saturating_sub(surface).min(depth);
                #[allow(clippy::cast_possible_truncation)] // ≤ DEPTH_DARKEN
                let dark = (d * DEPTH_DARKEN as usize / depth) as u32;
                256 - dark
            })
            .collect();

        let frame = vec![0; world.width() * h * 4];
        Ok(Demo {
            world,
            water,
            air,
            lut,
            depth_tinted,
            row_light,
            frame,
            frame_tick: None,
        })
    }

    /// Width in cells (and frame pixels).
    #[must_use]
    pub fn width(&self) -> u32 {
        side(self.world.width())
    }

    /// Height in cells (and frame pixels).
    #[must_use]
    pub fn height(&self) -> u32 {
        side(self.world.height())
    }

    /// Advances one fixed 1/20 s tick.
    pub fn tick(&mut self) {
        self.world.tick();
    }

    /// Ticks simulated so far (an `f64` so JS gets a plain number; exact
    /// below 2^53).
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // exact below 2^53 ticks
    pub fn tick_count(&self) -> f64 {
        self.world.tick_count() as f64
    }

    /// Chunks currently awake.
    #[must_use]
    pub fn awake_chunks(&self) -> u32 {
        count(self.world.awake_chunks())
    }

    /// Total chunks.
    #[must_use]
    pub fn chunk_count(&self) -> u32 {
        count(self.world.chunk_count())
    }

    /// Material names, ordered by ID.
    #[must_use]
    pub fn materials(&self) -> Vec<String> {
        self.world
            .materials()
            .iter()
            .map(|m| m.name.clone())
            .collect()
    }

    /// Pours `material` in a disc of `radius` cells at `(x, y)`, filling
    /// only fluid cells. Returns how many cells changed.
    ///
    /// # Errors
    ///
    /// Returns a message if the material name is unknown.
    pub fn pour(&mut self, material: &str, x: i32, y: i32, radius: u32) -> Result<u32, String> {
        self.brush(material, x, y, radius, Brush::Pour)
    }

    /// Paints `material` in a disc of `radius` cells at `(x, y)`,
    /// overwriting everything. Returns how many cells changed.
    ///
    /// # Errors
    ///
    /// Returns a message if the material name is unknown.
    pub fn paint(&mut self, material: &str, x: i32, y: i32, radius: u32) -> Result<u32, String> {
        self.brush(material, x, y, radius, Brush::Replace)
    }

    /// Erases a disc of `radius` cells at `(x, y)` back to the empty tank:
    /// water below the configured surface, air above. Returns how many
    /// cells changed.
    pub fn erase(&mut self, x: i32, y: i32, radius: u32) -> u32 {
        let (cx, cy) = (i64::from(x), i64::from(y));
        // As in `World::paint`: a radius past the grid covers it all, and the
        // loop is clipped to the grid, so the maths stays far from overflow.
        let r = i64::from(radius.min(2 * MAX_DEMO_SIDE));
        let (width, height) = (i64::from(self.width()), i64::from(self.height()));
        let surface = self.world.surface_row();
        let mut changed = 0;
        for py in (cy - r).max(0)..=(cy + r).min(height - 1) {
            for px in (cx - r).max(0)..=(cx + r).min(width - 1) {
                let (dx, dy) = (px - cx, py - cy);
                if dx * dx + dy * dy > r * r + r {
                    continue;
                }
                let (Ok(px), Ok(py)) = (usize::try_from(px), usize::try_from(py)) else {
                    continue;
                };
                let Some(cell) = self.world.cell(px, py) else {
                    continue;
                };
                let empty = if py >= surface { self.water } else { self.air };
                if cell.material != empty && self.world.set_cell(px, py, empty).is_ok() {
                    changed += 1;
                }
            }
        }
        if changed > 0 {
            self.frame_tick = None;
        }
        changed
    }

    /// Redraws the RGBA frame if any cell changed since the last call (a
    /// settled tank is not redrawn). Returns whether it did, so the caller
    /// can skip the canvas upload.
    pub fn render(&mut self) -> bool {
        let tick = self.world.last_change_tick();
        if self.frame_tick == Some(tick) {
            return false;
        }
        let w = self.world.width();
        for (i, (cell, px)) in self
            .world
            .cells()
            .iter()
            .zip(self.frame.as_chunks_mut::<4>().0)
            .enumerate()
        {
            let m = usize::from(cell.material.0);
            let mut c = self.lut[m][shade(cell.variant)];
            if self.depth_tinted[m] {
                let light = self.row_light[i / w];
                c = c.map(|v| dim(v, light));
            }
            *px = [c[0], c[1], c[2], 255];
        }
        self.frame_tick = Some(tick);
        true
    }

    /// Pointer to the RGBA frame (`width × height × 4` bytes) in wasm
    /// memory. Valid until the next call that may allocate; re-read it
    /// after each [`Demo::render`].
    #[must_use]
    pub fn frame_ptr(&self) -> *const u8 {
        self.frame.as_ptr()
    }

    /// Length of the RGBA frame in bytes.
    #[must_use]
    pub fn frame_len(&self) -> usize {
        self.frame.len()
    }
}

impl Demo {
    /// The RGBA frame, row-major from the top-left.
    #[must_use]
    pub fn frame(&self) -> &[u8] {
        &self.frame
    }

    /// The wrapped world.
    #[must_use]
    pub fn world(&self) -> &World {
        &self.world
    }

    fn brush(
        &mut self,
        material: &str,
        x: i32,
        y: i32,
        radius: u32,
        brush: Brush,
    ) -> Result<u32, String> {
        let id = self
            .world
            .materials()
            .id(material)
            .ok_or_else(|| format!("unknown material {material:?}"))?;
        let changed = self
            .world
            .paint(i64::from(x), i64::from(y), radius, id, brush)
            .map_err(|e| e.to_string())?;
        if changed > 0 {
            self.frame_tick = None;
        }
        Ok(count(changed))
    }
}

/// Picks a shade: mostly the base, sometimes a lighter or darker speck.
fn shade(variant: u8) -> usize {
    match variant {
        0..=119 => 0,
        120..=191 => 1,
        192..=239 => 2,
        _ => 3,
    }
}

/// Scales a channel by `light`/256 (`light` ≤ 256, so it never overflows).
fn dim(v: u8, light: u32) -> u8 {
    u8::try_from(u32::from(v) * light / 256).unwrap_or(u8::MAX)
}

fn rgb(hex: u32) -> [u8; 3] {
    let [_, r, g, b] = hex.to_be_bytes();
    [r, g, b]
}

/// Fallback shades for a material without a palette: its own colour,
/// slightly darker, slightly lighter, darker still.
fn shades_of(c: [u8; 3]) -> [[u8; 3]; SHADES] {
    let scale = |k: u16| c.map(|v| u8::try_from((u16::from(v) * k / 16).min(255)).unwrap_or(255));
    [c, scale(15), scale(17), scale(13)]
}

/// Grid sides are at most [`MAX_DEMO_SIDE`], so they fit a `u32`.
fn side(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Counts are bounded by the cell count (≤ 2^20 here).
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Demo {
        Demo::new(128, 72, 7, 0.9).unwrap()
    }

    fn pixel(d: &Demo, x: usize, y: usize) -> [u8; 4] {
        let i = (y * d.world().width() + x) * 4;
        d.frame()[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn rejects_bad_sizes() {
        assert!(Demo::new(0, 10, 0, 0.9).is_err());
        assert!(Demo::new(MAX_DEMO_SIDE + 1, 10, 0, 0.9).is_err());
        assert!(Demo::new(10, 10, 0, 1.5).is_err());
    }

    #[test]
    fn lists_builtin_materials() {
        let names = demo().materials();
        for m in [
            "water",
            "air",
            "sand",
            "gravel",
            "rock",
            "air_bubble",
            "flake_food",
        ] {
            assert!(names.iter().any(|n| n == m), "{m} missing");
        }
    }

    #[test]
    fn every_builtin_material_has_a_palette() {
        let d = demo();
        for m in d.world().materials().iter() {
            assert!(
                PALETTES.iter().any(|(n, _)| *n == m.name),
                "{} has no palette",
                m.name
            );
        }
    }

    #[test]
    fn renders_opaque_frame_once_per_change() {
        let mut d = demo();
        assert_eq!(d.frame_len(), 128 * 72 * 4);
        assert!(d.render());
        assert!(!d.render(), "nothing changed");
        assert!(d.frame().as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        // Water is darker at the floor than at the surface.
        let surface = d.world().surface_row();
        let top = pixel(&d, 5, surface);
        let floor = pixel(&d, 5, 71);
        assert!(floor[2] < top[2], "{floor:?} vs {top:?}");
        d.tick();
        assert!(!d.render(), "a still tank isn't redrawn");
        d.pour("sand", 64, 5, 2).unwrap();
        assert!(d.render(), "an edit invalidates the frame");
        d.tick();
        assert!(d.render(), "falling sand changes cells");
    }

    #[test]
    fn huge_erase_radius_clears_everything_without_overflow() {
        let mut d = demo();
        d.pour("sand", 64, 50, 6).unwrap();
        assert!(
            d.erase(-5, i32::MAX, u32::MAX) == 0,
            "disc far below the grid"
        );
        assert!(d.erase(64, 36, u32::MAX) > 0);
        let sand = d.world().materials().id("sand").unwrap();
        assert_eq!(d.world().material_counts()[usize::from(sand.0)], 0);
    }

    #[test]
    fn pour_settles_sand_and_erase_restores_water() {
        let mut d = demo();
        assert!(d.pour("sand", 64, 20, 4).unwrap() > 0);
        assert!(d.render(), "an edit invalidates the frame");
        for _ in 0..200 {
            d.tick();
        }
        let sand = d.world().materials().id("sand").unwrap();
        let floor = d.world().cell(64, 71).unwrap();
        assert_eq!(floor.material, sand);
        assert!(d.erase(64, 71, 3) > 0);
        assert_eq!(d.world().cell(64, 71).unwrap().material, d.water);
        // Above the surface, erase leaves air.
        d.paint("rock", 10, 1, 1).unwrap();
        assert_eq!(d.erase(10, 1, 1), 9, "a radius-1 disc is 3×3");
        assert_eq!(d.world().cell(10, 1).unwrap().material, d.air);
        assert_eq!(d.erase(10, 1, 1), 0, "already empty");
    }

    #[test]
    fn unknown_material_is_an_error() {
        assert!(demo().pour("lava", 1, 1, 1).is_err());
    }

    #[test]
    fn same_seed_same_world() {
        let run = || {
            let mut d = demo();
            d.pour("gravel", 30, 10, 5).unwrap();
            for _ in 0..50 {
                d.tick();
            }
            d.world().state_hash()
        };
        assert_eq!(run(), run());
    }
}
