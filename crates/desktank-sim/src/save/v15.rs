//! Format version 15 snapshot types: version 14 with each creature's rest
//! for the night (SPEC §2.4, #195: asleep, and how long it stays up after
//! being woken). Frozen like [`super::v1`]: once version 15 has shipped
//! these types never change.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

use super::v14;

/// A whole tank: version 14's, with each creature's rest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Save {
    /// Everything version 14 saved.
    pub base: v14::Save,
    /// Rest per creature, parallel to the creature list (same length and
    /// id order).
    pub rest: Vec<CreatureRest>,
}

/// A creature's rest for the night.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct CreatureRest {
    /// Asleep.
    pub asleep: bool,
    /// Ticks it stays up after being woken at night.
    pub woken: u16,
}
