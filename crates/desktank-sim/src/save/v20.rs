//! Format version 20 snapshot types, frozen in wire order like [`super::v1`].

use serde::{Deserialize, Serialize};

use super::v19;

/// A whole tank with each creature's saved need state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Save {
    pub base: v19::Save,
    /// Action code (0 idle, 1 forage, 2 school, 3 wander), remaining steering
    /// ticks of commitment (0 when undecided), and social satisfaction (0–100).
    pub needs: Vec<[u8; 3]>,
}
