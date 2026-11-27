//! Pure DeskTank simulation: cell grid, chunks, materials, reactions, fields,
//! creatures, ecosystem, time and save format.
//!
//! This crate has no GPU or OS dependencies and must stay deterministic for a
//! given seed and time step (SPEC §2.1, §4.1).
#![forbid(unsafe_code)]

pub mod cheats;
pub mod daylight;
pub mod device;
pub mod ecology;
pub mod field;
mod grid;
mod hash;
pub mod material;
pub mod palette;
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

pub use daylight::{DaylightClock, DaylightSchedule, PolarDaylight};
pub use device::{
    CLOG_FLOW_FLOOR, Device, DeviceArt, DeviceError, DeviceId, DeviceKind, DeviceKinds,
    DeviceStatus, MAX_DEVICES, Mount, RinseKind,
};
pub use ecology::{AlgaeLook, AlgaeStyle, Ecology, EcologyError};
pub use field::{Field, FieldKind};
pub use grid::{Cell, Rect};
pub use material::{
    DRIFTWOOD_WET_DENSITY, GLUED_VARIANT_BIT, MAX_DRIFTWOOD_WETNESS, Material, MaterialError,
    MaterialId, MaterialTable, MovementClass, driftwood_variant, driftwood_wetness,
};
pub use palette::{Holds, PalettePath, PalettePathError};
pub use placeable::Placeable;
pub use plants::{
    Factor, Limit, Part, PlantSpecies, PlantSpeciesError, PlantSpeciesId, PlantSpeciesTable,
    Propagule,
};
pub use script::{Player, Script, ScriptError};
pub use species::{
    Breeding, BreedingMode, Glow, Look, NeedWeights, RestSpot, Species, SpeciesError, SpeciesId,
    SpeciesTable, TEXELS_PER_CELL,
};
pub use world::{
    ALGAE_DENSITY_MASK, BAND_REACH, BITE_ENERGY, BREED_CHECK_SECONDS, BodyId, BodyInfo, Brush,
    CHUNK_SIZE, COLOUR_PER_SECOND, CONCEIVE_ODDS, CORPSE_SINK, CatchUp, Clutch, Corpse, Creature,
    CreatureCard, CreatureId, DEFAULT_CREATURE_CAP, DEFAULT_TIME_RATIO, DEPTH_BANDS,
    DRIFTWOOD_AIR_DROP, DRIFTWOOD_DRY_DIV, DRIFTWOOD_SOAK_EVERY, DeathCause, ENERGY_RESERVE,
    Ecosystem, FULL_MASK, GlassPane, LIFE_DAY_SECONDS, LIFE_DAY_TICKS, LIVE_FOOD_BITES, LifeStage,
    MAX_CATCH_UP_SECONDS, MAX_CLUTCHES, MAX_CORPSES, MAX_CREATURE_CAP, MAX_LIVE_FOOD,
    MAX_NAME_CHARS, MAX_PLANTS, MAX_SIDE, MAX_TIME_RATIO, MAX_UPDATES_PER_TICK, MIN_TIME_RATIO,
    MUTATION_PER_MILLE, Need, NeedReading, PEARL_CO2, PEARL_HEALTH, PEARL_LIGHT,
    PEARL_MAX_PER_SECOND, PINCH, POOR_QUALITY, PREY_ENERGY, Pane, PlantId, PlantInfo,
    RANDOM_TICK_BUDGET, REVERSION_PER_MILLE, ROT_AFTER_SECONDS, ROT_EVERY_SECONDS, RandomTickStats,
    SETTLE_TICKS, SLEEP_TICKS, STRESS_LIMIT, STRIKE_ODDS, TICKS_PER_SECOND, TINT_STEPS, Traits,
    UPDATE_SECONDS, UnmetNeed, World, WorldConfig, WorldError, catch_up_seconds, clean_name,
    disc_mask, night, pursuit, quarter_bit, tint_step,
};
