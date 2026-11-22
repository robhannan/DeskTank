//! Format version 13 snapshot types: version 12 with each device's
//! filtration state (D27, DV3: media load, biological maturity, intake
//! guard). Frozen like [`super::v1`]: once version 13 has shipped these
//! types never change.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

use super::v12;

/// A whole tank: version 12's, with each device's filtration state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Save {
    /// Everything version 12 saved.
    pub base: v12::Save,
    /// Filtration state per device, parallel to `base.devices` (same length
    /// and id order).
    pub filtration: Vec<DeviceFiltration>,
}

/// A device's filtration state (DV3).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DeviceFiltration {
    /// Mechanical media load in `[0.0, 1.0]`.
    pub media_load: f32,
    /// Biological maturity in `[0.0, 1.0]`.
    pub maturity: f32,
    /// Whether the intake guard is fitted.
    pub intake_guard: bool,
}
