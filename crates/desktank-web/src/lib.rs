//! DeskTank web host (SPEC §3.5, WB1, WB2, WB3).
//!
//! A [`wasm-bindgen`](https://docs.rs/wasm-bindgen) wrapper that drives the
//! loop-agnostic [`desktank_render::Tank`] on a page's canvas. The page
//! (`web/main.js`) owns the `requestAnimationFrame` loop, the DOM controls,
//! gesture recognition (touch), visibility and battery signals; this crate
//! owns the world, the pacing (a fixed 20 Hz tick, frames capped at 30 fps and
//! drawn only when something changed), the interact tools and the GPU:
//!
//! - [`WebTank`] is the world plus its pacing and the platform-free
//!   [`desktank_sim::tools::Tools`] (pour, scoop, glue, net, inspect and
//!   devices; brush radius; stroke interpolation). The page passes pointer
//!   events in canvas pixels, which map to cells with
//!   [`desktank_render::Layout`]. It needs no GPU, so it is tested natively on
//!   every OS.
//! - `Graphics` (wasm32 only) is a wgpu device and canvas surface. It tries
//!   WebGPU first and falls back to WebGL2 (or uses the one forced by
//!   `?backend=`). After device or context loss the page detaches it and
//!   opens a new one; the world is unaffected.
//!
//! The palette is DOM buttons rather than the shader's strip
//! ([`desktank_render::PaletteLayout`]): touch needs larger targets than the
//! pixel-art slots, and buttons give the page a keyboard-navigable radio
//! group. Its items come from [`Tools::palette`], so the page lists exactly
//! the sim's materials, tools and devices, and [`WebTank::palette_groups`]
//! gives each one's group for the page's group tabs. The brush outline is the shader's
//! ([`Tank::set_cursor`]).
//!
//! Saves (WB3) are [`desktank_sim::save`]'s format, the same bytes as the
//! native app's `.dtank` files: [`WebTank::save`] encodes, and
//! [`WebTank::from_save`] opens one at its own grid size. The page keeps
//! them in `IndexedDB` and owns the tab ownership (Web Locks).
//!
//! `unsafe_code`: the workspace denies it, and this crate needs no
//! exception. The `unsafe` that `#[wasm_bindgen]` generates comes from an
//! external proc macro, which rustc's `unsafe_code` lint doesn't report (CI
//! runs clippy for `wasm32` to catch it if that ever changes). Code written
//! here stays safe.

use core::time::Duration;

use desktank_render::{BrushPreview, Layout, Tank, TankConfig};
use desktank_sim::tools::{
    self, Button, Inspection, Outcome, PaletteGroup, PaletteItem, Tool, Tools,
};
use desktank_sim::{
    CreatureCard, CreatureId, FieldKind, MaterialTable, Propagule, Script, World, WorldConfig,
    cheats, save,
};
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

/// Pointer button codes the page passes: the primary button (left mouse, a
/// one-finger touch) uses the selected tool.
pub const BUTTON_PRIMARY: u32 = 0;
/// The secondary button (right mouse, a two-finger touch) always scoops.
pub const BUTTON_SECONDARY: u32 = 2;

/// The grid for a canvas of `width`×`height` device pixels:
/// `[scale, columns, rows]`. With `scale` 0 (auto) it is the largest even
/// scale (2 to [`MAX_SCALE`]) that gives at least [`MIN_CELLS_ACROSS`]
/// columns, or 1 if even 2 gives fewer. Any other `scale` is the user's
/// override (settings), clamped to 1..=[`MAX_SCALE`] and rounded down to an
/// even number above 1. Even scales draw every sprite texel (two per cell
/// side) the same size; see [`desktank_render::Layout`].
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = gridFor))]
#[must_use]
pub fn grid_for(width: u32, height: u32, scale: u32) -> Vec<u32> {
    let scale = if scale == 0 {
        width / MIN_CELLS_ACROSS
    } else {
        scale
    }
    .clamp(1, MAX_SCALE);
    let scale = desktank_render::even_scale(scale);
    let cells = |px: u32| (px / scale).clamp(1, MAX_GRID_SIDE);
    vec![scale, cells(width), cells(height)]
}

/// Smallest and largest brush radius in cells, `[min, max]`.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = brushRange))]
#[must_use]
pub fn brush_range() -> Vec<u32> {
    vec![tools::MIN_RADIUS, tools::MAX_RADIUS]
}

/// A tank on the page: the world, its pacing and the interact tools, plus
/// the attached graphics on wasm32.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct WebTank {
    tank: Tank,
    tools: Tools,
    avoid_cursor: bool,
    palette: Vec<PaletteItem>,
    /// Each palette item's group.
    groups: Vec<PaletteGroup>,
    running: bool,
    /// What inspect found, not yet taken by [`WebTank::take_inspection`].
    inspection: String,
    /// When the page should call [`WebTank::frame`] again, on its clock.
    wake_at: Duration,
    /// Canvas size in device pixels.
    size: (u32, u32),
    frames: u32,
    /// The world changed since the last [`WebTank::save`] (or load).
    dirty: bool,
    /// Watches the page's key presses for the code that opens the cheat
    /// field.
    konami: cheats::Konami,
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

    /// Opens a save file (the native app's `.dtank` format, SPEC §2.6) at
    /// its own grid size, drawn into a `canvas_width`×`canvas_height` pixel
    /// canvas at the largest scale that fits, letterboxed. It continues
    /// exactly where the saved tank stopped.
    ///
    /// # Errors
    ///
    /// Returns a message if the bytes aren't a save this build loads, or
    /// its grid is larger than the web allows ([`MAX_GRID_SIDE`]).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = fromSave))]
    pub fn from_save(
        bytes: &[u8],
        canvas_width: u32,
        canvas_height: u32,
    ) -> Result<WebTank, String> {
        // The envelope check is cheap; a save too large for the web is
        // rejected after decoding, which is bounded by the format.
        let world = save::decode(bytes, builtin()?).map_err(|e| e.to_string())?;
        check_side(side(world.width()), side(world.height()))?;
        Self::with_world(world, (canvas_width, canvas_height))
    }

    /// Encodes the tank as a save file (SPEC §2.6; the native app's
    /// `.dtank` format) and marks it saved.
    #[must_use]
    pub fn save(&mut self) -> Vec<u8> {
        self.dirty = false;
        save::encode(self.tank.world())
    }

    /// Encodes the tank as a save file like [`WebTank::save`], for a file
    /// the user exports, without marking it saved.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = exportSave))]
    #[must_use]
    pub fn export_save(&self) -> Vec<u8> {
        save::encode(self.tank.world())
    }

    /// Whether the world changed (ticks, edits, creatures) since the last
    /// [`WebTank::save`] or since it was loaded.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = saveNeeded))]
    #[must_use]
    pub fn save_needed(&self) -> bool {
        self.dirty
    }

    /// Screen pixels per cell side in the current canvas.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = cellScale))]
    #[must_use]
    pub fn cell_scale(&self) -> u32 {
        self.layout().scale
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

    /// The palette, in order, as `"<kind> <name>"`: kind `material` (pour
    /// it; the name is the material's), `tool` (`scoop`, `glue`, `net`,
    /// `inspect`), `device` (place it; the name is its material's),
    /// `creature` (add one; the name is the species') or `plant` (the one
    /// plants slot, named `plants`: pour the species
    /// [`WebTank::set_plant_species`] picked, as the propagule
    /// [`WebTank::set_plant_propagule`] picked).
    #[must_use]
    pub fn palette(&self) -> Vec<String> {
        self.palette
            .iter()
            .map(|&item| {
                let (kind, name) = self.describe(item);
                format!("{kind} {name}")
            })
            .collect()
    }

    /// Each palette item's group, as `"<name> <label>"` (e.g.
    /// `"substrate Substrate"`); items of a group are consecutive, groups
    /// in palette order.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = paletteGroups))]
    #[must_use]
    pub fn palette_groups(&self) -> Vec<String> {
        self.groups
            .iter()
            .map(|g| format!("{} {}", g.name(), g.label()))
            .collect()
    }

    /// Swatch colour of each palette item as `#rrggbb` (the material's, or
    /// a species' main colour), or `""` for tools.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = paletteColours))]
    #[must_use]
    pub fn palette_colours(&self) -> Vec<String> {
        let world = self.tank.world();
        let table = world.materials();
        self.palette
            .iter()
            .map(|&item| {
                let colour = match item {
                    PaletteItem::Material(m) => table.get(m).map(|m| m.colour),
                    PaletteItem::Tool(Tool::Place(p)) => table
                        .id(p.material_name())
                        .and_then(|id| table.get(id))
                        .map(|m| m.colour),
                    PaletteItem::Tool(Tool::AddCreature(s)) => {
                        world.species().get(s).map(|s| s.colour)
                    }
                    PaletteItem::Tool(Tool::Plant) => self
                        .tools
                        .plant_species(world)
                        .and_then(|id| world.plant_species().get(id))
                        .map(desktank_sim::PlantSpecies::colour),
                    PaletteItem::Tool(_) => None,
                };
                colour.map_or_else(String::new, |[r, g, b]| format!("#{r:02x}{g:02x}{b:02x}"))
            })
            .collect()
    }

    /// The names of the species the palette offers (all but the cheat-only
    /// ones), ordered by ID.
    #[must_use]
    pub fn species(&self) -> Vec<String> {
        self.tank
            .world()
            .species()
            .iter()
            .filter(|s| !s.cheat_only)
            .map(|s| s.name.clone())
            .collect()
    }

    /// Plant species, ordered by ID, as `"<name> <description>"` (the plants
    /// slot's picker).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = plantSpecies))]
    #[must_use]
    pub fn plant_species(&self) -> Vec<String> {
        self.tank
            .world()
            .plant_species()
            .iter()
            .map(|s| format!("{} {}", s.name, s.description))
            .collect()
    }

    /// Index (in [`WebTank::plant_species`]) of the species the plants slot
    /// pours.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = plantSpeciesSelected))]
    #[must_use]
    pub fn plant_species_selected(&self) -> u32 {
        let world = self.tank.world();
        let picked = self.tools.plant_species(world);
        let i = world
            .plant_species()
            .iter()
            .position(|s| Some(s.id) == picked);
        count(i.unwrap_or(0))
    }

    /// Picks the species the plants slot pours, by index in
    /// [`WebTank::plant_species`] (ignored if out of range). Doesn't change
    /// the selected tool.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = setPlantSpecies))]
    pub fn set_plant_species(&mut self, index: u32) {
        let id = self
            .tank
            .world()
            .plant_species()
            .iter()
            .nth(index as usize)
            .map(|s| s.id);
        if let Some(id) = id {
            self.tools.set_plant_species(id);
        }
    }

    /// The propagules the picked species grows from, as names (`seeds`,
    /// `cuttings`, `fragments`, `runners`): the plants slot's second picker.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = plantPropagules))]
    #[must_use]
    pub fn plant_propagules(&self) -> Vec<String> {
        let world = self.tank.world();
        self.tools
            .plant_species(world)
            .and_then(|id| world.plant_species().get(id))
            .map_or_default(|s| s.propagules().map(|p| p.name().to_owned()).collect())
    }

    /// The propagule the plants slot pours (one of
    /// [`WebTank::plant_propagules`]), or `""` without plant species.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = plantPropagule))]
    #[must_use]
    pub fn plant_propagule(&self) -> String {
        self.tools
            .plant_propagule(self.tank.world())
            .map_or_else(String::new, |p| p.name().to_owned())
    }

    /// Picks the propagule the plants slot pours, by name; ignored (returns
    /// false) unless the picked species grows from it. A species picked
    /// later that doesn't grow from it pours its own default.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = setPlantPropagule))]
    pub fn set_plant_propagule(&mut self, name: &str) -> bool {
        Propagule::from_name(name)
            .is_some_and(|p| self.tools.set_plant_propagule(self.tank.world(), p))
    }

    /// Takes `old`'s plant species and propagule picks, for a new tank
    /// replacing it (a stored propagule pick survives even while the picked
    /// species doesn't grow from it).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = keepPlantPicks))]
    pub fn keep_plant_picks(&mut self, old: &WebTank) {
        self.tools.keep_plant_picks(&old.tools);
    }

    /// Plants in the tank.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = plantCount))]
    #[must_use]
    pub fn plant_count(&self) -> u32 {
        count(self.tank.world().plant_count())
    }

    /// Creatures in the tank.
    #[must_use]
    pub fn creatures(&self) -> u32 {
        count(self.tank.world().creatures().len())
    }

    /// Index of the selected palette item.
    #[must_use]
    pub fn selected(&self) -> u32 {
        let i = self
            .palette
            .iter()
            .position(|&item| self.tools.is_selected(item));
        count(i.unwrap_or(0))
    }

    /// Selects palette item `index` (ignored if out of range). Ends any
    /// stroke in progress.
    pub fn select(&mut self, index: u32) {
        if let Some(&item) = self.palette.get(index as usize) {
            self.tools.select(item);
            self.sync_cursor();
        }
    }

    /// Brush radius in cells.
    #[must_use]
    pub fn radius(&self) -> u32 {
        self.tools.radius()
    }

    /// Sets the brush radius, clamped to [`brush_range`].
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = setRadius))]
    pub fn set_radius(&mut self, radius: u32) {
        let steps = i64::from(radius) - i64::from(self.tools.radius());
        self.resize_brush(i32::try_from(steps).unwrap_or(i32::MAX));
    }

    /// Grows (positive) or shrinks the brush by `steps` cells of radius.
    /// Returns the new radius.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = resizeBrush))]
    pub fn resize_brush(&mut self, steps: i32) -> u32 {
        if self.tools.resize(steps) {
            self.sync_cursor();
        }
        self.tools.radius()
    }

    /// Turns easy mode on or off (SPEC §2.4: only old age kills). Off by
    /// default.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = setEasyMode))]
    pub fn set_easy_mode(&mut self, easy: bool) {
        self.tank.world_mut().set_easy_mode(easy);
    }

    /// Whether easy mode is on.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = easyMode))]
    #[must_use]
    pub fn easy_mode(&self) -> bool {
        self.tank.world().easy_mode()
    }

    /// Enables fish fleeing the pointer. Off by default.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = setCursorAvoidance))]
    pub fn set_cursor_avoidance(&mut self, enabled: bool) {
        self.avoid_cursor = enabled;
        self.sync_cursor();
    }

    /// A pointer `button` ([`BUTTON_PRIMARY`] or [`BUTTON_SECONDARY`]; others
    /// are ignored) went down at canvas pixel `(x, y)`. Returns how many
    /// cells (or, for glue, bodies) changed.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = pointerDown))]
    pub fn pointer_down(&mut self, button: u32, x: f64, y: f64) -> u32 {
        let Some(button) = to_button(button) else {
            return 0;
        };
        let cell = self.cell(x, y);
        let out = self.tools.press(self.tank.world_mut(), button, cell);
        self.outcome(out)
    }

    /// The pointer moved to canvas pixel `(x, y)`, with or without a button
    /// down. A stroke covers every cell on the way from the last sample.
    /// Returns how many cells (or bodies) changed.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = pointerMove))]
    pub fn pointer_move(&mut self, x: f64, y: f64) -> u32 {
        let cell = self.cell(x, y);
        let out = self.tools.moved(self.tank.world_mut(), cell);
        self.outcome(out)
    }

    /// `button` was released, ending the stroke it started (letting a
    /// netted creature go, or lifting it out above the water). Returns 1 if
    /// a creature was lifted out, else 0.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = pointerUp))]
    pub fn pointer_up(&mut self, button: u32) -> u32 {
        let Some(button) = to_button(button) else {
            return 0;
        };
        let out = self.tools.release(self.tank.world_mut(), button);
        self.dirty |= !matches!(out, Outcome::None);
        self.sync_cursor();
        u32::from(matches!(out, Outcome::Removed(_)))
    }

    /// The pointer left the canvas (or was lost): ends any stroke and hides
    /// the brush outline.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = pointerLeave))]
    pub fn pointer_leave(&mut self) {
        // May let go of a netted creature; conservatively a change.
        self.dirty |= self.tools.pointer().is_some();
        self.tools.leave(self.tank.world_mut());
        self.sync_cursor();
    }

    /// Inspects the cell at canvas pixel `(x, y)` whatever tool is selected
    /// (touch long-press, SPEC §3.5), keeping the selection. Returns what
    /// it found, which [`WebTank::take_inspection`] then also returns once.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = inspectAt))]
    pub fn inspect_at(&mut self, x: f64, y: f64) -> String {
        let keep = self
            .palette
            .iter()
            .copied()
            .find(|&item| self.tools.is_selected(item));
        self.tools.select(PaletteItem::Tool(Tool::Inspect));
        let cell = self.cell(x, y);
        let out = self
            .tools
            .press(self.tank.world_mut(), Button::Primary, cell);
        self.tools.release(self.tank.world_mut(), Button::Primary);
        self.outcome(out);
        if let Some(item) = keep {
            self.tools.select(item);
        }
        self.sync_cursor();
        self.inspection.clone()
    }

    /// What inspect found since the last call, once: `"cell <material>
    /// <column> <row>"`, `"plant <species> <id> <age seconds> <health %>
    /// <cells> <limit> <column> <row>"` (limit as
    /// [`desktank_sim::Limit::key`]), `"creature <species> <id> <material>
    /// <column> <row> <stage> <age days> <health %> <energy %> <female|male>
    /// <morph|-> <carrying days|-> <name…>"` (the name, last, may hold
    /// spaces or be empty; see [`desktank_sim::CreatureCard`]), or `""` if
    /// nothing new.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = takeInspection))]
    pub fn take_inspection(&mut self) -> String {
        core::mem::take(&mut self.inspection)
    }

    /// Names creature `id` `name` (cleaned as [`desktank_sim::clean_name`]
    /// does; blank removes its name) and returns the name it now has, `""`
    /// for none. Names are saved with the tank.
    ///
    /// # Errors
    ///
    /// Returns a message if there is no such creature (it died or was
    /// netted out).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = nameCreature))]
    pub fn name_creature(&mut self, id: u32, name: &str) -> Result<String, String> {
        let kept = self
            .tank
            .world_mut()
            .set_creature_name(CreatureId(id), name)
            .map(str::to_owned)
            .ok_or_else(|| "That creature has left the tank".to_owned())?;
        self.dirty = true;
        Ok(kept)
    }

    /// Creature `id`'s name (`""` for none), or `None` (`undefined`) if
    /// there is no such creature (tests).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = creatureName))]
    #[must_use]
    pub fn creature_name(&self, id: u32) -> Option<String> {
        self.tank
            .world()
            .creature(CreatureId(id))
            .map(|c| c.name().unwrap_or("").to_owned())
    }

    /// The material name at cell `(x, y)`, or `""` outside the grid (tests).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = materialAt))]
    #[must_use]
    pub fn material_at(&self, x: u32, y: u32) -> String {
        let world = self.tank.world();
        world
            .cell(x as usize, y as usize)
            .and_then(|c| world.materials().get(c.material))
            .map_or_else(String::new, |m| m.name.clone())
    }

    /// Number of glued (static) rigid bodies (tests).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = gluedBodies))]
    #[must_use]
    pub fn glued_bodies(&self) -> u32 {
        count(self.tank.world().bodies().filter(|b| b.anchored).count())
    }

    /// Number of cells of the material named `name` (0 if unknown), for
    /// tests.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = materialCount))]
    #[must_use]
    pub fn material_count(&self, name: &str) -> u32 {
        let world = self.tank.world();
        world
            .materials()
            .id(name)
            .map_or(0, |id| count(world.material_counts()[usize::from(id.0)]))
    }

    /// Sprinkles the default food (flake food) just below the water surface
    /// in a dozen pinches across the tank (the tray's Feed, SPEC §2.5).
    /// Returns how many cells changed.
    pub fn feed(&mut self) -> u32 {
        let changed = cheats::feed(self.tank.world_mut());
        self.dirty |= changed > 0;
        self.edited(changed);
        count(changed)
    }

    /// Takes one key press (a `KeyboardEvent.key`, without auto-repeats)
    /// and returns `true` when it completes the Konami code, so the page
    /// should open its cheat field. The page has keyboard focus, so no
    /// chord is needed. A modifier on its own (Shift for a capital B) is
    /// ignored.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = cheatKey))]
    pub fn cheat_key(&mut self, key: &str) -> bool {
        let key = match key {
            "ArrowUp" => cheats::CheatKey::Up,
            "ArrowDown" => cheats::CheatKey::Down,
            "ArrowLeft" => cheats::CheatKey::Left,
            "ArrowRight" => cheats::CheatKey::Right,
            "b" | "B" => cheats::CheatKey::B,
            "a" | "A" => cheats::CheatKey::A,
            "Shift" | "Control" | "Alt" | "Meta" | "AltGraph" | "CapsLock" | "Fn" => {
                return false;
            }
            _ => cheats::CheatKey::Other,
        };
        self.konami.key(key, false)
    }

    /// Runs a line typed in the cheat field ([`cheats::run`]) and returns
    /// what it did.
    ///
    /// # Errors
    ///
    /// Returns a short message for an empty line, an unknown cheat or one
    /// that can't run; the tank is unchanged.
    pub fn cheat(&mut self, line: &str) -> Result<String, String> {
        let out = cheats::run(self.tank.world_mut(), line).map_err(|e| e.to_string())?;
        self.dirty |= out.world_changed;
        self.edited(out.cells_changed);
        Ok(out.message)
    }

    /// Tints the tank by a field (`"nutrient"`, `"co2"`, `"light"`), or
    /// with `""` turns the tint off.
    ///
    /// # Errors
    ///
    /// Returns a message if the field name is unknown.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = setFieldOverlay))]
    pub fn set_field_overlay(&mut self, field: &str) -> Result<(), String> {
        let field = match field {
            "" => None,
            name => {
                Some(FieldKind::from_name(name).ok_or_else(|| format!("unknown field {name:?}"))?)
            }
        };
        self.tank.set_field_overlay(field);
        Ok(())
    }

    /// The field overlay shown (`""` if none).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = fieldOverlay))]
    #[must_use]
    pub fn field_overlay(&self) -> String {
        self.tank
            .field_overlay()
            .map_or_else(String::new, |f| f.name().to_owned())
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
        self.dirty = true;
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
        let (cx, cy) = self.layout().cell_at(x, y);
        let clamp = |v: i64| i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX });
        vec![clamp(cx), clamp(cy)]
    }

    /// The canvas pixel at the centre of cell `(x, y)`, as `[x, y]` (tests
    /// aim pointer events with it).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = pixelAt))]
    #[must_use]
    pub fn pixel_at(&self, x: i32, y: i32) -> Vec<f64> {
        let l = self.layout();
        let s = f64::from(l.scale);
        #[allow(clippy::cast_precision_loss)] // offsets are canvas pixels
        let at = |c: i32, o: i64| o as f64 + (f64::from(c) + 0.5) * s;
        vec![at(x, l.offset.0), at(y, l.offset.1)]
    }

    /// Advances to `now_ms` (`performance.now()`): runs the simulation ticks
    /// due, re-applies a held stroke once per tick, and draws a frame if one
    /// is due and allowed. Returns [`FRAME_DREW`], [`FRAME_WANTED`] and
    /// [`FRAME_LOST`] flags, with the simulation ticks run shifted by
    /// [`FRAME_TICKS_SHIFT`].
    pub fn frame(&mut self, now_ms: f64) -> u32 {
        let now = Duration::from_secs_f64(now_ms.max(0.0) / 1000.0);
        if !self.running {
            // Holding the clock: tick() re-anchors and runs nothing.
            self.tank.pause();
        }
        let request = self.tank.tick(now);
        // Holding still keeps pouring, once per tick run.
        for _ in 0..request.sim_ticks {
            self.tools.hold(self.tank.world_mut());
        }
        self.wake_at = request.wake_at;
        self.dirty |= request.sim_ticks > 0;
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

    /// When the last [`WebTank::frame`] wants to be called again, in the
    /// page's milliseconds: the next tick, or the next frame the cap allows.
    /// Nothing happens before then unless input arrives, so the page can
    /// sleep on a timer instead of waking every vsync.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = wakeAt))]
    #[must_use]
    pub fn wake_at(&self) -> f64 {
        self.wake_at.as_secs_f64() * 1000.0
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
        let sand = world
            .materials()
            .id("sand")
            .ok_or_else(|| "no sand material".to_owned())?;
        let (groups, palette) = Tools::palette_groups(&world)
            .into_iter()
            .flat_map(|(g, items)| items.into_iter().map(move |i| (g, i)))
            .unzip();
        Ok(Self {
            tank: Tank::new(world, TankConfig::default()),
            tools: Tools::new(sand),
            avoid_cursor: false,
            palette,
            groups,
            running: true,
            inspection: String::new(),
            wake_at: Duration::ZERO,
            size: (size.0.max(1), size.1.max(1)),
            frames: 0,
            dirty: false,
            konami: cheats::Konami::new(),
            #[cfg(target_arch = "wasm32")]
            graphics: None,
        })
    }

    /// The wrapped tank.
    #[must_use]
    pub fn tank(&self) -> &Tank {
        &self.tank
    }

    fn layout(&self) -> Layout {
        self.tank.layout(self.size)
    }

    /// The cell under canvas pixel `(x, y)`, clamped to just outside the
    /// grid (one brush radius), so a captured drag far off the canvas
    /// doesn't interpolate a long run of stamps.
    fn cell(&self, x: f64, y: f64) -> (i64, i64) {
        let (cx, cy) = self.layout().cell_at(x, y);
        let pad = i64::from(self.tools.radius()) + 1;
        let clamp = |v: i64, len: usize| {
            let len = i64::try_from(len).unwrap_or(i64::MAX);
            v.clamp(-pad, len - 1 + pad)
        };
        let world = self.tank.world();
        (clamp(cx, world.width()), clamp(cy, world.height()))
    }

    /// Kind and name of a palette item, as [`WebTank::palette`] lists it.
    fn describe(&self, item: PaletteItem) -> (&'static str, String) {
        let table = self.tank.world().materials();
        match item {
            PaletteItem::Material(m) => (
                "material",
                table.get(m).map_or_else(String::new, |m| m.name.clone()),
            ),
            PaletteItem::Tool(Tool::Place(p)) => ("device", p.material_name().to_owned()),
            PaletteItem::Tool(Tool::AddCreature(s)) => (
                "creature",
                self.tank
                    .world()
                    .species()
                    .get(s)
                    .map_or_else(String::new, |s| s.name.clone()),
            ),
            PaletteItem::Tool(Tool::Plant) => ("plant", "plants".to_owned()),
            PaletteItem::Tool(t) => (
                "tool",
                match t {
                    Tool::Scoop => "scoop",
                    Tool::Glue => "glue",
                    Tool::Net => "net",
                    Tool::Inspect => "inspect",
                    Tool::Pour | Tool::Place(_) | Tool::AddCreature(_) | Tool::Plant => "pour",
                }
                .to_owned(),
            ),
        }
    }

    /// Records a tool outcome: shows edits made while paused, keeps the
    /// inspection and the brush outline. Returns how many cells or bodies
    /// changed.
    fn outcome(&mut self, out: Outcome) -> u32 {
        let changed = match out {
            Outcome::Painted(n) | Outcome::Glued(n) => n,
            Outcome::Inspected(found) => {
                self.inspection = match found {
                    Inspection::Cell { x, y, material } => {
                        let table = self.tank.world().materials();
                        let name = table.get(material).map_or("?", |m| m.name.as_str());
                        format!("cell {name} {x} {y}")
                    }
                    Inspection::Plant { id, x, y, .. } => {
                        let world = self.tank.world();
                        world.plant(id).map_or_else(String::new, |p| {
                            let name = world
                                .plant_species()
                                .get(p.species)
                                .map_or("?", |s| s.name.as_str());
                            format!(
                                "plant {name} {} {:.0} {:.0} {} {} {x} {y}",
                                id.0,
                                p.age.floor(),
                                p.health * 100.0,
                                p.size(),
                                p.limit.key()
                            )
                        })
                    }
                    Inspection::Creature {
                        id,
                        species: _,
                        x,
                        y,
                        material,
                    } => {
                        let world = self.tank.world();
                        let over = world
                            .materials()
                            .get(material)
                            .map_or("?", |m| m.name.as_str());
                        world
                            .creature_card(id)
                            .map_or_else(String::new, |card| creature_line(&card, over, x, y))
                    }
                };
                0
            }
            // Creatures aren't cells: their frames follow the sprites.
            Outcome::Caught(_) | Outcome::Dropped(_) | Outcome::Removed(_) | Outcome::Added(_) => {
                self.dirty = true;
                0
            }
            Outcome::None => 0,
        };
        self.dirty |= changed > 0;
        self.edited(changed);
        self.sync_cursor();
        count(changed)
    }

    /// While paused, an edit re-uploads the grid so it shows without a tick.
    fn edited(&mut self, changed: usize) {
        if changed > 0 && !self.running {
            self.tank.cells_edited();
        }
    }

    /// Gives the tank the brush outline to draw (or none).
    fn sync_cursor(&mut self) {
        let pointer = self.tools.pointer().filter(|_| self.avoid_cursor);
        self.tank.world_mut().set_creature_cursor(pointer);
        let cursor = self.tools.preview().map(|(x, y, radius)| BrushPreview {
            cell: (x, y),
            radius,
        });
        self.tank.set_cursor(cursor);
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

/// A creature's inspect line (see [`WebTank::take_inspection`]).
fn creature_line(card: &CreatureCard, over: &str, x: usize, y: usize) -> String {
    let dash = |v: Option<String>| v.unwrap_or_else(|| "-".to_owned());
    format!(
        "creature {} {} {over} {x} {y} {} {} {:.0} {:.0} {} {} {} {}",
        card.species,
        card.id.0,
        card.stage.name(),
        card.age_days,
        card.health * 100.0,
        card.energy * 100.0,
        if card.female { "female" } else { "male" },
        dash(card.morph.clone()),
        dash(card.carrying_days.map(|d| d.to_string())),
        card.name.as_deref().unwrap_or(""),
    )
}

fn to_button(code: u32) -> Option<Button> {
    match code {
        BUTTON_PRIMARY => Some(Button::Primary),
        BUTTON_SECONDARY => Some(Button::Secondary),
        _ => None,
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
mod tests;
