//! Pure DeskTank simulation: cell grid, chunks, materials, reactions, fields,
//! creatures, ecosystem, time and save format.
//!
//! This crate has no GPU or OS dependencies and must stay deterministic for a
//! given seed and time step (SPEC §2.1, §4.1).
#![forbid(unsafe_code)]

pub mod daylight;
pub mod ecology;
pub mod field;
mod grid;
mod hash;
pub mod material;
mod placeable;
mod rng;
pub mod script;
pub mod snapshot;
pub mod species;
pub mod tools;
mod world;

pub use ecology::{Ecology, EcologyError};
pub use field::{Field, FieldKind};
pub use grid::{Cell, Rect};
pub use material::{
    GLUED_VARIANT_BIT, Material, MaterialError, MaterialId, MaterialTable, MovementClass,
};
pub use placeable::Placeable;
pub use script::{Player, Script, ScriptError};
pub use species::{Species, SpeciesError, SpeciesId, SpeciesTable, TEXELS_PER_CELL};
pub use world::{
    BodyId, BodyInfo, Brush, CHUNK_SIZE, Creature, CreatureId, Ecosystem, MAX_CREATURES, MAX_SIDE,
    RANDOM_TICK_BUDGET, RandomTickStats, SLEEP_TICKS, TICKS_PER_SECOND, UnmetNeed, World,
    WorldConfig, WorldError,
};
