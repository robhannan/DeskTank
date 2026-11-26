//! Format version 18 snapshot types, frozen in wire order like [`super::v1`].

use serde::{Deserialize, Serialize};

use super::v17;

/// A whole tank with the pitch each swimmer last held.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Save {
    pub base: v17::Save,
    /// Eighth turns, in creature list order. Zero for level or on a face.
    pub pitches: Vec<i8>,
}
