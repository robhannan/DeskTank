//! Format version 5 snapshot types: version 4 with each creature's home (a
//! home owning species' patch of substrate, SPEC §2.5's cheat-only
//! creatures). Frozen like [`super::v1`]: once version 5 has shipped these
//! types never change, and the parts earlier versions already had keep
//! their types.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

pub(crate) use super::v1::{Body, Chunk, Grid, Life, Named, Plants};
use super::v4;
pub(crate) use super::v4::{Clutch, Corpse, Morphs};

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

/// One creature: version 4's, then its home.
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
    /// The substrate cell `(x, y)` it picked as its home, if it has one.
    pub home: Option<(u32, u32)>,
}

impl Save {
    /// Migrates a version 4 snapshot: no creature has a home yet.
    pub(crate) fn from_v4(s: v4::Save) -> Self {
        let list = s
            .creatures
            .list
            .into_iter()
            .map(|c| Creature {
                id: c.id,
                species: c.species,
                x: c.x,
                y: c.y,
                z: c.z,
                vx: c.vx,
                vy: c.vy,
                tx: c.tx,
                ty: c.ty,
                tz: c.tz,
                dodging: c.dodging,
                stray: c.stray,
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
                home: None,
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
                corpses: s.creatures.corpses,
                next_clutch: s.creatures.next_clutch,
                clutches: s.creatures.clutches,
            },
        }
    }
}
