//! Deterministic input scripts: timed pours replayed against a world, for
//! `desktank capture`, render tests and determinism tests (SPEC §7).
//!
//! A script is TOML. Every key is optional:
//!
//! ```toml
//! ticks = 200        # ticks to run before the first frame (default 0)
//! frame_every = 2    # ticks between frames when capturing several (default 1)
//!
//! [world]
//! width = 160        # grid size in cells (default 160×90)
//! height = 90
//! water_level = 0.96 # fraction of the height (default 0.96)
//! starter = true     # lay out the starter scene first (default false)
//!
//! [[pour]]
//! tick = 0           # first tick to pour on (default 0)
//! repeat = 40        # pour on this many consecutive ticks (default 1)
//! x = 80             # disc centre, in cells from the top left
//! y = 10
//! dx = 1             # move the centre this many cells per repeat (default 0)
//! radius = 3         # disc radius in cells (default 2)
//! material = "sand"  # a material name from assets/materials
//! brush = "pour"     # "pour" fills only fluid cells, "replace" overwrites (default "pour")
//! glued = true       # a rigid material's body is glued in place (default false)
//! ```
//!
//! Pours due on tick `t` are applied just before the world's `t`-th tick
//! (counting from 0), so they show from that tick on. Each `[[pour]]` entry
//! is one stroke: the rigid cells of all its repeats form one body, held
//! still until its last repeat (D17).

use serde::Deserialize;
use thiserror::Error;

use crate::{BodyId, Brush, World, WorldConfig, WorldError};

/// Error reading or applying a script.
#[derive(Debug, Error)]
pub enum ScriptError {
    /// `frame_every` is zero.
    #[error("frame_every must be at least 1")]
    ZeroFrameEvery,
    /// The TOML didn't parse or has unknown keys.
    #[error("bad script: {0}")]
    Parse(#[from] toml::de::Error),
    /// A pour names a material the table doesn't have.
    #[error("unknown material {0:?}")]
    UnknownMaterial(String),
    /// The world rejected an edit or its configuration.
    #[error(transparent)]
    World(#[from] WorldError),
}

/// A parsed input script.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Script {
    /// Ticks to run before the first frame.
    #[serde(default)]
    pub ticks: u64,
    /// Ticks between frames when capturing several.
    #[serde(default = "one")]
    pub frame_every: u64,
    /// The world to build.
    #[serde(default)]
    pub world: WorldSpec,
    /// Timed pours.
    #[serde(default, rename = "pour")]
    pub pours: Vec<Pour>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            ticks: 0,
            frame_every: 1,
            world: WorldSpec::default(),
            pours: Vec::new(),
        }
    }
}

/// The `[world]` table.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct WorldSpec {
    /// Width in cells.
    pub width: usize,
    /// Height in cells.
    pub height: usize,
    /// Water level as a fraction of the height.
    pub water_level: f32,
    /// Lay out the host's starter scene before the script runs.
    pub starter: bool,
}

impl Default for WorldSpec {
    fn default() -> Self {
        Self {
            width: 160,
            height: 90,
            water_level: WorldConfig::default().water_level,
            starter: false,
        }
    }
}

/// One `[[pour]]` entry.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Pour {
    /// First tick to pour on.
    #[serde(default)]
    pub tick: u64,
    /// Number of consecutive ticks to pour on.
    #[serde(default = "one")]
    pub repeat: u64,
    /// Disc centre column.
    pub x: i64,
    /// Disc centre row.
    pub y: i64,
    /// Column step per repeat.
    #[serde(default)]
    pub dx: i64,
    /// Disc radius in cells.
    #[serde(default = "two")]
    pub radius: u32,
    /// Material name.
    pub material: String,
    /// How the disc treats the cells it covers.
    #[serde(default)]
    pub brush: BrushName,
    /// Glue the body a rigid pour makes.
    #[serde(default)]
    pub glued: bool,
}

/// A brush as written in a script.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BrushName {
    /// [`Brush::Pour`].
    #[default]
    Pour,
    /// [`Brush::Replace`].
    Replace,
}

fn one() -> u64 {
    1
}

fn two() -> u32 {
    2
}

impl Script {
    /// Parses a script.
    ///
    /// # Errors
    ///
    /// Returns an error if the TOML is malformed, has unknown keys, or sets
    /// `frame_every` to zero.
    pub fn parse(source: &str) -> Result<Self, ScriptError> {
        let script: Self = toml::from_str(source)?;
        if script.frame_every == 0 {
            return Err(ScriptError::ZeroFrameEvery);
        }
        Ok(script)
    }

    /// The world configuration for `seed`.
    #[must_use]
    pub fn world_config(&self, seed: u64) -> WorldConfig {
        WorldConfig {
            width: self.world.width,
            height: self.world.height,
            seed,
            water_level: self.world.water_level,
        }
    }

    /// A player that replays this script against one world.
    #[must_use]
    pub fn player(&self) -> Player<'_> {
        Player {
            script: self,
            bodies: vec![None; self.pours.len()],
        }
    }

    /// Ticks the world from its current tick up to `until`, applying pours
    /// on the way (with a fresh [`Player`]).
    ///
    /// # Errors
    ///
    /// Returns an error if a pour names an unknown material.
    pub fn run(&self, world: &mut World, until: u64) -> Result<(), ScriptError> {
        self.player().run(world, until)
    }
}

/// Replays a [`Script`] against one world, remembering the body each rigid
/// pour is filling.
#[derive(Clone, Debug)]
pub struct Player<'a> {
    script: &'a Script,
    bodies: Vec<Option<BodyId>>,
}

impl Player<'_> {
    /// Applies the pours due on `tick` (call before the world's tick).
    ///
    /// # Errors
    ///
    /// Returns an error if a pour names an unknown material.
    pub fn apply(&mut self, world: &mut World, tick: u64) -> Result<(), ScriptError> {
        for (p, body) in self.script.pours.iter().zip(&mut self.bodies) {
            let Some(i) = tick.checked_sub(p.tick).filter(|&i| i < p.repeat) else {
                continue;
            };
            let material = world
                .materials()
                .id(&p.material)
                .ok_or_else(|| ScriptError::UnknownMaterial(p.material.clone()))?;
            let brush = match p.brush {
                BrushName::Pour => Brush::Pour,
                BrushName::Replace => Brush::Replace,
            };
            let x =
                p.x.saturating_add(p.dx.saturating_mul(i64::try_from(i).unwrap_or(i64::MAX)));
            // Clamped far outside any grid, so `paint`'s clipping maths can't
            // overflow; a disc there misses the grid either way.
            let far = 4 * i64::try_from(crate::MAX_SIDE).unwrap_or(i64::MAX / 8);
            let (x, y) = (x.clamp(-far, far), p.y.clamp(-far, far));
            let (_, joined) = world.paint_into(x, y, p.radius, material, brush, *body)?;
            *body = joined;
            if p.glued
                && let Some(id) = joined
            {
                world.set_anchored(id, true);
            }
        }
        Ok(())
    }

    /// Ticks the world from its current tick up to `until`, applying pours
    /// on the way.
    ///
    /// # Errors
    ///
    /// Returns an error if a pour names an unknown material.
    pub fn run(&mut self, world: &mut World, until: u64) -> Result<(), ScriptError> {
        while world.tick_count() < until {
            self.apply(world, world.tick_count())?;
            world.tick();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MaterialTable;

    const SCRIPT: &str = r#"
        ticks = 60
        [world]
        width = 64
        height = 48
        [[pour]]
        tick = 2
        repeat = 10
        x = 20
        y = 8
        dx = 2
        material = "sand"
    "#;

    fn world(script: &Script, seed: u64) -> World {
        World::new(script.world_config(seed), MaterialTable::builtin().unwrap()).unwrap()
    }

    #[test]
    fn parses_defaults() {
        let s = Script::parse("").unwrap();
        assert_eq!((s.ticks, s.frame_every), (0, 1));
        assert_eq!((s.world.width, s.world.height), (160, 90));
        let s = Script::parse(SCRIPT).unwrap();
        assert_eq!(s.pours[0].radius, 2);
        assert_eq!(s.pours[0].brush, BrushName::Pour);
    }

    #[test]
    fn rejects_unknown_keys_and_materials() {
        assert!(Script::parse("tickz = 3").is_err());
        let s = Script::parse("[[pour]]\nx = 1\ny = 1\nmaterial = \"lava\"").unwrap();
        let mut w = world(&s, 1);
        assert!(matches!(
            s.run(&mut w, 1),
            Err(ScriptError::UnknownMaterial(_))
        ));
    }

    #[test]
    fn replays_deterministically() {
        let s = Script::parse(SCRIPT).unwrap();
        let (mut a, mut b) = (world(&s, 3), world(&s, 3));
        s.run(&mut a, s.ticks).unwrap();
        s.run(&mut b, 20).unwrap();
        s.run(&mut b, s.ticks).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        let sand = a.materials().id("sand").unwrap();
        assert!(a.material_counts()[usize::from(sand.0)] > 50);
    }

    #[test]
    fn rejects_zero_frame_every_and_survives_extreme_coordinates() {
        assert!(matches!(
            Script::parse("frame_every = 0"),
            Err(ScriptError::ZeroFrameEvery)
        ));
        assert_eq!(Script::default().frame_every, 1);
        let s = Script::parse(&format!(
            "[[pour]]\nx = {}\ny = {}\nmaterial = \"sand\"\n[[pour]]\nx = {}\ny = 0\ndx = {}\nrepeat = 3\nmaterial = \"sand\"",
            i64::MAX,
            i64::MIN,
            i64::MIN,
            i64::MAX
        ))
        .unwrap();
        let mut w = world(&s, 1);
        s.run(&mut w, 3).unwrap();
    }

    #[test]
    fn pours_only_within_their_window() {
        let s = Script::parse(SCRIPT).unwrap();
        let mut w = world(&s, 3);
        s.run(&mut w, 2).unwrap();
        let sand = w.materials().id("sand").unwrap();
        assert_eq!(w.material_counts()[usize::from(sand.0)], 0);
    }
}
