//! Format version 8 snapshot types: version 7 with each plant's
//! colouration (T6d: how far it has turned its species' colour in strong
//! light with low nutrients). Frozen like [`super::v1`]: once version 8 has
//! shipped these types never change, and the parts earlier versions
//! already had keep their types.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

pub(crate) use super::v1::{Body, Chunk, Grid, Life, Named, Plants};
pub(crate) use super::v5::Morphs;
pub(crate) use super::v6::Creatures;
use super::v7;
pub(crate) use super::v7::Raft;

/// A whole tank: version 7's fields, then the plant colourations.
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

/// A plant's colouration.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlantColour {
    /// The plant's id.
    pub plant: u16,
    /// How far it has turned its species' colour, 0–1.
    pub colour: f32,
}

impl Save {
    /// Migrates a version 7 snapshot: no build that wrote one coloured its
    /// plants, so every plant is uncoloured.
    pub(crate) fn from_v7(s: v7::Save) -> Self {
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
            rafts: s.rafts,
            colours: Vec::new(),
        }
    }
}
