//! Format version 7 snapshot types: version 6 with each floater's raft
//! (T6c part 2, D22: a floater plant is a buoyant body; the plant table
//! keeps its plant state, the raft its motion). Frozen like [`super::v1`]:
//! once version 7 has shipped these types never change, and the parts
//! earlier versions already had keep their types.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

pub(crate) use super::v1::{Body, Chunk, Grid, Life, Named, Plants};
pub(crate) use super::v5::Morphs;
use super::v6;
pub(crate) use super::v6::Creatures;

/// A whole tank: version 6's fields, then the rafts.
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
}

/// A floater's motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Raft {
    /// The floater plant's id.
    pub plant: u16,
    /// Where it is relative to where its cells are drawn, 1/256 cell.
    pub off: (i64, i64),
    /// Velocity, 1/256 cell per tick.
    pub vel: (i64, i64),
    /// Ticks it has been at rest.
    pub still: u16,
    pub awake: bool,
}

impl Save {
    /// Migrates a version 6 snapshot: no build that wrote one had
    /// floaters, so there are no rafts (a floater plant without one would
    /// get one at rest on load).
    pub(crate) fn from_v6(s: v6::Save) -> Self {
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
            creatures: s.creatures,
            rafts: Vec::new(),
        }
    }
}
