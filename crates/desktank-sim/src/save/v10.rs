//! Format version 10 snapshot types: version 9 with the sub-cell outline
//! masks of rigid cells and of the rigid bodies' reference shapes
//! (`world::hardscape`). Frozen like [`super::v1`]: once version 10 has
//! shipped these types never change.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

use super::v9;

/// A whole tank: version 9's, with outline masks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Save {
    /// Everything version 9 saved.
    pub base: v9::Save,
    /// The partial outline masks of rigid cells, as (cell index, mask) in
    /// ascending index order; a rigid cell not listed is whole.
    pub masks: Vec<(u32, u16)>,
    /// The bodies whose reference shape has partial masks.
    pub shapes: Vec<ShapeMasks>,
}

/// A rigid body's reference shape masks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ShapeMasks {
    /// The body's id.
    pub body: u16,
    /// One mask per cell of its shape's bitmap, row-major.
    pub masks: Vec<u16>,
}

impl Save {
    /// Migrates a version 9 snapshot: every rigid cell and shape is whole.
    pub(crate) fn from_v9(base: v9::Save) -> Self {
        Self {
            base,
            masks: Vec::new(),
            shapes: Vec::new(),
        }
    }
}
