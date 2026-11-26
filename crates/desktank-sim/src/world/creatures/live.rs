//! Live food (SPEC §2.4 *Live food*): small living foods such as
//! bloodworms and brine shrimp, offered from Food › Live a pinch at a time.
//!
//! - **Species.** A species with `live_food = "<material>"` is live food.
//!   Bloodworms sink at [`SINK`] and wriggle over the bed. Brine shrimp
//!   swim in short bursts in midwater. Live food is not stock:
//!   the palette's Creatures group, the `spawn` cheat and
//!   `scene::stock` leave it out, it never breeds, and it doesn't count
//!   against the population cap. At most [`MAX_LIVE_FOOD`] live foods are
//!   in a tank at once, beside the cap.
//! - **Eaters.** A species eats a live food when its diet holds the
//!   live food's material (fish that take frozen bloodworms take live
//!   ones) and it is at least as long as the worm, as grown. A hungry
//!   eater hunts it through [`super::pursuit`] like prey, from further
//!   away ([`LIVE_SENSE`]), in easy mode too, and, once a second,
//!   swallows one it touches whole: no strike odds, worth
//!   [`LIVE_FOOD_BITES`] bites of food. Predators' `prey_cm` doesn't
//!   apply to live food, and live food is never prey to it.
//! - **Fleeing.** Live food squirms straight away from a creature chasing
//!   it, at its own (slow) top speed; it doesn't seek cover.
//! - **Death.** Live food dies as any creature does (old age, foul water)
//!   but leaves no corpse: it becomes one cell of its material in the
//!   water it lay in (a bloodworm becomes frozen bloodworm), which rots
//!   like any food. One whose body covers no water cell (wholly in
//!   another fluid) leaves nothing.
//! - **A pinch.** Adding a live food from the palette adds [`PINCH`] of
//!   them, scattered a few cells round the pointer, within the limit.

use rand_core::Rng as _;

use super::{
    BITE, BITE_ENERGY, Creature, CreatureId, ENERGY_RESERVE, FULL, Kind, TICKS_PER_SECOND, Traits,
    World, WorldError,
};
use crate::grid::Rect;
use crate::material::MaterialId;
use crate::rng::chunk_stream;
use crate::species::SpeciesId;

/// Most live foods in a tank at once, beside the population cap.
pub const MAX_LIVE_FOOD: usize = 32;
/// Live foods a palette click adds.
pub const PINCH: usize = 5;
/// Bites of food a live food is worth to the creature that eats it.
pub const LIVE_FOOD_BITES: u16 = 2;
/// How far a hungry eater notices live food, in cells (40 cm).
pub const LIVE_SENSE: i32 = 80;
/// A hunt for live food is lost past this distance, in cells.
pub const LIVE_LOSE: i32 = 96;
/// Cells round the pointer a pinch scatters over (each way).
const PINCH_SPREAD: u32 = 3;
/// How fast live food sinks, in 1/256 cells per tick (6 cells a second).
#[allow(clippy::cast_possible_wrap)]
pub(super) const SINK: i32 = 6 * super::ONE / super::TICKS_PER_SECOND as i32;
const PINCH_STREAM: u64 = (1 << 48) | (4 << 40);

/// Midwater live food alternates a short kick with slow drift. Stagger
/// the rhythm within a pinch and derive it from saved creature/world state.
pub(super) fn swim_pace(k: &Kind, id: CreatureId, tick: u64) -> i32 {
    const PERIOD: u64 = 2 * TICKS_PER_SECOND as u64;
    const KICK: u64 = PERIOD / 4;
    let phase = (tick % PERIOD + u64::from(id.0) * 13) % PERIOD;
    if phase < KICK { k.top } else { k.cruise }
}

impl World {
    /// Live foods in the tank now (at most [`MAX_LIVE_FOOD`]).
    #[must_use]
    pub fn live_food_count(&self) -> usize {
        self.creatures
            .list
            .iter()
            .filter(|c| self.is_live(c.species))
            .count()
    }

    /// Creatures that count against the population cap: all but live food.
    #[must_use]
    pub fn stock_count(&self) -> usize {
        self.creatures.list.len() - self.live_food_count()
    }

    pub(super) fn is_live(&self, species: SpeciesId) -> bool {
        self.kind(species).is_some_and(|k| k.live)
    }

    /// Adds a pinch of live food: [`PINCH`] of `species` scattered round
    /// cell `(x, y)` where each fits, up to [`MAX_LIVE_FOOD`]. Returns the
    /// first added.
    pub(super) fn add_pinch(
        &mut self,
        species: SpeciesId,
        x: i64,
        y: i64,
    ) -> Result<CreatureId, WorldError> {
        let mut rng = chunk_stream(
            self.seed,
            self.tick,
            PINCH_STREAM + u64::from(self.creatures.next_id),
        );
        let spread = |rng: &mut rand_pcg::Pcg32| {
            i64::from(rng.next_u32() % (2 * PINCH_SPREAD + 1)) - i64::from(PINCH_SPREAD)
        };
        let mut first = None;
        let mut error = WorldError::NoRoom;
        for _ in 0..PINCH {
            let (dx, dy) = (spread(&mut rng), spread(&mut rng));
            match self.add_creature_with_traits(species, x + dx, y + dy, Traits::default()) {
                Ok(id) => {
                    first.get_or_insert(id);
                }
                Err(e @ WorldError::PopulationCap(_)) => {
                    error = e;
                    break;
                }
                Err(e) => error = e,
            }
        }
        first.ok_or(error)
    }

    /// Writes a cell of each dead live food's material into the water its
    /// body covered (the lowest such cell), or nothing where there is none.
    pub(super) fn drop_live_food(&mut self, dead: &[(Rect, Option<MaterialId>)]) {
        let water = self.creatures.water;
        for &(b, food) in dead {
            let Some(food) = food else { continue };
            let spot = (b.y0..b.y1)
                .rev()
                .flat_map(|y| (b.x0..b.x1).map(move |x| (x, y)))
                .find(|&(x, y)| self.cells[y * self.width + x].material == water);
            if let Some((x, y)) = spot {
                self.write_cell(x, y, food, &mut None);
            }
        }
    }

    /// Once a second: each hungry creature touching a live food it eats
    /// swallows one whole (see the module docs). Returns whether any was
    /// eaten.
    pub(super) fn eat_live_food(&self, list: &mut Vec<Creature>) -> bool {
        if !list.iter().any(|c| self.is_live(c.species)) {
            return false;
        }
        let mut eaten: Vec<usize> = Vec::new();
        for i in 0..list.len() {
            let p = &list[i];
            let Some(k) = self.grown_kind(p) else {
                continue;
            };
            if !k.eats_live
                || eaten.contains(&i)
                || !p.hungry()
                || p.held_until >= self.tick
                || p.asleep
            {
                continue;
            }
            let length = self.length_mm(p);
            let b = self.creature_body(p);
            let reach = Rect {
                x0: b.x0.saturating_sub(k.reach),
                y0: b.y0.saturating_sub(k.reach),
                x1: b.x1 + k.reach,
                y1: b.y1 + k.reach,
            };
            let food = (0..list.len()).find(|&j| {
                let q = &list[j];
                j != i
                    && !eaten.contains(&j)
                    && Self::eats_live(&k, length, q.species, self.length_mm(q))
                    && q.held_until < self.tick
                    && q.band().abs_diff(p.band()) <= super::BAND_REACH
                    && !reach.intersect(self.creature_body(q)).is_empty()
            });
            let Some(j) = food else {
                continue;
            };
            eaten.push(j);
            let p = &mut list[i];
            p.satiety = p.satiety.saturating_add(BITE * LIVE_FOOD_BITES).min(FULL);
            p.energy = p
                .energy
                .saturating_add(BITE_ENERGY * u32::from(LIVE_FOOD_BITES))
                .min(ENERGY_RESERVE);
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

    /// Whether `eater` (a creature of kind `k`, `length` mm long as grown)
    /// eats the live food `species` `prey_mm` long.
    pub(super) fn eats_live(
        k: &super::Kind,
        length: u32,
        species: SpeciesId,
        prey_mm: u32,
    ) -> bool {
        k.live_prey[usize::from(species.0 >> 6)] & (1 << (species.0 & 63)) != 0 && prey_mm <= length
    }
}
