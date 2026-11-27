//! The screensaver camera tour (D29): a deterministic director that picks
//! the camera's view over time, to give a screensaver something that is
//! never still.
//!
//! The director is pure logic. The caller seeds it and advances it by steps
//! of world time (no wall clock, no unseeded randomness), so the same seed,
//! world and steps always give the same views. It keeps its state in grid
//! terms (a centre in cells and a zoom over the full-tank view), so a resize
//! of the target only changes how that state is read: it is turned into a
//! [`ViewRect`] with the target's [`Stage`] on demand.
//!
//! The tour runs in four phases:
//!
//! - **Focus**: the view holds a zoom of 2–3× (from the creature's sprite
//!   size, never below the zoom that covers the target) on one live
//!   creature, following it through a critically damped spring. A focus
//!   lasts 20–40 s, or less if the creature dies or is netted out.
//! - **Glide**: 6–10 s between two views, with smootherstep on both pan and
//!   zoom. The end of a glide to a creature follows the creature, so the
//!   glide lands on it however it swam meanwhile.
//! - **Hold**: after every 3–5 focuses the tour glides out to the full-tank
//!   view and holds it for about 20 s.
//! - **Drift**: with no creatures the view wanders slowly round a 1.0–1.15×
//!   ellipse, so the picture still moves.
//!
//! While a focus has settled it also says which page of the inspect card to
//! show (see [`Tour::card`]): live, then species, every 12 s.

use std::collections::VecDeque;
use std::f32::consts::TAU;

use desktank_sim::{Creature, CreatureId, World};

use crate::camera::{MAX_ZOOM, ViewRect};
use crate::inspect::InspectPage;
use crate::layout::Layout;

/// How long a focus on one creature lasts, in seconds, at the shortest and
/// longest.
const FOCUS_SECONDS: (f32, f32) = (20.0, 40.0);
/// How long a glide between views takes.
const GLIDE_SECONDS: (f32, f32) = (6.0, 10.0);
/// How many focuses come before a pull-back to the full tank, fewest and most.
const FOCUSES_PER_PULL_BACK: (u32, u32) = (3, 5);
/// How long the full-tank view is held after a pull-back.
const HOLD_SECONDS: f32 = 20.0;
/// The time constant of the follow spring, in seconds.
const FOLLOW_SECONDS: f32 = 2.0;
/// How long each page of the tour card is shown.
const CARD_PAGE_SECONDS: f32 = 12.0;
/// How many of the last creatures a new focus avoids.
const RECENT: usize = 3;
/// The zoom range of a focus.
const FOCUS_ZOOM: (f32, f32) = (2.0, 3.0);
/// A creature's zoom is this many cells over its longest side, within
/// [`FOCUS_ZOOM`]: smaller creatures are closed in on more.
const FOCUS_CELLS: f32 = 40.0;
/// How much the empty-tank view zooms over the full-tank view (zoom 1.0 up
/// to 1.15).
const DRIFT_ZOOM_SPAN: f32 = 0.15;
/// Seconds for the empty-tank view to go once round its ellipse.
const DRIFT_PAN_SECONDS: f32 = 150.0;
/// Seconds for it to zoom in and out once.
const DRIFT_ZOOM_SECONDS: f32 = 100.0;
/// The drift clock wraps at a whole number of both periods.
const DRIFT_WRAP_SECONDS: f32 = 300.0;

/// Where the tour is shown: the tank's layout in the target and the target's
/// size in pixels, which fix what a zoom and a centre mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stage {
    /// Where the grid sits in the target.
    pub(crate) layout: Layout,
    /// The target's size in pixels.
    pub(crate) target: (u32, u32),
}

impl Stage {
    fn grid(&self) -> (f32, f32) {
        #[allow(clippy::cast_precision_loss)] // grid cells
        (self.layout.grid.0 as f32, self.layout.grid.1 as f32)
    }

    fn grid_centre(&self) -> (f32, f32) {
        let (w, h) = self.grid();
        (w / 2.0, h / 2.0)
    }

    /// The zoom from which the view lies inside the tank on both axes.
    fn min_zoom(&self) -> f32 {
        ViewRect::min_cover_zoom(&self.layout, self.target)
    }

    fn fit_pose(&self) -> Pose {
        Pose {
            centre: self.grid_centre(),
            zoom: 1.0,
        }
    }

    /// The view of `pose`: clamped inside the tank, whatever the centre.
    pub(crate) fn view(&self, pose: Pose) -> ViewRect {
        ViewRect::cover(&self.layout, self.target, pose.centre, pose.zoom)
    }

    /// The view of the whole tank, letterboxed if the target's shape
    /// differs from the tank's.
    pub(crate) fn fit(&self) -> ViewRect {
        ViewRect::fit(&self.layout, self.target)
    }
}

/// Which page of whose inspect card the tour shows right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TourCard {
    /// The creature in focus.
    pub creature: CreatureId,
    /// The page to show.
    pub page: InspectPage,
}

/// What the tour is doing, without the detail.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TourPhase {
    /// Wandering with no creature to show.
    Drift,
    /// Moving between two views.
    Glide,
    /// Following a creature.
    Focus,
    /// Holding the full-tank view.
    Hold,
}

/// A view in grid terms: where its middle is and how far it is zoomed in
/// over the full-tank view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pose {
    /// The centre, in cells.
    pub(crate) centre: (f32, f32),
    /// The zoom over the full-tank view (1 is the full tank).
    pub(crate) zoom: f32,
}

/// Where a glide is going.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Dest {
    /// A focus on a creature, at the zoom its size wants (before the
    /// target's minimum is applied).
    Creature { id: CreatureId, zoom: f32 },
    /// The full tank.
    Fit,
    /// The empty-tank drift.
    Drift,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Drift,
    Glide {
        from: Pose,
        to: Dest,
        t: f32,
        dur: f32,
    },
    Focus {
        id: CreatureId,
        zoom: f32,
        /// Seconds the focus has settled.
        t: f32,
        /// Seconds it lasts.
        dur: f32,
    },
    Hold {
        left: f32,
    },
}

/// A critically damped spring on a point, with its velocity.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Spring {
    pos: (f32, f32),
    vel: (f32, f32),
}

impl Spring {
    fn at(pos: (f32, f32)) -> Self {
        Self {
            pos,
            vel: (0.0, 0.0),
        }
    }

    /// Moves `dt` seconds towards `target`, exactly (the closed form for a
    /// target held over the step), so any step size is stable.
    fn step(&mut self, target: (f32, f32), dt: f32) {
        axis(&mut self.pos.0, &mut self.vel.0, target.0, dt);
        axis(&mut self.pos.1, &mut self.vel.1, target.1, dt);
    }
}

fn axis(pos: &mut f32, vel: &mut f32, target: f32, dt: f32) {
    let rate = 1.0 / FOLLOW_SECONDS;
    let off = *pos - target;
    let carry = *vel + rate * off;
    let decay = (-rate * dt).exp();
    *pos = target + (off + carry * dt) * decay;
    *vel = (*vel - rate * carry * dt) * decay;
}

/// 0 to 1 with zero slope at both ends (`6t⁵ − 15t⁴ + 10t³`).
fn smootherstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// The zoom a creature of body size `(w, h)` cells wants: about 2–3×, more
/// for the smaller ones.
fn zoom_for_size((w, h): (usize, usize)) -> f32 {
    #[allow(clippy::cast_precision_loss)] // a few cells
    let longest = w.max(h).max(1) as f32;
    (FOCUS_CELLS / longest).clamp(FOCUS_ZOOM.0, FOCUS_ZOOM.1)
}

/// A small seeded generator (`SplitMix64`), so the tour needs no dependency
/// and gives the same picks on every platform.
#[derive(Clone, Copy, Debug)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In `[0, 1)`.
    fn unit(&mut self) -> f32 {
        #[allow(clippy::cast_precision_loss)] // 24 bits, exact
        let bits = (self.next() >> 40) as f32;
        bits / 16_777_216.0
    }

    fn range(&mut self, (lo, hi): (f32, f32)) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    /// In `0..n` (`n` above 0).
    fn below(&mut self, n: usize) -> usize {
        let n = n.max(1) as u64;
        // The high half, scaled: unbiased enough for a few creatures.
        usize::try_from(((self.next() >> 32) * n) >> 32).unwrap_or(0)
    }
}

/// The director.
#[derive(Clone, Debug)]
pub(crate) struct Tour {
    rng: Rng,
    state: State,
    /// Where the view is (`None` before the first step).
    pose: Option<Pose>,
    /// Tracks the creature in focus (or being glided to).
    follow: Spring,
    /// The last few creatures focused, oldest first.
    recent: VecDeque<CreatureId>,
    /// Focuses since the last pull-back.
    focuses: u32,
    /// How many make the next pull-back.
    pull_after: u32,
    /// The empty-tank drift's clock, in seconds.
    drift_t: f32,
}

impl Tour {
    /// A tour that makes its choices from `seed`.
    pub(crate) fn new(seed: u64) -> Self {
        let mut rng = Rng(seed);
        let pull_after = Self::roll_pull_after(&mut rng);
        let drift_t = rng.unit() * DRIFT_WRAP_SECONDS;
        Self {
            rng,
            state: State::Drift,
            pose: None,
            follow: Spring::default(),
            recent: VecDeque::with_capacity(RECENT + 1),
            focuses: 0,
            pull_after,
            drift_t,
        }
    }

    fn roll_pull_after(rng: &mut Rng) -> u32 {
        let (lo, hi) = FOCUSES_PER_PULL_BACK;
        lo + u32::try_from(rng.below((hi - lo + 1) as usize)).unwrap_or(0)
    }

    /// What the tour is doing.
    #[cfg(test)]
    pub(crate) fn phase(&self) -> TourPhase {
        match self.state {
            State::Drift => TourPhase::Drift,
            State::Glide { .. } => TourPhase::Glide,
            State::Focus { .. } => TourPhase::Focus,
            State::Hold { .. } => TourPhase::Hold,
        }
    }

    /// The creature in focus, or being glided to.
    #[cfg(test)]
    pub(crate) fn target(&self) -> Option<CreatureId> {
        match self.state {
            State::Focus { id, .. }
            | State::Glide {
                to: Dest::Creature { id, .. },
                ..
            } => Some(id),
            _ => None,
        }
    }

    /// The tour card to show: the creature in focus once a glide has
    /// settled on it, turning from its live page to its species page and
    /// back every 12 s. `None` during glides, holds and drifts.
    pub(crate) fn card(&self) -> Option<TourCard> {
        let State::Focus { id, t, .. } = self.state else {
            return None;
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // small, non-negative
        let turns = (t / CARD_PAGE_SECONDS) as u32;
        Some(TourCard {
            creature: id,
            page: if turns.is_multiple_of(2) {
                InspectPage::Live
            } else {
                InspectPage::Species
            },
        })
    }

    /// The view to show on `stage`: before the first step, the full tank.
    pub(crate) fn view(&self, stage: &Stage) -> ViewRect {
        self.pose
            .map_or_else(|| stage.fit(), |pose| stage.view(pose))
    }

    /// Advances by `dt` seconds of world time (a step of about 50 ms is
    /// the simulation's own; any size works) on `stage`, with `world`'s
    /// creatures to choose from.
    pub(crate) fn advance(&mut self, dt: f32, world: &World, stage: &Stage) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.drift_t = (self.drift_t + dt) % DRIFT_WRAP_SECONDS;
        let here = self.pose.unwrap_or_else(|| self.drift_pose(stage));
        let pose = match self.state {
            State::Drift => match self.pick(world) {
                Some(to) => {
                    self.begin_glide(to, here);
                    here
                }
                None => self.drift_pose(stage),
            },
            State::Glide { from, to, t, dur } => {
                self.glide(world, stage, here, (from, to, t, dur), dt)
            }
            State::Focus { id, zoom, t, dur } => {
                self.focus(world, stage, here, (id, zoom, t, dur), dt)
            }
            State::Hold { left } => {
                let left = left - dt;
                if left <= 0.0 {
                    let to = self.pick(world).unwrap_or(Dest::Drift);
                    self.begin_glide(to, stage.fit_pose());
                } else {
                    self.state = State::Hold { left };
                }
                stage.fit_pose()
            }
        };
        self.pose = Some(pose);
    }

    fn glide(
        &mut self,
        world: &World,
        stage: &Stage,
        here: Pose,
        (from, to, t, dur): (Pose, Dest, f32, f32),
        dt: f32,
    ) -> Pose {
        // A creature that left mid-glide: glide on from where the view is
        // to the next thing, without a jump.
        if let Dest::Creature { id, .. } = to
            && world.creature(id).is_none()
        {
            let next = self.pick(world).unwrap_or(Dest::Drift);
            self.begin_glide(next, here);
            return here;
        }
        let t = t + dt;
        let goal = self.goal(world, stage, to, dt);
        let pose = blend(from, goal, smootherstep(t / dur));
        if t >= dur {
            self.state = match to {
                Dest::Creature { id, zoom } => State::Focus {
                    id,
                    zoom,
                    t: 0.0,
                    dur: self.rng.range(FOCUS_SECONDS),
                },
                Dest::Fit => State::Hold { left: HOLD_SECONDS },
                Dest::Drift => State::Drift,
            };
        } else {
            self.state = State::Glide { from, to, t, dur };
        }
        pose
    }

    fn focus(
        &mut self,
        world: &World,
        stage: &Stage,
        here: Pose,
        (id, zoom, t, dur): (CreatureId, f32, f32, f32),
        dt: f32,
    ) -> Pose {
        let Some(creature) = world.creature(id) else {
            self.end_focus(world, here);
            return here;
        };
        let t = t + dt;
        if t >= dur {
            self.end_focus(world, here);
            return here;
        }
        self.state = State::Focus { id, zoom, t, dur };
        self.follow.step(clamp_in(creature.centre(), stage), dt);
        Pose {
            centre: self.follow.pos,
            zoom: focus_zoom(zoom, stage),
        }
    }

    /// Moves on from a focus: to the full tank every few focuses, else to
    /// the next creature, else to the drift.
    fn end_focus(&mut self, world: &World, here: Pose) {
        self.focuses += 1;
        let to = if self.focuses >= self.pull_after {
            self.focuses = 0;
            self.pull_after = Self::roll_pull_after(&mut self.rng);
            Dest::Fit
        } else {
            self.pick(world).unwrap_or(Dest::Drift)
        };
        self.begin_glide(to, here);
    }

    fn begin_glide(&mut self, to: Dest, from: Pose) {
        self.state = State::Glide {
            from,
            to,
            t: 0.0,
            dur: self.rng.range(GLIDE_SECONDS),
        };
    }

    /// Where a glide to `to` is heading now: it follows a creature, so it
    /// lands on it however it swam.
    fn goal(&mut self, world: &World, stage: &Stage, to: Dest, dt: f32) -> Pose {
        match to {
            Dest::Creature { id, zoom } => {
                if let Some(c) = world.creature(id) {
                    self.follow.step(clamp_in(c.centre(), stage), dt);
                }
                Pose {
                    centre: self.follow.pos,
                    zoom: focus_zoom(zoom, stage),
                }
            }
            Dest::Fit => stage.fit_pose(),
            Dest::Drift => self.drift_pose(stage),
        }
    }

    /// The next creature to show: a live one that isn't among the last few
    /// (or, with so few creatures that none is left, not the last one; or,
    /// with only one, that one). `None` in an empty tank. Starts the
    /// follow spring on it.
    fn pick(&mut self, world: &World) -> Option<Dest> {
        let live = world.creatures();
        let not_recent = |c: &&Creature| !self.recent.contains(&c.id());
        let mut pool: Vec<&Creature> = live.iter().filter(not_recent).collect();
        if pool.is_empty() {
            let last = self.recent.back().copied();
            pool = live.iter().filter(|c| Some(c.id()) != last).collect();
        }
        if pool.is_empty() {
            pool = live.iter().collect();
        }
        if pool.is_empty() {
            return None;
        }
        let c = pool[self.rng.below(pool.len())];
        self.recent.push_back(c.id());
        while self.recent.len() > RECENT {
            self.recent.pop_front();
        }
        // The spring starts on the creature, not on the camera: a glide
        // blends from the view it began at to wherever the creature is.
        self.follow = Spring::at(c.centre());
        Some(Dest::Creature {
            id: c.id(),
            zoom: zoom_for_size(world.creature_size(c)),
        })
    }

    /// The view the empty tank wanders through: zoom 1× to 1.15× over the
    /// fit (or over the least zoom that covers the target, so no letterbox
    /// stays still on a target of another shape), panning round an
    /// ellipse as wide as the zoom leaves room for.
    fn drift_pose(&self, stage: &Stage) -> Pose {
        let min = stage.min_zoom();
        let breathe = 0.5 * (1.0 - (TAU * self.drift_t / DRIFT_ZOOM_SECONDS).cos());
        let zoom = (min * (1.0 + DRIFT_ZOOM_SPAN * breathe)).min(MAX_ZOOM);
        let fit = stage.fit();
        let (gw, gh) = stage.grid();
        let room = |grid: f32, view: f32| ((grid - view / zoom) / 2.0).max(0.0);
        let (rx, ry) = (room(gw, fit.w), room(gh, fit.h));
        let a = TAU * self.drift_t / DRIFT_PAN_SECONDS;
        let (cx, cy) = stage.grid_centre();
        Pose {
            centre: (cx + rx * a.cos(), cy + ry * a.sin()),
            zoom,
        }
    }
}

/// `want` as a zoom of `stage`: never below the one that covers the target
/// (so no letterbox shows) nor above [`MAX_ZOOM`].
fn focus_zoom(want: f32, stage: &Stage) -> f32 {
    want.max(stage.min_zoom()).clamp(1.0, MAX_ZOOM)
}

/// `p` held inside the grid.
fn clamp_in(p: (f32, f32), stage: &Stage) -> (f32, f32) {
    let (w, h) = stage.grid();
    (p.0.clamp(0.0, w), p.1.clamp(0.0, h))
}

/// The pose `s` of the way (0 to 1) from `a` to `b`: the centre by a
/// straight line, the zoom geometrically (the same zoom factor per second).
fn blend(a: Pose, b: Pose, s: f32) -> Pose {
    let lerp = |a: f32, b: f32| a + (b - a) * s;
    Pose {
        centre: (lerp(a.centre.0, b.centre.0), lerp(a.centre.1, b.centre.1)),
        zoom: a.zoom * (b.zoom / a.zoom).powf(s),
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
mod tests {
    use super::*;
    use desktank_sim::{MaterialTable, WorldConfig};

    const STEP: f32 = 0.05;
    /// A 160×90 tank in a 16:9 target (scale 8, no letterbox).
    const WIDE_16_9: (u32, u32) = (1280, 720);
    /// A 21:9-ish target, letterboxed at the sides (min cover zoom 1.3125).
    const WIDE_21_9: (u32, u32) = (1680, 720);

    fn world(species: &[&str]) -> World {
        let mut w = World::new(
            WorldConfig {
                width: 160,
                height: 90,
                seed: 3,
                water_level: 1.0,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        for (i, name) in species.iter().enumerate() {
            let sp = w.species().id(name).unwrap();
            let i = i64::try_from(i).unwrap();
            w.add_creature(sp, 20 + 17 * (i % 8), 20 + 13 * (i % 4))
                .unwrap();
        }
        w
    }

    const FISH: [&str; 8] = [
        "neon_tetra",
        "guppy",
        "angelfish",
        "common_goldfish",
        "betta",
        "harlequin_rasbora",
        "honey_gourami",
        "pea_puffer",
    ];

    fn stage(target: (u32, u32)) -> Stage {
        Stage {
            layout: Layout::fit((160, 90), target),
            target,
        }
    }

    fn zoom_of(view: ViewRect, stage: &Stage) -> f32 {
        stage.fit().w / view.w
    }

    fn inside(v: ViewRect) -> bool {
        let e = 1e-3;
        v.x >= -e && v.y >= -e && v.x + v.w <= 160.0 + e && v.y + v.h <= 90.0 + e
    }

    /// Everything a test might compare, per step.
    #[derive(Clone, Debug, PartialEq)]
    struct Frame {
        view: ViewRect,
        phase: TourPhase,
        card: Option<TourCard>,
        target: Option<CreatureId>,
    }

    fn frame(tour: &Tour, stage: &Stage) -> Frame {
        Frame {
            view: tour.view(stage),
            phase: tour.phase(),
            card: tour.card(),
            target: tour.target(),
        }
    }

    /// Runs `seconds` of world time, ticking the world as it goes (so the
    /// creatures swim), and records every step.
    fn run(tour: &mut Tour, w: &mut World, stage: &Stage, seconds: f32) -> Vec<Frame> {
        let steps = (seconds / STEP).round() as u32;
        (0..steps)
            .map(|_| {
                w.tick();
                tour.advance(STEP, w, stage);
                frame(tour, stage)
            })
            .collect()
    }

    /// As [`run`], with the world standing still (no creature dies, is eaten
    /// or swims), so every time is the tour's own.
    fn run_still(tour: &mut Tour, w: &World, stage: &Stage, seconds: f32) -> Vec<Frame> {
        let steps = (seconds / STEP).round() as u32;
        (0..steps)
            .map(|_| {
                tour.advance(STEP, w, stage);
                frame(tour, stage)
            })
            .collect()
    }

    fn jump(a: ViewRect, b: ViewRect) -> (f32, f32) {
        let centre = |v: ViewRect| (v.x + v.w / 2.0, v.y + v.h / 2.0);
        let (ca, cb) = (centre(a), centre(b));
        (
            (ca.0 - cb.0).abs().max((ca.1 - cb.1).abs()),
            (a.w / b.w).max(b.w / a.w),
        )
    }

    #[test]
    fn the_same_seed_and_steps_give_the_same_views() {
        let s = stage(WIDE_16_9);
        let (mut w1, mut w2) = (world(&FISH), world(&FISH));
        let (mut a, mut b, mut c) = (Tour::new(5), Tour::new(5), Tour::new(6));
        let fa = run(&mut a, &mut w1, &s, 400.0);
        let fb = run(&mut b, &mut w2, &s, 400.0);
        assert_eq!(fa, fb);
        let mut w3 = world(&FISH);
        assert_ne!(
            fa,
            run(&mut c, &mut w3, &s, 400.0),
            "another seed, another tour"
        );
    }

    #[test]
    fn the_step_size_does_not_change_where_a_glide_goes() {
        // A static world, so only the step size differs: the same seed holds
        // the same picks and times, and the views agree to a small fraction.
        let s = stage(WIDE_16_9);
        let w = world(&FISH);
        let (mut fine, mut coarse) = (Tour::new(9), Tour::new(9));
        for _ in 0..400 {
            fine.advance(0.05, &w, &s);
        }
        for _ in 0..40 {
            coarse.advance(0.5, &w, &s);
        }
        let (a, b) = (fine.view(&s), coarse.view(&s));
        assert_eq!(fine.target(), coarse.target());
        assert!(
            (a.x - b.x).abs() < 1.0 && (a.w - b.w).abs() < 0.5,
            "{a:?} {b:?}"
        );
    }

    #[test]
    fn a_critically_damped_spring_settles_without_overshoot() {
        let mut s = Spring::at((0.0, 10.0));
        let mut last = 0.0;
        for _ in 0..2000 {
            s.step((10.0, 0.0), 0.01);
            assert!(
                s.pos.0 >= last && s.pos.0 <= 10.0,
                "no overshoot: {}",
                s.pos.0
            );
            assert!(s.pos.1 >= 0.0 && s.pos.1 <= 10.0);
            last = s.pos.0;
        }
        assert!((s.pos.0 - 10.0).abs() < 1e-2, "settled after 20 s");
        // One long step is the same as many short ones.
        let (mut one, mut many) = (Spring::at((0.0, 0.0)), Spring::at((0.0, 0.0)));
        one.step((6.0, 3.0), 1.0);
        for _ in 0..100 {
            many.step((6.0, 3.0), 0.01);
        }
        assert!((one.pos.0 - many.pos.0).abs() < 1e-3 && (one.pos.1 - many.pos.1).abs() < 1e-3);
        // A time constant of 2 s: after it, 1 − 2/e of the way.
        let mut s = Spring::at((0.0, 0.0));
        s.step((1.0, 0.0), 2.0);
        assert!((s.pos.0 - (1.0 - 2.0 / std::f32::consts::E)).abs() < 1e-4);
    }

    #[test]
    fn smootherstep_eases_in_and_out() {
        assert_eq!((smootherstep(0.0), smootherstep(1.0)), (0.0, 1.0));
        assert_eq!(smootherstep(-1.0), 0.0);
        assert_eq!(smootherstep(2.0), 1.0);
        assert!((smootherstep(0.5) - 0.5).abs() < 1e-6);
        assert!(smootherstep(0.01) < 1e-4, "no speed at the start");
        assert!(smootherstep(0.99) > 1.0 - 1e-4, "no speed at the end");
        let mut last = 0.0;
        for i in 0..=100 {
            let v = smootherstep(i as f32 / 100.0);
            assert!(v >= last);
            last = v;
        }
    }

    #[test]
    fn a_focus_zooms_two_to_three_times_by_sprite_size_and_never_below_the_cover() {
        assert_eq!(zoom_for_size((6, 3)), 3.0);
        assert_eq!(zoom_for_size((40, 20)), 2.0);
        let mid = zoom_for_size((16, 8));
        assert!(
            (2.0..=3.0).contains(&mid) && mid < 3.0 && mid > 2.0,
            "{mid}"
        );
        assert_eq!(zoom_for_size((0, 0)), 3.0);
        // A 21:9 target needs 1.3125: a focus never wants less.
        let narrow = stage(WIDE_21_9);
        assert!((narrow.min_zoom() - 1.3125).abs() < 1e-4);
        assert_eq!(focus_zoom(2.0, &narrow), 2.0);
        assert_eq!(focus_zoom(1.0, &narrow), narrow.min_zoom());
        // On a much wider target the cover rules over the size.
        let wider = stage((4000, 720));
        assert!(wider.min_zoom() > 3.0);
        assert_eq!(focus_zoom(3.0, &wider), wider.min_zoom());
    }

    #[test]
    fn the_view_stays_inside_the_tank_and_the_zoom_in_bounds() {
        for target in [WIDE_16_9, WIDE_21_9, (900, 900)] {
            let s = stage(target);
            let min = s.min_zoom();
            let mut w = world(&FISH);
            let mut tour = Tour::new(11);
            for f in run(&mut tour, &mut w, &s, 450.0) {
                assert!(f.view.is_valid(), "{target:?}: {f:?}");
                let zoom = zoom_of(f.view, &s);
                assert!(
                    zoom >= 1.0 - 1e-3 && zoom <= 3.0f32.max(min) + 1e-3,
                    "{target:?}: {zoom}"
                );
                // No letterbox shows from the covering zoom up (and a
                // focus never goes below it).
                if zoom >= min + 1e-3 {
                    assert!(inside(f.view), "{target:?}: {f:?}");
                }
                if f.phase == TourPhase::Focus {
                    assert!(zoom >= min - 1e-3 && inside(f.view), "{target:?}: {f:?}");
                }
                if f.phase == TourPhase::Hold {
                    assert_eq!(f.view, s.fit(), "the pull-back is the full tank");
                }
            }
        }
    }

    /// The runs of equal `phase` in `frames`, as `(phase, steps)`.
    fn runs(frames: &[Frame]) -> Vec<(TourPhase, usize)> {
        let mut out: Vec<(TourPhase, usize)> = Vec::new();
        for f in frames {
            match out.last_mut() {
                Some((p, n)) if *p == f.phase => *n += 1,
                _ => out.push((f.phase, 1)),
            }
        }
        out
    }

    #[test]
    fn it_pulls_back_after_three_to_five_focuses_and_holds_the_full_tank() {
        let s = stage(WIDE_16_9);
        for seed in 0..6 {
            let w = world(&FISH);
            let mut tour = Tour::new(seed);
            let frames = run_still(&mut tour, &w, &s, 1500.0);
            let seq = runs(&frames);
            let secs = |n: usize| n as f32 * STEP;
            let mut focuses = 0;
            let mut pull_backs = 0;
            for (i, &(phase, n)) in seq.iter().enumerate() {
                match phase {
                    TourPhase::Focus if i + 1 < seq.len() => {
                        focuses += 1;
                        assert!((20.0..=40.1).contains(&secs(n)), "focus of {} s", secs(n));
                    }
                    TourPhase::Hold if i + 1 < seq.len() => {
                        pull_backs += 1;
                        assert!(
                            (secs(n) - 20.0).abs() < 0.2,
                            "seed {seed}: a hold of {} s",
                            secs(n)
                        );
                        assert!((3..=5).contains(&focuses), "seed {seed}: {focuses} focuses");
                        focuses = 0;
                    }
                    TourPhase::Glide if i + 1 < seq.len() => {
                        assert!((6.0..=10.1).contains(&secs(n)), "glide of {} s", secs(n));
                    }
                    _ => {}
                }
            }
            assert!(
                pull_backs >= 2,
                "seed {seed}: {pull_backs} pull-backs in 1500 s"
            );
        }
    }

    #[test]
    fn it_avoids_the_last_few_creatures_and_copes_with_one_or_two() {
        let s = stage(WIDE_16_9);
        let targets = |names: &[&str], seed| {
            let w = world(names);
            let mut tour = Tour::new(seed);
            let mut seen: Vec<CreatureId> = Vec::new();
            for f in run_still(&mut tour, &w, &s, 1500.0) {
                if let (TourPhase::Focus, Some(id)) = (f.phase, f.target)
                    && seen.last() != Some(&id)
                {
                    seen.push(id);
                }
            }
            seen
        };
        for seed in 0..4 {
            let seen = targets(&FISH, seed);
            assert!(seen.len() >= 10);
            for window in seen.windows(4) {
                // Of any four in a row, no creature comes twice within three.
                assert_ne!(window[0], window[3]);
                assert_ne!(window[0], window[1]);
                assert_ne!(window[0], window[2]);
                assert_ne!(window[1], window[2]);
            }
        }
        // Two creatures take turns; one is shown again and again.
        let two = targets(&FISH[..2], 1);
        assert!(two.windows(2).all(|p| p[0] != p[1]) && two.len() >= 10);
        let one = targets(&FISH[..1], 1);
        assert_ne!(one.len(), 0);
    }

    #[test]
    fn a_dead_or_removed_target_ends_the_focus_with_a_glide() {
        let s = stage(WIDE_16_9);
        let mut w = world(&FISH);
        let mut tour = Tour::new(2);
        // Run to the middle of a focus.
        let mut prev;
        loop {
            w.tick();
            tour.advance(STEP, &w, &s);
            if tour.phase() == TourPhase::Focus && tour.card().is_some() {
                break;
            }
        }
        for _ in 0..100 {
            w.tick();
            tour.advance(STEP, &w, &s);
        }
        assert_eq!(tour.phase(), TourPhase::Focus);
        let gone = tour.target().unwrap();
        assert!(tour.card().is_some());
        prev = tour.view(&s);
        assert!(w.remove_creature(gone));
        tour.advance(STEP, &w, &s);
        // A glide to something else, with no card and no jump.
        assert_eq!(tour.phase(), TourPhase::Glide);
        assert_ne!(tour.target(), Some(gone));
        assert_eq!(tour.card(), None);
        let (pan, zoom) = jump(prev, tour.view(&s));
        assert!(pan < 1.0 && zoom < 1.01, "{pan} {zoom}");
        // And it gets there, and follows the new one.
        for _ in 0..(11.0 / STEP) as u32 {
            w.tick();
            tour.advance(STEP, &w, &s);
            let now = tour.view(&s);
            let (pan, zoom) = jump(prev, now);
            assert!(pan < 4.0 && zoom < 1.04, "{pan} {zoom}");
            prev = now;
        }
        assert_eq!(tour.phase(), TourPhase::Focus);
        assert_ne!(tour.target(), Some(gone));
    }

    #[test]
    fn a_target_that_goes_mid_glide_redirects_the_glide_without_a_jump() {
        let s = stage(WIDE_16_9);
        let mut w = world(&FISH);
        let mut tour = Tour::new(4);
        w.tick();
        tour.advance(STEP, &w, &s);
        assert_eq!(tour.phase(), TourPhase::Glide);
        for _ in 0..40 {
            w.tick();
            tour.advance(STEP, &w, &s);
        }
        let before = tour.view(&s);
        let heading = tour.target().unwrap();
        w.remove_creature(heading);
        tour.advance(STEP, &w, &s);
        assert_eq!(tour.phase(), TourPhase::Glide);
        assert_ne!(tour.target(), Some(heading));
        assert_eq!(tour.view(&s), before, "no jump");
        // Every creature gone mid-glide: it glides to the drift.
        for id in w.creatures().iter().map(Creature::id).collect::<Vec<_>>() {
            w.remove_creature(id);
        }
        tour.advance(STEP, &w, &s);
        assert_eq!(tour.target(), None);
        assert_eq!(tour.phase(), TourPhase::Glide);
        for _ in 0..(11.0 / STEP) as u32 {
            tour.advance(STEP, &w, &s);
        }
        assert_eq!(tour.phase(), TourPhase::Drift);
    }

    #[test]
    fn an_empty_tank_drifts_slowly_and_stays_inside() {
        for target in [WIDE_16_9, WIDE_21_9] {
            let s = stage(target);
            let min = s.min_zoom();
            let mut w = world(&[]);
            let mut tour = Tour::new(8);
            let frames = run(&mut tour, &mut w, &s, 600.0);
            assert!(
                frames
                    .iter()
                    .all(|f| f.phase == TourPhase::Drift && f.card.is_none())
            );
            let zooms: Vec<f32> = frames.iter().map(|f| zoom_of(f.view, &s)).collect();
            let (lo, hi) = zooms
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), &z| (lo.min(z), hi.max(z)));
            assert!(
                lo >= min - 1e-3 && hi <= min * 1.15 + 1e-3,
                "{lo}..{hi} over {min}"
            );
            assert!(hi - lo > 0.1 * min, "it zooms: {lo}..{hi}");
            assert!(frames.iter().all(|f| inside(f.view)));
            // It moves (the picture is never still) and slowly (no step
            // moves it by more than a hundredth of a cell).
            let mut centres = std::collections::BTreeSet::new();
            for pair in frames.windows(2) {
                let (pan, zoom) = jump(pair[0].view, pair[1].view);
                assert!(pan < 0.05 && zoom < 1.001, "{pan} {zoom}");
                centres.insert((
                    (pair[1].view.x * 10.0) as i32,
                    (pair[1].view.y * 10.0) as i32,
                ));
            }
            assert!(
                centres.len() > 20,
                "{target:?}: {} distinct places",
                centres.len()
            );
        }
    }

    #[test]
    fn creatures_appearing_in_an_empty_tank_start_a_glide_from_the_drift() {
        let s = stage(WIDE_16_9);
        let mut w = world(&[]);
        let mut tour = Tour::new(3);
        run(&mut tour, &mut w, &s, 30.0);
        let before = tour.view(&s);
        let sp = w.species().id("guppy").unwrap();
        w.add_creature(sp, 80, 40).unwrap();
        tour.advance(STEP, &w, &s);
        assert_eq!(tour.phase(), TourPhase::Glide);
        assert!(tour.target().is_some());
        assert_eq!(
            tour.view(&s),
            before,
            "the glide starts where the drift was"
        );
        let frames = run(&mut tour, &mut w, &s, 12.0);
        assert!(frames.iter().any(|f| f.phase == TourPhase::Focus));
    }

    #[test]
    fn glides_and_follows_move_the_view_by_small_steps() {
        // The longest glide crosses the tank in 6 s; smootherstep's peak
        // speed is 1.875× the mean, so a 50 ms step moves the view a few
        // cells at most, and its size by a few percent.
        for target in [WIDE_16_9, WIDE_21_9] {
            let s = stage(target);
            for seed in 0..4 {
                // Swimming creatures for the first seed, still ones for the
                // rest (the ticking world is what costs).
                let mut w = world(&FISH);
                let mut tour = Tour::new(seed);
                let frames = if seed == 0 {
                    run(&mut tour, &mut w, &s, 400.0)
                } else {
                    run_still(&mut tour, &w, &s, 1200.0)
                };
                for pair in frames.windows(2) {
                    let (pan, zoom) = jump(pair[0].view, pair[1].view);
                    assert!(
                        pan < 4.0 && zoom < 1.03,
                        "{target:?} seed {seed}: {:?} -> {:?}: {pan} cells, ×{zoom}",
                        pair[0],
                        pair[1]
                    );
                }
            }
        }
    }

    #[test]
    fn a_resize_mid_glide_keeps_a_valid_view_and_a_covering_focus() {
        let (wide, wider) = (stage(WIDE_16_9), stage(WIDE_21_9));
        let mut w = world(&FISH);
        let mut tour = Tour::new(12);
        for step in 0..4000 {
            w.tick();
            // Resize back and forth, mid-glide and mid-focus alike.
            let s = if (step / 150) % 2 == 0 { &wide } else { &wider };
            tour.advance(STEP, &w, s);
            let v = tour.view(s);
            assert!(v.is_valid());
            if tour.phase() == TourPhase::Focus {
                assert!(inside(v), "step {step}: {v:?}");
            }
        }
    }

    #[test]
    fn the_card_turns_pages_every_twelve_seconds_once_a_glide_settles() {
        let s = stage(WIDE_16_9);
        let mut w = world(&FISH);
        let mut tour = Tour::new(1);
        let frames = run(&mut tour, &mut w, &s, 800.0);
        let mut shown = 0;
        for (i, f) in frames.iter().enumerate() {
            match f.phase {
                TourPhase::Focus => {
                    let card = f.card.expect("a settled focus shows its card");
                    assert_eq!(Some(card.creature), f.target);
                    shown += 1;
                    // Seconds since the focus began, counted back to its start.
                    let since = frames[..=i]
                        .iter()
                        .rev()
                        .take_while(|g| g.phase == TourPhase::Focus && g.target == f.target)
                        .count();
                    let t = (since - 1) as f32 * STEP;
                    let want = if (((t + 1e-3) / 12.0) as u32).is_multiple_of(2) {
                        InspectPage::Live
                    } else {
                        InspectPage::Species
                    };
                    // Away from a page boundary the page is exactly right.
                    let off = (t % 12.0).min(12.0 - t % 12.0);
                    if off > 0.1 {
                        assert_eq!(card.page, want, "at {t} s into a focus");
                    }
                    if t < 11.9 {
                        assert_eq!(card.page, InspectPage::Live, "at {t} s");
                    }
                }
                _ => assert_eq!(f.card, None, "no card while {:?}", f.phase),
            }
        }
        assert!(shown > 100);
        // Every focus longer than 12 s shows the species page, and one
        // longer than 24 s turns back to live.
        let mut pages = std::collections::BTreeSet::new();
        for f in &frames {
            if let Some(c) = f.card {
                pages.insert(c.page as u8);
            }
        }
        assert_eq!(pages.len(), 2);
    }
}
