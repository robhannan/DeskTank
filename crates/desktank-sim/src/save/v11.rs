//! Format version 11 snapshot types: version 10 with the device table
//! (D27, DV1). Frozen like [`super::v1`]: once version 11 has shipped these
//! types never change.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

use super::v10;

/// A whole tank: version 10's, with its devices.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Save {
    /// Everything version 10 saved.
    pub base: v10::Save,
    /// The devices, in id order. Their footprint cells are in the grid.
    pub devices: Vec<Device>,
    /// The next device id to try.
    pub next_device: u16,
}

/// A device in the table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Device {
    /// Its id, which its footprint cells hold in their state.
    pub id: u16,
    /// Its kind's name (`desktank_sim::DeviceKind::name`).
    pub kind: String,
    /// Top-left cell of its art's box.
    pub x: u32,
    /// Top-left row of its art's box.
    pub y: u32,
    /// Mirrored: on the right glass, jetting left.
    pub facing_left: bool,
}
