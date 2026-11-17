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
use crate::ecology::{Ecology, Gate, Release};
use crate::field::{FIELD_EVERY, FIELD_SCALE, Field, FieldKind};
use crate::grid::{Cell, Rect};
use crate::hash::Fnv64;
use crate::material::{MaterialId, MaterialTable, MovementClass};
use crate::rng::chunk_stream;

/// Living cells visited per tick by default (SPEC §2.1: 256 per display).
pub const RANDOM_TICK_BUDGET: usize = 256;

/// Stream id for the life RNG: above any chunk id.
const LIFE_STREAM: u64 = u64::MAX;
/// Weakest current, in cells per second, that still moves cells.
const MIN_CURRENT: f32 = 0.25;

const LIVING: u8 = 1;
const DEVICE: u8 = 2;
/// Blocks light: solids, substrate, food.
const OPAQUE: u8 = 4;
/// A dense liquid such as water (not air).
const WATERY: u8 = 8;
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

/// Per-chunk lists of cell indices whose material has a flag.
#[derive(Clone, Debug)]
struct Registry {
    flag: u8,
    lists: Vec<Vec<u32>>,
    total: usize,
}

impl Registry {
    fn new(flag: u8, chunks: usize) -> Self {
        Self {
            flag,
            lists: vec![Vec::new(); chunks],
            total: 0,
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
        let mut changed = false;
        list.retain(|&i| {
            let i = i as usize;
            let keep = !rect.contains(i % width, i / width);
            changed |= !keep;
            keep
        });
        for y in rect.y0..rect.y1 {
            let row = y * width;
            for x in rect.x0..rect.x1 {
                if flags[usize::from(cells[row + x].material.0)] & self.flag != 0 {
                    #[allow(clippy::cast_possible_truncation)] // grids are ≤ 8192²
                    list.push((row + x) as u32);
                    changed = true;
                }
            }
        }
        self.total = self.total + list.len() - before;
        changed
    }
}

/// Life state owned by the world.
#[derive(Clone, Debug)]
pub(super) struct Life {
    eco: Ecology,
    flags: Vec<u8>,
    living: Registry,
    devices: Registry,
    budget: usize,
    cursor: usize,
    stats: RandomTickStats,
    rng: Pcg32,
    /// Scratch list of cell indices for one batch.
    batch: Vec<u32>,
    nutrient: Field,
    co2: Field,
    light: Field,
    /// Per field cell: fraction of opaque and of watery cells.
    opacity: Vec<(f32, f32)>,
    /// Field cells whose opacity needs recounting.
    opacity_dirty: Vec<bool>,
    any_opacity_dirty: bool,
    /// Sideways current per field cell, cells per second (+ = rightwards).
    current: Vec<f32>,
    /// Field cells with a current strong enough to move cells.
    current_blocks: Vec<usize>,
    devices_dirty: bool,
    /// Bumped whenever a field changes, for renderers.
    version: u64,
    water: Option<MaterialId>,
    air: Option<MaterialId>,
}

impl Life {
    pub(super) fn new(
        materials: &MaterialTable,
        eco: Ecology,
        width: usize,
        height: usize,
        chunks: usize,
        seed: u64,
    ) -> Self {
        let mut flags = vec![0u8; 256];
        for m in materials.iter() {
            let f = &mut flags[usize::from(m.id.0)];
            if eco.behaviour[usize::from(m.id.0)].is_living() {
                *f |= LIVING;
            }
            if Some(m.id) == eco.bubbler.material || Some(m.id) == eco.current.material {
                *f |= DEVICE;
            }
            if !m.class.is_fluid() {
                *f |= OPAQUE;
            }
            if m.class == MovementClass::Liquid && m.density >= WATERY_DENSITY {
                *f |= WATERY;
            }
        }
        let field = Field::new(width, height);
        let blocks = field.width() * field.height();
        Self {
            eco,
            flags,
            living: Registry::new(LIVING, chunks),
            devices: Registry::new(DEVICE, chunks),
            budget: RANDOM_TICK_BUDGET,
            cursor: 0,
            stats: RandomTickStats::default(),
            rng: chunk_stream(seed, 0, LIFE_STREAM),
            batch: Vec::new(),
            nutrient: field.clone(),
            co2: field.clone(),
            light: field,
            opacity: vec![(0.0, 0.0); blocks],
            opacity_dirty: vec![true; blocks],
            any_opacity_dirty: true,
            current: vec![0.0; blocks],
            current_blocks: Vec::new(),
            devices_dirty: false,
            version: 0,
            water: materials.id("water"),
            air: materials.id("air"),
        }
    }

    fn field_mut(&mut self, kind: FieldKind) -> &mut Field {
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

    fn chance(&mut self, p: f64) -> bool {
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
        w.life = Life::new(
            &w.materials,
            ecology,
            w.width,
            w.height,
            w.chunks.len(),
            w.seed,
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
    /// [`RANDOM_TICK_BUDGET`]). At least 1.
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
        if self.tick.is_multiple_of(FIELD_EVERY) {
            self.step_fields();
        }
    }

    fn refresh_registries(&mut self, ci: usize, r: Rect) {
        let life = &mut self.life;
        life.living
            .refresh(ci, r, self.width, &self.cells, &life.flags);
        if life
            .devices
            .refresh(ci, r, self.width, &self.cells, &life.flags)
        {
            life.devices_dirty = true;
        }
        let fw = life.light.width();
        for fy in r.y0 / FIELD_SCALE..=(r.y1 - 1) / FIELD_SCALE {
            for fx in r.x0 / FIELD_SCALE..=(r.x1 - 1) / FIELD_SCALE {
                life.opacity_dirty[fy * fw + fx] = true;
            }
        }
        life.any_opacity_dirty = true;
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
        let (x, y) = (i % self.width, i / self.width);
        let b = &self.life.eco.behaviour[m];
        let (buffer, emit, decay, reactions) = (b.buffer, b.emit, b.decay, b.reactions.len());
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
        for r in 0..reactions {
            let rule = self.life.eco.behaviour[m].reactions[r];
            if let Some(g) = rule.gate
                && !self.gate_open(g, x, y)
            {
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

    fn gate_open(&self, g: Gate, x: usize, y: usize) -> bool {
        let v = self.life.field(g.field).at_cell(x, y);
        if g.below { v < g.value } else { v > g.value }
    }

    fn release(&mut self, r: Option<Release>, x: usize, y: usize) {
        if let Some(r) = r {
            self.life.field_mut(r.field).add_at_cell(x, y, r.amount);
        }
    }

    /// Bubblers: each cell with water above releases a bubble at the
    /// configured rate. A bubble turns into air when it pops at the surface,
    /// so to keep the water level, each release turns the first air cell
    /// above it back into water.
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
        life.version += 1;
    }

    /// Moves floating and liquid cells along the current: each one swaps
    /// with the liquid next to it downstream with probability speed / 20
    /// per tick. The water itself is the medium: it never starts a swap (it
    /// would look the same moved), it only fills in behind what the current
    /// carries, so a jet through clear water changes no cells and wakes no
    /// chunks. Dense liquids (liquid fertiliser) only trade places with
    /// dense liquids, never air, so they stay in the water; a floater rides
    /// along on either. The sweep runs upstream, so a carried cell moves at
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
                    let ok = t != m
                        && self.props[usize::from(t.0)].class == MovementClass::Liquid
                        && (floating || self.life.flags[usize::from(t.0)] & WATERY != 0);
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
        let mut columns = vec![false; fw];
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
                    let f = life.flags[usize::from(c.material.0)];
                    opaque += u32::from(f & OPAQUE != 0);
                    watery += u32::from(f & WATERY != 0);
                }
            }
            #[allow(clippy::cast_precision_loss)] // ≤ 64
            let area = ((x1 - x0) * (y1 - y0)) as f32;
            #[allow(clippy::cast_precision_loss)] // ≤ 64
            let frac = (opaque as f32 / area, watery as f32 / area);
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
        if changed {
            life.version += 1;
        }
    }
}

/// `base^k` by squaring: plain multiplications, so it is the same on every
/// IEEE platform (unlike `powi`).
fn powu(base: f64, mut k: u16) -> f64 {
    let (mut acc, mut b) = (1.0, base);
    while k > 0 {
        if k & 1 == 1 {
            acc *= b;
        }
        b *= b;
        k >>= 1;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::powu;

    #[test]
    fn powu_matches_powi() {
        for k in [0u16, 1, 2, 7, 20, 1000] {
            assert!((powu(0.97, k) - 0.97f64.powi(i32::from(k))).abs() < 1e-12);
        }
    }
}
