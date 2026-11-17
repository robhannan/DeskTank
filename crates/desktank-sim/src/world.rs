//! The cell world: grid, chunk sleep and wake, dirty rectangles, the
//! movement pass, the edit API and the state hash (SPEC §2.1).

use core::mem;

use rand_core::Rng as _;
use rand_pcg::Pcg32;
use thiserror::Error;

use crate::grid::{Cell, Rect};
use crate::hash::Fnv64;
use crate::material::{MAX_RUN, MaterialId, MaterialTable, MovementClass};
use crate::rng::{chunk_stream, splitmix64};

/// Side of a square chunk, in cells.
pub const CHUNK_SIZE: usize = 64;
/// Simulation ticks per second of world time (a fixed 1/20 s tick).
pub const TICKS_PER_SECOND: u32 = 20;
/// Ticks without change after which a chunk falls asleep (1 s).
pub const SLEEP_TICKS: u32 = TICKS_PER_SECOND;
/// How far a blocked liquid looks sideways for somewhere to fall, in cells.
/// Sets how level a settled surface is; at most half a chunk, so a change
/// still reaches only direct neighbour chunks.
pub const LEVEL_REACH: usize = 32;
/// Largest grid side, in cells.
pub const MAX_SIDE: usize = 8192;

/// Parameters for a new world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldConfig {
    /// Width in cells.
    pub width: usize,
    /// Height in cells.
    pub height: usize,
    /// Seed for every random stream.
    pub seed: u64,
    /// Water level as a fraction of the height, from the floor: 1.0 is a
    /// full tank, 0.96 the default (D12). Above it is air.
    pub water_level: f32,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            width: 640,
            height: 360,
            seed: 0,
            water_level: 0.96,
        }
    }
}

/// Error creating or editing a world.
#[derive(Debug, Error, PartialEq)]
pub enum WorldError {
    /// The grid is empty or larger than [`MAX_SIDE`].
    #[error("grid size {width}x{height} is outside 1..=8192 per side")]
    BadSize {
        /// Requested width.
        width: usize,
        /// Requested height.
        height: usize,
    },
    /// The water level isn't within `0.0..=1.0`.
    #[error("water level {0} is outside 0..=1")]
    BadWaterLevel(f32),
    /// The material table lacks a material the world needs.
    #[error("material table has no {0:?} material")]
    MissingMaterial(&'static str),
    /// The material ID isn't in the table.
    #[error("unknown material id {}", .0.0)]
    UnknownMaterial(MaterialId),
    /// The cell is outside the grid.
    #[error("cell ({x}, {y}) is outside the grid")]
    OutOfBounds {
        /// Column.
        x: usize,
        /// Row.
        y: usize,
    },
}

/// How a brush treats the cells it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brush {
    /// Overwrite every cell.
    Replace,
    /// Only fill fluid cells (water, air, bubbles): pouring never
    /// overwrites substrate or rock.
    Pour,
}

/// Movement properties looked up per cell, indexed by material ID.
#[derive(Clone, Copy, Debug)]
struct Props {
    class: MovementClass,
    density: f32,
    float_density: f32,
    dispersion: u8,
    /// Mean sideways run per row of drop, in 1/256 cells: `256 / tan(repose)`.
    run_k: u32,
    float_ticks: u16,
    pops_into: Option<MaterialId>,
}

/// Unknown IDs behave as immovable rock.
const UNKNOWN: Props = Props {
    class: MovementClass::Solid,
    density: f32::INFINITY,
    float_density: f32::INFINITY,
    dispersion: 0,
    run_k: 256,
    float_ticks: 0,
    pops_into: None,
};

#[derive(Clone, Copy, Debug)]
struct Chunk {
    bounds: Rect,
    /// Cells to update this tick.
    work: Rect,
    /// Cells to update next tick: changed cells plus their reach.
    work_next: Rect,
    /// Cells that changed during the last tick (for GPU uploads).
    changed: Rect,
    changed_next: Rect,
    idle_ticks: u32,
    awake: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Fall,
    Rise,
}

/// A deterministic falling-sand world.
#[derive(Clone)]
pub struct World {
    width: usize,
    height: usize,
    seed: u64,
    tick: u64,
    /// Low 32 bits of the tick being computed; marks cells already moved.
    stamp: u32,
    surface_row: usize,
    /// Horizontal reach of one cell update: max dispersion or slide run.
    reach_x: usize,
    materials: MaterialTable,
    props: Vec<Props>,
    cells: Vec<Cell>,
    moved: Vec<u32>,
    chunks_x: usize,
    chunks_y: usize,
    chunks: Vec<Chunk>,
    rngs: Vec<Pcg32>,
    last_change_tick: u64,
}

impl World {
    /// A tank filled with water up to the configured level and air above.
    /// Every chunk starts asleep.
    ///
    /// # Errors
    ///
    /// Returns an error if the size or water level is out of range, or the
    /// table has no `water` or `air` material.
    pub fn new(config: WorldConfig, materials: MaterialTable) -> Result<Self, WorldError> {
        let WorldConfig {
            width,
            height,
            seed,
            water_level,
        } = config;
        if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) {
            return Err(WorldError::BadSize { width, height });
        }
        if !(0.0..=1.0).contains(&water_level) {
            return Err(WorldError::BadWaterLevel(water_level));
        }
        let water = materials
            .id("water")
            .ok_or(WorldError::MissingMaterial("water"))?;
        let air = materials
            .id("air")
            .ok_or(WorldError::MissingMaterial("air"))?;

        let mut props = vec![UNKNOWN; 256];
        let mut reach_x = 1;
        for m in materials.iter() {
            let run_k = run_k(m.repose);
            props[usize::from(m.id.0)] = Props {
                class: m.class,
                density: m.density,
                float_density: m.float_density,
                dispersion: m.dispersion,
                run_k,
                float_ticks: float_ticks(m.float_time),
                pops_into: m.pops_into,
            };
            // A powder looks up to ⌈run⌉ cells sideways, a fluid up to
            // LEVEL_REACH.
            let flows = matches!(m.class, MovementClass::Liquid | MovementClass::Buoyant);
            if flows && m.dispersion > 0 {
                reach_x = reach_x.max(LEVEL_REACH);
            }
            reach_x = reach_x.max(run_k.div_ceil(256) as usize);
        }

        let water_rows = water_rows(height, water_level);
        let surface_row = height - water_rows;
        let mut cells = Vec::with_capacity(width * height);
        for y in 0..height {
            let m = if y >= surface_row { water } else { air };
            for x in 0..width {
                cells.push(Cell::new(m, variant(seed, 0, y * width + x)));
            }
        }

        let chunks_x = width.div_ceil(CHUNK_SIZE);
        let chunks_y = height.div_ceil(CHUNK_SIZE);
        let mut chunks = Vec::with_capacity(chunks_x * chunks_y);
        for cy in 0..chunks_y {
            for cx in 0..chunks_x {
                let bounds = Rect {
                    x0: cx * CHUNK_SIZE,
                    y0: cy * CHUNK_SIZE,
                    x1: ((cx + 1) * CHUNK_SIZE).min(width),
                    y1: ((cy + 1) * CHUNK_SIZE).min(height),
                };
                chunks.push(Chunk {
                    bounds,
                    work: Rect::EMPTY,
                    work_next: Rect::EMPTY,
                    changed: Rect::EMPTY,
                    changed_next: Rect::EMPTY,
                    idle_ticks: SLEEP_TICKS,
                    awake: false,
                });
            }
        }
        let rngs = (0..chunks.len() as u64)
            .map(|c| chunk_stream(seed, 0, c))
            .collect();

        Ok(Self {
            width,
            height,
            seed,
            tick: 0,
            stamp: 0,
            surface_row,
            reach_x,
            materials,
            props,
            cells,
            moved: vec![0; width * height],
            chunks_x,
            chunks_y,
            chunks,
            rngs,
            last_change_tick: 0,
        })
    }

    /// Width in cells.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in cells.
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Ticks simulated so far.
    #[must_use]
    pub fn tick_count(&self) -> u64 {
        self.tick
    }

    /// World time in seconds (ticks / [`TICKS_PER_SECOND`]).
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // exact below 2^53 ticks
    pub fn world_time(&self) -> f64 {
        self.tick as f64 / f64::from(TICKS_PER_SECOND)
    }

    /// The configured top water row: rows at or below it start as water.
    #[must_use]
    pub fn surface_row(&self) -> usize {
        self.surface_row
    }

    /// The material table.
    #[must_use]
    pub fn materials(&self) -> &MaterialTable {
        &self.materials
    }

    /// All cells, row-major from the top-left.
    #[must_use]
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// The cell at `(x, y)`, if inside the grid.
    #[must_use]
    pub fn cell(&self, x: usize, y: usize) -> Option<Cell> {
        (x < self.width && y < self.height).then(|| self.cells[y * self.width + x])
    }

    /// Number of chunks currently awake.
    #[must_use]
    pub fn awake_chunks(&self) -> usize {
        self.chunks.iter().filter(|c| c.awake).count()
    }

    /// Number of chunks.
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Rectangles of cells that changed during the last tick, one per chunk
    /// with changes, in chunk order. The renderer uploads only these.
    pub fn dirty_rects(&self) -> impl Iterator<Item = Rect> + '_ {
        self.chunks
            .iter()
            .map(|c| c.changed)
            .filter(|r| !r.is_empty())
    }

    /// The last tick in which any cell changed, counting edits made just
    /// before it (0 if none has).
    #[must_use]
    pub fn last_change_tick(&self) -> u64 {
        self.last_change_tick
    }

    /// Cell count per material ID.
    #[must_use]
    pub fn material_counts(&self) -> Vec<usize> {
        let mut counts = vec![0; 256];
        for c in &self.cells {
            counts[usize::from(c.material.0)] += 1;
        }
        counts
    }

    /// A stable 64-bit hash of the size, tick and every cell.
    #[must_use]
    pub fn state_hash(&self) -> u64 {
        let mut h = Fnv64::new();
        h.write(&(self.width as u64).to_le_bytes());
        h.write(&(self.height as u64).to_le_bytes());
        h.write(&self.tick.to_le_bytes());
        for c in &self.cells {
            let [s0, s1] = c.state.to_le_bytes();
            h.write(&[c.material.0, c.variant, s0, s1]);
        }
        h.finish()
    }

    /// Sets one cell to `material` (fresh state, deterministic colour
    /// noise) and wakes its chunk and any neighbour within reach.
    ///
    /// # Errors
    ///
    /// Returns an error if the cell is outside the grid or the material is
    /// unknown.
    pub fn set_cell(&mut self, x: usize, y: usize, material: MaterialId) -> Result<(), WorldError> {
        self.check_material(material)?;
        if x >= self.width || y >= self.height {
            return Err(WorldError::OutOfBounds { x, y });
        }
        self.write_cell(x, y, material);
        Ok(())
    }

    /// Paints a disc of `radius` cells centred on `(cx, cy)`, clipped to the
    /// grid. Returns how many cells changed.
    ///
    /// # Errors
    ///
    /// Returns an error if the material is unknown.
    pub fn paint(
        &mut self,
        cx: i64,
        cy: i64,
        radius: u32,
        material: MaterialId,
        brush: Brush,
    ) -> Result<usize, WorldError> {
        self.check_material(material)?;
        // A radius beyond the grid diagonal covers it all; clamping keeps the
        // i64 maths far from overflow and the loop clipped to the grid.
        #[allow(clippy::cast_possible_wrap)] // MAX_SIDE is small
        let r = i64::from(radius).min(2 * MAX_SIDE as i64);
        #[allow(clippy::cast_possible_wrap)] // sides are at most MAX_SIDE
        let clip = |c: i64, len: usize| {
            let lo = (c - r).max(0);
            let hi = (c + r).min(len as i64 - 1);
            lo..=hi
        };
        let mut changed = 0;
        for y in clip(cy, self.height) {
            for x in clip(cx, self.width) {
                let (dx, dy) = (x - cx, y - cy);
                if dx * dx + dy * dy > r * r + r {
                    continue;
                }
                if let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y))
                    && self.brush_cell(x, y, material, brush)
                {
                    changed += 1;
                }
            }
        }
        Ok(changed)
    }

    /// Fills a rectangle (clipped to the grid). Returns how many cells
    /// changed.
    ///
    /// # Errors
    ///
    /// Returns an error if the material is unknown.
    pub fn fill_rect(
        &mut self,
        rect: Rect,
        material: MaterialId,
        brush: Brush,
    ) -> Result<usize, WorldError> {
        self.check_material(material)?;
        let r = rect.intersect(Rect {
            x0: 0,
            y0: 0,
            x1: self.width,
            y1: self.height,
        });
        let mut changed = 0;
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                if self.brush_cell(x, y, material, brush) {
                    changed += 1;
                }
            }
        }
        Ok(changed)
    }

    /// Advances the world by one fixed tick (1/20 s of world time).
    pub fn tick(&mut self) {
        self.tick += 1;
        #[allow(clippy::cast_possible_truncation)] // wraps after 2^32 ticks (6.8 years)
        let stamp = self.tick as u32;
        self.stamp = stamp;
        for (i, c) in self.chunks.iter_mut().enumerate() {
            c.work = mem::take(&mut c.work_next);
            if !c.work.is_empty() {
                self.rngs[i] = chunk_stream(self.seed, self.tick, i as u64);
            }
        }
        // Even ticks sweep rows left to right, odd ticks right to left.
        let rightward = self.tick.is_multiple_of(2);
        // Falling materials: bottom-up, so a column falls together.
        for cy in (0..self.chunks_y).rev() {
            let band = self.chunks[cy * self.chunks_x].bounds;
            for y in (band.y0..band.y1).rev() {
                self.sweep_row(cy, y, rightward, Pass::Fall);
            }
        }
        // Buoyant materials: top-down, so a column rises together.
        for cy in 0..self.chunks_y {
            let band = self.chunks[cy * self.chunks_x].bounds;
            for y in band.y0..band.y1 {
                self.sweep_row(cy, y, rightward, Pass::Rise);
            }
        }
        // Edits since the last tick are already in `changed_next`.
        let mut any_change = false;
        for c in &mut self.chunks {
            c.changed = mem::take(&mut c.changed_next);
            any_change |= !c.changed.is_empty();
            if c.work_next.is_empty() {
                c.idle_ticks = c.idle_ticks.saturating_add(1);
                if c.idle_ticks >= SLEEP_TICKS {
                    c.awake = false;
                }
            } else {
                c.idle_ticks = 0;
                c.awake = true;
            }
        }
        if any_change {
            self.last_change_tick = self.tick;
        }
    }

    fn check_material(&self, material: MaterialId) -> Result<(), WorldError> {
        self.materials
            .get(material)
            .map(|_| ())
            .ok_or(WorldError::UnknownMaterial(material))
    }

    fn brush_cell(&mut self, x: usize, y: usize, material: MaterialId, brush: Brush) -> bool {
        let current = self.cells[y * self.width + x].material;
        if current == material
            || (brush == Brush::Pour && !self.props[usize::from(current.0)].class.is_fluid())
        {
            return false;
        }
        self.write_cell(x, y, material);
        true
    }

    fn write_cell(&mut self, x: usize, y: usize, material: MaterialId) {
        let i = y * self.width + x;
        self.cells[i] = Cell::new(material, variant(self.seed, self.tick, i));
        self.mark_changed(x, y);
    }

    fn sweep_row(&mut self, cy: usize, y: usize, rightward: bool, pass: Pass) {
        for k in 0..self.chunks_x {
            let cx = if rightward { k } else { self.chunks_x - 1 - k };
            let ci = cy * self.chunks_x + cx;
            let w = self.chunks[ci].work;
            if w.is_empty() || y < w.y0 || y >= w.y1 {
                continue;
            }
            if rightward {
                for x in w.x0..w.x1 {
                    self.update_cell(x, y, ci, pass);
                }
            } else {
                for x in (w.x0..w.x1).rev() {
                    self.update_cell(x, y, ci, pass);
                }
            }
        }
    }

    fn update_cell(&mut self, x: usize, y: usize, ci: usize, pass: Pass) {
        let i = y * self.width + x;
        if self.moved[i] == self.stamp {
            return;
        }
        let cell = self.cells[i];
        let p = self.props[usize::from(cell.material.0)];
        match (pass, p.class) {
            (Pass::Fall, MovementClass::Powder) => self.powder(x, y, ci, p.density, p.run_k),
            (Pass::Fall, MovementClass::Liquid) => {
                self.flow(x, y, ci, p.density, p.dispersion, 1);
            }
            (Pass::Fall, MovementClass::Floating) => self.floating(x, y, ci, cell, p),
            (Pass::Rise, MovementClass::Buoyant) => {
                if let Some(into) = p.pops_into
                    && self.pops(x, y, into)
                {
                    self.cells[i] = Cell::new(into, cell.variant);
                    self.moved[i] = self.stamp;
                    self.mark_changed(x, y);
                    return;
                }
                self.flow(x, y, ci, p.density, p.dispersion, -1);
            }
            _ => {}
        }
    }

    /// Index of `(x + dx, y + dy)`, if inside the grid.
    #[inline]
    fn at(&self, x: usize, y: usize, dx: isize, dy: isize) -> Option<usize> {
        let nx = x.checked_add_signed(dx)?;
        let ny = y.checked_add_signed(dy)?;
        (nx < self.width && ny < self.height).then_some(ny * self.width + nx)
    }

    /// The index of `(x + dx, y + dy)` if a mover of `density` travelling
    /// down (`falling`) or up can swap into it: it must be a liquid that is
    /// lighter (falling) or heavier (rising). Equal densities never swap.
    ///
    /// A liquid displaced earlier this tick can be entered again, so a
    /// column falls (or rises) together; the displaced liquid can then be
    /// carried more than one cell in a tick, but never acts itself twice.
    #[inline]
    fn enterable(
        &self,
        x: usize,
        y: usize,
        dx: isize,
        dy: isize,
        density: f32,
        falling: bool,
    ) -> Option<usize> {
        let j = self.at(x, y, dx, dy)?;
        let p = self.props[usize::from(self.cells[j].material.0)];
        let lighter_way = if falling {
            p.density < density
        } else {
            p.density > density
        };
        (p.class.is_displaceable() && lighter_way).then_some(j)
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.cells.swap(i, j);
        self.moved[i] = self.stamp;
        self.moved[j] = self.stamp;
        self.mark_changed(i % self.width, i / self.width);
        self.mark_changed(j % self.width, j / self.width);
    }

    /// Random -1 or +1 from the chunk's stream.
    #[inline]
    fn coin(&mut self, ci: usize) -> isize {
        if self.rngs[ci].next_u32() & 1 == 0 {
            -1
        } else {
            1
        }
    }

    /// Powder: fall; else slide down a diagonal (random order); else, if
    /// this row's repose run is longer than one cell, step sideways towards
    /// a free cell up to `run` cells away on the row below.
    fn powder(&mut self, x: usize, y: usize, ci: usize, density: f32, run_k: u32) {
        let i = y * self.width + x;
        if let Some(j) = self.enterable(x, y, 0, 1, density, true) {
            self.swap(i, j);
            return;
        }
        let first = self.coin(ci);
        for d in [first, -first] {
            if let Some(j) = self.enterable(x, y, d, 1, density, true) {
                self.swap(i, j);
                return;
            }
        }
        let run = run_length(y, run_k).cast_signed();
        for d in [first, -first] {
            let Some(side) = self.enterable(x, y, d, 0, density, true) else {
                continue;
            };
            for r in 2..=run {
                if r > 2
                    && self
                        .enterable(x, y, d * (r - 1), 0, density, true)
                        .is_none()
                {
                    break;
                }
                if self.enterable(x, y, d * r, 1, density, true).is_some() {
                    self.swap(i, side);
                    return;
                }
            }
        }
    }

    /// Liquid (`dy = 1`) or buoyant (`dy = -1`) flow: move along `dy`; else
    /// a diagonal; else look sideways along a free path, up to
    /// [`LEVEL_REACH`] cells, for the nearest cell it can fall (or rise) from,
    /// and move up to `dispersion` cells towards it. With no such cell in
    /// reach it stays put, so a surface settles (level to within one cell
    /// per `LEVEL_REACH` columns) instead of jittering forever.
    fn flow(&mut self, x: usize, y: usize, ci: usize, density: f32, dispersion: u8, dy: isize) {
        let i = y * self.width + x;
        let falling = dy > 0;
        if let Some(j) = self.enterable(x, y, 0, dy, density, falling) {
            self.swap(i, j);
            return;
        }
        let first = self.coin(ci);
        for d in [first, -first] {
            if let Some(j) = self.enterable(x, y, d, dy, density, falling) {
                self.swap(i, j);
                return;
            }
        }
        let step = isize::from(dispersion);
        if step == 0 {
            return;
        }
        for d in [first, -first] {
            // The farthest free cell within `dispersion` along the path.
            let mut to = None;
            for r in 1..=LEVEL_REACH.cast_signed() {
                let Some(j) = self.enterable(x, y, d * r, 0, density, falling) else {
                    break;
                };
                if r <= step {
                    to = Some(j);
                }
                if let Some(to) = to
                    && self.enterable(x, y, d * r, dy, density, falling).is_some()
                {
                    self.swap(i, to);
                    return;
                }
            }
        }
    }

    /// Floating: while the timer runs it falls through lighter fluids and
    /// rises through heavier ones at its `float_density`; resting counts the
    /// timer up. Once it reaches `float_ticks` it moves as a powder.
    fn floating(&mut self, x: usize, y: usize, ci: usize, cell: Cell, p: Props) {
        if cell.state >= p.float_ticks {
            self.powder(x, y, ci, p.density, p.run_k);
            return;
        }
        let i = y * self.width + x;
        for dy in [1, -1] {
            if let Some(j) = self.enterable(x, y, 0, dy, p.float_density, dy > 0) {
                self.swap(i, j);
                return;
            }
        }
        self.cells[i].state = cell.state + 1;
        self.moved[i] = self.stamp;
        self.mark_changed(x, y);
    }

    /// Whether a buoyant cell at `(x, y)` touches `into` above it, or the
    /// top edge.
    fn pops(&self, x: usize, y: usize, into: MaterialId) -> bool {
        match self.at(x, y, 0, -1) {
            None => true,
            Some(j) => self.cells[j].material == into,
        }
    }

    /// Records a change at `(x, y)`: grows its chunk's changed rectangle
    /// and adds the cells that read it next tick (its reach) to the work
    /// rectangle of every chunk they fall in, waking those chunks.
    fn mark_changed(&mut self, x: usize, y: usize) {
        let own = (y / CHUNK_SIZE) * self.chunks_x + x / CHUNK_SIZE;
        let point = Rect {
            x0: x,
            y0: y,
            x1: x + 1,
            y1: y + 1,
        };
        self.chunks[own].changed_next = self.chunks[own].changed_next.union(point);
        let reach = Rect {
            x0: x.saturating_sub(self.reach_x),
            y0: y.saturating_sub(1),
            x1: (x + self.reach_x + 1).min(self.width),
            y1: (y + 2).min(self.height),
        };
        for cy in reach.y0 / CHUNK_SIZE..=(reach.y1 - 1) / CHUNK_SIZE {
            for cx in reach.x0 / CHUNK_SIZE..=(reach.x1 - 1) / CHUNK_SIZE {
                let c = &mut self.chunks[cy * self.chunks_x + cx];
                c.work_next = c.work_next.union(reach.intersect(c.bounds));
                c.awake = true;
                c.idle_ticks = 0;
            }
        }
    }
}

/// `256 / tan(repose)`: the mean sideways run per row of drop, in 1/256
/// cells.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // repose is validated to 15..=45°
fn run_k(repose_deg: f32) -> u32 {
    let k = 256.0 / f64::from(repose_deg).to_radians().tan();
    (k.round() as u32).clamp(256, 256 * MAX_RUN as u32)
}

/// Sideways run, in whole cells, allowed between row `y` and the row below.
/// Rows alternate between ⌊k⌋ and ⌈k⌉ so the mean slope over a pile matches
/// the repose angle exactly; the pattern depends only on the row, so it is
/// deterministic and the same on both flanks.
#[inline]
fn run_length(y: usize, run_k: u32) -> usize {
    let k = u64::from(run_k);
    let y = y as u64;
    #[allow(clippy::cast_possible_truncation)] // at most MAX_RUN
    let run = ((((y + 1) * k) >> 8) - ((y * k) >> 8)) as usize;
    run.clamp(1, MAX_RUN)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // validated to (0, 3000] s
fn float_ticks(seconds: f32) -> u16 {
    let t = (f64::from(seconds) * f64::from(TICKS_PER_SECOND)).round();
    t.clamp(0.0, f64::from(u16::MAX - 1)) as u16
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)] // height ≤ 8192 and level in 0..=1
fn water_rows(height: usize, level: f32) -> usize {
    ((height as f64) * f64::from(level))
        .round()
        .clamp(0.0, height as f64) as usize
}

/// Colour-noise byte for a new cell, from the seed, tick and position.
fn variant(seed: u64, tick: u64, index: usize) -> u8 {
    splitmix64(seed ^ splitmix64(tick ^ ((index as u64) << 20))).to_le_bytes()[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_lengths_average_the_repose_slope() {
        for (deg, want) in [(45.0_f32, 1.0_f64), (35.0, 1.428), (26.57, 2.0)] {
            let k = run_k(deg);
            let total: usize = (0..1000).map(|y| run_length(y, k)).sum();
            #[allow(clippy::cast_precision_loss)]
            let mean = total as f64 / 1000.0;
            assert!((mean - want).abs() < 0.01, "{deg}°: mean run {mean}");
        }
    }

    #[test]
    fn rejects_bad_config() {
        let t = MaterialTable::builtin().unwrap();
        let bad = WorldConfig {
            width: 0,
            ..WorldConfig::default()
        };
        assert!(matches!(
            World::new(bad, t.clone()),
            Err(WorldError::BadSize { .. })
        ));
        let bad = WorldConfig {
            water_level: 1.5,
            ..WorldConfig::default()
        };
        assert!(matches!(
            World::new(bad, t),
            Err(WorldError::BadWaterLevel(_))
        ));
    }

    #[test]
    fn edits_wake_neighbours_within_reach() {
        let t = MaterialTable::builtin().unwrap();
        let sand = t.id("sand").unwrap();
        let mut w = World::new(
            WorldConfig {
                width: 128,
                height: 128,
                ..WorldConfig::default()
            },
            t,
        )
        .unwrap();
        assert_eq!(w.awake_chunks(), 0);
        // Interior of chunk 0: only it wakes.
        w.set_cell(30, 30, sand).unwrap();
        assert_eq!(w.awake_chunks(), 1);
        // One cell from the corner shared by all four chunks.
        w.set_cell(63, 63, sand).unwrap();
        assert_eq!(w.awake_chunks(), 4);
        assert_eq!(
            w.set_cell(128, 0, sand),
            Err(WorldError::OutOfBounds { x: 128, y: 0 })
        );
        assert_eq!(
            w.set_cell(0, 0, MaterialId(99)),
            Err(WorldError::UnknownMaterial(MaterialId(99)))
        );
    }
}
