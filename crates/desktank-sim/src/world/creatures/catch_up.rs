use super::breeding::BREED_CHECK_TICKS;
use super::{
    BITE, Creature, DeathCause, ENERGY_RESERVE, FULL, Kind, STRESS_LIMIT, TICKS_PER_SECOND, World,
};
use crate::CatchUp;

impl World {
    /// Samples water quality once, then advances the animals to each minute
    /// boundary to preserve the breeding schedule. Positions stay fixed.
    pub(in crate::world) fn catch_up_creatures(&mut self, end: u64, report: &mut CatchUp) {
        if self.creatures.list.is_empty()
            && self.creatures.clutches.is_empty()
            && self.creatures.corpses.is_empty()
        {
            self.tick = end;
            return;
        }
        let quality =
            (!self.creatures.easy && !self.creatures.list.is_empty()).then(|| self.water_quality());
        let food: Vec<_> = self.life.food_lists().iter().flatten().copied().collect();
        let mut changed = false;
        while self.tick < end {
            let start = self.tick;
            self.tick = (start + (BREED_CHECK_TICKS - start % BREED_CHECK_TICKS)).min(end);
            let ticks = u32::try_from(self.tick - start).unwrap_or(0);
            let second = u64::from(TICKS_PER_SECOND);
            let seconds = u32::try_from(self.tick / second - start / second).unwrap_or(0);
            let mut list = core::mem::take(&mut self.creatures.list);
            let mut breeding = self.breed_tick(&list);
            let mut dead = Vec::new();
            let mut died_live = Vec::new();
            list.retain_mut(|c| {
                let Some(k) = self.kind(c.species).copied() else {
                    return true;
                };
                let lifespan = self.creature_lifespan(c);
                self.age_and_grow(c, &k, lifespan, ticks, &mut changed);
                if c.age < lifespan {
                    report.food_eaten += self.catch_up_feed(&k, c, ticks, &food);
                }
                let cause = self.age_or_hunger_death(c, lifespan).or_else(|| {
                    let q = quality?;
                    Self::update_water_stress(c, q, seconds, false);
                    (c.stress >= STRESS_LIMIT).then_some(DeathCause::PoorWater)
                });
                if let Some(cause) = cause {
                    report.record_death(cause);
                    changed = true;
                    if k.live {
                        died_live.push((self.creature_body(c), k.food_as));
                    } else {
                        dead.push(self.corpse_of(c, cause));
                    }
                    return false;
                }
                self.breed_over(c, &mut breeding, ticks, &mut changed);
                true
            });
            self.bury(dead);
            self.drop_live_food(&died_live);
            let survivors = list.len();
            self.creatures.list = list;
            changed |= self.finish_breeding_over(breeding, seconds > 0);
            report.births += self.creatures.list.len() - survivors;
        }
        changed |= self.catch_up_corpses();
        if changed {
            self.creatures.version += 1;
        }
    }

    fn catch_up_feed(&mut self, k: &Kind, c: &mut Creature, ticks: u32, food: &[u32]) -> usize {
        let mut remaining = u16::try_from(ticks).unwrap_or(u16::MAX);
        if c.satiety.saturating_sub(remaining) > FULL - BITE {
            c.satiety -= remaining;
            return 0;
        }
        let mut eaten = 0;
        for &i in food {
            if c.energy == ENERGY_RESERVE {
                break;
            }
            let wait = c.satiety.saturating_sub(FULL - BITE).max(1);
            if remaining < wait {
                break;
            }
            let i = i as usize;
            if self.edible_cell(k, i) {
                remaining -= wait;
                c.satiety = c.satiety.saturating_sub(wait);
                self.eat_cell(c, i % self.width, i / self.width);
                eaten += 1;
            }
        }
        c.satiety = c.satiety.saturating_sub(remaining);
        eaten
    }
}
