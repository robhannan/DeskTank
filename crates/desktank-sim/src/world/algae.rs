//! Surface film uses bounded batches of field blocks, including when the
//! material chunks are asleep. Only mixed blocks and wet block edges can
//! grow a surface coat. Coated cells are indexed by chunk for grazers.
//! Catch-up visits disjoint groups of blocks in turn and credits each group
//! with the time its neighbours skipped, so it grows film at the live rate.
//!
//! Each coat byte (and each glass block) holds one variety: where several
//! would grow on a bare cell, the one with the highest priority takes it
//! (or, if a neighbour's variety would grow there too, that one creeps in),
//! and it keeps the cell until it has died back to bare (D28, #369).

use std::collections::BTreeSet;

use super::catch_up::STEP_TICKS;
use super::{TICKS_PER_SECOND, World};
use crate::ecology::{
    AlgaeConfig, AlgaeLook, Conditions, HOST_FRONT_GLASS, HOST_HARDSCAPE, HOST_REAR_GLASS,
    HOST_SUBSTRATE, Variety, Verdict,
};
use crate::field::{FIELD_EVERY, FIELD_SCALE, FieldKind};
use crate::grid::Cell;
use crate::hash::Fnv64;
use crate::rng::chunk_stream;

/// The low nibble of a coat or glass byte: density, 0 (bare) to 15 (full).
pub const ALGAE_DENSITY_MASK: u8 = 0x0f;
pub(super) const DENSITY_MASK: u8 = ALGAE_DENSITY_MASK;
pub(super) const FRONT_CAP: u8 = 5;
const BLOCKS_PER_STEP: usize = 64;
const CATCH_UP_BLOCKS: usize = 4096;
const ALGAE_STREAM: u64 = u64::MAX - 1;
/// The generic green film's variety id.
const GREEN_FILM: u8 = 1;

/// A glass surface of the tank that algae can cover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlassPane {
    /// The front pane, drawn over the tank and capped at density 5.
    Front,
    /// The back wall.
    Back,
    /// The left side wall.
    Left,
    /// The right side wall.
    Right,
}

#[derive(Clone)]
pub(super) struct Algae {
    pub(super) coat: Vec<u8>,
    pub(super) front: Vec<u8>,
    pub(super) back: Vec<u8>,
    pub(super) sides: Vec<[u8; 2]>,
    pub(super) by_chunk: Vec<BTreeSet<u32>>,
    pub(super) coated: usize,
    /// Coated cells by variety id.
    pub(super) by_variety: [usize; 16],
    pub(super) glass: usize,
    /// Glass film blocks and side strips by variety id.
    pub(super) glass_by_variety: [usize; 16],
    by_field: Vec<u8>,
    coat_version: u64,
    glass_version: u64,
}

impl Algae {
    pub(super) fn new(width: usize, height: usize, chunks: usize) -> Self {
        let fw = width.div_ceil(FIELD_SCALE);
        let fh = height.div_ceil(FIELD_SCALE);
        Self {
            coat: vec![0; width * height],
            front: vec![0; fw * fh],
            back: vec![0; fw * fh],
            sides: vec![[0; 2]; fh],
            by_chunk: vec![BTreeSet::new(); chunks],
            coated: 0,
            by_variety: [0; 16],
            glass: 0,
            glass_by_variety: [0; 16],
            by_field: vec![0; fw * fh],
            coat_version: 0,
            glass_version: 0,
        }
    }
    pub(super) fn glass_changed(&mut self, (old, new): (u8, u8)) {
        if old != new {
            self.glass = self.glass + usize::from(new != 0) - usize::from(old != 0);
            if old != 0 {
                self.glass_by_variety[usize::from(old >> 4)] -= 1;
            }
            if new != 0 {
                self.glass_by_variety[usize::from(new >> 4)] += 1;
            }
            self.glass_version = self.glass_version.wrapping_add(1);
        }
    }

    pub(super) fn clear_dry_glass(&mut self, block: usize, width: usize) {
        let front = std::mem::take(&mut self.front[block]);
        let back = std::mem::take(&mut self.back[block]);
        self.glass_changed((front, 0));
        self.glass_changed((back, 0));
        let (x, y) = (block % width, block / width);
        for (edge, column) in [(0, 0), (1, width - 1)] {
            if x == column {
                let old = std::mem::take(&mut self.sides[y][edge]);
                self.glass_changed((old, 0));
            }
        }
    }

    /// Whether any coat is of a variety `edible` says a grazer takes.
    pub(super) fn any_coat_where(&self, edible: impl Fn(u8) -> bool) -> bool {
        (1..16u8).any(|v| self.by_variety[usize::from(v)] != 0 && edible(v))
    }

    /// Whether any glass film is of a variety `edible` says a grazer takes.
    pub(super) fn any_glass_where(&self, edible: impl Fn(u8) -> bool) -> bool {
        (1..16u8).any(|v| self.glass_by_variety[usize::from(v)] != 0 && edible(v))
    }
}

impl World {
    /// Row-major coat, one byte per world cell. The high nibble is the
    /// variety id (see [`World::algae_variety`]); the low nibble is density
    /// (0 = bare, 15 = full). Bare cells have byte 0.
    #[must_use]
    pub fn coat(&self) -> &[u8] {
        &self.algae.coat
    }

    /// Front glass film at field resolution, row-major like
    /// [`World::field`]. Each byte packs the variety (high nibble) and a
    /// density on the shared 0..=15 scale, at most 5 on the front pane.
    #[must_use]
    pub fn front_glass_algae(&self) -> &[u8] {
        &self.algae.front
    }

    /// Back wall film at field resolution, row-major like
    /// [`World::field`], packed like the front pane's with densities 0..=15.
    #[must_use]
    pub fn back_glass_algae(&self) -> &[u8] {
        &self.algae.back
    }

    /// Side wall film, one packed `[left, right]` pair per field row.
    /// Each strip follows its edge column and uses the 0..=15 density scale.
    #[must_use]
    pub fn side_glass_algae(&self) -> &[[u8; 2]] {
        &self.algae.sides
    }

    /// The algae variety called `name` (as in `assets/fields.toml`), and
    /// its id: the high nibble of its coat bytes.
    #[must_use]
    pub fn algae_variety(&self, name: &str) -> Option<u8> {
        self.life.eco.algae.as_ref()?.by_name(name).map(|v| v.id)
    }

    /// How every variety is drawn, in file order.
    pub fn algae_looks(&self) -> impl Iterator<Item = &AlgaeLook> {
        self.life
            .eco
            .algae
            .iter()
            .flat_map(|cfg| cfg.varieties.iter().map(|v| &v.look))
    }

    /// How many coated cells are of variety `id`.
    #[must_use]
    pub fn coated_cells_of(&self, id: u8) -> usize {
        self.algae
            .by_variety
            .get(usize::from(id))
            .copied()
            .unwrap_or(0)
    }

    /// Changes whenever the coat or glass density changes.
    #[must_use]
    pub fn algae_version(&self) -> u64 {
        self.algae
            .coat_version
            .wrapping_add(self.algae.glass_version)
    }

    /// Changes whenever a coat density changes.
    #[must_use]
    pub fn coat_version(&self) -> u64 {
        self.algae.coat_version
    }

    /// Changes whenever a glass density changes.
    #[must_use]
    pub fn glass_version(&self) -> u64 {
        self.algae.glass_version
    }

    /// How many cells carry a coat.
    #[must_use]
    pub fn coated_cells(&self) -> usize {
        self.algae.coated
    }

    /// Initialises green film for a simulation fixture. Returns whether
    /// density changed. Only eligible cells with a water neighbour can be
    /// coated. Density is clamped to 15 and zero clears the coat.
    pub fn set_algae_coat(&mut self, x: usize, y: usize, density: u8) -> bool {
        self.set_algae_variety(x, y, GREEN_FILM, density)
    }

    /// Initialises a coat of variety `variety` for a simulation fixture.
    /// Returns whether the coat changed. Only a cell of one of the
    /// variety's hosts with a water neighbour can be coated. Density is
    /// clamped to 15 and zero clears the coat.
    pub fn set_algae_variety(&mut self, x: usize, y: usize, variety: u8, density: u8) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let i = y * self.width + x;
        let new = if density == 0 {
            0
        } else {
            let Some(cfg) = self.life.eco.algae.clone() else {
                return false;
            };
            let Some(v) = cfg.variety(variety) else {
                return false;
            };
            if !self.algae_surface(self.cells[i])
                || v.hosts & cfg_host(&cfg, self.cells[i]) == 0
                || self.algae_water(i).next().is_none()
            {
                return false;
            }
            film(variety << 4, density.min(DENSITY_MASK))
        };
        let changed = self.algae.coat[i] != new;
        self.write_coat(i, new);
        changed
    }

    /// Initialises glass film of variety `variety` on the field block
    /// (`fx`, `fy`) of `pane` for a simulation fixture. Returns whether the
    /// film changed. The variety must grow on that pane; density is clamped
    /// to the pane's cap and zero clears it. Film over dry blocks is
    /// cleared again at the next light update.
    pub fn set_algae_glass(
        &mut self,
        pane: GlassPane,
        fx: usize,
        fy: usize,
        variety: u8,
        density: u8,
    ) -> bool {
        let fw = self.width.div_ceil(FIELD_SCALE);
        if fx >= fw || fy >= self.height.div_ceil(FIELD_SCALE) {
            return false;
        }
        let (host, cap) = match pane {
            GlassPane::Front => (HOST_FRONT_GLASS, FRONT_CAP),
            _ => (HOST_REAR_GLASS, DENSITY_MASK),
        };
        let new = if density == 0 {
            0
        } else {
            let grows = self
                .life
                .eco
                .algae
                .as_ref()
                .and_then(|cfg| cfg.variety(variety))
                .is_some_and(|v| v.hosts & host != 0);
            if !grows {
                return false;
            }
            film(variety << 4, density.min(cap))
        };
        let block = fy * fw + fx;
        let slot = match pane {
            GlassPane::Front => &mut self.algae.front[block],
            GlassPane::Back => &mut self.algae.back[block],
            GlassPane::Left | GlassPane::Right => {
                let edge = usize::from(pane == GlassPane::Right);
                if fx != edge * (fw - 1) {
                    return false;
                }
                &mut self.algae.sides[fy][edge]
            }
        };
        let old = std::mem::replace(slot, new);
        self.algae.glass_changed((old, new));
        old != new
    }

    /// Whether `id` is a configured variety.
    pub(super) fn algae_known(&self, id: u8) -> bool {
        self.life
            .eco
            .algae
            .as_ref()
            .is_some_and(|cfg| cfg.variety(id).is_some())
    }

    pub(super) fn algae_surface(&self, cell: Cell) -> bool {
        self.life
            .eco
            .algae
            .as_ref()
            .is_some_and(|a| a.surfaces.contains(cell.material))
    }

    pub(super) fn algae_water(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.algae_neighbours(i)
            .filter(|&j| Some(self.cells[j].material) == self.life.water)
    }

    fn algae_neighbours(&self, i: usize) -> impl Iterator<Item = usize> + use<> {
        let (x, y) = (i % self.width, i / self.width);
        [
            i.checked_sub(self.width),
            (x > 0).then(|| i - 1),
            (x + 1 < self.width).then_some(i + 1),
            (y + 1 < self.height).then_some(i + self.width),
        ]
        .into_iter()
        .flatten()
    }

    pub(super) fn write_coat(&mut self, i: usize, value: u8) {
        let old = self.algae.coat[i];
        if old == value {
            return;
        }
        self.algae.coat[i] = value;
        for (byte, add) in [(old, false), (value, true)] {
            if byte != 0 {
                let count = &mut self.algae.by_variety[usize::from(byte >> 4)];
                *count = if add { *count + 1 } else { *count - 1 };
            }
        }
        let (x, y) = (i % self.width, i / self.width);
        let field = self.algae_field_index(x, y);
        if (old == 0) != (value == 0) {
            let chunk = self.chunk_of(i);
            #[allow(clippy::cast_possible_truncation)] // MAX_SIDE squared fits in u32
            let index = i as u32;
            if value == 0 {
                self.algae.by_chunk[chunk].remove(&index);
                self.algae.coated -= 1;
                self.algae.by_field[field] -= 1;
            } else {
                self.algae.by_chunk[chunk].insert(index);
                self.algae.coated += 1;
                self.algae.by_field[field] += 1;
            }
        }
        self.algae.coat_version = self.algae.coat_version.wrapping_add(1);
        self.mark_film_light_dirty(field);
    }

    fn algae_field_index(&self, x: usize, y: usize) -> usize {
        y / FIELD_SCALE * self.field(FieldKind::Light).width() + x / FIELD_SCALE
    }

    pub(super) fn hash_algae(&self, h: &mut Fnv64) {
        if self.algae.coated != 0 || self.algae.glass != 0 {
            h.write(self.coat());
            h.write(self.front_glass_algae());
            h.write(self.back_glass_algae());
            for sides in self.side_glass_algae() {
                h.write(sides);
            }
        }
    }

    pub(super) fn step_algae(&mut self) {
        let blocks = self.algae.front.len();
        let batches = blocks.div_ceil(BLOCKS_PER_STEP);
        let step = self.tick / FIELD_EVERY - 1;
        #[allow(clippy::cast_possible_truncation)] // The modulo fits the field grid.
        let start = (step % batches as u64) as usize * BLOCKS_PER_STEP;
        #[allow(clippy::cast_precision_loss)] // At most MAX_SIDE² / FIELD_SCALE² blocks
        let seconds = (batches as f32) * crate::field::FIELD_DT;
        self.update_algae(start, (blocks - start).min(BLOCKS_PER_STEP), seconds);
    }

    pub(super) fn catch_up_algae(&mut self, start: u64) {
        let blocks = self.algae.front.len();
        if blocks == 0 || self.tick == start {
            return;
        }
        // Consecutive coarse intervals visit disjoint groups of blocks until
        // they wrap, and each visit stands for every interval it skipped.
        let size = blocks.min(CATCH_UP_BLOCKS);
        let groups = blocks.div_ceil(size);
        #[allow(clippy::cast_possible_truncation)] // The modulo fits the field grid.
        let first = (start / u64::from(STEP_TICKS) % groups as u64) as usize * size;
        #[allow(clippy::cast_precision_loss)] // At most five minutes of ticks, and a few groups
        let seconds = (self.tick - start) as f32 / TICKS_PER_SECOND as f32 * groups as f32;
        self.update_algae(first, (blocks - first).min(size), seconds);
        self.refresh_light_if_dirty();
    }

    fn update_algae(&mut self, start: usize, count: usize, seconds: f32) {
        let Some(cfg) = self.life.eco.algae.clone() else {
            return;
        };
        let water = self.algae_water_cells();
        let competition = self.algae_plant_competition(water);
        let mut rng = chunk_stream(self.seed, self.tick, ALGAE_STREAM);
        let blocks = self.algae.front.len();
        let mut step = FilmStep {
            cfg: &cfg,
            competition,
            seconds,
            maturity: self.filter_bio_capacity(),
            memo: None,
            rng: &mut rng,
        };
        for row in start..start + count {
            let block = row % blocks;
            let co2 = self.field(FieldKind::Co2).values()[block];
            // Plants with CO₂ to work with are stronger competitors.
            step.competition = (competition * (1.0 + cfg.co2_boost * co2)).min(1.0);
            self.update_glass(block, &mut step);
            self.update_substrate_coat(block, &mut step);
        }
    }

    /// Bits by variety id that grow, and that wait for light, in `block`.
    fn block_verdicts(&self, block: usize, step: &mut FilmStep<'_>) -> (u16, u16) {
        if let Some((b, verdicts)) = step.memo
            && b == block
        {
            return verdicts;
        }
        let verdicts = step.cfg.verdicts(Conditions {
            nutrient: self.field(FieldKind::Nutrient).values()[block],
            light: self.field(FieldKind::Light).values()[block],
            co2: self.field(FieldKind::Co2).values()[block],
            maturity: step.maturity,
        });
        step.memo = Some((block, verdicts));
        verdicts
    }

    fn update_substrate_coat(&mut self, block: usize, step: &mut FilmStep<'_>) {
        let field = self.field(FieldKind::Nutrient);
        let (fw, fh) = (field.width(), field.height());
        let (fx, fy) = (block % fw, block / fw);
        let (x0, y0) = (fx * FIELD_SCALE, fy * FIELD_SCALE);
        let (x1, y1) = (
            (x0 + FIELD_SCALE).min(self.width),
            (y0 + FIELD_SCALE).min(self.height),
        );
        let on_surface = step.cfg.hosted(HOST_SUBSTRATE) | step.cfg.hosted(HOST_HARDSCAPE);
        if self.algae.by_field[block] == 0 {
            let water = self.algae_field_water(block);
            if step.competition >= 1.0 || water == (x1 - x0) * (y1 - y0) {
                return;
            }
            // Border cells can grow from water in a neighbouring field block.
            let blocks = [
                Some(block),
                block.checked_sub(fw),
                (fx > 0).then(|| block - 1),
                (fx + 1 < fw).then_some(block + 1),
                (fy + 1 < fh).then_some(block + fw),
            ];
            if !blocks.into_iter().flatten().any(|b| {
                self.algae_field_water(b) != 0 && self.block_verdicts(b, step).0 & on_surface != 0
            }) {
                return;
            }
        }
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * self.width + x;
                let material = self.cells[i].material;
                if !step.cfg.surfaces.contains(material) {
                    continue;
                }
                let host = cfg_host(step.cfg, self.cells[i]);
                let (mut grow, mut dormant) = (0, 0);
                for j in self.algae_water(i) {
                    let b = self.algae_field_index(j % self.width, j / self.width);
                    let (g, d) = self.block_verdicts(b, step);
                    (grow, dormant) = (grow | g, dormant | d);
                }
                let old = self.algae.coat[i];
                let new = if old != 0 {
                    step.regrow(old, DENSITY_MASK, grow, dormant)
                } else {
                    let mut near = [0; 4];
                    for (slot, j) in near.iter_mut().zip(self.algae_neighbours(i)) {
                        *slot = self.algae.coat[j];
                    }
                    step.settle(host, grow, &near, DENSITY_MASK)
                };
                self.write_coat(i, new);
            }
        }
    }

    fn update_glass(&mut self, block: usize, step: &mut FilmStep<'_>) {
        if self.algae_field_water(block) == 0 {
            return;
        }
        let fw = self.field(FieldKind::Light).width();
        let (x, y) = (block % fw, block / fw);
        let (grow, dormant) = self.block_verdicts(block, step);
        let plane = |plane: &[u8]| -> [u8; 4] {
            [
                (x > 0).then(|| plane[block - 1]),
                (x + 1 < fw).then(|| plane[block + 1]),
                (y > 0).then(|| plane[block - fw]),
                plane.get(block + fw).copied(),
            ]
            .map(Option::unwrap_or_default)
        };
        let mut update = |old: u8, near: [u8; 4], cap: u8, host: u8| {
            let new = if old != 0 {
                step.regrow(old, cap, grow, dormant)
            } else {
                step.settle(host, grow, &near, cap)
            };
            (old, new)
        };
        let near = plane(&self.algae.front);
        let (old, new) = update(self.algae.front[block], near, FRONT_CAP, HOST_FRONT_GLASS);
        self.algae.front[block] = new;
        self.algae.glass_changed((old, new));
        let near = plane(&self.algae.back);
        let (old, new) = update(self.algae.back[block], near, DENSITY_MASK, HOST_REAR_GLASS);
        self.algae.back[block] = new;
        self.algae.glass_changed((old, new));
        for (edge, column) in [(0, 0), (1, fw - 1)] {
            if x != column {
                continue;
            }
            let near = [
                (y > 0).then(|| self.algae.sides[y - 1][edge]),
                self.algae.sides.get(y + 1).map(|row| row[edge]),
                None,
                None,
            ]
            .map(Option::unwrap_or_default);
            let (old, new) = update(
                self.algae.sides[y][edge],
                near,
                DENSITY_MASK,
                HOST_REAR_GLASS,
            );
            self.algae.sides[y][edge] = new;
            self.algae.glass_changed((old, new));
        }
    }
}

/// The host a surface cell is for the varieties: hardscape or substrate.
fn cfg_host(cfg: &AlgaeConfig, cell: Cell) -> u8 {
    if cfg.hardscape.contains(cell.material) {
        HOST_HARDSCAPE
    } else {
        HOST_SUBSTRATE
    }
}

fn film(variety: u8, density: u8) -> u8 {
    if density == 0 {
        0
    } else {
        (variety & !DENSITY_MASK) | density
    }
}

struct FilmStep<'a> {
    cfg: &'a AlgaeConfig,
    competition: f32,
    seconds: f32,
    /// The filters' biological maturity.
    maturity: f32,
    /// The last block's verdicts: a surface cell's neighbours are mostly in
    /// its own block.
    memo: Option<(usize, (u16, u16))>,
    rng: &'a mut rand_pcg::Pcg32,
}

impl FilmStep<'_> {
    /// The coat byte of an already coated cell (or glass block) after a
    /// step: it grows, waits or recedes by its own variety's verdict.
    fn regrow(&mut self, old: u8, cap: u8, grow: u16, dormant: u16) -> u8 {
        let Some(v) = self.cfg.variety(old >> 4) else {
            return old;
        };
        let bit = 1 << v.id;
        let verdict = if grow & bit != 0 {
            Verdict::Grow
        } else if dormant & bit != 0 {
            Verdict::Dormant
        } else {
            Verdict::Fade
        };
        let density = self.density(v, old & DENSITY_MASK, cap, verdict, false);
        film(old, density)
    }

    /// The coat byte of a bare cell (or glass block) on `host` after a step.
    /// `near` holds its neighbours' bytes. The highest-priority variety that
    /// can grow there takes root, unless a neighbour's variety that can too
    /// ranks as high: that one creeps in instead, faster. Returns 0 if none
    /// does.
    fn settle(&mut self, host: u8, grow: u16, near: &[u8], cap: u8) -> u8 {
        let candidates = grow & self.cfg.hosted(host);
        if candidates == 0 || self.competition >= 1.0 {
            return 0;
        }
        let Some(best) = self.cfg.best(candidates) else {
            return 0;
        };
        let creeper = near
            .iter()
            .filter(|&&b| b != 0 && candidates & (1 << (b >> 4)) != 0)
            .filter_map(|&b| self.cfg.variety(b >> 4))
            .filter(|v| v.priority >= best.priority)
            .fold(None::<&Variety>, |top, v| {
                top.filter(|t| t.priority >= v.priority).or(Some(v))
            });
        let v = creeper.unwrap_or(best);
        let density = self.density(v, 0, cap, Verdict::Grow, creeper.is_some());
        film(v.id << 4, density)
    }

    fn density(&mut self, v: &Variety, old: u8, cap: u8, verdict: Verdict, creep: bool) -> u8 {
        use rand_core::Rng as _;

        let (grow, rate) = if verdict == Verdict::Fade || self.competition >= 1.0 {
            (false, v.die_back_rate * (1.0 + self.competition))
        } else if verdict == Verdict::Grow {
            let rate = if old != 0 {
                v.growth_rate
            } else if creep {
                v.creep_rate
            } else {
                v.seed_rate
            };
            (true, rate * (1.0 - self.competition))
        } else {
            return old;
        };
        let room = if grow { cap - old } else { old };
        if room == 0 {
            return old;
        }
        let levels = (rate * self.seconds).min(f32::from(room));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=15
        let whole = levels as u8;
        let fraction = f64::from(levels - f32::from(whole));
        let amount = whole
            + u8::from(
                fraction > 0.0 && f64::from(self.rng.next_u32()) < fraction * 4_294_967_296.0,
            );
        if grow { old + amount } else { old - amount }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Brush, MaterialTable, Rect, WorldConfig};

    #[test]
    fn catch_up_credits_every_block_across_a_cycle_of_intervals() {
        let mut w = World::new(
            WorldConfig {
                width: 1280,
                height: 240,
                seed: 3,
                water_level: 1.0,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let sand = w.materials().id("sand").unwrap();
        let bed = Rect {
            x0: 0,
            y0: 200,
            x1: 1280,
            y1: 240,
        };
        w.fill_rect(bed, sand, Brush::Replace).unwrap();
        for y in (0..w.height()).step_by(FIELD_SCALE) {
            for x in (0..w.width()).step_by(FIELD_SCALE) {
                w.add_to_field(FieldKind::Nutrient, x, y, 1.0);
            }
        }
        for _ in 0..TICKS_PER_SECOND {
            w.tick();
        }
        let blocks = w.algae.front.len();
        let groups = blocks.div_ceil(CATCH_UP_BLOCKS);
        assert!(groups > 1, "needs several groups, has {blocks} blocks");
        let top = 200 * w.width;
        for x in 0..w.width {
            assert!(w.set_algae_coat(x, 200, 1));
        }
        let before: usize = w.algae.coat[top..top + w.width]
            .iter()
            .map(|&v| usize::from(v & DENSITY_MASK))
            .sum();
        // One short interval per group, as `catch_up` would visit them.
        let interval = 10 * TICKS_PER_SECOND;
        for g in 0..groups as u64 {
            let start = (1 + g) * u64::from(STEP_TICKS);
            w.tick = start + u64::from(interval);
            w.catch_up_algae(start);
        }
        let after: usize = w.algae.coat[top..top + w.width]
            .iter()
            .map(|&v| usize::from(v & DENSITY_MASK))
            .sum();
        // Every block has grown for the whole 10 s × groups it stands for.
        #[allow(clippy::cast_precision_loss)]
        let gain = (after - before) as f32 / w.width as f32;
        #[allow(clippy::cast_precision_loss)]
        let expected = 0.15 * 10.0 * groups as f32;
        assert!(
            (gain - expected).abs() < expected * 0.2,
            "mean gain {gain}, expected {expected}"
        );
    }

    #[test]
    fn co2_strengthens_plant_competition_against_film() {
        let film_after = |co2_boost: f32| {
            let mut w = World::new(
                WorldConfig {
                    width: 64,
                    height: 48,
                    seed: 9,
                    water_level: 1.0,
                },
                MaterialTable::builtin().unwrap(),
            )
            .unwrap();
            let sand = w.materials().id("sand").unwrap();
            let bed = Rect {
                x0: 0,
                y0: 42,
                x1: 64,
                y1: 48,
            };
            w.fill_rect(bed, sand, Brush::Replace).unwrap();
            let rotala = w.plant_species().id("rotala").unwrap();
            // Six stands: tapered rotala (bare tops, short upper leaves) covers
            // less water than it used to, and five left the boost's effect
            // marginal.
            for x in [4, 11, 18, 25, 32, 39] {
                w.paint_plant(x, 41, 0, rotala, crate::Propagule::Cuttings)
                    .unwrap();
            }
            std::sync::Arc::make_mut(w.life.eco.algae.as_mut().unwrap()).co2_boost = co2_boost;
            for _ in 0..12 {
                for y in (0..w.height()).step_by(FIELD_SCALE) {
                    for x in (0..w.width()).step_by(FIELD_SCALE) {
                        w.add_to_field(FieldKind::Nutrient, x, y, 1.0);
                        w.add_to_field(FieldKind::Co2, x, y, 0.4);
                    }
                }
                for _ in 0..10 * TICKS_PER_SECOND {
                    w.tick();
                }
            }
            (w.plant_cells(), w.coated_cells())
        };
        let (plants, plain) = film_after(0.0);
        let (more_plants, boosted) = film_after(0.5);
        assert!(
            plain > 40 && boosted * 4 < plain * 3,
            "{boosted} coated cells against {plain}"
        );
        // The plants themselves are the same either way.
        assert_eq!(plants, more_plants);
    }
}
