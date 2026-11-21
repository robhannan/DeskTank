//! The life cycle (SPEC §2.4, T9 part 1): age, life stages, growth that
//! depends on feeding, deaths, and corpses that sink and rot into detritus.
//!
//! - **Life clock.** Species give their lifespan and maturity age in real
//!   days; a day of a creature's life passes every [`LIFE_DAY_SECONDS`] of
//!   world time. Each creature's own lifespan is within ±10% of its
//!   species', picked from its id, so a cohort doesn't die on one tick.
//! - **Energy.** Every creature stores up to [`ENERGY_RESERVE`] ticks of
//!   energy. It burns one a tick and each bite adds [`BITE_ENERGY`]; at
//!   zero it starves. It grows (towards maturity, at full rate) while the
//!   reserve is at least half full, at half rate down to an eighth, and not
//!   at all below that, so an underfed fry stays small for longer.
//! - **Stages.** Fry and juveniles are drawn and collide at a fraction of
//!   the adult size, from 35% at hatching to 100% at maturity; a creature
//!   past 80% of its own lifespan is an elder.
//! - **Water quality** is sampled once a second ([`World::water_quality`],
//!   counts kept per field cell, not a grid scan). Below
//!   [`POOR_QUALITY`] a creature builds up stress, faster the fouler the
//!   water, and recovers above it; it dies when the stress reaches
//!   [`STRESS_LIMIT`] (5 minutes in the foulest water).
//! - **Easy mode** (D4) allows only deaths from old age.
//! - **Corpses** sink at [`CORPSE_SINK`] cells a second through anything a
//!   creature swims through, and after [`ROT_AFTER_SECONDS`] release a
//!   detritus cell every [`ROT_EVERY_SECONDS`] into the water they cover,
//!   fading as they go, until a third of their cells have become detritus.

use super::{Creature, CreatureId, Kind, ONE, World, body_box};
use crate::TICKS_PER_SECOND;
use crate::grid::Rect;
use crate::hash::Fnv64;
use crate::material::MaterialId;
use crate::rng::splitmix64;
use crate::species::SpeciesId;

/// World seconds per day of a creature's life (10 minutes): a 5-year neon
/// tetra lives about 300 hours of world time, a cherry shrimp about 90.
pub const LIFE_DAY_SECONDS: u32 = 600;
/// Ticks per day of a creature's life.
pub const LIFE_DAY_TICKS: u32 = LIFE_DAY_SECONDS * TICKS_PER_SECOND;
/// Most energy a creature stores, in ticks it lasts unfed (8 hours).
pub const ENERGY_RESERVE: u32 = 8 * 3600 * TICKS_PER_SECOND;
/// Energy one bite (a food or grazed cell) gives, in ticks (20 minutes).
pub const BITE_ENERGY: u32 = 20 * 60 * TICKS_PER_SECOND;
/// Water quality below which creatures build up stress.
pub const POOR_QUALITY: f32 = 0.5;
/// Stress at which a creature dies of poor water: in thousandths of a
/// second at quality 0 (5 minutes there, longer in less foul water).
pub const STRESS_LIMIT: u32 = 300_000;
/// Stress shed per second in good water.
const STRESS_RECOVERY: u32 = 500;
/// How fast a corpse sinks, in cells per second.
pub const CORPSE_SINK: f32 = 1.0;
/// World seconds after death when a corpse starts to rot.
pub const ROT_AFTER_SECONDS: u32 = 60;
/// World seconds between the detritus cells a rotting corpse releases.
pub const ROT_EVERY_SECONDS: u32 = 2;
/// Most corpses a tank holds; past it the oldest is gone at once.
pub const MAX_CORPSES: usize = 128;
/// Sinking speed in 1/256 cell per tick.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)] // 13, from small constants
const SINK: i32 = (CORPSE_SINK * ONE as f32 / TICKS_PER_SECOND as f32 + 0.5) as i32;
/// Size at hatching, in percent of the adult's.
const HATCH_PERCENT: u64 = 35;
/// Elders are past this percentage of their own lifespan.
const ELDER_PERCENT: u64 = 80;

/// A creature's life stage (SPEC §2.4). Eggs aren't creatures: they are
/// cells of the `egg` material until their clutch hatches (T9 part 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LifeStage {
    /// Newly hatched or born: small, growing fast while fed.
    Fry,
    /// Past a quarter of the way to maturity.
    Juvenile,
    /// Mature and full-sized.
    Adult,
    /// Past 80% of its lifespan.
    Elder,
}

impl LifeStage {
    /// The stage's name, for the inspect card and tests.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Fry => "fry",
            Self::Juvenile => "juvenile",
            Self::Adult => "adult",
            Self::Elder => "elder",
        }
    }

    /// The stage with this name, if any.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Fry, Self::Juvenile, Self::Adult, Self::Elder]
            .into_iter()
            .find(|s| s.name() == name)
    }
}

/// Why a creature died (SPEC §2.4). Temperature comes with E1 and chaos
/// materials with E2. Prey a predator eats (T9 part 2) leaves no corpse,
/// so predation is not a cause here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeathCause {
    /// It reached its lifespan (the only cause in easy mode).
    OldAge,
    /// Its energy ran out.
    Starvation,
    /// It lived too long in foul water.
    PoorWater,
}

impl DeathCause {
    /// Every cause, in save order.
    pub const ALL: [Self; 3] = [Self::OldAge, Self::Starvation, Self::PoorWater];

    /// The cause's name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::OldAge => "old age",
            Self::Starvation => "starvation",
            Self::PoorWater => "poor water",
        }
    }

    pub(crate) fn code(self) -> u8 {
        match self {
            Self::OldAge => 0,
            Self::Starvation => 1,
            Self::PoorWater => 2,
        }
    }

    pub(crate) fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code)).copied()
    }
}

/// A dead creature: a sprite that sinks, then rots into detritus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Corpse {
    pub(super) species: SpeciesId,
    pub(super) cause: DeathCause,
    /// Body centre, 1/256 cells.
    pub(super) x: i32,
    pub(super) y: i32,
    /// Body size in cells when it died.
    pub(super) w: u8,
    pub(super) h: u8,
    pub(super) facing_left: bool,
    /// The depth band it died in, which it sinks in (D21).
    pub(super) band: u8,
    /// Tick it died on.
    pub(super) died: u64,
    /// Detritus cells released so far, of `total`.
    pub(super) released: u16,
    pub(super) total: u16,
    /// The morph it showed and its sex, to draw it as it was.
    pub(super) morph: u8,
    pub(super) female: bool,
}

impl Corpse {
    /// Its species.
    #[must_use]
    pub fn species(&self) -> SpeciesId {
        self.species
    }

    /// The depth band it died and sinks in (D21).
    #[must_use]
    pub fn band(&self) -> u8 {
        self.band
    }

    /// What it died of.
    #[must_use]
    pub fn cause(&self) -> DeathCause {
        self.cause
    }

    /// Body centre in cells (fractional).
    #[must_use]
    pub fn centre(&self) -> (f32, f32) {
        // Positions are at most 8192 cells: exact in f32.
        #[allow(clippy::cast_precision_loss)]
        let f = |v: i32| v as f32 / ONE as f32;
        (f(self.x), f(self.y))
    }

    /// Body size in cells, `(width, height)`, as it lies (belly up).
    #[must_use]
    pub fn size(&self) -> (usize, usize) {
        (usize::from(self.w), usize::from(self.h))
    }

    /// Whether its sprite is mirrored (as it faced when it died).
    #[must_use]
    pub fn facing_left(&self) -> bool {
        self.facing_left
    }

    /// The tick it died on.
    #[must_use]
    pub fn died(&self) -> u64 {
        self.died
    }

    /// The colour morph it showed (see [`World::creature_morph`]).
    #[must_use]
    pub fn morph(&self) -> u8 {
        self.morph
    }

    /// Whether it was female.
    #[must_use]
    pub fn female(&self) -> bool {
        self.female
    }

    /// How far it has rotted away, from 0 (whole) to 1 (gone).
    #[must_use]
    pub fn decay(&self) -> f32 {
        f32::from(self.released) / f32::from(self.total.max(1))
    }
}

impl World {
    /// Whether easy mode is on (D4: only old age kills).
    #[must_use]
    pub fn easy_mode(&self) -> bool {
        self.creatures.easy
    }

    /// Turns easy mode on or off (SPEC §2.4, §4.3). Saved with the tank.
    pub fn set_easy_mode(&mut self, easy: bool) {
        self.creatures.easy = easy;
    }

    /// The population cap: most creatures the tank holds (SPEC §2.4).
    #[must_use]
    pub fn creature_cap(&self) -> usize {
        self.creatures.cap
    }

    /// Sets the population cap, within 1..=[`super::MAX_CREATURE_CAP`] and
    /// never below the creatures already in the tank (live food aside), so the population
    /// never exceeds it. Returns the cap set. Saved with the tank.
    pub fn set_creature_cap(&mut self, cap: usize) -> usize {
        let low = self.stock_count().max(1);
        self.creatures.cap = cap.clamp(low, super::MAX_CREATURE_CAP);
        self.creatures.cap
    }

    /// The corpses in the tank, oldest first.
    #[must_use]
    pub fn corpses(&self) -> &[Corpse] {
        &self.creatures.corpses
    }

    /// The cells a corpse covers.
    #[must_use]
    pub fn corpse_body(&self, corpse: &Corpse) -> Rect {
        body_box(
            i32::from(corpse.w),
            i32::from(corpse.h),
            corpse.x,
            corpse.y,
            self.width,
            self.height,
        )
        .unwrap_or(Rect::EMPTY)
    }

    /// `creature`'s life stage.
    #[must_use]
    pub fn life_stage(&self, creature: &Creature) -> LifeStage {
        let Some(k) = self.kind(creature.species) else {
            return LifeStage::Adult;
        };
        let lifespan = u64::from(self.lifespan_of(k, creature.id));
        if u64::from(creature.age) * 100 >= lifespan * ELDER_PERCENT {
            LifeStage::Elder
        } else if creature.growth >= k.maturity {
            LifeStage::Adult
        } else if creature.growth * 4 >= k.maturity {
            LifeStage::Juvenile
        } else {
            LifeStage::Fry
        }
    }

    /// `creature`'s own lifespan in ticks: its species' within ±10%.
    #[must_use]
    pub fn creature_lifespan(&self, creature: &Creature) -> u32 {
        self.kind(creature.species)
            .map_or(0, |k| self.lifespan_of(k, creature.id))
    }

    /// Body size in cells, `(width, height)`, upright: the adult size, or
    /// less while it grows.
    #[must_use]
    pub fn creature_size(&self, creature: &Creature) -> (usize, usize) {
        self.grown_kind(creature).map_or((0, 0), |k| {
            (
                usize::try_from(k.w).unwrap_or(0),
                usize::try_from(k.h).unwrap_or(0),
            )
        })
    }

    /// Adds a creature as [`World::add_creature`] does, `age` ticks old
    /// and grown as a well-fed one would be by then (a fry at 0).
    ///
    /// # Errors
    ///
    /// As [`World::add_creature`].
    pub fn add_creature_aged(
        &mut self,
        species: SpeciesId,
        x: i64,
        y: i64,
        age: u32,
    ) -> Result<CreatureId, crate::WorldError> {
        self.add_creature_with_traits(
            species,
            x,
            y,
            super::Traits {
                age: Some(age),
                ..super::Traits::default()
            },
        )
    }

    /// Sets a creature's stored energy, in ticks (at most
    /// [`ENERGY_RESERVE`]), as a host's debug tools or tests do. Returns
    /// whether it exists.
    pub fn set_creature_energy(&mut self, id: CreatureId, energy: u32) -> bool {
        self.creature_mut(id)
            .map(|c| c.energy = energy.min(ENERGY_RESERVE))
            .is_some()
    }

    /// Sets a creature's age in ticks (its growth is left as it is), as a
    /// host's debug tools or tests do. Returns whether it exists.
    pub fn set_creature_age(&mut self, id: CreatureId, age: u32) -> bool {
        self.creature_mut(id).map(|c| c.age = age).is_some()
    }

    /// The species' movement numbers with the body scaled to the
    /// creature's growth.
    pub(super) fn grown_kind(&self, c: &Creature) -> Option<Kind> {
        let mut k = *self.kind(c.species)?;
        if c.growth < k.maturity {
            let percent = grown_percent(c.growth, k.maturity);
            k.w = grown_side(k.w, percent);
            k.h = grown_side(k.h, percent);
        }
        Some(k)
    }

    fn lifespan_of(&self, k: &Kind, id: CreatureId) -> u32 {
        let spread = splitmix64(self.seed ^ splitmix64(u64::from(id.0) ^ 0x6c69_6665)) % 21;
        // Lifespans are at most 7300 days: well inside u32 at ±10%.
        u32::try_from(u64::from(k.lifespan) * (90 + spread) / 100).unwrap_or(u32::MAX)
    }

    /// One tick of `c`'s life, before it moves: it ages, burns energy and
    /// grows, and may die. `quality` is this second's water-quality sample
    /// (on sample ticks only). Sets `redraw` when its drawn size or look
    /// changed (it grew a size step or became an elder).
    pub(super) fn live(
        &self,
        c: &mut Creature,
        quality: Option<f32>,
        redraw: &mut bool,
    ) -> Option<DeathCause> {
        let k = self.kind(c.species)?;
        let lifespan = self.lifespan_of(k, c.id);
        let elder = |age: u32| u64::from(age) * 100 >= u64::from(lifespan) * ELDER_PERCENT;
        let was_elder = elder(c.age);
        c.age = c.age.saturating_add(1);
        *redraw |= elder(c.age) != was_elder;
        c.energy = c.energy.saturating_sub(1);
        if c.growth < k.maturity {
            let step = if c.energy >= ENERGY_RESERVE / 2 {
                1
            } else {
                u32::from(
                    c.energy >= ENERGY_RESERVE / 8
                        && (self.tick + u64::from(c.id.0)).is_multiple_of(2),
                )
            };
            if step > 0 {
                let size = |growth: u32| {
                    let percent = grown_percent(growth, k.maturity);
                    (grown_side(k.w, percent), grown_side(k.h, percent))
                };
                let before = size(c.growth);
                c.growth = (c.growth + step).min(k.maturity);
                *redraw |= c.growth == k.maturity || size(c.growth) != before;
            }
        }
        if c.age >= lifespan {
            return Some(DeathCause::OldAge);
        }
        if self.creatures.easy {
            return None;
        }
        if c.energy == 0 {
            return Some(DeathCause::Starvation);
        }
        if let Some(q) = quality {
            if q < POOR_QUALITY {
                // Thousandths of a second at quality 0, 1000 at most.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let add = ((POOR_QUALITY - q) / POOR_QUALITY * 1000.0).round() as u32;
                c.stress = c.stress.saturating_add(add);
            } else {
                c.stress = c.stress.saturating_sub(STRESS_RECOVERY);
            }
            if c.stress >= STRESS_LIMIT {
                return Some(DeathCause::PoorWater);
            }
        }
        None
    }

    /// The corpse a creature leaves, lying where it died.
    pub(super) fn corpse_of(&self, c: &Creature, cause: DeathCause) -> Corpse {
        let (w, h) = self
            .grown_kind(c)
            .map_or((1, 1), |k| (k.w.clamp(1, 255), k.h.clamp(1, 255)));
        let cells = u16::try_from(w * h).unwrap_or(u16::MAX);
        Corpse {
            species: c.species,
            cause,
            x: c.x,
            y: c.y,
            w: u8::try_from(w).unwrap_or(1),
            h: u8::try_from(h).unwrap_or(1),
            facing_left: c.facing_left,
            band: c.band(),
            died: self.tick,
            released: 0,
            total: cells.div_ceil(3).max(1),
            morph: self.creature_morph(c),
            female: c.female,
        }
    }

    /// Adds corpses, dropping the oldest past [`MAX_CORPSES`].
    pub(super) fn bury(&mut self, corpses: impl IntoIterator<Item = Corpse>) {
        let list = &mut self.creatures.corpses;
        list.extend(corpses);
        if list.len() > MAX_CORPSES {
            let extra = list.len() - MAX_CORPSES;
            list.drain(..extra);
        }
    }

    /// Sinks and rots every corpse for one tick. Returns whether any moved,
    /// faded or was gone.
    pub(super) fn update_corpses(&mut self, detritus: Option<MaterialId>) -> bool {
        let mut corpses = core::mem::take(&mut self.creatures.corpses);
        let mut changed = false;
        let (after, every) = (
            u64::from(ROT_AFTER_SECONDS * TICKS_PER_SECOND),
            u64::from(ROT_EVERY_SECONDS * TICKS_PER_SECOND),
        );
        corpses.retain_mut(|c| {
            let (w, h) = (i32::from(c.w), i32::from(c.h));
            if self.fits_box(w, h, c.x, c.y + SINK, Some(c.band)) {
                c.y += SINK;
                changed = true;
            } else {
                // Settle the rest of the way onto whatever stopped it.
                let top = c.y - h * ONE / 2;
                let settled = (top + ONE - 1).div_euclid(ONE) * ONE + h * ONE / 2;
                if settled > c.y && self.fits_box(w, h, c.x, settled, Some(c.band)) {
                    c.y = settled;
                    changed = true;
                }
            }
            let age = self.tick.saturating_sub(c.died);
            if age >= after && (age - after).is_multiple_of(every) {
                if let Some(m) = detritus {
                    self.rot_into(c, m);
                }
                c.released += 1;
                changed = true;
            }
            c.released < c.total
        });
        self.creatures.corpses = corpses;
        changed
    }

    /// Writes one detritus cell into the first water cell the corpse covers
    /// (from its lowest row up), or beside it if it covers none.
    fn rot_into(&mut self, c: &Corpse, detritus: MaterialId) {
        let water = self.creatures.water;
        let Some(b) = body_box(
            i32::from(c.w) + 2,
            i32::from(c.h) + 2,
            c.x,
            c.y,
            self.width,
            self.height,
        )
        .or_else(|| {
            body_box(
                i32::from(c.w),
                i32::from(c.h),
                c.x,
                c.y,
                self.width,
                self.height,
            )
        }) else {
            return;
        };
        for y in (b.y0..b.y1).rev() {
            for x in b.x0..b.x1 {
                if self.cells[y * self.width + x].material == water {
                    self.write_cell(x, y, detritus, &mut None);
                    return;
                }
            }
        }
    }

    pub(super) fn hash_corpses(&self, h: &mut Fnv64) {
        let cr = &self.creatures;
        h.write(&[u8::from(cr.easy)]);
        h.write(&(cr.cap as u64).to_le_bytes());
        h.write(&(cr.corpses.len() as u64).to_le_bytes());
        for c in &cr.corpses {
            h.write(&[
                c.species.0,
                c.cause.code(),
                c.w,
                c.h,
                u8::from(c.facing_left),
                c.morph,
                u8::from(c.female),
                c.band,
            ]);
            h.write(&c.x.to_le_bytes());
            h.write(&c.y.to_le_bytes());
            h.write(&c.died.to_le_bytes());
            h.write(&c.released.to_le_bytes());
            h.write(&c.total.to_le_bytes());
        }
    }
}

/// Size in percent of the adult's after `growth` of `maturity` ticks of
/// growth: from [`HATCH_PERCENT`] to 100.
pub(super) fn grown_percent(growth: u32, maturity: u32) -> u64 {
    if growth >= maturity {
        return 100;
    }
    HATCH_PERCENT + (100 - HATCH_PERCENT) * u64::from(growth) / u64::from(maturity.max(1))
}

/// A body side `full` cells long at `percent` of its size: at least a cell,
/// and odd when the full side is, so a grown body stays centred on whole
/// cells as the adult's does.
fn grown_side(full: i32, percent: u64) -> i32 {
    let full_u = u64::try_from(full).unwrap_or(1);
    let mut side = i32::try_from((full_u * percent + 50) / 100)
        .unwrap_or(1)
        .max(1);
    if side > 1 && side % 2 != full % 2 {
        side -= 1;
    }
    side.min(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grown_sides_keep_parity_and_bounds() {
        assert_eq!(grown_side(7, 100), 7);
        assert_eq!(grown_side(7, 35), 1); // 2.45 → 2, odd → 1
        assert_eq!(grown_side(7, 60), 3); // 4.2 → 4 → 3
        assert_eq!(grown_side(8, 35), 2); // 2.8 → 3 → 2
        assert_eq!(grown_side(3, 35), 1);
        assert_eq!(grown_side(1, 35), 1);
        for full in 1..=16 {
            for p in 35..=100 {
                let s = grown_side(full, p);
                assert!((1..=full).contains(&s));
                assert!(s == 1 || s % 2 == full % 2);
            }
        }
    }

    #[test]
    fn causes_round_trip_their_codes() {
        for c in DeathCause::ALL {
            assert_eq!(DeathCause::from_code(c.code()), Some(c));
        }
        assert_eq!(DeathCause::from_code(4), None);
        for s in ["fry", "juvenile", "adult", "elder"] {
            assert_eq!(LifeStage::from_name(s).map(LifeStage::name), Some(s));
        }
    }
}
