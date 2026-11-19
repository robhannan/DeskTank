//! Breeding, colour genetics and predation (SPEC §2.4, T9 part 2).
//!
//! - **Sexes.** Every creature is male or female. One added from the
//!   palette takes whichever sex its species has fewer of in the tank (a
//!   male on a tie), so a pair added one after the other is a pair; young
//!   are either at even odds.
//! - **Breeding** is checked every [`BREED_CHECK_SECONDS`] for every
//!   female of a species that breeds. She conceives when she is an adult
//!   (mature, not an elder), well fed (her energy reserve at least half
//!   full), unstressed by poor water, rested since her last brood and not
//!   held in the net, an adult male of her species is in the tank and the
//!   tank is under its population cap, with a chance of 1 in
//!   [`CONCEIVE_ODDS`] times the share of the cap still free: a crowded tank
//!   breeds slower and a full one not at all. A creature added from the
//!   palette settles in for [`SETTLE_TICKS`] (a day of its life) first.
//!   - A **livebearer** (guppy; a cherry shrimp carrying her eggs) carries
//!     her young for the species' `days` and gives birth to a brood of fry
//!     around her, as many as the cap leaves room for.
//!   - An **egg layer** lays a clutch at once: egg cells (the `egg`
//!     material, each holding the clutch's id in its state) stuck to the
//!     glass, rock, plants or bed around her or, where there aren't enough
//!     such spots, sunk to whatever lies below her. A clutch hatches into fry after the species' `days`, egg by egg where
//!     the egg is still there; an egg that can't hatch because the tank is
//!     at its cap rots into detritus. At most [`MAX_CLUTCHES`] clutches.
//!
//!   Either way she then rests for the species' `rest_days`.
//! - **Colour genetics** (D13) is a per-species allele table, not real
//!   genetics: every creature carries two morph alleles (indices into its
//!   species' morphs) and shows a morph only when both match; any mix
//!   shows the wildtype. Each young takes one of its mother's alleles and
//!   one of its father's at random; an allele mutates into a random other
//!   morph [`MUTATION_PER_MILLE`] times in 1000 and reverts to the
//!   wildtype [`REVERSION_PER_MILLE`] times in 1000. So same-morph lines
//!   breed true, crossing two morphs gives wildtype young, and mixed lines
//!   drift back towards wildtype, as mixed neocaridina revert to brown.
//! - **Predation.** Once a second a hungry adult of a species with
//!   `prey_cm` that touches (within its reach) a creature no longer than
//!   that, of any species including its own young, eats it with a chance
//!   of 1 in [`STRIKE_ODDS`]: the prey is gone (no corpse) and the
//!   predator is fed. Not in easy mode, and not while either is in the
//!   net.
//!
//! Randomness comes from streams per creature or clutch per tick, so it
//! never depends on update order.

use rand_core::Rng as _;
use rand_pcg::Pcg32;

use super::{BITE_ENERGY, Creature, ENERGY_RESERVE, FULL, LIFE_DAY_TICKS, LifeStage, World};
use crate::TICKS_PER_SECOND;
use crate::grid::Rect;
use crate::hash::Fnv64;
use crate::rng::chunk_stream;
use crate::save::{SaveError, v3};
use crate::species::{BreedingMode, SpeciesId};

/// World seconds between breeding checks.
pub const BREED_CHECK_SECONDS: u32 = 60;
/// A ready female conceives at a check with a chance of 1 in this, times
/// the share of the population cap still free.
pub const CONCEIVE_ODDS: u32 = 4;
/// Ticks a creature added from the palette (or by a script or a host)
/// settles in before it breeds: a day of its life.
pub const SETTLE_TICKS: u32 = LIFE_DAY_TICKS;
/// Chance in 1000 that an allele passed to a young mutates into another
/// morph.
pub const MUTATION_PER_MILLE: u32 = 5;
/// Chance in 1000 that an allele passed to a young reverts to the
/// wildtype.
pub const REVERSION_PER_MILLE: u32 = 10;
/// A hungry predator touching its prey takes it with a chance of 1 in this
/// each second.
pub const STRIKE_ODDS: u32 = 4;
/// Energy a predator gets from a meal, in ticks (an hour).
pub const PREY_ENERGY: u32 = 3 * BITE_ENERGY;
/// Most egg clutches a tank holds at once.
pub const MAX_CLUTCHES: usize = 64;
/// Cells around her body a female lays her eggs in.
const EGG_REACH: usize = 6;
/// Ticks between breeding checks.
const BREED_CHECK_TICKS: u64 = BREED_CHECK_SECONDS as u64 * TICKS_PER_SECOND as u64;
/// Random streams for breeding, clutches and predation: above every
/// creature's movement stream (creature ids are below 2³²), below the life
/// stream.
const BREED_STREAM: u64 = (1 << 48) | (1 << 40);
const CLUTCH_STREAM: u64 = (1 << 48) | (2 << 40);
const PREY_STREAM: u64 = (1 << 48) | (3 << 40);

/// A species' breeding in ticks.
#[derive(Clone, Copy, Debug)]
pub(super) struct Breed {
    pub eggs: bool,
    /// Ticks carried, or until a clutch hatches.
    pub days: u32,
    pub brood: [u8; 2],
    /// Ticks of rest after a brood.
    pub rest: u32,
}

impl Breed {
    pub(super) fn of(b: &crate::species::Breeding) -> Self {
        Self {
            eggs: b.mode == BreedingMode::Eggs,
            // Validated to at most 365 days: inside u32.
            days: b.days.saturating_mul(LIFE_DAY_TICKS),
            brood: b.brood,
            rest: b.rest_days.saturating_mul(LIFE_DAY_TICKS),
        }
    }
}

/// A clutch of eggs waiting to hatch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clutch {
    /// Its id, which its egg cells hold in their state (never 0).
    pub(super) id: u16,
    pub(super) species: SpeciesId,
    /// The mother's and father's alleles.
    pub(super) mother: [u8; 2],
    pub(super) sire: [u8; 2],
    /// Tick it hatches on (or at the first second after).
    pub(super) hatch: u64,
    /// The egg cells as laid (cell indices); an egg no longer there is
    /// lost.
    pub(super) eggs: Vec<u32>,
}

impl Clutch {
    /// Its id, which its egg cells hold in their state.
    #[must_use]
    pub fn id(&self) -> u16 {
        self.id
    }

    /// The species that laid it.
    #[must_use]
    pub fn species(&self) -> SpeciesId {
        self.species
    }

    /// The tick it hatches on.
    #[must_use]
    pub fn hatch_tick(&self) -> u64 {
        self.hatch
    }

    /// The egg cells as laid, as cell indices (`y * width + x`).
    #[must_use]
    pub fn eggs(&self) -> &[u32] {
        &self.eggs
    }

    /// The mother's and the father's morph alleles.
    #[must_use]
    pub fn parents(&self) -> ([u8; 2], [u8; 2]) {
        (self.mother, self.sire)
    }
}

/// A young about to be born.
pub(super) struct Birth {
    species: SpeciesId,
    x: i64,
    y: i64,
    female: bool,
    alleles: [u8; 2],
}

/// A clutch about to be laid.
pub(super) struct Lay {
    species: SpeciesId,
    /// The mother's body.
    body: Rect,
    mother: [u8; 2],
    sire: [u8; 2],
    count: u8,
    /// Ticks until it hatches.
    days: u32,
}

/// What this tick's breeding needs and makes.
#[derive(Default)]
pub(super) struct BreedTick {
    /// On check ticks: every adult male's species and alleles.
    males: Option<Vec<(SpeciesId, [u8; 2])>>,
    /// The population at the start of the tick.
    population: usize,
    births: Vec<Birth>,
    lays: Vec<Lay>,
}

/// The morph a creature with `alleles` shows: theirs if they match,
/// otherwise the wildtype (`wild`), which dominates every other morph.
#[must_use]
pub(super) fn phenotype(alleles: [u8; 2], wild: u8) -> u8 {
    if alleles[0] == alleles[1] {
        alleles[0]
    } else {
        wild
    }
}

/// A young's alleles: one of its mother's and one of its father's, each
/// rarely mutated or reverted. `morphs` is the species' morph count.
pub(super) fn inherit(
    mother: [u8; 2],
    sire: [u8; 2],
    morphs: u8,
    wild: u8,
    rng: &mut Pcg32,
) -> [u8; 2] {
    [
        pass(mother, morphs, wild, rng),
        pass(sire, morphs, wild, rng),
    ]
}

/// One allele a parent passes on.
fn pass(parent: [u8; 2], morphs: u8, wild: u8, rng: &mut Pcg32) -> u8 {
    let allele = parent[usize::from(rng.next_u32() & 1 == 1)];
    let roll = rng.next_u32() % 1000;
    if roll < MUTATION_PER_MILLE && morphs > 1 {
        // Any morph but the wildtype; fewer than 8, so the cast is exact.
        #[allow(clippy::cast_possible_truncation)]
        let m = (rng.next_u32() % u32::from(morphs - 1)) as u8;
        if m >= wild { m + 1 } else { m }
    } else if roll < MUTATION_PER_MILLE + REVERSION_PER_MILLE {
        wild
    } else {
        allele
    }
}

/// A brood size in `[fewest, most]`.
fn brood_size(brood: [u8; 2], rng: &mut Pcg32) -> u8 {
    let [low, high] = brood;
    // At most 16 values: the cast is exact.
    #[allow(clippy::cast_possible_truncation)]
    let extra = (rng.next_u32() % (u32::from(high - low) + 1)) as u8;
    low + extra
}

impl World {
    /// The egg clutches waiting to hatch, oldest first.
    #[must_use]
    pub fn clutches(&self) -> &[Clutch] {
        &self.creatures.clutches
    }

    /// The colour morph `creature` shows (an index into its species'
    /// [`crate::Species::morphs`]): its alleles' if they match, otherwise
    /// the wildtype.
    #[must_use]
    pub fn creature_morph(&self, creature: &Creature) -> u8 {
        self.kind(creature.species)
            .map_or(0, |k| phenotype(creature.alleles, k.wild))
    }

    /// Skips `ticks` of every breeding clock without ticking the world: a
    /// female's rest and the time she has left to carry her young (she
    /// gives birth on the next tick at the earliest), and every clutch's
    /// time to hatch (on the next whole second at the earliest), as a
    /// host's debug tools or tests do.
    pub fn skip_breeding_time(&mut self, ticks: u32) {
        for c in &mut self.creatures.list {
            c.rest = c.rest.saturating_sub(ticks);
            if c.carrying > 0 {
                c.carrying = c.carrying.saturating_sub(ticks).max(1);
            }
        }
        for clutch in &mut self.creatures.clutches {
            clutch.hatch = clutch.hatch.saturating_sub(u64::from(ticks));
        }
    }

    /// The sex a creature added to the tank takes: whichever its species
    /// has fewer of, a male on a tie.
    pub(super) fn default_sex(&self, species: SpeciesId) -> bool {
        let (mut males, mut females) = (0_usize, 0_usize);
        for c in self.creatures.list.iter().filter(|c| c.species == species) {
            if c.female {
                females += 1;
            } else {
                males += 1;
            }
        }
        females < males
    }

    /// Starts a tick's breeding: on check ticks, the adult males of every
    /// species that breeds.
    pub(super) fn breed_tick(&self, list: &[Creature]) -> BreedTick {
        let males = self.tick.is_multiple_of(BREED_CHECK_TICKS).then(|| {
            list.iter()
                .filter(|c| {
                    !c.female
                        && c.held_until < self.tick
                        && self
                            .kind(c.species)
                            .is_some_and(|k| k.breeding.is_some() && c.growth >= k.maturity)
                })
                .map(|c| (c.species, c.alleles))
                .collect()
        });
        BreedTick {
            males,
            population: list.len(),
            births: Vec::new(),
            lays: Vec::new(),
        }
    }

    /// One tick of a living creature's breeding: its rest and pregnancy
    /// count down, a carrying female gives birth when hers is over, and a
    /// ready female may conceive on a check tick.
    pub(super) fn breed(&self, c: &mut Creature, t: &mut BreedTick) {
        let Some(k) = self.kind(c.species) else {
            return;
        };
        let Some(b) = k.breeding else {
            return;
        };
        c.rest = c.rest.saturating_sub(1);
        if c.carrying > 0 {
            c.carrying -= 1;
            if c.carrying == 0 {
                let mut rng = self.breed_rng(c);
                for _ in 0..brood_size(b.brood, &mut rng) {
                    let female = rng.next_u32() & 1 == 1;
                    t.births.push(Birth {
                        species: c.species,
                        x: i64::from(c.x >> 8),
                        y: i64::from(c.y >> 8),
                        female,
                        alleles: inherit(c.alleles, c.sire, k.morphs, k.wild, &mut rng),
                    });
                }
                c.rest = b.rest;
            }
            return;
        }
        let Some(males) = &t.males else {
            return;
        };
        if !c.female
            || c.rest > 0
            || c.energy < ENERGY_RESERVE / 2
            || c.stress > 0
            || c.held_until >= self.tick
            || self.life_stage(c) != LifeStage::Adult
        {
            return;
        }
        let partners = males.iter().filter(|(s, _)| *s == c.species).count();
        let (cap, room) = (
            self.creatures.cap,
            self.creatures.cap.saturating_sub(t.population),
        );
        if partners == 0 || room == 0 {
            return;
        }
        let mut rng = self.breed_rng(c);
        // The cap is at most 128, so the casts are exact.
        #[allow(clippy::cast_possible_truncation)]
        let (cap, room) = (cap as u32, room as u32);
        if rng.next_u32() % (CONCEIVE_ODDS * cap) >= room {
            return;
        }
        let pick = rng.next_u32() as usize % partners;
        let Some(&(_, sire)) = males.iter().filter(|(s, _)| *s == c.species).nth(pick) else {
            return;
        };
        if b.eggs {
            t.lays.push(Lay {
                species: c.species,
                body: self.creature_body(c),
                mother: c.alleles,
                sire,
                count: brood_size(b.brood, &mut rng),
                days: b.days,
            });
            c.rest = b.rest;
        } else {
            c.carrying = b.days.max(1);
            c.sire = sire;
        }
    }

    fn breed_rng(&self, c: &Creature) -> Pcg32 {
        chunk_stream(self.seed, self.tick, BREED_STREAM + u64::from(c.id.0))
    }

    /// Ends a tick's breeding, after the creatures moved: the young are
    /// born, clutches laid and, once a second, due clutches hatch. Returns
    /// whether anything was added.
    pub(super) fn finish_breeding(&mut self, t: BreedTick) -> bool {
        let mut changed = false;
        for b in t.births {
            changed |= self
                .spawn(b.species, b.x, b.y, 0, b.female, b.alleles, 0)
                .is_ok();
        }
        for lay in &t.lays {
            changed |= self.lay(lay);
        }
        if self.tick.is_multiple_of(u64::from(TICKS_PER_SECOND))
            && !self.creatures.clutches.is_empty()
        {
            changed |= self.hatch();
        }
        changed
    }

    /// Lays a clutch on water cells around the mother's body that touch
    /// something solid, then on the bed (or whatever lies) below her, never
    /// under a creature. Returns whether any egg was laid.
    fn lay(&mut self, lay: &Lay) -> bool {
        let Some(egg) = self.creatures.egg else {
            return false;
        };
        if self.creatures.clutches.len() >= MAX_CLUTCHES || lay.body.is_empty() {
            return false;
        }
        let water = self.creatures.water;
        let area = Rect {
            x0: lay.body.x0.saturating_sub(EGG_REACH),
            y0: lay.body.y0.saturating_sub(EGG_REACH).max(self.surface_row),
            x1: (lay.body.x1 + EGG_REACH).min(self.width),
            y1: (lay.body.y1 + EGG_REACH).min(self.height),
        };
        // Not under any creature (her included), which would then overlap
        // a solid cell.
        let bodies: Vec<Rect> = self
            .creatures
            .list
            .iter()
            .map(|c| self.creature_body(c).intersect(area))
            .filter(|b| !b.is_empty())
            .collect();
        let mut sticky = Vec::new();
        for y in area.y0..area.y1 {
            for x in area.x0..area.x1 {
                let i = y * self.width + x;
                if self.cells[i].material == water
                    && !lay.body.contains(x, y)
                    && !bodies.iter().any(|b| b.contains(x, y))
                    && self.touches_solid(x, y)
                {
                    sticky.push(i);
                }
            }
        }
        // Eggs with nothing to stick to sink: in each column, to the first
        // cell resting on something below her.
        let mut sunk = Vec::new();
        for x in area.x0..area.x1 {
            let mut y = area.y1 - 1;
            if self.cells[y * self.width + x].material != water {
                continue;
            }
            while y + 1 < self.height && self.cells[(y + 1) * self.width + x].material == water {
                y += 1;
            }
            let i = y * self.width + x;
            let covered = self
                .creatures
                .list
                .iter()
                .any(|c| self.creature_body(c).contains(x, y));
            if !covered && !sticky.contains(&i) {
                sunk.push(i);
            }
        }
        let id = self.next_clutch_id();
        let mut rng = chunk_stream(self.seed, self.tick, CLUTCH_STREAM + u64::from(id));
        let mut eggs = Vec::with_capacity(usize::from(lay.count));
        for spots in [&mut sticky, &mut sunk] {
            // A partial shuffle: random spots, those beside her first.
            while eggs.len() < usize::from(lay.count) && !spots.is_empty() {
                let at = rng.next_u32() as usize % spots.len();
                eggs.push(spots.swap_remove(at));
            }
        }
        if eggs.is_empty() {
            return false;
        }
        eggs.sort_unstable();
        for &i in &eggs {
            self.write_cell(i % self.width, i / self.width, egg, &mut None);
            self.cells[i].state = id;
        }
        self.creatures.clutches.push(Clutch {
            id,
            species: lay.species,
            mother: lay.mother,
            sire: lay.sire,
            hatch: self.tick + u64::from(lay.days),
            // Cell indices are below MAX_SIDE², inside u32.
            eggs: eggs
                .into_iter()
                .map(|i| u32::try_from(i).unwrap_or(u32::MAX))
                .collect(),
        });
        true
    }

    /// Whether cell `(x, y)` touches the grid's edge or a cell no creature
    /// swims through (glass, rock, plants, substrate, eggs).
    fn touches_solid(&self, x: usize, y: usize) -> bool {
        if x == 0 || y == 0 || x + 1 == self.width || y + 1 == self.height {
            return true;
        }
        [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
            .into_iter()
            .any(|(nx, ny)| {
                !self.creatures.swim[usize::from(self.cells[ny * self.width + nx].material.0)]
            })
    }

    /// An id no clutch has (never 0).
    fn next_clutch_id(&mut self) -> u16 {
        loop {
            let id = self.creatures.next_clutch.max(1);
            self.creatures.next_clutch = id.wrapping_add(1).max(1);
            if !self.creatures.clutches.iter().any(|c| c.id == id) {
                return id;
            }
        }
    }

    /// Hatches every clutch that is due: each egg still in place becomes a
    /// fry, or detritus if the tank is at its cap. Returns whether any
    /// clutch hatched.
    fn hatch(&mut self) -> bool {
        let tick = self.tick;
        if !self.creatures.clutches.iter().any(|c| c.hatch <= tick) {
            return false;
        }
        let (due, waiting): (Vec<_>, Vec<_>) = core::mem::take(&mut self.creatures.clutches)
            .into_iter()
            .partition(|c| c.hatch <= tick);
        self.creatures.clutches = waiting;
        let egg = self.creatures.egg;
        for clutch in due {
            let Some(k) = self.kind(clutch.species).copied() else {
                continue;
            };
            let mut rng = chunk_stream(self.seed, tick, CLUTCH_STREAM + u64::from(clutch.id));
            for &i in &clutch.eggs {
                let i = i as usize;
                let cell = self.cells[i];
                if Some(cell.material) != egg || cell.state != clutch.id {
                    continue;
                }
                let (x, y) = (i % self.width, i / self.width);
                let alleles = inherit(clutch.mother, clutch.sire, k.morphs, k.wild, &mut rng);
                let female = rng.next_u32() & 1 == 1;
                if self.creatures.list.len() >= self.creatures.cap {
                    let rot = self.creatures.detritus.unwrap_or(self.creatures.water);
                    self.write_cell(x, y, rot, &mut None);
                    continue;
                }
                let water = if y < self.surface_row {
                    self.creatures.air
                } else {
                    self.creatures.water
                };
                self.write_cell(x, y, water, &mut None);
                let (x, y) = (i64::try_from(x).unwrap_or(0), i64::try_from(y).unwrap_or(0));
                // No room (the egg is boxed in): it is lost.
                let _ = self.spawn(clutch.species, x, y, 0, female, alleles, 0);
            }
        }
        true
    }

    /// Once a second: every hungry adult predator touching prey small
    /// enough may eat it. Returns whether any creature was eaten.
    pub(super) fn predation(&self, list: &mut Vec<Creature>) -> bool {
        let mut eaten: Vec<usize> = Vec::new();
        for i in 0..list.len() {
            let p = &list[i];
            let Some(k) = self.kind(p.species) else {
                continue;
            };
            if k.prey_mm == 0
                || eaten.contains(&i)
                || p.growth < k.maturity
                || !p.hungry()
                || p.held_until >= self.tick
            {
                continue;
            }
            let b = self.creature_body(p);
            let reach = Rect {
                x0: b.x0.saturating_sub(k.reach),
                y0: b.y0.saturating_sub(k.reach),
                x1: b.x1 + k.reach,
                y1: b.y1 + k.reach,
            };
            let prey = (0..list.len()).find(|&j| {
                let q = &list[j];
                j != i
                    && !eaten.contains(&j)
                    && q.held_until < self.tick
                    && self.length_mm(q) <= k.prey_mm
                    && !reach.intersect(self.creature_body(q)).is_empty()
            });
            let Some(j) = prey else {
                continue;
            };
            let mut rng = chunk_stream(self.seed, self.tick, PREY_STREAM + u64::from(p.id.0));
            if !rng.next_u32().is_multiple_of(STRIKE_ODDS) {
                continue;
            }
            eaten.push(j);
            let p = &mut list[i];
            p.satiety = FULL;
            p.energy = p.energy.saturating_add(PREY_ENERGY).min(ENERGY_RESERVE);
            p.eaten += 1;
        }
        if eaten.is_empty() {
            return false;
        }
        let mut index = 0;
        list.retain(|_| {
            index += 1;
            !eaten.contains(&(index - 1))
        });
        true
    }

    /// A creature's body length in mm as grown so far (its species' length
    /// at its size).
    fn length_mm(&self, c: &Creature) -> u32 {
        self.kind(c.species).map_or(0, |k| {
            let percent = super::lifecycle::grown_percent(c.growth, k.maturity);
            // At most a few hundred mm.
            u32::try_from(u64::from(k.length_mm) * percent / 100).unwrap_or(u32::MAX)
        })
    }

    /// The clutches from a save: ids unique and not 0, alleles among the
    /// species' morphs, eggs inside the grid in ascending order, at most
    /// [`MAX_CLUTCHES`] of them.
    pub(super) fn load_clutches(&self, saved: Vec<v3::Clutch>) -> Result<Vec<Clutch>, SaveError> {
        if saved.len() > MAX_CLUTCHES {
            return Err(SaveError::Invalid(format!("{} clutches", saved.len())));
        }
        let mut clutches: Vec<Clutch> = Vec::with_capacity(saved.len());
        for c in saved {
            let invalid = |what: &str| SaveError::Invalid(format!("clutch {}: {what}", c.id));
            let species = SpeciesId(c.species);
            let k = self
                .kind(species)
                .ok_or_else(|| invalid("unknown species"))?;
            if c.id == 0 || clutches.iter().any(|o| o.id == c.id) {
                return Err(invalid("duplicate id"));
            }
            if c.mother.iter().chain(&c.sire).any(|&a| a >= k.morphs)
                || c.eggs.len() > usize::from(crate::species::MAX_BROOD)
                || !c.eggs.is_sorted_by(|a, b| a < b)
            {
                return Err(invalid("morph or eggs out of range"));
            }
            for &e in &c.eggs {
                self.cell_index(e, "an egg")?;
            }
            clutches.push(Clutch {
                id: c.id,
                species,
                mother: c.mother,
                sire: c.sire,
                hatch: c.hatch,
                eggs: c.eggs,
            });
        }
        Ok(clutches)
    }

    pub(super) fn hash_breeding(&self, h: &mut Fnv64) {
        let cr = &self.creatures;
        h.write(&cr.next_clutch.to_le_bytes());
        h.write(&(cr.clutches.len() as u64).to_le_bytes());
        for c in &cr.clutches {
            h.write(&c.id.to_le_bytes());
            h.write(&[c.species.0, c.mother[0], c.mother[1], c.sire[0], c.sire[1]]);
            h.write(&c.hatch.to_le_bytes());
            h.write(&(c.eggs.len() as u64).to_le_bytes());
            for e in &c.eggs {
                h.write(&e.to_le_bytes());
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_lossless
)]
mod tests {
    use super::*;

    fn rng(n: u64) -> Pcg32 {
        chunk_stream(7, n, 3)
    }

    #[test]
    fn a_morph_shows_only_when_both_alleles_match() {
        assert_eq!(phenotype([2, 2], 1), 2);
        assert_eq!(phenotype([1, 1], 1), 1);
        assert_eq!(phenotype([0, 2], 1), 1);
        assert_eq!(phenotype([1, 3], 1), 1);
    }

    /// The model's rates, over many draws: a parent's two alleles pass
    /// equally often, about 0.5% mutate into a morph other than the
    /// wildtype and about 1% revert.
    #[test]
    fn alleles_pass_mutate_and_revert_at_their_rates() {
        let (mut first, mut mutated, mut reverted) = (0, 0, 0);
        let n = 200_000;
        for i in 0..n {
            // Mother red/blue (0/2), father yellow/yellow (3/3); wild is 1.
            let [m, f] = inherit([0, 2], [3, 3], 4, 1, &mut rng(i));
            match m {
                0 => first += 1,
                2 => {}
                1 => reverted += 1,
                _ => mutated += 1,
            }
            match f {
                3 => {}
                1 => reverted += 1,
                _ => mutated += 1,
            }
        }
        let share = |k: u32, of: u64| f64::from(k) / of as f64;
        assert!((share(first, n) - 0.4925).abs() < 0.01, "{first}");
        // A mutation lands on the allele's own morph a third of the time
        // here (three non-wild morphs), so a third go unseen.
        let seen = MUTATION_PER_MILLE as f64 / 1000.0 * 2.0 / 3.0;
        assert!((share(mutated, 2 * n) - seen).abs() < 0.001, "{mutated}");
        let back = REVERSION_PER_MILLE as f64 / 1000.0;
        assert!((share(reverted, 2 * n) - back).abs() < 0.001, "{reverted}");
        // A species with one morph never mutates.
        for i in 0..1000 {
            assert_eq!(inherit([0, 0], [0, 0], 1, 0, &mut rng(i)), [0, 0]);
        }
    }

    /// The seeded multi-generation test of the allele model alone, on a
    /// large population mating at random: lines of one morph keep breeding
    /// true, while a cross of two morphs gives wildtype young and the mixed
    /// line stays mostly wildtype, drifting further towards it.
    #[test]
    fn same_morph_lines_breed_true_and_mixed_lines_revert() {
        const WILD: u8 = 1;
        let generation = |parents: &[[u8; 2]], g: u64| -> Vec<[u8; 2]> {
            (0..400_u64)
                .map(|i| {
                    let mut r = chunk_stream(11, g, i);
                    let n = parents.len() as u32;
                    let mother = parents[(r.next_u32() % n) as usize];
                    let sire = parents[(r.next_u32() % n) as usize];
                    inherit(mother, sire, 4, WILD, &mut r)
                })
                .collect()
        };
        let share = |pop: &[[u8; 2]], morph: u8| {
            pop.iter().filter(|a| phenotype(**a, WILD) == morph).count() as f64 / pop.len() as f64
        };
        for morph in [0, 2] {
            let mut pop = vec![[morph, morph]; 400];
            for g in 0..10 {
                pop = generation(&pop, g);
                assert!(share(&pop, morph) > 0.75, "gen {g}: {}", share(&pop, morph));
            }
        }
        // Red (0) crossed with blue (2): every F1 is wildtype but mutants.
        let mut r = rng(0);
        let f1: Vec<_> = (0..400)
            .map(|_| inherit([0, 0], [2, 2], 4, WILD, &mut r))
            .collect();
        assert!(share(&f1, WILD) > 0.97);
        let mut pop = f1;
        let mut wild = Vec::new();
        for g in 0..10 {
            pop = generation(&pop, 100 + g);
            wild.push(share(&pop, WILD));
        }
        assert!(wild.iter().all(|&w| w > 0.45), "{wild:?}");
        assert!(wild[9] > wild[0], "{wild:?}");
    }

    #[test]
    fn broods_stay_in_range() {
        let mut r = rng(1);
        let sizes: Vec<_> = (0..500).map(|_| brood_size([3, 8], &mut r)).collect();
        assert!(sizes.iter().all(|s| (3..=8).contains(s)));
        assert!(sizes.contains(&3) && sizes.contains(&8));
        assert_eq!(brood_size([5, 5], &mut r), 5);
    }
}
