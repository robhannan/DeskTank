//! Format version 3 snapshot types (T9 part 2): version 2 with each
//! creature's sex, colour-morph alleles and breeding state (the young it
//! carries, its rest), each corpse's morph and sex, the egg clutches, and
//! every species' morph names. Frozen like [`super::v1`] and
//! [`super::v2`]: once version 3 has shipped these types never change, and
//! the parts earlier versions already had keep their types.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

pub(crate) use super::v1::{Body, Chunk, Grid, Life, Named, Plants};
use super::v2;

/// A whole tank.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Save {
    pub width: u32,
    pub height: u32,
    pub seed: u64,
    pub tick: u64,
    pub last_change_tick: u64,
    pub surface_row: u32,
    /// The table entries the save was written with, to check on load.
    pub materials: Vec<Named>,
    pub plant_species: Vec<Named>,
    pub species: Vec<Named>,
    /// Every species' morph names in allele order, to check on load.
    pub morphs: Vec<Morphs>,
    pub grid: Grid,
    pub chunks: Vec<Chunk>,
    pub next_body: u16,
    pub bodies: Vec<Body>,
    pub life: Life,
    pub plants: Plants,
    pub creatures: Creatures,
}

/// One species' morph names, in allele order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Morphs {
    pub species: u8,
    pub names: Vec<String>,
}

/// The creatures, corpses, clutches and life settings (T7, T9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Creatures {
    pub next_id: u32,
    /// The population cap.
    pub cap: u32,
    /// Easy mode: only old age kills.
    pub easy: bool,
    pub list: Vec<Creature>,
    pub corpses: Vec<Corpse>,
    /// The next clutch id to try.
    pub next_clutch: u16,
    pub clutches: Vec<Clutch>,
}

/// One creature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Creature {
    pub id: u32,
    pub species: u8,
    pub x: i32,
    pub y: i32,
    pub vx: i32,
    pub vy: i32,
    pub tx: i32,
    pub ty: i32,
    pub retarget: u32,
    pub satiety: u16,
    pub food: Option<u32>,
    pub ignore: Option<u32>,
    pub facing_left: bool,
    pub turn: u8,
    pub held_until: u64,
    pub next_breath: u64,
    pub eaten: u32,
    pub age: u32,
    pub growth: u32,
    pub energy: u32,
    pub stress: u32,
    pub female: bool,
    /// Morph alleles, indices into the species' morphs.
    pub alleles: [u8; 2],
    /// Ticks left carrying young.
    pub carrying: u32,
    /// The father's alleles while carrying.
    pub sire: [u8; 2],
    /// Ticks until it may breed again.
    pub rest: u32,
}

/// A dead creature, sinking and rotting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Corpse {
    pub species: u8,
    /// 0 old age, 1 starvation, 2 poor water, 3 predation.
    pub cause: u8,
    pub x: i32,
    pub y: i32,
    pub w: u8,
    pub h: u8,
    pub facing_left: bool,
    pub died: u64,
    pub released: u16,
    pub total: u16,
    /// The morph it showed.
    pub morph: u8,
    pub female: bool,
}

/// A clutch of eggs waiting to hatch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Clutch {
    /// Its egg cells hold this in their state.
    pub id: u16,
    pub species: u8,
    pub mother: [u8; 2],
    pub sire: [u8; 2],
    /// Tick it hatches on.
    pub hatch: u64,
    /// Egg cell indices, ascending.
    pub eggs: Vec<u32>,
}

/// Ticks a version 2 creature waits before it breeds: a day of its life
/// (`SETTLE_TICKS`, 600 s at 20 ticks a second), as for one just added.
const V2_REST: u32 = 600 * 20;

/// What a species was like before version 3, for the migration.
pub(crate) struct Before {
    /// The morph the palette adds (the sprite's colours, which every
    /// creature was drawn in).
    pub palette_morph: u8,
    /// Morph names, in allele order.
    pub morphs: Vec<String>,
}

impl Save {
    /// Migrates a version 2 snapshot. Every creature shows the morph it
    /// was drawn in (the palette's, `species` gives it by species id), with
    /// two alleles of it; within each species they alternate male, female,
    /// … in id order, as the palette now adds them; none is carrying, and
    /// each settles for a day before it breeds. Corpses keep the palette's
    /// morph and are male. There are no clutches.
    pub(crate) fn from_v2(s: v2::Save, species: impl Fn(u8) -> Option<Before>) -> Self {
        let mut seen = [0_u32; 256];
        let palette = |id: u8| species(id).map_or(0, |b| b.palette_morph);
        let list = s
            .creatures
            .list
            .into_iter()
            .map(|c| {
                let n = &mut seen[usize::from(c.species)];
                let female = *n % 2 == 1;
                *n += 1;
                let morph = palette(c.species);
                Creature {
                    id: c.id,
                    species: c.species,
                    x: c.x,
                    y: c.y,
                    vx: c.vx,
                    vy: c.vy,
                    tx: c.tx,
                    ty: c.ty,
                    retarget: c.retarget,
                    satiety: c.satiety,
                    food: c.food,
                    ignore: c.ignore,
                    facing_left: c.facing_left,
                    turn: c.turn,
                    held_until: c.held_until,
                    next_breath: c.next_breath,
                    eaten: c.eaten,
                    age: c.age,
                    growth: c.growth,
                    energy: c.energy,
                    stress: c.stress,
                    female,
                    alleles: [morph; 2],
                    carrying: 0,
                    sire: [morph; 2],
                    rest: V2_REST,
                }
            })
            .collect();
        let corpses = s
            .creatures
            .corpses
            .into_iter()
            .map(|c| Corpse {
                species: c.species,
                cause: c.cause,
                x: c.x,
                y: c.y,
                w: c.w,
                h: c.h,
                facing_left: c.facing_left,
                died: c.died,
                released: c.released,
                total: c.total,
                morph: palette(c.species),
                female: false,
            })
            .collect();
        let morphs = s
            .species
            .iter()
            .filter_map(|n| {
                species(n.id).map(|b| Morphs {
                    species: n.id,
                    names: b.morphs,
                })
            })
            .collect();
        Self {
            width: s.width,
            height: s.height,
            seed: s.seed,
            tick: s.tick,
            last_change_tick: s.last_change_tick,
            surface_row: s.surface_row,
            materials: s.materials,
            plant_species: s.plant_species,
            species: s.species,
            morphs,
            grid: s.grid,
            chunks: s.chunks,
            next_body: s.next_body,
            bodies: s.bodies,
            life: s.life,
            plants: s.plants,
            creatures: Creatures {
                next_id: s.creatures.next_id,
                cap: s.creatures.cap,
                easy: s.creatures.easy,
                list,
                corpses,
                next_clutch: 1,
                clutches: Vec::new(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_2_creatures_settle_for_a_day() {
        assert_eq!(super::V2_REST, crate::world::SETTLE_TICKS);
    }
}
