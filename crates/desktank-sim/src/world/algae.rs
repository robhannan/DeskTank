//! Surface film uses bounded batches of field blocks, including when the
//! material chunks are asleep. Only mixed blocks and wet block edges can
//! grow a surface coat. Coated cells are indexed by chunk for grazers.
//! Catch-up visits disjoint groups of blocks in turn and credits each group
//! with the time its neighbours skipped, so it grows film at the live rate.

use std::collections::BTreeSet;

use super::catch_up::STEP_TICKS;
use super::{TICKS_PER_SECOND, World};
use crate::ecology::AlgaeConfig;
use crate::field::{FIELD_EVERY, FIELD_SCALE, FieldKind};
use crate::grid::Cell;
use crate::hash::Fnv64;
use crate::rng::chunk_stream;

const GREEN_FILM: u8 = 0x10;
pub(super) const DENSITY_MASK: u8 = 0x0f;
pub(super) const FRONT_CAP: u8 = 5;
const BLOCKS_PER_STEP: usize = 64;
const CATCH_UP_BLOCKS: usize = 4096;
const ALGAE_STREAM: u64 = u64::MAX - 1;

#[derive(Clone)]
pub(super) struct Algae {
    pub(super) coat: Vec<u8>,
    pub(super) front: Vec<u8>,
    pub(super) back: Vec<u8>,
    pub(super) sides: Vec<[u8; 2]>,
    pub(super) by_chunk: Vec<BTreeSet<u32>>,
    pub(super) coated: usize,
    pub(super) glass: usize,
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
            glass: 0,
            by_field: vec![0; fw * fh],
            coat_version: 0,
            glass_version: 0,
        }
    }
    pub(super) fn glass_changed(&mut self, (old, new): (u8, u8)) {
        if old != new {
            self.glass = self.glass + usize::from(new != 0) - usize::from(old != 0);
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
}

impl World {
    /// Row-major coat, one byte per world cell. The high nibble is the
    /// variety (1 = green film); the low nibble is density (0 = bare,
    /// 15 = full). Bare cells have byte 0.
    #[must_use]
    pub fn coat(&self) -> &[u8] {
        &self.algae.coat
    }

    /// Front glass density at field resolution, row-major like
    /// [`World::field`]. Values are 0..=5 on the shared 0..=15 density scale.
    #[must_use]
    pub fn front_glass_algae(&self) -> &[u8] {
        &self.algae.front
    }

    /// Back wall density at field resolution, row-major like
    /// [`World::field`]. Values are 0..=15.
    #[must_use]
    pub fn back_glass_algae(&self) -> &[u8] {
        &self.algae.back
    }

    /// Side wall density, one `[left, right]` pair per field row.
    /// Each strip follows its edge column and uses the 0..=15 density scale.
    #[must_use]
    pub fn side_glass_algae(&self) -> &[[u8; 2]] {
        &self.algae.sides
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
        if x >= self.width || y >= self.height {
            return false;
        }
        let i = y * self.width + x;
        if density != 0
            && (!self.algae_surface(self.cells[i]) || self.algae_water(i).next().is_none())
        {
            return false;
        }
        let new = film(GREEN_FILM, density.min(DENSITY_MASK));
        let changed = self.algae.coat[i] != new;
        self.write_coat(i, new);
        changed
    }

    pub(super) fn algae_surface(&self, cell: Cell) -> bool {
        self.life
            .eco
            .algae
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
        let Some(cfg) = self.life.eco.algae else {
            return;
        };
        let water = self.algae_water_cells();
        let competition = self.algae_plant_competition(water);
        let mut rng = chunk_stream(self.seed, self.tick, ALGAE_STREAM);
        let blocks = self.algae.front.len();
        for row in start..start + count {
            let block = row % blocks;
            let nutrient = self.field(FieldKind::Nutrient).values()[block];
            let light = self.field(FieldKind::Light).values()[block];
            let co2 = self.field(FieldKind::Co2).values()[block];
            let mut step = FilmStep {
                cfg,
                // Plants with CO₂ to work with are stronger competitors.
                competition: (competition * (1.0 + cfg.co2_boost * co2)).min(1.0),
                seconds,
                rng: &mut rng,
            };
            self.update_glass(block, nutrient, light, &mut step);
            self.update_substrate_coat(block, &mut step);
        }
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
                self.algae_field_water(b) != 0
                    && field.values()[b] > step.cfg.nutrient
                    && self.field(FieldKind::Light).values()[b] > step.cfg.light
            }) {
                return;
            }
        }
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * self.width + x;
                if !step.cfg.surfaces.contains(self.cells[i].material) {
                    continue;
                }
                let old = self.algae.coat[i];
                let (mut fed, mut lit) = (false, false);
                for j in self.algae_water(i) {
                    let (x, y) = (j % self.width, j / self.width);
                    if self.field(FieldKind::Nutrient).at_cell(x, y) > step.cfg.nutrient {
                        fed = true;
                        if self.field(FieldKind::Light).at_cell(x, y) > step.cfg.light {
                            lit = true;
                            break;
                        }
                    }
                }
                let creep =
                    old == 0 && lit && self.algae_neighbours(i).any(|j| self.algae.coat[j] != 0);
                let density = step.density(old & DENSITY_MASK, DENSITY_MASK, fed, lit, creep);
                self.write_coat(i, film(if old == 0 { GREEN_FILM } else { old }, density));
            }
        }
    }

    fn update_glass(&mut self, block: usize, nutrient: f32, light: f32, step: &mut FilmStep<'_>) {
        if self.algae_field_water(block) == 0 {
            return;
        }
        let fw = self.field(FieldKind::Light).width();
        let (x, y) = (block % fw, block / fw);
        let fed = nutrient > step.cfg.nutrient;
        let lit = light > step.cfg.light;
        let update = |plane: &mut [u8], cap, step: &mut FilmStep<'_>| {
            let old = plane[block];
            let creep = old == 0
                && ((x > 0 && plane[block - 1] > 0)
                    || (x + 1 < fw && plane[block + 1] > 0)
                    || (y > 0 && plane[block - fw] > 0)
                    || plane.get(block + fw).is_some_and(|&v| v > 0));
            let new = step.density(old, cap, fed, lit, creep);
            plane[block] = new;
            (old, new)
        };
        let front = update(&mut self.algae.front, FRONT_CAP, step);
        let back = update(&mut self.algae.back, DENSITY_MASK, step);
        self.algae.glass_changed(front);
        self.algae.glass_changed(back);
        for (edge, column) in [(0, 0), (1, fw - 1)] {
            if x != column {
                continue;
            }
            let old = self.algae.sides[y][edge];
            let creep = old == 0
                && ((y > 0 && self.algae.sides[y - 1][edge] > 0)
                    || self.algae.sides.get(y + 1).is_some_and(|row| row[edge] > 0));
            let new = step.density(old, DENSITY_MASK, fed, lit, creep);
            self.algae.sides[y][edge] = new;
            self.algae.glass_changed((old, new));
        }
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
    cfg: AlgaeConfig,
    competition: f32,
    seconds: f32,
    rng: &'a mut rand_pcg::Pcg32,
}

impl FilmStep<'_> {
    fn density(&mut self, old: u8, cap: u8, fed: bool, lit: bool, creep: bool) -> u8 {
        use rand_core::Rng as _;

        let (grow, rate) = if !fed || self.competition >= 1.0 {
            (false, self.cfg.die_back_rate * (1.0 + self.competition))
        } else if lit {
            let rate = if old != 0 {
                self.cfg.growth_rate
            } else if creep {
                self.cfg.creep_rate
            } else {
                self.cfg.seed_rate
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
            for x in [4, 12, 20, 28, 36] {
                w.paint_plant(x, 41, 0, rotala, crate::Propagule::Cuttings)
                    .unwrap();
            }
            w.life.eco.algae.as_mut().unwrap().co2_boost = co2_boost;
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
