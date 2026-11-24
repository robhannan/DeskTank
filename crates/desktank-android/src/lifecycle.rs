//! The surface and running state of each Android host, reduced to which
//! surface draws now. Every callback carries its host's token, so a late
//! `surfaceDestroyed` can only drop the surface it came from.

use std::collections::BTreeMap;

/// The token the shim gives each host instance, increasing across both kinds.
pub type HostId = i64;

/// An Android host of the shared tank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    /// The non-interactive screensaver.
    Dream,
    /// The launcher Activity, with touch input.
    App,
}

/// A running host with a drawable surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drawable {
    /// The host's token.
    pub id: HostId,
    /// Which kind of host is drawing.
    pub kind: HostKind,
    /// Surface size in pixels.
    pub size: (u32, u32),
}

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
    /// `DreamService.onDreamingStarted` or `Activity.onResume`.
    Started(HostKind),
    /// `DreamService.onDreamingStopped` or `Activity.onPause`.
    Stopped,
}

/// One host's surface and running state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lifecycle {
    /// The surface exists; its size once known (zero until `surfaceChanged`).
    surface: Option<(u32, u32)>,
    running: Option<HostKind>,
}

impl Lifecycle {
    /// Applies `event`.
    pub fn handle(&mut self, event: Event) {
        match event {
            Event::SurfaceCreated => self.surface = Some((0, 0)),
            Event::SurfaceChanged { width, height } => self.surface = Some((width, height)),
            Event::SurfaceDestroyed => self.surface = None,
            Event::Started(kind) => self.running = Some(kind),
            Event::Stopped => self.running = None,
        }
    }

    /// Whether the surface exists (between `surfaceCreated` and
    /// `surfaceDestroyed`).
    #[must_use]
    pub fn has_surface(&self) -> bool {
        self.surface.is_some()
    }

    /// Whether the host is running.
    #[must_use]
    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    /// Draws only while running with a surface of nonzero size (SPEC §3.6).
    fn drawable(&self, id: HostId) -> Option<Drawable> {
        let kind = self.running?;
        let size = self.surface.filter(|&(w, h)| w > 0 && h > 0)?;
        Some(Drawable { id, kind, size })
    }
}

/// The shim's hosts with a surface or running state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hosts {
    hosts: BTreeMap<HostId, Lifecycle>,
}

impl Hosts {
    /// Applies `event` for host `id`.
    pub fn handle(&mut self, id: HostId, event: Event) {
        let state = self.hosts.entry(id).or_default();
        state.handle(event);
        if !state.has_surface() && !state.running() {
            self.hosts.remove(&id);
        }
    }

    /// The newest host that can draw.
    #[must_use]
    pub fn active(&self) -> Option<Drawable> {
        self.active_with_surface(|_| true)
    }

    pub(crate) fn active_with_surface(
        &self,
        has_surface: impl Fn(HostId) -> bool,
    ) -> Option<Drawable> {
        self.hosts
            .iter()
            .rev()
            .find_map(|(&id, lifecycle)| lifecycle.drawable(id).filter(|_| has_surface(id)))
    }

    /// Host `id`'s state, if it has a surface or is running.
    #[must_use]
    pub fn get(&self, id: HostId) -> Option<&Lifecycle> {
        self.hosts.get(&id)
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

    const DREAM_STARTED: Event = Event::Started(HostKind::Dream);
    const APP_STARTED: Event = Event::Started(HostKind::App);

    fn dream(id: HostId, size: (u32, u32)) -> Drawable {
        Drawable {
            id,
            kind: HostKind::Dream,
            size,
        }
    }

    #[test]
    fn draws_once_surface_and_dream_are_both_ready() {
        // Surface first, then the Dream starts (the usual order).
        let mut d = Hosts::default();
        d.handle(1, Event::SurfaceCreated);
        d.handle(1, SIZE);
        assert_eq!(d.active(), None);
        d.handle(1, DREAM_STARTED);
        assert_eq!(d.active(), Some(dream(1, HD)));

        // Dream first, then the surface.
        let mut d = Hosts::default();
        d.handle(1, DREAM_STARTED);
        d.handle(1, Event::SurfaceCreated);
        assert_eq!(d.active(), None, "no size yet");
        d.handle(1, SIZE);
        assert_eq!(d.active(), Some(dream(1, HD)));
    }

    #[test]
    fn follows_resizes_and_ignores_empty_sizes() {
        let mut d = Hosts::default();
        d.handle(1, Event::SurfaceCreated);
        d.handle(1, DREAM_STARTED);
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
        assert_eq!(d.active(), Some(dream(1, (3840, 2160))));
    }

    #[test]
    fn stops_drawing_when_dreaming_stops_or_the_surface_goes() {
        let mut d = Hosts::default();
        d.handle(1, Event::SurfaceCreated);
        d.handle(1, SIZE);
        d.handle(1, DREAM_STARTED);
        d.handle(1, Event::Stopped);
        assert_eq!(d.active(), None);
        assert!(d.get(1).is_some_and(Lifecycle::has_surface));
        d.handle(1, Event::SurfaceDestroyed);
        assert_eq!(d.get(1), None, "forgotten with no surface and no dream");

        d.handle(2, Event::SurfaceCreated);
        d.handle(2, SIZE);
        d.handle(2, DREAM_STARTED);
        d.handle(2, Event::SurfaceDestroyed);
        assert_eq!(d.active(), None, "no surface");
        assert!(d.get(2).is_some_and(Lifecycle::running));
    }

    #[test]
    fn a_late_destroy_from_the_old_dream_keeps_the_new_surface() {
        // PR #53 review: Dream 1 stops, Dream 2 starts and gets its surface,
        // then Dream 1's surfaceDestroyed arrives.
        let mut d = Hosts::default();
        for e in [Event::SurfaceCreated, SIZE, DREAM_STARTED] {
            d.handle(1, e);
        }
        d.handle(1, Event::Stopped);
        for e in [Event::SurfaceCreated, SIZE, DREAM_STARTED] {
            d.handle(2, e);
        }
        d.handle(1, Event::SurfaceDestroyed);
        assert_eq!(d.active(), Some(dream(2, HD)));
        assert_eq!(d.get(1), None);
    }

    #[test]
    fn the_newest_drawable_host_wins_across_kinds() {
        let mut d = Hosts::default();
        for e in [Event::SurfaceCreated, SIZE, DREAM_STARTED] {
            d.handle(1, e);
        }
        d.handle(2, APP_STARTED);
        d.handle(2, Event::SurfaceCreated);
        assert_eq!(d.active(), Some(dream(1, HD)), "app has no size yet");
        d.handle(2, SIZE);
        let app = Drawable {
            id: 2,
            kind: HostKind::App,
            size: HD,
        };
        assert_eq!(d.active(), Some(app));
        assert_eq!(
            d.active_with_surface(|id| id != 2),
            Some(dream(1, HD)),
            "fall back while the app has no native surface"
        );
        for e in [Event::SurfaceCreated, SIZE, DREAM_STARTED] {
            d.handle(3, e);
        }
        assert_eq!(d.active(), Some(dream(3, HD)));
        d.handle(3, Event::Stopped);
        assert_eq!(d.active(), Some(app));
        d.handle(3, Event::SurfaceDestroyed);
        assert_eq!(d.active(), Some(app), "late destroy from the Dream");
        d.handle(2, Event::Stopped);
        assert_eq!(d.active(), Some(dream(1, HD)));
        d.handle(2, APP_STARTED);
        assert_eq!(d.active(), Some(app), "resume with the existing surface");
    }
}
