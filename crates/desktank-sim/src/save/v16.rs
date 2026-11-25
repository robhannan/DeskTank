//! Format version 16 snapshot types: version 15 with the pane each
//! creature crawls (#371: off, the front glass or the back glass). Frozen
//! like [`super::v1`]: once version 16 has shipped these types never
//! change.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

use super::v15;

/// A whole tank: version 15's, with each creature's pane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Save {
    /// Everything version 15 saved.
    pub base: v15::Save,
    /// The pane per creature (0 off, 1 front, 2 back), parallel to the
    /// creature list (same length and id order).
    pub panes: Vec<u8>,
}
