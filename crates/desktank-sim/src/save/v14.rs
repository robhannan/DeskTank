//! Format version 14 snapshot types: version 13 with ball roll angles.
//! Field order is the wire order and freezes once this version ships.

use serde::{Deserialize, Serialize};

use super::v13;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Save {
    pub base: v13::Save,
    /// Roll angles, parallel to the version 9 raft table.
    pub raft_rolls: Vec<u16>,
}
