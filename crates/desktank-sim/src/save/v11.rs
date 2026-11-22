//! Format version 11 snapshot types: version 10 with per-cell driftwood
//! waterlogging stored in the upper 7 bits of each driftwood cell's
//! `variant` byte (`variant >> 1`, 0 = dry, 127 = fully waterlogged; bit 0
//! stays [`crate::GLUED_VARIANT_BIT`]). Frozen like [`super::v1`]: once
//! version 11 has shipped these types never change.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

use super::v10;

/// A whole tank: version 10's wire layout, where driftwood cell variants
/// encode per-cell waterlogging rather than random colour noise.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Save {
    /// Everything version 10 saved.
    pub base: v10::Save,
}
