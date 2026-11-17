//! Offscreen rendering with pixel readback, for `desktank capture` and the
//! render tests.

use thiserror::Error;

use crate::{Gpu, Tank};

/// Error reading a frame back from the GPU.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// The size is zero or larger than the device allows.
    #[error("capture size {0}x{1} is outside the device's texture limits")]
    BadSize(u32, u32),
    /// Mapping the readback buffer failed.
    #[error("could not map the readback buffer: {0}")]
    Map(#[from] wgpu::BufferAsyncError),
    /// Reading the mapped buffer failed.
    #[error("could not read the readback buffer: {0}")]
    Range(#[from] wgpu::MapRangeError),
    /// Waiting for the GPU failed.
    #[error("waiting for the GPU failed: {0}")]
    Poll(#[from] wgpu::PollError),
}

/// Pixel format of captures: 8-bit RGBA holding sRGB-encoded values, as
/// written to PNG.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Tank {
    /// Draws a frame into a fresh offscreen `width`×`height` target and
    /// returns its pixels as tightly packed RGBA rows, top row first.
    ///
    /// # Errors
    ///
    /// Returns an error if the size is out of range or the readback fails.
    pub fn capture(
        &mut self,
        gpu: &Gpu,
        (width, height): (u32, u32),
    ) -> Result<Vec<u8>, CaptureError> {
        let limits = gpu.device.limits();
        let max = limits.max_texture_dimension_2d;
        let padded = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        if width == 0
            || height == 0
            || width > max
            || height > max
            || u64::from(padded) * u64::from(height) > limits.max_buffer_size
        {
            return Err(CaptureError::BadSize(width, height));
        }
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        self.render_to(gpu, &view, FORMAT, (width, height));

        let row = width * 4;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture readback"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: None,
                },
            },
            size,
        );
        gpu.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
        // The callback has run once the wait returns; a missing result means
        // the buffer was dropped, which can't happen while we hold it.
        rx.recv().unwrap_or(Err(wgpu::BufferAsyncError))?;
        let data = buffer.get_mapped_range(..)?;
        Ok(data
            .chunks(padded as usize)
            .flat_map(|r| &r[..row as usize])
            .copied()
            .collect())
    }
}
