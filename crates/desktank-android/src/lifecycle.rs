//! When the Dream draws: the surface and dreaming state the shim reports,
//! reduced to "draw a frame now" decisions. OS-agnostic, so it is tested on
//! the desktop.

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

/// The surface and Dream state. A frame is drawn only while the Dream is
/// dreaming and its surface has a size; nothing runs otherwise (SPEC §3.6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lifecycle {
    /// The surface exists; its size once known (zero until `surfaceChanged`).
    surface: Option<(u32, u32)>,
    dreaming: bool,
}

impl Lifecycle {
    /// Applies `event` and returns whether a frame should be drawn now: when
    /// the surface gets a (new) size while dreaming, or dreaming starts with
    /// a sized surface. The A0 skeleton draws one still frame per change; A1
    /// replaces this with the paced render loop.
    pub fn handle(&mut self, event: Event) -> bool {
        match event {
            Event::SurfaceCreated => {
                self.surface = Some((0, 0));
                false
            }
            Event::SurfaceChanged { width, height } => {
                self.surface = Some((width, height));
                self.can_draw()
            }
            Event::SurfaceDestroyed => {
                self.surface = None;
                false
            }
            Event::DreamingStarted => {
                self.dreaming = true;
                self.can_draw()
            }
            Event::DreamingStopped => {
                self.dreaming = false;
                false
            }
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

    fn can_draw(&self) -> bool {
        self.dreaming && matches!(self.surface, Some((w, h)) if w > 0 && h > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Event = Event::SurfaceChanged {
        width: 1920,
        height: 1080,
    };

    #[test]
    fn draws_once_surface_and_dream_are_both_ready() {
        // Surface first, then the Dream starts (the usual order).
        let mut l = Lifecycle::default();
        assert!(!l.handle(Event::SurfaceCreated));
        assert!(!l.handle(SIZE));
        assert!(l.handle(Event::DreamingStarted));

        // Dream first, then the surface.
        let mut l = Lifecycle::default();
        assert!(!l.handle(Event::DreamingStarted));
        assert!(!l.handle(Event::SurfaceCreated));
        assert!(l.handle(SIZE));
    }

    #[test]
    fn redraws_on_resize_while_dreaming() {
        let mut l = Lifecycle::default();
        l.handle(Event::SurfaceCreated);
        l.handle(Event::DreamingStarted);
        assert!(l.handle(SIZE));
        assert!(l.handle(Event::SurfaceChanged {
            width: 3840,
            height: 2160
        }));
    }

    #[test]
    fn never_draws_without_a_sized_surface_or_after_stopping() {
        let mut l = Lifecycle::default();
        l.handle(Event::SurfaceCreated);
        assert!(!l.handle(Event::DreamingStarted), "no size yet");
        assert!(!l.handle(Event::SurfaceChanged {
            width: 0,
            height: 1080
        }));
        assert!(l.handle(SIZE));
        assert!(!l.handle(Event::DreamingStopped));
        assert!(!l.handle(SIZE), "stopped: no frames");
        assert!(!l.dreaming());

        l.handle(Event::DreamingStarted);
        assert!(!l.handle(Event::SurfaceDestroyed));
        assert!(!l.has_surface());
        assert!(!l.handle(Event::DreamingStarted), "no surface");
    }
}
