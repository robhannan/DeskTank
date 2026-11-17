//! Pure DeskTank simulation: cell grid, chunks, materials, reactions, fields,
//! creatures, ecosystem, time and save format.
//!
//! This crate has no GPU or OS dependencies and must stay deterministic for a
//! given seed and time step (SPEC §2.1, §4.1).
#![forbid(unsafe_code)]
