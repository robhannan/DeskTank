//! Frame pacing: a fixed simulation tick and a capped frame rate, driven by
//! the host's clock (SPEC §4.2). Pure logic, so it is tested without a GPU.

use core::time::Duration;

/// Most simulation ticks run in one [`Pacer::due_ticks`] call. A host that
/// falls further behind (a stall, a debugger) drops the backlog instead of
/// spiralling.
pub const MAX_CATCH_UP: u32 = 4;

/// What the host should do after [`crate::Tank::tick`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameRequest {
    /// Draw a frame now: the world changed (or the target was invalidated)
    /// and the frame cap allows it.
    pub redraw: bool,
    /// When to call `tick` again, on the host's clock. Sleep until then
    /// (e.g. `ControlFlow::WaitUntil`); nothing happens in between.
    pub wake_at: Duration,
    /// Simulation ticks run by this call.
    pub sim_ticks: u32,
}

/// Fixed-step tick clock plus frame-rate cap.
#[derive(Clone, Debug)]
pub struct Pacer {
    tick_interval: Duration,
    frame_interval: Duration,
    /// Host time of the next simulation tick; `None` until started or after
    /// a pause, so the clock re-anchors instead of catching up.
    next_tick: Option<Duration>,
    last_frame: Option<Duration>,
}

impl Pacer {
    /// A pacer ticking `ticks_per_second` and drawing at most `max_fps`.
    #[must_use]
    pub fn new(ticks_per_second: u32, max_fps: u32) -> Self {
        Self {
            tick_interval: Duration::from_secs(1) / ticks_per_second.max(1),
            frame_interval: Duration::from_secs(1) / max_fps.max(1),
            next_tick: None,
            last_frame: None,
        }
    }

    /// Simulation ticks due at `now`. The first call after creation or a
    /// pause starts the clock and runs none.
    pub fn due_ticks(&mut self, now: Duration) -> u32 {
        let Some(mut next) = self.next_tick else {
            self.next_tick = Some(now + self.tick_interval);
            return 0;
        };
        let mut n = 0;
        while now >= next && n < MAX_CATCH_UP {
            n += 1;
            next += self.tick_interval;
        }
        if now >= next {
            next = now + self.tick_interval;
        }
        self.next_tick = Some(next);
        n
    }

    /// Whether the frame cap allows a frame at `now`.
    #[must_use]
    pub fn frame_allowed(&self, now: Duration) -> bool {
        self.last_frame
            .is_none_or(|t| now >= t + self.frame_interval)
    }

    /// Records that a frame is being drawn at `now`.
    pub fn frame_drawn(&mut self, now: Duration) {
        self.last_frame = Some(now);
    }

    /// When the host should next call in: the next tick, or earlier if a
    /// frame is waiting only on the cap.
    #[must_use]
    pub fn wake_at(&self, now: Duration, frame_waiting: bool) -> Duration {
        let tick = self.next_tick.unwrap_or(now + self.tick_interval);
        match self.last_frame {
            Some(t) if frame_waiting => tick.min(t + self.frame_interval),
            _ => tick,
        }
    }

    /// Stops the clock: the next [`Self::due_ticks`] re-anchors, so world
    /// time doesn't advance while the host is hidden (SPEC §6.2).
    pub fn pause(&mut self) {
        self.next_tick = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn ticks_at_a_fixed_rate() {
        let mut p = Pacer::new(20, 30);
        assert_eq!(p.due_ticks(Duration::ZERO), 0);
        assert_eq!(p.due_ticks(49 * MS), 0);
        assert_eq!(p.due_ticks(50 * MS), 1);
        assert_eq!(p.due_ticks(99 * MS), 0);
        assert_eq!(p.due_ticks(160 * MS), 2);
        assert_eq!(p.wake_at(160 * MS, false), 200 * MS);
    }

    #[test]
    fn drops_a_long_backlog() {
        let mut p = Pacer::new(20, 30);
        p.due_ticks(Duration::ZERO);
        assert_eq!(p.due_ticks(Duration::from_secs(10)), MAX_CATCH_UP);
        assert_eq!(
            p.wake_at(Duration::from_secs(10), false),
            Duration::from_secs(10) + 50 * MS
        );
    }

    #[test]
    fn pause_stops_world_time() {
        let mut p = Pacer::new(20, 30);
        p.due_ticks(Duration::ZERO);
        p.pause();
        // An hour hidden: resuming re-anchors and runs nothing.
        assert_eq!(p.due_ticks(Duration::from_secs(3600)), 0);
        assert_eq!(p.due_ticks(Duration::from_secs(3600) + 50 * MS), 1);
    }

    #[test]
    fn caps_the_frame_rate() {
        let mut p = Pacer::new(20, 30);
        p.due_ticks(Duration::ZERO);
        assert!(p.frame_allowed(Duration::ZERO));
        p.frame_drawn(Duration::ZERO);
        assert!(!p.frame_allowed(20 * MS));
        // A frame waiting on the cap wakes at the cap, before the next tick.
        assert_eq!(p.wake_at(20 * MS, true), Duration::from_secs(1) / 30);
        assert_eq!(p.wake_at(20 * MS, false), 50 * MS);
        assert!(p.frame_allowed(34 * MS));
    }
}
