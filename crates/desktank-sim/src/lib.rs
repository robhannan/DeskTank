//! Pure DeskTank simulation: cell grid, chunks, materials, reactions, fields,
//! creatures, ecosystem, time and save format.
//!
//! This crate has no GPU or OS dependencies and must stay deterministic for a
//! given seed and time step (SPEC §2.1, §4.1).
#![forbid(unsafe_code)]

mod grid;
mod hash;
pub mod material;
mod rng;
pub mod script;
pub mod snapshot;
pub mod tools;
mod world;

pub use grid::{Cell, Rect};
pub use material::{
    GLUED_VARIANT_BIT, Material, MaterialError, MaterialId, MaterialTable, MovementClass,
};
pub use script::{Player, Script, ScriptError};
pub use world::{
    BodyId, BodyInfo, Brush, CHUNK_SIZE, MAX_SIDE, SLEEP_TICKS, TICKS_PER_SECOND, World,
    WorldConfig, WorldError,
};
