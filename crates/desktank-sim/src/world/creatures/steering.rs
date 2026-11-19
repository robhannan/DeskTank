//! Species-specific steering in cell space. None of these probes edits the grid.

use rand_pcg::Pcg32;

use super::{
    Creature, FEET, Kind, ONE, REACHED, TICKS_PER_SECOND, World, Zone, body, centre_of, dist,
    extent, to_i64,
};

impl World {
    /// One pixel step at the species' cruising speed. Trace the contour
    /// with solid material on the right, including inside/outside corners.
    /// A scooped support or a net drop into open water lets the snail sink.
    /// After the step it turns its foot onto the face it holds.
    pub(super) fn crawl(&self, k: &Kind, c: &mut Creature) {
        let interval = u64::try_from((ONE + k.cruise / 2) / k.cruise).unwrap_or(1);
        if !(self.tick + u64::from(c.id.0)).is_multiple_of(interval) {
            return;
        }
        let (turn, band) = (c.turn, c.band());
        let free = |dx, dy| self.fits(k, turn, c.x + dx * ONE, c.y + dy * ONE, band);
        let face = self.support_face(k, turn, c.x, c.y, band);
        let heading = if (c.vx, c.vy) != (0, 0)
            && (face.is_some() || self.at_corner(k, turn, c.x, c.y, band))
        {
            (c.vx.signum(), c.vy.signum())
        } else if let Some((x, y)) = face {
            (y, -x)
        } else {
            if free(0, 1) {
                c.y += ONE;
            }
            (c.vx, c.vy) = (0, 0);
            return;
        };
        let (dx, dy) = heading;
        if let Some((x, y)) = [(-dy, dx), heading, (dy, -dx), (-dx, -dy)]
            .into_iter()
            .find(|&(x, y)| {
                free(x, y) && self.on_surface(k, turn, c.x + x * ONE, c.y + y * ONE, band)
            })
        {
            c.x += x * ONE;
            c.y += y * ONE;
            (c.vx, c.vy) = (x * k.cruise, y * k.cruise);
        }
        // Only diagonal support (an outside corner): keep the turn until a
        // face is found, so it doesn't flicker.
        if let Some(face) = self.support_face(k, c.turn, c.x, c.y, band) {
            self.attach(k, c, face);
        }
        c.face_along(c.vx, c.vy);
    }

    /// Turns `c` so its foot rests on `face`. The new box keeps the old
    /// box's edge on that face and slides along it, nearest first and ahead
    /// before behind, by up to the difference in its sides; if no spot fits
    /// and touches the face, `c` keeps its turn.
    fn attach(&self, k: &Kind, c: &mut Creature, face: (i32, i32)) {
        let Some(turn) = FEET.iter().position(|&f| f == face) else {
            return;
        };
        let turn = u8::try_from(turn).unwrap_or(0);
        if turn == c.turn {
            return;
        }
        let ((ow, oh), (nw, nh)) = (extent(k, c.turn), extent(k, turn));
        let (fx, fy) = face;
        let (x, y) = (
            c.x + fx * (ow - nw) * ONE / 2,
            c.y + fy * (oh - nh) * ONE / 2,
        );
        // Along the face, signed so positive is the way it was going.
        let (ax, ay) = (fy.abs(), fx.abs());
        let ahead = if c.vx * ax + c.vy * ay < 0 { -1 } else { 1 };
        let slide = (k.w - k.h).abs();
        for s in std::iter::once(0).chain((1..=slide).flat_map(|s| [s, -s])) {
            let (nx, ny) = (x + ax * ahead * s * ONE, y + ay * ahead * s * ONE);
            let band = c.band();
            if self.fits(k, turn, nx, ny, band)
                && self.supported(k, turn, nx + fx * ONE, ny + fy * ONE, band)
            {
                (c.x, c.y, c.turn) = (nx, ny, turn);
                return;
            }
        }
    }

    /// Whether a body in `band` at `(x, y)` overlaps something it can hold:
    /// the tank glass, or a cell that blocks it other than air (a plant
    /// only in reach of its band).
    fn supported(&self, k: &Kind, turn: u8, x: i32, y: i32, band: u8) -> bool {
        let Some(b) = body(k, turn, x, y, self.width, self.height) else {
            return true; // tank glass
        };
        (b.y0..b.y1).any(|y| {
            (y * self.width + b.x0..y * self.width + b.x1)
                .any(|i| self.cells[i].material != self.creatures.air && self.blocks(i, Some(band)))
        })
    }

    /// The side a body turned `turn` at `(x, y)` holds on to: the face under
    /// its foot if there is one, so a crawler in an inside corner keeps its
    /// turn, then down, right, left and up.
    fn support_face(&self, k: &Kind, turn: u8, x: i32, y: i32, band: u8) -> Option<(i32, i32)> {
        std::iter::once(FEET[usize::from(turn)])
            .chain([(0, 1), (1, 0), (-1, 0), (0, -1)])
            .find(|&(dx, dy)| self.supported(k, turn, x + dx * ONE, y + dy * ONE, band))
    }

    fn at_corner(&self, k: &Kind, turn: u8, x: i32, y: i32, band: u8) -> bool {
        [(1, 1), (-1, 1), (1, -1), (-1, -1)]
            .into_iter()
            .any(|(dx, dy)| self.supported(k, turn, x + dx * ONE, y + dy * ONE, band))
    }

    pub(super) fn on_surface(&self, k: &Kind, turn: u8, x: i32, y: i32, band: u8) -> bool {
        self.support_face(k, turn, x, y, band).is_some() || self.at_corner(k, turn, x, y, band)
    }

    pub(super) fn steering_target(
        &self,
        k: &Kind,
        c: &mut Creature,
        rng: &mut Pcg32,
    ) -> (i32, i32, i32) {
        if c.retarget == 0 || dist(c.tx - c.x, c.ty - c.y) <= REACHED {
            self.pick_target(k, c, rng);
        } else {
            c.retarget -= 1;
        }

        if let Some((px, py)) = self.creatures.cursor {
            let (mut dx, dy) = (c.x - px, c.y - py);
            if dist(dx, dy) < i64::from(3 * k.w * ONE) {
                if dx == 0 && (dy == 0 || k.zone == Zone::Bottom) {
                    dx = if c.facing_left { -ONE } else { ONE };
                }
                return (c.x + dx * 8, c.y + dy * 8, k.top);
            }
        }

        if k.breathes_air
            && self.surface_row > 0
            && let Some(target) = self.air_target(k, c)
        {
            return target;
        }
        if let Some(i) = c.food {
            let i = i as usize;
            return (
                centre_of(to_i64(i % self.width)),
                centre_of(to_i64(i / self.width)),
                k.top,
            );
        }
        if k.schooling {
            if c.stray > 0 {
                c.stray -= 1;
            } else if let Some(target) = self.school_target(k, c) {
                return target;
            }
        }
        (
            c.tx,
            if k.zone == Zone::Bottom { c.y } else { c.ty },
            k.cruise,
        )
    }

    fn air_target(&self, k: &Kind, c: &mut Creature) -> Option<(i32, i32, i32)> {
        const BETWEEN_BREATHS: u64 = 20 * TICKS_PER_SECOND as u64;
        const SKIM: u64 = 3 * TICKS_PER_SECOND as u64;
        if self.tick >= c.next_breath {
            let b = self.creature_body(c);
            if b.y0 > 0
                && (b.x0..b.x1)
                    .any(|x| self.cells[(b.y0 - 1) * self.width + x].material == self.creatures.air)
            {
                c.next_breath = self.tick + BETWEEN_BREATHS;
            } else {
                // Rise vertically, moving sideways under ledges until
                // there is open water overhead.
                let tx = if self.fits(k, c.turn, c.x, c.y - ONE, c.band()) {
                    c.x
                } else {
                    c.tx
                };
                return Some((tx, c.y - 8 * ONE, k.top));
            }
        }
        if c.next_breath.saturating_sub(self.tick) > BETWEEN_BREATHS - SKIM {
            // Skim the interface for floating food before diving again.
            let tx = c
                .food
                .map_or(c.tx, |i| centre_of(to_i64(i as usize % self.width)));
            return Some((tx, c.y - ONE, k.top));
        }
        None
    }

    /// The first free schoolmate wanders. Others follow its heading, with
    /// close neighbours pushing apart. All sample the same tick snapshot.
    fn school_target(&self, k: &Kind, c: &Creature) -> Option<(i32, i32, i32)> {
        let mut mates = self
            .creatures
            .school
            .iter()
            .filter(|m| m.species == c.species);
        let lead = mates.next()?;
        if c.id == lead.id {
            return None;
        }
        let spacing = k.w * ONE;
        let mut tx = lead.x + 8 * lead.vx;
        let mut ty = lead.y + 8 * lead.vy;
        for m in std::iter::once(lead).chain(mates).filter(|m| m.id != c.id) {
            let (mut dx, dy) = (c.x - m.x, c.y - m.y);
            if dx.abs() >= spacing || dy.abs() >= spacing {
                continue;
            }
            if (dx, dy) == (0, 0) {
                dx = if c.id > m.id { ONE } else { -ONE };
            }
            let d = i32::try_from(dist(dx, dy)).unwrap_or(spacing).max(1);
            let push = (spacing - d).max(0) * 3;
            tx += dx * push / d;
            ty += dy * push / d;
        }
        Some((tx, ty, k.top))
    }
}
