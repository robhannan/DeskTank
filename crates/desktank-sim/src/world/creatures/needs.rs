//! Satisfaction describes the fish's state. Species weights and individual
//! quirks determine how much a deficit matters when choosing what to do.
//! A plant's care factors are read the same way ([`World::plant_needs`]),
//! so one meter panel serves both.

use rand_core::Rng as _;
use rand_pcg::Pcg32;

use super::{BITE, Creature, FULL, Kind, ONE, REACHED, SaveError, World, dist, extent, salt, z_of};

/// A need tracked by the simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// How full the creature is.
    Hunger,
    /// How much company the creature has nearby.
    Social,
    /// How hidden the creature is: full is in plant cover, empty is long
    /// exposed in open water.
    Shelter,
    /// How well the light where a plant grows suits its species (a plant's
    /// care factor, [`World::plant_needs`]): full at the species' optimum,
    /// less above and below it.
    Light,
    /// How well the CO₂ around a plant suits its species.
    Co2,
    /// How well the nutrients around a plant suit its species.
    Nutrients,
}

impl Need {
    /// A short label for a meter whose empty end is the need unmet and
    /// whose full end is it satisfied ("Hunger" full is not hungry,
    /// "Social" full is in company, "Shelter" full is under cover).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hunger => "Hunger",
            Self::Social => "Social",
            Self::Shelter => "Shelter",
            Self::Light => "Light",
            Self::Co2 => "CO₂",
            Self::Nutrients => "Nutrients",
        }
    }
}

/// The satisfaction of one need, independent of its influence on steering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NeedReading {
    /// Which need this describes.
    pub need: Need,
    /// From 0 (unsatisfied) to 100 (fully satisfied).
    pub satisfaction: u8,
}

impl NeedReading {
    /// How well the need is met, in words.
    #[must_use]
    pub const fn level(self) -> NeedLevel {
        NeedLevel::of(self.satisfaction)
    }

    /// The reading as plain text for a screen reader or summary: `Hunger 49%,
    /// low`.
    #[must_use]
    pub fn describe(self) -> String {
        format!(
            "{} {}%, {}",
            self.need.label(),
            self.satisfaction,
            self.level().name()
        )
    }
}

/// How well a need is met, from a satisfaction percentage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NeedLevel {
    /// 0 to 25: critically unmet. A fish that has just taken its first
    /// bite is still here.
    Critical,
    /// 26 to 49: low.
    Low,
    /// 50 to 74: middling.
    Middling,
    /// 75 to 100: comfortable. At 75, there is room for another bite.
    Comfortable,
}

impl NeedLevel {
    /// The level of `satisfaction` percent (values over 100 count as 100).
    #[must_use]
    pub const fn of(satisfaction: u8) -> Self {
        match satisfaction {
            0..=25 => Self::Critical,
            26..=49 => Self::Low,
            50..=74 => Self::Middling,
            _ => Self::Comfortable,
        }
    }

    /// One lower-case word for it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::Low => "low",
            Self::Middling => "middling",
            Self::Comfortable => "comfortable",
        }
    }
}

const PERCENT: u8 = 100;
const MAX_COMMITMENT: u8 = 23;
/// How much a fish values staying with a school it is already part of, in
/// the units of a deficit. It is enough to keep a fish that has just eaten
/// beside its mates when the next flake is some way off, and too little to
/// hold back a hungry one.
const COMPANY: u64 = 50;
/// Ticks between looks for cover, as for any other sense.
const SHELTER_SAMPLE: u64 = super::pursuit::SENSE_EVERY;
/// How far a fish that wants cover looks for it, in cells (20 cm).
const SHELTER_SENSE: i32 = super::pursuit::HUNT_SENSE;
/// Exposure points a look in open water adds (a 2% rise a second, so fifty
/// seconds from sheltered to wholly exposed) and a look in cover takes off
/// (five times as fast, so a hidden fish settles in at most ten seconds).
const EXPOSE_STEP: u8 = 1;
const SETTLE_STEP: u8 = 5;
/// Exposure a fish puts up with before open water sends it for cover,
/// before its own boldness scales it. A fish already hiding stays until it
/// has settled.
const EXPOSURE_TOLERANCE: u64 = 30;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Action {
    #[default]
    Idle = 0,
    Forage = 1,
    School = 2,
    Wander = 3,
    /// Heads for plant cover, or holds still in it until settled.
    Cover = 4,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct State {
    pub action: Action,
    remaining: u8,
    /// The need a species tracks over time, which is never both at once: the
    /// company seen at the last decision (a schooler's social satisfaction)
    /// or the exposure built up in open water (a sheltering fish's).
    observed: u8,
}

impl State {
    pub(super) fn at_spawn(weights: Option<crate::species::NeedWeights>) -> Self {
        Self {
            observed: if weights.is_some_and(|w| w.social > 0) {
                75
            } else {
                0
            },
            ..Self::default()
        }
    }

    /// Drop the old decision while keeping the last observation.
    pub(super) fn interrupt(&mut self) {
        self.remaining = 0;
        self.action = Action::Idle;
    }

    pub(super) fn encode(self) -> [u8; 3] {
        [self.action as u8, self.remaining, self.observed]
    }

    fn decode([action, remaining, observed]: [u8; 3]) -> Option<Self> {
        let action = match action {
            0 => Action::Idle,
            1 => Action::Forage,
            2 => Action::School,
            3 => Action::Wander,
            4 => Action::Cover,
            _ => return None,
        };
        (observed <= PERCENT
            && remaining <= MAX_COMMITMENT
            && (remaining != 0 || action == Action::Idle))
            .then_some(Self {
                action,
                remaining,
                observed,
            })
    }
}

/// Whether a species with `weights` could have made `state`: only the
/// actions and observation of the needs it supports.
fn allowed(weights: Option<crate::species::NeedWeights>, state: State) -> bool {
    let Some(w) = weights else {
        return state == State::default();
    };
    (w.social > 0 || state.action != Action::School)
        && (w.shelter > 0 || state.action != Action::Cover)
        && (w.social > 0 || w.shelter > 0 || state.observed == 0)
}

fn satiety_percent(c: &Creature) -> u8 {
    u8::try_from(u32::from(c.satiety.min(FULL)) * u32::from(PERCENT) / u32::from(FULL))
        .unwrap_or(PERCENT)
}

/// How strongly food draws the fish, from 0 (not at all) to 100 (starving).
///
/// Satiety is a stock, but a flake is a small meal. The first bite's worth
/// of satiety counts three times as much towards feeling fed as the rest, so
/// a fish that has just eaten one flake gives the next little attention and
/// recovers its appetite over the half-minute the bite lasts. It reads only
/// the stored satiety, so a repeated feeding needs no extra saved state.
fn appetite(c: &Creature) -> u64 {
    const {
        assert!(BITE as u64 * 4 == FULL as u64);
    }
    let satiety = u64::from(satiety_percent(c));
    let bite = u64::from(BITE) * u64::from(PERCENT) / u64::from(FULL);
    let (first, rest) = (satiety.min(bite), satiety.saturating_sub(bite));
    u64::from(PERCENT).saturating_sub(first * 3 + rest / 3)
}

/// Discount the benefit by travel distance in whole cells, with roughly
/// half the benefit remaining at 48 cells.
fn benefit(score: u64, distance: i64) -> u64 {
    score * 48 / (48 + distance.unsigned_abs() / u64::from(ONE.unsigned_abs()))
}

impl World {
    /// The supported need readings for this creature, in display order.
    #[must_use]
    pub fn creature_needs(&self, c: &Creature) -> Vec<NeedReading> {
        let Some(weights) = self.kind(c.species).and_then(|k| k.needs) else {
            return Vec::new();
        };
        [Need::Hunger, Need::Social, Need::Shelter]
            .into_iter()
            .filter(|&need| weights.supports(need))
            .map(|need| NeedReading {
                need,
                satisfaction: match need {
                    Need::Hunger => satiety_percent(c),
                    Need::Social => c.needs.observed,
                    Need::Shelter => PERCENT.saturating_sub(c.needs.observed),
                    Need::Light | Need::Co2 | Need::Nutrients => 0,
                },
            })
            .collect()
    }

    /// A look for cover, every [`SHELTER_SAMPLE`] ticks at a tick of the
    /// fish's own: exposure builds while it is out in the open and settles
    /// while it is in cover, so it is a state of the fish, not of one moment.
    /// It runs from `steering_target`, so a fish that is asleep, held or
    /// climbing takes no look, and a tank that is away keeps what it had.
    pub(super) fn observe_shelter(&self, k: &Kind, c: &mut Creature) {
        if k.needs.is_none_or(|w| w.shelter == 0)
            || !(self.tick + u64::from(c.id.0)).is_multiple_of(SHELTER_SAMPLE)
        {
            return;
        }
        let (w, h) = extent(k, c.turn);
        c.needs.observed = if self.in_cover(w, h, c.x, c.y, c.band()) {
            c.needs.observed.saturating_sub(SETTLE_STEP)
        } else {
            (c.needs.observed + EXPOSE_STEP).min(PERCENT)
        };
    }

    /// The target of a fish committed to cover: where it is heading, or
    /// holding still once it is there.
    pub(super) fn cover_target(k: &Kind, c: &Creature) -> (i32, i32, i32) {
        let speed = if dist(c.tx - c.x, c.ty - c.y) <= REACHED {
            0
        } else {
            k.cruise
        };
        (c.tx, c.ty, speed)
    }

    pub(super) fn choose_need_action(
        &self,
        k: &Kind,
        c: &mut Creature,
        food: Option<(i32, i32, i32)>,
        rng: &mut Pcg32,
    ) -> Action {
        let Some(weights) = k.needs else {
            return Action::Idle;
        };
        let mates = &self.creatures.school[usize::from(c.species.0)];
        let school_available = weights.social > 0
            && k.schooling
            && c.stray == 0
            && (mates.len() > 1 || mates.first().is_some_and(|m| m.id != c.id));
        let feasible = match c.needs.action {
            // An idle fish that has had enough of open water looks for cover
            // on its shelter-sample cadence rather than carrying on.
            Action::Idle => {
                food.is_none()
                    && !school_available
                    && !(self.tired_of_open_water(k, c)
                        && (self.tick + u64::from(c.id.0)).is_multiple_of(SHELTER_SAMPLE))
            }
            Action::Wander | Action::Cover => true,
            Action::Forage => food.is_some(),
            Action::School => school_available,
        };
        if feasible && c.needs.remaining > 1 {
            c.needs.remaining -= 1;
            return c.needs.action;
        }
        let (observed, nearest) = if weights.social > 0 {
            self.social_observation(k, c)
        } else {
            (c.needs.observed, None)
        };
        let q = self.quirks(c);
        let hunger = q.scale(salt::HUNGER_WEIGHT, 85, 115, u64::from(weights.hunger));
        let sociability = q.scale(salt::SOCIAL_WEIGHT, 85, 115, u64::from(weights.social));
        let forage = food
            .map(|(x, y, _)| benefit(appetite(c) * hunger / 100, super::dist(c.x - x, c.y - y)));
        let school = nearest.filter(|_| school_available).map(|distance| {
            // Even a satisfied fish can maintain contact with its school.
            benefit(
                COMPANY + u64::from(PERCENT - observed) * sociability / 100,
                distance,
            )
        });
        let cover = self.cover_option(k, c);
        let candidates = [
            (Action::Forage, forage),
            (Action::School, school),
            (Action::Cover, cover.map(|(score, ..)| score)),
        ];
        let jitter = |rng: &mut Pcg32| u64::from(rng.next_u32() % 5);
        // Idle uses the wander steering too, but reconsiders as soon as
        // the next opportunity appears.
        let fallback = if food.is_none() && !school_available && cover.is_none() {
            Action::Idle
        } else {
            Action::Wander
        };
        let mut best = (4 + jitter(rng), fallback);
        for (action, score) in candidates {
            if let Some(score) = score {
                let varied = score + jitter(rng);
                if varied > best.0 {
                    best = (varied, action);
                }
            }
        }
        let remaining = MAX_COMMITMENT - u8::try_from(rng.next_u32() % 8).unwrap_or(0);
        if let (Action::Cover, Some((_, x, y, band))) = (best.1, cover) {
            // Cover keeps its own target: the wander target is not redrawn
            // while it holds (see `steering_target`).
            (c.tx, c.ty) = (x, y);
            if c.dodging == 0 {
                c.tz = z_of(band);
            }
            c.retarget = u32::from(remaining);
        } else if c.needs.action == Action::Cover {
            c.retarget = 0;
        }
        c.needs = State {
            action: best.1,
            remaining,
            observed,
        };
        best.1
    }

    /// How much exposure a fish puts up with in the open before it wants
    /// cover: a bold fish's is larger.
    fn exposure_tolerance(&self, c: &Creature) -> u64 {
        self.quirks(c)
            .scale(salt::BOLD, 70, 130, EXPOSURE_TOLERANCE)
    }

    /// Whether a fish that wants cover (see [`World::cover_option`]) is
    /// out in the open past its tolerance.
    fn tired_of_open_water(&self, k: &Kind, c: &Creature) -> bool {
        self.plant_count() > 0
            && k.needs.is_some_and(|w| w.shelter > 0)
            && u64::from(c.needs.observed) > self.exposure_tolerance(c)
    }

    /// What taking cover is worth to a fish that wants it: the score, and
    /// where (its own spot if it is hidden already, else the nearest cover
    /// it senses, in the band behind it). `None` if it wants none, or none
    /// is near.
    ///
    /// A fish in the open ignores exposure up to its own tolerance (a bold
    /// fish's is larger), so it wanders out from the plants; once it has
    /// had enough, or while it is hiding and not yet settled, the whole of
    /// its exposure counts. A hungry fish with food in reach weighs that
    /// against it (see the weights in `choose_need_action`).
    #[allow(clippy::many_single_char_names)] // cell maths
    fn cover_option(&self, k: &Kind, c: &Creature) -> Option<(u64, i32, i32, u8)> {
        let weights = k.needs.filter(|w| w.shelter > 0)?;
        let exposure = u64::from(c.needs.observed);
        if exposure == 0 || self.plant_count() == 0 {
            return None;
        }
        let q = self.quirks(c);
        let weight = q.scale(salt::SHELTER_WEIGHT, 85, 115, u64::from(weights.shelter));
        let (w, h) = extent(k, c.turn);
        if self.in_cover(w, h, c.x, c.y, c.band()) {
            return Some((exposure * weight / 100, c.x, c.y, c.band()));
        }
        let wanted = exposure.saturating_sub(self.exposure_tolerance(c));
        if wanted == 0 {
            return None;
        }
        // Looking round is a sense like the others: the idle fish that
        // reconsiders every tick looks every half second, not every tick.
        let due = c.needs.action != Action::Idle
            || (self.tick + u64::from(c.id.0)).is_multiple_of(SHELTER_SAMPLE);
        if !due {
            return None;
        }
        let (x, y, band) = self.find_cover_within(k, c, SHELTER_SENSE)?;
        Some((
            benefit(wanted * weight / 100, dist(c.x - x, c.y - y)),
            x,
            y,
            band,
        ))
    }

    fn social_observation(&self, k: &Kind, c: &Creature) -> (u8, Option<i64>) {
        let mates = &self.creatures.school[usize::from(c.species.0)];
        let adult_length = self
            .kind(c.species)
            .map_or(k.w, |adult| adult.size_of(c.female).0);
        let reach_squared = super::school::align_reach(adult_length * ONE).pow(2);
        let mut nearby = 0_u8;
        let mut nearest = i64::MAX;
        for m in mates.iter().filter(|m| m.id != c.id) {
            let dx = i64::from(c.x) - i64::from(m.x);
            let dy = i64::from(c.y) - i64::from(m.y);
            let squared = dx * dx + dy * dy;
            nearby = (nearby + u8::from(squared <= reach_squared)).min(2);
            nearest = nearest.min(squared);
        }
        (
            nearby * (PERCENT / 2),
            (nearest != i64::MAX).then(|| nearest.isqrt()),
        )
    }

    pub(in crate::world) fn save_needs(&self) -> Vec<[u8; 3]> {
        self.creatures
            .list
            .iter()
            .map(|c| c.needs.encode())
            .collect()
    }

    pub(in crate::world) fn load_needs(&mut self, saved: &[[u8; 3]]) -> Result<(), SaveError> {
        if saved.len() != self.creatures.list.len() {
            return Err(SaveError::Invalid(
                "wrong number of creature need states".into(),
            ));
        }
        for (c, &encoded) in self.creatures.list.iter_mut().zip(saved) {
            let weights = self.creatures.kinds[usize::from(c.species.0)].and_then(|k| k.needs);
            let state = State::decode(encoded)
                .filter(|&state| allowed(weights, state))
                .ok_or_else(|| SaveError::Invalid("invalid creature need state".into()))?;
            c.needs = state;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::BITE;
    use super::NeedLevel::{Comfortable, Critical, Low, Middling};
    use super::*;
    use crate::{CreatureId, MaterialTable, WorldConfig, save};

    fn tank(fish: &[(i64, i64)]) -> (World, Vec<CreatureId>) {
        let mut world =
            World::new(WorldConfig::default(), MaterialTable::builtin().unwrap()).unwrap();
        let tetra = world.species().id("neon_tetra").unwrap();
        let ids = fish
            .iter()
            .map(|&(x, y)| world.add_creature(tetra, x, y).unwrap())
            .collect();
        (world, ids)
    }

    fn readings(world: &World, id: CreatureId) -> Vec<u8> {
        world
            .creature_card(id)
            .unwrap()
            .needs
            .iter()
            .map(|r| r.satisfaction)
            .collect()
    }

    #[test]
    fn readings_use_feeding_state_and_nearby_mates() {
        let (mut world, ids) = tank(&[(40, 160), (45, 160), (50, 160), (300, 160)]);
        world.creature_mut(ids[0]).unwrap().satiety = FULL / 2;
        world.tick();
        assert_eq!(readings(&world, ids[0]), [49, 100]);
        assert_eq!(readings(&world, ids[3]), [74, 0]);
        let guppy = world.species().id("guppy").unwrap();
        let guppy = world.add_creature(guppy, 40, 170).unwrap();
        assert_eq!(readings(&world, guppy), [] as [u8; 0]);
    }

    #[test]
    fn readings_are_labelled_and_banded_for_display() {
        assert_eq!(Need::Hunger.label(), "Hunger");
        assert_eq!(Need::Social.label(), "Social");
        assert_eq!(Need::Shelter.label(), "Shelter");
        // 0 is unmet and 100 satisfied; the bands start where a fish gets
        // hungry (a bite fits at 75%).
        let bands: Vec<_> = [0, 25, 26, 49, 50, 74, 75, 100]
            .into_iter()
            .map(NeedLevel::of)
            .collect();
        assert_eq!(
            bands,
            [
                Critical,
                Critical,
                Low,
                Low,
                Middling,
                Middling,
                Comfortable,
                Comfortable
            ]
        );
        assert!(Critical < Low && Low < Middling && Middling < Comfortable);
        assert_eq!(
            NeedReading {
                need: Need::Hunger,
                satisfaction: 49
            }
            .describe(),
            "Hunger 49%, low"
        );
        // A first bite from empty is a quarter of a full stomach: still
        // critical.
        assert_eq!(u32::from(BITE) * 100 / u32::from(FULL), 25);
    }

    #[test]
    fn commitment_yields_to_danger_and_lost_opportunities() {
        let (mut world, ids) = tank(&[(40, 160), (45, 160)]);
        world.tick();
        let mut c = world.creature(ids[0]).unwrap().clone();
        let k = world.grown_kind(&c).unwrap();
        assert_eq!(c.needs.action, Action::School);
        let mut rng = crate::rng::chunk_stream(world.seed, world.tick, 99);
        let food = Some((c.x, c.y, k.top));
        assert_eq!(
            world.choose_need_action(&k, &mut c, food, &mut rng),
            Action::School
        );
        c.fear = 1;
        let flee = super::super::pursuit::flee_target(&k, &c).unwrap();
        assert_eq!(world.steering_target(&k, &mut c, None, &mut rng), flee);
        assert_eq!(c.needs.remaining, 0);

        c.fear = 0;
        c.satiety = 0;
        assert_eq!(
            world.choose_need_action(&k, &mut c, food, &mut rng),
            Action::Forage
        );
        assert_eq!(
            world.choose_need_action(&k, &mut c, None, &mut rng),
            Action::School
        );

        // On the next snapshot only the fish herself is left.
        assert!(world.remove_creature(ids[1]));
        world.tick();
        assert_eq!(
            world.choose_need_action(&k, &mut c, None, &mut rng),
            Action::Idle
        );
        assert_eq!(c.needs.observed, 0);
        assert_eq!(
            world.choose_need_action(&k, &mut c, food, &mut rng),
            Action::Forage
        );
    }

    #[test]
    fn travel_cost_changes_the_choice_and_seeds_vary_near_ties() {
        let (mut world, ids) = tank(&[(40, 160), (45, 160)]);
        world.tick();
        let mut c = world.creature(ids[0]).unwrap().clone();
        c.satiety = 0;
        c.needs.interrupt();
        let k = world.grown_kind(&c).unwrap();
        let choose = |distance, seed| {
            let mut c = c.clone();
            let food = Some((c.x + distance * ONE, c.y, k.top));
            let mut rng = crate::rng::chunk_stream(seed, world.tick, 99);
            world.choose_need_action(&k, &mut c, food, &mut rng)
        };
        assert_eq!(choose(0, 1), Action::Forage);
        assert_eq!(choose(160, 1), Action::School);
        assert!((1..160).any(|d| {
            let choice = choose(d, 1);
            (2..32).any(|seed| choose(d, seed) != choice)
        }));
    }

    #[test]
    fn a_flake_takes_the_edge_off_a_starving_fish_for_about_half_a_minute() {
        let (world, ids) = tank(&[(40, 160)]);
        let c = world.creature(ids[0]).unwrap();
        let at = |satiety| {
            appetite(&Creature {
                satiety,
                ..c.clone()
            })
        };
        assert_eq!((at(0), at(BITE), at(FULL)), (100, 25, 0));
        // Eating a flake takes three quarters off, then it comes back as the
        // bite wears off; the rest of the belly counts for less.
        assert!(at(BITE / 2) > at(BITE) && at(BITE / 2) < at(0));
        assert!(at(BITE) - at(BITE + BITE / 2) < at(BITE / 2) - at(BITE));
        assert!((0..=FULL).step_by(7).map(at).is_sorted_by(|a, b| a >= b));
    }

    #[test]
    fn a_fish_that_just_ate_stays_with_its_school_while_a_hungry_one_goes() {
        let (mut world, ids) = tank(&[(40, 160), (45, 160), (50, 160)]);
        world.tick();
        let c = world.creature(ids[0]).unwrap().clone();
        let k = world.grown_kind(&c).unwrap();
        let food = Some((c.x + 40 * ONE, c.y, k.top));
        let choose = |satiety, seed| {
            let mut c = c.clone();
            c.satiety = satiety;
            c.needs.interrupt();
            let mut rng = crate::rng::chunk_stream(seed, world.tick, 99);
            world.choose_need_action(&k, &mut c, food, &mut rng)
        };
        for seed in 1..64 {
            assert_eq!(choose(0, seed), Action::Forage, "hungry, seed {seed}");
            assert_eq!(choose(BITE, seed), Action::School, "fed, seed {seed}");
            // A little over twenty seconds later the bite has worn off and
            // food that is still there draws it again.
            assert_eq!(choose(BITE / 5, seed), Action::Forage, "later, seed {seed}");
        }
    }

    #[test]
    fn needs_replay_across_save_and_keep_the_social_observation_while_away() {
        let (mut world, ids) = tank(&[(40, 160), (45, 160)]);
        world.creature_mut(ids[0]).unwrap().satiety = FULL;
        world.tick();
        let state = world.creature(ids[0]).unwrap().needs;
        assert!(state.remaining > 0);
        assert_eq!(state.observed, 50);

        let mut loaded = save::decode(&save::encode(&world), world.materials().clone()).unwrap();
        assert_eq!(loaded.creature(ids[0]).unwrap().needs, state);
        assert_eq!(loaded.state_hash(), world.state_hash());
        for _ in 0..50 {
            world.tick();
            loaded.tick();
            assert_eq!(loaded.state_hash(), world.state_hash());
        }

        let before = readings(&loaded, ids[0]);
        let position = loaded.creature_body(loaded.creature(ids[0]).unwrap());
        loaded.catch_up(10);
        let after = readings(&loaded, ids[0]);
        assert!(after[0] < before[0]);
        assert_eq!(before[1], after[1]);
        let c = loaded.creature(ids[0]).unwrap();
        assert_eq!(loaded.creature_body(c), position);
        assert_eq!(c.needs.remaining, 0);
    }

    #[test]
    fn older_tanks_start_undecided_and_invalid_need_states_are_rejected() {
        let (mut world, ids) = tank(&[(40, 160), (45, 160)]);
        world.tick();
        let materials = world.materials().clone();
        let older = World::from_v19(world.to_v19(), materials.clone()).unwrap();
        assert_eq!(older.creature(ids[0]).unwrap().needs, State::default());
        assert_eq!(readings(&older, ids[0]), [74, 0]);

        // A tetra has no cover to take, so it can't save the action either.
        for invalid in [[5, 0, 0], [1, 0, 0], [0, 24, 0], [0, 0, 101], [4, 3, 0]] {
            let mut snapshot = world.to_v20();
            snapshot.needs[0] = invalid;
            assert!(matches!(
                World::from_v20(snapshot, materials.clone()),
                Err(SaveError::Invalid(_))
            ));
        }
        let mut snapshot = world.to_v20();
        snapshot.needs.clear();
        assert!(matches!(
            World::from_v20(snapshot, materials),
            Err(SaveError::Invalid(_))
        ));
    }

    /// A 120x64 tank with one honey gourami, over soil and with or without
    /// a stand of grown hygrophila.
    fn gourami_tank(planted: bool) -> (World, CreatureId) {
        let mut world = World::new(
            crate::WorldConfig {
                width: 120,
                height: 64,
                seed: 3,
                water_level: 0.95,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let soil = world.materials().id("aquasoil").unwrap();
        world
            .fill_rect(
                crate::Rect {
                    x0: 0,
                    y0: 58,
                    x1: 120,
                    y1: 64,
                },
                soil,
                crate::Brush::Replace,
            )
            .unwrap();
        if planted {
            let kind = world.plant_species().id("hygrophila").unwrap();
            let from = world.plant_species().get(kind).unwrap().default_propagule();
            for x in (30..=90).step_by(6) {
                world.paint_plant(x, 57, 0, kind, from).unwrap();
            }
            for _ in 0..6 * 60 * crate::TICKS_PER_SECOND {
                world.tick();
            }
        }
        let gourami = world.species().id("honey_gourami").unwrap();
        let id = world.add_creature(gourami, 10, 20).unwrap();
        // Let it take its first breath, so the next is a while off.
        for _ in 0..8 * crate::TICKS_PER_SECOND {
            world.tick();
        }
        world.creature_mut(id).unwrap().needs = State::default();
        (world, id)
    }

    fn state(world: &World, id: CreatureId) -> State {
        world.creature(id).unwrap().needs
    }

    fn in_cover(world: &World, id: CreatureId) -> bool {
        world.creature_in_cover(world.creature(id).unwrap())
    }

    /// Somewhere in the open that senses cover but is not in it.
    fn open_spot_near_cover(world: &mut World, id: CreatureId) -> (i64, i64) {
        for x in (10..110).step_by(2) {
            for y in (24..50).step_by(2) {
                if world.move_creature(id, x, y) {
                    let c = world.creature(id).unwrap().clone();
                    let k = world.grown_kind(&c).unwrap();
                    if !in_cover(world, id) && world.find_cover(&k, &c).is_some() {
                        return (x, y);
                    }
                }
            }
        }
        panic!("no open spot near cover");
    }

    #[test]
    fn each_species_shows_only_the_needs_it_has() {
        let (mut world, gourami) = gourami_tank(false);
        let tetra = world.species().id("neon_tetra").unwrap();
        let tetra = world.add_creature(tetra, 40, 20).unwrap();
        let labels = |world: &World, id| {
            world
                .creature_card(id)
                .unwrap()
                .needs
                .iter()
                .map(|r| r.need.label())
                .collect::<Vec<_>>()
        };
        // A solitary fish has no social need and a schooler has no shelter.
        assert_eq!(labels(&world, gourami), ["Hunger", "Shelter"]);
        assert_eq!(labels(&world, tetra), ["Hunger", "Social"]);
        let guppy = world.species().id("guppy").unwrap();
        let guppy = world.add_creature(guppy, 80, 20).unwrap();
        assert_eq!(labels(&world, guppy), [] as [&str; 0]);
    }

    #[test]
    fn new_creatures_start_with_comfortable_needs_and_room_for_a_bite() {
        let (mut world, ids) = tank(&[(40, 160)]);
        for (name, x) in [("honey_gourami", 75), ("guppy", 110)] {
            let species = world.species().id(name).unwrap();
            world.add_creature(species, x, 160).unwrap();
        }
        let expected = [vec![75, 75], vec![75, 100], vec![]];
        for (c, values) in world.creatures().iter().zip(expected) {
            assert_eq!(readings(&world, c.id()), values);
            assert!(
                world
                    .creature_needs(c)
                    .iter()
                    .all(|r| r.level() == Comfortable)
            );
            assert!(c.hungry());
            assert_eq!(c.satiety, FULL - BITE);
        }
        let loaded = save::decode(&save::encode(&world), world.materials().clone()).unwrap();
        assert_eq!(loaded.state_hash(), world.state_hash());
        assert_eq!(readings(&loaded, ids[0]), [75, 75]);

        let worm = world.materials().id("bloodworm").unwrap();
        world.set_cell(40, 160, worm).unwrap();
        world.tick();
        assert_eq!(world.creature(ids[0]).unwrap().eaten(), 1);
    }

    #[test]
    fn species_data_chooses_which_needs_apply() {
        let materials = MaterialTable::builtin().unwrap();
        let gourami = include_str!("../../../../../assets/species/honey_gourami.toml");
        let load = |source: &str| crate::SpeciesTable::from_toml_sources(&[source], &materials);
        assert!(load(gourami).is_ok());
        // Company is kept with a school, and one observed need at a time.
        assert!(load(&gourami.replace("shelter = 120", "social = 50")).is_err());
        let both = include_str!("../../../../../assets/species/neon_tetra.toml");
        assert!(load(&both.replace("social = 100", "social = 100\nshelter = 50")).is_err());
        assert!(load(&both.replace("hunger = 180", "hunger = 0")).is_err());
    }

    #[test]
    fn exposure_builds_in_open_water_and_settles_in_cover() {
        let (mut world, id) = gourami_tank(false);
        // A fresh fish is sheltered; a sample every half second adds a point
        // of exposure while there is no cover to be in.
        assert_eq!(readings(&world, id)[1], 100);
        for _ in 0..10 * 20 {
            world.tick();
        }
        let open = readings(&world, id)[1];
        assert!((78..=82).contains(&open), "{open}");

        let (mut world, id) = gourami_tank(true);
        world.creature_mut(id).unwrap().needs.observed = 60;
        open_spot_near_cover(&mut world, id);
        // Out in the open it keeps building; in cover it settles five times
        // as fast as it built.
        let before = state(&world, id).observed;
        for _ in 0..2 * 20 {
            world.tick();
        }
        assert!(state(&world, id).observed > before);
        let spot = (0..110)
            .step_by(2)
            .flat_map(|x| (24..50).step_by(2).map(move |y| (x, y)))
            .find(|&(x, y)| world.move_creature(id, x, y) && in_cover(&world, id))
            .expect("a spot in cover");
        assert!(in_cover(&world, id), "{spot:?}");
        let before = state(&world, id).observed;
        for _ in 0..4 * 20 {
            world.tick();
        }
        assert!(state(&world, id).observed + 12 <= before, "settles");
    }

    #[test]
    fn an_exposed_fish_goes_to_cover_and_holds_there_until_it_has_settled() {
        let (mut world, id) = gourami_tank(true);
        world.set_easy_mode(true);
        open_spot_near_cover(&mut world, id);
        {
            let c = world.creature_mut(id).unwrap();
            c.needs.observed = 90;
            c.satiety = FULL;
        }
        let mut chose_cover = false;
        let mut arrived = None;
        for tick in 0..60 * 20 {
            world.tick();
            chose_cover |= state(&world, id).action == Action::Cover;
            if arrived.is_none() && in_cover(&world, id) {
                arrived = Some(tick);
            }
        }
        assert!(chose_cover, "it went for cover");
        assert!(arrived.is_some_and(|t| t < 30 * 20), "and got there");
        // Settled, the need is met and the fish wanders again: it is not
        // kept in the plants for good, and exposure builds once more.
        let again = (0..120 * 20).any(|_| {
            world.tick();
            !in_cover(&world, id) && state(&world, id).observed > 0
        });
        assert!(again, "it leaves cover when settled");
    }

    #[test]
    fn without_cover_exposure_builds_and_the_fish_keeps_wandering() {
        let (mut world, id) = gourami_tank(false);
        world.set_easy_mode(true);
        world.creature_mut(id).unwrap().needs.observed = 90;
        let mut moved = 0.0_f32;
        let mut last = world.creature(id).unwrap().centre();
        for _ in 0..30 * 20 {
            world.tick();
            assert_ne!(state(&world, id).action, Action::Cover);
            let now = world.creature(id).unwrap().centre();
            moved += (now.0 - last.0).abs() + (now.1 - last.1).abs();
            last = now;
        }
        assert!(moved > 20.0, "it wanders on: {moved}");
        assert_eq!(readings(&world, id)[1], 0, "critically unsheltered");
    }

    #[test]
    fn a_hungry_fish_weighs_food_against_cover_and_a_fed_one_hides() {
        let (mut world, id) = gourami_tank(true);
        open_spot_near_cover(&mut world, id);
        world.tick();
        let c = world.creature(id).unwrap().clone();
        let k = world.grown_kind(&c).unwrap();
        let food = Some((c.x + 8 * ONE, c.y, k.top));
        let choose = |satiety, exposure, food, seed| {
            let mut c = c.clone();
            c.satiety = satiety;
            c.needs.interrupt();
            // Not idle, so that it looks for cover at once.
            c.needs.action = Action::Wander;
            c.needs.observed = exposure;
            let mut rng = crate::rng::chunk_stream(seed, world.tick, 99);
            world.choose_need_action(&k, &mut c, food, &mut rng)
        };
        for seed in 1..64 {
            assert_eq!(choose(0, 95, food, seed), Action::Forage, "starving {seed}");
            assert_eq!(choose(FULL, 95, food, seed), Action::Cover, "fed {seed}");
            // With no meal in reach, a hungry one still hides.
            assert_eq!(choose(0, 95, None, seed), Action::Cover, "no food {seed}");
            // Within its tolerance, open water is no reason to hide.
            assert_ne!(choose(FULL, 10, food, seed), Action::Cover, "calm {seed}");
        }
    }

    #[test]
    fn urgent_responses_interrupt_the_cover_commitment_at_once() {
        let (mut world, id) = gourami_tank(true);
        open_spot_near_cover(&mut world, id);
        world.creature_mut(id).unwrap().needs.observed = 95;
        let mut c = world.creature(id).unwrap().clone();
        c.satiety = FULL;
        let k = world.grown_kind(&c).unwrap();
        let mut rng = crate::rng::chunk_stream(7, world.tick, 99);
        c.needs.interrupt();
        c.needs.action = Action::Wander;
        world.steering_target(&k, &mut c, None, &mut rng);
        assert_eq!(c.needs.action, Action::Cover);
        let spot = (c.tx, c.ty);

        // A fright runs it off.
        let mut scared = c.clone();
        scared.fear = 1;
        let flee = super::super::pursuit::flee_target(&k, &scared).unwrap();
        assert_eq!(world.steering_target(&k, &mut scared, None, &mut rng), flee);
        assert_eq!(scared.needs.action, Action::Idle);

        // An overdue breath takes it up for air, as it would any gourami.
        let mut gasping = c.clone();
        gasping.next_breath = world.tick;
        let (_, y, _) = world.steering_target(&k, &mut gasping, None, &mut rng);
        assert!(y < gasping.y, "swims up for air");
        assert_eq!((gasping.tx, gasping.ty), spot, "then returns to its cover");
        assert_eq!(gasping.needs.action, Action::Cover);

        // The cursor too: it shies from it and forgets the commitment.
        world.set_creature_cursor(Some((i64::from((c.x - ONE) >> 8), i64::from(c.y >> 8))));
        let mut shy = c.clone();
        let target = world.steering_target(&k, &mut shy, None, &mut rng);
        assert_eq!(shy.needs.action, Action::Idle);
        assert!(target.0 > c.x || target.1 != c.y);
    }

    #[test]
    fn a_solitary_fish_never_schools_and_still_rests_between_choices() {
        let (mut world, a) = gourami_tank(false);
        let gourami = world.species().id("honey_gourami").unwrap();
        let b = world.add_creature(gourami, 14, 20).unwrap();
        for _ in 0..60 * 20 {
            world.tick();
            for id in [a, b] {
                assert_ne!(state(&world, id).action, Action::School);
            }
        }
        // Idle while nothing calls it: it reconsiders as soon as food
        // appears, as a tetra does.
        let c = world.creature(a).unwrap().clone();
        let k = world.grown_kind(&c).unwrap();
        let food = Some((c.x + ONE, c.y, k.top));
        let mut c = c;
        c.satiety = 0;
        c.needs.interrupt();
        let mut rng = crate::rng::chunk_stream(1, world.tick, 99);
        assert_eq!(
            world.choose_need_action(&k, &mut c, food, &mut rng),
            Action::Forage
        );
    }

    #[test]
    fn cover_decisions_replay_across_a_save_and_the_same_seed() {
        let run = || {
            let (mut world, id) = gourami_tank(true);
            open_spot_near_cover(&mut world, id);
            world.creature_mut(id).unwrap().needs.observed = 90;
            for _ in 0..3 * 20 {
                world.tick();
            }
            (world, id)
        };
        let ((mut a, id), (b, _)) = (run(), run());
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(state(&a, id).action, Action::Cover);
        let mut loaded = save::decode(&save::encode(&a), a.materials().clone()).unwrap();
        assert_eq!(state(&loaded, id), state(&a, id));
        assert_eq!(loaded.state_hash(), a.state_hash());
        for _ in 0..30 * 20 {
            a.tick();
            loaded.tick();
            assert_eq!(loaded.state_hash(), a.state_hash());
        }

        // A saved state the species couldn't have made is refused.
        let materials = a.materials().clone();
        for invalid in [[2, 5, 0], [4, 0, 0], [0, 0, 101]] {
            let mut snapshot = a.to_v20();
            let at = a.creatures().iter().position(|c| c.id() == id).unwrap();
            snapshot.needs[at] = invalid;
            assert!(matches!(
                World::from_v20(snapshot, materials.clone()),
                Err(SaveError::Invalid(_))
            ));
        }
        // Older saves, before any gourami had needs, start it sheltered.
        let older = World::from_v19(a.to_v19(), materials).unwrap();
        assert_eq!(state(&older, id), State::default());
        assert_eq!(readings(&older, id), [readings(&a, id)[0], 100]);
    }
}
