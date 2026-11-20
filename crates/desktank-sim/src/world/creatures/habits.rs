//! Habits a few species have (the cheat-only crabs, SPEC §2.5), each
//! switched on by a species key and off for every other species:
//!
//! - **Waste** (`waste_every`, its bioload): every so many seconds it
//!   leaves a cell of waste in the water just behind its feet, which sinks
//!   and breaks down like any other waste (`assets/reactions.toml`).
//! - **Corpses** (`eats_corpses`): a hungry creature bites a corpse within
//!   reach, which then has a cell less left to rot.
//! - **A home** (`home_carpet`): once a second, until it has one, a bottom
//!   walker standing on a substrate its carpet species grows in may (one
//!   chance in [`CLAIM_ODDS`], from its seeded stream, so it wanders a few
//!   seconds first and its home differs from tank to tank) claim the cell
//!   under it as its home and pours [`PATCH`] plantlets of that carpet
//!   there, as the plants tool does ([`World::paint_plant`]). It does this
//!   once; from then on it wanders within [`HOME_RANGE`] cells of the home
//!   and forages within [`HOME_FORAGE`]. The home is creature state (saved
//!   and hashed).
//!
//! Each runs at a tick picked from the creature's id, so a group doesn't
//! act as one.

use super::{BITE, BITE_ENERGY, Creature, ENERGY_RESERVE, FULL, Kind, TICKS_PER_SECOND, World};
use crate::grid::Rect;
use rand_core::Rng as _;
use rand_pcg::Pcg32;

/// Plantlets a home owner pours at its home.
pub const PATCH: usize = 3;
/// Cells between the plantlets of a patch.
const PATCH_SPACING: i64 = 3;
/// How far from its home, in cells, a home owner wanders.
pub const HOME_RANGE: i64 = 12;
/// How far from its home, in cells, a home owner goes for food.
pub const HOME_FORAGE: i64 = 24;
/// A creature without a home looks for one this often, in ticks.
const CLAIM_EVERY: u64 = TICKS_PER_SECOND as u64;
/// One look in this many settles on the spot.
pub const CLAIM_ODDS: u32 = 4;

impl World {
    /// Runs `c`'s habits for this tick: a dropping when one is due, and a
    /// look for a home while it has none.
    pub(super) fn habits(&mut self, k: &Kind, c: &mut Creature, rng: &mut Pcg32) {
        let phase = self.tick + u64::from(c.id.0) * 7919;
        if k.waste_every > 0 && phase.is_multiple_of(k.waste_every) {
            self.drop_waste(k, c);
        }
        if k.homebody
            && c.home.is_none()
            && phase.is_multiple_of(CLAIM_EVERY)
            && rng.next_u32().is_multiple_of(CLAIM_ODDS)
        {
            self.claim_home(k, c);
        }
    }

    /// Whether a hungry creature of kind `k` has anything to eat in the
    /// tank: a food cell, or a corpse if it eats them.
    pub(super) fn anything_to_eat(&self, k: &Kind) -> bool {
        self.life.food_cells() > 0 || (k.eats_corpses && !self.creatures.corpses.is_empty())
    }

    /// Whether a saved `home` suits a creature of kind `k`: none, or a cell
    /// in the grid for a species that keeps one.
    pub(super) fn home_fits(&self, k: &Kind, home: Option<(u32, u32)>) -> bool {
        home.is_none_or(|(x, y)| {
            (k.homebody || k.territory > 0)
                && (x as usize) < self.width
                && (y as usize) < self.height
        })
    }

    /// Bites the first corpse (oldest first) overlapping `reach` with cells
    /// left to rot. Returns whether it ate.
    pub(super) fn bite_corpse(&mut self, reach: Rect, c: &mut Creature) -> bool {
        let found = self.creatures.corpses.iter().position(|corpse| {
            corpse.released < corpse.total && !self.corpse_body(corpse).intersect(reach).is_empty()
        });
        let Some(i) = found else {
            return false;
        };
        // The corpse goes once nothing is left of it (`update_corpses`).
        self.creatures.corpses[i].released += 1;
        c.satiety = c.satiety.saturating_add(BITE).min(FULL);
        c.energy = c.energy.saturating_add(BITE_ENERGY).min(ENERGY_RESERVE);
        c.eaten += 1;
        true
    }

    /// Leaves a cell of waste in the water beside the bottom row of `c`'s
    /// body, behind it if that cell is water, else ahead; nowhere if
    /// neither is.
    fn drop_waste(&mut self, k: &Kind, c: &Creature) {
        let Some(waste) = self.creatures.waste else {
            return;
        };
        let Some(b) = super::body(k, c.turn, c.x, c.y, self.width, self.height) else {
            return;
        };
        let y = b.y1 - 1;
        let left = b.x0.checked_sub(1);
        let right = (b.x1 < self.width).then_some(b.x1);
        let (behind, ahead) = if c.facing_left {
            (right, left)
        } else {
            (left, right)
        };
        let water = self.creatures.water;
        if let Some(x) = [behind, ahead]
            .into_iter()
            .flatten()
            .find(|&x| self.cells[y * self.width + x].material == water)
        {
            self.write_cell(x, y, waste, &mut None);
        }
    }

    /// Claims the substrate cell under `c`'s centre as its home if its
    /// carpet species grows there, and pours the patch's plantlets into the
    /// water just above the bed. A spot where none fit isn't claimed.
    fn claim_home(&mut self, k: &Kind, c: &mut Creature) {
        let Some(b) = super::body(k, c.turn, c.x, c.y, self.width, self.height) else {
            return;
        };
        if b.y1 >= self.height {
            return; // on the tank's floor glass
        }
        let Some(carpet) = self
            .species()
            .get(c.species)
            .and_then(|s| s.home_carpet.as_deref())
            .and_then(|name| self.plant_species().id(name))
        else {
            return;
        };
        let Some(species) = self.plant_species().get(carpet) else {
            return;
        };
        let propagule = species.default_propagule();
        let x = usize::try_from(c.x >> 8).unwrap_or(0).min(self.width - 1);
        let bed = self.cells[b.y1 * self.width + x].material;
        if !species.form.substrates().is_some_and(|s| s.contains(&bed)) {
            return;
        }
        let (cx, row) = (to_i64(x), to_i64(b.y1) - 1);
        let half = i64::try_from(PATCH / 2).unwrap_or(0);
        let mut planted = 0;
        for i in 0..i64::try_from(PATCH).unwrap_or(0) {
            let px = cx + (i - half) * PATCH_SPACING;
            if (0..to_i64(self.width)).contains(&px) {
                planted += self.paint_plant(px, row, 0, carpet, propagule).unwrap_or(0);
            }
        }
        if planted > 0 {
            #[allow(clippy::cast_possible_truncation)] // within MAX_SIDE
            {
                c.home = Some((x as u32, b.y1 as u32));
            }
        }
    }
}

/// The columns a home owner wanders between, `[lo, hi)`, if it has a home:
/// within [`HOME_RANGE`] of it, or a territorial fish within its
/// territory's radius.
pub(super) fn home_span(k: &Kind, c: &Creature) -> Option<(i64, i64)> {
    let range = if k.territory > 0 {
        i64::from(k.territory)
    } else {
        HOME_RANGE
    };
    c.home
        .map(|(x, _)| (i64::from(x) - range, i64::from(x) + range + 1))
}

/// Whether column `x` is close enough to `c`'s home to forage at (any
/// column for a creature without a carpet home).
pub(super) fn near_home(k: &Kind, c: &Creature, x: i64) -> bool {
    !k.homebody
        || c.home
            .is_none_or(|(hx, _)| (x - i64::from(hx)).abs() <= HOME_FORAGE)
}

fn to_i64(v: usize) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}
