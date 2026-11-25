//! Species-specific steering in cell space, including curious fish approaching
//! the host cursor. None of these probes edits the grid.

use rand_pcg::Pcg32;

use super::quirks::Quirks;

use super::{
    Creature, DEPTH_BANDS, FEET, Kind, ONE, REACHED, TICKS_PER_SECOND, World, Zone, body,
    centre_of, clamp_target, dist, extent, school, to_i64, z_of, zone,
};
use crate::rng::splitmix64;

/// How far a hungry shrimp senses a grazeable sponge filter, in cells (DV4).
const SPONGE_GRAZE_SENSE: i64 = 96;
/// Ticks a grazing shrimp keeps one perch spot on the sponge before shifting
/// along the foam (10 world seconds).
const SPONGE_PERCH_TICKS: u64 = 200;
/// Salt for deterministic sponge-perch selection.
const SPONGE_PERCH_SALT: u64 = 0x7370_6f6e_6765;

const CURSOR_SENSE_LENGTHS: i64 = 6;
const CURSOR_INTEREST_TICKS: u64 = 20 * TICKS_PER_SECOND as u64;
const CURSOR_COOLDOWN_TICKS: u64 = 30 * TICKS_PER_SECOND as u64;
/// The approach eases in and holds within a fraction of a cell of its target.
const CURSOR_ARRIVAL: i64 = ONE as i64 / 8;

/// Salt for a curious fish's interest rhythm, and for its pour linger draw.
const CURSOR_INTEREST_SALT: u64 = 0x6375_7269_6f75;
pub(super) const POUR_SALT: u64 = 0x706f_7572;

/// Whether a curious fish is interested in the cursor this tick: for
/// [`CURSOR_INTEREST_TICKS`] of every interest-and-cool-down cycle, at a
/// phase of its own. It is worked out from the tick, not kept, so a saved
/// and reloaded tank watches and loses interest exactly as before.
pub(super) fn interested(seed: u64, id: super::CreatureId, tick: u64) -> bool {
    let period = CURSOR_INTEREST_TICKS + CURSOR_COOLDOWN_TICKS;
    let phase = splitmix64(seed ^ CURSOR_INTEREST_SALT ^ u64::from(id.0).wrapping_mul(0x9e37_79b9))
        % period;
    (tick + phase) % period < CURSOR_INTEREST_TICKS
}

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
    pub(super) fn supported(&self, k: &Kind, turn: u8, x: i32, y: i32, band: u8) -> bool {
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

    /// A shrimp foraging up a face (#370): a hungry upright shrimp whose
    /// food is above its body takes hold of a face beside it on the food's
    /// side (glass, rock, wood or a plant), turned onto it like a crawler.
    /// Returns whether it took hold.
    pub(super) fn start_climb(&self, k: &Kind, c: &mut Creature, hungry: bool) -> bool {
        if !k.is_shrimp || c.turn != 0 || !hungry || c.fear > 0 {
            return false;
        }
        let Some((fx, fy)) = c.food.map(|i| self.food_centre(i)) else {
            return false;
        };
        // Beyond the reach of its head: food it can eat from the bed (or
        // with a step up) isn't worth a climb.
        if fy >= c.y - extent(k, 0).1 * ONE / 2 - ONE {
            return false;
        }
        // Into the band of the face that holds the food, to reach it.
        if let Some((_, _, b)) = c.food.and_then(|i| self.food_face(k, c, i))
            && b != c.band()
        {
            c.tz = z_of(b);
        }
        let band = c.band();
        let sides = match fx.cmp(&c.x) {
            std::cmp::Ordering::Greater => [1, 1],
            std::cmp::Ordering::Less => [-1, -1],
            std::cmp::Ordering::Equal => [1, -1],
        };
        // Against the face, or at the spot beside it it walked to: a face
        // above its head (a film over a bare foot of rock) is held turned.
        let at_spot = self
            .climb_spot(k, c)
            .is_some_and(|x| (x - c.x).abs() <= ONE);
        for side in sides {
            if at_spot || self.supported(k, 0, c.x + side * ONE, c.y, band) {
                self.attach(k, c, (side, 0));
                if c.turn != 0 {
                    (c.vx, c.vy) = (0, -k.cruise);
                    c.face_along(0, -1);
                    return true;
                }
            }
        }
        false
    }

    /// A foraging shrimp (#370) climbs to food above it. Its tick on a
    /// face, or taking hold of one: it eats
    /// what it reaches, then steps (see [`World::climb`]). Returns whether
    /// it holds a face still, which takes the rest of its tick.
    pub(super) fn forage_climb(&mut self, k: &Kind, c: &mut Creature, hungry: bool) -> bool {
        if !k.is_shrimp || (c.turn == 0 && !self.start_climb(k, c, hungry)) {
            return false;
        }
        if hungry && self.anything_to_eat(k) && self.eat(k, c) {
            c.food = None;
        }
        self.climb(k, c, c.hungry());
        c.turn != 0
    }

    /// One tick of a shrimp on a face (`c.turn != 0`): fed, frightened or
    /// out of food, it lets go and drifts down; otherwise it steps along the
    /// face towards its food at its cruising pace, a step only where it
    /// gets closer, so it never paces back and forth. With no step closer
    /// and the food out of reach it gives that food up and lets go. A step
    /// onto a top face turns it upright, a walker again.
    pub(super) fn climb(&self, k: &Kind, c: &mut Creature, hungry: bool) {
        if c.turn == 0 {
            return;
        }
        if !hungry || c.fear > 0 {
            self.let_go(k, c);
            return;
        }
        if c.food.is_none() {
            c.food = self.nearest_food(k, c);
        }
        let Some((fx, fy)) = c.food.map(|i| self.food_centre(i)) else {
            self.let_go(k, c);
            return;
        };
        let interval = u64::try_from((ONE + k.cruise / 2) / k.cruise).unwrap_or(1);
        if !(self.tick + u64::from(c.id.0)).is_multiple_of(interval) {
            return;
        }
        let (turn, band) = (c.turn, c.band());
        let here = dist(fx - c.x, fy - c.y);
        let step = [(0, -1), (1, 0), (-1, 0), (0, 1)]
            .into_iter()
            .map(|(dx, dy)| (dx, dy, dist(fx - c.x - dx * ONE, fy - c.y - dy * ONE)))
            .filter(|&(dx, dy, d)| {
                let (x, y) = (c.x + dx * ONE, c.y + dy * ONE);
                d < here && self.fits(k, turn, x, y, band) && self.on_surface(k, turn, x, y, band)
            })
            .min_by_key(|&(_, _, d)| d);
        let Some((dx, dy, _)) = step else {
            c.ignore = c.food.take();
            self.let_go(k, c);
            return;
        };
        c.x += dx * ONE;
        c.y += dy * ONE;
        (c.vx, c.vy) = (dx * k.cruise, dy * k.cruise);
        if let Some(face) = self.support_face(k, c.turn, c.x, c.y, band) {
            self.attach(k, c, face);
        }
        // Pressed to the face: where a bite or a step left a gap under its
        // foot, it closes it, by half a cell where a box with an even side
        // sits off the cell grid.
        let (fx, fy) = FEET[usize::from(c.turn)];
        if c.turn != 0
            && let Some(d) = [ONE, ONE / 2]
                .into_iter()
                .find(|&d| self.fits(k, c.turn, c.x + fx * d, c.y + fy * d, band))
        {
            c.x += fx * d;
            c.y += fy * d;
        }
        c.face_along(c.vx, c.vy);
    }

    /// Turns a shrimp on a face upright where it is, shifted off the face
    /// as far as it must to fit, to drift back down to the bed. It holds on
    /// if there is no room to turn.
    pub(super) fn let_go(&self, k: &Kind, c: &mut Creature) {
        if c.turn == 0 {
            return;
        }
        let (fx, fy) = FEET[usize::from(c.turn)];
        let slide = (k.w - k.h).abs() / 2 + 1;
        if let Some((x, y)) = (0..=slide)
            .map(|s| (c.x - fx * s * ONE, c.y - fy * s * ONE))
            .find(|&(x, y)| self.fits(k, 0, x, y, c.band()))
        {
            (c.x, c.y, c.turn) = (x, y, 0);
            (c.vx, c.vy) = (0, k.cruise);
        }
    }

    /// The face that holds food cell `i`, for a shrimp to climb to it: the
    /// side of the food a blocking cell is on, `(dx, dy)`, and the band
    /// nearest `c`'s, within its species' bands, that cell blocks in.
    /// `None` if nothing but more food beside it blocks in any of those
    /// bands.
    fn food_face(&self, k: &Kind, c: &Creature, food: u32) -> Option<(i32, i32, u8)> {
        let (cell, width) = (food as usize, self.width);
        let (col, row) = (cell % width, cell / width);
        let beside = [
            (col + 1 < width).then(|| (1, 0, cell + 1)),
            (col > 0).then(|| (-1, 0, cell - 1)),
            (row + 1 < self.height).then(|| (0, 1, cell + width)),
            (row > 0).then(|| (0, -1, cell - width)),
        ];
        let (band, (lo, hi)) = (c.band(), k.bands);
        (0..DEPTH_BANDS)
            .flat_map(|d| [band.checked_sub(d), band.checked_add(d)])
            .flatten()
            .filter(|b| (lo..=hi).contains(b))
            .find_map(|b| {
                beside.iter().flatten().find_map(|&(dx, dy, j)| {
                    // More of a film is food, not the face it grows on.
                    (self.cells[j].material != self.creatures.air
                        && !self.edible_cell(k, j)
                        && self.blocks(j, Some(b)))
                    .then_some((dx, dy, b))
                })
            })
    }

    /// Where a hungry upright shrimp heads for food it would climb to
    /// (see [`World::start_climb`]): beside the face that holds it, its
    /// body's edge against the face, so it takes hold in the face's band.
    /// `None` for food it can reach from the bed, or on a top or under face.
    pub(super) fn climb_spot(&self, k: &Kind, c: &Creature) -> Option<i32> {
        let i = c.food?;
        let (_, fy) = self.food_centre(i);
        if !k.is_shrimp || c.turn != 0 || fy >= c.y - extent(k, 0).1 * ONE / 2 - ONE {
            return None;
        }
        let (dx, _, band) = self.food_face(k, c, i).filter(|&(dx, _, _)| dx != 0)?;
        // A film is held where it grows, and loose food by what it lies on.
        let col = to_i64(i as usize % self.width);
        let col = if self.blocks(i as usize, Some(band)) {
            col
        } else {
            col + i64::from(dx)
        };
        Some(centre_of(col) - dx * ONE / 2 - dx * k.w * ONE / 2)
    }

    /// The centre of food cell `i`, fixed point.
    fn food_centre(&self, i: u32) -> (i32, i32) {
        let i = i as usize;
        (
            centre_of(to_i64(i % self.width)),
            centre_of(to_i64(i / self.width)),
        )
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

    /// Target centre and speed. Zero speed means hovering in place.
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

        if !k.curious
            && let Some((px, py)) = self.creatures.cursor
        {
            let (mut dx, dy) = (c.x - px, c.y - py);
            if dist(dx, dy) < cursor_reach(k) {
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
        if let Some(x) = self.climb_spot(k, c) {
            return (x, c.y, k.top);
        }
        if let Some(i) = c.food {
            let (x, mut y) = self.food_centre(i);
            if k.zone != Zone::Bottom && self.is_floating_leaf(i) {
                // Bite from below. A leaf can sit above the waterline or
                // beside a solid leaf in the same band.
                let reach = i32::try_from(k.reach).unwrap_or(0);
                y += extent(k, c.turn).1 * ONE / 2 + reach * ONE;
            }
            return (x, y, k.top);
        }
        // A chase (a betta driving off a rival) comes before the cursor.
        if let Some(target) = self.chase_target(k, c) {
            return target;
        }
        if k.curious
            && let Some(target) = self.curious_cursor_target(k, c)
        {
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

    /// Aim along the line from the cursor to the fish, one and a half
    /// grown body lengths away from the cursor.
    fn curious_cursor_target(&self, k: &Kind, c: &mut Creature) -> Option<(i32, i32, i32)> {
        let (px, py) = self.creatures.cursor?;
        let (dx, dy) = (c.x - px, c.y - py);
        let distance = dist(dx, dy);
        let length = i64::from(k.w * ONE);
        if distance > CURSOR_SENSE_LENGTHS * length || !interested(self.seed, c.id, self.tick) {
            return None;
        }
        let stop = 3 * length / 2;
        if distance > stop {
            let offset = |axis| i32::try_from(i64::from(axis) * stop / distance).unwrap_or(0);
            let (tx, ty) = self.clamp_to_swim_zone(k, c, px + offset(dx), py + offset(dy));
            if dist(tx - c.x, ty - c.y) > CURSOR_ARRIVAL {
                return Some((tx, ty, k.cruise));
            }
        }
        c.face_along(-dx, -dy);
        Some((c.x, c.y, 0))
    }

    /// Body centres inside the depth zone and glass, using the wander bounds.
    pub(super) fn clamp_to_swim_zone(&self, k: &Kind, c: &Creature, x: i32, y: i32) -> (i32, i32) {
        let (top, bottom) = zone(k, self.surface_row, self.height);
        let (hw, hh) = (i64::from(k.w / 2 + 1), i64::from(k.h / 2));
        let x = clamp_target(x, hw, to_i64(self.width) - hw);
        let y = if k.zone == Zone::Bottom {
            c.y
        } else {
            clamp_target(y, to_i64(top) + hh, to_i64(bottom) - hh)
        };
        (x, y)
    }

    /// The school's heads wander (its lead, and a sub-lead while it is
    /// split); the others swim loosely around their head (see
    /// [`school`](super::school) for why the school stays irregular and out
    /// of step, and how it splits and rejoins). All sample the same tick
    /// snapshot.
    fn school_target(&self, k: &Kind, c: &Creature) -> Option<(i32, i32, i32)> {
        let all = &self.creatures.school[usize::from(c.species.0)];
        let lead = all.first()?;
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
        for m in all.iter().filter(|m| m.id != c.id) {
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
        // follower near an edge isn't led out of its band or the tank. A
        // place beyond a side wall folds back off it (see
        // `school::fold_to_walls`) rather than pinning there.
        tx = school::fold_to_walls(tx, k.w, self.width);
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

/// Distance at which an avoided cursor disturbs a creature, in fixed point.
pub(super) fn cursor_reach(k: &Kind) -> i64 {
    let lengths = if k.shy { 5 } else { 3 };
    i64::from(lengths * k.w * ONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curious_interest_lasts_twenty_seconds_in_every_fifty_at_each_fishs_own_phase() {
        let period = CURSOR_INTEREST_TICKS + CURSOR_COOLDOWN_TICKS;
        let mut starts = std::collections::BTreeSet::new();
        for id in 1..20 {
            let id = crate::world::creatures::CreatureId(id);
            let on = (0..period).filter(|&t| interested(7, id, t)).count();
            assert_eq!(on as u64, CURSOR_INTEREST_TICKS);
            let start =
                (0..period).find(|&t| interested(7, id, t) && !interested(7, id, t + period - 1));
            starts.insert(start);
        }
        // The fish don't all lose interest at once.
        assert!(starts.len() > 10, "{starts:?}");
    }
}
