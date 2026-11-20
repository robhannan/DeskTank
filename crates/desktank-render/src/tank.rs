//! The loop-agnostic tank: a world, its pacing and its renderer (SPEC §4.2).

use core::time::Duration;

use desktank_sim::{TICKS_PER_SECOND, World, daylight};
use thiserror::Error;

use crate::grid::DirtyRects;
use crate::hud::{self, BrushPreview, Palette, PaletteLayout};
use crate::layout::Layout;
use crate::pacing::{FrameRequest, Pacer};
use crate::renderer::Renderer;
use crate::timer::GpuTimer;
use crate::{Gpu, TankSurface};

/// Frame-rate cap for optional plant sway (SPEC §2.7, T6d).
pub const SWAY_MAX_FPS: u32 = 10;

/// Minimum interval between sway-only redraws (100 ms = 10 fps).
const SWAY_INTERVAL: Duration = Duration::from_millis(100);

/// Tank settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TankConfig {
    /// Frame-rate cap (SPEC §6.2: 30 on AC power).
    pub max_fps: u32,
}

impl Default for TankConfig {
    fn default() -> Self {
        Self { max_fps: 30 }
    }
}

/// Error drawing a frame.
#[derive(Debug, Error)]
pub enum DrawError {
    /// The surface rejected the frame as invalid; a bug, not a transient.
    #[error("surface texture failed validation")]
    Validation,
}

/// A simulated world plus everything needed to show it. The host calls
/// [`Tank::tick`] on its clock and [`Tank::draw`] (or [`Tank::render_to`])
/// when the returned [`FrameRequest`] asks for a frame.
pub struct Tank {
    world: World,
    pacer: Pacer,
    dirty: DirtyRects,
    /// A frame is wanted: the world changed or the target was invalidated.
    needs_frame: bool,
    renderer: Option<Renderer>,
    timer: Option<GpuTimer>,
    gpu_time: Option<Duration>,
    /// The interact palette, if shown.
    palette: Option<Palette>,
    /// The palette image needs uploading.
    palette_dirty: bool,
    /// The brush outline, if shown.
    cursor: Option<BrushPreview>,
    /// The field tinted over the tank (debug overlay), if any.
    field_overlay: Option<crate::FieldOverlay>,
    /// Field version last drawn with the overlay.
    fields_drawn: u64,
    /// Local time of day, minutes after midnight (day and night).
    time_of_day: u16,
    /// Creatures version last drawn (`None` before the first frame).
    creatures_drawn: Option<u64>,
    /// Plants version last drawn (`None` before the first frame).
    plants_drawn: Option<u64>,
    /// Optional render-only plant sway (SPEC §2.7, T6d); off by default.
    sway: bool,
    /// Host is running on battery power: suppresses sway frames.
    on_battery: bool,
    /// Host is running as a screensaver: suppresses sway frames.
    saver_mode: bool,
    /// Host surface is hidden/paused: suppresses sway frames.
    hidden: bool,
    /// Host time of the last frame drawn while sway was active.
    last_sway_frame: Option<Duration>,
}

impl Tank {
    /// A tank showing `world`.
    #[must_use]
    pub fn new(world: World, config: TankConfig) -> Self {
        let dirty = DirtyRects::new(world.width(), world.height());
        Self {
            world,
            pacer: Pacer::new(TICKS_PER_SECOND, config.max_fps),
            dirty,
            needs_frame: true,
            renderer: None,
            timer: None,
            gpu_time: None,
            palette: None,
            palette_dirty: false,
            cursor: None,
            field_overlay: None,
            fields_drawn: 0,
            time_of_day: daylight::NOON,
            creatures_drawn: None,
            plants_drawn: None,
            sway: false,
            on_battery: false,
            saver_mode: false,
            hidden: false,
            last_sway_frame: None,
        }
    }

    /// Turns optional render-only plant sway on or off (SPEC §2.7, T6d).
    /// Off by default. While active, sway redraws a settled planted tank at
    /// most [`SWAY_MAX_FPS`] (10 fps), and is automatically disabled on
    /// battery, in screensaver mode and while hidden.
    pub fn set_sway(&mut self, enabled: bool) {
        let was = self.sway_active();
        self.sway = enabled;
        if self.sway_active() != was && !self.hidden {
            self.needs_frame = true;
            if !self.sway_active() {
                self.last_sway_frame = None;
            }
        }
    }

    /// Whether the user has enabled plant sway.
    #[must_use]
    pub fn sway(&self) -> bool {
        self.sway
    }

    /// Tells the tank whether the host is on battery power (disables sway).
    pub fn set_on_battery(&mut self, on_battery: bool) {
        let was = self.sway_active();
        self.on_battery = on_battery;
        if self.sway_active() != was && !self.hidden {
            self.needs_frame = true;
            if !self.sway_active() {
                self.last_sway_frame = None;
            }
        }
    }

    /// Tells the tank whether the host is a screensaver (disables sway).
    pub fn set_saver_mode(&mut self, saver_mode: bool) {
        let was = self.sway_active();
        self.saver_mode = saver_mode;
        if self.sway_active() != was && !self.hidden {
            self.needs_frame = true;
            if !self.sway_active() {
                self.last_sway_frame = None;
            }
        }
    }

    /// Tells the tank whether the host surface is hidden (disables sway
    /// without requesting a frame).
    pub fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
        if hidden {
            self.last_sway_frame = None;
        }
    }

    /// Whether plant sway is currently active (enabled, not on battery, not
    /// in screensaver mode, not hidden, and the tank has at least one plant).
    #[must_use]
    pub fn sway_active(&self) -> bool {
        self.sway
            && !self.on_battery
            && !self.saver_mode
            && !self.hidden
            && self.world.plant_count() > 0
    }

    fn sway_frame_due(&self, now: Duration) -> bool {
        self.last_sway_frame
            .is_none_or(|t| now >= t + SWAY_INTERVAL)
    }

    /// Current sway phase in seconds of world time (deterministic for
    /// capture; wrapped hourly to keep f32 precision crisp).
    fn sway_phase(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)] // < 72_000 ticks, exact in f32
        let ticks = (self.world.tick_count() % (u64::from(TICKS_PER_SECOND) * 3600)) as f32;
        #[allow(clippy::cast_precision_loss)] // 20, exact
        let hz = TICKS_PER_SECOND as f32;
        ticks / hz
    }

    /// Sets the local time of day (minutes after midnight) the tank is lit
    /// for (SPEC §2.6: day and night follow local time; the host injects
    /// it). Asks for a frame only if the tint changes, so a host can call
    /// this every minute. Default: noon.
    pub fn set_time_of_day(&mut self, minute: u16) {
        let minute = minute % daylight::MINUTES_PER_DAY;
        let bits = |m: u16| daylight::tint(m).map(f32::to_bits);
        if bits(minute) != bits(self.time_of_day) {
            self.needs_frame = true;
        }
        self.time_of_day = minute;
    }

    /// The local time of day the tank is lit for, minutes after midnight.
    #[must_use]
    pub fn time_of_day(&self) -> u16 {
        self.time_of_day
    }

    /// Tints the tank by a field's value (a debug overlay), or with `None`
    /// turns the tint off. While on, a field change asks for a frame.
    pub fn set_field_overlay(&mut self, field: Option<crate::FieldOverlay>) {
        if self.field_overlay != field {
            self.field_overlay = field;
            self.needs_frame = true;
        }
    }

    /// The field overlay shown, if any.
    #[must_use]
    pub fn field_overlay(&self) -> Option<crate::FieldOverlay> {
        self.field_overlay
    }

    /// Shows (or with `None`, hides) the interact palette. A change asks
    /// for a frame; setting the same palette again costs nothing.
    pub fn set_palette(&mut self, palette: Option<Palette>) {
        if self.palette != palette {
            self.palette = palette;
            self.palette_dirty = true;
            self.needs_frame = true;
        }
    }

    /// Shows (or hides) the brush outline. A change asks for a frame.
    pub fn set_cursor(&mut self, cursor: Option<BrushPreview>) {
        if self.cursor != cursor {
            self.cursor = cursor;
            self.needs_frame = true;
        }
    }

    /// Where the palette is drawn in a `target`-pixel surface, for hit
    /// tests; `None` if no palette is shown.
    #[must_use]
    pub fn palette_layout(&self, target: (u32, u32)) -> Option<PaletteLayout> {
        let p = self.palette.as_ref()?;
        Some(PaletteLayout::fit(
            target,
            &self.layout(target),
            p.tabs.len(),
            p.items.len(),
        ))
    }

    /// Where the grid is drawn in a `target`-pixel surface.
    #[must_use]
    pub fn layout(&self, target: (u32, u32)) -> Layout {
        let side = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Layout::fit(
            (side(self.world.width()), side(self.world.height())),
            target,
        )
    }

    /// The world.
    #[must_use]
    pub fn world(&self) -> &World {
        &self.world
    }

    /// The world, for edits. Edits show after the next simulation tick.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Advances to `now` on the host's monotonic clock (any origin): runs
    /// the simulation ticks due (fixed 20 Hz) and says whether to draw and
    /// when to call again.
    pub fn tick(&mut self, now: Duration) -> FrameRequest {
        let sim_ticks = self.pacer.due_ticks(now);
        for _ in 0..sim_ticks {
            self.step();
        }
        let world_wanted = self.frame_wanted();
        let sway_on = self.sway_active();
        let sway_wanted = sway_on && self.sway_frame_due(now);
        let wanted = world_wanted || sway_wanted;
        let redraw = !self.hidden && wanted && self.pacer.frame_allowed(now);
        if redraw {
            self.pacer.frame_drawn(now);
            if sway_on {
                self.last_sway_frame = Some(now);
            }
        }
        let mut wake_at = self.pacer.wake_at(now, wanted && !redraw);
        if sway_on
            && !redraw
            && let Some(last) = self.last_sway_frame
        {
            wake_at = wake_at.min(last + SWAY_INTERVAL);
        }
        FrameRequest {
            redraw,
            wake_at,
            sim_ticks,
        }
    }

    /// Runs one simulation tick now, outside the real-time pacing, and
    /// records what changed for the next frame. For headless hosts such as
    /// `desktank capture`, which step world time themselves.
    pub fn step(&mut self) {
        self.world.tick();
        for r in self.world.dirty_rects() {
            self.dirty.add(r);
        }
        if self.field_overlay.is_some() && self.world.fields_version() != self.fields_drawn {
            self.needs_frame = true;
        }
    }

    /// Stops world time, e.g. while the surface is hidden. The next
    /// [`Tank::tick`] restarts the clock without catching up.
    pub fn pause(&mut self) {
        self.pacer.pause();
        self.last_sway_frame = None;
    }

    /// Asks for a frame even if the world hasn't changed (resize, expose).
    pub fn invalidate(&mut self) {
        self.needs_frame = true;
    }

    /// Re-uploads the whole grid with the next frame, so edits made through
    /// [`Tank::world_mut`] show without a simulation tick (while paused).
    pub fn cells_edited(&mut self) {
        self.dirty.mark_all();
    }

    /// Whether a frame is waiting: the world changed since the last frame
    /// (cells, a creature moved or a plant's tint changed), or the target
    /// was invalidated. A tank with swimming creatures wants a frame every
    /// tick (20 a second, under the frame cap) even when every chunk
    /// sleeps; a plant's tint changes at most once per plant update.
    #[must_use]
    pub fn frame_wanted(&self) -> bool {
        self.needs_frame
            || !self.dirty.is_empty()
            || self.creatures_drawn != Some(self.world.creatures_version())
            || self.plants_drawn != Some(self.world.plants_version())
    }

    /// Changes the frame-rate cap (SPEC §6.2: 15 fps on battery). The
    /// simulation tick is unaffected.
    pub fn set_max_fps(&mut self, max_fps: u32) {
        self.pacer.set_max_fps(max_fps);
    }

    /// Drops every GPU resource (after device or context loss). The world is
    /// untouched; the next frame, drawn with a new [`Gpu`], rebuilds the
    /// renderer and uploads the whole grid.
    pub fn release_gpu(&mut self) {
        self.renderer = None;
        self.timer = None;
        self.gpu_time = None;
        self.dirty.mark_all();
        self.needs_frame = true;
    }

    /// Measures GPU time per frame with timestamp queries. Returns whether
    /// the device supports them. Reading the time blocks, so only
    /// benchmarks should turn this on.
    pub fn enable_gpu_timing(&mut self, gpu: &Gpu) -> bool {
        self.timer = GpuTimer::new(gpu);
        self.timer.is_some()
    }

    /// GPU time of the last frame, if timing is enabled and it was read.
    #[must_use]
    pub fn last_gpu_time(&self) -> Option<Duration> {
        self.gpu_time
    }

    /// Draws a frame to `surface` and presents it. Returns `Ok(false)` if
    /// the surface had no frame to give (occluded, timed out, or outdated
    /// and now reconfigured); the frame stays wanted, so the next tick
    /// asks again.
    ///
    /// # Errors
    ///
    /// Returns an error if the surface texture fails validation.
    pub fn draw(&mut self, gpu: &Gpu, surface: &TankSurface) -> Result<bool, DrawError> {
        let Some((frame, suboptimal)) = surface.acquire(gpu)? else {
            return Ok(false);
        };
        let drawn = self.present(gpu, surface, frame);
        if suboptimal {
            // Drawn; reconfigure for the next.
            surface.reconfigure(gpu);
        }
        Ok(drawn)
    }

    fn present(&mut self, gpu: &Gpu, surface: &TankSurface, frame: wgpu::SurfaceTexture) -> bool {
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.render_to(gpu, &view, surface.format(), surface.size());
        gpu.queue.present(frame);
        true
    }

    /// Draws a frame into `target` (`size` pixels of `format`, with
    /// `RENDER_ATTACHMENT` usage), e.g. an offscreen texture. Uploads the
    /// dirty rectangles first.
    pub fn render_to(
        &mut self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        size: (u32, u32),
    ) {
        if !self
            .renderer
            .as_ref()
            .is_some_and(|r| r.matches(&self.world, format))
        {
            self.renderer = Some(Renderer::new(gpu, &self.world, format));
            self.dirty.mark_all();
            self.palette_dirty = true;
        }
        let palette = self.palette_layout(size);
        let sway = self.sway_active().then(|| self.sway_phase());
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        renderer.upload(gpu, &self.world, &mut self.dirty);
        renderer.upload_sprites(gpu, &self.world);
        self.creatures_drawn = Some(self.world.creatures_version());
        renderer.upload_plants(gpu, &self.world);
        self.plants_drawn = Some(self.world.plants_version());
        if self.field_overlay.is_some() {
            renderer.upload_fields(gpu, &self.world);
            self.fields_drawn = self.world.fields_version();
        }
        if self.palette_dirty {
            if let Some(p) = &self.palette {
                let (image, image_size) = hud::build(
                    p,
                    self.world.materials(),
                    self.world.species(),
                    self.world.plant_species(),
                );
                renderer.upload_hud(gpu, &image, image_size);
            }
            self.palette_dirty = false;
        }

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        renderer.encode(
            gpu,
            &mut encoder,
            target,
            size,
            self.world.surface_row(),
            daylight::tint(self.time_of_day),
            sway,
            (palette, self.cursor, self.field_overlay),
            self.timer.as_ref(),
        );
        if let Some(t) = &self.timer {
            t.resolve(&mut encoder);
        }
        let submission = gpu.queue.submit([encoder.finish()]);
        self.needs_frame = false;
        if let Some(t) = &self.timer {
            self.gpu_time = t.read(gpu, submission);
        }
    }

    /// Simulates the state-clearing side of `render_to` for GPU-free pacing
    /// and frame-count tests.
    #[cfg(test)]
    fn mark_drawn(&mut self) {
        self.dirty.take(|_| {});
        self.creatures_drawn = Some(self.world.creatures_version());
        self.plants_drawn = Some(self.world.plants_version());
        if self.field_overlay.is_some() {
            self.fields_drawn = self.world.fields_version();
        }
        self.needs_frame = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktank_sim::{Brush, MaterialTable, Propagule, Rect, WorldConfig};

    /// Builds a settled planted tank (a mature plant on sand/rock with 0
    /// awake chunks and no excess nutrients to spawn algae).
    fn settled_planted_tank() -> Tank {
        let mut world = World::new(
            WorldConfig {
                width: 64,
                height: 48,
                seed: 7,
                water_level: 0.9,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let sand = world.materials().id("sand").unwrap();
        let rock = world.materials().id("rock").unwrap();
        world
            .fill_rect(
                Rect {
                    x0: 0,
                    y0: 42,
                    x1: 64,
                    y1: 48,
                },
                sand,
                Brush::Replace,
            )
            .unwrap();
        world
            .fill_rect(
                Rect {
                    x0: 28,
                    y0: 38,
                    x1: 36,
                    y1: 42,
                },
                rock,
                Brush::Replace,
            )
            .unwrap();
        for b in world.bodies().map(|b| b.id).collect::<Vec<_>>() {
            world.set_anchored(b, true);
        }
        let moss = world.plant_species().id("java_moss").unwrap();
        world
            .paint_plant(32, 37, 0, moss, Propagule::Fragments)
            .unwrap();
        // Grow around the small rock until every site is covered and all
        // chunks sleep.
        for _ in 0..20 * 600 {
            world.tick();
        }
        assert!(world.plant_count() > 0, "expected a rooted plant");
        assert_eq!(world.awake_chunks(), 0, "expected a settled tank");
        let mut tank = Tank::new(world, TankConfig::default());
        // Anchor clock and draw the initial frame.
        let req = tank.tick(Duration::ZERO);
        assert!(req.redraw);
        tank.mark_drawn();
        tank
    }

    /// Steps `tank` at 50 ms intervals for `seconds` starting from `start`,
    /// returning how many frames `tick` asked to draw.
    fn count_frames(tank: &mut Tank, start: Duration, seconds: u32) -> u32 {
        let mut frames = 0;
        let steps = seconds * TICKS_PER_SECOND;
        for step in 1..=steps {
            let now = start + Duration::from_millis(u64::from(step) * 50);
            let req = tank.tick(now);
            if req.redraw {
                tank.mark_drawn();
                frames += 1;
            }
        }
        frames
    }

    #[test]
    fn settled_planted_tank_draws_no_extra_frames_with_sway_off() {
        let mut tank = settled_planted_tank();
        assert!(!tank.sway(), "sway must be off by default");
        assert!(!tank.sway_active());
        let frames = count_frames(&mut tank, Duration::ZERO, 10);
        assert_eq!(frames, 0, "settled tank with sway off drew {frames} frames");
    }

    #[test]
    fn sway_caps_redraws_at_10_fps_and_disables_on_battery_saver_and_hidden() {
        let mut tank = settled_planted_tank();
        tank.set_sway(true);
        assert!(tank.sway());
        assert!(tank.sway_active());
        // Draw the state-change frame at t = 1 s, then measure 10 s.
        let mut now = Duration::from_secs(1);
        let req = tank.tick(now);
        assert!(req.redraw);
        tank.mark_drawn();
        let frames = count_frames(&mut tank, now, 10);
        assert!(frames > 0, "sway should draw frames when active");
        assert!(
            frames <= 10 * SWAY_MAX_FPS,
            "sway drew {frames} frames in 10 s (cap {})",
            10 * SWAY_MAX_FPS
        );
        now += Duration::from_secs(10);

        // On battery: one frame to snap back to rest, then 0 sway frames.
        tank.set_on_battery(true);
        assert!(!tank.sway_active());
        let req = tank.tick(now + Duration::from_millis(50));
        if req.redraw {
            tank.mark_drawn();
        }
        now += Duration::from_millis(50);
        assert_eq!(
            count_frames(&mut tank, now, 5),
            0,
            "sway must draw nothing on battery"
        );
        now += Duration::from_secs(5);
        tank.set_on_battery(false);

        // In screensaver mode: 0 sway frames.
        tank.set_saver_mode(true);
        assert!(!tank.sway_active());
        let req = tank.tick(now + Duration::from_millis(50));
        if req.redraw {
            tank.mark_drawn();
        }
        now += Duration::from_millis(50);
        assert_eq!(
            count_frames(&mut tank, now, 5),
            0,
            "sway must draw nothing in saver mode"
        );
        now += Duration::from_secs(5);
        tank.set_saver_mode(false);

        // While hidden: 0 sway frames immediately (no rest-snap frame either).
        tank.set_hidden(true);
        assert!(!tank.sway_active());
        assert_eq!(
            count_frames(&mut tank, now, 5),
            0,
            "sway must draw nothing while hidden"
        );
    }

    #[test]
    fn sway_draws_nothing_without_plants() {
        let world = World::new(
            WorldConfig {
                width: 64,
                height: 48,
                seed: 7,
                water_level: 0.9,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let mut tank = Tank::new(world, TankConfig::default());
        let req = tank.tick(Duration::ZERO);
        assert!(req.redraw);
        tank.mark_drawn();
        tank.set_sway(true);
        assert!(!tank.sway_active());
        assert_eq!(count_frames(&mut tank, Duration::ZERO, 5), 0);
    }
}
