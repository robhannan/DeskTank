//! Format version 9 snapshot types: version 8 with each creature's
//! hunting, territory and fright state (its quarry, pursuit counter and
//! fear). Frozen like [`super::v1`]: once version 9 has shipped these types
//! never change, and the parts earlier versions already had keep their
//! types.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

pub(crate) use super::v1::{Body, Chunk, Grid, Life, Named, Plants};
pub(crate) use super::v5::{Clutch, Corpse, Morphs};
use super::v6;
use super::v8;
pub(crate) use super::v8::{PlantColour, Raft};

/// A whole tank: version 8's, with version 9's creatures.
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
    /// Every floater plant's raft, by plant id.
    pub rafts: Vec<Raft>,
    /// The colouration of every plant that has any, by plant id.
    pub colours: Vec<PlantColour>,
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

/// One creature: version 6's, then its hunting and fright state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Creature {
    /// Everything version 6 saved.
    pub base: v6::Creature,
    /// The creature it is hunting or driving out of its territory.
    pub quarry: Option<u32>,
    /// Ticks it has chased its quarry, or without one ticks it rests.
    pub pursuit: u16,
    /// Ticks of fright left.
    pub fear: u16,
}

impl Save {
    /// Migrates a version 8 snapshot: no creature is hunting, driving,
    /// resting from a chase or frightened. Territorial fish claim their
    /// territory where they are when they next sense.
    pub(crate) fn from_v8(s: v8::Save) -> Self {
        let c = s.creatures;
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
                next_id: c.next_id,
                cap: c.cap,
                easy: c.easy,
                list: c
                    .list
                    .into_iter()
                    .map(|base| Creature {
                        base,
                        quarry: None,
                        pursuit: 0,
                        fear: 0,
                    })
                    .collect(),
                corpses: c.corpses,
                next_clutch: c.next_clutch,
                clutches: c.clutches,
            },
            rafts: s.rafts,
            colours: s.colours,
        }
    }
}
