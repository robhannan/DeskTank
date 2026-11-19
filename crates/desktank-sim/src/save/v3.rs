//! Format version 3 snapshot types: version 2 with each creature's home (a
//! home owning species' patch of substrate, SPEC §2.5's cheat-only
//! creatures). Frozen like [`super::v2`]: once version 3 has shipped these
//! types never change, and the parts earlier versions already had keep
//! their types.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

use super::v2;
pub(crate) use super::v2::{Body, Chunk, Corpse, Grid, Life, Named, Plants};

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

/// One creature: version 2's, then its home.
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
    /// The substrate cell `(x, y)` it picked as its home, if it has one.
    pub home: Option<(u32, u32)>,
}

impl Save {
    /// Migrates a version 2 snapshot: no creature has a home yet.
    pub(crate) fn from_v2(s: v2::Save) -> Self {
        let creatures = Creatures {
            next_id: s.creatures.next_id,
            cap: s.creatures.cap,
            easy: s.creatures.easy,
            list: s
                .creatures
                .list
                .into_iter()
                .map(|c| Creature {
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
                    home: None,
                })
                .collect(),
            corpses: s.creatures.corpses,
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
