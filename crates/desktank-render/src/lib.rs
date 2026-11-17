//! DeskTank renderer: grid texture and palette shader, sprite atlas and
//! lighting. Draws a [`desktank_sim`] snapshot to a surface or offscreen target.
//!
//! OS-agnostic; any `unsafe` needed for raw-handle surface creation belongs in
//! `desktank-platform` (SPEC §4.1).
#![forbid(unsafe_code)]

// Dependency edge from SPEC §4.1, kept ahead of first use.
use desktank_sim as _;
