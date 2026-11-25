//! Plant individuals (SPEC §2.7, D18, T6b): every plant is one entry in a
//! table, of one species, with its own age, health, stored energy and size.
//!
//! - **Table.** As for rigid bodies, each plant part cell's `state` holds
//!   its plant id (1–65535) and the world keeps a `BTreeMap` from id to
//!   [`Plant`], so every pass runs in id order. Part counts and bounds are
//!   kept exact: growth adds to them, and any edit that removes or adds a
//!   plant's cells marks it for a tidy (at the start and end of each tick),
//!   which rescans its bounds. So after every tick the table matches the
//!   grid.
//! - **Splitting.** A tidy splits a plant into its 4-connected pieces. The
//!   piece holding the base cell keeps the id (if the base is gone, the
//!   first piece does, and its lowest stem cell becomes the base); every
//!   other piece becomes a new plant of the species with the parent id set
//!   (a fragment) if the species propagates by fragments and the piece
//!   holds a stem, or else dead plant matter.
//! - **Random ticks** visit plant cells within the budget (SPEC §2.1): a
//!   visit samples the light (leaves and stems), nutrient and CO₂ at the
//!   cell into its plant's entry, and a mat cell that no longer touches
//!   what it clings to may fall off. A seed or cutting resting where its
//!   species roots, with every factor in range, roots and becomes a plant.
//! - **The plant update** visits each plant about every
//!   [`UPDATE_SECONDS`] of world time, at most [`MAX_UPDATES_PER_TICK`]
//!   plants per tick. It turns the samples into the species' response,
//!   health and stored energy, and decides growth (new cells from the
//!   species' form), dropping the oldest leaves, and death. Only those
//!   cell changes wake a chunk: health, energy and the per-plant tint live
//!   in the table.
//!
//! - **Floaters** (T6c part 2, D22) are buoyant bodies: each floater plant
//!   also has a raft ([`raft`]) holding its sub-cell offset and velocity,
//!   and its cells move together each tick as a rigid body's do (T4c), held
//!   at the surface by its floating leaves and carried by the current.
//!   A ball (marimo) has a raft too, but sinks: it rests on what is below
//!   it and the current only nudges it.
//!
//! The table is plain data (numbers, ids and a rectangle), so the save
//! format (T8) can store it as it is; the samples and update bookkeeping
//! can be reset on load.

mod raft;

pub(super) use raft::Raft;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use rand_core::Rng as _;

use super::creatures::DEPTH_BANDS;
use super::{World, WorldError, variant};
use crate::ecology::{DIRS, keep_per_tick, powu};
use crate::field::FieldKind;
use crate::grid::{Cell, Rect};
use crate::hash::Fnv64;
use crate::material::{MaterialId, MaterialTable};
use crate::plants::{
    BallForm, COUNTED_PARTS, EpiphyteForm, Factor, FloaterForm, Form, Limit, Part, PlantSpecies,
    PlantSpeciesId, PlantSpeciesTable, Propagule, RosetteForm, plant_position, plant_variant,
    propagule_age, propagule_species, propagule_state,
};
use crate::rng::splitmix64;
use crate::save::{SaveError, v1, v8};

/// Most plants a tank holds by default (SPEC §2.7 *Cap*), set from
/// measurement in T6b: a settled tank with 528 plants ticks in about
/// 11 µs, and every plant update stays bounded per tick, so the cap guards
/// the table and the tint texture rather than the tick.
pub const MAX_PLANTS: usize = 2048;
/// World seconds between two updates of one plant (while the tank holds at
/// most [`MAX_UPDATES_PER_TICK`] × 100 plants).
pub const UPDATE_SECONDS: u32 = 5;
/// Most plants updated in one tick.
pub const MAX_UPDATES_PER_TICK: usize = 16;

/// Ticks between two updates of one plant.
const UPDATE_TICKS: usize = (UPDATE_SECONDS * super::TICKS_PER_SECOND) as usize;
/// Most cells' worth of energy a plant stores.
const MAX_ENERGY: f32 = 16.0;
/// Most cells one update grows (96 a minute at one update per 5 s, above
/// the fastest species rate allowed).
const MAX_GROWTH_PER_UPDATE: usize = 8;
/// Below this health a plant drops its oldest leaves.
const DROP_BELOW: f32 = 0.5;
/// Updates a plant with nowhere to grow skips before looking again.
const BLOCKED_UPDATES: u8 = 6;
/// Chance per second that a resting seed or cutting roots (T6's values).
const SEED_ROOTING: f32 = 0.02;
const CUTTING_ROOTING: f32 = 0.5;
/// One rot step per 36 unsuitable ticks on average. At 20 Hz, 250 steps
/// take about 7.5 minutes, leaving margin within a 10-minute tank day.
const CUTTING_ROT_STEP_TICKS: u16 = 36;
const CUTTING_ROT_STEPS: u8 = 250;
/// Chance per second that a mat cell touching nothing it clings to falls
/// off (T6's moss).
const MAT_FALLING: f32 = 0.05;
/// Stem-form growth weights per site: a stem tip, a leaf, a root (T6's
/// per-cell chances, 0.08, 0.03 and 0.02 per second).
const STEM_WEIGHT: u32 = 8;
const LEAF_WEIGHT: u32 = 3;
const ROOT_WEIGHT: u32 = 2;
/// Rosette and carpet growth weights per site (T6c): a blade's tip grows
/// like a stem's, a new blade or crown cell like a leaf, and a runner
/// creeps more slowly.
const BLADE_WEIGHT: u32 = 8;
const NEW_BLADE_WEIGHT: u32 = 3;
const CROWN_WEIGHT: u32 = 3;
const RUNNER_WEIGHT: u32 = 2;
/// Floater growth weights per site (T6c part 2): a new frond like a leaf,
/// a hanging root like a root, and a bud more slowly.
const FROND_WEIGHT: u32 = 3;
const HANGING_ROOT_WEIGHT: u32 = 2;
const BUD_WEIGHT: u32 = 1;
/// Epiphyte growth weights per site (T6c part 3): rhizome creeping along
/// rock/wood and leaves rising from it.
const RHIZOME_WEIGHT: u32 = 3;
const EPIPHYTE_LEAF_WEIGHT: u32 = 6;
/// Layered mat growth weights: spreading over what it clings to beats
/// building up outer layers.
const MAT_BASE_WEIGHT: u32 = 3;
const MAT_LAYER_WEIGHT: u32 = 1;
/// A leaf pad spreading from its petiole's tip, like a new blade.
const PAD_WEIGHT: u32 = 3;
/// How much further from its middle (in quarter cells², from the nearest)
/// a ball may grow a cell: a little, so it grows round but not in rings.
const BALL_SLACK: u32 = 2;
/// How much nearer (in quarter cells²) a ball's site counts for each of
/// its cells it touches.
const BALL_HOLD: u32 = 2;
/// Chance per maturity cycle (~35 s when topped out, or 5 s at `growth.size`)
/// that a healthy plant of a seeding species drops a seed into adjacent water.
const SEED_DROP_CHANCE: f64 = 0.05;

/// Minimum light level for a healthy leaf to pearl (T6d, SPEC §2.7).
pub const PEARL_LIGHT: f32 = 0.65;
/// Minimum CO₂ level for a healthy leaf to pearl (T6d, SPEC §2.7).
pub const PEARL_CO2: f32 = 0.6;
/// Minimum plant health for pearling (matches the vigorous threshold).
pub const PEARL_HEALTH: f32 = 0.8;
/// Hard cap on oxygen bubbles released by pearling per second of world time
/// across the whole tank (1 bubble every `TICKS_PER_SECOND / PEARL_MAX_PER_SECOND` ticks).
pub const PEARL_MAX_PER_SECOND: u32 = 4;
/// Ticks between eligible pearling ticks (5 ticks = 0.25 s at 20 Hz).
const PEARL_TICK_STRIDE: u64 = (super::TICKS_PER_SECOND / PEARL_MAX_PER_SECOND) as u64;
/// Per-cell pearling rate (per second on eligible ticks) when thresholds are met.
const PEARL_RATE: f32 = 0.25;

/// A plant's id, as stored in its cells' `state` (never 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlantId(pub u16);

/// A plant as the world reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlantInfo {
    /// Its id.
    pub id: PlantId,
    /// Its species.
    pub species: PlantSpeciesId,
    /// Age in seconds of world time.
    pub age: f64,
    /// Health from 0 (dying) to 1.
    pub health: f32,
    /// Stored energy, in cells' worth of growth.
    pub energy: f32,
    /// Cells per part: roots (with runners), stems and leaves
    /// ([`Part::count_slot`]).
    pub parts: [u32; COUNTED_PARTS],
    /// The cell it grows from: where it rooted (or its lowest stem cell).
    pub base: (usize, usize),
    /// The rectangle covering its cells.
    pub bounds: Rect,
    /// The plant it was cut from (a fragment) or whose runner rooted it.
    pub parent: Option<PlantId>,
    /// 0 for a plant grown from a seed or cutting, one more than its
    /// parent's for a fragment or a plant rooted by its runner.
    pub generation: u16,
    /// What held it back most at its last update.
    pub limit: Limit,
    /// How far (0–1) it has turned its species' colouration colour (T6d):
    /// it moves towards what the light and nutrient at its updates call
    /// for, at [`COLOUR_PER_SECOND`].
    pub colour: f32,
    /// Clockwise ball rotation, in 1/256 turns. Zero for other forms.
    pub roll: u8,
}

impl PlantInfo {
    /// Its size in cells.
    #[must_use]
    pub fn size(&self) -> u32 {
        self.parts.iter().sum()
    }
}

/// One plant in the table.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Plant {
    species: PlantSpeciesId,
    /// Tick it was created.
    born: u64,
    health: f32,
    energy: f32,
    parts: [u32; COUNTED_PARTS],
    /// Cell index of its base.
    base: usize,
    bounds: Rect,
    /// 0 if none.
    parent: u16,
    generation: u16,
    /// Seconds spent outside its ranges.
    stress: f32,
    limit: Limit,
    /// Colouration, 0–1 (T6d).
    colour: f32,
    // Bookkeeping, rebuilt on load.
    last_update: u64,
    /// Sums of light, nutrient and CO₂ samples since the last update, and
    /// how many of each.
    sums: [f32; 3],
    counts: [u32; 3],
    /// Edits touched its cells: tidy it.
    pending: bool,
    /// Updates to skip before looking for somewhere to grow again.
    idle: u8,
}

/// Health and colouration steps that change a plant's look: a declining
/// or colouring plant asks for a new plant texel (and a frame) every 1/32,
/// not every 1/255.
pub const TINT_STEPS: f32 = 32.0;

/// How fast a plant's colouration follows its light and nutrient, per
/// second of world time: two minutes from green to fully coloured, so the
/// noise in its samples doesn't flicker it.
pub const COLOUR_PER_SECOND: f32 = 1.0 / 120.0;

impl Plant {
    fn tint_key(&self) -> (u8, u8) {
        (tint_step(self.health), tint_step(self.colour))
    }
}

/// The step (0..=[`TINT_STEPS`]) a 0–1 value is drawn at: the renderer
/// draws a plant's health and colouration at these steps, and the plants
/// version changes when one does.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped
#[must_use]
pub fn tint_step(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * TINT_STEPS).round() as u8
}

/// Plant state owned by the world.
#[derive(Clone, Debug)]
pub(super) struct Plants {
    species: Arc<PlantSpeciesTable>,
    table: BTreeMap<u16, Plant>,
    /// Each floater's raft, by plant id (T6c part 2): a floater plant has
    /// one, no other plant does.
    rafts: BTreeMap<u16, Raft>,
    /// Per material id: 1 + the part index, or 0.
    part_of: Vec<u8>,
    /// Material per part, in [`Part::ALL`] order (runners, floating leaves
    /// and rhizomes are optional).
    parts: [Option<MaterialId>; 6],
    seed: Option<MaterialId>,
    cutting: Option<MaterialId>,
    dead: Option<MaterialId>,
    water: Option<MaterialId>,
    air_bubble: Option<MaterialId>,
    next_id: u16,
    /// Some plant is marked for a tidy.
    any_pending: bool,
    /// Next id the plant update starts from.
    cursor: u16,
    cap: usize,
    /// Bumped when a plant is added or removed or its tint changes.
    version: u64,
    keep_seed: f64,
    keep_cutting: f64,
    keep_falling: f64,
    keep_pearl: f64,
    /// Whether a leaf has already released a pearling bubble on this tick
    /// (reset at the start of every tick; caps pearling to
    /// [`PEARL_MAX_PER_SECOND`] bubbles/s with no persistent state).
    pub(super) pearled_this_tick: bool,
    /// Pearling bubbles released since this world was built or loaded (a
    /// diagnostic counter, not saved and not part of the state hash).
    pearls: u64,
}

impl Plants {
    pub(super) fn new(materials: &MaterialTable) -> Result<Self, WorldError> {
        let species = PlantSpeciesTable::builtin_for(materials)
            .map_err(|e| WorldError::PlantSpecies(e.to_string()))?;
        let parts = Part::ALL.map(|p| materials.id(p.material_name()));
        let dead = materials.id("dead_plant");
        let water = materials.id("water");
        // Without root, stem and leaf, dead plant matter and water there
        // are no plants (a test's own material set); without runners no
        // runners grow.
        let enabled =
            parts[..COUNTED_PARTS].iter().all(Option::is_some) && dead.is_some() && water.is_some();
        let mut part_of = vec![0u8; 256];
        if enabled {
            for (k, m) in parts.iter().enumerate() {
                if let Some(m) = m {
                    #[allow(clippy::cast_possible_truncation)] // 5 parts
                    let tag = k as u8 + 1;
                    part_of[usize::from(m.0)] = tag;
                }
            }
        }
        let on = |name| enabled.then(|| materials.id(name)).flatten();
        Ok(Self {
            species: Arc::new(species),
            table: BTreeMap::new(),
            rafts: BTreeMap::new(),
            part_of,
            parts,
            seed: on("seed"),
            cutting: on("cutting"),
            dead,
            water,
            air_bubble: on("air_bubble"),
            next_id: 1,
            any_pending: false,
            cursor: 0,
            cap: MAX_PLANTS,
            version: 0,
            keep_seed: keep_per_tick(SEED_ROOTING),
            keep_cutting: keep_per_tick(CUTTING_ROOTING),
            keep_falling: keep_per_tick(MAT_FALLING),
            keep_pearl: keep_per_tick(PEARL_RATE),
            pearled_this_tick: false,
            pearls: 0,
        })
    }

    /// The part a material is, if it is a plant part.
    #[inline]
    pub(super) fn part(&self, m: MaterialId) -> Option<Part> {
        match self.part_of[usize::from(m.0)] {
            0 => None,
            k => Part::ALL.get(usize::from(k) - 1).copied(),
        }
    }

    fn is_propagule(&self, m: MaterialId) -> bool {
        Some(m) == self.seed || Some(m) == self.cutting
    }

    /// Materials whose cells the random ticks must visit.
    pub(super) fn living_materials(&self) -> Vec<MaterialId> {
        let mut out: Vec<MaterialId> = self.parts.iter().flatten().copied().collect();
        out.extend(self.seed);
        out.extend(self.cutting);
        if self.part_of.iter().all(|&p| p == 0) {
            out.clear();
        }
        out
    }

    fn material(&self, part: Part) -> Option<MaterialId> {
        self.parts[part.index()]
    }

    /// Dead plant matter, if plants are enabled.
    pub(super) fn dead(&self) -> Option<MaterialId> {
        self.dead
    }
}

impl World {
    /// The plant species.
    #[must_use]
    pub fn plant_species(&self) -> &PlantSpeciesTable {
        &self.plants.species
    }

    /// All plants, by id, as of the last tick (edits made since apply at
    /// the next tick).
    pub fn plants(&self) -> impl Iterator<Item = PlantInfo> + '_ {
        self.plants
            .table
            .iter()
            .map(|(&id, p)| self.plant_info(id, p))
    }

    /// The plant with this id, if it exists.
    #[must_use]
    pub fn plant(&self, id: PlantId) -> Option<PlantInfo> {
        self.plants
            .table
            .get(&id.0)
            .map(|p| self.plant_info(id.0, p))
    }

    /// The plant the cell at `(x, y)` belongs to, if any.
    #[must_use]
    pub fn plant_at(&self, x: usize, y: usize) -> Option<PlantId> {
        let c = self.cell(x, y)?;
        let part = self.plants.part(c.material).is_some();
        (part && self.plants.table.contains_key(&c.state)).then_some(PlantId(c.state))
    }

    /// The depth band (0 at the front glass to [`DEPTH_BANDS`] − 1 at the
    /// back) of the plant part cell at `(x, y)`, or `None` for any other
    /// cell (D21). Creatures collide with it only from a band within
    /// [`BAND_REACH`](crate::BAND_REACH) of it.
    #[must_use]
    pub fn plant_band(&self, x: usize, y: usize) -> Option<u8> {
        (x < self.width && y < self.height).then(|| self.plant_band_at(y * self.width + x))?
    }

    /// [`World::plant_band`] for every cell of `r` (clipped to the grid),
    /// row by row into `out`, replacing its contents; 0 for cells that
    /// aren't plant parts. Returns whether any cell was a plant part. For
    /// the renderer, which draws plant cells in front of creatures behind
    /// them (D21).
    pub fn plant_bands(&self, r: Rect, out: &mut Vec<u8>) -> bool {
        self.pack_bands(r, out, false)
    }

    /// Packs the renderer's depth texture, row by row in the clipped
    /// rectangle. Plant cells hold their band. Rigid cells hold their
    /// front band plus [`DEPTH_BANDS`], and all other cells hold zero.
    /// Returns whether the rectangle contains a plant or rigid cell.
    pub fn render_bands(&self, r: Rect, out: &mut Vec<u8>) -> bool {
        self.pack_bands(r, out, true)
    }

    fn pack_bands(&self, r: Rect, out: &mut Vec<u8>, rigid: bool) -> bool {
        out.clear();
        let (x1, y1) = (r.x1.min(self.width), r.y1.min(self.height));
        let mut memo = None;
        let mut any = false;
        for y in r.y0.min(y1)..y1 {
            for i in y * self.width + r.x0.min(x1)..y * self.width + x1 {
                let band = if rigid && self.is_rigid(i) {
                    Some(DEPTH_BANDS + self.body_bands_at(i).0)
                } else {
                    self.plant_band_with(i, &mut memo)
                };
                any |= band.is_some();
                out.push(band.unwrap_or(0));
            }
        }
        any
    }

    /// [`World::plant_band`] by cell index. Bands aren't stored: they
    /// follow from the world seed, the plant id and the cell's place in
    /// the plant, so they cost no cell bits and need no save field. A
    /// plant's stem (a rosette's or carpet's crown, its roots and runners)
    /// keeps the plant's band; a stem plant's leaf (one per node and side)
    /// shifts it by up to one band, so the plant has depth; a rosette's or
    /// carpet's blades shift it by up to two, by column, so they fan
    /// across the bands; a mat's cells shift it by up to one each, so it
    /// is fuzzy; a floater lies in its own band.
    pub(super) fn plant_band_at(&self, i: usize) -> Option<u8> {
        self.plant_band_with(i, &mut None)
    }

    /// [`World::plant_band_at`], reusing the plant's [`PlantDepth`] in
    /// `memo` when the cell is of the same plant (a creature's box mostly
    /// meets one or two plants), and leaving its own there.
    pub(super) fn plant_band_with(&self, i: usize, memo: &mut Option<PlantDepth>) -> Option<u8> {
        let cell = self.cells[i];
        let part = self.plants.part(cell.material)?;
        let id = cell.state;
        let depth = match memo {
            Some(d) if d.id == id => *d,
            _ => *memo.insert(self.plant_depth(id)),
        };
        let (x, y) = (i % self.width, i / self.width);
        let (key, spread) = match (part, depth.shape) {
            (Part::Leaf, Shape::Stem { base_x }) => (((y as u64) << 1) | u64::from(x < base_x), 1),
            // Leaf pads lie across columns: a plant's pads share a band.
            (
                Part::Leaf,
                Shape::Blades {
                    pads_from: Some(len),
                },
            ) if plant_position(cell.variant) > len => (1, 2),
            (Part::Leaf, Shape::Blades { .. }) => ((x as u64) << 1, 2),
            (Part::Stem, Shape::Mat) => (i as u64, 1),
            _ => return u8::try_from(depth.base).ok(),
        };
        // Within ±2: exact.
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let shift = (self.band_mix(id, key + 1) % (2 * spread + 1)) as i32 - spread as i32;
        u8::try_from((depth.base + shift).clamp(0, i32::from(DEPTH_BANDS) - 1)).ok()
    }

    /// What plant `id`'s cell bands follow from: its own band and form.
    fn plant_depth(&self, id: u16) -> PlantDepth {
        // Below DEPTH_BANDS (8): exact.
        #[allow(clippy::cast_possible_truncation)]
        let base = (self.band_mix(id, 0) % u64::from(DEPTH_BANDS)) as i32;
        let p = self.plants.table.get(&id);
        let shape = match p
            .and_then(|p| self.plants.species.get(p.species))
            .map(|s| &s.form)
        {
            Some(Form::Stem(_)) => Shape::Stem {
                base_x: p.map_or(0, |p| p.base % self.width),
            },
            Some(Form::Rosette(f) | Form::Carpet(f)) => Shape::Blades {
                pads_from: (f.pad > 0).then_some(f.blade_length),
            },
            Some(Form::Epiphyte(_)) => Shape::Blades { pads_from: None },
            Some(Form::Mat(_)) => Shape::Mat,
            Some(Form::Floater(_)) => Shape::Floater,
            Some(Form::Ball(_)) | None => Shape::Plain,
        };
        PlantDepth { id, base, shape }
    }

    fn band_mix(&self, id: u16, key: u64) -> u64 {
        splitmix64(self.seed ^ splitmix64((u64::from(id) << 40) ^ key))
    }

    /// Pearling bubbles released since this world was built or loaded (a
    /// diagnostic counter: not saved and not part of the state hash).
    #[must_use]
    pub fn pearls_released(&self) -> u64 {
        self.plants.pearls
    }

    /// Number of plants.
    #[must_use]
    pub fn plant_count(&self) -> usize {
        self.plants.table.len()
    }

    /// Plant cells in the tank, from the table.
    #[must_use]
    pub fn plant_cells(&self) -> usize {
        self.plants
            .table
            .values()
            .map(|p| p.parts.iter().map(|&n| n as usize).sum::<usize>())
            .sum()
    }

    /// Bumped whenever a plant is added or removed, its health tint
    /// changes or its base moves (renderers re-upload the plant texture,
    /// and the bands of a plant whose base moved, on a change).
    #[must_use]
    pub fn plants_version(&self) -> u64 {
        self.plants.version
    }

    /// Most plants the tank may hold (default [`MAX_PLANTS`]).
    #[must_use]
    pub fn plant_cap(&self) -> usize {
        self.plants.cap
    }

    /// Sets the plant cap, within 1..=65535. Plants already over it stay;
    /// no new ones root, and cut pieces become dead plant matter.
    pub fn set_plant_cap(&mut self, cap: usize) {
        self.plants.cap = cap.clamp(1, usize::from(u16::MAX));
    }

    /// Pours `species`' `propagule` (its material,
    /// [`Propagule::material_name`]) into the fluid cells of a disc, like
    /// [`World::paint`] with [`super::Brush::Pour`]. Returns how many cells
    /// changed.
    ///
    /// # Errors
    ///
    /// Returns an error if the species is unknown, doesn't grow from
    /// `propagule`, or the table lacks its material.
    pub fn paint_plant(
        &mut self,
        cx: i64,
        cy: i64,
        radius: u32,
        species: PlantSpeciesId,
        propagule: Propagule,
    ) -> Result<usize, WorldError> {
        let s = self
            .plants
            .species
            .get(species)
            .ok_or(WorldError::UnknownPlantSpecies(species))?;
        if !s.grows_from(propagule) {
            return Err(WorldError::PlantPropagule(species, propagule));
        }
        let name = propagule.material_name();
        let material = self
            .materials
            .id(name)
            .ok_or(WorldError::MissingMaterial(name))?;
        let mut changed = 0;
        for (x, y) in self.disc(cx, cy, radius) {
            let c = self.cells[y * self.width + x];
            let same = c.material == material && propagule_species(c.state) == species.0;
            if same || !self.props[usize::from(c.material.0)].class.is_fluid() {
                continue;
            }
            self.write_cell(x, y, material, &mut None);
            self.cells[y * self.width + x].state = propagule_state(species.0, 0);
            changed += 1;
        }
        Ok(changed)
    }

    fn plant_info(&self, id: u16, p: &Plant) -> PlantInfo {
        #[allow(clippy::cast_precision_loss)] // exact below 2^53 ticks
        let age = self.tick.saturating_sub(p.born) as f64 / f64::from(super::TICKS_PER_SECOND);
        PlantInfo {
            id: PlantId(id),
            species: p.species,
            age,
            health: p.health,
            energy: p.energy,
            parts: p.parts,
            base: (p.base % self.width, p.base / self.width),
            bounds: p.bounds,
            parent: (p.parent != 0).then_some(PlantId(p.parent)),
            generation: p.generation,
            limit: p.limit,
            colour: p.colour,
            roll: self
                .plants
                .species
                .get(p.species)
                .filter(|s| s.form.ball().is_some())
                .and_then(|_| self.plants.rafts.get(&id))
                .map_or(0, |r| (r.roll >> 8) as u8),
        }
    }

    /// The plant table as saved (T8), bookkeeping included, so a loaded
    /// tank updates its plants exactly when the saved one would have.
    pub(super) fn save_plants(&self) -> v1::Plants {
        let plants = &self.plants;
        v1::Plants {
            next_id: plants.next_id,
            cursor: plants.cursor,
            any_pending: plants.any_pending,
            cap: u32::try_from(plants.cap).unwrap_or(u32::MAX),
            table: plants
                .table
                .iter()
                .map(|(&id, p)| v1::Plant {
                    id,
                    species: p.species.0,
                    born: p.born,
                    health: p.health,
                    energy: p.energy,
                    parts: p.parts,
                    #[allow(clippy::cast_possible_truncation)] // cells < MAX_SIDE²
                    base: p.base as u32,
                    bounds: super::persist::rect_v1(p.bounds),
                    parent: p.parent,
                    generation: p.generation,
                    stress: p.stress,
                    limit: limit_v1(p.limit),
                    last_update: p.last_update,
                    sums: p.sums,
                    counts: p.counts,
                    pending: p.pending,
                    idle: p.idle,
                })
                .collect(),
        }
    }

    /// Restores the plant table from a save.
    pub(super) fn load_plants(&mut self, saved: v1::Plants) -> Result<(), SaveError> {
        let mut table = BTreeMap::new();
        for p in saved.table {
            let invalid = |what: &str| SaveError::Invalid(format!("plant {}: {what}", p.id));
            if p.id == 0 || table.contains_key(&p.id) {
                return Err(invalid("id 0 or repeated"));
            }
            let species = PlantSpeciesId(p.species);
            if self.plants.species.get(species).is_none() {
                return Err(invalid("unknown species"));
            }
            let finite = [p.health, p.energy, p.stress]
                .into_iter()
                .chain(p.sums)
                .all(f32::is_finite);
            if !finite {
                return Err(invalid("not a number"));
            }
            table.insert(
                p.id,
                Plant {
                    species,
                    born: p.born,
                    health: p.health,
                    energy: p.energy,
                    parts: p.parts,
                    base: self.cell_index(p.base, "a plant's base")?,
                    bounds: super::persist::rect_from_v1(p.bounds, self.width, self.height)?,
                    parent: p.parent,
                    generation: p.generation,
                    stress: p.stress,
                    limit: limit_from_v1(p.limit),
                    colour: 0.0,
                    last_update: p.last_update,
                    sums: p.sums,
                    counts: p.counts,
                    pending: p.pending,
                    idle: p.idle,
                },
            );
        }
        let plants = &mut self.plants;
        plants.table = table;
        plants.next_id = saved.next_id.max(1);
        plants.cursor = saved.cursor;
        plants.any_pending = saved.any_pending;
        plants.cap = saved.cap as usize;
        plants.version += 1;
        Ok(())
    }

    /// The colouration of every plant that has any (format version 8).
    pub(super) fn save_plant_colours(&self) -> Vec<v8::PlantColour> {
        self.plants
            .table
            .iter()
            .filter(|(_, p)| p.colour > 0.0)
            .map(|(&plant, p)| v8::PlantColour {
                plant,
                colour: p.colour,
            })
            .collect()
    }

    /// Restores the plants' colouration, after [`World::load_plants`].
    pub(super) fn load_plant_colours(
        &mut self,
        saved: &[v8::PlantColour],
    ) -> Result<(), SaveError> {
        let mut last = None;
        for c in saved {
            let invalid =
                |what: &str| SaveError::Invalid(format!("plant {} colour: {what}", c.plant));
            if !(0.0..=1.0).contains(&c.colour) {
                return Err(invalid("not within 0..=1"));
            }
            // Saved in id order, each plant once.
            if last.is_some_and(|l| l >= c.plant) {
                return Err(invalid("out of order or repeated"));
            }
            last = Some(c.plant);
            let p = self
                .plants
                .table
                .get_mut(&c.plant)
                .ok_or_else(|| invalid("no such plant"))?;
            p.colour = c.colour;
        }
        Ok(())
    }

    pub(super) fn hash_plants(&self, h: &mut Fnv64) {
        for (&id, p) in &self.plants.table {
            h.write(&id.to_le_bytes());
            h.write(&[p.species.0]);
            h.write(&p.born.to_le_bytes());
            for v in [p.health, p.energy, p.stress] {
                h.write(&v.to_bits().to_le_bytes());
            }
            // An uncoloured plant hashes as before format version 8.
            if p.colour != 0.0 {
                h.write(&p.colour.to_bits().to_le_bytes());
            }
            for n in p.parts {
                h.write(&n.to_le_bytes());
            }
            h.write(&(p.base as u64).to_le_bytes());
            h.write(&p.parent.to_le_bytes());
            h.write(&p.generation.to_le_bytes());
            h.write(&p.last_update.to_le_bytes());
            h.write(&[p.idle]);
        }
        h.write(&self.plants.next_id.to_le_bytes());
        h.write(&self.plants.cursor.to_le_bytes());
        self.hash_rafts(h);
    }

    pub(super) fn palatable_plant(&self, id: u16) -> bool {
        self.plants
            .table
            .get(&id)
            .and_then(|p| self.plants.species.get(p.species))
            .is_some_and(|s| s.palatable)
    }

    /// A bite costs five percent of the individual’s maximum health.
    pub(super) fn graze_plant(&mut self, id: u16) {
        if let Some(plant) = self.plants.table.get_mut(&id) {
            plant.health = (plant.health - 0.05).max(0.0);
            self.plants.version += 1;
        }
    }

    /// Records that a cell of plant `id` was overwritten.
    pub(super) fn plant_removed(&mut self, id: u16) {
        if let Some(p) = self.plants.table.get_mut(&id) {
            p.pending = true;
            self.plants.any_pending = true;
        }
    }

    /// The plant a new part cell at index `i` written by an edit joins:
    /// `group` if it exists, else a new plant of the default species
    /// (recorded in `group`). Returns 0 if there is no room for one.
    pub(super) fn join_plant(&mut self, group: &mut Option<u16>, part: Part, i: usize) -> u16 {
        let id = match *group {
            Some(id) if self.plants.table.contains_key(&id) => id,
            _ => {
                // A floating leaf starts a floater (the first floater
                // species); a rhizome starts an epiphyte; any other part
                // the default species.
                let species = &self.plants.species;
                let pick = match part {
                    Part::Float => species.iter().find(|s| s.form.floater().is_some()),
                    Part::Rhizome => species.iter().find(|s| s.form.epiphyte().is_some()),
                    _ => species.default_species(),
                };
                let Some(species) = pick.map(|s| s.id) else {
                    return 0;
                };
                let id = self.new_plant(species, i, None);
                *group = (id != 0).then_some(id);
                id
            }
        };
        let (x, y) = (i % self.width, i / self.width);
        if let Some(p) = self.plants.table.get_mut(&id) {
            p.parts[part.count_slot()] += 1;
            p.bounds = p.bounds.union(point(x, y));
            p.pending = true;
            self.plants.any_pending = true;
        }
        id
    }

    /// A new plant of `species` based at cell `base`, or a fragment (or
    /// runner-rooted child) of `parent`. Returns its id, or 0 if the tank is
    /// at its cap.
    fn new_plant(&mut self, species: PlantSpeciesId, base: usize, parent: Option<u16>) -> u16 {
        let plants = &mut self.plants;
        if plants.table.len() >= plants.cap.min(usize::from(u16::MAX)) {
            return 0;
        }
        // A fragment keeps its parent's health and colouration.
        let (health, stress, generation, colour) = parent
            .and_then(|id| plants.table.get(&id))
            .map_or((1.0, 0.0, 0, 0.0), |p| {
                (p.health, p.stress, p.generation.saturating_add(1), p.colour)
            });
        for _ in 0..u16::MAX {
            let id = plants.next_id;
            plants.next_id = plants.next_id.checked_add(1).unwrap_or(1);
            if let std::collections::btree_map::Entry::Vacant(e) = plants.table.entry(id) {
                e.insert(Plant {
                    species,
                    born: self.tick,
                    health,
                    energy: 0.0,
                    parts: [0; COUNTED_PARTS],
                    base,
                    bounds: point(base % self.width, base / self.width),
                    parent: parent.unwrap_or(0),
                    generation,
                    stress,
                    limit: Limit::None,
                    colour,
                    last_update: self.tick,
                    sums: [0.0; 3],
                    counts: [0; 3],
                    pending: false,
                    idle: 0,
                });
                let drifts = plants.species.get(species).is_some_and(|s| s.form.drifts());
                if drifts {
                    // A bud or fragment starts moving as its parent does.
                    let raft = parent
                        .and_then(|p| plants.rafts.get(&p))
                        .map_or_else(Raft::new, |r| r.child());
                    plants.rafts.insert(id, raft);
                }
                plants.version += 1;
                return id;
            }
        }
        0
    }

    /// Tidies every plant edits touched (see the module docs).
    pub(super) fn tidy_plants(&mut self) {
        if !self.plants.any_pending {
            return;
        }
        self.plants.any_pending = false;
        let pending: Vec<u16> = self
            .plants
            .table
            .iter()
            .filter(|(_, p)| p.pending)
            .map(|(&id, _)| id)
            .collect();
        for id in pending {
            self.tidy_plant(id);
        }
    }

    /// The cells of plant `id` within `r`, in index order.
    fn plant_members(&self, id: u16, r: Rect) -> Vec<usize> {
        let mut out = Vec::new();
        for y in r.y0..r.y1 {
            let row = y * self.width;
            for (k, c) in self.cells[row + r.x0..row + r.x1].iter().enumerate() {
                if c.state == id && self.plants.part(c.material).is_some() {
                    out.push(row + r.x0 + k);
                }
            }
        }
        out
    }

    #[inline]
    fn is_plant_member(&self, i: usize, id: u16) -> bool {
        let c = self.cells[i];
        c.state == id && self.plants.part(c.material).is_some()
    }

    fn tidy_plant(&mut self, id: u16) {
        let Some(p) = self.plants.table.get(&id) else {
            return;
        };
        let (species, base, bounds) = (p.species, p.base, self.clip_to_grid(p.bounds));
        let members = self.plant_members(id, bounds);
        if members.is_empty() {
            self.plants.table.remove(&id);
            self.plants.rafts.remove(&id);
            self.plants.version += 1;
            return;
        }
        let pieces = self.plant_pieces(&members, id, bounds);
        let floater = self
            .plants
            .species
            .get(species)
            .is_some_and(|s| s.form.floater().is_some());
        let epiphyte = self
            .plants
            .species
            .get(species)
            .is_some_and(|s| s.form.epiphyte().is_some());
        // A floater lives on only in a piece with a floating leaf to hold
        // it up; an epiphyte only in a piece with a rhizome.
        let float = self.plants.material(Part::Float);
        let rhizome = self.plants.material(Part::Rhizome);
        let viable = |piece: &[usize]| {
            if floater {
                piece
                    .iter()
                    .any(|&i| float.is_some() && Some(self.cells[i].material) == float)
            } else if epiphyte {
                piece
                    .iter()
                    .any(|&i| rhizome.is_some() && Some(self.cells[i].material) == rhizome)
            } else {
                true
            }
        };
        let keeper = pieces
            .iter()
            .position(|piece| piece.binary_search(&base).is_ok() && viable(piece))
            .or_else(|| {
                (floater || epiphyte)
                    .then(|| pieces.iter().position(|piece| viable(piece)))
                    .flatten()
            })
            .unwrap_or(0);
        if (floater || epiphyte) && !viable(&pieces[keeper]) {
            self.kill_plant(id);
            return;
        }
        let fragments = self
            .plants
            .species
            .get(species)
            .is_some_and(|s| s.propagation.fragments);
        for (k, piece) in pieces.iter().enumerate() {
            if k == keeper {
                continue;
            }
            // A piece lives on if it holds the core part for its form: a
            // floating leaf for a floater, a rhizome for an epiphyte, or a
            // stem for the rest.
            let core = if floater {
                float
            } else if epiphyte {
                rhizome
            } else {
                self.plants.material(Part::Stem)
            };
            let has_core = piece
                .iter()
                .any(|&i| core.is_some() && Some(self.cells[i].material) == core);
            let new = if fragments && has_core {
                self.new_plant(species, self.piece_base(piece), Some(id))
            } else {
                0
            };
            if new == 0 {
                self.make_dead(piece);
            } else {
                for &i in piece {
                    self.cells[i].state = new;
                    self.mark_changed(i % self.width, i / self.width);
                }
                self.recount_plant(new, piece);
            }
        }
        let piece = &pieces[keeper];
        let new_base = if piece.binary_search(&base).is_ok() {
            base
        } else {
            self.piece_base(piece)
        };
        self.recount_plant(id, piece);
        if new_base != base {
            // A stem plant's leaf bands follow its base column (D21), so
            // renderers re-read its bands.
            self.plants.version += 1;
        }
        if let Some(p) = self.plants.table.get_mut(&id) {
            p.base = new_base;
            p.pending = false;
            p.idle = 0;
        }
        self.wake_raft(id);
    }

    /// Sets plant `id`'s part counts and bounds from its cells.
    fn recount_plant(&mut self, id: u16, cells: &[usize]) {
        let mut parts = [0u32; COUNTED_PARTS];
        let mut bounds = Rect::EMPTY;
        for &i in cells {
            if let Some(part) = self.plants.part(self.cells[i].material) {
                parts[part.count_slot()] += 1;
            }
            bounds = bounds.union(point(i % self.width, i / self.width));
        }
        if let Some(p) = self.plants.table.get_mut(&id) {
            p.parts = parts;
            p.bounds = bounds;
        }
    }

    /// Where a piece grows from: its lowest stem cell, else its first
    /// rhizome or floating leaf, else its first cell.
    fn piece_base(&self, piece: &[usize]) -> usize {
        let stem = self.plants.material(Part::Stem);
        let rhizome = self.plants.material(Part::Rhizome);
        let float = self.plants.material(Part::Float);
        piece
            .iter()
            .rev()
            .find(|&&i| Some(self.cells[i].material) == stem)
            .or_else(|| {
                piece
                    .iter()
                    .find(|&&i| rhizome.is_some() && Some(self.cells[i].material) == rhizome)
            })
            .or_else(|| {
                piece
                    .iter()
                    .find(|&&i| float.is_some() && Some(self.cells[i].material) == float)
            })
            .or(piece.first())
            .copied()
            .unwrap_or(0)
    }

    /// Turns `cells` into dead plant matter.
    fn make_dead(&mut self, cells: &[usize]) {
        let Some(dead) = self.plants.dead else {
            return;
        };
        for &i in cells {
            self.cells[i] = Cell::new(dead, variant(self.seed, self.tick, i));
            self.mark_changed(i % self.width, i / self.width);
        }
    }

    /// The 4-connected pieces of plant `id`'s `cells` (sorted, within `r`),
    /// each sorted, in order of their lowest cell.
    fn plant_pieces(&self, cells: &[usize], id: u16, r: Rect) -> Vec<Vec<usize>> {
        let bw = r.x1 - r.x0;
        let local = |i: usize| (i / self.width - r.y0) * bw + (i % self.width - r.x0);
        let mut seen = vec![false; r.area()];
        let mut pieces = Vec::new();
        let mut stack = Vec::new();
        for &start in cells {
            if seen[local(start)] {
                continue;
            }
            seen[local(start)] = true;
            stack.push(start);
            let mut piece = Vec::new();
            while let Some(i) = stack.pop() {
                piece.push(i);
                let (x, y) = (i % self.width, i / self.width);
                let neighbours = [
                    (x > r.x0).then(|| i - 1),
                    (x + 1 < r.x1).then(|| i + 1),
                    (y > r.y0).then(|| i - self.width),
                    (y + 1 < r.y1).then(|| i + self.width),
                ];
                for j in neighbours.into_iter().flatten() {
                    if !seen[local(j)] && self.is_plant_member(j, id) {
                        seen[local(j)] = true;
                        stack.push(j);
                    }
                }
            }
            piece.sort_unstable();
            pieces.push(piece);
        }
        pieces
    }

    /// `r` clipped to the grid.
    fn clip_to_grid(&self, r: Rect) -> Rect {
        r.intersect(Rect {
            x0: 0,
            y0: 0,
            x1: self.width,
            y1: self.height,
        })
    }

    /// One random tick of the cell at `i` if it is a plant part, seed or
    /// cutting (see the module docs), acting for `k` ticks. Returns whether
    /// it was one.
    pub(super) fn visit_plant(&mut self, i: usize, cell: Cell, k: u16) -> bool {
        match self.plants.part(cell.material) {
            Some(part) => {
                self.visit_plant_cell(i, cell, part, k);
                true
            }
            None => self.visit_propagule(i, cell, k),
        }
    }

    /// One random tick of a plant part cell.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn visit_plant_cell(&mut self, i: usize, cell: Cell, part: Part, k: u16) {
        let Some(p) = self.plants.table.get(&cell.state) else {
            return;
        };
        let health = p.health;
        let (x, y) = (i % self.width, i / self.width);
        let species = Arc::clone(&self.plants.species);
        let s = species.get(p.species);
        let unanchored = match s.map(|s| &s.form) {
            Some(Form::Mat(mat)) => {
                !self.mat_supported(i, cell.state, plant_position(cell.variant), &mat.on)
            }
            Some(Form::Epiphyte(e)) if part == Part::Rhizome => !self.touches_any(i, &e.on),
            _ => false,
        };
        if unanchored {
            if self.life.chance(1.0 - powu(self.plants.keep_falling, k))
                && let Some(dead) = self.plants.dead
            {
                self.write_cell(x, y, dead, &mut None);
            }
            return;
        }
        let light = part
            .sees_light()
            .then(|| self.field(FieldKind::Light).at_cell(x, y));
        // A root feeder takes its nutrients through its roots and runners.
        let root_feeder = s.is_some_and(|s| s.growth.root_feeder);
        let nutrient = (!root_feeder || !part.sees_light())
            .then(|| self.field(FieldKind::Nutrient).at_cell(x, y));
        let co2 = self.field(FieldKind::Co2).at_cell(x, y);
        if let Some(p) = self.plants.table.get_mut(&cell.state) {
            for (k, v) in [light, nutrient, Some(co2)].into_iter().enumerate() {
                if let Some(v) = v {
                    p.sums[k] += v;
                    p.counts[k] += 1;
                }
            }
        }
        // Pearling (SPEC §2.7, T6d): under strong light and high CO₂, a
        // healthy plant releases small oxygen bubbles (`air_bubble`) from
        // submerged leaf cells into water above, capped at
        // `PEARL_MAX_PER_SECOND` bubbles per second across the tank.
        if part == Part::Leaf
            && !self.plants.pearled_this_tick
            && self.tick.is_multiple_of(PEARL_TICK_STRIDE)
            && health >= PEARL_HEALTH
            && light.is_some_and(|l| l >= PEARL_LIGHT)
            && co2 >= PEARL_CO2
            && let Some(bubble) = self.plants.air_bubble
            && let Some(above) = self.at(x, y, 0, -1)
            && Some(self.cells[above].material) == self.plants.water
            && self.life.chance(1.0 - powu(self.plants.keep_pearl, k))
        {
            self.write_cell(x, y - 1, bubble, &mut None);
            self.plants.pearled_this_tick = true;
            self.plants.pearls += 1;
        }
    }

    /// One random tick of a seed or cutting: it may root (see the module
    /// docs). Returns whether the material is a propagule.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn visit_propagule(&mut self, i: usize, cell: Cell, k: u16) -> bool {
        if !self.plants.is_propagule(cell.material) {
            return false;
        }
        let species = Arc::clone(&self.plants.species);
        let Some(s) = species.for_state(cell.state) else {
            return true;
        };
        let (x, y) = (i % self.width, i / self.width);
        let below = self.at(x, y, 0, 1);
        let ready = match (&s.form, s.form.substrates()) {
            (Form::Mat(m), _) => self.touches_any(i, &m.on),
            (Form::Epiphyte(e), _) => {
                self.plants.material(Part::Rhizome).is_some() && self.touches_any(i, &e.on)
            }
            // A ball starts where its piece comes to rest on something.
            (Form::Ball(_), _) => below.is_some_and(|j| !self.is_fluid(self.cells[j].material)),
            // A floater's plantlet opens in open water (water above it)
            // and rises to the surface.
            (Form::Floater(_), _) => {
                self.plants.material(Part::Float).is_some()
                    && self
                        .at(x, y, 0, -1)
                        .is_some_and(|j| Some(self.cells[j].material) == self.plants.water)
            }
            (_, Some(subs)) => below.is_some_and(|j| subs.contains(&self.cells[j].material)),
            (_, None) => false,
        };
        if !ready || self.response_at(s, x, y).0 <= 0.0 {
            self.age_cutting(i, cell, k);
            return true;
        }
        let mut clustered = false;
        if let Form::Ball(form) = &s.form
            && Some(cell.material) == self.plants.cutting
        {
            if self.coalesce_ball_fragment(i, s, form) {
                return true;
            }
            // Cuttings in a falling pile can support each other. Defer to
            // a cutting directly below when it can itself root, so only the
            // bottom layer starts a new ball without stalling behind an
            // unrootable support cell.
            if below.is_some_and(|j| {
                Some(self.cells[j].material) == self.plants.cutting
                    && self.response_at(s, x, y + 1).0 > 0.0
            }) {
                return true;
            }
            clustered = DIRS.into_iter().any(|(dx, dy)| {
                self.at(x, y, dx, dy)
                    .is_some_and(|j| self.is_ball_fragment(j, s.id))
            });
        }
        let keep = if Some(cell.material) == self.plants.seed {
            self.plants.keep_seed
        } else {
            self.plants.keep_cutting
        };
        // A resting cluster roots on contact before its grains spread.
        if !clustered && !self.life.chance(1.0 - powu(keep, k)) {
            return true;
        }
        let id = self.new_plant(s.id, i, None);
        if id == 0 {
            return true;
        }
        match &s.form {
            Form::Stem(_) => {
                self.put_part(i, Part::Stem, id, 1);
                if let Some(j) = below {
                    self.put_part(j, Part::Root, id, 1);
                }
            }
            // A rosette's or carpet's crown sits on the substrate (position
            // 0), its first root below.
            Form::Rosette(_) | Form::Carpet(_) => {
                self.put_part(i, Part::Stem, id, 0);
                if let Some(j) = below {
                    self.put_part(j, Part::Root, id, 1);
                }
            }
            Form::Mat(_) => self.put_part(i, Part::Stem, id, 0),
            Form::Ball(form) => {
                self.put_part(i, Part::Stem, id, 0);
                if clustered {
                    self.collect_ball_fragments(i, id, s, form);
                }
            }
            Form::Floater(_) => self.put_part(i, Part::Float, id, 0),
            Form::Epiphyte(_) => self.put_part(i, Part::Rhizome, id, 1),
        }
        true
    }

    fn age_cutting(&mut self, index: usize, cell: Cell, ticks: u16) {
        if Some(cell.material) != self.plants.cutting {
            return;
        }
        let Some(dead) = self.plants.dead else {
            return;
        };
        let mut steps = ticks / CUTTING_ROT_STEP_TICKS;
        let remainder = ticks % CUTTING_ROT_STEP_TICKS;
        // Round fractional steps without advancing the shared ecology RNG.
        if remainder != 0 {
            let hash = splitmix64(
                self.seed
                    ^ self.tick.wrapping_mul(0x9e37_79b9_7f4a_7c15)
                    ^ (index as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9),
            );
            steps += u16::from(hash % u64::from(CUTTING_ROT_STEP_TICKS) < u64::from(remainder));
        }
        let age = u16::from(propagule_age(cell.state)) + steps;
        if age >= u16::from(CUTTING_ROT_STEPS) {
            self.write_cell(index % self.width, index / self.width, dead, &mut None);
        } else if steps != 0 {
            // Age is invisible. Only a material change wakes the chunk.
            #[allow(clippy::cast_possible_truncation)] // below CUTTING_ROT_STEPS
            let age = age as u8;
            self.cells[index].state = propagule_state(propagule_species(cell.state), age);
        }
    }

    /// Joins a neighbouring ball or waits for it to reach this fragment
    /// through the connected cuttings. Returns false if a new ball can root.
    fn coalesce_ball_fragment(
        &mut self,
        i: usize,
        species: &PlantSpecies,
        form: &BallForm,
    ) -> bool {
        let span = 2 * usize::from(form.radius) + 1;
        let neighbours = |i| DIRS.map(|(dx, dy)| self.at(i % self.width, i / self.width, dx, dy));
        let accepting_ball = |j: usize| {
            let cell = self.cells[j];
            (self.plants.part(cell.material) == Some(Part::Stem))
                .then(|| self.ball_fragment_position(cell.state, i, species, form))
                .flatten()
                .map(|pos| (cell.state, pos))
        };
        if let Some((id, pos)) = neighbours(i).into_iter().flatten().find_map(accepting_ball) {
            self.put_part(i, Part::Stem, id, pos);
            return true;
        }

        let cutting = |j| self.is_ball_fragment(j, species.id);
        if !neighbours(i).into_iter().flatten().any(cutting) {
            return false;
        }
        let mut seen = BTreeSet::from([i]);
        let mut queue = VecDeque::from([i]);
        while let Some(j) = queue.pop_front() {
            for k in neighbours(j).into_iter().flatten() {
                if accepting_ball(k).is_some() {
                    return true;
                }
                if seen.len() < span * span && cutting(k) && seen.insert(k) {
                    queue.push_back(k);
                }
            }
        }
        false
    }

    fn is_ball_fragment(&self, i: usize, species: PlantSpeciesId) -> bool {
        let c = self.cells[i];
        Some(c.material) == self.plants.cutting && propagule_species(c.state) == species.0
    }

    fn ball_fragment_position(
        &self,
        id: u16,
        i: usize,
        species: &PlantSpecies,
        form: &BallForm,
    ) -> Option<u8> {
        let p = self.plants.table.get(&id)?;
        let (x, y) = (i % self.width, i / self.width);
        let span = 2 * usize::from(form.radius) + 1;
        let bounds = p.bounds.union(point(x, y));
        if p.species != species.id
            || p.parts.iter().sum::<u32>() >= species.growth.size
            || bounds.x1 - bounds.x0 > span
            || bounds.y1 - bounds.y0 > span
        {
            return None;
        }
        let dx = x.abs_diff(usize::midpoint(p.bounds.x0, p.bounds.x1 - 1));
        let dy = y.abs_diff(usize::midpoint(p.bounds.y0, p.bounds.y1 - 1));
        let distance_sq = dx * dx + dy * dy;
        let distance = distance_sq.isqrt();
        let rounded = distance + usize::from(distance_sq - distance * distance > distance);
        Some(u8::try_from(rounded).unwrap_or(u8::MAX).min(form.radius))
    }

    /// Capture connected fragments before powder motion scatters them
    /// across the bed. Each added cell stays within the ball's size limits.
    fn collect_ball_fragments(
        &mut self,
        i: usize,
        id: u16,
        species: &PlantSpecies,
        form: &BallForm,
    ) {
        let mut queue = VecDeque::from([i]);
        while let Some(j) = queue.pop_front() {
            let (x, y) = (j % self.width, j / self.width);
            for (dx, dy) in DIRS {
                if let Some(k) = self.at(x, y, dx, dy)
                    && self.is_ball_fragment(k, species.id)
                    && let Some(position) = self.ball_fragment_position(id, k, species, form)
                {
                    self.put_part(k, Part::Stem, id, position);
                    queue.push_back(k);
                }
            }
        }
    }

    /// The species' response to the fields at `(x, y)`.
    fn response_at(&self, s: &PlantSpecies, x: usize, y: usize) -> (f32, Limit) {
        let v = Factor::ALL.map(|f| {
            self.field(match f {
                Factor::Light => FieldKind::Light,
                Factor::Nutrient => FieldKind::Nutrient,
                Factor::Co2 => FieldKind::Co2,
            })
            .at_cell(x, y)
        });
        s.prefer.response(v)
    }

    /// Whether the cell at `i` touches (4-neighbours) one of `materials`.
    fn touches_any(&self, i: usize, materials: &[MaterialId]) -> bool {
        let (x, y) = (i % self.width, i / self.width);
        DIRS.iter().any(|&(dx, dy)| {
            self.at(x, y, dx, dy)
                .is_some_and(|j| materials.contains(&self.cells[j].material))
        })
    }

    /// Whether a mat cell of plant `id` at `i` (layer `pos`) is held up:
    /// directly touching one of `on`, or for outer layers (`pos > 0`)
    /// touching a cell of the same plant in a nearer layer.
    fn mat_supported(&self, i: usize, id: u16, pos: u8, on: &[MaterialId]) -> bool {
        if self.touches_any(i, on) {
            return true;
        }
        if pos == 0 {
            return false;
        }
        let (x, y) = (i % self.width, i / self.width);
        DIRS.iter().any(|&(dx, dy)| {
            self.at(x, y, dx, dy).is_some_and(|j| {
                self.is_plant_member(j, id) && plant_position(self.cells[j].variant) < pos
            })
        })
    }

    /// Writes a grown cell of `part` for plant `id` at index `i`.
    fn put_part(&mut self, i: usize, part: Part, id: u16, position: u8) {
        let Some(material) = self.plants.material(part) else {
            return;
        };
        let (x, y) = (i % self.width, i / self.width);
        self.cells[i] = Cell {
            material,
            variant: plant_variant(variant(self.seed, self.tick, i), position),
            state: id,
        };
        self.mark_changed(x, y);
        if let Some(p) = self.plants.table.get_mut(&id) {
            p.parts[part.count_slot()] += 1;
            p.bounds = p.bounds.union(point(x, y));
        }
        // A floater that grew has a new weight to settle.
        self.wake_raft(id);
    }

    /// Updates the next few plants that are due (see the module docs): in
    /// id order from where the last tick stopped, each at most every
    /// [`UPDATE_SECONDS`], at most `⌈plants / 100⌉` (up to
    /// [`MAX_UPDATES_PER_TICK`]) per tick. The walk waits at the first plant
    /// that isn't due yet.
    pub(super) fn update_plants(&mut self) {
        let n = self.plants.table.len();
        if n == 0 {
            return;
        }
        let k = n.div_ceil(UPDATE_TICKS).min(MAX_UPDATES_PER_TICK);
        let (tick, cursor) = (self.tick, self.plants.cursor);
        let table = &self.plants.table;
        let mut ids = Vec::with_capacity(k);
        for (&id, p) in table.range(cursor..).chain(table.range(..cursor)) {
            if ids.len() == k || tick.saturating_sub(p.last_update) < UPDATE_TICKS as u64 {
                break;
            }
            ids.push(id);
        }
        if let Some(&last) = ids.last() {
            self.plants.cursor = last.checked_add(1).unwrap_or(0);
        }
        for id in ids {
            self.update_plant(id);
        }
    }

    pub(super) fn catch_up_plants(&mut self) {
        let ids: Vec<_> = self.plants.table.keys().copied().collect();
        for id in ids {
            self.update_plant(id);
        }
        self.tidy_plants();
    }

    #[allow(clippy::too_many_lines)] // samples, health, seeding and growth
    fn update_plant(&mut self, id: u16) {
        let species = Arc::clone(&self.plants.species);
        let tick = self.tick;
        let Some(p) = self.plants.table.get(&id) else {
            return;
        };
        let Some(s) = species.get(p.species) else {
            return;
        };
        let elapsed = tick.saturating_sub(p.last_update);
        if elapsed == 0 {
            return;
        }
        // Means of the samples since the last update; the fields at the
        // base where there are none.
        let (bx, by) = (p.base % self.width, p.base / self.width);
        let at_base = Factor::ALL.map(|f| {
            self.field(match f {
                Factor::Light => FieldKind::Light,
                Factor::Nutrient => FieldKind::Nutrient,
                Factor::Co2 => FieldKind::Co2,
            })
            .at_cell(bx, by)
        });
        #[allow(clippy::cast_precision_loss)] // sample counts are small
        let means: [f32; 3] = core::array::from_fn(|k| {
            if p.counts[k] > 0 {
                p.sums[k] / p.counts[k] as f32
            } else {
                at_base[k]
            }
        });
        let (response, limit) = s.prefer.response(means);
        // Roots over an under-gravel filter's plate are starved and drowned in
        // the water it pulls down through the bed (DV6): such a plant is under
        // stress however good its light, nutrients and CO₂, and doesn't grow.
        let rooted = matches!(s.form, Form::Stem(_) | Form::Rosette(_) | Form::Carpet(_));
        let hurt_by_ugf = rooted && self.is_over_floor_filter(bx, by);
        let response = if hurt_by_ugf { 0.0 } else { response };
        #[allow(clippy::cast_precision_loss)] // ≤ 2^32 ticks
        let dt = elapsed as f32 / super::TICKS_PER_SECOND as f32;
        #[allow(clippy::cast_precision_loss)] // exact below 2^53 ticks
        let age = tick.saturating_sub(p.born) as f32 / super::TICKS_PER_SECOND as f32;
        let h = s.hardiness;
        let Some(p) = self.plants.table.get_mut(&id) else {
            return;
        };
        let tint = p.tint_key();
        p.last_update = tick;
        p.sums = [0.0; 3];
        p.counts = [0; 3];
        p.limit = limit;
        let target = s.colouration.map_or(0.0, |c| c.target(means[0], means[1]));
        // Within a tint step of a target inside (0, 1) it holds, so noise in
        // the samples doesn't flip it across a step (and ask for a frame)
        // at every update; it always settles fully at 0 or 1.
        let gap = target - p.colour;
        let ends = target <= 0.0 || target >= 1.0;
        if ends || gap.abs() >= 1.0 / TINT_STEPS {
            let reach = COLOUR_PER_SECOND * dt;
            p.colour = (p.colour + gap.clamp(-reach, reach)).clamp(0.0, 1.0);
        }
        if response > 0.0 {
            p.stress = 0.0;
            p.health = (p.health + h.recovery * dt).min(1.0);
        } else {
            p.stress += dt;
            if p.stress > h.tolerance {
                p.health -= h.decline * dt;
            }
        }
        if s.growth.lifespan.is_some_and(|l| age > l) {
            p.health -= h.decline * dt;
        }
        p.health = p.health.max(0.0);
        let health = p.health;
        if p.tint_key() != tint {
            self.plants.version += 1;
        }
        if health <= 0.0 {
            self.kill_plant(id);
            return;
        }
        if health < DROP_BELOW {
            let leaves = p.parts[Part::Leaf.index()];
            if leaves > 0 {
                self.drop_leaves(id, (leaves as usize / 8).max(1));
            }
            return;
        }
        let size = p.parts.iter().sum::<u32>();
        // At its mature size a plant stops growing, except a rosette's or
        // carpet's runners, which carry on to root the next plant (each is
        // at most twice its spacing long), and a floater's buds.
        let mature = size >= s.growth.size;
        let spreads = (s.propagation.runners && s.form.rosette().is_some()) || s.propagation.buds;
        if mature && !spreads {
            if s.propagation.seeds && health >= 0.8 && response > 0.0 {
                self.maybe_drop_seed(id, s.id, s.growth.cost);
            }
            return;
        }
        p.energy = (p.energy + s.growth.rate / 60.0 * response * dt).min(MAX_ENERGY);
        if p.energy < 1.0 {
            return;
        }
        if p.idle > 0 {
            p.idle -= 1;
            return;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // ≤ MAX_ENERGY
        let want = (p.energy as usize)
            .min(MAX_GROWTH_PER_UPDATE)
            .min(s.growth.size.saturating_sub(size).max(u32::from(mature)) as usize);
        let grown = self.grow_plant(id, s, want, mature);
        if let Some(p) = self.plants.table.get_mut(&id) {
            #[allow(clippy::cast_precision_loss)] // ≤ 8
            let spent = grown as f32;
            p.energy -= spent;
            if grown == 0 {
                p.idle = BLOCKED_UPDATES;
            }
        }
        if grown == 0
            && size >= s.growth.size / 2
            && s.propagation.seeds
            && health >= 0.8
            && response > 0.0
        {
            self.maybe_drop_seed(id, s.id, s.growth.cost);
        }
    }

    /// Grows up to `want` cells of plant `id` where its form allows (only
    /// runners and buds if `spread_only`, at its mature size), taking the
    /// species' nutrient cost from the field at each. Returns how many it
    /// grew.
    fn grow_plant(&mut self, id: u16, s: &PlantSpecies, want: usize, spread_only: bool) -> usize {
        let Some(p) = self.plants.table.get(&id) else {
            return 0;
        };
        let members = self.plant_members(id, self.clip_to_grid(p.bounds));
        let mut sites = match &s.form {
            Form::Stem(_) => self.stem_sites(id, p.base, s, &members),
            Form::Mat(_) => self.mat_sites(s, &members),
            Form::Rosette(f) | Form::Carpet(f) => {
                self.rosette_sites(id, p.base, f, s.propagation.runners, &members)
            }
            Form::Floater(f) => self.floater_sites(id, p.base, f, spread_only, &members),
            Form::Epiphyte(f) => self.epiphyte_sites(id, p.base, f, &members),
            Form::Ball(f) => self.ball_sites(f, &members),
        };
        if spread_only {
            sites.retain(|s| s.part == Part::Runner || s.bud);
        }
        let mut grown = 0;
        while grown < want && !sites.is_empty() {
            let total: u32 = sites.iter().map(|s| s.weight).sum();
            let mut pick = self.life.rng.next_u32() % total;
            let mut chosen = 0;
            for (k, site) in sites.iter().enumerate() {
                if pick < site.weight {
                    chosen = k;
                    break;
                }
                pick -= site.weight;
            }
            let site = sites[chosen];
            // Sites next to the new cell may break a spacing rule now.
            sites.retain(|o| !near(o.at, site.at, self.width));
            if site.bud {
                // At the cap a bud doesn't open; the site is dropped.
                if !self.bud(id, s.id, site.at) {
                    continue;
                }
            } else {
                let sprouted = site.part == Part::Runner
                    && s.form.rosette().is_some_and(|f| site.position >= f.spacing)
                    && self.sprout(id, s.id, site.at);
                if !sprouted {
                    self.put_part(site.at, site.part, id, site.position);
                }
            }
            let (x, y) = (site.at % self.width, site.at / self.width);
            self.life
                .field_mut(FieldKind::Nutrient)
                .add_at_cell(x, y, -s.growth.cost);
            self.life.version += 1;
            grown += 1;
        }
        grown
    }

    /// Occasionally drops one seed of `species` into an open water cell
    /// beside plant `id`'s upper half (SPEC §2.7 *Propagation*), unless the
    /// patch is already crowded with plants of the same species.
    fn maybe_drop_seed(&mut self, id: u16, species: PlantSpeciesId, cost: f32) {
        if self.plants.table.len() >= self.plants.cap || !self.life.chance(SEED_DROP_CHANCE) {
            return;
        }
        let Some(seed) = self.plants.seed else {
            return;
        };
        let Some(water) = self.plants.water else {
            return;
        };
        let Some(p) = self.plants.table.get(&id) else {
            return;
        };
        let bx = p.base % self.width;
        let neighbours = self
            .plants
            .table
            .iter()
            .filter(|&(&oid, o)| {
                oid != id && o.species == species && (o.base % self.width).abs_diff(bx) <= 16
            })
            .count();
        if neighbours >= 1 {
            return;
        }
        // Also skip if an ungerminated seed of this species is already
        // sinking or waiting to germinate in the same window.
        let x0 = bx.saturating_sub(16);
        let x1 = (bx + 17).min(self.width);
        let sp_state = u16::from(species.0);
        for y in 0..self.height {
            let row = y * self.width;
            if self.cells[row + x0..row + x1]
                .iter()
                .any(|c| c.material == seed && c.state == sp_state)
            {
                return;
            }
        }
        let bounds = self.clip_to_grid(p.bounds);
        let mid_y = usize::midpoint(bounds.y0, bounds.y1);
        let members = self.plant_members(id, bounds);
        let mut spots = Vec::new();
        for &i in &members {
            let (x, y) = (i % self.width, i / self.width);
            if y > mid_y {
                continue;
            }
            for &(dx, dy) in &DIRS {
                if let Some(j) = self.at(x, y, dx, dy)
                    && self.cells[j].material == water
                {
                    spots.push(j);
                }
            }
        }
        if spots.is_empty() {
            return;
        }
        let at = spots[self.life.rng.next_u32() as usize % spots.len()];
        let (x, y) = (at % self.width, at / self.width);
        self.cells[at] = Cell {
            material: seed,
            variant: variant(self.seed, self.tick, at),
            state: u16::from(species.0),
        };
        self.mark_changed(x, y);
        self.life
            .field_mut(FieldKind::Nutrient)
            .add_at_cell(x, y, -cost);
        self.life.version += 1;
    }

    /// Where a stem-form plant can grow: up from every stem tip (and
    /// trailing sideways along the top water row once it hits air), a leaf
    /// or side shoot out from each node, a leaf on from its tip, and a root
    /// on from each root tip.
    #[allow(clippy::many_single_char_names, clippy::too_many_lines)] // cell maths
    fn stem_sites(&self, id: u16, base: usize, s: &PlantSpecies, members: &[usize]) -> Vec<Site> {
        let Form::Stem(f) = &s.form else {
            return Vec::new();
        };
        let water = self.plants.water;
        let is_water = |j: usize| Some(self.cells[j].material) == water;
        let is_air_like = |j: usize| {
            let m = self.cells[j].material;
            Some(m) != water && self.is_fluid(m)
        };
        let stem_mat = self.plants.material(Part::Stem);
        let is_own_stem = |j: usize| {
            let c = self.cells[j];
            Some(c.material) == stem_mat && c.state == id
        };
        let mut sites = Vec::new();
        for &i in members {
            let c = self.cells[i];
            let (x, y) = (i % self.width, i / self.width);
            let pos = plant_position(c.variant);
            match self.plants.part(c.material) {
                Some(Part::Stem) => {
                    let above = self.at(x, y, 0, -1);
                    let below = self.at(x, y, 0, 1);
                    let left = self.at(x, y, -1, 0);
                    let right = self.at(x, y, 1, 0);
                    let stem_above = above.is_some_and(is_own_stem);
                    let stem_below = below.is_some_and(is_own_stem);
                    let stem_left = left.is_some_and(is_own_stem);
                    let stem_right = right.is_some_and(is_own_stem);
                    if pos < f.height {
                        // Grow straight up from a stem tip if water is above
                        // and no other stem cell of this plant touches it
                        // (so a 1-cell branch stub steps out a second cell
                        // before rising).
                        if let Some(j) = above
                            && is_water(j)
                            && self.lone_part(j, i, id, Part::Stem)
                        {
                            sites.push(Site::new(j, Part::Stem, pos + 1, STEM_WEIGHT));
                        } else if above.is_some_and(is_air_like) {
                            // Topped out at the surface: trail sideways along
                            // the top water row.
                            for dx in [-1, 1] {
                                if let Some(j) = self.at(x, y, dx, 0)
                                    && is_water(j)
                                    && self
                                        .at(j % self.width, j / self.width, 0, -1)
                                        .is_some_and(is_air_like)
                                    && self.lone_part(j, i, id, Part::Stem)
                                {
                                    sites.push(Site::new(j, Part::Stem, pos + 1, STEM_WEIGHT));
                                }
                            }
                        } else if f.branching > 0.0
                            && !stem_above
                            && !stem_below
                            && (stem_left != stem_right)
                        {
                            // A 1-cell horizontal branch stub: step one more
                            // cell away from the parent stem so the rising
                            // shoot keeps a 1-cell gap from it.
                            let dx = if stem_left { 1 } else { -1 };
                            if let Some(j) = self.at(x, y, dx, 0)
                                && is_water(j)
                                && self.lone_part(j, i, id, Part::Stem)
                            {
                                sites.push(Site::new(j, Part::Stem, pos + 1, STEM_WEIGHT));
                            }
                        }
                    }
                    // Nodes on upright stem cells put out opposite leaves,
                    // or a side shoot with probability `f.branching`.
                    let upright = stem_below || stem_above || i == base;
                    if upright && pos >= f.internode && pos.is_multiple_of(f.internode) {
                        for dx in [-1, 1] {
                            let Some(j) = self.at(x, y, dx, 0).filter(|&j| is_water(j)) else {
                                continue;
                            };
                            let branch = f.branching > 0.0
                                && stem_above
                                && pos.saturating_add(3) < f.height
                                && self.lone_part(j, i, id, Part::Stem)
                                && self.at(x, y, dx * 2, 0).is_some_and(is_water)
                                && self.at(x, y, dx * 2, -1).is_some_and(is_water)
                                && !self.at(x, y, dx * 2, 1).is_some_and(is_own_stem)
                                && !self.at(x, y, dx * 3, 0).is_some_and(is_own_stem)
                                && self.branch_node(id, i, dx > 0, f.branching);
                            if branch {
                                sites.push(Site::new(j, Part::Stem, pos + 1, LEAF_WEIGHT));
                            } else if !self
                                .at(j % self.width, j / self.width, 0, 1)
                                .is_some_and(is_own_stem)
                            {
                                sites.push(Site::new(j, Part::Leaf, 1, LEAF_WEIGHT));
                            }
                        }
                    }
                }
                Some(Part::Leaf) if pos < f.leaf_length => {
                    let from_left = self.at(x, y, -1, 0).is_some_and(|l| {
                        let lc = self.cells[l];
                        lc.state == id
                            && ((pos == 1 && self.plants.part(lc.material) == Some(Part::Stem))
                                || (pos > 1
                                    && self.plants.part(lc.material) == Some(Part::Leaf)
                                    && plant_position(lc.variant) == pos - 1))
                    });
                    let dx = if from_left { 1 } else { -1 };
                    if let Some(j) = self.at(x, y, dx, 0).filter(|&j| is_water(j))
                        && !self
                            .at(j % self.width, j / self.width, 0, 1)
                            .is_some_and(is_own_stem)
                    {
                        sites.push(Site::new(j, Part::Leaf, pos + 1, LEAF_WEIGHT));
                    }
                }
                Some(Part::Root) if pos < f.root_depth => {
                    for (dx, dy) in [(0, 1), (-1, 0), (1, 0)] {
                        let Some(j) = self.at(x, y, dx, dy) else {
                            continue;
                        };
                        if f.substrates.contains(&self.cells[j].material)
                            && self.lone_root(j, i, id)
                        {
                            sites.push(Site::new(j, Part::Root, pos + 1, ROOT_WEIGHT));
                        }
                    }
                }
                _ => {}
            }
        }
        sites.sort_unstable_by_key(|s| (s.at, core::cmp::Reverse(s.weight), s.position));
        sites.dedup_by_key(|s| s.at);
        sites
    }

    /// Deterministic per-node branching decision: each node side of plant
    /// `id` at cell `i` is either a branching site (with probability
    /// `branching`) or a leaf site, so repeated updates never turn every
    /// node into a branch.
    fn branch_node(&self, id: u16, i: usize, right: bool, branching: f32) -> bool {
        let key = (u64::from(id) << 33) | ((i as u64) << 1) | u64::from(right);
        let h = splitmix64(self.seed ^ splitmix64(key));
        #[allow(clippy::cast_precision_loss)] // 24 bits fit f32
        let u = (h >> 40) as f32 / (1u32 << 24) as f32;
        u < branching
    }

    /// Whether growing `part` from `from` into `j` would touch no other
    /// cell of `part` belonging to plant `id`.
    fn lone_part(&self, j: usize, from: usize, id: u16, part: Part) -> bool {
        let mat = self.plants.material(part);
        let (x, y) = (j % self.width, j / self.width);
        DIRS.iter().all(|&(dx, dy)| {
            self.at(x, y, dx, dy).is_none_or(|e| {
                let c = self.cells[e];
                e == from || Some(c.material) != mat || c.state != id
            })
        })
    }

    /// Whether a root growing from `from` into `j` would touch no other
    /// root of plant `id` (T6's `alone`, by plant id).
    fn lone_root(&self, j: usize, from: usize, id: u16) -> bool {
        self.lone_part(j, from, id, Part::Root)
    }

    /// Where a rosette or carpet can grow (see [`RosetteForm`]): the crown
    /// out along the substrate, a new blade above each crown cell (and beside
    /// the full crown's ends), each blade's tip up (or a cell outwards every
    /// `lean` cells), roots down from the first root and on from the
    /// others, and, once every blade has started, a runner each side of the
    /// root under the crown and on from each runner's tip.
    #[allow(clippy::many_single_char_names, clippy::too_many_lines)] // one arm per part
    fn rosette_sites(
        &self,
        id: u16,
        base: usize,
        f: &RosetteForm,
        runners: bool,
        members: &[usize],
    ) -> Vec<Site> {
        let is_water = |j: usize| Some(self.cells[j].material) == self.plants.water;
        let substrate = |j: usize| f.substrates.contains(&self.cells[j].material);
        let (bx, by) = (base % self.width, base / self.width);
        // The crown grows rightwards from the base, so its centre is fixed:
        // twice it, to keep a two-cell crown's centre whole.
        #[allow(clippy::cast_possible_wrap)] // MAX_SIDE is small
        let centre2 = 2 * bx as isize + isize::from(f.crown) - 1;
        // Which side of the crown a cell is on (-1, 0, 1) and how many cells
        // out from its centre.
        #[allow(clippy::cast_possible_wrap)]
        let side = |x: usize| -> (isize, usize) {
            let off = 2 * x as isize - centre2;
            (off.signum(), off.unsigned_abs() / 2)
        };
        // The crown's extent along the base row and how many blades started.
        let (mut x1, mut crown, mut blades) = (bx, 0u8, 0u8);
        for &i in members {
            let c = self.cells[i];
            let (x, y) = (i % self.width, i / self.width);
            match self.plants.part(c.material) {
                Some(Part::Stem) if y == by && x >= bx => {
                    x1 = x1.max(x);
                    crown = crown.saturating_add(1);
                }
                Some(Part::Leaf) if plant_position(c.variant) == 1 => {
                    blades = blades.saturating_add(1);
                }
                _ => {}
            }
        }
        let new_blades = blades < f.blades;
        // How long a petiole grows (tiger lotus type), by plant and the
        // column it rose from: the one from the crown's first cell to
        // `blade_length`, so one reaches as high as it can, the others to
        // between 55% and all of it, so they stop at differing heights.
        let limit = |from: usize| -> u8 {
            if from == bx {
                return f.blade_length;
            }
            let k = splitmix64(self.seed ^ (u64::from(id) << 32) ^ from as u64);
            #[allow(clippy::cast_possible_truncation)] // ≤ blade_length
            let len = (u64::from(f.blade_length) * (55 + k % 46) / 100) as u8;
            len.max(2)
        };
        // A leaf pad cell at `j`: floating leaves at the surface.
        let pad_part = |j: usize| {
            let (x, y) = (j % self.width, j / self.width);
            let surface = self.at(x, y, 0, -1).is_some_and(|a| {
                let m = self.cells[a].material;
                Some(m) != self.plants.water && self.is_fluid(m)
            });
            if surface && self.plants.material(Part::Float).is_some() {
                Part::Float
            } else {
                Part::Leaf
            }
        };
        let mut sites = Vec::new();
        // A pad grows out sideways from `(x, y)` at `position`, and, under
        // water, puts a cell of its narrower upper row above it.
        let spread_pad = |sites: &mut Vec<Site>, x: usize, y: usize, position: u8| {
            for dx in [-1, 1] {
                if let Some(j) = self.at(x, y, dx, 0).filter(|&j| is_water(j)) {
                    sites.push(Site::new(j, pad_part(j), position, PAD_WEIGHT));
                }
            }
            if let Some(j) = self.at(x, y, 0, -1).filter(|&j| is_water(j))
                && pad_part(j) == Part::Leaf
            {
                sites.push(Site::new(j, Part::Leaf, f.pad_top(), PAD_WEIGHT));
            }
        };
        for &i in members {
            let c = self.cells[i];
            let (x, y) = (i % self.width, i / self.width);
            let pos = plant_position(c.variant);
            match self.plants.part(c.material) {
                Some(Part::Stem) if new_blades && y == by => {
                    if let Some(j) = self.at(x, y, 0, -1).filter(|&j| is_water(j)) {
                        sites.push(Site::new(j, Part::Leaf, 1, NEW_BLADE_WEIGHT));
                    }
                }
                // A leaf pad spreads on sideways from its outer cells (not
                // from its edge, nor its upper row).
                Some(Part::Leaf | Part::Float) if f.is_pad(pos) => {
                    if pos < f.blade_length + f.pad {
                        spread_pad(&mut sites, x, y, pos + 1);
                    }
                }
                // A petiole (with a pad) grows on from its tip, leaning
                // outwards by the column it rose from (every `lean` cells
                // from the crown, twice as often from beside it, and so
                // on), so the petioles fan apart; where it grows no further
                // (at its length, or under the air or something else) it
                // spreads its leaf pad.
                Some(Part::Leaf)
                    if f.pad > 0
                        && pos <= f.blade_length
                        && !self.petiole_goes_on(x, y, id, pos, f) =>
                {
                    let from = self.petiole_base(x, y, id, pos, f);
                    let (dir, out) = side(from);
                    let lean = (usize::from(f.lean) / (out + 1)).max(3);
                    let out = f.lean > 0
                        && dir != 0
                        && pos >= 2
                        && usize::from(pos - 2).is_multiple_of(lean);
                    let (dx, dy) = if out { (dir, 0) } else { (0, -1) };
                    let next = self.at(x, y, dx, dy).filter(|&j| is_water(j));
                    match next {
                        Some(j) if pos < limit(from) => {
                            sites.push(Site::new(j, Part::Leaf, pos + 1, BLADE_WEIGHT));
                        }
                        _ => spread_pad(&mut sites, x, y, f.blade_length + 1),
                    }
                }
                Some(Part::Leaf) if f.pad == 0 && pos < f.blade_length => {
                    // A blade's next cell follows from its position and
                    // column alone, so only its tip finds water there. It
                    // steps outwards at its second cell and every `lean`
                    // cells after, more often the further out it is (down
                    // to every third cell), so outer blades arch away.
                    let (dir, out) = side(x);
                    let lean = usize::from(f.lean).saturating_sub(out).max(3);
                    let out = f.lean > 0
                        && dir != 0
                        && pos >= 2
                        && usize::from(pos - 2).is_multiple_of(lean);
                    let (dx, dy) = if out { (dir, 0) } else { (0, -1) };
                    if let Some(j) = self.at(x, y, dx, dy).filter(|&j| is_water(j)) {
                        sites.push(Site::new(j, Part::Leaf, pos + 1, BLADE_WEIGHT));
                    }
                }
                Some(Part::Root) if pos < f.root_depth => {
                    // The first root goes straight down, leaving the top
                    // layer beside it to the runners.
                    let dirs: &[(isize, isize)] = if pos < 2 {
                        &[(0, 1)]
                    } else {
                        &[(0, 1), (-1, 0), (1, 0)]
                    };
                    for &(dx, dy) in dirs {
                        if let Some(j) = self.at(x, y, dx, dy)
                            && substrate(j)
                            && self.lone_root(j, i, id)
                        {
                            sites.push(Site::new(j, Part::Root, pos + 1, ROOT_WEIGHT));
                        }
                    }
                }
                // Only a runner's tip creeps on: a cell further back can
                // find substrate ahead again once its runner has climbed
                // over it.
                Some(Part::Runner) if pos < f.runner_limit() && self.runner_tip(x, y, id, pos) => {
                    let dir = if x < bx { -1 } else { 1 };
                    let step = if f.runner_depth == 0 {
                        self.runner_step(x, y, dir, &f.substrates)
                    } else {
                        self.deep_runner_step(x, y, dir, f, pos)
                    };
                    if let Some(j) = step {
                        sites.push(Site::new(j, Part::Runner, pos + 1, RUNNER_WEIGHT));
                    }
                }
                _ => {}
            }
        }
        for (x, dx) in [(bx, -1), (x1, 1)] {
            let Some(j) = self.at(x, by, dx, 0).filter(|&j| is_water(j)) else {
                continue;
            };
            if dx > 0 && crown < f.crown {
                let (jx, jy) = (j % self.width, j / self.width);
                if self.at(jx, jy, 0, 1).is_some_and(substrate) {
                    sites.push(Site::new(j, Part::Stem, 0, CROWN_WEIGHT));
                }
            } else if new_blades {
                sites.push(Site::new(j, Part::Leaf, 1, NEW_BLADE_WEIGHT));
            }
        }
        // Runners leave the root under the crown once every blade started.
        if runners
            && !new_blades
            && self.plants.material(Part::Runner).is_some()
            && let Some(r) = self.at(bx, by, 0, 1)
            && self.is_plant_member(r, id)
        {
            for dx in [-1, 1] {
                if let Some(j) = self.at(bx, by + 1, dx, 0)
                    && substrate(j)
                    && self.top_layer(j) <= 2
                {
                    sites.push(Site::new(j, Part::Runner, 1, RUNNER_WEIGHT));
                }
            }
        }
        sites.sort_unstable_by_key(|s| (s.at, core::cmp::Reverse(s.weight), s.position));
        sites.dedup_by_key(|s| s.at);
        sites
    }

    /// The column plant `id`'s petiole through `(x, y)` (at `pos`) rose
    /// from: where its first cell is, found by walking down it.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn petiole_base(&self, x: usize, y: usize, id: u16, pos: u8, f: &RosetteForm) -> usize {
        let leaf = self.plants.material(Part::Leaf);
        let (mut x, mut y, mut pos) = (x, y, pos);
        while pos > 1 {
            let prev = [(0, 1), (-1, 0), (1, 0)].into_iter().find_map(|(dx, dy)| {
                let j = self.at(x, y, dx, dy)?;
                let c = self.cells[j];
                (Some(c.material) == leaf
                    && c.state == id
                    && plant_position(c.variant) == pos - 1
                    && !f.is_pad(pos - 1))
                .then_some(j)
            });
            let Some(j) = prev else {
                break;
            };
            (x, y, pos) = (j % self.width, j / self.width, pos - 1);
        }
        x
    }

    /// Whether plant `id`'s petiole cell at `(x, y)`, at `pos`, has a next
    /// petiole cell beside it (it is not its petiole's tip).
    fn petiole_goes_on(&self, x: usize, y: usize, id: u16, pos: u8, f: &RosetteForm) -> bool {
        let leaf = self.plants.material(Part::Leaf);
        DIRS.iter().any(|&(dx, dy)| {
            self.at(x, y, dx, dy).is_some_and(|j| {
                let c = self.cells[j];
                Some(c.material) == leaf
                    && c.state == id
                    && plant_position(c.variant) == pos + 1
                    && !f.is_pad(pos + 1)
            })
        })
    }

    /// Whether plant `id`'s runner cell at `(x, y)`, at `pos`, is its
    /// runner's tip: no runner cell of the plant one further on beside it.
    fn runner_tip(&self, x: usize, y: usize, id: u16, pos: u8) -> bool {
        let runner = self.plants.material(Part::Runner);
        DIRS.iter().all(|&(dx, dy)| {
            self.at(x, y, dx, dy).is_none_or(|j| {
                let c = self.cells[j];
                Some(c.material) != runner || c.state != id || plant_position(c.variant) != pos + 1
            })
        })
    }

    /// Where a runner at `(x, y)` heading `dir` creeps next, keeping to the
    /// top two layers of `substrates`: on along its row; up a cell where the
    /// substrate ahead lies deeper than that; down a cell where it ends
    /// (water ahead). `None` where something else blocks it (rock, another
    /// plant, its own next cell).
    fn runner_step(
        &self,
        x: usize,
        y: usize,
        dir: isize,
        substrates: &[MaterialId],
    ) -> Option<usize> {
        if dir == 0 {
            return None;
        }
        let substrate = |j: usize| substrates.contains(&self.cells[j].material);
        let ahead = self.at(x, y, dir, 0)?;
        if substrate(ahead) {
            if self.top_layer(ahead) <= 2 {
                return Some(ahead);
            }
            return self.at(x, y, 0, -1).filter(|&j| substrate(j));
        }
        if self.is_fluid(self.cells[ahead].material) {
            return self
                .at(x, y, 0, 1)
                .filter(|&j| substrate(j) && self.top_layer(j) <= 2);
        }
        None
    }

    /// How deep the cell at `j` lies in the ground: 1 if the cell above is
    /// open (fluid or a plant), 2 if the one above that is, else 3.
    /// The next cell of a deep runner (`runner_depth` > 0, tiger lotus
    /// type) whose tip is at `(x, y)` and `pos`: short of `spacing` it dives
    /// to `runner_depth` cells below the bed's top layer and creeps on there
    /// (rising where the bed rises); from `spacing` on it climbs back up
    /// through the substrate, to root a plant where water is above.
    fn deep_runner_step(
        &self,
        x: usize,
        y: usize,
        dir: isize,
        f: &RosetteForm,
        pos: u8,
    ) -> Option<usize> {
        let substrate = |j: usize| f.substrates.contains(&self.cells[j].material);
        if pos >= f.spacing {
            let up = self.at(x, y, 0, -1)?;
            let m = self.cells[up].material;
            if substrate(up) {
                return Some(up);
            }
            // Under the plant it rooted (or water, where it couldn't): it
            // is done. Under rock or the like it creeps on to get past.
            if self.is_fluid(m) || self.plants.part(m).is_some() {
                return None;
            }
            return self.runner_step(x, y, dir, &f.substrates);
        }
        let depth = f.runner_depth;
        let down = self.at(x, y, 0, 1).filter(|&j| substrate(j));
        if self.depth_in_bed(x + y * self.width, depth) < depth
            && let Some(down) = down
        {
            return Some(down);
        }
        let ahead = self.at(x, y, dir, 0)?;
        if substrate(ahead) {
            if self.depth_in_bed(ahead, depth) <= depth {
                return Some(ahead);
            }
            // The bed rises ahead: keep to its depth.
            return self.at(x, y, 0, -1).filter(|&j| substrate(j));
        }
        // Rock or glass ahead, or the bed drops away: go under if it can.
        down
    }

    /// How many cells of the bed (and the roots and runners in it) lie
    /// above `j` before water or a plant above the bed (0 in the top
    /// layer), counted up to `cap + 1`.
    fn depth_in_bed(&self, j: usize, cap: u8) -> u8 {
        let (x, y) = (j % self.width, j / self.width);
        let mut depth = 0;
        while depth <= cap {
            let Some(above) = self.at(x, y, 0, -isize::from(depth) - 1) else {
                break;
            };
            let m = self.cells[above].material;
            let buried = matches!(self.plants.part(m), None | Some(Part::Root | Part::Runner));
            if self.is_fluid(m) || !buried {
                break;
            }
            depth += 1;
        }
        depth
    }

    fn top_layer(&self, j: usize) -> u8 {
        let (x, y) = (j % self.width, j / self.width);
        let open = |k: Option<usize>| {
            k.is_none_or(|k| {
                self.is_fluid(self.cells[k].material)
                    || self.plants.part(self.cells[k].material).is_some()
            })
        };
        if open(self.at(x, y, 0, -1)) {
            1
        } else if open(self.at(x, y, 0, -2)) {
            2
        } else {
            3
        }
    }

    /// Plant `parent`'s runner reaching the substrate cell at `at` roots a
    /// child plant there if the cell above is water: the child's first root
    /// at `at` and its crown above (parent id set, generation + 1). Returns
    /// whether it did (not at the plant cap, nor under something else).
    fn sprout(&mut self, parent: u16, species: PlantSpeciesId, at: usize) -> bool {
        let (x, y) = (at % self.width, at / self.width);
        let Some(above) = self
            .at(x, y, 0, -1)
            .filter(|&j| Some(self.cells[j].material) == self.plants.water)
        else {
            return false;
        };
        let child = self.new_plant(species, above, Some(parent));
        if child == 0 {
            return false;
        }
        self.put_part(above, Part::Stem, child, 0);
        self.put_part(at, Part::Root, child, 1);
        true
    }

    /// Where a floater can grow (see [`FloaterForm`]): until it has all
    /// its fronds, a new one in the water beside an end frond, if that cell
    /// is open to the air above (so fronds keep to the top water row); a
    /// root strand from under a frond, not beside another, up to `roots`
    /// strands, each growing on down to `root_length`; once it has its
    /// fronds (or reached its mature size, `mature`), a bud where a frond
    /// would go.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn floater_sites(
        &self,
        id: u16,
        base: usize,
        f: &FloaterForm,
        mature: bool,
        members: &[usize],
    ) -> Vec<Site> {
        let water = self.plants.water;
        let is_water = |j: usize| Some(self.cells[j].material) == water;
        // Water with a lighter fluid (air) above it.
        let at_surface = |j: usize| {
            let (x, y) = (j % self.width, j / self.width);
            is_water(j)
                && self.at(x, y, 0, -1).is_some_and(|a| {
                    let m = self.cells[a].material;
                    Some(m) != water && self.is_fluid(m)
                })
        };
        let float = self.plants.material(Part::Float);
        let root = self.plants.material(Part::Root);
        let (mut fronds, mut strands) = (0u8, 0u8);
        for &i in members {
            let c = self.cells[i];
            if Some(c.material) == float {
                fronds = fronds.saturating_add(1);
            } else if Some(c.material) == root && plant_position(c.variant) == 1 {
                strands = strands.saturating_add(1);
            }
        }
        let grow_fronds = fronds < f.fronds && !mature;
        let budding = fronds >= f.fronds || mature;
        let bx = base % self.width;
        let mut sites = Vec::new();
        for &i in members {
            let c = self.cells[i];
            let (x, y) = (i % self.width, i / self.width);
            let pos = plant_position(c.variant);
            match self.plants.part(c.material) {
                Some(Part::Float) => {
                    for dx in [-1, 1] {
                        let Some(j) = self.at(x, y, dx, 0).filter(|&j| at_surface(j)) else {
                            continue;
                        };
                        if grow_fronds {
                            let out =
                                u8::try_from((j % self.width).abs_diff(bx)).unwrap_or(u8::MAX);
                            sites.push(Site::new(j, Part::Float, out, FROND_WEIGHT));
                        } else if budding {
                            sites.push(Site::bud(j));
                        }
                    }
                    if strands < f.roots
                        && let Some(j) = self.at(x, y, 0, 1)
                        && is_water(j)
                        && self.lone_root(j, i, id)
                    {
                        sites.push(Site::new(j, Part::Root, 1, HANGING_ROOT_WEIGHT));
                    }
                }
                Some(Part::Root) if pos < f.root_length => {
                    if let Some(j) = self.at(x, y, 0, 1)
                        && is_water(j)
                        && self.lone_root(j, i, id)
                    {
                        sites.push(Site::new(j, Part::Root, pos + 1, HANGING_ROOT_WEIGHT));
                    }
                }
                _ => {}
            }
        }
        sites.sort_unstable_by_key(|s| (s.at, core::cmp::Reverse(s.weight), s.position));
        sites.dedup_by_key(|s| s.at);
        sites
    }

    /// Floater `parent` buds a new plant of `species` at the water cell
    /// `at`: one floating leaf (parent id set, generation + 1), moving as
    /// its parent does. Returns whether it did (not at the plant cap).
    fn bud(&mut self, parent: u16, species: PlantSpeciesId, at: usize) -> bool {
        let child = self.new_plant(species, at, Some(parent));
        if child == 0 {
            return false;
        }
        self.put_part(at, Part::Float, child, 0);
        true
    }

    /// Where an epiphyte can grow (see [`EpiphyteForm`]): its rhizome
    /// creeps into water cells touching what it clings to (staying a single
    /// strand via [`Self::lone_part`]), puts up a leaf every `spacing`
    /// rhizome cells, and each leaf grows upward (leaning outward from the
    /// base every `lean` cells).
    #[allow(clippy::many_single_char_names)] // cell maths
    fn epiphyte_sites(
        &self,
        id: u16,
        base: usize,
        f: &EpiphyteForm,
        members: &[usize],
    ) -> Vec<Site> {
        let water = self.plants.water;
        let is_water = |j: usize| Some(self.cells[j].material) == water;
        let bx = base % self.width;
        let mut sites = Vec::new();
        for &i in members {
            let c = self.cells[i];
            let (x, y) = (i % self.width, i / self.width);
            let pos = plant_position(c.variant);
            match self.plants.part(c.material) {
                Some(Part::Rhizome) => {
                    if pos < f.rhizome_length {
                        for &(dx, dy) in &DIRS {
                            if let Some(j) = self.at(x, y, dx, dy)
                                && is_water(j)
                                && self.touches_any(j, &f.on)
                                && self.lone_part(j, i, id, Part::Rhizome)
                            {
                                sites.push(Site::new(j, Part::Rhizome, pos + 1, RHIZOME_WEIGHT));
                            }
                        }
                    }
                    if pos > 0
                        && (pos - 1).is_multiple_of(f.spacing)
                        && let Some(j) = self.at(x, y, 0, -1)
                        && is_water(j)
                    {
                        sites.push(Site::new(j, Part::Leaf, 1, EPIPHYTE_LEAF_WEIGHT));
                    }
                }
                Some(Part::Leaf) if pos < f.leaf_length => {
                    let dir = match x.cmp(&bx) {
                        core::cmp::Ordering::Less => -1,
                        core::cmp::Ordering::Equal => 0,
                        core::cmp::Ordering::Greater => 1,
                    };
                    let lean = usize::from(f.lean).max(3);
                    let out = f.lean > 0
                        && dir != 0
                        && pos >= 2
                        && usize::from(pos - 2).is_multiple_of(lean);
                    let (dx, dy) = if out { (dir, 0) } else { (0, -1) };
                    if let Some(j) = self.at(x, y, dx, dy).filter(|&j| is_water(j)) {
                        sites.push(Site::new(j, Part::Leaf, pos + 1, BLADE_WEIGHT));
                    }
                }
                _ => {}
            }
        }
        sites.sort_unstable_by_key(|s| (s.at, core::cmp::Reverse(s.weight), s.position));
        sites.dedup_by_key(|s| s.at);
        sites
    }

    /// Where a ball can grow (see [`BallForm`]): water beside its cells,
    /// within `radius` + ½ of their middle (their mean) and keeping its
    /// bounds within `2 · radius + 1`, nearest the middle first, so it
    /// grows round, out from where it rests.
    #[allow(clippy::cast_possible_wrap, clippy::many_single_char_names)] // cell maths
    fn ball_sites(&self, f: &BallForm, members: &[usize]) -> Vec<Site> {
        let water = self.plants.water;
        let mut bounds = Rect::EMPTY;
        let (mut sx, mut sy) = (0usize, 0usize);
        for &i in members {
            let (x, y) = (i % self.width, i / self.width);
            bounds = bounds.union(point(x, y));
            sx += x;
            sy += y;
        }
        // The middle, in half cells.
        let n = members.len().max(1);
        let (cx2, cy2) = ((4 * sx / n).div_ceil(2), (4 * sy / n).div_ceil(2));
        let span = 2 * usize::from(f.radius) + 1;
        // Distances in half cells: within `radius` + ½.
        let reach = (span * span) as isize;
        let mut sites = Vec::new();
        for &i in members {
            let (x, y) = (i % self.width, i / self.width);
            for &(dx, dy) in &DIRS {
                let Some(j) = self
                    .at(x, y, dx, dy)
                    .filter(|&j| Some(self.cells[j].material) == water)
                else {
                    continue;
                };
                let (jx, jy) = (j % self.width, j / self.width);
                let b = bounds.union(point(jx, jy));
                if b.x1 - b.x0 > span || b.y1 - b.y0 > span {
                    continue;
                }
                let ex = 2 * jx as isize - cx2 as isize;
                let ey = 2 * jy as isize - cy2 as isize;
                let d2 = ex * ex + ey * ey;
                if d2 > reach {
                    continue;
                }
                // How far out, in cells, rounded.
                #[allow(clippy::cast_precision_loss)] // d2 <= reach, small
                let out = (d2.unsigned_abs() as f64).sqrt() / 2.0;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // ≤ radius
                let position = (out.round() as u8).min(f.radius);
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)] // 0..=reach
                let weight = 1 + (reach - d2) as u32;
                sites.push(Site::new(j, Part::Stem, position, weight));
            }
        }
        sites.sort_unstable_by_key(|s| (s.at, core::cmp::Reverse(s.weight), s.position));
        sites.dedup_by_key(|s| s.at);
        // It grows nearest its middle first, so it stays round (a site
        // touching more of its cells counts as a little nearer, so it fills
        // in before it bulges).
        let score = |s: &Site| {
            let (x, y) = (s.at % self.width, s.at / self.width);
            let held = DIRS
                .iter()
                .filter(|&&(dx, dy)| {
                    self.at(x, y, dx, dy)
                        .is_some_and(|j| members.binary_search(&j).is_ok())
                })
                .count();
            #[allow(clippy::cast_possible_truncation)] // ≤ 4
            let held = held as u32;
            s.weight + BALL_HOLD * held
        };
        let best = sites.iter().map(score).max().unwrap_or(0);
        sites.retain(|s| score(s) + BALL_SLACK >= best);
        sites
    }

    /// Where a mat can grow: water next to its cells that touches what it
    /// clings to (layer 0), or an outer layer (`pos + 1 < layers`) off a
    /// nearer layer.
    fn mat_sites(&self, s: &PlantSpecies, members: &[usize]) -> Vec<Site> {
        let Form::Mat(m) = &s.form else {
            return Vec::new();
        };
        let water = self.plants.water;
        let mut sites = Vec::new();
        for &i in members {
            let (x, y) = (i % self.width, i / self.width);
            let pos = plant_position(self.cells[i].variant);
            for &(dx, dy) in &DIRS {
                if let Some(j) = self.at(x, y, dx, dy)
                    && Some(self.cells[j].material) == water
                {
                    if self.touches_any(j, &m.on) {
                        sites.push(Site::new(j, Part::Stem, 0, MAT_BASE_WEIGHT));
                    } else if pos + 1 < m.layers {
                        sites.push(Site::new(j, Part::Stem, pos + 1, MAT_LAYER_WEIGHT));
                    }
                }
            }
        }
        sites.sort_unstable_by_key(|s| (s.at, s.position));
        sites.dedup_by_key(|s| s.at);
        sites
    }

    /// Drops plant `id`'s `n` oldest leaf cells as dead plant matter: the
    /// lowest first (outermost first along a leaf, so the plant stays in
    /// one piece); a rosette's or carpet's blades die back from their tips.
    fn drop_leaves(&mut self, id: u16, n: usize) {
        let Some(p) = self.plants.table.get(&id) else {
            return;
        };
        let base_x = p.base % self.width;
        let floats = self
            .plants
            .species
            .get(p.species)
            .is_some_and(|s| s.form.floater().is_some());
        // A floater's leaves are floating leaves; it loses its outermost
        // first (sorted below as a stem plant's are: one row, furthest out).
        // A tiger lotus type's floating pads count with its blades.
        let leaf = self
            .plants
            .material(if floats { Part::Float } else { Part::Leaf });
        let pad = self.plants.material(Part::Float);
        let mut leaves: Vec<usize> = self
            .plant_members(id, self.clip_to_grid(p.bounds))
            .into_iter()
            .filter(|&i| {
                let m = Some(self.cells[i].material);
                m == leaf || (m == pad && pad.is_some())
            })
            .collect();
        let blades = self
            .plants
            .species
            .get(p.species)
            .is_some_and(|s| s.form.rosette().is_some() || s.form.epiphyte().is_some());
        if blades {
            // Blades die back from their tips, so no blade is cut off.
            leaves.sort_unstable_by_key(|&i| {
                (core::cmp::Reverse(plant_position(self.cells[i].variant)), i)
            });
        } else {
            leaves.sort_unstable_by_key(|&i| {
                let (x, y) = (i % self.width, i / self.width);
                (
                    core::cmp::Reverse(y),
                    core::cmp::Reverse(x.abs_diff(base_x)),
                    i,
                )
            });
        }
        leaves.truncate(n);
        self.make_dead(&leaves);
        if let Some(p) = self.plants.table.get_mut(&id) {
            p.pending = true;
            self.plants.any_pending = true;
        }
    }

    /// Plant `id` dies: every cell becomes dead plant matter.
    fn kill_plant(&mut self, id: u16) {
        let Some(p) = self.plants.table.remove(&id) else {
            return;
        };
        self.plants.rafts.remove(&id);
        self.plants.version += 1;
        let cells = self.plant_members(id, self.clip_to_grid(p.bounds));
        self.make_dead(&cells);
    }

    /// The cells of a disc of `radius` centred on `(cx, cy)`, clipped to
    /// the grid (the same disc as [`World::paint`]).
    pub(super) fn disc(&self, cx: i64, cy: i64, radius: u32) -> Vec<(usize, usize)> {
        #[allow(clippy::cast_possible_wrap)] // MAX_SIDE is small
        let r = i64::from(radius).min(2 * super::MAX_SIDE as i64);
        let mut out = Vec::new();
        #[allow(clippy::cast_possible_wrap)] // sides are at most MAX_SIDE
        let (w, h) = (self.width as i64, self.height as i64);
        for y in (cy - r).max(0)..=(cy + r).min(h - 1) {
            for x in (cx - r).max(0)..=(cx + r).min(w - 1) {
                let (dx, dy) = (x - cx, y - cy);
                if dx * dx + dy * dy > r * r + r {
                    continue;
                }
                if let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) {
                    out.push((x, y));
                }
            }
        }
        out
    }
}

/// A plant's own depth band and the form its cells' bands follow (see
/// [`World::plant_band_at`]).
#[derive(Clone, Copy, Debug)]
pub(super) struct PlantDepth {
    id: u16,
    base: i32,
    shape: Shape,
}

impl PlantDepth {
    /// Whether the plant is a floater, whose roots are soft strands
    /// hanging in the water (a swimmer passes through them).
    pub(super) fn floats(&self) -> bool {
        matches!(self.shape, Shape::Floater)
    }
}

#[derive(Clone, Copy, Debug)]
enum Shape {
    /// A stem plant: leaves left of `base_x` are one leaf, right another.
    Stem {
        base_x: usize,
    },
    /// A rosette's, carpet's or epiphyte's blades; leaf cells past
    /// `pads_from` are a tiger lotus type's leaf pads.
    Blades {
        pads_from: Option<u8>,
    },
    Mat,
    /// One band: a floater, whose roots hang soft in the water.
    Floater,
    /// One band: a ball (marimo), or a plant not in the table (between an
    /// edit and its tidy).
    Plain,
}

/// A place a plant can grow a cell.
#[derive(Clone, Copy, Debug)]
struct Site {
    at: usize,
    part: Part,
    position: u8,
    weight: u32,
    /// A floater's bud: a new plant starts here (T6c part 2).
    bud: bool,
}

impl Site {
    fn new(at: usize, part: Part, position: u8, weight: u32) -> Self {
        Self {
            at,
            part,
            position,
            weight,
            bud: false,
        }
    }

    fn bud(at: usize) -> Self {
        Self {
            bud: true,
            ..Self::new(at, Part::Float, 0, BUD_WEIGHT)
        }
    }
}

/// Whether cells `a` and `b` are the same or 4-neighbours.
fn near(a: usize, b: usize, width: usize) -> bool {
    let (ax, ay, bx, by) = (a % width, a / width, b % width, b / width);
    ax.abs_diff(bx) + ay.abs_diff(by) <= 1
}

/// The one-cell rectangle at `(x, y)`.
fn point(x: usize, y: usize) -> Rect {
    Rect {
        x0: x,
        y0: y,
        x1: x + 1,
        y1: y + 1,
    }
}

fn factor_v1(f: Factor) -> v1::Factor {
    match f {
        Factor::Light => v1::Factor::Light,
        Factor::Nutrient => v1::Factor::Nutrient,
        Factor::Co2 => v1::Factor::Co2,
    }
}

fn factor_from_v1(f: v1::Factor) -> Factor {
    match f {
        v1::Factor::Light => Factor::Light,
        v1::Factor::Nutrient => Factor::Nutrient,
        v1::Factor::Co2 => Factor::Co2,
    }
}

fn limit_v1(l: Limit) -> v1::Limit {
    match l {
        Limit::None => v1::Limit::None,
        Limit::TooLow(f) => v1::Limit::TooLow(factor_v1(f)),
        Limit::TooHigh(f) => v1::Limit::TooHigh(factor_v1(f)),
    }
}

fn limit_from_v1(l: v1::Limit) -> Limit {
    match l {
        v1::Limit::None => Limit::None,
        v1::Limit::TooLow(f) => Limit::TooLow(factor_from_v1(f)),
        v1::Limit::TooHigh(f) => Limit::TooHigh(factor_from_v1(f)),
    }
}
