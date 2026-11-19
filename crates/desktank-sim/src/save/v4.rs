//! Format version 4 snapshot types (2.5D depth, part 1, D21): version 3
//! with each creature's depth, target depth and dodge and stray counters,
//! and each corpse's band. Frozen like [`super::v1`]: once version 4 has
//! shipped these types never change, and the parts earlier versions
//! already had keep their types.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

pub(crate) use super::v1::{Body, Chunk, Grid, Life, Named, Plants};
use super::v3;
pub(crate) use super::v3::{Clutch, Morphs};

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
    /// Depth, 1/256 band from the front.
    pub z: i32,
    pub vx: i32,
    pub vy: i32,
    pub tx: i32,
    pub ty: i32,
    /// Target depth, 1/256 band (a whole band).
    pub tz: i32,
    /// Ticks it has held still shifting band past a plant.
    pub dodging: u16,
    /// Ticks a schooler wanders on its own.
    pub stray: u16,
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
    /// 0 old age, 1 starvation, 2 poor water.
    pub cause: u8,
    pub x: i32,
    pub y: i32,
    pub w: u8,
    pub h: u8,
    pub facing_left: bool,
    /// The depth band it sinks in.
    pub band: u8,
    pub died: u64,
    pub released: u16,
    pub total: u16,
    /// The morph it showed.
    pub morph: u8,
    pub female: bool,
}

impl Save {
    /// Migrates a version 3 snapshot. Every creature settles in the band
    /// `band` gives it (from its id and species), as a creature just added
    /// would: a version 3 creature collided with plants in every band, so
    /// it fits in any. Corpses sink in the front band, where they fit too.
    pub(crate) fn from_v3(s: v3::Save, band: impl Fn(u32, u8) -> u8) -> Self {
        let list = s
            .creatures
            .list
            .into_iter()
            .map(|c| {
                let z = i32::from(band(c.id, c.species)) * 256;
                Creature {
                    id: c.id,
                    species: c.species,
                    x: c.x,
                    y: c.y,
                    z,
                    vx: c.vx,
                    vy: c.vy,
                    tx: c.tx,
                    ty: c.ty,
                    tz: z,
                    dodging: 0,
                    stray: 0,
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
                    female: c.female,
                    alleles: c.alleles,
                    carrying: c.carrying,
                    sire: c.sire,
                    rest: c.rest,
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
                band: 0,
                died: c.died,
                released: c.released,
                total: c.total,
                morph: c.morph,
                female: c.female,
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
            morphs: s.morphs,
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
                next_clutch: s.creatures.next_clutch,
                clutches: s.creatures.clutches,
            },
        }
    }
}
