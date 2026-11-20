//! Pure DeskTank simulation: cell grid, chunks, materials, reactions, fields,
//! creatures, ecosystem, time and save format.
//!
//! This crate has no GPU or OS dependencies and must stay deterministic for a
//! given seed and time step (SPEC §2.1, §4.1).
#![forbid(unsafe_code)]

pub mod cheats;
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
pub mod scene;
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
pub use species::{
    Breeding, BreedingMode, Glow, Look, Species, SpeciesError, SpeciesId, SpeciesTable,
    TEXELS_PER_CELL,
};
pub use world::{
    BAND_REACH, BITE_ENERGY, BREED_CHECK_SECONDS, BodyId, BodyInfo, Brush, CHUNK_SIZE,
    COLOUR_PER_SECOND, CONCEIVE_ODDS, CORPSE_SINK, Clutch, Corpse, Creature, CreatureCard,
    CreatureId, DEFAULT_CREATURE_CAP, DEPTH_BANDS, DeathCause, ENERGY_RESERVE, Ecosystem,
    LIFE_DAY_SECONDS, LIFE_DAY_TICKS, LifeStage, MAX_CLUTCHES, MAX_CORPSES, MAX_CREATURE_CAP,
    MAX_NAME_CHARS, MAX_PLANTS, MAX_SIDE, MAX_UPDATES_PER_TICK, MUTATION_PER_MILLE, POOR_QUALITY,
    PREY_ENERGY, PlantId, PlantInfo, RANDOM_TICK_BUDGET, REVERSION_PER_MILLE, ROT_AFTER_SECONDS,
    ROT_EVERY_SECONDS, RandomTickStats, SETTLE_TICKS, SLEEP_TICKS, STRESS_LIMIT, STRIKE_ODDS,
    TICKS_PER_SECOND, TINT_STEPS, Traits, UPDATE_SECONDS, UnmetNeed, World, WorldConfig,
    WorldError, clean_name, tint_step,
};
