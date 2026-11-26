//! Resting at night (SPEC §2.4, #195).
//!
//! The host injects the local time of day ([`World::set_time_of_day`]; the
//! simulation never reads the wall clock). Each creature has a bedtime of
//! its own between [`BEDTIME`] and an hour after, and wakes between
//! [`WAKE_TIME`] and an hour after, both fixed for life by a hash of the
//! seed and its id, so a tank doesn't drop off or wake as one.
//!
//! - **Falling asleep.** At its bedtime a creature that isn't held,
//!   frightened or chasing settles where its species rests
//!   ([`RestSpot`]): a swimmer picks a spot (the nearest plant within
//!   [`PLANT_SEARCH`] cells, the surface or the bed) and drifts there at a
//!   third of its cruising speed, then holds still; a walker sinks back to
//!   the bed and a crawler stops on the face it holds. Live food and
//!   nocturnal species ([`RestSpot::Never`]) stay up.
//! - **Asleep** it costs less than awake: it skips steering, eating and
//!   habits, and once still it neither moves nor redraws. It burns energy
//!   at a quarter of the rate, so a well-fed creature lasts the night.
//! - **Waking.** It wakes at its wake time, or when disturbed: the net
//!   holds it, it takes fright, the avoided cursor comes near, the keeper
//!   pours something near it, or food it eats goes in anywhere (feeding).
//!   An air-breather breathes four times slower asleep; a surface fish
//!   breathes where it rests, and any other wakes to go up for air and
//!   settles again once it has breathed. Woken at night, it stays up for
//!   [`WOKEN_TICKS`] (two minutes: long enough to eat) before settling
//!   again.
//!
//! The same pour hook makes awake curious fish investigate nearby non-food.
//!
//! Sleep and the woken counter are saved (format version 15) and hashed
//! only while set, so a tank by day hashes as before.

use super::pursuit::SENSE_EVERY;
use super::quirks::salt;
use super::{
    Creature, Kind, Move, ONE, REACHED, TICKS_PER_SECOND, World, Zone, centre_of, dist, steer,
    to_i64, zone,
};
use crate::daylight::MINUTES_PER_DAY;
use crate::material::MaterialId;
use crate::plants::Part;
use crate::rng::chunk_stream;
use crate::save::{SaveError, v15};
use crate::species::RestSpot;

/// Distance in fixed point from a curious fish's centre to a pour's centre.
const POUR_INTEREST_REACH: i64 = 24 * ONE as i64;

/// The earliest bedtime, in minutes after midnight (21:00, as the tank
/// light dims towards night).
pub const BEDTIME: u16 = 21 * 60;
/// The earliest wake time, in minutes after midnight (06:00, in the dawn
/// ramp).
pub const WAKE_TIME: u16 = 6 * 60;
/// Each creature's own bedtime and wake time fall up to this many minutes
/// after [`BEDTIME`] and [`WAKE_TIME`].
pub const STAGGER_MINUTES: u16 = 60;
/// Ticks a creature woken at night stays up before it may settle again
/// (two minutes).
#[allow(clippy::cast_possible_truncation)] // 2400
pub const WOKEN_TICKS: u16 = 120 * TICKS_PER_SECOND as u16;
/// How far (each way, in cells) a swimmer that rests among plants looks
/// for one.
pub const PLANT_SEARCH: i64 = 48;
/// A pour within this many cells of a sleeping creature's body wakes it.
pub const DISTURB_REACH: i64 = 12;
/// Asleep, it burns one tick of energy in this many.
const SLEEP_BURN_EVERY: u64 = 4;

impl World {
    /// Sets the local time of day, in minutes after midnight (taken modulo
    /// a day), that creatures rest by (SPEC §2.4). Hosts set it with the
    /// tank's lighting; noon by default. Not saved: a host sets it again
    /// after loading.
    pub fn set_time_of_day(&mut self, minute: u16) {
        self.creatures.minute = minute % MINUTES_PER_DAY;
    }

    /// The local time of day creatures rest by, in minutes after midnight.
    #[must_use]
    pub fn time_of_day(&self) -> u16 {
        self.creatures.minute
    }

    /// Whether it is night for `c`: past its own bedtime and before its
    /// own wake time.
    fn night_for(&self, c: &Creature) -> bool {
        let q = self.quirks(c);
        // Below STAGGER_MINUTES.
        let own = |salt| u16::try_from(q.percent(salt, 0, u64::from(STAGGER_MINUTES) - 1));
        let (Ok(bed), Ok(wake)) = (own(salt::BEDTIME), own(salt::WAKE)) else {
            return false;
        };
        let m = self.creatures.minute;
        m >= BEDTIME + bed || m < WAKE_TIME + wake
    }

    /// Updates whether `c` sleeps this tick. Returns whether it is asleep
    /// (and so only rests: see [`World::rest_step`]).
    pub(super) fn sleeps(&self, k: &Kind, c: &mut Creature) -> bool {
        if k.live || k.rests == RestSpot::Never {
            c.wake();
            c.woken = 0;
            return false;
        }
        if !self.night_for(c) {
            c.wake();
            c.woken = 0;
            return false;
        }
        if c.woken > 0 {
            c.woken -= 1;
            return false;
        }
        // A sleeper still senses threats on its sensing tick, as awake
        // creatures do in `pursue`, so a predator or chaser wakes it.
        if c.asleep && (self.tick + u64::from(c.id.0)).is_multiple_of(SENSE_EVERY) {
            self.take_fright(k, c);
        }
        let disturbed = c.fear > 0 || self.cursor_near(k, c);
        if disturbed {
            c.wake();
            c.woken = WOKEN_TICKS;
            return false;
        }
        // An air-breather breathes where it rests if it can, else wakes
        // and goes up until it has.
        if k.breathes_air && self.tick >= c.next_breath && !self.breathe(c) {
            c.wake();
            return false;
        }
        if !c.asleep {
            if c.quarry.is_some() {
                return false;
            }
            c.asleep = true;
            c.food = None;
            c.dodging = 0;
            c.tz = c.z;
            (c.tx, c.ty) = self.rest_spot(k, c);
        }
        true
    }

    /// Whether the avoided cursor is near enough to scare `c`.
    fn cursor_near(&self, k: &Kind, c: &Creature) -> bool {
        self.creatures
            .cursor
            .is_some_and(|(px, py)| dist(c.x - px, c.y - py) < super::steering::cursor_reach(k))
    }

    /// Where a swimmer settling for the night heads (body centre, 1/256
    /// cells); where it is for a walker or crawler.
    fn rest_spot(&self, k: &Kind, c: &Creature) -> (i32, i32) {
        if matches!(k.zone, Zone::Bottom | Zone::Surfaces) {
            return (c.x, c.y);
        }
        let half = k.h * ONE / 2;
        // Its body's top in the water's top row: a surface fish breathes
        // there without waking.
        let top = i32::try_from(self.surface_row).unwrap_or(0) * ONE + half;
        let floor = centre_of(to_i64(self.height) - 1) - half;
        let (x, y) = match k.rests {
            RestSpot::Surface => (c.x, top),
            RestSpot::Bottom => (c.x, floor),
            RestSpot::Plants => self.nearest_plant(c).unwrap_or_else(|| {
                // No plants near: the lower part of its zone.
                let (_, bottom) = zone(k, self.surface_row, self.height);
                (c.x, centre_of(to_i64(bottom) - 1) - half)
            }),
            RestSpot::InPlace | RestSpot::Never => (c.x, c.y),
        };
        (x, y.clamp(top, floor.max(top)))
    }

    /// The centre of the nearest stem or leaf cell within [`PLANT_SEARCH`]
    /// cells of `c`, if any. Runs once as it settles, not every tick.
    fn nearest_plant(&self, c: &Creature) -> Option<(i32, i32)> {
        if self.plant_count() == 0 {
            return None;
        }
        let (cx, cy) = c.cell();
        let span = |v: i64, len: usize| {
            let lo = usize::try_from((v - PLANT_SEARCH).max(0)).unwrap_or(0);
            let hi = usize::try_from((v + PLANT_SEARCH).max(0)).unwrap_or(0);
            lo..=hi.min(len - 1)
        };
        let mut best: Option<(i64, usize, usize)> = None;
        for y in span(cy, self.height) {
            for x in span(cx, self.width) {
                let m = self.cells[y * self.width + x].material;
                if !matches!(self.plants.part(m), Some(Part::Stem | Part::Leaf)) {
                    continue;
                }
                let d = (to_i64(x) - cx).pow(2) + (to_i64(y) - cy).pow(2);
                if best.is_none_or(|(b, _, _)| d < b) {
                    best = Some((d, x, y));
                }
            }
        }
        best.map(|(_, x, y)| (centre_of(to_i64(x)), centre_of(to_i64(y))))
    }

    /// One tick of a sleeping creature. Returns whether it moved.
    pub(super) fn rest_step(&self, k: &Kind, c: &mut Creature) -> bool {
        let before = (c.x, c.y, c.facing_left);
        match k.zone {
            Zone::Surfaces => {
                c.pitch = 0;
                // It holds its face (or its pane); one that fell asleep
                // mid-swim sinks until it touches something it can hold.
                (c.vx, c.vy) = (0, 0);
                if c.pane == super::Pane::Off
                    && !self.on_surface(k, c.turn, c.x, c.y, c.band())
                    && self.fits(k, c.turn, c.x, c.y + ONE, c.band())
                {
                    c.y += ONE;
                }
            }
            Zone::Bottom => {
                c.pitch = 0;
                // It sinks back to the bed and stays, off any face it
                // climbed.
                self.let_go(k, c);
                (c.vx, c.vy) = (0, k.cruise);
                if self.try_move(k, c) != Move::Moved {
                    c.vy = 0;
                }
                c.vx = 0;
            }
            Zone::Mid | Zone::Surface => {
                // Settled, it eases level, so one that rose or sank to its
                // rest spot doesn't sleep nose-up or nose-down.
                if (c.tx, c.ty) == (c.x, c.y) || dist(c.tx - c.x, c.ty - c.y) <= REACHED / 2 {
                    (c.tx, c.ty, c.vx, c.vy) = (c.x, c.y, 0, 0);
                    super::level_off(c);
                } else {
                    steer(k, c, c.tx, c.ty, (k.cruise / 3).max(1), false);
                    if self.try_move(k, c) == Move::Moved {
                        // A drift to rest mirrors on any sideways motion.
                        super::update_swim_heading(c, 0);
                        // Too slow to set a pitch, it eases level on the
                        // way rather than keeping its daytime one.
                        if c.vx.abs().max(c.vy.abs()) < super::PITCH_SPEED {
                            super::level_off(c);
                        }
                    } else {
                        // Settled where something stops it.
                        (c.tx, c.ty, c.vx, c.vy) = (c.x, c.y, 0, 0);
                        super::level_off(c);
                    }
                }
            }
        }
        (c.x, c.y, c.facing_left) != before
    }

    /// Energy and breath back for a sleeping creature this tick: it burns
    /// one tick of energy, and an air-breather's next breath comes one tick
    /// nearer, in [`SLEEP_BURN_EVERY`]. `before` is its energy before the
    /// tick's burn.
    pub(super) fn sleep_refund(&self, k: &Kind, c: &mut Creature, before: u32) {
        if c.asleep && !(self.tick + u64::from(c.id.0)).is_multiple_of(SLEEP_BURN_EVERY) {
            c.energy = (c.energy + 1).min(before);
            if k.breathes_air && c.next_breath > self.tick {
                c.next_breath += 1;
            }
        }
    }

    /// The keeper poured `material` over the disc at `(x, y)`, `radius`
    /// cells: at night it wakes every creature that eats it (they were
    /// fed) and every creature near the pour (disturbed). A nearby pour
    /// that isn't food also frightens an awake shy creature or draws an
    /// awake, calm curious one to investigate.
    pub(in crate::world) fn disturb_creatures(
        &mut self,
        x: i64,
        y: i64,
        radius: u32,
        material: MaterialId,
    ) {
        let reach = i64::from(radius) + DISTURB_REACH;
        let (px, py) = (centre_of(x), centre_of(y));
        let mut list = core::mem::take(&mut self.creatures.list);
        for c in &mut list {
            let Some(k) = self.grown_kind(c) else {
                continue;
            };
            let wake = !k.live && k.rests != RestSpot::Never && self.night_for(c);
            let fed = k.edible[usize::from(material.0)];
            // Netted or burrowed, it can't move: a pour neither frightens
            // nor draws it.
            let held = c.held_until >= self.tick || self.sunk_depth(c) > 0;
            let startle = k.shy && !c.asleep && !fed && !held;
            if k.curious
                && !c.asleep
                && !held
                && c.fear == 0
                && !fed
                && dist(c.x - px, c.y - py) <= POUR_INTEREST_REACH
            {
                (c.tx, c.ty) = self.clamp_to_swim_zone(&k, c, px, py);
                let mut rng = chunk_stream(
                    self.seed,
                    self.tick,
                    (super::CREATURE_STREAM + u64::from(c.id.0)) ^ super::steering::POUR_SALT,
                );
                c.retarget = self.wander_span(c, &mut rng);
            }
            if !(wake || startle) {
                continue;
            }
            let b = self.creature_body(c);
            let near = |v: i64, lo: usize, hi: usize| {
                v + reach >= to_i64(lo) && v - reach < to_i64(hi.max(lo + 1))
            };
            let nearby = near(x, b.x0, b.x1) && near(y, b.y0, b.y1);
            if startle && nearby {
                self.frighten(&k, c, px, py, true);
            }
            if wake && (fed || nearby) {
                c.wake();
                c.woken = WOKEN_TICKS;
            }
        }
        self.creatures.list = list;
    }
}

impl World {
    /// Each creature's rest as saved, in id order (version 15).
    pub(in crate::world) fn save_rest(&self) -> Vec<v15::CreatureRest> {
        self.creatures
            .list
            .iter()
            .map(|c| v15::CreatureRest {
                asleep: c.asleep,
                woken: c.woken,
            })
            .collect()
    }

    /// Restores each creature's rest from a save.
    pub(in crate::world) fn load_rest(
        &mut self,
        saved: &[v15::CreatureRest],
    ) -> Result<(), SaveError> {
        if saved.len() != self.creatures.list.len() {
            return Err(SaveError::Invalid(format!(
                "{} creature rests for {} creatures",
                saved.len(),
                self.creatures.list.len()
            )));
        }
        let never: Vec<bool> = self
            .creatures
            .list
            .iter()
            .map(|c| {
                self.kind(c.species)
                    .is_none_or(|k| k.live || k.rests == RestSpot::Never)
            })
            .collect();
        for ((c, r), never) in self.creatures.list.iter_mut().zip(saved).zip(never) {
            let rested = r.asleep || r.woken > 0;
            if r.woken > WOKEN_TICKS || (r.asleep && r.woken > 0) || (never && rested) {
                return Err(SaveError::Invalid(format!(
                    "creature {}: rest out of range",
                    c.id.0
                )));
            }
            (c.asleep, c.woken) = (r.asleep, r.woken);
        }
        Ok(())
    }
}

impl Creature {
    /// Whether it is asleep for the night.
    #[must_use]
    pub fn asleep(&self) -> bool {
        self.asleep
    }

    /// Ticks it stays up after being woken at night (0: it may sleep).
    #[must_use]
    pub fn woken(&self) -> u32 {
        u32::from(self.woken)
    }

    /// Wakes it: it picks a new wander target at once.
    pub(super) fn wake(&mut self) {
        if self.asleep {
            self.asleep = false;
            self.retarget = 0;
        }
    }
}
