use super::breeding::BREED_CHECK_TICKS;
use super::{
    BITE, Creature, DeathCause, ENERGY_RESERVE, FULL, Kind, STRESS_LIMIT, TICKS_PER_SECOND, World,
};
use crate::CatchUp;
use crate::world::LIFE_ACCUM_SCALE;

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
            let world_ticks = self.tick - start;
            // Life ticks lived in the span at the time ratio, carrying the
            // fraction over (exactly the span at ratio 1).
            let rate = self.life_step();
            let scale = u64::from(LIFE_ACCUM_SCALE);
            let lived = world_ticks * u64::from(rate) + u64::from(self.life_accum);
            let ticks = u32::try_from(lived / scale).unwrap_or(u32::MAX);
            self.life_accum = u32::try_from(lived % scale).unwrap_or(0);
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
                    Self::update_water_stress(c, q, seconds, rate, false);
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
        let mut remaining = ticks;
        let sat_drop = u16::try_from(remaining).unwrap_or(u16::MAX);
        if c.satiety.saturating_sub(sat_drop) > FULL - BITE {
            c.satiety -= sat_drop;
            return 0;
        }
        let mut eaten = 0;
        for &i in food {
            if c.energy == ENERGY_RESERVE {
                break;
            }
            let wait = c.satiety.saturating_sub(FULL - BITE).max(1);
            if remaining < u32::from(wait) {
                break;
            }
            let i = i as usize;
            if self.edible_material_cell(k, i) {
                remaining -= u32::from(wait);
                c.satiety = c.satiety.saturating_sub(wait);
                self.eat_cell(c, i % self.width, i / self.width);
                eaten += 1;
            }
        }
        if self.has_film_to_graze(k)
            && let Some(reach) = self.feeding_reach(k, c)
        {
            loop {
                let wait = c.satiety.saturating_sub(FULL - BITE).max(1);
                if c.energy == ENERGY_RESERVE || remaining < u32::from(wait) {
                    break;
                }
                let satiety = c.satiety;
                c.satiety = c.satiety.saturating_sub(wait);
                if !self.graze_algae(k, reach, c) {
                    c.satiety = satiety;
                    break;
                }
                remaining -= u32::from(wait);
                eaten += 1;
            }
        }
        c.satiety = c
            .satiety
            .saturating_sub(u16::try_from(remaining).unwrap_or(u16::MAX));
        eaten
    }
}
