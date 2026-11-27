//! Format version 19 snapshot types, frozen in wire order like [`super::v1`].

use serde::{Deserialize, Serialize};

use super::v18;

/// A whole tank with its time ratio and fractional life-tick remainder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Save {
    pub base: v18::Save,
    /// Life-time multiplier ([`crate::MIN_TIME_RATIO`]..=[`crate::MAX_TIME_RATIO`]).
    pub time_ratio: f32,
    /// Thousandths of a life tick carried between world ticks (below 1000).
    pub life_accum: u32,
}
