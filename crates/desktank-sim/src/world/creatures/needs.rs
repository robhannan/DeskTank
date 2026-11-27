//! Satisfaction describes the fish's state. Species weights and individual
//! quirks determine how much a deficit matters when choosing what to do.

use rand_core::Rng as _;
use rand_pcg::Pcg32;

use super::{Creature, FULL, Kind, ONE, SaveError, World, salt};

/// A need tracked by the simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// How full the creature is.
    Hunger,
    /// How much company the creature has nearby.
    Social,
}

/// The satisfaction of one need, independent of its influence on steering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NeedReading {
    /// Which need this describes.
    pub need: Need,
    /// From 0 (unsatisfied) to 100 (fully satisfied).
    pub satisfaction: u8,
}

const PERCENT: u8 = 100;
const MAX_COMMITMENT: u8 = 23;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Action {
    #[default]
    Idle = 0,
    Forage = 1,
    School = 2,
    Wander = 3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct State {
    pub action: Action,
    remaining: u8,
    social: u8,
}

impl State {
    /// Drop the old decision while keeping the last social observation.
    pub(super) fn interrupt(&mut self) {
        self.remaining = 0;
        self.action = Action::Idle;
    }

    pub(super) fn encode(self) -> [u8; 3] {
        [self.action as u8, self.remaining, self.social]
    }

    fn decode([action, remaining, social]: [u8; 3]) -> Option<Self> {
        let action = match action {
            0 => Action::Idle,
            1 => Action::Forage,
            2 => Action::School,
            3 => Action::Wander,
            _ => return None,
        };
        (social <= PERCENT
            && remaining <= MAX_COMMITMENT
            && (remaining != 0 || action == Action::Idle))
            .then_some(Self {
                action,
                remaining,
                social,
            })
    }
}

fn satiety_percent(c: &Creature) -> u8 {
    u8::try_from(u32::from(c.satiety.min(FULL)) * u32::from(PERCENT) / u32::from(FULL))
        .unwrap_or(PERCENT)
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
        if self.kind(c.species).is_none_or(|k| k.needs.is_none()) {
            return Vec::new();
        }
        vec![
            NeedReading {
                need: Need::Hunger,
                satisfaction: satiety_percent(c),
            },
            NeedReading {
                need: Need::Social,
                satisfaction: c.needs.social,
            },
        ]
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
        let school_available = k.schooling
            && c.stray == 0
            && (mates.len() > 1 || mates.first().is_some_and(|m| m.id != c.id));
        let feasible = match c.needs.action {
            Action::Idle => food.is_none() && !school_available,
            Action::Wander => true,
            Action::Forage => food.is_some(),
            Action::School => school_available,
        };
        if feasible && c.needs.remaining > 1 {
            c.needs.remaining -= 1;
            return c.needs.action;
        }
        let (social, nearest) = self.social_observation(k, c);
        let q = self.quirks(c);
        let hunger = q.scale(salt::HUNGER_WEIGHT, 85, 115, u64::from(weights.hunger));
        let sociability = q.scale(salt::SOCIAL_WEIGHT, 85, 115, u64::from(weights.social));
        let forage = food.map(|(x, y, _)| {
            let deficit = u64::from(PERCENT - satiety_percent(c));
            benefit(deficit * hunger / 100, super::dist(c.x - x, c.y - y))
        });
        let school = nearest.filter(|_| school_available).map(|distance| {
            // Even a satisfied fish can maintain contact with its school.
            benefit(
                16 + u64::from(PERCENT - social) * sociability / 100,
                distance,
            )
        });
        let candidates = [(Action::Forage, forage), (Action::School, school)];
        let jitter = |rng: &mut Pcg32| u64::from(rng.next_u32() % 5);
        // Idle uses the wander steering too, but reconsiders as soon as
        // the next opportunity appears.
        let fallback = if food.is_none() && !school_available {
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
        c.needs = State {
            action: best.1,
            remaining,
            social,
        };
        best.1
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
            let supported =
                self.creatures.kinds[usize::from(c.species.0)].is_some_and(|k| k.needs.is_some());
            let state = State::decode(encoded)
                .filter(|&state| supported || state == State::default())
                .ok_or_else(|| SaveError::Invalid("invalid creature need state".into()))?;
            c.needs = state;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(readings(&world, ids[3]), [0, 0]);
        let guppy = world.species().id("guppy").unwrap();
        let guppy = world.add_creature(guppy, 40, 170).unwrap();
        assert_eq!(readings(&world, guppy), [] as [u8; 0]);
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
        assert_eq!(c.needs.social, 0);
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
        c.satiety = FULL / 2;
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
    fn needs_replay_across_save_and_keep_the_social_observation_while_away() {
        let (mut world, ids) = tank(&[(40, 160), (45, 160)]);
        world.creature_mut(ids[0]).unwrap().satiety = FULL;
        world.tick();
        let state = world.creature(ids[0]).unwrap().needs;
        assert!(state.remaining > 0);
        assert_eq!(state.social, 50);

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
        assert_eq!(readings(&older, ids[0]), [0, 0]);

        for invalid in [[5, 0, 0], [1, 0, 0], [0, 24, 0], [0, 0, 101]] {
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
}
