//! Format version 17 snapshot types. Frozen like [`super::v1`], in wire
//! order. Version 16's world is followed by the four algae coat planes.

use serde::{Deserialize, Serialize};

use super::v16;

/// A whole tank with its surface and glass film.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Save {
    pub base: v16::Save,
    /// Cell-resolution runs of packed variety and density.
    pub coat: Vec<(u32, u8)>,
    /// Field-resolution front and back density planes, 0..=5 and 0..=15.
    pub front: Vec<(u32, u8)>,
    pub back: Vec<(u32, u8)>,
    /// Field-height pairs of left and right densities, 0..=15.
    pub sides: Vec<(u32, [u8; 2])>,
}
