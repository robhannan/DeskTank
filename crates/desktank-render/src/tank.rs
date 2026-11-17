//! The loop-agnostic tank: a world, its pacing and its renderer (SPEC §4.2).

use core::time::Duration;

use desktank_sim::{TICKS_PER_SECOND, World};
use thiserror::Error;

use crate::grid::DirtyRects;
use crate::hud::{self, BrushPreview, Palette, PaletteLayout};
use crate::layout::Layout;
use crate::pacing::{FrameRequest, Pacer};
use crate::renderer::Renderer;
use crate::timer::GpuTimer;
use crate::{Gpu, TankSurface};

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
        }
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
        let wanted = self.frame_wanted();
        let redraw = wanted && self.pacer.frame_allowed(now);
        if redraw {
            self.pacer.frame_drawn(now);
        }
        FrameRequest {
            redraw,
            wake_at: self.pacer.wake_at(now, wanted && !redraw),
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
    }

    /// Stops world time, e.g. while the surface is hidden. The next
    /// [`Tank::tick`] restarts the clock without catching up.
    pub fn pause(&mut self) {
        self.pacer.pause();
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

    /// Whether a frame is waiting: the world changed since the last frame,
    /// or the target was invalidated.
    #[must_use]
    pub fn frame_wanted(&self) -> bool {
        self.needs_frame || !self.dirty.is_empty()
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
        let frame = match surface.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                // Draw this one; reconfigure for the next.
                let drawn = self.present(gpu, surface, f);
                surface.reconfigure(gpu);
                return Ok(drawn);
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                surface.reconfigure(gpu);
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Validation => return Err(DrawError::Validation),
        };
        Ok(self.present(gpu, surface, frame))
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
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        renderer.upload(gpu, &self.world, &mut self.dirty);
        if self.palette_dirty {
            if let Some(p) = &self.palette {
                let (image, image_size) = hud::build(p, self.world.materials());
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
            (palette, self.cursor),
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
}
