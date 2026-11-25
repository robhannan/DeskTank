//! Bounded ecology and lifecycle updates for time spent away from the tank.

use super::{DeathCause, TICKS_PER_SECOND, World};

/// Most elapsed world seconds applied by one catch-up call.
pub const MAX_CATCH_UP_SECONDS: u32 = 24 * 3600;

/// Bounds a host's elapsed wall-clock seconds and skips gaps under a minute.
/// Hosts pass zero when their clock went backwards.
#[must_use]
pub fn catch_up_seconds(elapsed: u64) -> u32 {
    if elapsed < 60 {
        return 0;
    }
    u32::try_from(elapsed)
        .unwrap_or(u32::MAX)
        .min(MAX_CATCH_UP_SECONDS)
}

pub(super) const STEP_TICKS: u32 = 5 * 60 * TICKS_PER_SECOND;

/// Creature lifecycle events during one catch-up call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CatchUp {
    /// World seconds applied, after clamping.
    pub seconds: u32,
    /// Fry born or hatched and added to the tank.
    pub births: usize,
    /// Creatures that reached their lifespan.
    pub old_age_deaths: usize,
    /// Creatures that ran out of energy.
    pub starvation_deaths: usize,
    /// Creatures that accumulated too much water-quality stress.
    pub poor_water_deaths: usize,
    /// Food cells and plant parts eaten.
    pub food_eaten: usize,
}

impl CatchUp {
    pub(super) fn record_death(&mut self, cause: DeathCause) {
        match cause {
            DeathCause::OldAge => self.old_age_deaths += 1,
            DeathCause::Starvation => self.starvation_deaths += 1,
            // Catch-up skips air-breathing checks, so it never suffocates;
            // count one as a stress death rather than panic.
            DeathCause::PoorWater | DeathCause::Suffocation => self.poor_water_deaths += 1,
        }
    }
}

impl World {
    /// Advances the tank by up to 24 hours using coarse, seeded updates.
    /// Cells and creatures stay in place. Feeding uses the tank's food and
    /// nearby algae, and skips live prey, corpse bites and sponge grazing.
    /// Device activity, air-breathing checks and current are skipped.
    pub fn catch_up(&mut self, seconds: u32) -> CatchUp {
        let seconds = seconds.min(MAX_CATCH_UP_SECONDS);
        let mut report = CatchUp {
            seconds,
            ..CatchUp::default()
        };
        if seconds == 0 {
            return report;
        }
        let end = self.tick + u64::from(seconds * TICKS_PER_SECOND);
        self.tidy_plants();
        self.catch_up_refresh();
        while self.tick < end {
            let start = self.tick;
            let next = (start + u64::from(STEP_TICKS)).min(end);
            self.catch_up_creatures(next, &mut report);
            self.plants.pearled_this_tick = false;
            self.catch_up_cells(start);
            self.catch_up_plants();
            self.catch_up_fields(start);
            self.catch_up_refresh();
            self.catch_up_algae(start);
        }
        self.catch_up_current();
        // The next normal tick also hands these edits to the renderer.
        for chunk in &mut self.chunks {
            chunk.changed_next = chunk.changed;
        }
        report
    }
}
