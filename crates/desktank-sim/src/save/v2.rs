//! Format version 2 snapshot types (T9 part 1): version 1 with each
//! creature's life (age, growth, energy, stress), the corpses and the life
//! settings (easy mode, population cap). Frozen like [`super::v1`]: once
//! version 2 has shipped these types never change, and the parts version 1
//! already had keep its types.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

use super::v1;
pub(crate) use super::v1::{Body, Chunk, Grid, Life, Named, Plants};

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
    pub grid: Grid,
    pub chunks: Vec<Chunk>,
    pub next_body: u16,
    pub bodies: Vec<Body>,
    pub life: Life,
    pub plants: Plants,
    pub creatures: Creatures,
}

/// The creatures, corpses and life settings (T7, T9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Creatures {
    pub next_id: u32,
    /// The population cap.
    pub cap: u32,
    /// Easy mode: only old age kills.
    pub easy: bool,
    pub list: Vec<Creature>,
    pub corpses: Vec<Corpse>,
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
    /// Ticks lived.
    pub age: u32,
    /// Ticks of growth towards maturity.
    pub growth: u32,
    /// Stored energy, in ticks.
    pub energy: u32,
    /// Stress from poor water.
    pub stress: u32,
}

/// A dead creature, sinking and rotting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Corpse {
    pub species: u8,
    /// 0 old age, 1 starvation, 2 poor water.
    pub cause: u8,
    pub x: i32,
    pub y: i32,
    pub w: u8,
    pub h: u8,
    pub facing_left: bool,
    pub died: u64,
    pub released: u16,
    pub total: u16,
}

/// Version 1 had no life cycle: the population cap was fixed at 60, there
/// was no easy mode and nothing died.
const V1_CAP: u32 = 60;
/// Energy a version 1 creature starts version 2 with: a full reserve
/// (8 hours at 20 ticks a second), as for a creature just added.
const V1_ENERGY: u32 = 8 * 3600 * 20;

impl Save {
    /// Migrates a version 1 snapshot. Its creatures become adults just
    /// mature (`maturity` gives a species id's maturity age in ticks), well
    /// fed and unstressed, in a tank with the default cap and easy mode off.
    pub(crate) fn from_v1(s: v1::Save, maturity: impl Fn(u8) -> u32) -> Self {
        let creatures = Creatures {
            next_id: s.creatures.next_id,
            cap: V1_CAP,
            easy: false,
            list: s
                .creatures
                .list
                .into_iter()
                .map(|c| {
                    let grown = maturity(c.species);
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
                        age: grown,
                        growth: grown,
                        energy: V1_ENERGY,
                        stress: 0,
                    }
                })
                .collect(),
            corpses: Vec::new(),
        };
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
            grid: s.grid,
            chunks: s.chunks,
            next_body: s.next_body,
            bodies: s.bodies,
            life: s.life,
            plants: s.plants,
            creatures,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_1_energy_is_a_full_reserve() {
        assert_eq!(super::V1_ENERGY, crate::world::ENERGY_RESERVE);
        assert_eq!(super::V1_CAP as usize, crate::world::DEFAULT_CREATURE_CAP);
    }
}
