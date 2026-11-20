//! A presentable surface with its configuration. The host creates the
//! `wgpu::Surface` (from a winit window, a canvas, a saver view); this only
//! configures and presents it.

use thiserror::Error;

use crate::{DrawError, Gpu};

/// Error configuring a surface.
#[derive(Debug, Error)]
pub enum SurfaceError {
    /// The adapter can't present to this surface.
    #[error("the GPU adapter cannot present to this surface")]
    Unsupported,
}

/// A configured surface.
#[derive(Debug)]
pub struct TankSurface {
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) config: wgpu::SurfaceConfiguration,
}

impl TankSurface {
    /// Configures `surface` for `width`×`height` pixels: sRGB if available,
    /// vsynced, opaque, at most one frame queued (frames are rare, latency
    /// matters more than throughput).
    ///
    /// # Errors
    ///
    /// Returns an error if the adapter can't present to the surface.
    pub fn new(
        gpu: &Gpu,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Result<Self, SurfaceError> {
        let caps = surface.get_capabilities(&gpu.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| caps.formats.first().copied())
            .ok_or(SurfaceError::Unsupported)?;
        let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            caps.alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto)
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 1,
            alpha_mode,
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
        };
        surface.configure(&gpu.device, &config);
        Ok(Self { surface, config })
    }

    /// Reconfigures for a new size in pixels. A zero side (minimised) is
    /// ignored; the host should stop drawing instead.
    pub fn resize(&mut self, gpu: &Gpu, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if (self.config.width, self.config.height) != (width, height) {
            self.config.width = width;
            self.config.height = height;
            self.reconfigure(gpu);
        }
    }

    /// Size in pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// The colour format frames are drawn in.
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Fills the surface with one colour and presents it (a host's
    /// placeholder frame, e.g. the Android skeleton's). `colour` is in the
    /// surface format's space: linear for an sRGB format. Returns whether a
    /// frame was presented; a lost, outdated or busy surface skips it, as
    /// [`crate::Tank::draw`] does.
    ///
    /// # Errors
    ///
    /// Returns an error if the surface texture fails validation.
    pub fn clear(&self, gpu: &Gpu, colour: wgpu::Color) -> Result<bool, DrawError> {
        let Some((frame, suboptimal)) = self.acquire(gpu)? else {
            return Ok(false);
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("clear"),
            });
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(colour),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        gpu.queue.submit([encoder.finish()]);
        gpu.queue.present(frame);
        if suboptimal {
            self.reconfigure(gpu);
        }
        Ok(true)
    }

    /// The next frame to draw into and whether the surface wants
    /// reconfiguring after it is presented, or `None` to skip this frame
    /// (timeout, occluded, or outdated or lost and now reconfigured).
    pub(crate) fn acquire(
        &self,
        gpu: &Gpu,
    ) -> Result<Option<(wgpu::SurfaceTexture, bool)>, DrawError> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => Ok(Some((f, false))),
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => Ok(Some((f, true))),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.reconfigure(gpu);
                Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Validation => Err(DrawError::Validation),
        }
    }

    pub(crate) fn reconfigure(&self, gpu: &Gpu) {
        self.surface.configure(&gpu.device, &self.config);
    }
}
