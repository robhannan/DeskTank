//! When the Dream draws: the surface and dreaming state the shim reports for
//! each Dream, reduced to "which surface draws now, at what size".
//! OS-agnostic, so it is tested on the desktop.
//!
//! Every callback carries its Dream's token (a number the shim gives each
//! `DreamService` instance, increasing), so a stopping Dream's late
//! `surfaceDestroyed` can't drop the next Dream's surface.

use std::collections::BTreeMap;

/// The token the shim gives each Dream instance; later Dreams get larger
/// tokens.
pub type DreamId = i64;

/// A callback the Kotlin shim forwards over JNI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// `SurfaceHolder.Callback.surfaceCreated`.
    SurfaceCreated,
    /// `SurfaceHolder.Callback.surfaceChanged`, with the size in pixels.
    SurfaceChanged {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// `SurfaceHolder.Callback.surfaceDestroyed`.
    SurfaceDestroyed,
    /// `DreamService.onDreamingStarted`.
    DreamingStarted,
    /// `DreamService.onDreamingStopped`.
    DreamingStopped,
}

/// One Dream's surface and dreaming state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lifecycle {
    /// The surface exists; its size once known (zero until `surfaceChanged`).
    surface: Option<(u32, u32)>,
    dreaming: bool,
}

impl Lifecycle {
    /// Applies `event`.
    pub fn handle(&mut self, event: Event) {
        match event {
            Event::SurfaceCreated => self.surface = Some((0, 0)),
            Event::SurfaceChanged { width, height } => self.surface = Some((width, height)),
            Event::SurfaceDestroyed => self.surface = None,
            Event::DreamingStarted => self.dreaming = true,
            Event::DreamingStopped => self.dreaming = false,
        }
    }

    /// Whether the surface exists (between `surfaceCreated` and
    /// `surfaceDestroyed`).
    #[must_use]
    pub fn has_surface(&self) -> bool {
        self.surface.is_some()
    }

    /// Whether the Dream is dreaming.
    #[must_use]
    pub fn dreaming(&self) -> bool {
        self.dreaming
    }

    /// The surface size, if it should draw: dreaming, with a surface whose
    /// size is known and not zero. Nothing runs otherwise (SPEC §3.6).
    #[must_use]
    pub fn drawable(&self) -> Option<(u32, u32)> {
        self.surface
            .filter(|&(w, h)| self.dreaming && w > 0 && h > 0)
    }
}

/// Every Dream the shim has told us about that still has a surface or is
/// dreaming. A Dream is forgotten once it has neither.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dreams {
    dreams: BTreeMap<DreamId, Lifecycle>,
}

impl Dreams {
    /// Applies `event` for Dream `id`.
    pub fn handle(&mut self, id: DreamId, event: Event) {
        let state = self.dreams.entry(id).or_default();
        state.handle(event);
        if !state.has_surface() && !state.dreaming() {
            self.dreams.remove(&id);
        }
    }

    /// The Dream that draws now and its surface size: the newest one that
    /// can draw. Normally there is at most one; while one Dream hands over
    /// to the next, the newer wins.
    #[must_use]
    pub fn active(&self) -> Option<(DreamId, (u32, u32))> {
        self.dreams
            .iter()
            .rev()
            .find_map(|(&id, l)| l.drawable().map(|size| (id, size)))
    }

    /// Dream `id`'s state, if it has a surface or is dreaming.
    #[must_use]
    pub fn get(&self, id: DreamId) -> Option<&Lifecycle> {
        self.dreams.get(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HD: (u32, u32) = (1920, 1080);
    const SIZE: Event = Event::SurfaceChanged {
        width: HD.0,
        height: HD.1,
    };

    #[test]
    fn draws_once_surface_and_dream_are_both_ready() {
        // Surface first, then the Dream starts (the usual order).
        let mut d = Dreams::default();
        d.handle(1, Event::SurfaceCreated);
        d.handle(1, SIZE);
        assert_eq!(d.active(), None);
        d.handle(1, Event::DreamingStarted);
        assert_eq!(d.active(), Some((1, HD)));

        // Dream first, then the surface.
        let mut d = Dreams::default();
        d.handle(1, Event::DreamingStarted);
        d.handle(1, Event::SurfaceCreated);
        assert_eq!(d.active(), None, "no size yet");
        d.handle(1, SIZE);
        assert_eq!(d.active(), Some((1, HD)));
    }

    #[test]
    fn follows_resizes_and_ignores_empty_sizes() {
        let mut d = Dreams::default();
        d.handle(1, Event::SurfaceCreated);
        d.handle(1, Event::DreamingStarted);
        d.handle(
            1,
            Event::SurfaceChanged {
                width: 0,
                height: 1080,
            },
        );
        assert_eq!(d.active(), None);
        d.handle(
            1,
            Event::SurfaceChanged {
                width: 3840,
                height: 2160,
            },
        );
        assert_eq!(d.active(), Some((1, (3840, 2160))));
    }

    #[test]
    fn stops_drawing_when_dreaming_stops_or_the_surface_goes() {
        let mut d = Dreams::default();
        d.handle(1, Event::SurfaceCreated);
        d.handle(1, SIZE);
        d.handle(1, Event::DreamingStarted);
        d.handle(1, Event::DreamingStopped);
        assert_eq!(d.active(), None);
        assert!(d.get(1).is_some_and(Lifecycle::has_surface));
        d.handle(1, Event::SurfaceDestroyed);
        assert_eq!(d.get(1), None, "forgotten with no surface and no dream");

        d.handle(2, Event::SurfaceCreated);
        d.handle(2, SIZE);
        d.handle(2, Event::DreamingStarted);
        d.handle(2, Event::SurfaceDestroyed);
        assert_eq!(d.active(), None, "no surface");
        assert!(d.get(2).is_some_and(Lifecycle::dreaming));
    }

    #[test]
    fn a_late_destroy_from_the_old_dream_keeps_the_new_surface() {
        // PR #53 review: Dream 1 stops, Dream 2 starts and gets its surface,
        // then Dream 1's surfaceDestroyed arrives.
        let mut d = Dreams::default();
        for e in [Event::SurfaceCreated, SIZE, Event::DreamingStarted] {
            d.handle(1, e);
        }
        d.handle(1, Event::DreamingStopped);
        for e in [Event::SurfaceCreated, SIZE, Event::DreamingStarted] {
            d.handle(2, e);
        }
        d.handle(1, Event::SurfaceDestroyed);
        assert_eq!(d.active(), Some((2, HD)));
        assert_eq!(d.get(1), None);
    }

    #[test]
    fn the_newest_dream_wins_while_two_can_draw() {
        let mut d = Dreams::default();
        for id in [1, 2] {
            for e in [Event::SurfaceCreated, SIZE, Event::DreamingStarted] {
                d.handle(id, e);
            }
        }
        assert_eq!(d.active(), Some((2, HD)));
        d.handle(2, Event::DreamingStopped);
        assert_eq!(d.active(), Some((1, HD)));
    }
}
