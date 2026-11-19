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
pub mod plants;
mod rng;
pub mod save;
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
pub use plants::{
    Factor, Limit, Part, PlantSpecies, PlantSpeciesError, PlantSpeciesId, PlantSpeciesTable,
    Propagule,
};
pub use script::{Player, Script, ScriptError};
pub use species::{Species, SpeciesError, SpeciesId, SpeciesTable, TEXELS_PER_CELL};
pub use world::{
    BITE_ENERGY, BodyId, BodyInfo, Brush, CHUNK_SIZE, CORPSE_SINK, Corpse, Creature, CreatureId,
    DEFAULT_CREATURE_CAP, DeathCause, ENERGY_RESERVE, Ecosystem, LIFE_DAY_SECONDS, LIFE_DAY_TICKS,
    LifeStage, MAX_CORPSES, MAX_CREATURE_CAP, MAX_PLANTS, MAX_SIDE, MAX_UPDATES_PER_TICK,
    POOR_QUALITY, PlantId, PlantInfo, RANDOM_TICK_BUDGET, ROT_AFTER_SECONDS, ROT_EVERY_SECONDS,
    RandomTickStats, SLEEP_TICKS, STRESS_LIMIT, TICKS_PER_SECOND, UPDATE_SECONDS, UnmetNeed, World,
    WorldConfig, WorldError,
};
