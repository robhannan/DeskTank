//! Everything after the movement pass (SPEC §2.1, T5): the random-tick
//! scheduler for living cells, the devices (bubbler, filter outlet and its
//! current) and the fields.
//!
//! - **Registries.** Living cells and device cells are kept in per-chunk
//!   index lists. After each tick only the cells that changed (the chunks'
//!   changed rectangles, which every move and edit already records) are
//!   rescanned, so a settled tank pays nothing to keep them current.
//! - **Random ticks.** Each tick visits at most `budget` living cells: a
//!   window over the lists that rotates by `budget` every tick (a
//!   stratified sample), so each living cell is visited once every
//!   ⌈living / budget⌉ ticks. A visit acts for the world time since the last
//!   one, `living / budget` ticks (rounded at random so the mean is exact).
//!   A visit wakes a chunk only if it changes a cell's material; ages and
//!   buffer levels live in the cell `state` and are written quietly.
//! - **Devices** act every tick: bubblers release bubbles, and the outlet
//!   jet's current pushes floating and liquid cells.
//! - **Fields** step every [`FIELD_EVERY`] ticks.

use core::mem;
use std::collections::BTreeSet;

use rand_core::Rng as _;
use rand_pcg::Pcg32;

use super::{World, WorldConfig, WorldError};
use crate::ecology::{DIRS, Ecology, Gate, Gates, Release, powu};
use crate::field::{FIELD_EVERY, FIELD_SCALE, Field, FieldKind};
use crate::grid::{Cell, Rect};
use crate::hash::Fnv64;
use crate::material::{MaterialId, MaterialTable, MovementClass};
use crate::rng::chunk_stream;
use crate::save::{SaveError, v1};

/// Living cells visited per tick by default (SPEC §2.1: 256 per display).
pub const RANDOM_TICK_BUDGET: usize = 256;

/// Stream id for the life RNG: above any chunk id.
const LIFE_STREAM: u64 = u64::MAX;
/// Weakest current, in cells per second, that still moves cells.
const MIN_CURRENT: f32 = 0.25;

const LIVING: u8 = 1;
const DEVICE: u8 = 2;
/// A material some species eats (T7).
const FOOD: u8 = 4;
/// A dense liquid such as water (not air).
const WATERY: u8 = 8;
/// A pollutant for the water-quality score.
const POLLUTANT: u8 = 16;
/// Liquids at least this dense count as water for light and current.
const WATERY_DENSITY: f32 = 0.5;

/// What the random-tick scheduler did in the last tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RandomTickStats {
    /// Living cells in the tank.
    pub living: usize,
    /// Living cells visited (at most the budget).
    pub visited: usize,
}

/// The simple ecosystem at a glance (SPEC §2.3): the water-quality score and
/// what it is made of.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ecosystem {
    /// Water quality from 0 (foul) to 1 (pristine): falls with excess
    /// nutrients and pollutants (waste, detritus, dead plant matter), rises
    /// with plants and water changes (`assets/fields.toml`, `[quality]`).
    pub quality: f32,
    /// Mean nutrient level over the water cells.
    pub mean_nutrient: f32,
    /// Water cells (dense liquids, not air).
    pub water_cells: usize,
    /// Plant cells (every plant's parts, from the plant table).
    pub plant_cells: usize,
    /// Pollutant cells.
    pub pollutant_cells: usize,
    /// Algae cells.
    pub algae_cells: usize,
    /// Fraction of the substrate surface covered by algae: of the columns
    /// with any non-fluid cell, those whose top non-fluid cell is algae.
    pub algae_coverage: f32,
}

/// Per-chunk lists of cell indices whose material has a flag.
#[derive(Clone, Debug)]
struct Registry {
    flag: u8,
    lists: Vec<Vec<u32>>,
    total: usize,
    /// Scratch: entries a refresh removed, and the ones it found.
    removed: Vec<u32>,
    found: Vec<u32>,
}

impl Registry {
    fn new(flag: u8, chunks: usize) -> Self {
        Self {
            flag,
            lists: vec![Vec::new(); chunks],
            total: 0,
            removed: Vec::new(),
            found: Vec::new(),
        }
    }

    /// Rescans `rect` of chunk `ci`. Returns whether the list changed.
    fn refresh(
        &mut self,
        ci: usize,
        rect: Rect,
        width: usize,
        cells: &[Cell],
        flags: &[u8],
    ) -> bool {
        let list = &mut self.lists[ci];
        let before = list.len();
        let (removed, found) = (&mut self.removed, &mut self.found);
        removed.clear();
        found.clear();
        list.retain(|&i| {
            let keep = !rect.contains(i as usize % width, i as usize / width);
            if !keep {
                removed.push(i);
            }
            keep
        });
        for y in rect.y0..rect.y1 {
            let row = y * width;
            for x in rect.x0..rect.x1 {
                if flags[usize::from(cells[row + x].material.0)] & self.flag != 0 {
                    #[allow(clippy::cast_possible_truncation)] // grids are ≤ 8192²
                    found.push((row + x) as u32);
                }
            }
        }
        list.extend_from_slice(found);
        self.total = self.total + list.len() - before;
        // Found in index order; the rect's old entries may not have been.
        removed.sort_unstable();
        removed != found
    }
}

/// Life state owned by the world.
#[derive(Clone, Debug)]
pub(super) struct Life {
    eco: Ecology,
    flags: Vec<u8>,
    living: Registry,
    devices: Registry,
    /// Food cells, for creatures to find (T7).
    food: Registry,
    budget: usize,
    cursor: usize,
    stats: RandomTickStats,
    pub(super) rng: Pcg32,
    /// Scratch list of cell indices for one batch.
    batch: Vec<u32>,
    nutrient: Field,
    co2: Field,
    light: Field,
    /// Per material: how much light a cell blocks, in 1/255.
    opacity_of: Vec<u8>,
    /// Per field cell: mean opacity and fraction of watery cells.
    opacity: Vec<(f32, f32)>,
    /// Scratch: field columns whose light needs recomputing.
    columns: Vec<bool>,
    /// Field cells whose opacity needs recounting.
    opacity_dirty: Vec<bool>,
    any_opacity_dirty: bool,
    /// Per field cell: its water and pollutant cells, for the water quality
    /// creatures sample once a second (T9). Recounted only where cells
    /// changed, so a settled tank pays nothing.
    census: Vec<(u16, u16)>,
    /// Field cells whose census needs recounting.
    census_dirty: Vec<bool>,
    /// Sideways current per field cell, cells per second (+ = rightwards).
    current: Vec<f32>,
    /// Field cells with a current strong enough to move cells.
    current_blocks: Vec<usize>,
    devices_dirty: bool,
    /// Bumped whenever a field changes, for renderers.
    pub(super) version: u64,
    water: Option<MaterialId>,
    air: Option<MaterialId>,
}

impl Life {
    /// Life for a `size` (width, height) grid of `chunks` chunks; the
    /// random ticks also visit `plants` (plant parts, seeds and cuttings,
    /// T6b).
    pub(super) fn new(
        materials: &MaterialTable,
        eco: Ecology,
        size: (usize, usize),
        chunks: usize,
        seed: u64,
        foods: &[MaterialId],
        plants: &[MaterialId],
    ) -> Self {
        let (width, height) = size;
        let mut flags = vec![0u8; 256];
        // Unknown IDs behave as solid (see `World`), so they block light.
        let mut opacity_of = vec![u8::MAX; 256];
        for m in materials.iter() {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // validated 0..=1
            let o = (m.opacity * 255.0).round() as u8;
            opacity_of[usize::from(m.id.0)] = o;
            let f = &mut flags[usize::from(m.id.0)];
            if eco.behaviour[usize::from(m.id.0)].is_living() || plants.contains(&m.id) {
                *f |= LIVING;
            }
            if Some(m.id) == eco.bubbler.material || Some(m.id) == eco.current.material {
                *f |= DEVICE;
            }
            if m.class == MovementClass::Liquid && m.density >= WATERY_DENSITY {
                *f |= WATERY;
            }
            if foods.contains(&m.id) {
                *f |= FOOD;
            }
            if eco.quality.pollutants.contains(m.id) {
                *f |= POLLUTANT;
            }
        }
        let field = Field::new(width, height);
        let blocks = field.width() * field.height();
        Self {
            eco,
            flags,
            living: Registry::new(LIVING, chunks),
            devices: Registry::new(DEVICE, chunks),
            food: Registry::new(FOOD, chunks),
            budget: RANDOM_TICK_BUDGET,
            cursor: 0,
            stats: RandomTickStats::default(),
            rng: chunk_stream(seed, 0, LIFE_STREAM),
            batch: Vec::new(),
            nutrient: field.clone(),
            co2: field.clone(),
            light: field,
            opacity_of,
            opacity: vec![(0.0, 0.0); blocks],
            columns: Vec::new(),
            opacity_dirty: vec![true; blocks],
            any_opacity_dirty: true,
            census: vec![(0, 0); blocks],
            census_dirty: vec![true; blocks],
            current: vec![0.0; blocks],
            current_blocks: Vec::new(),
            devices_dirty: false,
            version: 0,
            water: materials.id("water"),
            air: materials.id("air"),
        }
    }

    pub(super) fn field_mut(&mut self, kind: FieldKind) -> &mut Field {
        match kind {
            FieldKind::Nutrient => &mut self.nutrient,
            FieldKind::Co2 => &mut self.co2,
            FieldKind::Light => &mut self.light,
        }
    }

    fn field(&self, kind: FieldKind) -> &Field {
        match kind {
            FieldKind::Nutrient => &self.nutrient,
            FieldKind::Co2 => &self.co2,
            FieldKind::Light => &self.light,
        }
    }

    /// Food cells in the tank (T7).
    pub(super) fn food_cells(&self) -> usize {
        self.food.total
    }

    /// The food cells' indices, one list per chunk. Refreshed after each
    /// tick's movement, so a cell may have been eaten since: check it.
    pub(super) fn food_lists(&self) -> &[Vec<u32>] {
        &self.food.lists
    }

    pub(super) fn chance(&mut self, p: f64) -> bool {
        f64::from(self.rng.next_u32()) < p * 4_294_967_296.0
    }
}

impl World {
    /// A world like [`World::new`] that follows `ecology` instead of the
    /// built-in reaction table and field settings.
    ///
    /// # Errors
    ///
    /// As [`World::new`].
    pub fn with_ecology(
        config: WorldConfig,
        materials: MaterialTable,
        ecology: Ecology,
    ) -> Result<Self, WorldError> {
        let mut w = Self::new(config, materials)?;
        let foods = super::Creatures::foods(w.species());
        w.life = Life::new(
            &w.materials,
            ecology,
            (w.width, w.height),
            w.chunks.len(),
            w.seed,
            &foods,
            &w.plants.living_materials(),
        );
        w.init_life();
        Ok(w)
    }

    /// The reaction table and field settings in use.
    #[must_use]
    pub fn ecology(&self) -> &Ecology {
        &self.life.eco
    }

    /// A field.
    #[must_use]
    pub fn field(&self, kind: FieldKind) -> &Field {
        self.life.field(kind)
    }

    /// Adds `amount` (negative to remove) to `kind` over cell `(x, y)`, as
    /// an emission would; the value never drops below zero. Light is
    /// recomputed from the cells, so additions to it don't last.
    pub fn add_to_field(&mut self, kind: FieldKind, x: usize, y: usize, amount: f32) {
        self.life.field_mut(kind).add_at_cell(x, y, amount);
        self.life.version += 1;
    }

    /// Bumped whenever any field changes (renderers re-upload on a change).
    #[must_use]
    pub fn fields_version(&self) -> u64 {
        self.life.version
    }

    /// The filter outlets' sideways current per field cell, in cells per
    /// second (positive is rightwards), row-major like a [`Field`].
    #[must_use]
    pub fn current(&self) -> &[f32] {
        &self.life.current
    }

    /// The current over cell `(x, y)`, in cells per second.
    #[must_use]
    pub fn current_at(&self, x: usize, y: usize) -> f32 {
        let fw = self.life.light.width();
        let (fx, fy) = (x / FIELD_SCALE, y / FIELD_SCALE);
        if fx < fw && fy < self.life.light.height() {
            self.life.current[fy * fw + fx]
        } else {
            0.0
        }
    }

    /// Living cells visited per tick (SPEC §2.1; default
    /// [`RANDOM_TICK_BUDGET`]). At least 1. A visit acts for at most 65 535
    /// ticks, so with more than 65 535 living cells per unit of budget,
    /// slow processes run slower than configured.
    pub fn set_random_tick_budget(&mut self, budget: usize) {
        self.life.budget = budget.max(1);
    }

    /// The random-tick budget.
    #[must_use]
    pub fn random_tick_budget(&self) -> usize {
        self.life.budget
    }

    /// What the random-tick scheduler did in the last tick.
    #[must_use]
    pub fn random_tick_stats(&self) -> RandomTickStats {
        self.life.stats
    }

    /// A water change (SPEC §2.3, a tray action): the nutrient and CO₂
    /// fields lose the configured fraction, as fresh water replaces old,
    /// and the siphon removes waste, detritus, dead plant matter and
    /// uneaten food, leaving water. Returns how many cells it siphoned.
    pub fn water_change(&mut self) -> usize {
        let cfg = self.life.eco.water_change;
        let keep = 1.0 - cfg.fraction;
        self.life.nutrient.scale(keep);
        self.life.co2.scale(keep);
        self.life.version += 1;
        let Some(water) = self.life.water else {
            return 0;
        };
        let mut siphoned = 0;
        for i in 0..self.cells.len() {
            if cfg.siphon.contains(self.cells[i].material) {
                self.write_cell(i % self.width, i / self.width, water, &mut None);
                siphoned += 1;
            }
        }
        siphoned
    }

    /// The water-quality score and the counts behind it. Scans the whole
    /// grid, so call it on demand (an inspect card, a tray menu, a test),
    /// not every tick.
    #[must_use]
    pub fn ecosystem(&self) -> Ecosystem {
        let q = self.life.eco.quality;
        let nutrient = &self.life.nutrient;
        let mut e = Ecosystem::default();
        let mut nutrient_sum = 0.0_f64;
        for (i, c) in self.cells.iter().enumerate() {
            let m = c.material;
            if self.life.flags[usize::from(m.0)] & WATERY != 0 {
                e.water_cells += 1;
                nutrient_sum += f64::from(nutrient.at_cell(i % self.width, i / self.width));
            }
            e.pollutant_cells += usize::from(q.pollutants.contains(m));
            e.algae_cells += usize::from(Some(m) == q.algae);
        }
        e.plant_cells = self.plant_cells();
        let (mut surface, mut covered) = (0usize, 0usize);
        for x in 0..self.width {
            if let Some(m) = (0..self.height)
                .map(|y| self.cells[y * self.width + x].material)
                .find(|&m| !self.is_fluid(m))
            {
                surface += 1;
                covered += usize::from(Some(m) == q.algae);
            }
        }
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // counts ≪ 2^24
        {
            e.algae_coverage = if surface == 0 {
                0.0
            } else {
                covered as f32 / surface as f32
            };
            e.mean_nutrient = (nutrient_sum / e.water_cells.max(1) as f64) as f32;
        }
        e.quality = quality_score(
            &self.life.eco,
            e.mean_nutrient,
            e.water_cells,
            e.pollutant_cells,
            e.plant_cells,
        );
        e
    }

    /// Whether cells of `material` are fluid (liquid or buoyant).
    pub(crate) fn is_fluid(&self, material: MaterialId) -> bool {
        self.props[usize::from(material.0)].class.is_fluid()
    }

    /// Indexes every cell and works out the light, for a new world.
    pub(super) fn init_life(&mut self) {
        let all = Rect {
            x0: 0,
            y0: 0,
            x1: self.width,
            y1: self.height,
        };
        for ci in 0..self.chunks.len() {
            let r = self.chunks[ci].bounds.intersect(all);
            self.refresh_registries(ci, r);
        }
        self.update_light();
        self.recompute_current();
    }

    /// The random-tick scheduler and fields as saved (T8). The lists keep
    /// their order, which the rotation follows; the light is saved too, as
    /// it is only recomputed at the next field step.
    pub(super) fn save_life(&self) -> v1::Life {
        let life = &self.life;
        v1::Life {
            budget: u32::try_from(life.budget).unwrap_or(u32::MAX),
            cursor: life.cursor as u64,
            living: life.living.lists.clone(),
            devices: life.devices.lists.clone(),
            food: life.food.lists.clone(),
            devices_dirty: life.devices_dirty,
            nutrient: life.nutrient.to_v1(),
            co2: life.co2.to_v1(),
            light: life.light.to_v1(),
        }
    }

    /// Restores the scheduler and fields from a save, after the cells.
    pub(super) fn load_life(&mut self, mut saved: v1::Life) -> Result<(), SaveError> {
        for (name, lists) in [
            ("living", &saved.living),
            ("device", &saved.devices),
            ("food", &saved.food),
        ] {
            if lists.len() != self.chunks.len() {
                return Err(SaveError::Invalid(format!(
                    "{} {name} lists for {} chunks",
                    lists.len(),
                    self.chunks.len()
                )));
            }
            for (ci, list) in lists.iter().enumerate() {
                for &i in list {
                    let i = self.cell_index(i, name)?;
                    if self.chunk_of(i) != ci {
                        return Err(SaveError::Invalid(format!(
                            "{name} cell {i} listed in chunk {ci}"
                        )));
                    }
                }
            }
        }
        // Older saves indexed fronds as living cells only. Preserve list
        // order and leave pending edits for the next registry refresh.
        if let Some(frond) = self.materials.id("floating_leaf") {
            for (ci, food) in saved.food.iter_mut().enumerate() {
                let pending = self.chunks[ci].changed_next;
                let missing = saved.living[ci].iter().copied().filter(|&i| {
                    let at = i as usize;
                    self.cells[at].material == frond
                        && !pending.contains(at % self.width, at / self.width)
                });
                let mut indexed = None;
                for i in missing {
                    let sorted = indexed.get_or_insert_with(|| {
                        let mut indices = food.clone();
                        indices.sort_unstable();
                        indices
                    });
                    if sorted.binary_search(&i).is_err() {
                        food.push(i);
                    }
                }
            }
        }
        // The current is a function of the device cells; working it out
        // wakes the fields, so their saved state is restored after.
        let life = &mut self.life;
        for (registry, lists) in [
            (&mut life.living, saved.living),
            (&mut life.devices, saved.devices),
            (&mut life.food, saved.food),
        ] {
            registry.total = lists.iter().map(Vec::len).sum();
            registry.lists = lists;
        }
        self.recompute_current();
        let life = &mut self.life;
        life.budget = usize::try_from(saved.budget).unwrap_or(usize::MAX).max(1);
        life.cursor = usize::try_from(saved.cursor).unwrap_or(0);
        life.devices_dirty = saved.devices_dirty;
        life.nutrient.load_v1(saved.nutrient)?;
        life.co2.load_v1(saved.co2)?;
        life.light.load_v1(saved.light)?;
        // Recount every block's opacity at the next field step: the clean
        // ones give back the light they have, the dirty ones catch up.
        life.opacity_dirty.fill(true);
        life.any_opacity_dirty = true;
        life.version += 1;
        Ok(())
    }

    pub(super) fn hash_life(&self, h: &mut Fnv64) {
        for f in [&self.life.nutrient, &self.life.co2] {
            for v in f.values() {
                h.write(&v.to_bits().to_le_bytes());
            }
        }
    }

    /// Runs after the movement pass and the chunk bookkeeping of a tick.
    pub(super) fn after_tick(&mut self) {
        self.life.rng = chunk_stream(self.seed, self.tick, LIFE_STREAM);
        for ci in 0..self.chunks.len() {
            let r = self.chunks[ci].changed;
            if !r.is_empty() {
                self.refresh_registries(ci, r);
            }
        }
        if mem::take(&mut self.life.devices_dirty) {
            self.recompute_current();
        }
        self.run_devices();
        self.run_current();
        self.random_ticks();
        self.run_spawns();
        // Plants grow, drop leaves and die after their cells were sampled;
        // the tidy then keeps the table matching the grid.
        self.update_plants();
        self.tidy_plants();
        if self.tick.is_multiple_of(FIELD_EVERY) {
            self.step_fields();
        }
    }

    fn refresh_registries(&mut self, ci: usize, r: Rect) {
        if r.is_empty() {
            return;
        }
        let life = &mut self.life;
        life.living
            .refresh(ci, r, self.width, &self.cells, &life.flags);
        if life
            .devices
            .refresh(ci, r, self.width, &self.cells, &life.flags)
        {
            life.devices_dirty = true;
        }
        life.food
            .refresh(ci, r, self.width, &self.cells, &life.flags);
        let fw = life.light.width();
        for fy in r.y0 / FIELD_SCALE..=(r.y1 - 1) / FIELD_SCALE {
            for fx in r.x0 / FIELD_SCALE..=(r.x1 - 1) / FIELD_SCALE {
                life.opacity_dirty[fy * fw + fx] = true;
                life.census_dirty[fy * fw + fx] = true;
            }
        }
        life.any_opacity_dirty = true;
    }

    /// The water-quality score as [`World::ecosystem`] computes it, from
    /// per-field-cell counts that are recounted only where cells changed
    /// since the last call (including edits not yet seen by a tick), so it
    /// costs about a pass over the field cells and the plant table, not a
    /// scan of the grid. Creatures sample it once a second (T9).
    pub fn water_quality(&mut self) -> f32 {
        let fw = self.life.light.width();
        for ci in 0..self.chunks.len() {
            let r = self.chunks[ci].changed_next;
            if r.is_empty() {
                continue;
            }
            for fy in r.y0 / FIELD_SCALE..=(r.y1 - 1) / FIELD_SCALE {
                for fx in r.x0 / FIELD_SCALE..=(r.x1 - 1) / FIELD_SCALE {
                    self.life.census_dirty[fy * fw + fx] = true;
                }
            }
        }
        let life = &mut self.life;
        let mut water = 0usize;
        let mut pollutants = 0usize;
        let mut nutrient_sum = 0.0_f64;
        for (f, dirty) in life.census_dirty.iter_mut().enumerate() {
            if mem::take(dirty) {
                let (fx, fy) = (f % fw, f / fw);
                let (x0, y0) = (fx * FIELD_SCALE, fy * FIELD_SCALE);
                let (x1, y1) = (
                    (x0 + FIELD_SCALE).min(self.width),
                    (y0 + FIELD_SCALE).min(self.height),
                );
                let (mut w, mut p) = (0u16, 0u16);
                for y in y0..y1 {
                    for c in &self.cells[y * self.width + x0..y * self.width + x1] {
                        let flags = life.flags[usize::from(c.material.0)];
                        w += u16::from(flags & WATERY != 0);
                        p += u16::from(flags & POLLUTANT != 0);
                    }
                }
                life.census[f] = (w, p);
            }
            let (w, p) = life.census[f];
            water += usize::from(w);
            pollutants += usize::from(p);
            nutrient_sum += f64::from(w) * f64::from(life.nutrient.values()[f]);
        }
        let plants = self.plant_cells();
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // counts ≪ 2^24
        let mean = (nutrient_sum / water.max(1) as f64) as f32;
        quality_score(&self.life.eco, mean, water, pollutants, plants)
    }

    /// Visits up to the budget of living cells (see the module docs).
    fn random_ticks(&mut self) {
        let total = self.life.living.total;
        let n = self.life.budget.min(total);
        self.life.stats = RandomTickStats {
            living: total,
            visited: n,
        };
        if n == 0 {
            return;
        }
        let start = self.life.cursor % total;
        self.life.cursor = (start + n) % total;
        let mut batch = mem::take(&mut self.life.batch);
        batch.clear();
        // Walk the concatenated lists from `start`, wrapping round.
        let lists = &self.life.living.lists;
        let mut skip = start;
        let mut ci = 0;
        while skip >= lists[ci].len() {
            skip -= lists[ci].len();
            ci += 1;
        }
        let mut off = skip;
        while batch.len() < n {
            if off >= lists[ci].len() {
                ci = (ci + 1) % lists.len();
                off = 0;
                continue;
            }
            batch.push(lists[ci][off]);
            off += 1;
        }
        #[allow(clippy::cast_precision_loss)] // counts are far below 2^52
        let dt = (total as f64 / self.life.budget as f64).max(1.0);
        for &i in &batch {
            // Whole ticks since the last visit, rounded at random.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // ≥ 1, capped
            let mut k = dt.floor().min(f64::from(u16::MAX)) as u16;
            if dt.fract() > 0.0 && self.life.chance(dt.fract()) {
                k = k.saturating_add(1);
            }
            self.visit(i as usize, k);
        }
        self.life.batch = batch;
    }

    /// One random tick of the living cell at index `i`, acting for `k`
    /// ticks of world time.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn visit(&mut self, i: usize, k: u16) {
        let cell = self.cells[i];
        let m = usize::from(cell.material.0);
        if self.life.flags[m] & LIVING == 0 {
            return; // changed since it was indexed
        }
        if self.visit_plant(i, cell, k) {
            return;
        }
        let (x, y) = (i % self.width, i / self.width);
        let b = &self.life.eco.behaviour[m];
        let (buffer, emit, decay, reactions) = (b.buffer, b.emit, b.decay, b.reactions.len());
        let (wilt, grows) = (b.wilt, b.grows.len());
        let kf = f32::from(k);
        if let Some(buf) = buffer {
            // The state counts units drawn from a full buffer, so a fresh
            // cell is full; amounts are whole units so cell and field agree.
            let field = self.life.field_mut(buf.field);
            let f = field.at_cell(x, y);
            let drawn = cell.state;
            let step = buf.rate * kf;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to u16
            let units = |v: f32| (v.max(0.0) * buf.units).floor().min(f32::from(u16::MAX)) as u16;
            if f > buf.level {
                let take = units(step.min(f - buf.level)).min(drawn);
                if take > 0 {
                    field.add_at_cell(x, y, -f32::from(take) / buf.units);
                    self.cells[i].state = drawn - take;
                }
            } else {
                let give = units(step.min(buf.level - f)).min(u16::MAX - drawn);
                if give > 0 {
                    field.add_at_cell(x, y, f32::from(give) / buf.units);
                    self.cells[i].state = drawn + give;
                }
            }
        }
        if let Some(e) = emit {
            let field = self.life.field_mut(e.field);
            let f = field.at_cell(x, y);
            if f < e.up_to {
                field.add_at_cell(x, y, (e.rate * kf).min(e.up_to - f));
            }
        }
        if let Some(d) = decay {
            let age = cell.state.saturating_add(k);
            if age >= d.after_ticks {
                self.write_cell(x, y, d.into, &mut None);
                self.release(d.release, x, y);
                return;
            }
            self.cells[i].state = age;
        }
        if let Some(wt) = wilt {
            let attached = wt.attached.is_empty()
                || DIRS.iter().any(|&(dx, dy)| {
                    self.at(x, y, dx, dy)
                        .is_some_and(|j| wt.attached.contains(self.cells[j].material))
                });
            if !attached || !self.gates_open(wt.unless, x, y) {
                // Failing: it may wilt, and doesn't grow or react meanwhile.
                if self.life.chance(1.0 - powu(wt.keep_per_tick, k)) {
                    self.write_cell(x, y, wt.into, &mut None);
                }
                return;
            }
        }
        for r in 0..grows {
            if self.grow(i, cell, r, k) {
                return;
            }
        }
        for r in 0..reactions {
            let rule = self.life.eco.behaviour[m].reactions[r];
            if !self.gates_open(rule.gates, x, y) {
                continue;
            }
            let mut found = [0usize; 4];
            let mut n = 0;
            for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
                if let Some(j) = self.at(x, y, dx, dy)
                    && self.cells[j].material == rule.b
                {
                    found[n] = j;
                    n += 1;
                }
            }
            if n == 0 {
                continue;
            }
            let p = 1.0 - powu(rule.keep_per_tick, k);
            if !self.life.chance(p) {
                continue;
            }
            self.write_cell(x, y, rule.into, &mut None);
            if let Some(b_into) = rule.b_into {
                let j = found[self.life.rng.next_u32() as usize % n];
                self.write_cell(j % self.width, j / self.width, b_into, &mut None);
            }
            self.release(rule.release, x, y);
            return;
        }
    }

    /// Tries `[[grow]]` rule `r` of the living cell `cell` at index `i`,
    /// acting for `k` ticks. Returns whether it grew.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn grow(&mut self, i: usize, cell: Cell, r: usize, k: u16) -> bool {
        let g = self.life.eco.behaviour[usize::from(cell.material.0)].grows[r];
        let (x, y) = (i % self.width, i / self.width);
        let count = if g.makes == cell.material {
            cell.state.saturating_add(1)
        } else {
            1
        };
        if count > g.max || !self.gates_open(g.needs, x, y) {
            return false;
        }
        let mut found = [0usize; 4];
        let mut n = 0;
        for (bit, &(dx, dy)) in DIRS.iter().enumerate() {
            if g.dirs & (1 << bit) == 0 {
                continue;
            }
            let Some(j) = self.at(x, y, dx, dy) else {
                continue;
            };
            if !g.into.contains(self.cells[j].material) {
                continue;
            }
            if !g.touching.is_empty() || g.alone {
                let (jx, jy) = (j % self.width, j / self.width);
                let (mut touches, mut lonely) = (g.touching.is_empty(), true);
                for &(ex, ey) in &DIRS {
                    let Some(e) = self.at(jx, jy, ex, ey).filter(|&e| e != i) else {
                        continue;
                    };
                    let em = self.cells[e].material;
                    touches |= g.touching.contains(em);
                    lonely &= em != g.makes;
                }
                if !touches || (g.alone && !lonely) {
                    continue;
                }
            }
            found[n] = j;
            n += 1;
        }
        if n == 0 {
            return false;
        }
        let keep = if g.boost.is_some_and(|b| self.gate_open(b, x, y)) {
            g.keep_boosted
        } else {
            g.keep_per_tick
        };
        if !self.life.chance(1.0 - powu(keep, k)) {
            return false;
        }
        let j = found[self.life.rng.next_u32() as usize % n];
        self.write_cell(j % self.width, j / self.width, g.makes, &mut None);
        self.cells[j].state = count;
        if let Some(b) = g.becomes {
            self.write_cell(x, y, b, &mut None);
            self.cells[i].state = 1;
        }
        if let Some(c) = g.consume {
            self.life.field_mut(c.field).add_at_cell(x, y, -c.amount);
        }
        true
    }

    /// Spores: each `[[spawn]]` makes its attempts for this tick, each at a
    /// random column's first non-fluid cell.
    fn run_spawns(&mut self) {
        for s in 0..self.life.eco.spawns.len() {
            let sp = self.life.eco.spawns[s];
            #[allow(clippy::cast_precision_loss)] // width ≤ 8192
            let expected = sp.per_column_tick * self.width as f64;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // ≤ width
            let mut n = expected.floor() as usize;
            if self.life.chance(expected.fract()) {
                n += 1;
            }
            for _ in 0..n {
                let x = self.life.rng.next_u32() as usize % self.width;
                let Some(y) = (0..self.height)
                    .find(|&y| !self.is_fluid(self.cells[y * self.width + x].material))
                else {
                    continue;
                };
                if y == 0
                    || !sp.on.contains(self.cells[y * self.width + x].material)
                    || Some(self.cells[(y - 1) * self.width + x].material) != self.life.water
                    || !self.gates_open(sp.when, x, y - 1)
                {
                    continue;
                }
                self.write_cell(x, y - 1, sp.material, &mut None);
            }
        }
    }

    fn gate_open(&self, g: Gate, x: usize, y: usize) -> bool {
        g.holds(self.life.field(g.field).at_cell(x, y))
    }

    fn gates_open(&self, gates: Gates, x: usize, y: usize) -> bool {
        gates.iter().all(|g| self.gate_open(g, x, y))
    }

    fn release(&mut self, r: Option<Release>, x: usize, y: usize) {
        if let Some(r) = r {
            self.life.field_mut(r.field).add_at_cell(x, y, r.amount);
        }
    }

    /// Bubblers: each cell with water above releases a bubble at the
    /// configured rate. A bubble turns into air when it pops at the surface,
    /// so to keep the water level, each release turns the first air cell
    /// above it back into water. In a tank full to the lid there is no air
    /// to return until the first bubbles pop there, so those first releases
    /// cost a cell of water each.
    fn run_devices(&mut self) {
        let cfg = self.life.eco.bubbler;
        let (Some(bubbler), Some(emits), Some(water)) = (cfg.material, cfg.emits, self.life.water)
        else {
            return;
        };
        if self.life.devices.total == 0 {
            return;
        }
        let p = f64::from(cfg.rate) / f64::from(super::TICKS_PER_SECOND);
        let mut batch = mem::take(&mut self.life.batch);
        batch.clear();
        for list in &self.life.devices.lists {
            batch.extend_from_slice(list);
        }
        for &i in &batch {
            let i = i as usize;
            if self.cells[i].material != bubbler {
                continue;
            }
            let (x, y) = (i % self.width, i / self.width);
            let Some(above) = self.at(x, y, 0, -1) else {
                continue;
            };
            if self.cells[above].material != water || !self.life.chance(p) {
                continue;
            }
            self.write_cell(x, y - 1, emits, &mut None);
            if let Some(air) = self.life.air
                && let Some(top) = (0..y - 1)
                    .rev()
                    .find(|&ty| self.cells[ty * self.width + x].material == air)
            {
                self.write_cell(x, top, water, &mut None);
            }
        }
        self.life.batch = batch;
    }

    /// Rebuilds the current from the filter outlets: each outlet's field
    /// cell starts a jet pointing away from the nearer side wall, fading
    /// over `reach` cells and widening as it goes.
    fn recompute_current(&mut self) {
        let life = &mut self.life;
        life.current.fill(0.0);
        life.current_blocks.clear();
        let Some(outlet) = life.eco.current.material else {
            return;
        };
        let cfg = life.eco.current;
        let (fw, fh) = (life.light.width(), life.light.height());
        let mut sources = BTreeSet::new();
        for list in &life.devices.lists {
            for &i in list {
                let i = i as usize;
                if self.cells[i].material == outlet {
                    let x = i % self.width;
                    let right = 2 * x + 1 < self.width;
                    sources.insert((i / self.width / FIELD_SCALE, x / FIELD_SCALE, right));
                }
            }
        }
        let reach = (cfg.reach as usize).div_ceil(FIELD_SCALE).max(1);
        let spread = cfg.spread as usize;
        for (fy, fx, right) in sources {
            for k in 0..reach {
                let Some(bx) = (if right {
                    fx.checked_add(k)
                } else {
                    fx.checked_sub(k)
                }) else {
                    break;
                };
                if bx >= fw {
                    break;
                }
                #[allow(clippy::cast_precision_loss)] // small counts
                let speed = cfg.speed * (1.0 - k as f32 / reach as f32);
                let half = k / spread;
                for by in fy.saturating_sub(half)..=(fy + half).min(fh - 1) {
                    let v = &mut life.current[by * fw + bx];
                    *v += if right { speed } else { -speed };
                }
            }
        }
        for (b, v) in life.current.iter_mut().enumerate() {
            *v = v.clamp(-cfg.speed, cfg.speed);
            if v.abs() >= MIN_CURRENT {
                life.current_blocks.push(b);
            }
        }
        // A resting field may now be carried by the new current, and a
        // resting floater set drifting (T6c part 2).
        life.nutrient.wake();
        life.co2.wake();
        life.version += 1;
        self.wake_rafts();
    }

    /// The sideways current at cell `(x, y)`, in cells per second (+ =
    /// rightwards; 0 outside the grid or with no current).
    pub(super) fn current_at_cell(&self, x: usize, y: usize) -> f32 {
        let life = &self.life;
        if life.current_blocks.is_empty() {
            return 0.0;
        }
        let fw = life.light.width();
        let (fx, fy) = (x / FIELD_SCALE, y / FIELD_SCALE);
        if fx < fw && fy < life.light.height() {
            life.current[fy * fw + fx]
        } else {
            0.0
        }
    }

    /// Moves floating and liquid cells along the current: each one swaps
    /// with the liquid next to it downstream with probability speed / 20
    /// per tick. The water itself is the medium: it never starts a swap (it
    /// would look the same moved), it only fills in behind what the current
    /// carries, so a jet through clear water changes no cells and wakes no
    /// chunks. Dense liquids (liquid fertiliser) only trade places with
    /// dense liquids, never air, so they stay in the water; a floater rides
    /// along the air at the surface, or through water while submerged, so
    /// it never lifts water into the air. The sweep runs upstream, so a carried cell moves at
    /// most once per tick and a blob moves together.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn run_current(&mut self) {
        if self.life.current_blocks.is_empty() {
            return;
        }
        let water = self.life.water;
        let fw = self.life.light.width();
        let blocks = mem::take(&mut self.life.current_blocks);
        for &b in &blocks {
            let v = self.life.current[b];
            let p = f64::from(v.abs()) / f64::from(super::TICKS_PER_SECOND);
            let dir: isize = if v > 0.0 { 1 } else { -1 };
            let (x0, y0) = ((b % fw) * FIELD_SCALE, (b / fw) * FIELD_SCALE);
            let (x1, y1) = (
                (x0 + FIELD_SCALE).min(self.width),
                (y0 + FIELD_SCALE).min(self.height),
            );
            for y in y0..y1 {
                for k in 0..x1 - x0 {
                    let x = if dir > 0 { x1 - 1 - k } else { x0 + k };
                    let i = y * self.width + x;
                    let m = self.cells[i].material;
                    let floating = self.props[usize::from(m.0)].class == MovementClass::Floating;
                    let watery = self.life.flags[usize::from(m.0)] & WATERY != 0;
                    if !(floating || watery && Some(m) != water) {
                        continue;
                    }
                    let Some(j) = self.at(x, y, dir, 0) else {
                        continue;
                    };
                    let t = self.cells[j].material;
                    let t_watery = self.life.flags[usize::from(t.0)] & WATERY != 0;
                    // A floater keeps to its layer: along the air at the
                    // surface, through water when submerged.
                    let submerged = self.at(x, y, 0, -1).is_some_and(|a| {
                        self.life.flags[usize::from(self.cells[a].material.0)] & WATERY != 0
                    });
                    let ok = t != m
                        && self.props[usize::from(t.0)].class == MovementClass::Liquid
                        && if floating {
                            t_watery == submerged
                        } else {
                            t_watery
                        };
                    if ok && self.life.chance(p) {
                        self.swap(i, j);
                    }
                }
            }
        }
        self.life.current_blocks = blocks;
    }

    fn step_fields(&mut self) {
        if self.life.any_opacity_dirty {
            self.update_light();
        }
        let life = &mut self.life;
        let current: &[f32] = if life.current_blocks.is_empty() {
            &[]
        } else {
            &life.current
        };
        let (n, c) = (life.eco.nutrient, life.eco.co2);
        let a = life.nutrient.step(n.diffusion, n.decay, current);
        let b = life.co2.step(c.diffusion, c.decay, current);
        if a || b {
            life.version += 1;
        }
    }

    /// Recounts opaque and watery cells in the dirty field cells and
    /// recomputes light down every column that had one.
    fn update_light(&mut self) {
        let life = &mut self.life;
        life.any_opacity_dirty = false;
        let (fw, fh) = (life.light.width(), life.light.height());
        let mut columns = mem::take(&mut life.columns);
        columns.clear();
        columns.resize(fw, false);
        for b in 0..fw * fh {
            if !mem::take(&mut life.opacity_dirty[b]) {
                continue;
            }
            columns[b % fw] = true;
            let (x0, y0) = ((b % fw) * FIELD_SCALE, (b / fw) * FIELD_SCALE);
            let (x1, y1) = (
                (x0 + FIELD_SCALE).min(self.width),
                (y0 + FIELD_SCALE).min(self.height),
            );
            let (mut opaque, mut watery) = (0u32, 0u32);
            for y in y0..y1 {
                for c in &self.cells[y * self.width + x0..y * self.width + x1] {
                    let m = usize::from(c.material.0);
                    opaque += u32::from(life.opacity_of[m]);
                    watery += u32::from(life.flags[m] & WATERY != 0);
                }
            }
            #[allow(clippy::cast_precision_loss)] // ≤ 64
            let area = ((x1 - x0) * (y1 - y0)) as f32;
            #[allow(clippy::cast_precision_loss)] // ≤ 64 × 255
            let frac = (opaque as f32 / (area * 255.0), watery as f32 / area);
            life.opacity[b] = frac;
        }
        let cfg = life.eco.light;
        let mut changed = false;
        for (fx, _) in columns.iter().enumerate().filter(|(_, d)| **d) {
            let mut l = cfg.surface;
            for fy in 0..fh {
                let b = fy * fw + fx;
                if life.light.values()[b].to_bits() != l.to_bits() {
                    life.light.set(b, l);
                    changed = true;
                }
                let (o, w) = life.opacity[b];
                l *= (1.0 - cfg.opaque * o) * (1.0 - cfg.water * w);
            }
        }
        life.columns = columns;
        if changed {
            life.version += 1;
        }
    }
}

/// The water-quality score (SPEC §2.3) from a mean nutrient level over
/// `water` water cells holding `pollutants` pollutant cells, with `plants`
/// plant cells in the tank.
fn quality_score(
    eco: &Ecology,
    mean_nutrient: f32,
    water: usize,
    pollutants: usize,
    plants: usize,
) -> f32 {
    let q = eco.quality;
    #[allow(clippy::cast_precision_loss)] // counts ≪ 2^24
    let per_mille = |n: usize| n as f32 * 1000.0 / water.max(1) as f32;
    let excess =
        ((mean_nutrient - q.nutrient_ok) / (q.nutrient_bad - q.nutrient_ok)).clamp(0.0, 1.0);
    let pollution = (per_mille(pollutants) / q.pollution_bad).min(1.0);
    let plants = (per_mille(plants) / q.plant_full).min(1.0);
    (1.0 - q.nutrient_weight * excess - q.pollution_weight * pollution + q.plant_bonus * plants)
        .clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::LIVING;
    use crate::ecology::powu;
    use crate::{Brush, MaterialTable, Placeable, Rect, World, WorldConfig};

    /// Every living cell, by a full scan, in index order.
    fn scan(w: &World) -> Vec<u32> {
        (0..w.cells.len())
            .filter(|&i| w.life.flags[usize::from(w.cells[i].material.0)] & LIVING != 0)
            .map(|i| u32::try_from(i).unwrap())
            .collect()
    }

    fn listed(w: &World) -> Vec<u32> {
        let mut v: Vec<u32> = w.life.living.lists.iter().flatten().copied().collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn registries_match_a_full_rescan_and_report_only_real_changes() {
        let config = WorldConfig {
            width: 192,
            height: 128,
            seed: 4,
            water_level: 0.9,
        };
        let mut w = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
        let t = w.materials().clone();
        let id = |n: &str| t.id(n).unwrap();
        w.fill_rect(
            Rect {
                x0: 0,
                y0: 120,
                x1: 192,
                y1: 128,
            },
            id("aquasoil"),
            Brush::Replace,
        )
        .unwrap();
        w.place(Placeable::Bubbler, 100, 60).unwrap();
        for k in 0..400u32 {
            if k % 20 == 0 {
                let x = i64::from(k % 180) + 5;
                w.paint(
                    x,
                    15,
                    2,
                    id(["waste", "liquid_fertiliser", "detritus"][(k / 20) as usize % 3]),
                    Brush::Pour,
                )
                .unwrap();
            }
            w.tick();
            // Every living cell is listed. Cells a visit converted this
            // tick stay listed until the next refresh (visits skip them).
            let (listed, living) = (listed(&w), scan(&w));
            assert_eq!(w.life.living.total, listed.len());
            // Cells grown or spawned this tick (only grow and spawn
            // products) are listed at the next refresh, which rescans the
            // rects they changed.
            let mut products = crate::ecology::MaterialSet::default();
            for b in &w.life.eco.behaviour {
                for g in &b.grows {
                    products.insert(g.makes);
                    if let Some(m) = g.becomes {
                        products.insert(m);
                    }
                }
            }
            for sp in &w.life.eco.spawns {
                products.insert(sp.material);
            }
            let pending = |i: u32| {
                let (x, y) = (i as usize % w.width, i as usize / w.width);
                products.contains(w.cells[i as usize].material)
                    && w.chunks.iter().any(|c| c.changed_next.contains(x, y))
            };
            let missing = living
                .iter()
                .filter(|&&i| listed.binary_search(&i).is_err() && !pending(i))
                .count();
            assert_eq!(missing, 0, "tick {k}: living cells not listed");
            let mut dedup = listed.clone();
            dedup.dedup();
            assert_eq!(dedup.len(), listed.len(), "tick {k}: listed twice");
            assert!(
                // New products can make `living` the longer list.
                listed.len().saturating_sub(living.len()) <= 256,
                "tick {k}: too many stale"
            );
        }
        // Rescanning a rect whose cells didn't change reports no change.
        let r = Rect {
            x0: 0,
            y0: 64,
            x1: 64,
            y1: 128,
        };
        let (width, cells, flags) = (w.width, w.cells.clone(), w.life.flags.clone());
        assert!(
            !w.life
                .living
                .refresh(2, r.intersect(w.chunks[2].bounds), width, &cells, &flags)
        );
        assert!(
            !w.life
                .devices
                .refresh(4, w.chunks[4].bounds, width, &cells, &flags)
        );
    }

    #[test]
    fn placing_an_outlet_wakes_a_resting_field() {
        let config = WorldConfig {
            width: 256,
            height: 64,
            seed: 4,
            water_level: 1.0,
        };
        let mut w = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
        // A uniform field rests at once.
        for y in (0..64).step_by(8) {
            for x in (0..256).step_by(8) {
                w.add_to_field(crate::FieldKind::Nutrient, x, y, 1.0);
            }
        }
        for _ in 0..8 {
            w.tick();
        }
        assert!(!w.life.nutrient.is_active(), "the field came to rest");
        let before = w.field(crate::FieldKind::Nutrient).values().to_vec();
        w.place(Placeable::FilterOutlet, 4, 12).unwrap();
        for _ in 0..40 {
            w.tick();
        }
        assert_ne!(w.field(crate::FieldKind::Nutrient).values(), &before[..]);
    }

    #[test]
    fn powu_matches_powi() {
        for k in [0u16, 1, 2, 7, 20, 1000] {
            assert!((powu(0.97, k) - 0.97f64.powi(i32::from(k))).abs() < 1e-12);
        }
    }
}
