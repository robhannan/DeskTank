//! Species-specific steering in cell space. None of these probes edits the grid.

use rand_pcg::Pcg32;

use super::quirks::Quirks;

use super::{
    Creature, FEET, Kind, ONE, REACHED, World, Zone, body, centre_of, dist, extent, school, to_i64,
    zone,
};
use crate::rng::splitmix64;

/// How far a hungry shrimp senses a grazeable sponge filter, in cells (DV4).
const SPONGE_GRAZE_SENSE: i64 = 96;
/// Ticks a grazing shrimp keeps one perch spot on the sponge before shifting
/// along the foam (10 world seconds).
const SPONGE_PERCH_TICKS: u64 = 200;
/// Salt for deterministic sponge-perch selection.
const SPONGE_PERCH_SALT: u64 = 0x7370_6f6e_6765;

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
        // Landed from a swim: back onto whole cells, heading along a face.
        self.settle(k, c);
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

    /// A graze perch `(tx, ty, device_mid_x)` on the nearest accessible
    /// shrimp-grazeable device (sponge filter, DV4), if `c` is a hungry shrimp
    /// with no closer loose food cell. Returns `None` in O(1) when there is no
    /// grazeable device.
    #[allow(clippy::many_single_char_names)] // fixed-point cell maths
    pub(super) fn sponge_graze_target(
        &self,
        k: &Kind,
        c: &Creature,
        hungry: bool,
    ) -> Option<(i32, i32, i32)> {
        if !k.is_shrimp || !hungry || c.fear > 0 || !self.has_grazeable_device() {
            return None;
        }
        let (cx, cy) = (i64::from(c.x >> 8), i64::from(c.y >> 8));
        // If loose food is already targeted, only prefer a sponge that is closer.
        let mut best_d2 = SPONGE_GRAZE_SENSE * SPONGE_GRAZE_SENSE;
        if let Some(fi) = c.food {
            let fx = to_i64(fi as usize % self.width);
            let fy = to_i64(fi as usize / self.width);
            let food_d2 = (fx - cx).pow(2) + (fy - cy).pow(2);
            best_d2 = best_d2.min(food_d2);
        }
        let top_limit = i32::try_from(self.surface_row + 2).unwrap_or(i32::MAX) * ONE;
        let band = c.band();
        let mut best: Option<(i64, (i32, i32, i32))> = None;

        for d in self.devices.values() {
            let art = self.devices.kinds().get(d.kind);
            let Some((gy0, gy1)) = d.graze_band(art) else {
                continue;
            };
            let (x0, x1) = (d.x, d.x + art.size.0);
            let (y0, y1) = (gy0, gy1 + 1);
            let mid_x = to_i64(usize::midpoint(x0, x1));
            let mid_y = to_i64(usize::midpoint(y0, y1));
            let d2 = (mid_x - cx).pow(2) + (mid_y - cy).pow(2);
            if d2 > best_d2 {
                continue;
            }
            let h = splitmix64(
                self.seed
                    ^ u64::from(c.id.0).wrapping_mul(SPONGE_PERCH_SALT)
                    ^ (u64::from(d.id.0) << 16)
                    ^ (self.tick / SPONGE_PERCH_TICKS),
            );
            let fh = (y1 - y0).max(1);
            let fy_cell = y0 + usize::try_from(h % fh as u64).unwrap_or(0);
            let fy = centre_of(to_i64(fy_cell));
            let low_y = centre_of(to_i64(y1.saturating_sub(2).max(y0)));
            let lx = i32::try_from(x0).unwrap_or(0) * ONE - k.w * ONE / 2;
            let rx = i32::try_from(x1).unwrap_or(0) * ONE + k.w * ONE / 2;
            let sy = i32::try_from(y0).unwrap_or(0) * ONE - k.h * ONE / 2;
            let slx = centre_of(to_i64((x0 + 2).min(x1.saturating_sub(1))));
            let srx = centre_of(to_i64(x1.saturating_sub(3).max(x0)));
            let near_left = c.x < centre_of(mid_x);
            let prefer_shoulder = (h >> 12).is_multiple_of(4);
            let candidates = match (near_left, prefer_shoulder) {
                (true, false) => [(lx, fy), (lx, low_y), (slx, sy), (rx, fy), (srx, sy)],
                (true, true) => [(slx, sy), (lx, fy), (lx, low_y), (srx, sy), (rx, fy)],
                (false, false) => [(rx, fy), (rx, low_y), (srx, sy), (lx, fy), (slx, sy)],
                (false, true) => [(srx, sy), (rx, fy), (rx, low_y), (slx, sy), (lx, fy)],
            };
            let Some((tx, ty)) = candidates
                .into_iter()
                .find(|&(tx, ty)| ty >= top_limit && self.fits(k, 0, tx, ty, band))
            else {
                continue;
            };
            let target = (tx, ty, centre_of(mid_x));
            if best.is_none_or(|(bd, _)| d2 < bd) {
                best = Some((d2, target));
                best_d2 = d2;
            }
        }
        best.map(|(_, t)| t)
    }

    pub(super) fn steering_target(
        &self,
        k: &Kind,
        c: &mut Creature,
        sponge: Option<(i32, i32, i32)>,
        rng: &mut Pcg32,
    ) -> (i32, i32, i32) {
        if let Some(target) = super::pursuit::flee_target(k, c) {
            return target;
        }
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
            && let Some(target) = self.air_target(k, c)
        {
            return target;
        }
        if let Some((gx, gy, _)) = sponge {
            let speed = if dist(gx - c.x, gy - c.y) > i64::from(6 * ONE) {
                k.top
            } else {
                k.cruise
            };
            return (gx, gy, speed);
        }
        if let Some(i) = c.food {
            let i = i as usize;
            return (
                centre_of(to_i64(i % self.width)),
                centre_of(to_i64(i / self.width)),
                k.top,
            );
        }
        if let Some(target) = self.chase_target(k, c) {
            return target;
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

    /// The school's heads wander (its lead, and a sub-lead while it is
    /// split); the others swim loosely around their head (see
    /// [`school`](super::school) for why the school stays irregular and out
    /// of step, and how it splits and rejoins). All sample the same tick
    /// snapshot.
    fn school_target(&self, k: &Kind, c: &Creature) -> Option<(i32, i32, i32)> {
        let all = &self.creatures.school;
        let lead = all.iter().find(|m| m.species == c.species)?;
        let find = |id| {
            all.binary_search_by_key(&id, |m| m.id)
                .ok()
                .map(|i| &all[i])
        };
        let head = find(c.id).and_then(|m| find(m.head)).unwrap_or(lead);
        let quirks = Quirks::of(self.seed, c.id, self.tick);
        if c.id == head.id {
            let ty = if k.zone == Zone::Bottom { c.y } else { c.ty };
            return Some((c.tx, ty, school::lead_pace(quirks, k)));
        }
        // Places are laid out in the full-grown length, so young fish
        // spread through the school too; personal space is their own.
        let grown = self.kind(c.species).map_or(k.w, |g| g.size_of(c.female).0) * ONE;
        let shape = school::shape(Quirks::of(self.seed, head.id, self.tick));
        let (along, across) = school::slot(quirks, grown, shape);
        let (hx, hy) = school::axis(k, head);
        let mut tx = head.x + 8 * head.vx + (along * hx - across * hy) / ONE;
        let mut ty = head.y + 8 * head.vy + (along * hy + across * hx) / ONE;
        let len = k.w * ONE;
        let spacing = school::spacing(quirks, len);
        let (mut px, mut py) = (0, 0);
        // The mean heading of the group's mates close by.
        let (mut ax, mut ay, mut near) = (0, 0, 0);
        let reach = school::align_reach(grown);
        for m in all
            .iter()
            .filter(|m| m.species == c.species && m.id != c.id)
        {
            if m.head == head.id && dist(c.x - m.x, c.y - m.y) <= reach {
                (ax, ay, near) = (ax + m.vx, ay + m.vy, near + 1);
            }
            // Personal space is a body long and half as tall, like the
            // fish, so close neighbours stack loosely rather than in a disc.
            let (mut dx, dy) = (c.x - m.x, 2 * (c.y - m.y));
            if dx.abs() >= spacing || dy.abs() >= spacing {
                continue;
            }
            if (dx, dy) == (0, 0) {
                dx = if c.id > m.id { ONE } else { -ONE };
            }
            let d = i32::try_from(dist(dx, dy)).unwrap_or(spacing).max(1);
            let push = (spacing - d).max(0) * 3;
            px += dx * push / d;
            py += dy * push / d;
        }
        // However crowded it is, the push is at most its personal space:
        // summed over a packed clump it used to shove every fish round it
        // out to one radius, a ring.
        let p = dist(px, py);
        if p > i64::from(spacing) {
            // In i64: a big body in a packed clump can push past i32 here.
            let scale =
                |v: i32| i32::try_from(i64::from(v) * i64::from(spacing) / p).unwrap_or(spacing);
            (px, py) = (scale(px), scale(py));
        }
        tx += px;
        ty += py;
        if near > 0 {
            tx += ax / near * school::ALIGN_TICKS;
            ty += ay / near * school::ALIGN_TICKS;
        }
        // A slot reaches 3.5 lengths from its head: keep it to the rows
        // that hold the whole body in the species' zone and the columns
        // that hold it inside the side walls, as a wander target is, so a
        // follower near an edge isn't led out of its band or the tank.
        tx = school::clamp_to_walls(tx, k.w, self.width);
        let (top, bottom) = zone(k, self.surface_row, self.height);
        let hh = i64::from(k.h / 2);
        let (y0, y1) = (to_i64(top) + hh, to_i64(bottom) - hh - 1);
        if y0 <= y1 {
            ty = ty.clamp(centre_of(y0), centre_of(y1));
        }
        let far = dist(tx - c.x, ty - c.y) > i64::from(school::CATCH_UP * grown);
        Some((tx, ty, school::pace(quirks, k, far)))
    }
}
