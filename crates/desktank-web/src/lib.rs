//! DeskTank web host (SPEC §3.5, WB1).
//!
//! A [`wasm-bindgen`](https://docs.rs/wasm-bindgen) wrapper that drives the
//! loop-agnostic [`desktank_render::Tank`] on a page's canvas. The page
//! (`web/main.js`) owns the `requestAnimationFrame` loop, input, visibility
//! and battery signals; this crate owns the world, the pacing (a fixed 20 Hz
//! tick, frames capped at 30 fps and drawn only when something changed) and
//! the GPU:
//!
//! - [`WebTank`] is the world plus its pacing and edits. It needs no GPU, so
//!   it is tested natively on every OS.
//! - `Graphics` (wasm32 only) is a wgpu device and canvas surface. It tries
//!   WebGPU first and falls back to WebGL2 (or uses the one forced by
//!   `?backend=`). After device or context loss the page detaches it and
//!   opens a new one; the world is unaffected.
//!
//! `unsafe_code`: the workspace denies it, and this crate needs no
//! exception. The `unsafe` that `#[wasm_bindgen]` generates comes from an
//! external proc macro, which rustc's `unsafe_code` lint doesn't report (CI
//! runs clippy for `wasm32` to catch it if that ever changes). Code written
//! here stays safe.

use core::time::Duration;

use desktank_render::{Layout, Tank, TankConfig};
use desktank_sim::{Brush, MaterialId, MaterialTable, Script, World, WorldConfig};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;

#[cfg(target_arch = "wasm32")]
mod graphics;
#[cfg(target_arch = "wasm32")]
pub use graphics::Graphics;

/// Largest grid side: WebGL2's texture limit (SPEC §3.5).
pub const MAX_GRID_SIDE: u32 = 2048;

/// The web picks the largest whole-number cell scale that still gives at
/// least this many cells across (SPEC §3.5).
pub const MIN_CELLS_ACROSS: u32 = 320;

/// Largest cell scale (SPEC §2.1).
pub const MAX_SCALE: u32 = 8;

/// Water level of a new tank, as a fraction of the height.
const WATER_LEVEL: f32 = 0.9;

/// [`WebTank::frame`] drew and presented a frame.
pub const FRAME_DREW: u32 = 1;
/// A frame is still waiting (on the frame cap, or on a surface that gave
/// none), so the page should keep calling [`WebTank::frame`].
pub const FRAME_WANTED: u32 = 2;
/// The GPU device was lost; the page should detach and reopen graphics.
pub const FRAME_LOST: u32 = 4;
/// Simulation ticks run by the call are in the bits from here up.
pub const FRAME_TICKS_SHIFT: u32 = 8;

/// The grid for a canvas of `width`×`height` device pixels:
/// `[scale, columns, rows]`, with the largest whole-number scale (1 to
/// [`MAX_SCALE`]) that gives at least [`MIN_CELLS_ACROSS`] columns. A canvas
/// narrower than that gets scale 1.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = gridFor))]
#[must_use]
pub fn grid_for(width: u32, height: u32) -> Vec<u32> {
    let scale = (width / MIN_CELLS_ACROSS).clamp(1, MAX_SCALE);
    let cells = |px: u32| (px / scale).clamp(1, MAX_GRID_SIDE);
    vec![scale, cells(width), cells(height)]
}

/// A brush held still on the canvas: it keeps pouring every tick, like a
/// tap.
#[derive(Clone, Copy, Debug)]
struct Hold {
    x: i32,
    y: i32,
    radius: u32,
    /// `None` erases.
    material: Option<MaterialId>,
}

/// A tank on the page: the world, its pacing and its edits, plus the
/// attached graphics on wasm32.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct WebTank {
    tank: Tank,
    water: MaterialId,
    air: MaterialId,
    running: bool,
    hold: Option<Hold>,
    /// Canvas size in device pixels.
    size: (u32, u32),
    frames: u32,
    #[cfg(target_arch = "wasm32")]
    graphics: Option<Graphics>,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl WebTank {
    /// A tank of `width`×`height` cells, water up to 90% of the height and
    /// air above, drawn into a `canvas_width`×`canvas_height` pixel canvas.
    ///
    /// # Errors
    ///
    /// Returns a message if a side is 0 or above [`MAX_GRID_SIDE`].
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(constructor))]
    pub fn new(
        width: u32,
        height: u32,
        seed: u32,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Result<WebTank, String> {
        check_side(width, height)?;
        let config = WorldConfig {
            width: width as usize,
            height: height as usize,
            seed: u64::from(seed),
            water_level: WATER_LEVEL,
        };
        Self::with_world(
            World::new(config, builtin()?).map_err(|e| e.to_string())?,
            (canvas_width, canvas_height),
        )
    }

    /// A tank running the `desktank capture` input script `source` (TOML,
    /// `docs/capture.md`) with `seed` up to the script's first frame, then
    /// paused: the same scene `desktank capture --seed <seed> --script` renders.
    ///
    /// # Errors
    ///
    /// Returns a message if the script is invalid or asks for the starter
    /// scene, which only the native app lays out.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = fromScript))]
    pub fn from_script(
        source: &str,
        seed: u32,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Result<WebTank, String> {
        let script = Script::parse(source).map_err(|e| e.to_string())?;
        if script.world.starter {
            return Err("the web can't lay out the starter scene".to_owned());
        }
        let config = script.world_config(u64::from(seed));
        check_side(side(config.width), side(config.height))?;
        let mut world = World::new(config, builtin()?).map_err(|e| e.to_string())?;
        script
            .run(&mut world, script.ticks)
            .map_err(|e| e.to_string())?;
        let mut tank = Self::with_world(world, (canvas_width, canvas_height))?;
        tank.running = false;
        Ok(tank)
    }

    /// Width in cells.
    #[must_use]
    pub fn width(&self) -> u32 {
        side(self.tank.world().width())
    }

    /// Height in cells.
    #[must_use]
    pub fn height(&self) -> u32 {
        side(self.tank.world().height())
    }

    /// Material names, ordered by ID.
    #[must_use]
    pub fn materials(&self) -> Vec<String> {
        self.tank
            .world()
            .materials()
            .iter()
            .map(|m| m.name.clone())
            .collect()
    }

    /// Whether the simulation is running (not paused by the user).
    #[must_use]
    pub fn running(&self) -> bool {
        self.running
    }

    /// Pauses or resumes the simulation. Paused, world time stops; edits and
    /// [`WebTank::step`] still show.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = setRunning))]
    pub fn set_running(&mut self, running: bool) {
        self.running = running;
    }

    /// Stops world time until the next [`WebTank::frame`], e.g. while the
    /// page is hidden (SPEC §3.5): the clock re-anchors instead of catching
    /// up.
    pub fn pause(&mut self) {
        self.tank.pause();
    }

    /// Runs one simulation tick now (single-stepping while paused).
    pub fn step(&mut self) {
        self.tank.step();
    }

    /// Changes the frame-rate cap (30 on AC power, 15 on battery).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = setMaxFps))]
    pub fn set_max_fps(&mut self, fps: u32) {
        self.tank.set_max_fps(fps);
    }

    /// The canvas was resized to `width`×`height` device pixels. The grid
    /// keeps its size and is redrawn at the largest scale that fits,
    /// letterboxed.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.size = (width.max(1), height.max(1));
        #[cfg(target_arch = "wasm32")]
        if let Some(g) = &mut self.graphics {
            g.resize(self.size);
        }
        self.tank.invalidate();
    }

    /// The cell under canvas pixel `(x, y)` as `[column, row]`. Signed and
    /// unclipped: the letterbox maps to cells just outside the grid.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = cellAt))]
    #[must_use]
    pub fn cell_at(&self, x: f64, y: f64) -> Vec<i32> {
        let layout = Layout::fit((self.width(), self.height()), self.size);
        let (cx, cy) = layout.cell_at(x, y);
        let clamp = |v: i64| i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX });
        vec![clamp(cx), clamp(cy)]
    }

    /// Pours `material` in a disc of `radius` cells at `(x, y)`, filling only
    /// fluid cells. Returns how many cells changed.
    ///
    /// # Errors
    ///
    /// Returns a message if the material name is unknown.
    pub fn pour(&mut self, material: &str, x: i32, y: i32, radius: u32) -> Result<u32, String> {
        let id = self.material(material)?;
        Ok(self.stamp(Some(id), x, y, radius))
    }

    /// Erases a disc of `radius` cells at `(x, y)` back to the empty tank:
    /// water below the configured surface, air above. Returns how many cells
    /// changed.
    pub fn erase(&mut self, x: i32, y: i32, radius: u32) -> u32 {
        self.stamp(None, x, y, radius)
    }

    /// Holds the brush at `(x, y)`: until [`WebTank::release`], it pours
    /// `material` (or erases, if `None`) again on every simulation tick.
    ///
    /// # Errors
    ///
    /// Returns a message if the material name is unknown.
    pub fn hold(
        &mut self,
        material: Option<String>,
        x: i32,
        y: i32,
        radius: u32,
    ) -> Result<(), String> {
        let material = material.map(|m| self.material(&m)).transpose()?;
        self.hold = Some(Hold {
            x,
            y,
            radius,
            material,
        });
        Ok(())
    }

    /// Lifts the held brush.
    pub fn release(&mut self) {
        self.hold = None;
    }

    /// Advances to `now_ms` (`performance.now()`): runs the simulation ticks
    /// due, and draws a frame if one is due and allowed. Returns
    /// [`FRAME_DREW`], [`FRAME_WANTED`] and [`FRAME_LOST`] flags, with the
    /// simulation ticks run shifted by [`FRAME_TICKS_SHIFT`].
    pub fn frame(&mut self, now_ms: f64) -> u32 {
        let now = Duration::from_secs_f64(now_ms.max(0.0) / 1000.0);
        if !self.running {
            // Holding the clock: tick() re-anchors and runs nothing.
            self.tank.pause();
        }
        let request = self.tank.tick(now);
        if request.sim_ticks > 0
            && let Some(h) = self.hold
        {
            self.stamp(h.material, h.x, h.y, h.radius);
        }
        let mut flags = request.sim_ticks.min(0xff_ffff) << FRAME_TICKS_SHIFT;
        if request.redraw && self.draw() {
            self.frames = self.frames.wrapping_add(1);
            flags |= FRAME_DREW;
        }
        #[cfg(target_arch = "wasm32")]
        if self.graphics.as_ref().is_some_and(Graphics::is_lost) {
            flags |= FRAME_LOST;
        }
        if self.tank.frame_wanted() {
            flags |= FRAME_WANTED;
        }
        flags
    }

    /// Frames drawn so far (wraps).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = framesDrawn))]
    #[must_use]
    pub fn frames_drawn(&self) -> u32 {
        self.frames
    }

    /// Ticks simulated so far (an `f64` so JS gets a plain number; exact
    /// below 2^53).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = tickCount))]
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // exact below 2^53 ticks
    pub fn tick_count(&self) -> f64 {
        self.tank.world().tick_count() as f64
    }

    /// Chunks currently awake.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = awakeChunks))]
    #[must_use]
    pub fn awake_chunks(&self) -> u32 {
        count(self.tank.world().awake_chunks())
    }

    /// Total chunks.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = chunkCount))]
    #[must_use]
    pub fn chunk_count(&self) -> u32 {
        count(self.tank.world().chunk_count())
    }

    /// The world's state hash (FNV-1a) as 16 hex digits, for tests.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = stateHash))]
    #[must_use]
    pub fn state_hash(&self) -> String {
        format!("{:016x}", self.tank.world().state_hash())
    }
}

impl WebTank {
    fn with_world(world: World, size: (u32, u32)) -> Result<Self, String> {
        let id = |name| {
            world
                .materials()
                .id(name)
                .ok_or_else(|| format!("no {name} material"))
        };
        let (water, air) = (id("water")?, id("air")?);
        Ok(Self {
            tank: Tank::new(world, TankConfig::default()),
            water,
            air,
            running: true,
            hold: None,
            size: (size.0.max(1), size.1.max(1)),
            frames: 0,
            #[cfg(target_arch = "wasm32")]
            graphics: None,
        })
    }

    /// The wrapped tank.
    #[must_use]
    pub fn tank(&self) -> &Tank {
        &self.tank
    }

    fn material(&self, name: &str) -> Result<MaterialId, String> {
        self.tank
            .world()
            .materials()
            .id(name)
            .ok_or_else(|| format!("unknown material {name:?}"))
    }

    /// Pours `material`, or erases if `None`. While paused, an edit
    /// re-uploads the grid so it shows without a tick.
    fn stamp(&mut self, material: Option<MaterialId>, x: i32, y: i32, radius: u32) -> u32 {
        let changed = match material {
            Some(id) => self
                .tank
                .world_mut()
                .paint(i64::from(x), i64::from(y), radius, id, Brush::Pour)
                .unwrap_or(0),
            None => self.erase_disc(x, y, radius),
        };
        if changed > 0 && !self.running {
            self.tank.cells_edited();
        }
        count(changed)
    }

    fn erase_disc(&mut self, x: i32, y: i32, radius: u32) -> usize {
        let (water, air) = (self.water, self.air);
        let world = self.tank.world_mut();
        let (cx, cy) = (i64::from(x), i64::from(y));
        // As in `World::paint`: a radius past the grid covers it all, and the
        // loop is clipped to the grid, so the maths stays far from overflow.
        let r = i64::from(radius.min(2 * MAX_GRID_SIDE));
        let (width, height) = (world.width(), world.height());
        let surface = world.surface_row();
        let (Ok(cols), Ok(rows)) = (i64::try_from(width), i64::try_from(height)) else {
            return 0;
        };
        let mut changed = 0;
        for py in (cy - r).max(0)..=(cy + r).min(rows - 1) {
            for px in (cx - r).max(0)..=(cx + r).min(cols - 1) {
                let (dx, dy) = (px - cx, py - cy);
                if dx * dx + dy * dy > r * r + r {
                    continue;
                }
                let (Ok(px), Ok(py)) = (usize::try_from(px), usize::try_from(py)) else {
                    continue;
                };
                let Some(cell) = world.cell(px, py) else {
                    continue;
                };
                let empty = if py >= surface { water } else { air };
                if cell.material != empty && world.set_cell(px, py, empty).is_ok() {
                    changed += 1;
                }
            }
        }
        changed
    }

    /// Draws to the attached graphics, if any. Returns whether a frame was
    /// presented.
    #[cfg(target_arch = "wasm32")]
    fn draw(&mut self) -> bool {
        let Some(g) = &self.graphics else {
            return false;
        };
        if g.is_lost() {
            return false;
        }
        match self.tank.draw(g.gpu(), g.surface()) {
            Ok(drawn) => drawn,
            Err(e) => {
                web_sys::console::error_1(&format!("desktank: {e}").into());
                false
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    fn draw(&mut self) -> bool {
        false
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
impl WebTank {
    /// Takes `graphics` and draws with it from the next frame, rebuilding the
    /// renderer from the world.
    pub fn attach(&mut self, graphics: Graphics) {
        self.tank.release_gpu();
        self.graphics = Some(graphics);
        let size = self.size;
        self.resize(size.0, size.1);
    }

    /// Drops the graphics (after device or context loss). The world keeps
    /// running; nothing is drawn until [`WebTank::attach`].
    pub fn detach(&mut self) {
        self.tank.release_gpu();
        self.graphics = None;
    }

    /// The attached graphics backend (`"webgpu"`, `"webgl2"`), or `""`.
    #[must_use]
    pub fn backend(&self) -> String {
        self.graphics
            .as_ref()
            .map_or_else(String::new, Graphics::backend)
    }
}

fn builtin() -> Result<MaterialTable, String> {
    MaterialTable::builtin().map_err(|e| e.to_string())
}

fn check_side(width: u32, height: u32) -> Result<(), String> {
    if (1..=MAX_GRID_SIDE).contains(&width) && (1..=MAX_GRID_SIDE).contains(&height) {
        Ok(())
    } else {
        Err(format!(
            "grid size {width}x{height} is outside 1..={MAX_GRID_SIDE} per side"
        ))
    }
}

/// Grid sides fit a `u32` (checked against [`MAX_GRID_SIDE`]).
fn side(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Counts are bounded by the cell count (≤ 2^22 here).
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: f64 = 1.0;

    fn tank() -> WebTank {
        WebTank::new(128, 72, 7, 512, 288).unwrap()
    }

    fn id(t: &WebTank, name: &str) -> MaterialId {
        t.tank().world().materials().id(name).unwrap()
    }

    #[test]
    fn grid_scale_is_the_largest_giving_320_across() {
        assert_eq!(grid_for(1920, 1080), [6, 320, 180]);
        assert_eq!(grid_for(1896, 1066), [5, 379, 213]);
        assert_eq!(grid_for(640, 360), [2, 320, 180]);
        assert_eq!(grid_for(639, 359), [1, 639, 359]);
        assert_eq!(grid_for(200, 100), [1, 200, 100], "narrow: scale 1");
        assert_eq!(grid_for(5120, 2880), [8, 640, 360], "scale capped at 8");
        assert_eq!(grid_for(0, 0), [1, 1, 1]);
    }

    #[test]
    fn rejects_bad_sizes() {
        assert!(WebTank::new(0, 10, 0, 1, 1).is_err());
        assert!(WebTank::new(MAX_GRID_SIDE + 1, 10, 0, 1, 1).is_err());
    }

    #[test]
    fn lists_builtin_materials() {
        let names = tank().materials();
        for m in ["water", "air", "sand", "gravel", "rock", "air_bubble"] {
            assert!(names.iter().any(|n| n == m), "{m} missing");
        }
    }

    #[test]
    fn frame_ticks_at_20_hz_and_pauses() {
        let mut t = tank();
        let ticks = |f: u32| f >> FRAME_TICKS_SHIFT;
        assert_eq!(ticks(t.frame(0.0)), 0, "the first frame starts the clock");
        assert_eq!(ticks(t.frame(16.0 * MS)), 0);
        assert_eq!(ticks(t.frame(50.0 * MS)), 1);
        assert_eq!(ticks(t.frame(150.0 * MS)), 2);
        t.set_running(false);
        assert_eq!(ticks(t.frame(1000.0 * MS)), 0);
        assert_eq!(ticks(t.frame(2000.0 * MS)), 0, "paused: no world time");
        t.step();
        assert_eq!(t.tick_count(), 4.0);
        t.set_running(true);
        assert_eq!(ticks(t.frame(2000.0 * MS)), 0, "resuming doesn't catch up");
        assert_eq!(ticks(t.frame(2050.0 * MS)), 1);
    }

    #[test]
    fn a_frame_is_wanted_after_an_edit() {
        let mut t = tank();
        // Without graphics nothing is drawn, so the first frame stays wanted.
        assert_ne!(t.frame(0.0) & FRAME_WANTED, 0);
        assert_eq!(t.frame(0.0) & FRAME_DREW, 0);
    }

    #[test]
    fn held_brush_pours_every_tick() {
        let mut t = tank();
        let sand = id(&t, "sand");
        let count = |t: &WebTank| t.tank().world().material_counts()[usize::from(sand.0)];
        t.hold(Some("sand".into()), 64, 20, 2).unwrap();
        t.frame(0.0);
        assert_eq!(count(&t), 0, "no tick yet");
        t.frame(50.0);
        let one = count(&t);
        assert!(one > 0);
        for i in 2..10 {
            t.frame(f64::from(i) * 50.0);
        }
        assert!(count(&t) > one, "kept pouring");
        t.release();
        let before = count(&t);
        t.frame(600.0);
        assert_eq!(count(&t), before, "released");
        assert!(t.hold(Some("lava".into()), 0, 0, 1).is_err());
    }

    #[test]
    fn pour_settles_sand_and_erase_restores_water() {
        let mut t = tank();
        assert!(t.pour("sand", 64, 20, 4).unwrap() > 0);
        for _ in 0..200 {
            t.step();
        }
        let (sand, water, air) = (id(&t, "sand"), id(&t, "water"), id(&t, "air"));
        let world = || t.tank().world();
        assert_eq!(world().cell(64, 71).unwrap().material, sand);
        let mut t = t;
        assert!(t.erase(64, 71, 3) > 0);
        assert_eq!(t.tank().world().cell(64, 71).unwrap().material, water);
        // Above the surface, erase leaves air.
        t.tank
            .world_mut()
            .paint(10, 1, 1, sand, Brush::Replace)
            .unwrap();
        assert_eq!(t.erase(10, 1, 1), 9, "a radius-1 disc is 3×3");
        assert_eq!(t.tank().world().cell(10, 1).unwrap().material, air);
        assert_eq!(t.erase(10, 1, 1), 0, "already empty");
    }

    #[test]
    fn huge_erase_radius_clears_everything_without_overflow() {
        let mut t = tank();
        t.pour("sand", 64, 50, 6).unwrap();
        assert_eq!(
            t.erase(-5, i32::MAX, u32::MAX),
            0,
            "disc far below the grid"
        );
        assert!(t.erase(64, 36, u32::MAX) > 0);
        let sand = id(&t, "sand");
        assert_eq!(t.tank().world().material_counts()[usize::from(sand.0)], 0);
    }

    #[test]
    fn unknown_material_is_an_error() {
        assert!(tank().pour("lava", 1, 1, 1).is_err());
    }

    #[test]
    fn maps_canvas_pixels_to_cells_with_letterbox() {
        let mut t = tank();
        // 512×288 holds 128×72 at scale 4 exactly.
        assert_eq!(t.cell_at(0.0, 0.0), [0, 0]);
        assert_eq!(t.cell_at(511.0, 287.0), [127, 71]);
        // A wider canvas letterboxes left and right at scale 4.
        t.resize(600, 288);
        assert_eq!(t.cell_at(44.0, 0.0), [0, 0]);
        assert_eq!(t.cell_at(43.0, 0.0), [-1, 0]);
    }

    #[test]
    fn script_matches_native_capture_world() {
        let src = include_str!("../../desktank/tests/scenes/layers.toml");
        let t = WebTank::from_script(src, 7, 320, 180).unwrap();
        let script = Script::parse(src).unwrap();
        let mut world = World::new(script.world_config(7), builtin().unwrap()).unwrap();
        script.run(&mut world, script.ticks).unwrap();
        assert_eq!(t.tank().world().state_hash(), world.state_hash());
        assert_eq!(t.tick_count(), 160.0);
        assert!(!t.running(), "a scripted scene starts paused");
        let starter = include_str!("../../desktank/tests/scenes/starter.toml");
        assert!(WebTank::from_script(starter, 7, 320, 180).is_err());
    }

    #[test]
    fn same_seed_same_world() {
        let run = || {
            let mut t = tank();
            t.pour("gravel", 30, 10, 5).unwrap();
            for _ in 0..50 {
                t.step();
            }
            t.state_hash()
        };
        assert_eq!(run(), run());
    }
}
