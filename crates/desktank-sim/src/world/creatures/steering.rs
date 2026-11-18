//! Species-specific steering in cell space. None of these probes edits the grid.

use rand_pcg::Pcg32;

use super::{
    Creature, Kind, ONE, REACHED, TICKS_PER_SECOND, World, Zone, body, centre_of, dist, to_i64,
};

impl World {
    /// One pixel step at the species' cruising speed. Trace the contour
    /// with solid material on the right, including inside/outside corners.
    /// A scooped support or a net drop into open water lets the snail sink.
    pub(super) fn crawl(&self, k: &Kind, c: &mut Creature) {
        let interval = u64::try_from((ONE + k.cruise / 2) / k.cruise).unwrap_or(1);
        if !(self.tick + u64::from(c.id.0)).is_multiple_of(interval) {
            return;
        }
        let free = |dx, dy| self.fits(k, c.x + dx * ONE, c.y + dy * ONE);
        let face = self.support_face(k, c.x, c.y);
        let heading = if (c.vx, c.vy) != (0, 0) && (face.is_some() || self.at_corner(k, c.x, c.y)) {
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
            .find(|&(x, y)| free(x, y) && self.on_surface(k, c.x + x * ONE, c.y + y * ONE))
        {
            c.x += x * ONE;
            c.y += y * ONE;
            (c.vx, c.vy) = (x * k.cruise, y * k.cruise);
            if x != 0 {
                c.facing_left = x < 0;
            }
        }
    }

    fn supported(&self, k: &Kind, x: i32, y: i32) -> bool {
        let Some(b) = body(k, x, y, self.width, self.height) else {
            return true; // tank glass
        };
        (b.y0..b.y1).any(|y| {
            self.cells[y * self.width + b.x0..y * self.width + b.x1]
                .iter()
                .any(|cell| {
                    cell.material != self.creatures.air
                        && !self.creatures.swim[usize::from(cell.material.0)]
                })
        })
    }

    fn support_face(&self, k: &Kind, x: i32, y: i32) -> Option<(i32, i32)> {
        [(0, 1), (1, 0), (-1, 0), (0, -1)]
            .into_iter()
            .find(|&(dx, dy)| self.supported(k, x + dx * ONE, y + dy * ONE))
    }

    fn at_corner(&self, k: &Kind, x: i32, y: i32) -> bool {
        [(1, 1), (-1, 1), (1, -1), (-1, -1)]
            .into_iter()
            .any(|(dx, dy)| self.supported(k, x + dx * ONE, y + dy * ONE))
    }

    pub(super) fn on_surface(&self, k: &Kind, x: i32, y: i32) -> bool {
        self.support_face(k, x, y).is_some() || self.at_corner(k, x, y)
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
        if k.schooling
            && let Some(target) = self.school_target(k, c)
        {
            return target;
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
                let tx = if self.fits(k, c.x, c.y - ONE) {
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
