//! Crawling the front and back panes (#371).
//!
//! A surface crawler with pane art (top and under views) spends part of
//! its time on the front or back glass. In its trip window, upright on the
//! bed or a top face, it shifts to the nearer pane's band (the front band, 0, or the back band) and takes
//! hold of the glass there. On a pane it wanders the water in four
//! headings, its body the pane view's box turned to the heading, and
//! grazes as it goes. When its trip is over, or a fright sends it off, it
//! heads down until it is stopped (the bed, a rock, a plant in its band)
//! and steps off onto that face, a crawler on the edges again.
//!
//! The front pane shows its underside, the back pane its top; the
//! renderer picks the look from [`Creature::pane`].

use rand_core::Rng as _;
use rand_pcg::Pcg32;

use super::TICKS_PER_SECOND;
use super::{Creature, DEPTH_BANDS, Kind, ONE, World, z_of};
use crate::save::SaveError;

/// Seconds between the starts of a crawler's pane trips.
const TRIP_EVERY: u64 = 150;
/// Seconds of each trip it may spend on a pane.
const TRIP_SECONDS: u64 = 60;
/// Cells a crawler taking hold of a pane may shift to fit its pane box.
const HOLD_RADIUS: i32 = 3;
/// Ticks it keeps one wander target on a pane, at most.
const WANDER_TICKS: u32 = 20 * TICKS_PER_SECOND;

/// Which pane a creature is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    /// None: it is on the bed, the side glass, hardscape or plants, or
    /// swimming.
    Off,
    /// The front glass: seen from below.
    Front,
    /// The back glass: seen from above.
    Back,
}

impl Pane {
    /// The saved byte (0 off, 1 front, 2 back).
    pub(in crate::world) fn code(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Front => 1,
            Self::Back => 2,
        }
    }

    /// The pane for a saved byte.
    pub(in crate::world) fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Off),
            1 => Some(Self::Front),
            2 => Some(Self::Back),
            _ => None,
        }
    }

    /// Its depth band.
    fn band(self) -> u8 {
        if self == Self::Back {
            DEPTH_BANDS - 1
        } else {
            0
        }
    }
}

impl Creature {
    /// The pane it is on (#371).
    #[must_use]
    pub fn pane(&self) -> Pane {
        self.pane
    }
}

/// The quarter turn and mirroring that head a pane crawler along `(dx,
/// dy)`: its sprite faces right, turned clockwise (down is one turn).
fn heading(dx: i32, dy: i32) -> (u8, bool) {
    match (dx.signum(), dy.signum()) {
        (-1, _) => (0, true),
        (0, 1) => (1, false),
        (0, -1) => (3, false),
        _ => (0, false),
    }
}

impl World {
    /// Whether `c`'s pane trip window is open: [`TRIP_SECONDS`] of every
    /// [`TRIP_EVERY`], offset by its id.
    fn pane_time(&self, c: &Creature) -> bool {
        let every = TRIP_EVERY * u64::from(TICKS_PER_SECOND);
        (self.tick + u64::from(c.id.0) * 7_919) % every < TRIP_SECONDS * u64::from(TICKS_PER_SECOND)
    }

    /// A crawler off the panes (`k` its edge kind): in its trip window it
    /// heads for the nearer pane's band and, once there, takes hold of the
    /// pane where its pane box fits nearby, heading up. Returns whether it
    /// is on a pane now.
    pub(super) fn seek_pane(&self, k: &Kind, c: &mut Creature) -> bool {
        let Some(size) = k.pane else {
            return false;
        };
        // It sets off from the bed (or a top face), upright: a snail walking
        // up the glass from the sand.
        if c.fear > 0
            || !self.pane_time(c)
            || c.turn != 0
            || !self.supported(k, 0, c.x, c.y + ONE, c.band())
        {
            return false;
        }
        let pane = if c.z * 2 < z_of(DEPTH_BANDS - 1) {
            Pane::Front
        } else {
            Pane::Back
        };
        let band = pane.band();
        if !(k.bands.0..=k.bands.1).contains(&band) {
            return false;
        }
        if c.band() != band || c.z != z_of(band) {
            c.tz = z_of(band);
            return false;
        }
        let pk = k.sized(size);
        let Some((x, y)) = self.nearest_fit(&pk, 3, c.x, c.y, band, HOLD_RADIUS) else {
            return false;
        };
        (c.x, c.y, c.turn, c.facing_left, c.pane) = (x, y, 3, false, pane);
        (c.vx, c.vy) = (0, -k.cruise);
        c.retarget = 0;
        true
    }

    /// One tick of a crawler on a pane (`k` its pane kind): a step a cell
    /// at its cruising pace towards its wander target, along the axis
    /// furthest off first, turned to the step. Its trip over (or
    /// frightened) it heads down instead, and steps off where it is
    /// stopped.
    pub(super) fn pane_step(&self, k: &Kind, c: &mut Creature, rng: &mut Pcg32) {
        let interval = u64::try_from((ONE + k.cruise / 2) / k.cruise).unwrap_or(1);
        if !(self.tick + u64::from(c.id.0)).is_multiple_of(interval) {
            return;
        }
        let leaving = c.fear > 0 || !self.pane_time(c);
        let band = c.pane.band();
        if leaving {
            (c.tx, c.ty) = (c.x, i32::try_from(self.height).unwrap_or(i32::MAX) * ONE);
        } else if c.retarget == 0 || (c.tx - c.x).abs() < ONE && (c.ty - c.y).abs() < ONE {
            self.pane_target(k, c, rng);
        } else {
            c.retarget = c
                .retarget
                .saturating_sub(u32::try_from(interval).unwrap_or(1));
        }
        let (dx, dy) = ((c.tx - c.x) / ONE, (c.ty - c.y) / ONE);
        let along = |v: i32| if v == 0 { None } else { Some(v.signum()) };
        let mut moves = [along(dx).map(|s| (s, 0)), along(dy).map(|s| (0, s))];
        if dy.abs() > dx.abs() {
            moves.swap(0, 1);
        }
        for (sx, sy) in moves.into_iter().flatten() {
            let (turn, left) = heading(sx, sy);
            let (x, y) = (c.x + sx * ONE, c.y + sy * ONE);
            if self.fits(k, turn, x, y, band) {
                (c.x, c.y, c.turn, c.facing_left) = (x, y, turn, left);
                (c.vx, c.vy) = (sx * k.cruise, sy * k.cruise);
                return;
            }
        }
        (c.vx, c.vy) = (0, 0);
        if leaving || dy > 0 && dx == 0 {
            self.step_off(k, c);
        } else {
            c.retarget = 0;
        }
    }

    /// A new wander target in the water on `c`'s pane, kept for at most
    /// [`WANDER_TICKS`].
    fn pane_target(&self, k: &Kind, c: &mut Creature, rng: &mut Pcg32) {
        let (width, height) = (
            i32::try_from(self.width).unwrap_or(i32::MAX),
            i32::try_from(self.height).unwrap_or(i32::MAX),
        );
        let top = i32::try_from(self.surface_row).unwrap_or(0) + 1;
        let half = k.w.max(k.h) / 2 + 1;
        let span = |lo: i32, hi: i32, rng: &mut Pcg32| {
            let count = u32::try_from((hi - lo).max(1)).unwrap_or(1);
            lo + i32::try_from(rng.next_u32() % count).unwrap_or(0)
        };
        c.tx = span(half, width - half, rng) * ONE + ONE / 2;
        c.ty = span(top + half, height - half, rng) * ONE + ONE / 2;
        c.retarget = WANDER_TICKS;
    }

    /// Off the pane where `c` is, upright with its edge box (it takes hold
    /// of the face under or beside it on its next crawl); it holds on if
    /// that box doesn't fit nearby.
    pub(super) fn step_off(&self, k: &Kind, c: &mut Creature) {
        let ek = k.sized(k.side);
        let band = c.band();
        if let Some((x, mut y)) = self.nearest_fit(&ek, 0, c.x, c.y, band, HOLD_RADIUS) {
            // Down onto what stopped it: the edge box is shorter.
            for _ in 0..2 * HOLD_RADIUS {
                if !self.fits(&ek, 0, x, y + ONE / 2, band) {
                    break;
                }
                y += ONE / 2;
            }
            (c.x, c.y, c.turn, c.pane) = (x, y, 0, Pane::Off);
            (c.vx, c.vy) = (0, 0);
            c.retarget = 0;
        }
    }
}

impl World {
    /// Each creature's pane for a save.
    pub(in crate::world) fn save_panes(&self) -> Vec<u8> {
        self.creatures.list.iter().map(|c| c.pane.code()).collect()
    }

    /// Restores each creature's pane from a save: only a crawler with pane
    /// art is on one, in that pane's band.
    pub(in crate::world) fn load_panes(&mut self, saved: &[u8]) -> Result<(), SaveError> {
        if saved.len() != self.creatures.list.len() {
            return Err(SaveError::Invalid(format!(
                "{} creature panes for {} creatures",
                saved.len(),
                self.creatures.list.len()
            )));
        }
        let art: Vec<bool> = self
            .creatures
            .list
            .iter()
            .map(|c| {
                self.kind(c.species)
                    .is_some_and(|k| k.zone == super::Zone::Surfaces && k.pane.is_some())
            })
            .collect();
        for ((c, &code), art) in self.creatures.list.iter_mut().zip(saved).zip(art) {
            let pane = Pane::from_code(code)
                .filter(|&p| p == Pane::Off || (art && c.band() == p.band()))
                .ok_or_else(|| {
                    SaveError::Invalid(format!("creature {}: pane out of range", c.id.0))
                })?;
            c.pane = pane;
        }
        Ok(())
    }
}

impl Kind {
    /// This kind with a `(w, h)` body.
    pub(super) fn sized(&self, (w, h): (i32, i32)) -> Self {
        let mut k = *self;
        (k.w, k.h) = (w, h);
        k
    }
}
