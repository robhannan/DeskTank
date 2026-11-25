//! Hunting, fleeing and territory (SPEC §2.4 *Hunting and territory*).
//!
//! - **Sensing.** Every [`SENSE_EVERY`] ticks, at a tick picked from its
//!   id, a creature looks round a start-of-tick snapshot of the tank's
//!   creatures (`Creatures::near`, in id order). The snapshot is built only
//!   while the tank holds a hunter, a territorial or shy creature, or
//!   live food and an eater of it. The look costs at most n² /
//!   [`SENSE_EVERY`] distance checks a tick (n is at most the cap).
//! - **Hunting.** A hungry adult predator (`prey_cm`), outside easy mode,
//!   not in the net, not heading for food and rested picks the nearest
//!   creature it could eat (as `predation` would: no longer than its
//!   `prey_cm`) within [`HUNT_SENSE`] cells that isn't in cover, and gives
//!   chase: it stalks halfway between cruising and top speed, dashes at
//!   top speed within
//!   [`DASH_RANGE`] and shifts to its prey's band. It still strikes only
//!   as before (once a second while touching, 1 in `STRIKE_ODDS`). It
//!   gives up when the prey is eaten or gone, more than [`LOSE_RANGE`]
//!   away, in cover, after [`HUNT_TICKS`], or once it is fed, and then
//!   rests [`PURSUIT_REST`] before another chase.
//! - **Fleeing.** A creature that a hunting predator within [`FLEE_SENSE`]
//!   could eat takes fright for [`FEAR_TICKS`] (renewed while the threat
//!   lasts): it heads for the nearest plant cover within [`COVER_SENSE`]
//!   (the middle of five plant cells in a column, down a stem or blade, in
//!   one band), taking a band [`COVER_BANDS`] behind it (or in front, at the back of
//!   its range), or else straight away from the threat. Already in cover,
//!   it holds still. It flees at a burst speed: its top speed scaled by its
//!   size, at least its cruising speed, so fry are slow.
//! - **Startle.** A schooler that is calm and within one and a half of its
//!   species' full-grown lengths of a frightened schoolmate catches the fright
//!   (see `World::startle`): it takes `STARTLE_HOP` ticks less than that
//!   mate has left, so the wave fades as it passes through the school, and
//!   it runs the way the mate is heading. Each fish looks only every 3–8
//!   ticks (its own reaction time), so the fright ripples outward from the
//!   fish that saw the threat rather than reaching all at once. Calm tanks
//!   pay nothing: the look is skipped unless the tick's snapshot of the
//!   school (`Creatures::school`) holds a frightened fish.
//! - **Shyness.** A shy creature looks for cover when picking its wander
//!   target. A calm, awake one takes fright at an approaching swimmer at
//!   least half again as long as itself within three of its own grown
//!   lengths (at least eight cells before its boldness is applied).
//!   A nearby pour of something it doesn't eat also frightens it while
//!   awake. In both cases it flees to cover, or away from the disturbance.
//! - **Cover.** A creature is in cover when [`COVER_CELLS`] plant cells
//!   within [`COVER_BANDS`] of its band lie within [`COVER_REACH`] cells of
//!   its body. A predator neither sees nor strikes prey in cover.
//! - **Territory.** A territorial species with `territory_cm` claims a
//!   home where an adult first senses (its body centre's column, at the
//!   nearest row of its zone, so a palette fish's home is about where it
//!   was put) and from then on wanders
//!   within that radius of it. It drives out intruders: swimmers (surface
//!   and mid-water species) too big for it to eat, and adults of its own
//!   species and sex, inside its territory and within [`HUNT_SENSE`] of
//!   it. It chases at top speed until the intruder is [`TERRITORY_MARGIN`]
//!   cells past the edge, or for [`DRIVE_TICKS`], then turns for home and
//!   rests [`PURSUIT_REST`]. Blocked outside its territory (driven out of
//!   reach of home, behind a stand of plants say), it settles a new one
//!   where it is. A chase that is blocked is given up. A creature chased
//!   by one within
//!   [`FLEE_SENSE`] flees straight away from it. Territory is kept in easy
//!   mode too (no one is hurt); hunting is not.
//!
//! The quarry, the pursuit counter and fright are creature state (saved
//! and hashed); the home is the creature's `home`.

use super::live::{LIVE_LOSE, LIVE_SENSE};
use super::quirks::salt;
use super::{
    Creature, CreatureId, Kind, ONE, RETARGET_TICKS, TICKS_PER_SECOND, World, Zone, body_box,
    centre_of, dist, extent, lifecycle::grown_percent, to_i64, z_of, zone,
};

/// A creature senses its surroundings every this many ticks.
pub const SENSE_EVERY: u64 = 10;
/// How far a predator notices prey, or a territorial fish an intruder, in
/// cells (20 cm).
pub const HUNT_SENSE: i32 = 40;
/// A hunt is lost past this distance, in cells.
pub const LOSE_RANGE: i32 = 56;
/// Within this many cells a stalking predator dashes at top speed.
pub const DASH_RANGE: i32 = 12;
/// Longest hunt, in ticks (12 s).
#[allow(clippy::cast_possible_truncation)]
pub const HUNT_TICKS: u16 = 12 * TICKS_PER_SECOND as u16;
/// Longest drive after an intruder, in ticks (3 s).
#[allow(clippy::cast_possible_truncation)]
pub const DRIVE_TICKS: u16 = 3 * TICKS_PER_SECOND as u16;
/// Ticks a creature rests after a hunt or a drive before the next (5 s).
#[allow(clippy::cast_possible_truncation)]
pub const PURSUIT_REST: u16 = 5 * TICKS_PER_SECOND as u16;
/// How close a threat must be to frighten a creature, in cells.
pub const FLEE_SENSE: i32 = 14;
/// How long a fright lasts after the threat was last sensed, in ticks
/// (3 s).
#[allow(clippy::cast_possible_truncation)]
pub const FEAR_TICKS: u16 = 3 * TICKS_PER_SECOND as u16;
/// How far a frightened creature looks for cover, in cells.
pub const COVER_SENSE: i32 = 16;
/// Cells around a body a plant hides it from.
pub const COVER_REACH: usize = 2;
/// Bands from its own a plant cell hides a creature from.
pub const COVER_BANDS: u8 = 2;
/// Plant cells around a body that make cover.
pub const COVER_CELLS: u32 = 4;
/// Cells past its territory's edge a territorial fish drives an intruder.
pub const TERRITORY_MARGIN: i32 = 6;
/// How far a creature flees with no cover near, in cells.
const FLEE_RUN: i32 = 24;
/// How many of its own grown lengths away a shy fish senses a swimmer.
const SHY_SENSE: i32 = 3;
/// Minimum shy sensing range in cells, before applying boldness.
const SHY_MIN_SENSE: i32 = 8;
/// How far a startle reaches, in halves of the species' full-grown length.
const STARTLE_REACH: i32 = 3;
/// Ticks of fright a startle loses with each fish it passes through.
pub(super) const STARTLE_HOP: u16 = 4;
/// Ticks of a mate's heading the startled run from, behind the mate.
const STARTLE_BEHIND: i32 = 12;
/// A schooler's reaction time, 3–8 ticks (each fish draws its own).
const REACT_TICKS: (u64, u64) = (3, 8);

/// A creature as the others see it at the start of a tick.
#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_excessive_bools)] // independent facts
pub(super) struct Near {
    id: CreatureId,
    species: crate::species::SpeciesId,
    x: i32,
    y: i32,
    vx: i32,
    vy: i32,
    band: u8,
    /// Its body box (grown and turned), in cells.
    w: i32,
    h: i32,
    /// Its length as grown, in mm.
    length_mm: u32,
    /// The longest prey it would hunt now, in mm (0: it isn't hunting).
    hunts_mm: u32,
    /// What it is chasing.
    chasing: Option<CreatureId>,
    female: bool,
    adult: bool,
    /// A surface or mid-water species (one a territory is kept against).
    swimmer: bool,
    /// Held in the net or burrowed in the bed: no one chases it.
    held: bool,
    /// Live food (see [`super::live`]).
    live: bool,
}

impl World {
    /// Fills the sensing snapshot from `list` (the creatures in id order),
    /// or empties it when no creature there hunts, keeps a territory or is shy.
    pub(super) fn snapshot_near(&mut self, list: &[Creature]) {
        let mut near = core::mem::take(&mut self.creatures.near);
        near.clear();
        let easy = self.creatures.easy;
        let has_live = list.iter().any(|c| self.is_live(c.species));
        let active = list.iter().any(|c| {
            self.kind(c.species).is_some_and(|k| {
                (k.prey_mm > 0 && !easy) || k.territory > 0 || (has_live && k.eats_live) || k.shy
            })
        });
        if active {
            for c in list {
                let Some(k) = self.grown_kind(c) else {
                    continue;
                };
                let (w, h) = extent(&k, c.turn);
                near.push(Near {
                    id: c.id,
                    species: c.species,
                    x: c.x,
                    y: c.y,
                    vx: c.vx,
                    vy: c.vy,
                    band: c.band(),
                    w,
                    h,
                    length_mm: self.length_mm(c),
                    hunts_mm: if self.can_hunt(&k, c, false) {
                        k.prey_mm
                    } else {
                        0
                    },
                    chasing: c.quarry,
                    female: c.female,
                    adult: c.growth >= k.maturity,
                    swimmer: matches!(k.zone, Zone::Surface | Zone::Mid),
                    // In the net, or burrowed in the bed: out of reach.
                    held: c.held_until >= self.tick || self.sunk_depth(c) > 0,
                    live: k.live,
                });
            }
        }
        self.creatures.near = near;
    }

    /// One tick of `c`'s hunting, fright and territory, before it steers:
    /// counters run down, a quarry it should give up is dropped, and on its
    /// sensing tick it claims a territory, takes fright or picks a quarry.
    pub(super) fn pursue(&self, k: &Kind, c: &mut Creature) {
        c.fear = c.fear.saturating_sub(1);
        let sensing = (self.tick + u64::from(c.id.0)).is_multiple_of(SENSE_EVERY);
        if c.quarry.is_some() {
            c.pursuit = c.pursuit.saturating_add(1);
            if !self.keep_quarry(k, c, sensing) {
                self.give_up(k, c);
            }
        } else {
            c.pursuit = c.pursuit.saturating_sub(1);
        }
        if !sensing {
            return;
        }
        if k.territory > 0 && c.home.is_none() && c.growth >= k.maturity {
            self.claim_territory(k, c);
        }
        self.take_fright(k, c);
        if c.quarry.is_none() && c.pursuit == 0 && c.fear == 0 {
            c.quarry = self.find_prey(k, c).or_else(|| self.find_intruder(k, c));
        }
        // Head for the quarry's band, unless shifting past a plant.
        if let Some(n) = c.quarry.and_then(|q| self.near(q))
            && c.dodging == 0
        {
            c.tz = z_of(n.band.clamp(k.bands.0, k.bands.1));
        }
    }

    /// Where `c` steers chasing its quarry, and how fast, if it has one.
    pub(super) fn chase_target(&self, k: &Kind, c: &Creature) -> Option<(i32, i32, i32)> {
        let n = self.near(c.quarry?)?;
        let hunt = self.is_prey(k, c, n);
        let close = dist(n.x - c.x, n.y - c.y) <= i64::from(DASH_RANGE * ONE);
        let speed = if !hunt || close {
            k.top
        } else {
            i32::midpoint(k.cruise, k.top)
        };
        Some((n.x, n.y, speed))
    }

    /// Whether `c` could hunt now: a hungry adult predator, outside easy
    /// mode, not in the net; or, for `live` food, a hungry eater of some
    /// live food not in the net, at any age and in easy mode too.
    fn can_hunt(&self, k: &Kind, c: &Creature, live: bool) -> bool {
        let able = if live {
            k.eats_live
        } else {
            k.prey_mm > 0 && !self.creatures.easy && c.growth >= k.maturity
        };
        // Asleep, it hunts nothing (see `night`).
        able && c.hungry() && c.held_until < self.tick && !c.asleep
    }

    /// Whether `n` is prey to `c`, a creature of kind `k`: live food it
    /// eats, or a creature no longer than its `prey_cm`.
    fn is_prey(&self, k: &Kind, c: &Creature, n: &Near) -> bool {
        if n.live {
            Self::eats_live(k, self.length_mm(c), n.species, n.length_mm)
        } else {
            k.prey_mm > 0 && !self.creatures.easy && n.length_mm <= k.prey_mm
        }
    }

    fn near(&self, id: CreatureId) -> Option<&Near> {
        let near = &self.creatures.near;
        near.binary_search_by_key(&id, |n| n.id)
            .ok()
            .map(|i| &near[i])
    }

    /// Whether `c` keeps after its quarry this tick (cover is checked only
    /// on its sensing ticks).
    fn keep_quarry(&self, k: &Kind, c: &Creature, sensing: bool) -> bool {
        let Some(n) = c.quarry.and_then(|q| self.near(q)) else {
            return false;
        };
        if n.held {
            return false;
        }
        if self.is_prey(k, c, n) {
            let lose = if n.live { LIVE_LOSE } else { LOSE_RANGE };
            return self.can_hunt(k, c, n.live)
                && c.food.is_none()
                && c.pursuit <= self.quirks(c).ticks(salt::PERSIST, 60, 100, HUNT_TICKS)
                && dist(n.x - c.x, n.y - c.y) <= i64::from(lose * ONE)
                && !(sensing && !n.live && self.near_in_cover(n));
        }
        c.pursuit <= self.quirks(c).ticks(salt::PERSIST, 60, 100, DRIVE_TICKS)
            && in_territory(k, c, n.x, n.y, k.territory + TERRITORY_MARGIN)
    }

    /// `c` couldn't move: it gives up its chase, and a territorial fish
    /// outside its territory (driven out of reach of home, say behind a
    /// stand of plants) settles a new one where it is.
    pub(super) fn pursuit_blocked(&self, k: &Kind, c: &mut Creature) {
        if c.quarry.take().is_some() {
            c.pursuit = self.quirks(c).ticks(salt::PATIENCE, 60, 100, PURSUIT_REST);
        }
        if k.territory > 0 && c.home.is_some() && !in_territory(k, c, c.x, c.y, k.territory) {
            c.home = None;
            self.claim_territory(k, c);
        }
    }

    /// Claims `c`'s territory where it is: its column, at the nearest row
    /// of its zone (inside the grid).
    fn claim_territory(&self, k: &Kind, c: &mut Creature) {
        let (top, bottom) = zone(k, self.surface_row, self.height);
        // `max` before `min`: an empty zone (no water) can't panic.
        let row = usize::try_from(c.y >> 8)
            .unwrap_or(0)
            .max(top)
            .min(bottom.saturating_sub(1))
            .min(self.height.saturating_sub(1));
        c.home = Some((
            u32::try_from(c.x >> 8).unwrap_or(0),
            u32::try_from(row).unwrap_or(0),
        ));
    }

    /// Drops `c`'s quarry and rests; a territorial fish turns for home.
    fn give_up(&self, k: &Kind, c: &mut Creature) {
        let drove = c
            .quarry
            .and_then(|q| self.near(q))
            .is_none_or(|n| !self.is_prey(k, c, n));
        c.quarry = None;
        c.pursuit = self.quirks(c).ticks(salt::PATIENCE, 60, 100, PURSUIT_REST);
        if let Some((hx, hy)) = c.home
            && drove
            && k.territory > 0
        {
            (c.tx, c.ty) = (centre_of(i64::from(hx)), centre_of(i64::from(hy)));
            c.retarget = RETARGET_TICKS.1;
        }
    }

    /// The nearest prey `c` can see, if it hunts now: live food it eats
    /// within [`LIVE_SENSE`] (in cover or not), or prey within
    /// [`HUNT_SENSE`] that isn't in cover.
    fn find_prey(&self, k: &Kind, c: &Creature) -> Option<CreatureId> {
        let (hunt, live) = (self.can_hunt(k, c, false), self.can_hunt(k, c, true));
        if !(hunt || live) || c.food.is_some() {
            return None;
        }
        let mut best: Option<(i64, CreatureId)> = None;
        for n in &self.creatures.near {
            let able = if n.live { live } else { hunt };
            if n.id == c.id || n.held || !able || !self.is_prey(k, c, n) {
                continue;
            }
            let sense = i64::from(if n.live { LIVE_SENSE } else { HUNT_SENSE } * ONE);
            let d = dist(n.x - c.x, n.y - c.y);
            if d <= sense && best.is_none_or(|(bd, _)| d < bd) && (n.live || !self.near_in_cover(n))
            {
                best = Some((d, n.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// The nearest intruder in `c`'s territory, if it keeps one.
    fn find_intruder(&self, k: &Kind, c: &Creature) -> Option<CreatureId> {
        if k.territory == 0 || c.home.is_none() || c.growth < k.maturity {
            return None;
        }
        let mut best: Option<(i64, CreatureId)> = None;
        let sense = i64::from(HUNT_SENSE * ONE);
        for n in &self.creatures.near {
            let rival = n.species == c.species && n.adult && n.female == c.female;
            if n.id == c.id
                || n.held
                || n.live
                || !n.swimmer
                || n.length_mm <= k.prey_mm
                || (n.species == c.species && !rival)
                || !in_territory(k, c, n.x, n.y, k.territory)
            {
                continue;
            }
            let d = dist(n.x - c.x, n.y - c.y);
            if d <= sense && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, n.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// On a sensing tick: takes fright at the nearest hunter that could eat
    /// `c`, a chaser of it, or a large swimmer approaching a shy creature.
    pub(super) fn take_fright(&self, k: &Kind, c: &mut Creature) {
        if self.creatures.near.is_empty() {
            return;
        }
        let length = self.length_mm(c);
        // And lets a hunter closer: 80–100% of the range. A fish chasing it
        // off its territory it notices at the full range.
        let bold = i64::try_from(self.quirks(c).percent(salt::BOLD, 60, 100)).unwrap_or(100);
        let full = i64::from(FLEE_SENSE * ONE);
        let bold_scale = 50 + bold / 2;
        let bold_sense = full * bold_scale / 100;
        let shy = k.shy && !c.asleep && c.fear == 0;
        let shy_sense = i64::from((SHY_SENSE * k.w).max(SHY_MIN_SENSE) * ONE) * bold_scale / 100;
        let mut threat: Option<(i64, i32, i32, bool)> = None;
        for n in &self.creatures.near {
            if n.id == c.id || n.held {
                continue;
            }
            // Live food squirms away from a chaser only, seeking no cover.
            let hunter = !k.live && n.hunts_mm >= length && n.hunts_mm > 0;
            let chaser = n.chasing == Some(c.id);
            let bigger =
                shy && n.swimmer && !n.live && u64::from(n.length_mm) * 2 >= u64::from(length) * 3;
            if !(hunter || chaser || bigger) {
                continue;
            }
            let (dx, dy) = (c.x - n.x, c.y - n.y);
            let d = dist(dx, dy);
            let approaching = bigger
                && d <= shy_sense
                && i64::from(n.vx) * i64::from(dx) + i64::from(n.vy) * i64::from(dy) > 0;
            let under_threat = (hunter || chaser) && d <= if chaser { full } else { bold_sense };
            if (approaching || under_threat) && threat.is_none_or(|(td, ..)| d < td) {
                threat = Some((d, n.x, n.y, approaching || hunter));
            }
        }
        let Some((_, fx, fy, seek_cover)) = threat else {
            return;
        };
        self.frighten(k, c, fx, fy, seek_cover);
    }

    /// Starts a fright, seeking cover if appropriate or running from its source.
    pub(super) fn frighten(&self, k: &Kind, c: &mut Creature, fx: i32, fy: i32, seek_cover: bool) {
        // A bold one gets over it sooner.
        c.fear = self.quirks(c).ticks(salt::BOLD, 60, 100, FEAR_TICKS);
        if seek_cover {
            let (w, h) = extent(k, c.turn);
            if self.in_cover(w, h, c.x, c.y, c.band()) {
                (c.tx, c.ty) = (c.x, c.y);
                return;
            }
            if let Some((x, y, band)) = self.find_cover(k, c) {
                (c.tx, c.ty) = (x, y);
                if c.dodging == 0 {
                    c.tz = z_of(band);
                }
                return;
            }
        }
        self.run_from(k, c, fx, fy);
    }

    /// A calm schooler within reach of a frightened schoolmate takes fright
    /// from it: [`STARTLE_HOP`] ticks less, running away from a point
    /// behind the mate along its heading (the way it is going), or from
    /// the mate itself while it is still. It looks every 3–8 ticks, from
    /// its own reaction time. The start-of-tick snapshot makes the wave
    /// independent of update order.
    pub(super) fn startle(&self, k: &Kind, c: &mut Creature) {
        if !self.creatures.scared || !k.schooling || c.fear > 0 {
            return;
        }
        let delay = self
            .quirks(c)
            .percent(salt::REACT, REACT_TICKS.0, REACT_TICKS.1);
        if !(self.tick + u64::from(c.id.0)).is_multiple_of(delay) {
            return;
        }
        let grown = self.kind(c.species).map_or(k.w, |g| g.size_of(c.female).0);
        let reach = i64::from(STARTLE_REACH * grown * ONE / 2);
        let mut loudest: Option<&super::Schoolmate> = None;
        for m in &self.creatures.school[usize::from(c.species.0)] {
            if m.id != c.id
                && m.fear > STARTLE_HOP
                && loudest.is_none_or(|l| m.fear > l.fear)
                && dist(m.x - c.x, m.y - c.y) <= reach
            {
                loudest = Some(m);
            }
        }
        let Some(m) = loudest else {
            return;
        };
        c.fear = m.fear - STARTLE_HOP;
        self.run_from(
            k,
            c,
            m.x - m.vx * STARTLE_BEHIND,
            m.y - m.vy * STARTLE_BEHIND,
        );
    }

    /// Sets `c`'s target [`FLEE_RUN`] cells straight away from `(fx, fy)`,
    /// kept inside the water.
    fn run_from(&self, k: &Kind, c: &mut Creature, fx: i32, fy: i32) {
        let (mut dx, dy) = (c.x - fx, c.y - fy);
        if (dx, dy) == (0, 0) {
            dx = if c.facing_left { -ONE } else { ONE };
        }
        let d = dist(dx, dy).max(1);
        let run = i64::from(FLEE_RUN * ONE);
        let step = |v: i32| i32::try_from(i64::from(v) * run / d).unwrap_or(0);
        let (hw, hh) = (k.w / 2 + 1, k.h / 2 + 1);
        let clamp = |v: i32, lo: i64, hi: i64| {
            let (lo, hi) = (centre_of(lo), centre_of(hi.max(lo)));
            v.clamp(lo, hi)
        };
        c.tx = clamp(
            c.x + step(dx),
            i64::from(hw),
            to_i64(self.width) - i64::from(hw) - 1,
        );
        c.ty = clamp(
            c.y + step(dy),
            to_i64(self.surface_row) + i64::from(hh),
            to_i64(self.height) - i64::from(hh) - 1,
        );
    }

    /// The nearest plant cell within [`COVER_SENSE`] of `c` in the middle
    /// of a run of five down a stem or blade in one band (so cover by
    /// itself; scanned in rows, the first of equals), as a target centre
    /// and the band to hide in beside it.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn find_cover(&self, k: &Kind, c: &Creature) -> Option<(i32, i32, u8)> {
        let (cx, cy) = (i64::from(c.x >> 8), i64::from(c.y >> 8));
        let r = i64::from(COVER_SENSE);
        let x0 = usize::try_from((cx - r).max(0)).unwrap_or(0);
        let x1 = usize::try_from((cx + r + 1).min(to_i64(self.width))).unwrap_or(0);
        let y0 = usize::try_from((cy - r).max(0)).unwrap_or(0);
        let y1 = usize::try_from((cy + r + 1).min(to_i64(self.height))).unwrap_or(0);
        let mut memo = None;
        let mut best: Option<(i64, usize, u8)> = None;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * self.width + x;
                if self.plants.part(self.cells[i].material).is_none() {
                    continue;
                }
                let d = (to_i64(x) - cx).pow(2) + (to_i64(y) - cy).pow(2);
                if best.is_some_and(|(bd, ..)| d >= bd) {
                    continue;
                }
                let Some(p) = self.plant_band_with(i, &mut memo) else {
                    continue;
                };
                // The middle of a run of plant cells down a stem or blade
                // in one band: cover by itself.
                let mut run = |dy: usize| {
                    (y >= dy && y + dy < self.height)
                        && [i - dy * self.width, i + dy * self.width]
                            .into_iter()
                            .all(|j| {
                                self.plants.part(self.cells[j].material).is_some()
                                    && self.plant_band_with(j, &mut memo) == Some(p)
                            })
                };
                if !(run(1) && run(2)) {
                    continue;
                }
                let allowed = k.bands.0..=k.bands.1;
                let behind = p.checked_add(COVER_BANDS).filter(|b| allowed.contains(b));
                let front = p.checked_sub(COVER_BANDS).filter(|b| allowed.contains(b));
                if let Some(band) = behind.or(front) {
                    best = Some((d, i, band));
                }
            }
        }
        best.map(|(_, i, band)| {
            (
                centre_of(to_i64(i % self.width)),
                centre_of(to_i64(i / self.width)),
                band,
            )
        })
    }

    /// Whether `c` is in cover, at its grown size.
    pub(super) fn creature_in_cover(&self, c: &Creature) -> bool {
        self.grown_kind(c).is_some_and(|k| {
            let (w, h) = extent(&k, c.turn);
            self.in_cover(w, h, c.x, c.y, c.band())
        })
    }

    fn near_in_cover(&self, n: &Near) -> bool {
        self.in_cover(n.w, n.h, n.x, n.y, n.band)
    }

    /// Whether a `w × h` body at `(x, y)` in `band` is in cover (see the
    /// module docs).
    #[allow(clippy::many_single_char_names)] // cell maths
    pub(super) fn in_cover(&self, w: i32, h: i32, x: i32, y: i32, band: u8) -> bool {
        let Some(b) = body_box(w, h, x, y, self.width, self.height) else {
            return false;
        };
        let (x0, y0) = (
            b.x0.saturating_sub(COVER_REACH),
            b.y0.saturating_sub(COVER_REACH),
        );
        let (x1, y1) = (
            (b.x1 + COVER_REACH).min(self.width),
            (b.y1 + COVER_REACH).min(self.height),
        );
        let mut memo = None;
        let mut cells = 0;
        for y in y0..y1 {
            for i in y * self.width + x0..y * self.width + x1 {
                if self.plants.part(self.cells[i].material).is_some()
                    && self
                        .plant_band_with(i, &mut memo)
                        .is_some_and(|p| p.abs_diff(band) <= COVER_BANDS)
                {
                    cells += 1;
                    if cells >= COVER_CELLS {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Whether `(x, y)` (fixed point) is within `radius` cells of `c`'s home.
fn in_territory(k: &Kind, c: &Creature, x: i32, y: i32, radius: i32) -> bool {
    let Some((hx, hy)) = c.home else {
        return false;
    };
    k.territory > 0
        && dist(x - centre_of(i64::from(hx)), y - centre_of(i64::from(hy)))
            <= i64::from(radius * ONE)
}

/// Where `c` runs while frightened, and how fast: its burst speed.
pub(super) fn flee_target(k: &Kind, c: &Creature) -> Option<(i32, i32, i32)> {
    if c.fear == 0 {
        return None;
    }
    let percent = i32::try_from(grown_percent(c.growth, k.maturity)).unwrap_or(100);
    let burst = (k.top * percent / 100).max(k.cruise);
    let ty = if k.zone == Zone::Bottom { c.y } else { c.ty };
    Some((c.tx, ty, burst))
}

/// Whether saved pursuit state is as [`World::pursue`] keeps it: a quarry
/// is another creature in the tank (`ids`), chased at most the longest
/// chase; without one it rests at most [`PURSUIT_REST`]; fright lasts at
/// most [`FEAR_TICKS`].
pub(super) fn fits(id: u32, quarry: Option<u32>, pursuit: u16, fear: u16, ids: &[u32]) -> bool {
    let limit = if quarry.is_some() {
        HUNT_TICKS.max(DRIVE_TICKS)
    } else {
        PURSUIT_REST
    };
    quarry.is_none_or(|q| q != id && ids.contains(&q)) && pursuit <= limit && fear <= FEAR_TICKS
}
