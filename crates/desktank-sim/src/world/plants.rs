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
//! The table is plain data (numbers, ids and a rectangle), so the save
//! format (T8) can store it as it is; the samples and update bookkeeping
//! can be reset on load.

use std::collections::BTreeMap;
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
    COUNTED_PARTS, Factor, Form, Limit, Part, PlantSpecies, PlantSpeciesId, PlantSpeciesTable,
    Propagule, RosetteForm, plant_position, plant_variant,
};
use crate::rng::splitmix64;
use crate::save::{SaveError, v1};

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

/// Health steps that change a plant's tint: a declining plant asks for a
/// new tint (and a frame) every 1/32 of health, not every 1/255.
const TINT_STEPS: f32 = 32.0;

impl Plant {
    fn tint_key(&self) -> u8 {
        tint_step(self.health)
    }
}

/// The health step the tint follows.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped
fn tint_step(health: f32) -> u8 {
    (health.clamp(0.0, 1.0) * TINT_STEPS).round() as u8
}

/// Plant state owned by the world.
#[derive(Clone, Debug)]
pub(super) struct Plants {
    species: Arc<PlantSpeciesTable>,
    table: BTreeMap<u16, Plant>,
    /// Per material id: 1 + the part index, or 0.
    part_of: Vec<u8>,
    /// Material per part, in [`Part::ALL`] order (runners are optional).
    parts: [Option<MaterialId>; 4],
    seed: Option<MaterialId>,
    cutting: Option<MaterialId>,
    dead: Option<MaterialId>,
    water: Option<MaterialId>,
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
                    #[allow(clippy::cast_possible_truncation)] // 4 parts
                    let tag = k as u8 + 1;
                    part_of[usize::from(m.0)] = tag;
                }
            }
        }
        let on = |name| enabled.then(|| materials.id(name)).flatten();
        Ok(Self {
            species: Arc::new(species),
            table: BTreeMap::new(),
            part_of,
            parts,
            seed: on("seed"),
            cutting: on("cutting"),
            dead,
            water,
            next_id: 1,
            any_pending: false,
            cursor: 0,
            cap: MAX_PLANTS,
            version: 0,
            keep_seed: keep_per_tick(SEED_ROOTING),
            keep_cutting: keep_per_tick(CUTTING_ROOTING),
            keep_falling: keep_per_tick(MAT_FALLING),
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
        out.clear();
        let (x1, y1) = (r.x1.min(self.width), r.y1.min(self.height));
        let mut memo = None;
        let mut any = false;
        for y in r.y0.min(y1)..y1 {
            for i in y * self.width + r.x0.min(x1)..y * self.width + x1 {
                let band = self.plant_band_with(i, &mut memo);
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
    /// is fuzzy.
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
            (Part::Leaf, Shape::Blades) => ((x as u64) << 1, 2),
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
            Some(Form::Rosette(_) | Form::Carpet(_)) => Shape::Blades,
            Some(Form::Mat(_)) => Shape::Mat,
            None => Shape::Plain,
        };
        PlantDepth { id, base, shape }
    }

    fn band_mix(&self, id: u16, key: u64) -> u64 {
        splitmix64(self.seed ^ splitmix64((u64::from(id) << 40) ^ key))
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
            let same = c.material == material && c.state == u16::from(species.0);
            if same || !self.props[usize::from(c.material.0)].class.is_fluid() {
                continue;
            }
            self.write_cell(x, y, material, &mut None);
            self.cells[y * self.width + x].state = u16::from(species.0);
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

    pub(super) fn hash_plants(&self, h: &mut Fnv64) {
        for (&id, p) in &self.plants.table {
            h.write(&id.to_le_bytes());
            h.write(&[p.species.0]);
            h.write(&p.born.to_le_bytes());
            for v in [p.health, p.energy, p.stress] {
                h.write(&v.to_bits().to_le_bytes());
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
                let Some(species) = self.plants.species.default_species().map(|s| s.id) else {
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
        let (health, stress, generation) = parent
            .and_then(|id| plants.table.get(&id))
            .map_or((1.0, 0.0, 0), |p| {
                (p.health, p.stress, p.generation.saturating_add(1))
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
                    last_update: self.tick,
                    sums: [0.0; 3],
                    counts: [0; 3],
                    pending: false,
                    idle: 0,
                });
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
            self.plants.version += 1;
            return;
        }
        let pieces = self.plant_pieces(&members, id, bounds);
        let keeper = pieces
            .iter()
            .position(|piece| piece.binary_search(&base).is_ok())
            .unwrap_or(0);
        let fragments = self
            .plants
            .species
            .get(species)
            .is_some_and(|s| s.propagation.fragments);
        for (k, piece) in pieces.iter().enumerate() {
            if k == keeper {
                continue;
            }
            let stem = self.plants.material(Part::Stem);
            let has_stem = piece.iter().any(|&i| Some(self.cells[i].material) == stem);
            let new = if fragments && has_stem {
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

    /// Where a piece grows from: its lowest stem cell, else its first cell.
    fn piece_base(&self, piece: &[usize]) -> usize {
        let stem = self.plants.material(Part::Stem);
        piece
            .iter()
            .rev()
            .find(|&&i| Some(self.cells[i].material) == stem)
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
        let (x, y) = (i % self.width, i / self.width);
        let species = Arc::clone(&self.plants.species);
        let s = species.get(p.species);
        if let Some(Form::Mat(mat)) = s.map(|s| &s.form)
            && !self.touches_any(i, &mat.on)
        {
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
            (_, Some(subs)) => below.is_some_and(|j| subs.contains(&self.cells[j].material)),
            (_, None) => false,
        };
        if !ready || self.response_at(s, x, y).0 <= 0.0 {
            return true;
        }
        let keep = if Some(cell.material) == self.plants.seed {
            self.plants.keep_seed
        } else {
            self.plants.keep_cutting
        };
        if !self.life.chance(1.0 - powu(keep, k)) {
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
        }
        true
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
        // at most twice its spacing long).
        let mature = size >= s.growth.size;
        if mature && !(s.propagation.runners && s.form.rosette().is_some()) {
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
    }

    /// Grows up to `want` cells of plant `id` where its form allows (only
    /// runners if `runners_only`), taking the species' nutrient cost from
    /// the field at each. Returns how many it grew.
    fn grow_plant(&mut self, id: u16, s: &PlantSpecies, want: usize, runners_only: bool) -> usize {
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
        };
        if runners_only {
            sites.retain(|s| s.part == Part::Runner);
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
            let sprouted = site.part == Part::Runner
                && s.form.rosette().is_some_and(|f| site.position >= f.spacing)
                && self.sprout(id, s.id, site.at);
            if !sprouted {
                self.put_part(site.at, site.part, id, site.position);
            }
            let (x, y) = (site.at % self.width, site.at / self.width);
            self.life
                .field_mut(FieldKind::Nutrient)
                .add_at_cell(x, y, -s.growth.cost);
            self.life.version += 1;
            grown += 1;
            // Sites next to the new cell may break a spacing rule now.
            sites.retain(|o| !near(o.at, site.at, self.width));
        }
        grown
    }

    /// Where a stem-form plant can grow: up from the top of its stem, a
    /// leaf out from each node or leaf tip, a root on from each root tip.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn stem_sites(&self, id: u16, base: usize, s: &PlantSpecies, members: &[usize]) -> Vec<Site> {
        let Form::Stem(f) = &s.form else {
            return Vec::new();
        };
        let water = self.plants.water;
        let is_water = |j: usize| Some(self.cells[j].material) == water;
        let base_x = base % self.width;
        let mut sites = Vec::new();
        let mut top: Option<(usize, u8)> = None;
        for &i in members {
            let c = self.cells[i];
            let (x, y) = (i % self.width, i / self.width);
            let pos = plant_position(c.variant);
            match self.plants.part(c.material) {
                Some(Part::Stem) => {
                    if top.is_none_or(|(t, _)| i < t) {
                        top = Some((i, pos));
                    }
                    if pos >= f.internode && pos.is_multiple_of(f.internode) {
                        for dx in [-1, 1] {
                            if let Some(j) = self.at(x, y, dx, 0).filter(|&j| is_water(j)) {
                                sites.push(Site::new(j, Part::Leaf, 1, LEAF_WEIGHT));
                            }
                        }
                    }
                }
                Some(Part::Leaf) if pos < f.leaf_length && x != base_x => {
                    let dx = if x < base_x { -1 } else { 1 };
                    if let Some(j) = self.at(x, y, dx, 0).filter(|&j| is_water(j)) {
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
        if let Some((t, pos)) = top
            && pos < f.height
            && let Some(j) = self.at(t % self.width, t / self.width, 0, -1)
            && is_water(j)
        {
            sites.push(Site::new(j, Part::Stem, pos + 1, STEM_WEIGHT));
        }
        sites.sort_unstable_by_key(|s| s.at);
        sites.dedup_by_key(|s| s.at);
        sites
    }

    /// Whether a root growing from `from` into `j` would touch no other
    /// root of plant `id` (T6's `alone`, by plant id).
    fn lone_root(&self, j: usize, from: usize, id: u16) -> bool {
        let root = self.plants.material(Part::Root);
        let (x, y) = (j % self.width, j / self.width);
        DIRS.iter().all(|&(dx, dy)| {
            self.at(x, y, dx, dy).is_none_or(|e| {
                let c = self.cells[e];
                e == from || Some(c.material) != root || c.state != id
            })
        })
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
        let mut sites = Vec::new();
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
                Some(Part::Leaf) if pos < f.blade_length => {
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
                Some(Part::Runner)
                    if pos < f.spacing.saturating_mul(2) && self.runner_tip(x, y, id, pos) =>
                {
                    let dir = if x < bx { -1 } else { 1 };
                    if let Some(j) = self.runner_step(x, y, dir, &f.substrates) {
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
        sites.sort_unstable_by_key(|s| s.at);
        sites.dedup_by_key(|s| s.at);
        sites
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

    /// Where a mat can grow: water next to its cells that touches what it
    /// clings to.
    fn mat_sites(&self, s: &PlantSpecies, members: &[usize]) -> Vec<Site> {
        let Form::Mat(m) = &s.form else {
            return Vec::new();
        };
        let water = self.plants.water;
        let mut sites = Vec::new();
        for &i in members {
            let (x, y) = (i % self.width, i / self.width);
            for &(dx, dy) in &DIRS {
                if let Some(j) = self.at(x, y, dx, dy)
                    && Some(self.cells[j].material) == water
                    && self.touches_any(j, &m.on)
                {
                    sites.push(Site::new(j, Part::Stem, 0, 1));
                }
            }
        }
        sites.sort_unstable_by_key(|s| s.at);
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
        let leaf = self.plants.material(Part::Leaf);
        let mut leaves: Vec<usize> = self
            .plant_members(id, self.clip_to_grid(p.bounds))
            .into_iter()
            .filter(|&i| Some(self.cells[i].material) == leaf)
            .collect();
        let blades = self
            .plants
            .species
            .get(p.species)
            .is_some_and(|s| s.form.rosette().is_some());
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

#[derive(Clone, Copy, Debug)]
enum Shape {
    /// A stem plant: leaves left of `base_x` are one leaf, right another.
    Stem {
        base_x: usize,
    },
    /// A rosette's or carpet's blades.
    Blades,
    Mat,
    /// Not in the table (between an edit and its tidy).
    Plain,
}

/// A place a plant can grow a cell.
#[derive(Clone, Copy, Debug)]
struct Site {
    at: usize,
    part: Part,
    position: u8,
    weight: u32,
}

impl Site {
    fn new(at: usize, part: Part, position: u8, weight: u32) -> Self {
        Self {
            at,
            part,
            position,
            weight,
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
